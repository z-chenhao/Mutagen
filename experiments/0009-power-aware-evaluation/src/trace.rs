//! Artifact types, design constants, summaries, verifiers, and the
//! deterministic network-free synthetic datasets (for `self-test` and
//! verifier regression tests).
//!
//! The kernel is the authoritative execution recorder: every execution
//! field of a record originates from the Rust kernel's own in-episode
//! records. Raw artifacts are never reconstructed from server logs.
//!
//! Experiment 0009 has two artifacts with two record shapes:
//!
//! - Phase A (calibration): `CalibrationRecord` — baseline-only episodes
//!   over the three registered families × two development tasks ×
//!   five stress levels × five repetitions (150 episodes), in the
//!   registered cyclic stress order. The repair candidate is never
//!   executed in Phase A (candidate blindness).
//! - Phase B (evaluation): `EvaluationRecord` — baseline vs repair on
//!   the selected families' held-out tasks (96 episodes, 48 pairs),
//!   in the registered alternating condition order.
//!
//! The design constants (episode counts, orders, gates) are hardcoded
//! here and are NEVER inferred from artifacts (the expected Phase B
//! size is derived from the committed evaluation plan, spec §51).

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;

use crate::calibration::EvaluationPlan;
use crate::oracle::{self, TaskSpec};
use crate::power;
use crate::protocol::{TEMPERATURE, canonical_json};
use crate::stats;
use crate::tools::WriteAttempt;

// ===========================================================================
// Design constants (hardcoded, never inferred from artifacts)
// ===========================================================================

pub const EXPERIMENT_ID: &str = "0009";

/// The three registered structural families (spec §15), in registry order.
pub const FAMILIES: [&str; 3] = ["direct_set", "conditional_set", "replacement"];

/// Registered development tasks per family (spec §15).
pub const DEVELOPMENT_TASKS_PER_FAMILY: usize = 2;
/// Registered held-out tasks per family (spec §15).
pub const HELDOUT_TASKS_PER_FAMILY: usize = 2;

/// Phase A: 3 families × 2 development tasks × 5 stress levels ×
/// 5 repetitions = 150 episodes (spec §19).
pub const CALIBRATION_STRESS_LEVELS: u32 = 5;
pub const CALIBRATION_REPETITIONS: u32 = 5;
pub const CALIBRATION_FAMILIES: u32 = 3;
pub const CALIBRATION_EPISODES: usize = (CALIBRATION_FAMILIES
    * DEVELOPMENT_TASKS_PER_FAMILY as u32
    * CALIBRATION_STRESS_LEVELS
    * CALIBRATION_REPETITIONS) as usize; // 150

/// Phase B conditions (spec §37): baseline vs repair.
pub const CONDITIONS: [&str; 2] = ["baseline", "repair"];

/// Fixed Phase B budgets (spec §26–27): 48 paired comparisons →
/// 96 episodes, regardless of whether two or three families calibrate.
pub const PLANNED_PAIRS: u32 = power::PLANNED_PAIRS;
pub const EVAL_EPISODES: usize = (PLANNED_PAIRS * CONDITIONS.len() as u32) as usize; // 96

/// Registered alternating condition order per repetition (spec §28):
/// odd repetitions run baseline → repair, even repetitions run
/// repair → baseline, so each condition occurs first exactly half
/// the time.
pub fn eval_condition_order(repetition: u32) -> [&'static str; 2] {
    if repetition % 2 == 1 {
        ["baseline", "repair"]
    } else {
        ["repair", "baseline"]
    }
}

/// Registered cyclic stress order per (repetition, execution position)
/// (spec §20): every stress level occurs exactly once in every
/// execution position across the five repetitions.
pub fn calibration_stress_index(repetition: u32, execution_position: u32) -> u32 {
    (repetition - 1 + execution_position - 1) % CALIBRATION_STRESS_LEVELS
}

/// S0 sanity gate (spec §21): each development task must show at
/// least 4/5 reliable baseline success; BOTH must pass.
pub const S0_MIN_SUCCESS_PER_TASK: u32 = 4;

/// Power-aware failure-headroom eligibility (spec §22): a stress is
/// eligible iff every development task shows baseline success ≤ 2/5
/// (≤ 40%). There is deliberately no lower bound: 0/5 is eligible.
pub const MAX_DEV_SUCCESS_FOR_ELIGIBILITY: u32 = 2;

/// Minimum calibrated families to proceed to Phase B (spec §25).
pub const MIN_CALIBRATED_FAMILIES: u32 = 2;

/// Phase B repetitions per held-out task, derived from the calibrated
/// family count (spec §27): 4 held-out tasks × 12 = 48 pairs, or
/// 6 held-out tasks × 8 = 48 pairs. Never anything else.
pub fn eval_repetitions_for(calibrated_family_count: u32) -> Option<u32> {
    match calibrated_family_count {
        2 => Some(12),
        3 => Some(8),
        _ => None,
    }
}

/// Infrastructure-failure threshold above which the experiment is
/// inconclusive (spec §36): strictly more than 10% of all episodes.
pub const INFRA_FAILURE_RATE_THRESHOLD: f64 = 0.10;

/// Minimum valid held-out pairs (spec §35): ≥ 44 of the planned 48.
pub const MIN_VALID_PAIRS: u32 = power::MIN_VALID_PAIRS;

/// Minimum potential information capacity (baseline failures among
/// valid pairs) and minimum actual informative pairs (spec §33–34).
pub const MIN_POTENTIAL_INFORMATION: u32 = power::MIN_INFORMATIVE_PAIRS;
pub const MIN_ACTUAL_INFORMATIVE: u32 = power::MIN_INFORMATIVE_PAIRS;

// ===========================================================================
// Termination / failure classification
// ===========================================================================

/// Terminal reason for one episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationReason {
    Completed,
    // Agent failures (quality evidence; ordinary task failures):
    TurnLimit,
    ToolCallLimit,
    ParallelToolCallsUnsupported,
    UnknownTool,
    InvalidArguments,
    NoFinalAnswer,
    // Infrastructure failures (excludable from pairs, recorded separately):
    MalformedResponse,
    HttpError,
}

impl TerminationReason {
    pub fn as_str(self) -> &'static str {
        match self {
            TerminationReason::Completed => "completed",
            TerminationReason::TurnLimit => "turn_limit",
            TerminationReason::ToolCallLimit => "tool_call_limit",
            TerminationReason::ParallelToolCallsUnsupported => "parallel_tool_calls_unsupported",
            TerminationReason::UnknownTool => "unknown_tool",
            TerminationReason::InvalidArguments => "invalid_arguments",
            TerminationReason::NoFinalAnswer => "no_final_answer",
            TerminationReason::MalformedResponse => "malformed_response",
            TerminationReason::HttpError => "http_error",
        }
    }

    /// Agent-caused failures count as ordinary task failures and remain
    /// in the paired comparison.
    pub fn is_agent_failure(self) -> bool {
        matches!(
            self,
            TerminationReason::TurnLimit
                | TerminationReason::ToolCallLimit
                | TerminationReason::ParallelToolCallsUnsupported
                | TerminationReason::UnknownTool
                | TerminationReason::InvalidArguments
                | TerminationReason::NoFinalAnswer
        )
    }

    /// Infrastructure failures may be excluded from statistical pairs.
    pub fn is_infrastructure(self) -> bool {
        matches!(
            self,
            TerminationReason::MalformedResponse | TerminationReason::HttpError
        )
    }

    pub fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(Value::String(s.to_string())).ok()
    }
}

// ===========================================================================
// Shared per-episode execution records
// ===========================================================================

/// Per-request usage. Every field is `Option`: `null` when the provider
/// did not report it (never inferred, never zero by assumption).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelRequestRecord {
    pub turn: u32,
    pub finish_reason: Option<String>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    /// `usage.prompt_tokens_details.cached_tokens`, when reported.
    pub cached_prompt_tokens: Option<u64>,
    /// `usage.completion_tokens_details.reasoning_tokens` (a count,
    /// never reasoning content), when reported.
    pub reasoning_tokens: Option<u64>,
    /// `prompt_tokens - cached_prompt_tokens` when both reported;
    /// `null` otherwise.
    pub uncached_prompt_tokens: Option<u64>,
}

/// A tool call *requested* by the model (includes rejected parallel calls).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestedToolCall {
    pub sequence: u32,
    pub turn: u32,
    pub tool_call_id: String,
    pub tool_name: String,
    pub arguments: Value,
}

/// A tool call that was *executed* against the external state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutedToolCall {
    pub sequence: u32,
    pub turn: u32,
    pub tool_call_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub result: Value,
    pub duration_us: u64,
}

/// Wall-clock measurements for one episode. All labeled approximate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Timing {
    pub wall_time_ms: u64,
    pub model_wait_time_ms: u64,
    pub tool_execution_time_us: u64,
    pub kernel_overhead_estimate_ms: u64,
    #[serde(default = "true_fn")]
    pub approximate: bool,
}

fn true_fn() -> bool {
    true
}

/// The kernel-recorded execution core of one episode, shared by both
/// record shapes. See `crate::kernel::RecordCore`.
pub use crate::kernel::RecordCore as ExecutionCore;

// ===========================================================================
// Phase A record (calibration) — spec §44
// ===========================================================================

/// One persisted calibration trajectory (a JSONL line).
///
/// Candidate blindness (spec §5): `condition` is always `"baseline"`
/// and no candidate outcome/trajectory/cost field exists. The repair
/// prompt hash is frozen *configuration* provenance committed before
/// Phase A; it never influences selection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationRecord {
    pub experiment_id: String,
    pub phase: String,
    pub run_id: String,
    /// 1-based position of this episode in the run (strict sequence).
    pub sequence: u32,
    pub family: String,
    pub task_id: String,
    pub stress_level: String,
    pub drop_count: u32,
    pub repetition: u32,
    pub execution_position: u32,
    /// Always "baseline": the repair candidate is never executed in
    /// Phase A.
    pub condition: String,
    pub model: String,
    pub temperature: f64,
    pub redacted_endpoint: String,
    pub code_under_test_commit: String,
    pub baseline_prompt_sha256: String,
    /// Frozen configuration provenance only (spec §5/§44).
    pub repair_prompt_sha256: String,
    pub task_registry_sha256: String,
    pub stress_registry_sha256: String,
    pub initial_state: Value,
    pub target_state: Value,
    pub fault_mode: Value,
    pub model_requests: Vec<ModelRequestRecord>,
    pub requested_tool_calls: Vec<RequestedToolCall>,
    pub executed_tool_calls: Vec<ExecutedToolCall>,
    pub environment_write_attempts: Vec<WriteAttempt>,
    pub final_state: Value,
    pub final_answer: Option<String>,
    pub model_turn_count: u32,
    pub tool_call_count: u32,
    pub agent_failure: Option<String>,
    pub infrastructure_failure: Option<String>,
    pub usage_accounting_error: Option<String>,
    pub termination_reason: String,
    pub oracle_success: bool,
    pub oracle_failure_reasons: Vec<String>,
    pub timing: Timing,
}

// ===========================================================================
// Phase B record (evaluation) — spec §50
// ===========================================================================

/// One persisted held-out evaluation trajectory (a JSONL line).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationRecord {
    pub experiment_id: String,
    pub phase: String,
    pub run_id: String,
    /// 1-based position of this episode in the run (strict sequence).
    pub sequence: u32,
    pub family: String,
    pub task_id: String,
    pub selected_stress: String,
    pub drop_count: u32,
    pub condition: String,
    pub repetition: u32,
    pub condition_position: u32,
    pub calibration_raw_sha256: String,
    pub evaluation_plan_sha256: String,
    pub model: String,
    pub temperature: f64,
    pub redacted_endpoint: String,
    pub final_eval_commit: String,
    pub baseline_prompt_sha256: String,
    pub repair_prompt_sha256: String,
    pub task_registry_sha256: String,
    pub initial_state: Value,
    pub target_state: Value,
    pub fault_mode: Value,
    pub model_requests: Vec<ModelRequestRecord>,
    pub requested_tool_calls: Vec<RequestedToolCall>,
    pub executed_tool_calls: Vec<ExecutedToolCall>,
    pub environment_write_attempts: Vec<WriteAttempt>,
    pub final_state: Value,
    pub final_answer: Option<String>,
    pub model_turn_count: u32,
    pub tool_call_count: u32,
    pub agent_failure: Option<String>,
    pub infrastructure_failure: Option<String>,
    pub usage_accounting_error: Option<String>,
    pub termination_reason: String,
    pub oracle_success: bool,
    pub oracle_failure_reasons: Vec<String>,
    pub timing: Timing,
}

// ===========================================================================
// Summary types
// ===========================================================================

/// Aggregated cost for one population. Failed episodes consume
/// resources and are never omitted from their population. Token fields
/// are `null` (never zero) when no request reported the field.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CostBlock {
    pub episodes: u32,
    pub oracle_successes: u32,
    pub agent_failures: u32,
    pub infrastructure_failures: u32,
    pub model_requests: u32,
    pub executed_tool_calls: u32,
    pub nominal_prompt_tokens: Option<u64>,
    pub cached_prompt_tokens: Option<u64>,
    pub uncached_prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    /// Number of model requests whose provider usage reported
    /// `prompt_tokens` (the population of the token sums above).
    pub usage_reporting_requests: u32,
    pub wall_time_ms: u64,
    pub model_wait_time_ms: u64,
    pub tool_execution_time_us: u64,
}

/// Cache accounting for a population. `cache_hit_ratio` is
/// `cached / nominal` when both are reported; `null` otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CacheMetrics {
    pub nominal_prompt_tokens: Option<u64>,
    pub cached_prompt_tokens: Option<u64>,
    pub uncached_prompt_tokens: Option<u64>,
    pub cache_hit_ratio: Option<f64>,
}

/// Phase A: one row of the cache audit, stress level × execution
/// position (spec §55).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StressPositionCacheRow {
    pub stress_level: String,
    pub execution_position: u32,
    pub episodes: u32,
    pub nominal_prompt_tokens: Option<u64>,
    pub cached_prompt_tokens: Option<u64>,
    pub uncached_prompt_tokens: Option<u64>,
    pub cache_hit_ratio: Option<f64>,
}

/// Phase B: one row of the cache audit, condition × condition position
/// (spec §55).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionPositionCacheRow {
    pub condition: String,
    pub condition_position: u32,
    pub episodes: u32,
    pub nominal_prompt_tokens: Option<u64>,
    pub cached_prompt_tokens: Option<u64>,
    pub uncached_prompt_tokens: Option<u64>,
    pub cache_hit_ratio: Option<f64>,
}

/// Per-condition cost across the registered populations. The
/// `failure` block *is* the failure resource consumption.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionSummary {
    pub condition: String,
    pub all: CostBlock,
    pub success: CostBlock,
    pub failure: CostBlock,
    pub cache: CacheMetrics,
}

/// The baseline/repair comparison on the pooled held-out pairs of the
/// selected families, paired by (task_id, repetition) after
/// infrastructure exclusions (spec §37).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairComparison {
    pub baseline: String,
    pub candidate: String,
    pub selected_families: Vec<String>,
    pub expected_pairs: u32,
    pub valid_pairs: u32,
    pub excluded_infrastructure_pairs: u32,
    pub missing_pairs: u32,
    /// wins + losses (discordant oracle-success status; the only
    /// accepted definition of "informative", spec §34).
    pub informative_pairs: u32,
    pub wins: u32,
    pub losses: u32,
    pub ties: u32,
    pub sign_test_p: f64,
    pub classification: String,
}

/// Potential information capacity (spec §32): the number of valid pairs
/// whose baseline episode failed — the maximum possible number of wins
/// for an ideal no-harm repair, computed from the baseline alone.
/// Candidate-independent by construction.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PotentialInformationGate {
    pub valid_pairs: u32,
    pub baseline_successes_in_valid_pairs: u32,
    pub baseline_failures_in_valid_pairs: u32,
    pub potential_information_capacity: u32,
    pub sufficient: bool,
}

/// Per-family diagnostic (spec §42, §43): reported, but never the basis
/// of the primary statistical claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FamilyDiagnostic {
    pub family: String,
    pub selected_stress: String,
    pub heldout_tasks: Vec<String>,
    pub baseline_successes: u32,
    pub repair_successes: u32,
    pub wins: u32,
    pub losses: u32,
    pub ties: u32,
    pub informative_pairs: u32,
    /// Max of the two development variants' baseline success rates at
    /// the selected stress (from the frozen plan; 5 reps each).
    pub development_max_success_rate_at_selected_stress: f64,
    /// Held-out baseline success rate at the selected stress.
    pub heldout_baseline_success_rate: f64,
    /// heldout_baseline_rate − development_max_rate (descriptive only;
    /// spec §43: no separate transfer threshold decides the verdict).
    pub transfer_delta: f64,
}

/// Pre-registered information-availability gates (spec §40).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Gates {
    pub phase_a_complete: bool,
    pub calibrated_families_sufficient: bool,
    pub phase_b_artifact_complete: bool,
    pub infrastructure_within_threshold: bool,
    pub valid_pairs_sufficient: bool,
    pub potential_information_sufficient: bool,
    pub actual_informative_sufficient: bool,
}

/// Pre-registered experiment-level conclusion (spec §40): the three-way
/// distinction between "not enough opportunity to detect", "enough
/// opportunity but no improvement", and "enough opportunity and a
/// significant improvement".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conclusion {
    /// "supported" | "refuted" | "inconclusive"
    pub result: String,
    pub reasons: Vec<String>,
}

// ===========================================================================
// Phase A summary
// ===========================================================================

/// One family's calibration result, recomputable from the raw records
/// through the pure `calibration::select_family_stress` rule.
pub type FamilyCalibrationSummary = crate::calibration::FamilySelection;

/// One persisted calibration summary (regenerable from raw records).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationSummary {
    pub experiment_id: String,
    pub phase: String,
    pub model: String,
    pub redacted_endpoint: String,
    pub temperature: f64,
    pub code_under_test_commit: String,
    pub baseline_prompt_sha256: String,
    pub repair_prompt_sha256: String,
    pub task_registry_sha256: String,
    pub stress_registry_sha256: String,
    pub episodes_expected: u32,
    pub episodes_total: u32,
    pub artifacts_complete: bool,
    pub oracle_successes: u32,
    pub agent_failures: u32,
    pub infrastructure_failures: u32,
    pub infrastructure_failure_rate: f64,
    pub agent_failure_breakdown: BTreeMap<String, u32>,
    pub infrastructure_failure_breakdown: BTreeMap<String, u32>,
    pub per_family: Vec<FamilyCalibrationSummary>,
    pub calibrated_family_count: u32,
    /// Machine-derived Phase B planning (spec §27, §45).
    pub planned_pairs: u32,
    pub repetitions_per_task: u32,
    pub proceed_to_evaluation: bool,
    pub cache_overall: CacheMetrics,
    pub stress_position_cache_audit: Vec<StressPositionCacheRow>,
}

// ===========================================================================
// Phase B summary
// ===========================================================================

/// One persisted evaluation summary (regenerable from the immutable raw
/// artifact plus the committed evaluation plan).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationSummary {
    pub experiment_id: String,
    pub phase: String,
    pub model: String,
    pub redacted_endpoint: String,
    pub temperature: f64,
    pub final_eval_commit: String,
    pub baseline_prompt_sha256: String,
    pub repair_prompt_sha256: String,
    pub task_registry_sha256: String,
    pub calibration_raw_sha256: String,
    pub evaluation_plan_sha256: String,
    pub selected_families: Vec<String>,
    pub heldout_tasks: Vec<String>,
    pub repetitions_per_task: u32,
    pub episodes_expected: u32,
    pub episodes_total: u32,
    pub artifacts_complete: bool,
    pub oracle_successes: u32,
    pub agent_failures: u32,
    pub infrastructure_failures: u32,
    pub infrastructure_failure_rate: f64,
    pub agent_failure_breakdown: BTreeMap<String, u32>,
    pub infrastructure_failure_breakdown: BTreeMap<String, u32>,
    pub potential_information: PotentialInformationGate,
    pub comparison: PairComparison,
    pub per_family_diagnostics: Vec<FamilyDiagnostic>,
    pub conditions: Vec<ConditionSummary>,
    pub cache_overall: CacheMetrics,
    pub condition_position_cache_audit: Vec<ConditionPositionCacheRow>,
    pub gates: Gates,
    pub conclusion: Conclusion,
}

// ===========================================================================
// Aggregation helpers
// ===========================================================================

#[derive(Default)]
struct TokenAcc {
    sum: u64,
    count: u32,
}

impl TokenAcc {
    fn add(&mut self, v: Option<u64>) {
        if let Some(x) = v {
            self.sum += x;
            self.count += 1;
        }
    }

    fn finish(self) -> Option<u64> {
        (self.count > 0).then_some(self.sum)
    }
}

#[derive(Default)]
struct CostAccum {
    episodes: u32,
    successes: u32,
    agent_failures: u32,
    infra_failures: u32,
    requests: u32,
    calls: u32,
    nominal: TokenAcc,
    cached: TokenAcc,
    uncached: TokenAcc,
    completion: TokenAcc,
    reasoning: TokenAcc,
    usage_reporting: u32,
    wall_ms: u64,
    model_wait_ms: u64,
    tool_exec_us: u64,
}

impl CostAccum {
    fn add_core(&mut self, success: bool, core: &ExecutionCore) {
        self.episodes += 1;
        if success {
            self.successes += 1;
        }
        if core.agent_failure.is_some() {
            self.agent_failures += 1;
        }
        if core.infrastructure_failure.is_some() {
            self.infra_failures += 1;
        }
        self.requests += core.model_requests.len() as u32;
        self.calls += core.executed_tool_calls.len() as u32;
        for q in &core.model_requests {
            self.nominal.add(q.prompt_tokens);
            self.cached.add(q.cached_prompt_tokens);
            self.uncached.add(q.uncached_prompt_tokens);
            self.completion.add(q.completion_tokens);
            self.reasoning.add(q.reasoning_tokens);
            if q.prompt_tokens.is_some() {
                self.usage_reporting += 1;
            }
        }
        self.wall_ms += core.timing.wall_time_ms;
        self.model_wait_ms += core.timing.model_wait_time_ms;
        self.tool_exec_us += core.timing.tool_execution_time_us;
    }

    fn finish(self) -> CostBlock {
        CostBlock {
            episodes: self.episodes,
            oracle_successes: self.successes,
            agent_failures: self.agent_failures,
            infrastructure_failures: self.infra_failures,
            model_requests: self.requests,
            executed_tool_calls: self.calls,
            nominal_prompt_tokens: self.nominal.finish(),
            cached_prompt_tokens: self.cached.finish(),
            uncached_prompt_tokens: self.uncached.finish(),
            completion_tokens: self.completion.finish(),
            reasoning_tokens: self.reasoning.finish(),
            usage_reporting_requests: self.usage_reporting,
            wall_time_ms: self.wall_ms,
            model_wait_time_ms: self.model_wait_ms,
            tool_execution_time_us: self.tool_exec_us,
        }
    }
}

fn cache_metrics_core(
    rows: &[(bool, ExecutionCore)],
    keep: impl Fn(&(bool, ExecutionCore)) -> bool,
) -> CacheMetrics {
    let mut nominal = TokenAcc::default();
    let mut cached = TokenAcc::default();
    let mut uncached = TokenAcc::default();
    for r in rows.iter().filter(|r| keep(r)) {
        for q in &r.1.model_requests {
            nominal.add(q.prompt_tokens);
            cached.add(q.cached_prompt_tokens);
            uncached.add(q.uncached_prompt_tokens);
        }
    }
    let n = nominal.finish();
    let c = cached.finish();
    let u = uncached.finish();
    let ratio = match (n, c) {
        (Some(n), Some(c)) if n > 0 => Some(c as f64 / n as f64),
        _ => None,
    };
    CacheMetrics {
        nominal_prompt_tokens: n,
        cached_prompt_tokens: c,
        uncached_prompt_tokens: u,
        cache_hit_ratio: ratio,
    }
}

/// One (repair, baseline) pair outcome, from the candidate's
/// perspective (spec §37).
fn pair_outcome(candidate_success: bool, baseline_success: bool) -> &'static str {
    match (candidate_success, baseline_success) {
        (true, false) => "win",
        (false, true) => "loss",
        _ => "tie",
    }
}

/// The kernel-recorded execution core of a calibration record.
pub fn calibration_core(r: &CalibrationRecord) -> ExecutionCore {
    ExecutionCore {
        termination_reason: r.termination_reason.clone(),
        final_answer: r.final_answer.clone(),
        model_requests: r.model_requests.clone(),
        requested_tool_calls: r.requested_tool_calls.clone(),
        executed_tool_calls: r.executed_tool_calls.clone(),
        write_attempts: r.environment_write_attempts.clone(),
        final_state: r.final_state.clone(),
        model_turn_count: r.model_turn_count,
        tool_call_count: r.tool_call_count,
        agent_failure: r.agent_failure.clone(),
        infrastructure_failure: r.infrastructure_failure.clone(),
        usage_accounting_error: r.usage_accounting_error.clone(),
        timing: r.timing,
    }
}

/// The kernel-recorded execution core of an evaluation record.
pub fn evaluation_core(r: &EvaluationRecord) -> ExecutionCore {
    ExecutionCore {
        termination_reason: r.termination_reason.clone(),
        final_answer: r.final_answer.clone(),
        model_requests: r.model_requests.clone(),
        requested_tool_calls: r.requested_tool_calls.clone(),
        executed_tool_calls: r.executed_tool_calls.clone(),
        write_attempts: r.environment_write_attempts.clone(),
        final_state: r.final_state.clone(),
        model_turn_count: r.model_turn_count,
        tool_call_count: r.tool_call_count,
        agent_failure: r.agent_failure.clone(),
        infrastructure_failure: r.infrastructure_failure.clone(),
        usage_accounting_error: r.usage_accounting_error.clone(),
        timing: r.timing,
    }
}
// ===========================================================================
// Fault-schedule audit
// ===========================================================================

/// Validate one record's write-attempt audit against the registered
/// fault mode. Returns a violation message or `None` when the schedule
/// executed exactly as registered:
///
/// - `reliable`: every write applied cleanly, no fault reasons, no
///   drop indices.
/// - `drop_first_n_writes(key, count)` (count ∈ 1..4): the first `count`
///   writes to the target key were silently dropped (with the registered
///   fault reason); target-key writes beyond `count` applied; all
///   other-key writes applied; `registered_drop_index` is the 1-based
///   target-write ordinal for target-key writes and `null` otherwise.
pub fn audit_record(run_id: &str, fault_mode: &Value, attempts: &[WriteAttempt]) -> Option<String> {
    let mode = fault_mode.get("mode").and_then(Value::as_str)?;
    match mode {
        "reliable" => {
            for a in attempts {
                if !a.applied || a.fault_reason.is_some() || a.registered_drop_index.is_some() {
                    return Some(format!(
                        "{run_id}: reliable mode: write attempt {} not a clean apply",
                        a.sequence
                    ));
                }
            }
            None
        }
        "drop_first_n_writes" => {
            let key = fault_mode
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let count = fault_mode.get("count").and_then(Value::as_u64).unwrap_or(0) as u32;
            if !(1..=4).contains(&count) {
                return Some(format!(
                    "{run_id}: registered drop count {count} outside the registered ladder 1..4"
                ));
            }
            let mut ordinal = 0u32;
            for a in attempts {
                if a.key == key {
                    ordinal += 1;
                    let expected_drop = ordinal <= count;
                    if a.registered_drop_index != Some(ordinal) {
                        return Some(format!(
                            "{run_id}: write attempt {} to {key}: registered_drop_index {:?} != ordinal {ordinal}",
                            a.sequence, a.registered_drop_index
                        ));
                    }
                    if expected_drop && a.applied {
                        return Some(format!(
                            "{run_id}: write attempt {} ({ordinal}-th to {key}) was applied (registered drop #{ordinal})",
                            a.sequence
                        ));
                    }
                    if expected_drop && a.fault_reason.as_deref() != Some("drop_first_n_writes") {
                        return Some(format!(
                            "{run_id}: dropped write attempt {} missing fault_reason",
                            a.sequence
                        ));
                    }
                    if !expected_drop && (!a.applied || a.fault_reason.is_some()) {
                        return Some(format!(
                            "{run_id}: write attempt {} ({ordinal}-th to {key}, beyond the drop window) did not apply cleanly",
                            a.sequence
                        ));
                    }
                } else {
                    if !a.applied || a.fault_reason.is_some() {
                        return Some(format!(
                            "{run_id}: unregistered key {} write attempt {} did not apply cleanly",
                            a.key, a.sequence
                        ));
                    }
                    if a.registered_drop_index.is_some() {
                        return Some(format!(
                            "{run_id}: non-target-key write attempt {} carries a registered_drop_index",
                            a.sequence
                        ));
                    }
                }
            }
            None
        }
        other => Some(format!("{run_id}: unknown fault mode {other:?}")),
    }
}

// ===========================================================================
// Phase A summary / plan computation
// ===========================================================================

fn development_tasks_for(family: &str, registry: &[TaskSpec]) -> Vec<TaskSpec> {
    registry
        .iter()
        .filter(|t| t.family == family && t.split == oracle::TaskSplit::Development)
        .cloned()
        .collect()
}

fn heldout_tasks_for(family: &str, registry: &[TaskSpec]) -> Vec<TaskSpec> {
    registry
        .iter()
        .filter(|t| t.family == family && t.split == oracle::TaskSplit::HeldOut)
        .cloned()
        .collect()
}

/// Recompute the Phase A summary from raw records and the registry.
/// Selection is recomputed through the pure
/// `calibration::select_family_stress` — no candidate data is an input.
pub fn compute_calibration_summary(
    records: &[CalibrationRecord],
    registry: &[TaskSpec],
) -> CalibrationSummary {
    let first = records.first();
    let (model, endpoint, temp, commit, bh, rh, task_hash, stress_hash) = match first {
        Some(r) => (
            r.model.clone(),
            r.redacted_endpoint.clone(),
            r.temperature,
            r.code_under_test_commit.clone(),
            r.baseline_prompt_sha256.clone(),
            r.repair_prompt_sha256.clone(),
            r.task_registry_sha256.clone(),
            r.stress_registry_sha256.clone(),
        ),
        None => (
            String::new(),
            String::new(),
            0.0,
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        ),
    };

    let total = records.len() as u32;
    let infra: u32 = records
        .iter()
        .filter(|r| r.infrastructure_failure.is_some())
        .count() as u32;
    let agent: u32 = records.iter().filter(|r| r.agent_failure.is_some()).count() as u32;
    let successes: u32 = records.iter().filter(|r| r.oracle_success).count() as u32;
    let infra_rate = if total > 0 {
        infra as f64 / total as f64
    } else {
        0.0
    };

    let mut agent_breakdown: BTreeMap<String, u32> = BTreeMap::new();
    for r in records {
        if let Some(reason) = &r.agent_failure {
            *agent_breakdown.entry(reason.clone()).or_insert(0) += 1;
        }
    }
    let mut infra_breakdown: BTreeMap<String, u32> = BTreeMap::new();
    for r in records {
        if let Some(reason) = &r.infrastructure_failure {
            *infra_breakdown.entry(reason.clone()).or_insert(0) += 1;
        }
    }

    // Per family: the pre-registered pure selection rule over BOTH
    // development variants.
    let mut per_family: Vec<FamilyCalibrationSummary> = Vec::new();
    let mut calibrated = 0u32;
    for family in FAMILIES {
        let dev_ids: Vec<String> = development_tasks_for(family, registry)
            .iter()
            .map(|t| t.id.clone())
            .collect();
        let outcomes: Vec<crate::calibration::BaselineDevelopmentOutcome> = records
            .iter()
            .filter(|r| r.family == family)
            .map(|r| crate::calibration::BaselineDevelopmentOutcome {
                task_id: r.task_id.clone(),
                stress_id: r.stress_level.clone(),
                repetition: r.repetition,
                oracle_success: r.oracle_success,
            })
            .collect();
        let sel = crate::calibration::select_family_stress(family, &dev_ids, &outcomes);
        if sel.status == crate::calibration::SelectionStatus::Calibrated {
            calibrated += 1;
        }
        per_family.push(sel);
    }

    let complete = phase_a_artifact_complete(records, registry);
    let reps = eval_repetitions_for(calibrated).unwrap_or(0);

    let rows: Vec<(bool, ExecutionCore)> = records
        .iter()
        .map(|r| (r.oracle_success, calibration_core(r)))
        .collect();
    let cache_overall = cache_metrics_core(&rows, |_| true);
    let mut stress_position_cache_audit: Vec<StressPositionCacheRow> = Vec::new();
    for l in crate::tools::STRESS_LEVELS.iter() {
        for pos in 1..=CALIBRATION_STRESS_LEVELS {
            let mut nominal = TokenAcc::default();
            let mut cached = TokenAcc::default();
            let mut uncached = TokenAcc::default();
            let mut episodes = 0u32;
            for r in records
                .iter()
                .filter(|r| r.stress_level == l.id && r.execution_position == pos)
            {
                episodes += 1;
                for q in &r.model_requests {
                    nominal.add(q.prompt_tokens);
                    cached.add(q.cached_prompt_tokens);
                    uncached.add(q.uncached_prompt_tokens);
                }
            }
            let n = nominal.finish();
            let c = cached.finish();
            let u = uncached.finish();
            let ratio = match (n, c) {
                (Some(n), Some(c)) if n > 0 => Some(c as f64 / n as f64),
                _ => None,
            };
            stress_position_cache_audit.push(StressPositionCacheRow {
                stress_level: l.id.to_string(),
                execution_position: pos,
                episodes,
                nominal_prompt_tokens: n,
                cached_prompt_tokens: c,
                uncached_prompt_tokens: u,
                cache_hit_ratio: ratio,
            });
        }
    }

    CalibrationSummary {
        experiment_id: EXPERIMENT_ID.to_string(),
        phase: "calibration".to_string(),
        model,
        redacted_endpoint: endpoint,
        temperature: temp,
        code_under_test_commit: commit,
        baseline_prompt_sha256: bh,
        repair_prompt_sha256: rh,
        task_registry_sha256: task_hash,
        stress_registry_sha256: stress_hash,
        episodes_expected: CALIBRATION_EPISODES as u32,
        episodes_total: total,
        artifacts_complete: complete,
        oracle_successes: successes,
        agent_failures: agent,
        infrastructure_failures: infra,
        infrastructure_failure_rate: infra_rate,
        agent_failure_breakdown: agent_breakdown,
        infrastructure_failure_breakdown: infra_breakdown,
        per_family,
        calibrated_family_count: calibrated,
        planned_pairs: PLANNED_PAIRS,
        repetitions_per_task: reps,
        proceed_to_evaluation: calibrated >= MIN_CALIBRATED_FAMILIES,
        cache_overall,
        stress_position_cache_audit,
    }
}

// ===========================================================================
// Phase A / Phase B artifact completeness
// ===========================================================================

/// Phase A artifact completeness: exact count, exact full-factorial
/// tuple coverage, registered cyclic stress order, registry consistency,
/// baseline-only condition, fault audit, usage arithmetic, oracle
/// recomputation.
pub fn phase_a_artifact_complete(records: &[CalibrationRecord], registry: &[TaskSpec]) -> bool {
    if registry.len() != 12 || records.len() != CALIBRATION_EPISODES {
        return false;
    }
    let by_id: BTreeMap<&str, &TaskSpec> = registry.iter().map(|t| (t.id.as_str(), t)).collect();

    for (i, r) in records.iter().enumerate() {
        if r.sequence != (i + 1) as u32 {
            return false;
        }
        if !FAMILIES.contains(&r.family.as_str()) || r.condition != "baseline" {
            return false; // Phase A is baseline-only (spec §5, §19).
        }
        if !(1..=CALIBRATION_REPETITIONS).contains(&r.repetition)
            || !(1..=CALIBRATION_STRESS_LEVELS).contains(&r.execution_position)
        {
            return false;
        }
        match by_id.get(r.task_id.as_str()) {
            Some(t) => {
                if t.family != r.family || t.split != oracle::TaskSplit::Development {
                    return false;
                }
                if r.initial_state != t.initial_state || r.target_state != t.target_state {
                    return false;
                }
            }
            None => return false,
        }
        if audit_record(&r.run_id, &r.fault_mode, &r.environment_write_attempts).is_some() {
            return false;
        }
        if r.usage_accounting_error.is_some() {
            return false;
        }
        // Registered cyclic stress order (spec §20).
        let expected = crate::tools::STRESS_LEVELS
            [calibration_stress_index(r.repetition, r.execution_position) as usize];
        if r.stress_level != expected.id || r.drop_count != expected.drop_count {
            return false;
        }
    }

    let mut tuples: BTreeMap<(String, String, u32, u32), u32> = BTreeMap::new();
    for r in records {
        *tuples
            .entry((
                r.family.clone(),
                r.task_id.clone(),
                r.repetition,
                r.execution_position,
            ))
            .or_insert(0) += 1;
    }
    for f in FAMILIES {
        for t in registry
            .iter()
            .filter(|t| t.family == f && t.split == oracle::TaskSplit::Development)
        {
            for rep in 1..=CALIBRATION_REPETITIONS {
                for pos in 1..=CALIBRATION_STRESS_LEVELS {
                    if tuples.get(&(f.to_string(), t.id.clone(), rep, pos)) != Some(&1) {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// Phase B artifact completeness (spec §51–52): exact count derived
/// from the frozen plan, exact pair coverage, registered alternating
/// order, pair stress consistency, registry consistency, fault audit.
pub fn phase_b_artifact_complete(
    records: &[EvaluationRecord],
    registry: &[TaskSpec],
    plan: &EvaluationPlan,
    expected: usize,
) -> bool {
    if records.len() != expected || expected == 0 || registry.len() != 12 {
        return false;
    }
    let selected: BTreeSet<&str> = plan.selected_families.iter().map(String::as_str).collect();
    if selected.is_empty() || selected.len() > 3 {
        return false;
    }
    let reps = plan.repetitions_per_task;
    if reps == 0 {
        return false;
    }
    let by_id: BTreeMap<&str, &TaskSpec> = registry.iter().map(|t| (t.id.as_str(), t)).collect();

    for (i, r) in records.iter().enumerate() {
        if r.sequence != (i + 1) as u32 {
            return false;
        }
        if !CONDITIONS.contains(&r.condition.as_str())
            || !(1..=reps).contains(&r.repetition)
            || !(1..=2).contains(&r.condition_position)
        {
            return false;
        }
        let t = match by_id.get(r.task_id.as_str()) {
            Some(t) => t,
            None => return false,
        };
        if t.split != oracle::TaskSplit::HeldOut || !selected.contains(t.family.as_str()) {
            return false;
        }
        if t.family != r.family {
            return false;
        }
        let plan_stress = plan
            .per_family
            .iter()
            .find(|f| f.family == r.family)
            .and_then(|f| f.selected_stress.clone())
            .unwrap_or_default();
        if plan_stress != r.selected_stress {
            return false;
        }
        let level = match crate::tools::StressLevel::by_id(&r.selected_stress) {
            Some(l) => l,
            None => return false,
        };
        if r.drop_count != level.drop_count || level.drop_count == 0 {
            return false;
        }
        if audit_record(&r.run_id, &r.fault_mode, &r.environment_write_attempts).is_some() {
            return false;
        }
        if r.usage_accounting_error.is_some() {
            return false;
        }
    }

    // Full pair coverage for every selected held-out task.
    let mut tuples: BTreeSet<(String, String, u32)> = BTreeSet::new();
    for r in records {
        tuples.insert((r.task_id.clone(), r.condition.clone(), r.repetition));
    }
    for f in &selected {
        for t in registry
            .iter()
            .filter(|t| t.family == *f && t.split == oracle::TaskSplit::HeldOut)
        {
            for rep in 1..=reps {
                for c in CONDITIONS {
                    if !tuples.contains(&(t.id.clone(), c.to_string(), rep)) {
                        return false;
                    }
                }
            }
        }
    }
    // Registered alternating order + same stress within each pair.
    for t in registry
        .iter()
        .filter(|t| t.split == oracle::TaskSplit::HeldOut)
    {
        if !selected.contains(t.family.as_str()) {
            continue;
        }
        for rep in 1..=reps {
            let got: Vec<&str> = records
                .iter()
                .filter(|r| r.task_id == t.id && r.repetition == rep)
                .map(|r| r.condition.as_str())
                .collect();
            if got != eval_condition_order(rep).to_vec() {
                return false;
            }
            let stresses: BTreeSet<&str> = records
                .iter()
                .filter(|r| r.task_id == t.id && r.repetition == rep)
                .map(|r| r.selected_stress.as_str())
                .collect();
            if stresses.len() != 1 {
                return false;
            }
        }
    }
    true
}
// ===========================================================================
// Evaluation plan recompute (spec §45)
// ===========================================================================

/// Provenance of the evaluation plan: the code commit plus the frozen
/// hashes (spec §45). The repair prompt hash is a *configuration* hash
/// committed before calibration; the candidate was never executed in
/// Phase A, so the plan carries no candidate outcome data (spec §46).
#[derive(Debug, Clone, PartialEq)]
pub struct PlanProvenance {
    pub code_under_test_commit: String,
    pub baseline_prompt_sha256: String,
    pub repair_prompt_sha256: String,
    pub task_registry_sha256: String,
    pub stress_registry_sha256: String,
}

/// Recompute the evaluation plan from raw calibration records
/// (spec §45): everything is a deterministic function of the raw
/// calibration artifact + the frozen registries/constants. The caller
/// must fill `calibration_raw_sha256` from the artifact file's byte
/// hash before comparing.
pub fn recompute_evaluation_plan(
    records: &[CalibrationRecord],
    registry: &[TaskSpec],
    prov: &PlanProvenance,
) -> EvaluationPlan {
    let mut per_family = Vec::new();
    for family in FAMILIES {
        let dev_ids: Vec<String> = development_tasks_for(family, registry)
            .iter()
            .map(|t| t.id.clone())
            .collect();
        let outcomes: Vec<crate::calibration::BaselineDevelopmentOutcome> = records
            .iter()
            .filter(|r| r.family == family)
            .map(|r| crate::calibration::BaselineDevelopmentOutcome {
                task_id: r.task_id.clone(),
                stress_id: r.stress_level.clone(),
                repetition: r.repetition,
                oracle_success: r.oracle_success,
            })
            .collect();
        let sel = crate::calibration::select_family_stress(family, &dev_ids, &outcomes);
        per_family.push(crate::calibration::PlanFamilyEntry {
            family: family.to_string(),
            development_task_ids: sel.development_task_ids.clone(),
            per_task_per_stress_success_counts: sel.per_task_per_stress_success_counts.clone(),
            s0_sanity_pass: sel.s0_sanity_pass,
            selection_status: sel.status.as_str().to_string(),
            selected_stress: sel.selected_stress.clone(),
            selection_reason: sel.selection_reason.clone(),
        });
    }
    let selected_families: Vec<String> = per_family
        .iter()
        .filter(|f| f.selection_status == crate::calibration::SelectionStatus::Calibrated.as_str())
        .map(|f| f.family.clone())
        .collect();
    let calibrated = selected_families.len() as u32;
    let proceed = calibrated >= MIN_CALIBRATED_FAMILIES;
    let reps = if proceed {
        eval_repetitions_for(calibrated).unwrap_or(0)
    } else {
        0
    };
    let heldout_tasks: Vec<String> = if proceed {
        selected_families
            .iter()
            .flat_map(|f| heldout_tasks_for(f, registry))
            .map(|t| t.id)
            .collect()
    } else {
        Vec::new()
    };
    EvaluationPlan {
        experiment_id: EXPERIMENT_ID.to_string(),
        calibration_code_commit: prov.code_under_test_commit.clone(),
        calibration_raw_sha256: String::new(), // filled by the caller from the file hash
        baseline_prompt_sha256: prov.baseline_prompt_sha256.clone(),
        repair_prompt_sha256: prov.repair_prompt_sha256.clone(),
        task_registry_sha256: prov.task_registry_sha256.clone(),
        stress_registry_sha256: prov.stress_registry_sha256.clone(),
        per_family,
        selected_families,
        calibrated_family_count: calibrated,
        planned_pairs: PLANNED_PAIRS,
        heldout_tasks,
        repetitions_per_task: reps,
        min_valid_pairs: MIN_VALID_PAIRS,
        min_potential_information: MIN_POTENTIAL_INFORMATION,
        min_actual_informative: MIN_ACTUAL_INFORMATIVE,
        design_discordant_win_probability: power::DESIGN_DISCORDANT_WIN_PROBABILITY,
        conditional_power_at_min_informative: power::conditional_detection_power(
            MIN_ACTUAL_INFORMATIVE,
            power::DESIGN_DISCORDANT_WIN_PROBABILITY,
        ),
        proceed_to_evaluation: proceed,
    }
}

/// Convert a parsed plan `Value` into a typed plan (falls back to an
/// empty plan on parse failure, which then forces a failure in
/// verification).
impl From<Value> for EvaluationPlan {
    fn from(v: Value) -> Self {
        serde_json::from_value(v).unwrap_or_else(|_| EvaluationPlan {
            experiment_id: EXPERIMENT_ID.to_string(),
            calibration_code_commit: String::new(),
            calibration_raw_sha256: String::new(),
            baseline_prompt_sha256: String::new(),
            repair_prompt_sha256: String::new(),
            task_registry_sha256: String::new(),
            stress_registry_sha256: String::new(),
            per_family: Vec::new(),
            selected_families: Vec::new(),
            calibrated_family_count: 0,
            planned_pairs: 0,
            heldout_tasks: Vec::new(),
            repetitions_per_task: 0,
            min_valid_pairs: 0,
            min_potential_information: 0,
            min_actual_informative: 0,
            design_discordant_win_probability: 0.0,
            conditional_power_at_min_informative: 0.0,
            proceed_to_evaluation: false,
        })
    }
}

// ===========================================================================
// Phase B summary computation
// ===========================================================================

fn find_record<'a>(
    records: &'a [EvaluationRecord],
    task_id: &str,
    condition: &str,
    rep: u32,
) -> Option<&'a EvaluationRecord> {
    records
        .iter()
        .find(|x| x.task_id == task_id && x.condition == condition && x.repetition == rep)
}

/// Recompute the Phase B summary from raw records, the registry, and
/// the committed evaluation plan.
pub fn compute_evaluation_summary(
    records: &[EvaluationRecord],
    registry: &[TaskSpec],
    plan: &EvaluationPlan,
) -> EvaluationSummary {
    let first = records.first();
    let (model, endpoint, temp, commit, bh, rh, task_hash, cal_raw, plan_hash) = match first {
        Some(r) => (
            r.model.clone(),
            r.redacted_endpoint.clone(),
            r.temperature,
            r.final_eval_commit.clone(),
            r.baseline_prompt_sha256.clone(),
            r.repair_prompt_sha256.clone(),
            r.task_registry_sha256.clone(),
            r.calibration_raw_sha256.clone(),
            r.evaluation_plan_sha256.clone(),
        ),
        None => (
            String::new(),
            String::new(),
            0.0,
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        ),
    };

    let selected_families = plan.selected_families.clone();
    let reps = plan.repetitions_per_task;
    let heldout_tasks: Vec<String> = registry
        .iter()
        .filter(|t| selected_families.contains(&t.family) && t.split == oracle::TaskSplit::HeldOut)
        .map(|t| t.id.clone())
        .collect();

    // Expected episode count derived from the frozen plan, never from
    // the artifact (spec §51).
    let expected = heldout_tasks.len() as u32 * reps * CONDITIONS.len() as u32;

    let total = records.len() as u32;
    let infra: u32 = records
        .iter()
        .filter(|r| r.infrastructure_failure.is_some())
        .count() as u32;
    let agent: u32 = records.iter().filter(|r| r.agent_failure.is_some()).count() as u32;
    let successes: u32 = records.iter().filter(|r| r.oracle_success).count() as u32;
    let infra_rate = if total > 0 {
        infra as f64 / total as f64
    } else {
        0.0
    };
    let infra_within = infra_rate <= INFRA_FAILURE_RATE_THRESHOLD;

    let mut agent_breakdown: BTreeMap<String, u32> = BTreeMap::new();
    for r in records {
        if let Some(reason) = &r.agent_failure {
            *agent_breakdown.entry(reason.clone()).or_insert(0) += 1;
        }
    }
    let mut infra_breakdown: BTreeMap<String, u32> = BTreeMap::new();
    for r in records {
        if let Some(reason) = &r.infrastructure_failure {
            *infra_breakdown.entry(reason.clone()).or_insert(0) += 1;
        }
    }

    // Pooled comparison (spec §37): pair by (task_id, repetition);
    // infrastructure-failed episodes exclude their pair.
    let mut wins = 0u32;
    let mut losses = 0u32;
    let mut ties = 0u32;
    let mut valid = 0u32;
    let mut excluded = 0u32;
    let mut missing = 0u32;
    let mut expected_pairs = 0u32;
    let mut baseline_failures_valid = 0u32;
    let mut baseline_successes_valid = 0u32;
    for t in registry
        .iter()
        .filter(|t| selected_families.contains(&t.family) && t.split == oracle::TaskSplit::HeldOut)
    {
        for rep in 1..=reps {
            expected_pairs += 1;
            match (
                find_record(records, &t.id, "repair", rep),
                find_record(records, &t.id, "baseline", rep),
            ) {
                (Some(c), Some(b)) => {
                    if c.infrastructure_failure.is_some() || b.infrastructure_failure.is_some() {
                        excluded += 1;
                    } else {
                        valid += 1;
                        if b.oracle_success {
                            baseline_successes_valid += 1;
                        } else {
                            baseline_failures_valid += 1;
                        }
                        match pair_outcome(c.oracle_success, b.oracle_success) {
                            "win" => wins += 1,
                            "loss" => losses += 1,
                            _ => ties += 1,
                        }
                    }
                }
                _ => missing += 1,
            }
        }
    }
    let informative = wins + losses;
    let p = stats::exact_sign_test_p(wins as u64, losses as u64);
    let class = stats::classify_quality(wins as u64, losses as u64);
    let comparison = PairComparison {
        baseline: "baseline".to_string(),
        candidate: "repair".to_string(),
        selected_families: selected_families.clone(),
        expected_pairs,
        valid_pairs: valid,
        excluded_infrastructure_pairs: excluded,
        missing_pairs: missing,
        informative_pairs: informative,
        wins,
        losses,
        ties,
        sign_test_p: p,
        classification: class.as_str().to_string(),
    };

    // Potential information capacity (spec §32): baseline failures
    // among valid pairs — candidate-independent.
    let potential = PotentialInformationGate {
        valid_pairs: valid,
        baseline_successes_in_valid_pairs: baseline_successes_valid,
        baseline_failures_in_valid_pairs: baseline_failures_valid,
        potential_information_capacity: baseline_failures_valid,
        sufficient: baseline_failures_valid >= MIN_POTENTIAL_INFORMATION,
    };

    // Per-family diagnostics (spec §42–43). Development rates come
    // from the *committed* evaluation plan (frozen before Phase B).
    let per_family_diagnostics: Vec<FamilyDiagnostic> = selected_families
        .iter()
        .map(|f| {
            let fam_heldout: Vec<String> = heldout_tasks_for(f, registry)
                .iter()
                .map(|t| t.id.clone())
                .collect();
            let base: Vec<&EvaluationRecord> = records
                .iter()
                .filter(|r| r.family == *f && r.condition == "baseline")
                .collect();
            let rep: Vec<&EvaluationRecord> = records
                .iter()
                .filter(|r| r.family == *f && r.condition == "repair")
                .collect();
            let mut fw = 0u32;
            let mut fl = 0u32;
            let mut ft = 0u32;
            for t in heldout_tasks_for(f, registry).iter() {
                for r in 1..=reps {
                    if let (Some(c), Some(b)) = (
                        rep.iter().find(|x| x.task_id == t.id && x.repetition == r),
                        base.iter().find(|x| x.task_id == t.id && x.repetition == r),
                    ) {
                        if c.infrastructure_failure.is_none() && b.infrastructure_failure.is_none()
                        {
                            match pair_outcome(c.oracle_success, b.oracle_success) {
                                "win" => fw += 1,
                                "loss" => fl += 1,
                                _ => ft += 1,
                            }
                        }
                    }
                }
            }
            let base_ok = base.iter().filter(|r| r.oracle_success).count() as u32;
            let rep_ok = rep.iter().filter(|r| r.oracle_success).count() as u32;
            let base_rate_fam = if base.is_empty() {
                0.0
            } else {
                base_ok as f64 / base.len() as f64
            };
            let plan_entry = plan.per_family.iter().find(|pf| pf.family == *f);
            let dev_max = plan_entry
                .and_then(|pf| {
                    let s = pf.selected_stress.as_ref()?;
                    let level = crate::tools::StressLevel::by_id(s)?;
                    let rates: Vec<f64> = pf
                        .per_task_per_stress_success_counts
                        .iter()
                        .map(|c| {
                            c.count_at(level.drop_count) as f64 / CALIBRATION_REPETITIONS as f64
                        })
                        .collect();
                    Some(rates.iter().cloned().fold(0.0_f64, f64::max))
                })
                .unwrap_or(0.0);
            let selected = plan_entry
                .and_then(|pf| pf.selected_stress.clone())
                .unwrap_or_default();
            let te = base_rate_fam - dev_max;
            FamilyDiagnostic {
                family: (*f).to_string(),
                selected_stress: selected,
                heldout_tasks: fam_heldout,
                baseline_successes: base_ok,
                repair_successes: rep_ok,
                wins: fw,
                losses: fl,
                ties: ft,
                informative_pairs: fw + fl,
                development_max_success_rate_at_selected_stress: dev_max,
                heldout_baseline_success_rate: base_rate_fam,
                transfer_delta: te,
            }
        })
        .collect();

    let conditions: Vec<ConditionSummary> = CONDITIONS
        .iter()
        .map(|c| {
            let all: Vec<(bool, ExecutionCore)> = records
                .iter()
                .filter(|r| r.condition == *c)
                .map(|r| (r.oracle_success, evaluation_core(r)))
                .collect();
            let mut a = CostAccum::default();
            for (s, core) in &all {
                a.add_core(*s, core);
            }
            let mut s = CostAccum::default();
            for (x, core) in all.iter().filter(|(s, _)| *s) {
                s.add_core(*x, core);
            }
            let mut f = CostAccum::default();
            for (x, core) in all.iter().filter(|(s, _)| !*s) {
                f.add_core(*x, core);
            }
            ConditionSummary {
                condition: (*c).to_string(),
                all: a.finish(),
                success: s.finish(),
                failure: f.finish(),
                cache: cache_metrics_core(&all, |_| true),
            }
        })
        .collect();

    let cores: Vec<(bool, ExecutionCore)> = records
        .iter()
        .map(|r| (r.oracle_success, evaluation_core(r)))
        .collect();
    let cache_overall = cache_metrics_core(&cores, |_| true);
    let mut condition_position_cache_audit: Vec<ConditionPositionCacheRow> = Vec::new();
    for c in CONDITIONS.iter() {
        for pos in 1..=2 {
            let sub: Vec<(bool, ExecutionCore)> = records
                .iter()
                .filter(|r| r.condition == *c && r.condition_position == pos)
                .map(|r| (r.oracle_success, evaluation_core(r)))
                .collect();
            let m = cache_metrics_core(&sub, |_| true);
            condition_position_cache_audit.push(ConditionPositionCacheRow {
                condition: (*c).to_string(),
                condition_position: pos,
                episodes: sub.len() as u32,
                nominal_prompt_tokens: m.nominal_prompt_tokens,
                cached_prompt_tokens: m.cached_prompt_tokens,
                uncached_prompt_tokens: m.uncached_prompt_tokens,
                cache_hit_ratio: m.cache_hit_ratio,
            });
        }
    }

    let complete = phase_b_artifact_complete(records, registry, plan, expected as usize);

    // Pre-registered conclusion rule (spec §40): the three-way
    // distinction — not enough opportunity to detect improvement
    // → inconclusive; enough opportunity but no improvement → refuted;
    // enough opportunity and a significant improvement → supported.
    let valid_ok = valid >= MIN_VALID_PAIRS;
    let potential_ok = potential.sufficient;
    let informative_ok = informative >= MIN_ACTUAL_INFORMATIVE;
    let calibrated_ok = plan.calibrated_family_count >= MIN_CALIBRATED_FAMILIES;
    let gates = Gates {
        phase_a_complete: plan.proceed_to_evaluation,
        calibrated_families_sufficient: calibrated_ok,
        phase_b_artifact_complete: complete,
        infrastructure_within_threshold: infra_within,
        valid_pairs_sufficient: valid_ok,
        potential_information_sufficient: potential_ok,
        actual_informative_sufficient: informative_ok,
    };
    let mut reasons: Vec<String> = Vec::new();
    let mut result = "supported".to_string();
    if !gates.phase_a_complete {
        result = "inconclusive".into();
        reasons.push("Phase A incomplete (calibration did not pass its gates)".to_string());
    }
    if !gates.calibrated_families_sufficient {
        result = "inconclusive".into();
        reasons.push(format!(
            "fewer than {MIN_CALIBRATED_FAMILIES} families calibrated"
        ));
    }
    if !gates.phase_b_artifact_complete {
        result = "inconclusive".into();
        reasons.push("Phase B artifact incomplete".to_string());
    }
    if !gates.infrastructure_within_threshold {
        result = "inconclusive".into();
        reasons.push(format!(
            "infrastructure failure rate {infra_rate:.4} exceeds {INFRA_FAILURE_RATE_THRESHOLD:.2}"
        ));
    }
    if !gates.valid_pairs_sufficient {
        result = "inconclusive".into();
        reasons.push(format!("valid pairs {valid} < {MIN_VALID_PAIRS}"));
    }
    if !gates.potential_information_sufficient {
        result = "inconclusive".into();
        reasons.push(format!(
            "potential information capacity {} (baseline failures among valid pairs) < {MIN_POTENTIAL_INFORMATION}",
            potential.potential_information_capacity
        ));
    }
    if !gates.actual_informative_sufficient {
        result = "inconclusive".into();
        reasons.push(format!(
            "actual informative pairs {informative} < {MIN_ACTUAL_INFORMATIVE}"
        ));
    }
    if result == "supported" {
        if class == stats::QualityClass::Improvement {
            reasons.push(
                "all information gates passed and repair classified quality_improvement"
                    .to_string(),
            );
        } else {
            result = "refuted".into();
            reasons.push(format!(
                "all information gates passed but repair classified {} (expected quality_improvement)",
                class.as_str()
            ));
        }
    }

    EvaluationSummary {
        experiment_id: EXPERIMENT_ID.to_string(),
        phase: "evaluation".to_string(),
        model,
        redacted_endpoint: endpoint,
        temperature: temp,
        final_eval_commit: commit,
        baseline_prompt_sha256: bh,
        repair_prompt_sha256: rh,
        task_registry_sha256: task_hash,
        calibration_raw_sha256: cal_raw,
        evaluation_plan_sha256: plan_hash,
        selected_families,
        heldout_tasks,
        repetitions_per_task: reps,
        episodes_expected: expected,
        episodes_total: total,
        artifacts_complete: complete,
        oracle_successes: successes,
        agent_failures: agent,
        infrastructure_failures: infra,
        infrastructure_failure_rate: infra_rate,
        agent_failure_breakdown: agent_breakdown,
        infrastructure_failure_breakdown: infra_breakdown,
        potential_information: potential,
        comparison,
        per_family_diagnostics,
        conditions,
        cache_overall,
        condition_position_cache_audit,
        gates,
        conclusion: Conclusion { result, reasons },
    }
}
// ===========================================================================
// Artifact verifiers
// ===========================================================================

/// Keys that must never appear anywhere in a raw artifact record.
/// Reasoning *content* and credential material are redacted by
/// construction; the candidate-side keys guard the anti-leakage
/// requirement (spec §5, §46): the difficulty selector never sees the
/// candidate, and a calibration record must not carry any of it.
pub const FORBIDDEN_KEYS: &[&str] = &[
    "reasoning",
    "reasoning_content",
    "thinking",
    "analysis",
    "chain_of_thought",
    "co_t",
    "api_key",
    "apikey",
    "authorization",
    "x-api-key",
    "secret",
    "password",
    "token_value",
    "candidate_success",
    "candidate_outcome",
    "candidate_trajectory",
    "candidate_cost",
    "repair_success",
    "repair_outcome",
    "repair_trajectory",
    "repair_cost",
    "heldout_outcome",
];

fn scan_forbidden_keys(value: &Value, path: &str, out: &mut Vec<String>) {
    if let Some(obj) = value.as_object() {
        for (k, v) in obj {
            if FORBIDDEN_KEYS.contains(&k.to_lowercase().as_str()) {
                out.push(format!("{path}.{k} is a forbidden field"));
            }
            scan_forbidden_keys(v, &format!("{path}.{k}"), out);
        }
    } else if let Some(items) = value.as_array() {
        for (i, v) in items.iter().enumerate() {
            scan_forbidden_keys(v, &format!("{path}[{i}]"), out);
        }
    }
}

/// Validate one tool call's arguments against the registered two-tool
/// schema. Returns an error string, or `None` if valid (or if the
/// arguments are the verbatim raw string of an unparseable call, which
/// is a known agent failure).
fn check_call_schema(name: &str, args: &Value) -> Option<String> {
    let obj = match name {
        "state_read" | "state_write" => {
            if args.is_string() {
                return None;
            }
            args.as_object()?
        }
        other => return Some(format!("unknown tool {other:?}")),
    };
    let expected = match name {
        "state_read" => &["key"][..],
        "state_write" => &["key", "value"][..],
        _ => unreachable!(),
    };
    if !obj
        .get("key")
        .and_then(Value::as_str)
        .and_then(crate::tools::StateKey::parse)
        .is_some()
    {
        return Some(format!("{name}: missing or invalid \"key\""));
    }
    if name == "state_write" && obj.get("value").map(Value::is_string) != Some(true) {
        return Some(format!("{name}: missing or non-string \"value\""));
    }
    for k in obj.keys() {
        if !expected.contains(&k.as_str()) {
            return Some(format!("{name}: unexpected argument {k:?}"));
        }
    }
    None
}

/// Per-record structural checks shared by both verifiers: termination
/// consistency, usage arithmetic, requested/executed integrity,
/// model-visible result shapes, write-audit consistency, fault-schedule
/// audit, oracle recomputation.
fn check_record_fields(tag: &str, v: &Value, registry: &[TaskSpec]) -> Vec<String> {
    let mut errors = Vec::new();

    // Termination / failure consistency.
    let term_str = v
        .get("termination_reason")
        .and_then(Value::as_str)
        .unwrap_or("");
    match TerminationReason::parse(term_str) {
        Some(term) => {
            let agent = v.get("agent_failure").and_then(Value::as_str);
            let infra = v.get("infrastructure_failure").and_then(Value::as_str);
            if agent.is_some() && infra.is_some() {
                errors.push(format!("{tag}: both agent and infrastructure failure set"));
            }
            if term.is_agent_failure() != agent.is_some() {
                errors.push(format!(
                    "{tag}: agent_failure field does not match termination {term_str:?}"
                ));
            }
            if let Some(a) = agent {
                if a != term.as_str() {
                    errors.push(format!("{tag}: agent_failure {a:?} mismatches termination"));
                }
            }
            if term.is_infrastructure() != infra.is_some() {
                errors.push(format!(
                    "{tag}: infrastructure_failure field does not match termination {term_str:?}"
                ));
            }
            if let Some(x) = infra {
                if x != term.as_str() {
                    errors.push(format!(
                        "{tag}: infrastructure_failure {x:?} mismatches termination"
                    ));
                }
            }
        }
        None => errors.push(format!(
            "{tag}: unregistered termination_reason {term_str:?}"
        )),
    }

    if v.get("usage_accounting_error")
        .and_then(Value::as_str)
        .is_some()
    {
        errors.push(format!(
            "{tag}: usage accounting error recorded: {}",
            v.get("usage_accounting_error").unwrap()
        ));
    }

    // Per-request usage arithmetic.
    if let Some(reqs) = v.get("model_requests").and_then(Value::as_array) {
        for (idx, q) in reqs.iter().enumerate() {
            let prev_turn = idx as u32;
            let qtag = format!(
                "{} turn {}",
                tag,
                q.get("turn").and_then(Value::as_u64).unwrap_or(0)
            );
            if q.get("turn").and_then(Value::as_u64) != Some(prev_turn as u64 + 1) {
                errors.push(format!("{qtag}: model_requests turns are not sequential"));
            }
            let p = q.get("prompt_tokens").and_then(Value::as_u64);
            let c = q.get("cached_prompt_tokens").and_then(Value::as_u64);
            let unc = q.get("uncached_prompt_tokens").and_then(Value::as_u64);
            if let (Some(p), Some(c)) = (p, c) {
                if c > p {
                    errors.push(format!(
                        "{qtag}: cached_prompt_tokens {c} > prompt_tokens {p}"
                    ));
                }
            }
            let expected_uncached = match (p, c) {
                (Some(p), Some(c)) if c <= p => Some(p - c),
                _ => None,
            };
            if unc != expected_uncached {
                errors.push(format!(
                    "{qtag}: uncached_prompt_tokens {unc:?} inconsistent with prompt/cached {p:?}/{c:?}"
                ));
            }
            if let (Some(p), Some(cc), Some(t)) = (
                p,
                q.get("completion_tokens").and_then(Value::as_u64),
                q.get("total_tokens").and_then(Value::as_u64),
            ) {
                if t != p + cc {
                    errors.push(format!(
                        "{qtag}: total_tokens {t} != prompt {p} + completion {cc}"
                    ));
                }
            }
        }
    }

    // Requested / executed integrity: executed calls are an
    // order-preserving, identity-equal prefix of requested calls.
    let requested = v
        .get("requested_tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let executed = v
        .get("executed_tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let model_turns = v
        .get("model_turn_count")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    for (k, c) in requested.iter().enumerate() {
        if c.get("sequence").and_then(Value::as_u64) != Some(k as u64) {
            errors.push(format!(
                "{tag}: requested_tool_calls sequence not contiguous"
            ));
            break;
        }
        let turn = c.get("turn").and_then(Value::as_u64).unwrap_or(0) as u32;
        if turn == 0 || turn > model_turns {
            errors.push(format!(
                "{tag}: requested call turn {turn} inconsistent with model turns"
            ));
        }
        let name = c.get("tool_name").and_then(Value::as_str).unwrap_or("");
        if let Some(e) = check_call_schema(name, c.get("arguments").unwrap_or(&Value::Null)) {
            errors.push(format!("{tag}: requested call: {e}"));
        }
    }
    if v.get("tool_call_count").and_then(Value::as_u64) != Some(executed.len() as u64) {
        errors.push(format!(
            "{tag}: tool_call_count != executed_tool_calls.len() ({} vs {})",
            v.get("tool_call_count")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            executed.len()
        ));
    }
    for (k, e) in executed.iter().enumerate() {
        if e.get("sequence").and_then(Value::as_u64) != Some(k as u64) {
            errors.push(format!(
                "{tag}: executed_tool_calls sequence not contiguous"
            ));
            break;
        }
        match requested.get(k) {
            None => errors.push(format!(
                "{tag}: executed call has no matching requested call"
            )),
            Some(q) => {
                if q.get("turn") != e.get("turn")
                    || q.get("tool_call_id") != e.get("tool_call_id")
                    || q.get("tool_name") != e.get("tool_name")
                    || canonical_json(q.get("arguments").unwrap_or(&Value::Null))
                        != canonical_json(e.get("arguments").unwrap_or(&Value::Null))
                {
                    errors.push(format!(
                        "{tag}: executed call {k} does not match its requested call"
                    ));
                }
            }
        }
        // Model-visible result shapes (the fault must stay invisible).
        let name = e.get("tool_name").and_then(Value::as_str).unwrap_or("");
        match name {
            "state_write" => {
                let res = e.get("result").cloned().unwrap_or(Value::Null);
                let res_key = res.get("key").and_then(Value::as_str);
                let arg_key = e
                    .get("arguments")
                    .and_then(|a| a.get("key"))
                    .and_then(Value::as_str);
                let res_value = res.get("value").and_then(Value::as_str);
                let arg_value = e
                    .get("arguments")
                    .and_then(|a| a.get("value"))
                    .and_then(Value::as_str);
                if res.get("ok") != Some(&Value::Bool(true))
                    || res_key != arg_key
                    || res_value != arg_value
                {
                    errors.push(format!(
                        "{tag}: write result does not mirror the requested write"
                    ));
                }
                if let Some(obj) = res.as_object() {
                    for k in obj.keys() {
                        if !["ok", "key", "value"].contains(&k.as_str()) {
                            errors.push(format!(
                                "{tag}: write result exposes field {k:?} (fault leak)"
                            ));
                        }
                    }
                }
            }
            "state_read" => {
                if e.get("result").and_then(|r| r.get("ok")).is_some() {
                    errors.push(format!("{tag}: read result has unexpected shape"));
                }
            }
            other => errors.push(format!("{tag}: executed call with unknown tool {other:?}")),
        }
    }

    // Write-attempt audit vs executed writes.
    let writes: Vec<&Value> = executed
        .iter()
        .filter(|e| e.get("tool_name").and_then(Value::as_str) == Some("state_write"))
        .collect();
    let attempts = v
        .get("environment_write_attempts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if writes.len() != attempts.len() {
        errors.push(format!(
            "{tag}: {} executed writes vs {} write attempts",
            writes.len(),
            attempts.len()
        ));
    } else {
        for (w, a) in writes.iter().zip(attempts.iter()) {
            let wk = w
                .get("arguments")
                .and_then(|x| x.get("key"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let wv = w
                .get("arguments")
                .and_then(|x| x.get("value"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if a.get("key").and_then(Value::as_str) != Some(wk)
                || a.get("requested_value").and_then(Value::as_str) != Some(wv)
            {
                errors.push(format!(
                    "{tag}: write attempt does not match executed write"
                ));
            }
        }
        for (k, a) in attempts.iter().enumerate() {
            if a.get("sequence").and_then(Value::as_u64) != Some(k as u64) {
                errors.push(format!("{tag}: write attempt sequence not contiguous"));
                break;
            }
        }
    }

    // Fault schedule executed exactly as registered.
    if let Some(fault) = v.get("fault_mode").filter(|f| !f.is_null()) {
        if let Ok(typed_attempts) = serde_json::from_value::<Vec<WriteAttempt>>(
            v.get("environment_write_attempts")
                .cloned()
                .unwrap_or(Value::Null),
        ) {
            let run_id = v.get("run_id").and_then(Value::as_str).unwrap_or(tag);
            if let Some(viol) = audit_record(run_id, fault, &typed_attempts) {
                errors.push(format!("{tag}: fault audit: {viol}"));
            }
        }
    }

    // Oracle recomputation (anti-circularity: task + input only).
    let task_id = v.get("task_id").and_then(Value::as_str).unwrap_or("");
    if let Some(t) = registry.iter().find(|t| t.id == task_id) {
        let final_state = v.get("final_state").cloned().unwrap_or(Value::Null);
        let initial_state = v.get("initial_state").cloned().unwrap_or(Value::Null);
        let eval = oracle::evaluate_task(
            t,
            &oracle::OracleInput {
                termination_reason: term_str.to_string(),
                initial_state,
                final_state,
            },
        );
        if v.get("oracle_success").and_then(Value::as_bool) != Some(eval.success) {
            errors.push(format!(
                "{tag}: stored oracle_success {} != recomputed {}",
                v.get("oracle_success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                eval.success
            ));
        }
        let stored_reasons = v
            .get("oracle_failure_reasons")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter(|x| x.is_string())
                    .map(|x| x.as_str().unwrap().to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if stored_reasons != eval.reasons {
            errors.push(format!(
                "{tag}: oracle failure reasons mismatch recomputation"
            ));
        }
    }

    errors
}

/// Verify the frozen evaluation plan's pre-registered design constants
/// (spec §29–35, §45). Defense in depth on top of the deep-equality
/// recompute: a tampered plan must fail here with a specific message.
fn check_plan_design_constants(plan: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    if plan.get("planned_pairs").and_then(Value::as_u64) != Some(PLANNED_PAIRS as u64) {
        errors.push(format!("plan planned_pairs != registered {PLANNED_PAIRS}"));
    }
    if plan.get("min_valid_pairs").and_then(Value::as_u64) != Some(MIN_VALID_PAIRS as u64) {
        errors.push(format!(
            "plan min_valid_pairs != registered {MIN_VALID_PAIRS}"
        ));
    }
    for field in ["min_potential_information", "min_actual_informative"] {
        if plan.get(field).and_then(Value::as_u64) != Some(MIN_POTENTIAL_INFORMATION as u64) {
            errors.push(format!(
                "plan {field} != registered {MIN_POTENTIAL_INFORMATION}"
            ));
        }
    }
    match plan
        .get("design_discordant_win_probability")
        .and_then(Value::as_f64)
    {
        Some(theta) if (theta - power::DESIGN_DISCORDANT_WIN_PROBABILITY).abs() > 1e-12 => {
            errors.push(format!(
                "plan design_discordant_win_probability {theta} != registered {}",
                power::DESIGN_DISCORDANT_WIN_PROBABILITY
            ));
        }
        Some(_) => {}
        None => errors.push("plan design_discordant_win_probability missing".to_string()),
    }
    match (
        plan.get("design_discordant_win_probability")
            .and_then(Value::as_f64),
        plan.get("conditional_power_at_min_informative")
            .and_then(Value::as_f64),
    ) {
        (Some(theta), Some(stored)) => {
            let recomputed = power::conditional_detection_power(MIN_ACTUAL_INFORMATIVE, theta);
            if (stored - recomputed).abs() > 1e-9 {
                errors.push(format!(
                    "plan conditional_power_at_min_informative {stored} != recomputed {recomputed}"
                ));
            }
        }
        _ => errors.push("plan conditional_power_at_min_informative missing".to_string()),
    }
    let reps = plan
        .get("repetitions_per_task")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let count = plan
        .get("calibrated_family_count")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let expected_reps = if plan.get("proceed_to_evaluation").and_then(Value::as_bool) == Some(true)
    {
        eval_repetitions_for(count).unwrap_or(0)
    } else {
        0
    };
    if reps != expected_reps {
        errors.push(format!(
            "plan repetitions_per_task {reps} is not machine-derived from calibrated_family_count {count} (expected {expected_reps})"
        ));
    }
    if let Some(held) = plan.get("heldout_tasks").and_then(Value::as_array) {
        let held_ids: Vec<&str> = held.iter().filter_map(Value::as_str).collect();
        if reps > 0 && held_ids.len() as u32 * reps != PLANNED_PAIRS {
            errors.push(format!(
                "plan held-out tasks ({}) × repetitions ({reps}) != {PLANNED_PAIRS} planned pairs",
                held_ids.len()
            ));
        }
    }
    errors
}

/// Full Phase A verification (spec §47).
#[allow(clippy::too_many_arguments)]
pub fn verify_calibration_artifacts(
    record_values: &[Value],
    summary_value: &Value,
    plan_value: &Value,
    registry: &[TaskSpec],
    baseline_prompt_hash: &str,
    repair_prompt_hash: &str,
    task_registry_hash: &str,
    stress_registry_hash: &str,
    calibration_file_sha256: &str,
) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();

    // --- record count (hardcoded design, never inferred) ---
    if record_values.len() != CALIBRATION_EPISODES {
        errors.push(format!(
            "expected exactly {CALIBRATION_EPISODES} calibration records, found {}",
            record_values.len()
        ));
    }

    // --- field-level tampering: forbidden keys (candidate must be unseen) ---
    for (i, v) in record_values.iter().enumerate() {
        scan_forbidden_keys(v, &format!("_records[{i}]"), &mut errors);
    }

    // --- deserialize ---
    let mut records: Vec<CalibrationRecord> = Vec::with_capacity(record_values.len());
    for (i, v) in record_values.iter().enumerate() {
        match serde_json::from_value::<CalibrationRecord>(v.clone()) {
            Ok(r) => records.push(r),
            Err(e) => errors.push(format!("record[{i}] does not deserialize: {e}")),
        }
    }

    if registry.len() != 12 {
        errors.push(format!(
            "expected 12 registered tasks, found {}",
            registry.len()
        ));
    }
    let by_id: BTreeMap<&str, &TaskSpec> = registry.iter().map(|t| (t.id.as_str(), t)).collect();

    // --- registration + design checks per record ---
    for (i, r) in records.iter().enumerate() {
        let tag = format!("record[{}]({})", i, r.run_id);
        if r.experiment_id != EXPERIMENT_ID {
            errors.push(format!("{tag}: experiment_id is not {EXPERIMENT_ID:?}"));
        }
        if r.phase != "calibration" {
            errors.push(format!("{tag}: phase is not 'calibration'"));
        }
        if r.condition != "baseline" {
            errors.push(format!(
                "{tag}: condition {:?} — Phase A is baseline-only; candidate data must never enter calibration",
                r.condition
            ));
        }
        if !FAMILIES.contains(&r.family.as_str()) {
            errors.push(format!("{tag}: unregistered family {:?}", r.family));
        }
        if !(1..=CALIBRATION_REPETITIONS).contains(&r.repetition)
            || !(1..=CALIBRATION_STRESS_LEVELS).contains(&r.execution_position)
        {
            errors.push(format!("{tag}: repetition/position out of range"));
        }
        match by_id.get(r.task_id.as_str()) {
            Some(t) => {
                if t.family != r.family || t.split != oracle::TaskSplit::Development {
                    errors.push(format!(
                        "{tag}: task {} does not belong to development family {:?}",
                        r.task_id, r.family
                    ));
                }
                if r.initial_state != t.initial_state {
                    errors.push(format!("{tag}: initial_state does not match registry"));
                }
                if r.target_state != t.target_state {
                    errors.push(format!(
                        "{tag}: target_state does not match registry (task changed)"
                    ));
                }
                let expected_fault =
                    crate::tools::FaultMode::for_stress(&t.fault_key, r.drop_count);
                let fv: Value = serde_json::to_value(&expected_fault).unwrap_or(Value::Null);
                if r.fault_mode != fv {
                    errors.push(format!(
                        "{tag}: fault_mode does not match (fault key, stress)"
                    ));
                }
            }
            None => errors.push(format!("{tag}: unknown task {}", r.task_id)),
        }
        if let Some(l) = crate::tools::StressLevel::by_id(&r.stress_level) {
            if l.drop_count != r.drop_count {
                errors.push(format!(
                    "{tag}: stress_level {:?} inconsistent with drop_count {}",
                    r.stress_level, r.drop_count
                ));
            }
        } else {
            errors.push(format!(
                "{tag}: unregistered stress level {:?}",
                r.stress_level
            ));
        }
        if r.baseline_prompt_sha256 != baseline_prompt_hash {
            errors.push(format!(
                "{tag}: baseline_prompt_sha256 does not match the committed baseline prompt"
            ));
        }
        if r.repair_prompt_sha256 != repair_prompt_hash {
            errors.push(format!(
                "{tag}: repair_prompt_sha256 does not match the committed repair prompt (frozen before Phase A)"
            ));
        }
        if r.task_registry_sha256 != task_registry_hash {
            errors.push(format!(
                "{tag}: task_registry_sha256 does not match the registered task suite"
            ));
        }
        if r.stress_registry_sha256 != stress_registry_hash {
            errors.push(format!(
                "{tag}: stress_registry_sha256 does not match the registered stress ladder"
            ));
        }

        // Registered cyclic stress order (spec §20).
        let expected_stress = crate::tools::STRESS_LEVELS
            [calibration_stress_index(r.repetition, r.execution_position) as usize];
        if r.stress_level != expected_stress.id {
            errors.push(format!(
                "{tag}: cyclic order violation — position {} of rep {} must be {}",
                r.execution_position, r.repetition, expected_stress.id
            ));
        }
    }

    // --- full factorial coverage: every (family, task, rep, pos)
    //     exactly once ---
    let mut tuples: BTreeMap<(String, String, u32, u32), u32> = BTreeMap::new();
    for r in &records {
        *tuples
            .entry((
                r.family.clone(),
                r.task_id.clone(),
                r.repetition,
                r.execution_position,
            ))
            .or_insert(0) += 1;
    }
    for f in FAMILIES {
        for t in registry
            .iter()
            .filter(|t| t.family == f && t.split == oracle::TaskSplit::Development)
        {
            for rep in 1..=CALIBRATION_REPETITIONS {
                for pos in 1..=CALIBRATION_STRESS_LEVELS {
                    match tuples.get(&(f.to_string(), t.id.clone(), rep, pos)) {
                        None => errors.push(format!(
                            "missing development cell: {f} {} rep{rep} pos{pos}",
                            t.id
                        )),
                        Some(n) if *n != 1 => errors.push(format!(
                            "duplicate development cell: {f} {} rep{rep} pos{pos} ×{n}",
                            t.id
                        )),
                        _ => {}
                    }
                }
            }
        }
    }

    // --- strict sequence + single run prefix ---
    let mut prefix: Option<&str> = None;
    for (i, r) in records.iter().enumerate() {
        let tag = format!("record[{}]", i);
        let expected_seq = (i as u32) + 1;
        if r.sequence != expected_seq {
            errors.push(format!(
                "{tag}: sequence {} is not strict (expected {expected_seq})",
                r.sequence
            ));
        }
        match r.run_id.rsplit_once('-').map(|(p, _)| p) {
            None => errors.push(format!("{tag}: run_id has no sequence suffix")),
            Some(p) => {
                let suffix = r.run_id.len().saturating_sub(p.len()).saturating_sub(1);
                if suffix == 0 {
                    errors.push(format!("{tag}: run_id has no sequence suffix"));
                } else {
                    let seq_text = r.run_id[p.len() + 1..].to_string();
                    if seq_text.parse::<u32>().ok() != Some(expected_seq) {
                        errors.push(format!(
                            "{tag}: run_id sequence does not match strict order"
                        ));
                    }
                    match prefix {
                        None => prefix = Some(p),
                        Some(prev) if prev != p => {
                            errors.push(format!("{tag}: run prefix {p:?} differs from {prev:?}"))
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // --- provenance consistency across records ---
    let mut consistent = |get: fn(&CalibrationRecord) -> &str, name: &str| {
        let mut set: BTreeMap<&str, usize> = BTreeMap::new();
        for r in &records {
            *set.entry(get(r)).or_insert(0) += 1;
        }
        if set.len() > 1 {
            errors.push(format!(
                "field {name:?} differs across records ({} values)",
                set.len()
            ));
        }
    };
    consistent(|r| &r.model, "model");
    consistent(|r| &r.redacted_endpoint, "redacted_endpoint");
    consistent(|r| &r.code_under_test_commit, "code_under_test_commit");
    consistent(|r| &r.baseline_prompt_sha256, "baseline_prompt_sha256");
    consistent(|r| &r.repair_prompt_sha256, "repair_prompt_sha256");
    consistent(|r| &r.task_registry_sha256, "task_registry_sha256");
    consistent(|r| &r.stress_registry_sha256, "stress_registry_sha256");
    if !records.is_empty() {
        let mut temps: Vec<f64> = Vec::new();
        for r in &records {
            if !temps.iter().any(|t| t == &r.temperature) {
                temps.push(r.temperature);
            }
        }
        if temps.len() > 1 {
            errors.push(format!("temperature differs across records ({temps:?})"));
        }
        let commit = &records[0].code_under_test_commit;
        if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
            errors.push("code_under_test_commit is not a 40-hex git SHA".to_string());
        }
    }

    // --- per-record structural checks (usage, calls, audit, oracle) ---
    for (i, v) in record_values.iter().enumerate() {
        let tag = format!("record[{}]", i);
        errors.extend(check_record_fields(&tag, v, registry));
    }

    // --- summary deep-equality ---
    if !records.is_empty() {
        let recomputed = compute_calibration_summary(&records, registry);
        let recomputed_value: Value =
            serde_json::from_str(&serde_json::to_string(&recomputed).unwrap())
                .unwrap_or(Value::Null);
        if canonical_json(&recomputed_value) != canonical_json(summary_value) {
            errors
                .push("calibration summary does not deep-equal the recomputed summary".to_string());
        }
    }

    // --- evaluation plan: design constants + recompute, deep-compare ---
    errors.extend(check_plan_design_constants(plan_value));
    if !records.is_empty() {
        let first = &records[0];
        let prov = PlanProvenance {
            code_under_test_commit: first.code_under_test_commit.clone(),
            baseline_prompt_sha256: first.baseline_prompt_sha256.clone(),
            repair_prompt_sha256: repair_prompt_hash.to_string(),
            task_registry_sha256: first.task_registry_sha256.clone(),
            stress_registry_sha256: first.stress_registry_sha256.clone(),
        };
        let mut recomputed = recompute_evaluation_plan(&records, registry, &prov);
        recomputed.calibration_raw_sha256 = calibration_file_sha256.to_string();
        if canonical_json(&serde_json::to_value(&recomputed).unwrap()) != canonical_json(plan_value)
        {
            errors.push(
                "evaluation plan does not deep-equal the plan recomputed from raw calibration records"
                    .to_string(),
            );
        }
    }
    // Plan frozen provenance vs on-disk files + raw artifact hash.
    if plan_value
        .get("repair_prompt_sha256")
        .and_then(Value::as_str)
        != Some(repair_prompt_hash)
    {
        errors.push(
            "plan repair_prompt_sha256 does not match the committed repair prompt".to_string(),
        );
    }
    if plan_value
        .get("baseline_prompt_sha256")
        .and_then(Value::as_str)
        != Some(baseline_prompt_hash)
    {
        errors.push(
            "plan baseline_prompt_sha256 does not match the committed baseline prompt".to_string(),
        );
    }
    if plan_value
        .get("task_registry_sha256")
        .and_then(Value::as_str)
        != Some(task_registry_hash)
    {
        errors
            .push("plan task_registry_sha256 does not match the registered task suite".to_string());
    }
    if plan_value
        .get("stress_registry_sha256")
        .and_then(Value::as_str)
        != Some(stress_registry_hash)
    {
        errors.push(
            "plan stress_registry_sha256 does not match the registered stress ladder".to_string(),
        );
    }
    if plan_value
        .get("calibration_raw_sha256")
        .and_then(Value::as_str)
        != Some(calibration_file_sha256)
    {
        errors.push(
            "plan calibration_raw_sha256 does not match the calibration artifact".to_string(),
        );
    }

    errors
}
/// Full Phase B verification (spec §52).
#[allow(clippy::too_many_arguments)]
pub fn verify_evaluation_artifacts(
    record_values: &[Value],
    summary_value: &Value,
    plan_value: &Value,
    registry: &[TaskSpec],
    baseline_prompt_hash: &str,
    repair_prompt_hash: &str,
    task_registry_hash: &str,
    plan_file_sha256: &str,
) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();

    if registry.len() != 12 {
        errors.push(format!(
            "expected 12 registered tasks, found {}",
            registry.len()
        ));
    }
    let by_id: BTreeMap<&str, &TaskSpec> = registry.iter().map(|t| (t.id.as_str(), t)).collect();

    // --- evaluation plan frozen provenance vs on-disk files ---
    for (field, file_hash, label) in [
        (
            "baseline_prompt_sha256",
            baseline_prompt_hash,
            "baseline prompt",
        ),
        ("repair_prompt_sha256", repair_prompt_hash, "repair prompt"),
        ("task_registry_sha256", task_registry_hash, "task registry"),
    ] {
        if plan_value.get(field).and_then(Value::as_str) != Some(file_hash) {
            errors.push(format!(
                "plan {field} does not match the committed {label} file (pre-calibration hash changed)"
            ));
        }
    }
    errors.extend(check_plan_design_constants(plan_value));
    if plan_value
        .get("proceed_to_evaluation")
        .and_then(Value::as_bool)
        != Some(true)
    {
        errors.push(
            "evaluation plan does not authorize Phase B (proceed_to_evaluation != true)"
                .to_string(),
        );
    }
    let selected: Vec<&str> = plan_value
        .get("selected_families")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|f| f.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();
    if selected.is_empty() || selected.len() > 3 {
        errors.push(format!(
            "evaluation plan must list 2 or 3 calibrated families (found {})",
            selected.len()
        ));
    }

    // --- deserialize ---
    let mut records: Vec<EvaluationRecord> = Vec::with_capacity(record_values.len());
    for (i, v) in record_values.iter().enumerate() {
        match serde_json::from_value::<EvaluationRecord>(v.clone()) {
            Ok(r) => records.push(r),
            Err(e) => errors.push(format!("record[{i}] does not deserialize: {e}")),
        }
    }

    // --- field-level tampering: forbidden keys ---
    for (i, v) in record_values.iter().enumerate() {
        scan_forbidden_keys(v, &format!("_records[{i}]"), &mut errors);
    }

    // --- expected record count derived from the committed plan
    //     (spec §51): held-out tasks × repetitions × 2 conditions ---
    let held_plan: Vec<&str> = plan_value
        .get("heldout_tasks")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|t| t.as_str()).collect())
        .unwrap_or_default();
    let reps_plan = plan_value
        .get("repetitions_per_task")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let expected = held_plan.len() * reps_plan * CONDITIONS.len();
    if record_values.len() != expected {
        errors.push(format!(
            "expected {expected} evaluation records (derived from the committed evaluation plan: {} held-out tasks × {reps_plan} repetitions × 2 conditions), found {}",
            held_plan.len(),
            record_values.len()
        ));
    }
    if held_plan.len() as u32 * reps_plan as u32 != PLANNED_PAIRS {
        errors.push(format!(
            "plan pair budget: {} held-out tasks × {reps_plan} repetitions != {PLANNED_PAIRS} planned pairs",
            held_plan.len()
        ));
    }

    // --- per-record design checks ---
    for (i, r) in records.iter().enumerate() {
        let tag = format!("record[{}]({})", i, r.run_id);
        if r.experiment_id != EXPERIMENT_ID {
            errors.push(format!("{tag}: experiment_id is not {EXPERIMENT_ID:?}"));
        }
        if r.phase != "evaluation" {
            errors.push(format!("{tag}: phase is not 'evaluation'"));
        }
        if !CONDITIONS.contains(&r.condition.as_str()) {
            errors.push(format!("{tag}: unknown condition {:?}", r.condition));
        }
        if !(1..=reps_plan as u32).contains(&r.repetition)
            || !(1..=2).contains(&r.condition_position)
        {
            errors.push(format!("{tag}: repetition/condition_position out of range"));
        }
        if !FAMILIES.contains(&r.family.as_str()) {
            errors.push(format!("{tag}: unregistered family {:?}", r.family));
        }
        match by_id.get(r.task_id.as_str()) {
            Some(t) => {
                if t.split != oracle::TaskSplit::HeldOut {
                    errors.push(format!("{tag}: task {} is not held-out", r.task_id));
                }
                if t.family != r.family {
                    errors.push(format!("{tag}: family does not match registry"));
                }
                if !selected.contains(&t.family.as_str()) {
                    errors.push(format!(
                        "{tag}: task {} belongs to a non-selected family {:?}",
                        r.task_id, t.family
                    ));
                }
                if r.initial_state != t.initial_state {
                    errors.push(format!(
                        "{tag}: initial_state does not match registry (held-out task changed after calibration)"
                    ));
                }
                if r.target_state != t.target_state {
                    errors.push(format!(
                        "{tag}: target_state does not match registry (held-out task changed after calibration)"
                    ));
                }
                let expected_fault =
                    crate::tools::FaultMode::for_stress(&t.fault_key, r.drop_count);
                let fv: Value = serde_json::to_value(&expected_fault).unwrap_or(Value::Null);
                if r.fault_mode != fv {
                    errors.push(format!(
                        "{tag}: fault_mode does not match (fault key, selected stress)"
                    ));
                }
            }
            None => errors.push(format!("{tag}: unknown task {}", r.task_id)),
        }
        if let Some(l) = crate::tools::StressLevel::by_id(&r.selected_stress) {
            if l.drop_count != r.drop_count {
                errors.push(format!(
                    "{tag}: selected_stress {:?} inconsistent with drop_count {}",
                    r.selected_stress, r.drop_count
                ));
            }
            if l.drop_count == 0 {
                errors.push(format!(
                    "{tag}: S0 (the sanity control) may not be a selected evaluation stress"
                ));
            }
        } else {
            errors.push(format!(
                "{tag}: unregistered selected stress level {:?}",
                r.selected_stress
            ));
        }
        // The record's selected stress must equal the plan's.
        let plan_stress = plan_value
            .get("per_family")
            .and_then(Value::as_array)
            .and_then(|a| {
                a.iter()
                    .find(|f| f.get("family").and_then(Value::as_str) == Some(r.family.as_str()))
            })
            .and_then(|f| f.get("selected_stress").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        if plan_stress != r.selected_stress {
            errors.push(format!(
                "{tag}: selected stress {:?} does not match the frozen plan ({plan_stress:?}) for family {}",
                r.selected_stress, r.family
            ));
        }
        if r.baseline_prompt_sha256 != baseline_prompt_hash {
            errors.push(format!(
                "{tag}: baseline_prompt_sha256 does not match the committed baseline prompt"
            ));
        }
        if r.repair_prompt_sha256 != repair_prompt_hash {
            errors.push(format!(
                "{tag}: repair_prompt_sha256 does not match the committed repair prompt (prompt changed after calibration)"
            ));
        }
        if r.evaluation_plan_sha256 != plan_file_sha256 {
            errors.push(format!(
                "{tag}: evaluation_plan_sha256 does not match the committed evaluation plan"
            ));
        }
        if r.task_registry_sha256 != task_registry_hash {
            errors.push(format!(
                "{tag}: task_registry_sha256 does not match the registered task suite"
            ));
        }
        if r.calibration_raw_sha256
            != plan_value
                .get("calibration_raw_sha256")
                .and_then(Value::as_str)
                .unwrap_or("")
        {
            errors.push(format!(
                "{tag}: calibration_raw_sha256 does not match the evaluation plan"
            ));
        }
    }

    // --- pair checks: registered alternating order + same selected
    //     stress for both conditions of a pair ---
    for t in registry
        .iter()
        .filter(|t| t.split == oracle::TaskSplit::HeldOut)
    {
        if !selected.contains(&t.family.as_str()) {
            continue;
        }
        for rep in 1..=reps_plan as u32 {
            let pair: Vec<&EvaluationRecord> = records
                .iter()
                .filter(|r| r.task_id == t.id && r.repetition == rep)
                .collect();
            if pair.len() != 2 {
                errors.push(format!(
                    "task {} rep {rep}: pair has {} records (expected 2)",
                    t.id,
                    pair.len()
                ));
                continue;
            }
            let got: Vec<&str> = pair.iter().map(|r| r.condition.as_str()).collect();
            let want: Vec<&str> = eval_condition_order(rep).to_vec();
            if got != want {
                errors.push(format!(
                    "task {} rep {rep}: condition order {:?} does not match the registered alternating order {:?}",
                    t.id, got, want
                ));
            }
            let stresses: BTreeSet<&str> =
                pair.iter().map(|r| r.selected_stress.as_str()).collect();
            if stresses.len() != 1 {
                errors.push(format!(
                    "task {} rep {rep}: baseline and repair ran under different selected stresses",
                    t.id
                ));
            }
            let positions: BTreeSet<u32> = pair.iter().map(|r| r.condition_position).collect();
            if positions.len() != 2 {
                errors.push(format!(
                    "task {} rep {rep}: condition positions are not 1 and 2",
                    t.id
                ));
            }
        }
    }

    // --- strict sequence + single run prefix ---
    let mut prefix: Option<&str> = None;
    for (i, r) in records.iter().enumerate() {
        let tag = format!("record[{}]", i);
        let expected_seq = (i as u32) + 1;
        if r.sequence != expected_seq {
            errors.push(format!(
                "{tag}: sequence {} is not strict (expected {expected_seq})",
                r.sequence
            ));
        }
        match r.run_id.rsplit_once('-').map(|(p, _)| p) {
            None => errors.push(format!("{tag}: run_id has no sequence suffix")),
            Some(p) => {
                let suffix = r.run_id.len().saturating_sub(p.len()).saturating_sub(1);
                if suffix == 0 {
                    errors.push(format!("{tag}: run_id has no sequence suffix"));
                } else {
                    let seq_text = r.run_id[p.len() + 1..].to_string();
                    if seq_text.parse::<u32>().ok() != Some(expected_seq) {
                        errors.push(format!(
                            "{tag}: run_id sequence does not match strict order"
                        ));
                    }
                    match prefix {
                        None => prefix = Some(p),
                        Some(prev) if prev != p => {
                            errors.push(format!("{tag}: run prefix {p:?} differs from {prev:?}"))
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // --- provenance consistency across records ---
    let mut consistent = |get: fn(&EvaluationRecord) -> &str, name: &str| {
        let mut set: BTreeMap<&str, usize> = BTreeMap::new();
        for r in &records {
            *set.entry(get(r)).or_insert(0) += 1;
        }
        if set.len() > 1 {
            errors.push(format!(
                "field {name:?} differs across records ({} values)",
                set.len()
            ));
        }
    };
    consistent(|r| &r.model, "model");
    consistent(|r| &r.redacted_endpoint, "redacted_endpoint");
    consistent(|r| &r.final_eval_commit, "final_eval_commit");
    consistent(|r| &r.baseline_prompt_sha256, "baseline_prompt_sha256");
    consistent(|r| &r.repair_prompt_sha256, "repair_prompt_sha256");
    consistent(|r| &r.calibration_raw_sha256, "calibration_raw_sha256");
    consistent(|r| &r.evaluation_plan_sha256, "evaluation_plan_sha256");
    consistent(|r| &r.task_registry_sha256, "task_registry_sha256");
    if !records.is_empty() {
        let mut temps: Vec<f64> = Vec::new();
        for r in &records {
            if !temps.iter().any(|t| t == &r.temperature) {
                temps.push(r.temperature);
            }
        }
        if temps.len() > 1 {
            errors.push(format!("temperature differs across records ({temps:?})"));
        }
        let commit = &records[0].final_eval_commit;
        if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
            errors.push("final_eval_commit is not a 40-hex git SHA".to_string());
        }
    }

    // --- per-record structural checks (usage, calls, audit, oracle) ---
    for (i, v) in record_values.iter().enumerate() {
        let tag = format!("record[{}]", i);
        errors.extend(check_record_fields(&tag, v, registry));
    }

    // --- summary deep-equality (gates, comparison, conclusion included) ---
    if !records.is_empty() {
        let plan = plan_value.clone().into();
        let recomputed = compute_evaluation_summary(&records, registry, &plan);
        let recomputed_value: Value =
            serde_json::from_str(&serde_json::to_string(&recomputed).unwrap())
                .unwrap_or(Value::Null);
        if canonical_json(&recomputed_value) != canonical_json(summary_value) {
            errors.push(
                "evaluation summary does not deep-equal the recomputed summary (gates/conclusion recomputation failed)"
                    .to_string(),
            );
        }
    }

    errors
}

// ===========================================================================
// Synthetic network-free datasets (self-test + verifier regression tests)
// ===========================================================================

/// The value of one state key in a registered state object.
fn key_value(state: &Value, key: &str) -> String {
    state
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// Simulate one synthetic episode with the REAL tool environment (fresh
/// `ToolExecutor` under the registered fault): `write_count` writes of
/// `write_value` to the fault key, each followed by a read, then a
/// final answer turn. Returns consistent requested/executed/write-audit/
/// final-state records. The write plan stays inside the registered
/// limits (≤ 5 writes + ≤ 5 reads + 1 turn ≤ max_tool_calls 16).
fn synthetic_run(
    task: &TaskSpec,
    level: &crate::tools::StressLevel,
    write_value: &str,
    write_count: u32,
) -> (
    Vec<RequestedToolCall>,
    Vec<ExecutedToolCall>,
    Vec<WriteAttempt>,
    Value,
    u32,
) {
    let fault = crate::tools::FaultMode::for_stress(&task.fault_key, level.drop_count);
    let mut exec = crate::tools::ToolExecutor::fresh(&task.initial_state, fault);
    let key = task.fault_key.clone();
    let mut requested = Vec::new();
    let mut executed = Vec::new();
    let mut turn = 0u32;
    for _ in 0..write_count {
        let args = json!({ "key": key, "value": write_value });
        let res = exec
            .execute("state_write", &args)
            .expect("synthetic write must execute");
        turn += 1;
        requested.push(RequestedToolCall {
            sequence: requested.len() as u32,
            turn,
            tool_call_id: format!("call-{turn}"),
            tool_name: "state_write".into(),
            arguments: args.clone(),
        });
        executed.push(ExecutedToolCall {
            sequence: executed.len() as u32,
            turn,
            tool_call_id: format!("call-{turn}"),
            tool_name: "state_write".into(),
            arguments: args,
            result: res.payload,
            duration_us: 1_000,
        });
        let rargs = json!({ "key": key });
        let rres = exec
            .execute("state_read", &rargs)
            .expect("synthetic read must execute");
        turn += 1;
        requested.push(RequestedToolCall {
            sequence: requested.len() as u32,
            turn,
            tool_call_id: format!("call-{turn}"),
            tool_name: "state_read".into(),
            arguments: rargs.clone(),
        });
        executed.push(ExecutedToolCall {
            sequence: executed.len() as u32,
            turn,
            tool_call_id: format!("call-{turn}"),
            tool_name: "state_read".into(),
            arguments: rargs,
            result: rres.payload,
            duration_us: 1_000,
        });
    }
    turn += 1; // final answer turn
    (
        requested,
        executed,
        exec.write_attempts().to_vec(),
        exec.final_state_value(),
        turn,
    )
}

/// Synthetic per-request usage block (arithmetically consistent; the
/// provider fields are all reported so the null branches are exercised
/// elsewhere in protocol tests).
fn synthetic_requests(turns: u32) -> Vec<ModelRequestRecord> {
    (1..=turns)
        .map(|turn| {
            let prompt = 500u64 + 100 * turn as u64;
            let cached = if turn == 1 { 0u64 } else { 300 };
            let completion = 10u64 * turn as u64;
            ModelRequestRecord {
                turn,
                finish_reason: Some(if turn < turns {
                    "tool_calls".to_string()
                } else {
                    "stop".to_string()
                }),
                prompt_tokens: Some(prompt),
                completion_tokens: Some(completion),
                total_tokens: Some(prompt + completion),
                cached_prompt_tokens: Some(cached),
                reasoning_tokens: if turn == turns { Some(5) } else { None },
                uncached_prompt_tokens: Some(prompt - cached),
            }
        })
        .collect()
}

/// Synthetic calibration success schedule (per development task, per
/// stress level, per repetition). Deterministic; designed so that two
/// of the three families calibrate and one does not:
///
/// - direct_set (D1: 5,2,2,1,0 / D2: 5,1,2,0,0): S1 eligible
///   (both ≤ 2) → select S1
/// - conditional_set (D3: 5,3,2,0,0 / D4: 5,3,1,0,0): S1 not eligible
///   (3 > 2 on both), S2 eligible (2, 1) → select S2
/// - replacement (D5: 5,... / D6: S0 3/5): S0 sanity fails on D6 →
///   calibration_invalid
///
/// (lists are per-task S0..S4 success counts)
fn synthetic_calibration_success(task_id: &str, stress_index: u32, repetition: u32) -> bool {
    let r = repetition - 1; // 0..4
    let counts: [u32; 5] = match task_id {
        "D1" => [5, 2, 2, 1, 0],
        "D2" => [5, 1, 2, 0, 0],
        "D3" => [5, 3, 2, 0, 0],
        "D4" => [5, 3, 1, 0, 0],
        "D5" => [5, 0, 0, 0, 0],
        _ => [3, 0, 0, 0, 0], // D6: S0 3/5 → sanity gate fails
    };
    r < counts[stress_index as usize]
}

/// Build the full deterministic 150-record synthetic calibration
/// dataset.
pub fn synthetic_calibration_records(registry: &[TaskSpec]) -> Vec<CalibrationRecord> {
    let task_hash = crate::experiment::task_registry_sha256(registry).unwrap_or_default();
    let stress_hash = crate::experiment::stress_registry_sha256(
        &crate::tools::STRESS_LEVELS
            .iter()
            .map(|l| crate::tools::RawStressLevel {
                id: l.id.to_string(),
                drop_count: l.drop_count,
            })
            .collect::<Vec<_>>(),
    )
    .unwrap_or_default();
    let mut records = Vec::with_capacity(CALIBRATION_EPISODES);
    let mut seq = 0u32;
    for family in FAMILIES {
        for task in development_tasks_for(family, registry) {
            for rep in 1..=CALIBRATION_REPETITIONS {
                for pos in 1..=CALIBRATION_STRESS_LEVELS {
                    seq += 1;
                    let level =
                        crate::tools::STRESS_LEVELS[calibration_stress_index(rep, pos) as usize];
                    let success = synthetic_calibration_success(&task.id, level.drop_count, rep);
                    let target_value = key_value(&task.target_state, &task.fault_key);
                    let initial_value = key_value(&task.initial_state, &task.fault_key);
                    let (write_count, write_value) = if success {
                        (level.drop_count + 1, target_value)
                    } else {
                        (level.drop_count.max(1), initial_value)
                    };
                    let (requested, executed, write_attempts, final_state, turns) =
                        synthetic_run(&task, &level, &write_value, write_count);
                    let requests = synthetic_requests(turns);
                    let eval = oracle::evaluate_task(
                        &task,
                        &oracle::OracleInput {
                            termination_reason: "completed".to_string(),
                            initial_state: task.initial_state.clone(),
                            final_state: final_state.clone(),
                        },
                    );
                    debug_assert_eq!(eval.success, success);
                    let tool_calls = executed.len() as u32;
                    records.push(CalibrationRecord {
                        experiment_id: EXPERIMENT_ID.to_string(),
                        phase: "calibration".to_string(),
                        run_id: format!("exp0009-selftest-cal-{seq:03}"),
                        sequence: seq,
                        family: family.to_string(),
                        task_id: task.id.clone(),
                        stress_level: level.id.to_string(),
                        drop_count: level.drop_count,
                        repetition: rep,
                        execution_position: pos,
                        condition: "baseline".to_string(),
                        model: "selftest-synthetic".to_string(),
                        temperature: TEMPERATURE,
                        redacted_endpoint: "http://selftest.invalid/v1".to_string(),
                        code_under_test_commit: "0".repeat(40),
                        baseline_prompt_sha256: "1".repeat(64),
                        repair_prompt_sha256: "2".repeat(64),
                        task_registry_sha256: task_hash.clone(),
                        stress_registry_sha256: stress_hash.clone(),
                        initial_state: task.initial_state.clone(),
                        target_state: task.target_state.clone(),
                        fault_mode: serde_json::to_value(crate::tools::FaultMode::for_stress(
                            &task.fault_key,
                            level.drop_count,
                        ))
                        .unwrap_or(Value::Null),
                        model_requests: requests,
                        requested_tool_calls: requested,
                        executed_tool_calls: executed,
                        environment_write_attempts: write_attempts,
                        final_state,
                        final_answer: Some(
                            "The requested external state has been achieved.".to_string(),
                        ),
                        model_turn_count: turns,
                        tool_call_count: tool_calls,
                        agent_failure: None,
                        infrastructure_failure: None,
                        usage_accounting_error: None,
                        termination_reason: "completed".to_string(),
                        oracle_success: eval.success,
                        oracle_failure_reasons: eval.reasons,
                        timing: Timing {
                            wall_time_ms: 1_000 + 500 * turns as u64,
                            model_wait_time_ms: 800 * turns as u64,
                            tool_execution_time_us: 1_000 * tool_calls as u64,
                            kernel_overhead_estimate_ms: 100 * turns as u64,
                            approximate: true,
                        },
                    });
                }
            }
        }
    }
    records
}

/// Synthetic evaluation success schedule (per held-out task / rep /
/// condition), for the 12-repetition two-family design:
///
/// - H1: baseline ≤ 4, repair ≤ 8  → 4 wins, 8 ties
/// - H2: baseline ≤ 2, repair 2..6 (fail rep 1) → 4 wins, 1 loss, 7 ties
/// - H3: baseline ≤ 3, repair ≤ 7  → 4 wins, 8 ties
/// - H4: baseline ≤ 2, repair 2..6 (fail rep 1) → 4 wins, 1 loss, 7 ties
///
/// Pooled: 16 wins / 2 losses / 30 ties → 18 informative pairs,
/// exact sign test p ≈ 0.0012 → quality_improvement.
/// Baseline failures among valid pairs: 8+10+9+10 = 37 ≥ 12
/// (potential information capacity gate passes).
fn synthetic_eval_outcome(task_id: &str, repetition: u32, condition: &str) -> bool {
    let rep = repetition;
    match (task_id, condition) {
        ("H1", "baseline") => (1..=4).contains(&rep),
        ("H1", "repair") => (1..=8).contains(&rep),
        ("H2", "baseline") => (1..=2).contains(&rep),
        ("H2", "repair") => (2..=6).contains(&rep),
        ("H3", "baseline") => (1..=3).contains(&rep),
        ("H3", "repair") => (1..=7).contains(&rep),
        ("H4", "baseline") => (1..=2).contains(&rep),
        ("H4", "repair") => (2..=6).contains(&rep),
        _ => false,
    }
}

/// Build the full synthetic Phase B dataset for the given plan
/// (96 episodes for the two-family design; 96 likewise for three).
pub fn synthetic_evaluation_records(
    registry: &[TaskSpec],
    plan: &EvaluationPlan,
) -> Vec<EvaluationRecord> {
    let task_hash = crate::experiment::task_registry_sha256(registry).unwrap_or_default();
    let selected: Vec<&str> = plan.selected_families.iter().map(String::as_str).collect();
    let reps = if plan.repetitions_per_task == 0 {
        eval_repetitions_for(plan.calibrated_family_count).unwrap_or(1)
    } else {
        plan.repetitions_per_task
    };
    let mut records = Vec::new();
    let mut seq = 0u32;
    for family in FAMILIES {
        if !selected.contains(&family) {
            continue;
        }
        let sel_stress = plan
            .per_family
            .iter()
            .find(|f| f.family == family)
            .and_then(|f| f.selected_stress.clone())
            .unwrap_or_else(|| "S2".to_string());
        let level =
            crate::tools::StressLevel::by_id(&sel_stress).unwrap_or(crate::tools::STRESS_LEVELS[2]);
        for task in heldout_tasks_for(family, registry) {
            for rep in 1..=reps {
                for (pos, condition) in eval_condition_order(rep).iter().enumerate() {
                    seq += 1;
                    let success = synthetic_eval_outcome(&task.id, rep, condition);
                    let target_value = key_value(&task.target_state, &task.fault_key);
                    let initial_value = key_value(&task.initial_state, &task.fault_key);
                    let (write_count, write_value) = if success {
                        (level.drop_count + 1, target_value)
                    } else {
                        (level.drop_count.max(1), initial_value)
                    };
                    let (requested, executed, write_attempts, final_state, turns) =
                        synthetic_run(&task, &level, &write_value, write_count);
                    let requests = synthetic_requests(turns);
                    let eval = oracle::evaluate_task(
                        &task,
                        &oracle::OracleInput {
                            termination_reason: "completed".to_string(),
                            initial_state: task.initial_state.clone(),
                            final_state: final_state.clone(),
                        },
                    );
                    debug_assert_eq!(eval.success, success);
                    let tool_calls = executed.len() as u32;
                    records.push(EvaluationRecord {
                        experiment_id: EXPERIMENT_ID.to_string(),
                        phase: "evaluation".to_string(),
                        run_id: format!("exp0009-selftest-eval-{seq:03}"),
                        sequence: seq,
                        family: family.to_string(),
                        task_id: task.id.clone(),
                        selected_stress: level.id.to_string(),
                        drop_count: level.drop_count,
                        condition: condition.to_string(),
                        repetition: rep,
                        condition_position: (pos + 1) as u32,
                        calibration_raw_sha256: "a".repeat(64),
                        evaluation_plan_sha256: "b".repeat(64),
                        model: "selftest-synthetic".to_string(),
                        temperature: TEMPERATURE,
                        redacted_endpoint: "http://selftest.invalid/v1".to_string(),
                        final_eval_commit: "0".repeat(40),
                        baseline_prompt_sha256: "1".repeat(64),
                        repair_prompt_sha256: "2".repeat(64),
                        task_registry_sha256: task_hash.clone(),
                        initial_state: task.initial_state.clone(),
                        target_state: task.target_state.clone(),
                        fault_mode: serde_json::to_value(crate::tools::FaultMode::for_stress(
                            &task.fault_key,
                            level.drop_count,
                        ))
                        .unwrap_or(Value::Null),
                        model_requests: requests,
                        requested_tool_calls: requested,
                        executed_tool_calls: executed,
                        environment_write_attempts: write_attempts,
                        final_state,
                        final_answer: Some(
                            "The requested external state has been achieved.".to_string(),
                        ),
                        model_turn_count: turns,
                        tool_call_count: tool_calls,
                        agent_failure: None,
                        infrastructure_failure: None,
                        usage_accounting_error: None,
                        termination_reason: "completed".to_string(),
                        oracle_success: eval.success,
                        oracle_failure_reasons: eval.reasons,
                        timing: Timing {
                            wall_time_ms: 1_000 + 500 * turns as u64,
                            model_wait_time_ms: 800 * turns as u64,
                            tool_execution_time_us: 1_000 * tool_calls as u64,
                            kernel_overhead_estimate_ms: 100 * turns as u64,
                            approximate: true,
                        },
                    });
                }
            }
        }
    }
    records
}

// ===========================================================================
// Tests (spec §53 tamper matrix, both phases)
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Vec<TaskSpec> {
        crate::experiment::load_tasks(&crate::experiment::crate_dir().join("tasks.json"))
            .expect("tasks.json must load")
            .tasks
    }

    fn cal_values(records: &[CalibrationRecord]) -> Vec<Value> {
        records
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn eval_values(records: &[EvaluationRecord]) -> Vec<Value> {
        records
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn hash_inputs(reg: &[TaskSpec]) -> (String, String, String) {
        (
            "1".repeat(64),
            crate::experiment::task_registry_sha256(reg).unwrap(),
            crate::experiment::stress_registry_sha256(
                &crate::tools::STRESS_LEVELS
                    .iter()
                    .map(|l| crate::tools::RawStressLevel {
                        id: l.id.to_string(),
                        drop_count: l.drop_count,
                    })
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        )
    }

    fn synthetic_plan(reg: &[TaskSpec], records: &[CalibrationRecord]) -> EvaluationPlan {
        let (_bh, th, sh) = hash_inputs(reg);
        let prov = PlanProvenance {
            code_under_test_commit: "0".repeat(40),
            baseline_prompt_sha256: "1".repeat(64),
            repair_prompt_sha256: "2".repeat(64),
            task_registry_sha256: th,
            stress_registry_sha256: sh,
        };
        let mut p = recompute_evaluation_plan(records, reg, &prov);
        p.calibration_raw_sha256 = "a".repeat(64);
        p
    }

    fn verify_cal(
        records: &[CalibrationRecord],
        summary: &CalibrationSummary,
        plan: &EvaluationPlan,
        reg: &[TaskSpec],
    ) -> Vec<String> {
        let (bh, th, sh) = hash_inputs(reg);
        verify_calibration_artifacts(
            &cal_values(records),
            &serde_json::to_value(summary).unwrap(),
            &serde_json::to_value(plan).unwrap(),
            reg,
            &bh,
            &"2".repeat(64), // records carry the frozen repair hash "2"*64.
            &th,
            &sh,
            &"a".repeat(64),
        )
    }

    fn verify_eval(
        records: &[EvaluationRecord],
        summary: &EvaluationSummary,
        plan: &EvaluationPlan,
        reg: &[TaskSpec],
    ) -> Vec<String> {
        let (_bh, th, _sh) = hash_inputs(reg);
        verify_evaluation_artifacts(
            &eval_values(records),
            &serde_json::to_value(summary).unwrap(),
            &serde_json::to_value(plan).unwrap(),
            reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &th,
            &"b".repeat(64),
        )
    }

    // ------------------------------------------------------------------
    // Phase A
    // ------------------------------------------------------------------

    #[test]
    fn synthetic_calibration_design_selection() {
        let reg = registry();
        let records = synthetic_calibration_records(&reg);
        assert_eq!(records.len(), CALIBRATION_EPISODES);
        assert_eq!(CALIBRATION_EPISODES, 150);
        let plan = synthetic_plan(&reg, &records);
        assert_eq!(plan.calibrated_family_count, 2);
        assert!(plan.proceed_to_evaluation);
        assert_eq!(plan.planned_pairs, 48);
        assert_eq!(plan.repetitions_per_task, 12);
        assert_eq!(
            plan.heldout_tasks,
            vec![
                "H1".to_string(),
                "H2".to_string(),
                "H3".to_string(),
                "H4".to_string()
            ]
        );
        let direct = plan
            .per_family
            .iter()
            .find(|f| f.family == "direct_set")
            .unwrap();
        assert_eq!(direct.selected_stress.as_deref(), Some("S1"));
        assert_eq!(direct.selection_status, "calibrated");
        let cond = plan
            .per_family
            .iter()
            .find(|f| f.family == "conditional_set")
            .unwrap();
        assert_eq!(cond.selected_stress.as_deref(), Some("S2"));
        let rep = plan
            .per_family
            .iter()
            .find(|f| f.family == "replacement")
            .unwrap();
        assert_eq!(rep.selection_status, "calibration_invalid");
        assert_eq!(rep.selected_stress, None);
        // Power plan values (spec §29–30): computed, not hardcoded.
        let power_at_min = power::conditional_detection_power(MIN_ACTUAL_INFORMATIVE, 0.90);
        assert!((power_at_min - plan.conditional_power_at_min_informative).abs() < 1e-12);
        assert!(power_at_min >= 0.85);
    }

    #[test]
    fn synthetic_calibration_verifier_passes() {
        let reg = registry();
        let records = synthetic_calibration_records(&reg);
        let summary = compute_calibration_summary(&records, &reg);
        assert!(summary.artifacts_complete);
        assert!(summary.proceed_to_evaluation);
        let plan = synthetic_plan(&reg, &records);
        let errors = verify_cal(&records, &summary, &plan, &reg);
        assert!(errors.is_empty(), "unexpected: {errors:?}");
    }

    #[test]
    fn verifier_fails_on_missing_development_cell() {
        let reg = registry();
        let mut records = synthetic_calibration_records(&reg);
        records.pop(); // 149 records
        let summary = compute_calibration_summary(&records, &reg);
        let plan = synthetic_plan(&reg, &records);
        let errors = verify_cal(&records, &summary, &plan, &reg);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("expected exactly") || e.contains("missing development cell")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_duplicate_development_cell() {
        let reg = registry();
        let mut records = synthetic_calibration_records(&reg);
        records.push(records[0].clone()); // 151 records
        let summary = compute_calibration_summary(&records, &reg);
        let plan = synthetic_plan(&reg, &records);
        let errors = verify_cal(&records, &summary, &plan, &reg);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("expected exactly") || e.contains("duplicate development cell")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_candidate_record_inserted_into_calibration() {
        let reg = registry();
        let mut records = synthetic_calibration_records(&reg);
        records.last_mut().unwrap().condition = "repair".into();
        let summary = compute_calibration_summary(&records, &reg);
        let plan = synthetic_plan(&reg, &records);
        let errors = verify_cal(&records, &summary, &plan, &reg);
        assert!(
            errors.iter().any(|e| e.contains("baseline-only")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_candidate_outcome_key_in_calibration() {
        let reg = registry();
        let records = synthetic_calibration_records(&reg);
        let summary = compute_calibration_summary(&records, &reg);
        let plan = synthetic_plan(&reg, &records);
        let mut v = cal_values(&records);
        v.last_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("repair_success".into(), Value::Bool(true));
        let errors = verify_cal_custom_values(&records, &summary, &plan, &reg, &v);
        assert!(
            errors.iter().any(|e| e.contains("forbidden field")),
            "{errors:?}"
        );
    }

    fn verify_cal_custom_values(
        _records: &[CalibrationRecord],
        summary: &CalibrationSummary,
        plan: &EvaluationPlan,
        reg: &[TaskSpec],
        values: &[Value],
    ) -> Vec<String> {
        let (bh, th, sh) = hash_inputs(reg);
        verify_calibration_artifacts(
            values,
            &serde_json::to_value(summary).unwrap(),
            &serde_json::to_value(plan).unwrap(),
            reg,
            &bh,
            &"2".repeat(64),
            &th,
            &sh,
            &"a".repeat(64),
        )
    }

    #[test]
    fn verifier_fails_on_wrong_s0_result() {
        let reg = registry();
        let records = synthetic_calibration_records(&reg);
        let summary = compute_calibration_summary(&records, &reg);
        let plan = synthetic_plan(&reg, &records);
        // Tamper the raw records after the summary/plan were made: flip
        // an S0 success to a failure.
        let mut v = cal_values(&records);
        let idx = v
            .iter()
            .position(|x| x["stress_level"] == "S0" && x["oracle_success"] == true)
            .unwrap();
        v[idx]["oracle_success"] = Value::from(false);
        let errors = verify_cal_custom_values(&records, &summary, &plan, &reg, &v);
        assert!(!errors.is_empty(), "tampered calibration must not verify");
        assert!(
            errors
                .iter()
                .any(|e| e.contains("oracle_success") || e.contains("deep-equal")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_cyclic_order_violation() {
        let reg = registry();
        let mut records = synthetic_calibration_records(&reg);
        // Swap the stress levels of two adjacent records of the same
        // family/task/repetition.
        let a = records[0].clone();
        let b = records[1].clone();
        let tmp = a.stress_level.clone();
        let tmp_dc = a.drop_count;
        records[0].stress_level = b.stress_level.clone();
        records[0].drop_count = b.drop_count;
        records[1].stress_level = tmp;
        records[1].drop_count = tmp_dc;
        let summary = compute_calibration_summary(&records, &reg);
        let plan = synthetic_plan(&reg, &records);
        let errors = verify_cal(&records, &summary, &plan, &reg);
        assert!(
            errors.iter().any(|e| e.contains("cyclic order violation")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_tampered_plan_selection() {
        let reg = registry();
        let records = synthetic_calibration_records(&reg);
        let summary = compute_calibration_summary(&records, &reg);
        let mut plan = synthetic_plan(&reg, &records);
        if let Some(d) = plan
            .per_family
            .iter_mut()
            .find(|f| f.family == "direct_set")
        {
            d.selected_stress = Some("S4".into()); // higher than the eligible S1
        }
        let errors = verify_cal(&records, &summary, &plan, &reg);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("does not deep-equal the plan recomputed")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_selection_using_only_one_development_task() {
        let reg = registry();
        let records = synthetic_calibration_records(&reg);
        let summary = compute_calibration_summary(&records, &reg);
        let mut plan = synthetic_plan(&reg, &records);
        // Zero out task B's counts: a single-task selection would have
        // chosen S1 even though B is 3/5 there — the two-variant rule
        // must disagree with the tampered plan.
        if let Some(d) = plan
            .per_family
            .iter_mut()
            .find(|f| f.family == "direct_set")
        {
            if let Some(b) = d.per_task_per_stress_success_counts.get_mut(1) {
                b.s1_success = 3;
                b.s2_success = 3;
            }
        }
        let errors = verify_cal(&records, &summary, &plan, &reg);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("does not deep-equal the plan recomputed")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_repair_prompt_hash_change() {
        let reg = registry();
        let records = synthetic_calibration_records(&reg);
        let summary = compute_calibration_summary(&records, &reg);
        let plan = synthetic_plan(&reg, &records);
        let mut v = cal_values(&records);
        v.last_mut().unwrap()["repair_prompt_sha256"] = Value::from("3".repeat(64));
        let errors = verify_cal_custom_values(&records, &summary, &plan, &reg, &v);
        assert!(
            errors.iter().any(|e| e.contains("repair_prompt_sha256")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_task_registry_hash_change() {
        let reg = registry();
        let records = synthetic_calibration_records(&reg);
        let summary = compute_calibration_summary(&records, &reg);
        let plan = synthetic_plan(&reg, &records);
        let mut v = cal_values(&records);
        v.last_mut().unwrap()["task_registry_sha256"] = Value::from("9".repeat(64));
        let errors = verify_cal_custom_values(&records, &summary, &plan, &reg, &v);
        assert!(
            errors.iter().any(|e| e.contains("task_registry_sha256")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_plan_raw_hash_change() {
        let reg = registry();
        let records = synthetic_calibration_records(&reg);
        let summary = compute_calibration_summary(&records, &reg);
        let plan = synthetic_plan(&reg, &records);
        let errors = verify_calibration_artifacts(
            &cal_values(&records),
            &serde_json::to_value(&summary).unwrap(),
            &serde_json::to_value(&plan).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &hash_inputs(&reg).2,
            &"wrong".repeat(64), // a different calibration artifact hash
        );
        assert!(
            errors.iter().any(|e| e.contains("calibration_raw_sha256")),
            "{errors:?}"
        );
    }

    #[test]
    fn plan_stops_when_only_one_family_calibrates() {
        let reg = registry();
        let mut records = synthetic_calibration_records(&reg);
        // Make direct_set uncalibrated: every S1..S4 has at least one
        // development variant above 40% baseline success.
        for r in records.iter_mut() {
            if r.family == "direct_set" && r.drop_count > 0 {
                let want = if r.task_id == "D1" {
                    3 // 3/5 at every S1..S4
                } else {
                    match r.drop_count {
                        1 => 3,
                        2 => 0,
                        _ => 3,
                    }
                };
                let success = r.repetition <= want;
                r.oracle_success = success;
                r.final_state = if success {
                    r.target_state.clone()
                } else {
                    r.initial_state.clone()
                };
                r.oracle_failure_reasons = if success {
                    Vec::new()
                } else {
                    vec!["state".to_string()]
                };
            }
        }
        let summary = compute_calibration_summary(&records, &reg);
        let plan = synthetic_plan(&reg, &records);
        assert_eq!(plan.calibrated_family_count, 1);
        assert!(!plan.proceed_to_evaluation);
        assert_eq!(plan.repetitions_per_task, 0);
        assert!(plan.heldout_tasks.is_empty());
        assert_eq!(plan.planned_pairs, 48); // the budget constant is unchanged
        assert!(!summary.proceed_to_evaluation);
    }

    // ------------------------------------------------------------------
    // Phase B
    // ------------------------------------------------------------------

    fn eval_fixtures(
        reg: &[TaskSpec],
    ) -> (Vec<EvaluationRecord>, EvaluationSummary, EvaluationPlan) {
        let cal = synthetic_calibration_records(reg);
        let plan = synthetic_plan(reg, &cal);
        let records = synthetic_evaluation_records(reg, &plan);
        let summary = compute_evaluation_summary(&records, reg, &plan);
        (records, summary, plan)
    }

    #[test]
    fn synthetic_evaluation_design() {
        let reg = registry();
        let (records, summary, plan) = eval_fixtures(&reg);
        assert_eq!(records.len(), EVAL_EPISODES);
        assert_eq!(EVAL_EPISODES, 96);
        assert_eq!(plan.repetitions_per_task, 12);
        assert!(summary.artifacts_complete);
        assert_eq!(summary.episodes_expected, 96);
        assert_eq!(summary.comparison.expected_pairs, 48);
        assert_eq!(summary.comparison.valid_pairs, 48);
        assert_eq!(summary.comparison.missing_pairs, 0);
        let pot = &summary.potential_information;
        assert_eq!(pot.potential_information_capacity, 37);
        assert!(pot.sufficient);
        let c = &summary.comparison;
        assert_eq!(c.informative_pairs, 18);
        assert_eq!(c.wins, 16);
        assert_eq!(c.losses, 2);
        assert_eq!(c.ties, 30);
        assert!(c.sign_test_p < 0.002, "p={}", c.sign_test_p);
        assert_eq!(c.classification, "quality_improvement");
        assert!(summary.gates.valid_pairs_sufficient);
        assert!(summary.gates.potential_information_sufficient);
        assert!(summary.gates.actual_informative_sufficient);
        assert_eq!(summary.conclusion.result, "supported");
    }

    #[test]
    fn synthetic_evaluation_verifier_passes() {
        let reg = registry();
        let (records, summary, plan) = eval_fixtures(&reg);
        let errors = verify_eval(&records, &summary, &plan, &reg);
        assert!(errors.is_empty(), "unexpected: {errors:?}");
    }

    fn verify_eval_custom(
        values: &[Value],
        summary: &EvaluationSummary,
        plan: &EvaluationPlan,
        reg: &[TaskSpec],
    ) -> Vec<String> {
        verify_eval_value(values, &serde_json::to_value(summary).unwrap(), plan, reg)
    }

    fn verify_eval_value(
        values: &[Value],
        summary: &Value,
        plan: &EvaluationPlan,
        reg: &[TaskSpec],
    ) -> Vec<String> {
        let (_bh, th, _sh) = hash_inputs(reg);
        verify_evaluation_artifacts(
            values,
            summary,
            &serde_json::to_value(plan).unwrap(),
            reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &th,
            &"b".repeat(64),
        )
    }

    fn tamper_eval(records: &[EvaluationRecord], f: impl Fn(&mut Value)) -> Vec<Value> {
        let mut v = eval_values(records);
        f(v.last_mut().unwrap());
        v
    }

    #[test]
    fn verifier_fails_on_repair_prompt_hash_change_in_records() {
        let reg = registry();
        let (records, summary, plan) = eval_fixtures(&reg);
        let v = tamper_eval(&records, |r| {
            r["repair_prompt_sha256"] = Value::from("3".repeat(64));
        });
        let errors = verify_eval_custom(&v, &summary, &plan, &reg);
        assert!(
            errors.iter().any(|e| e.contains("repair_prompt_sha256")),
            "{errors:?}"
        );
    }

    #[test]
    fn summary_tampers_fail_deep_equality() {
        let reg = registry();
        let (records, summary, plan) = eval_fixtures(&reg);
        let mut sv = serde_json::to_value(&summary).unwrap();
        sv["potential_information"]["potential_information_capacity"] = Value::from(99u64);
        let e1 = verify_eval_custom(&eval_values(&records), &summary, &plan, &reg);
        assert!(e1.is_empty(), "untampered summary must verify: {e1:?}");
        let e2 = verify_eval_value(&eval_values(&records), &sv, &plan, &reg);
        assert!(e2.iter().any(|e| e.contains("deep-equal")), "{e2:?}");
        // informative / wins / losses / p / conclusion.
        for field in [
            ("comparison", "informative_pairs", Value::from(99u64)),
            ("comparison", "wins", Value::from(99u64)),
            ("comparison", "losses", Value::from(99u64)),
            ("comparison", "sign_test_p", Value::from(0.75)),
            ("conclusion", "result", Value::from("refuted")),
        ] {
            let mut sv = serde_json::to_value(&summary).unwrap();
            sv[field.0][field.1] = field.2.clone();
            let e = verify_eval_value(&eval_values(&records), &sv, &plan, &reg);
            assert!(
                e.iter().any(|x| x.contains("deep-equal")),
                "tampering {}.{} must fail: {e:?}",
                field.0,
                field.1
            );
        }
    }

    #[test]
    fn verifier_fails_when_cache_exceeds_prompt() {
        let reg = registry();
        let (records, summary, plan) = eval_fixtures(&reg);
        let v = tamper_eval(&records, |r| {
            let reqs = r["model_requests"].as_array_mut().unwrap();
            let q = reqs.first_mut().unwrap();
            let p = q["prompt_tokens"].as_u64().unwrap();
            q["cached_prompt_tokens"] = Value::from(p + 5);
            q["uncached_prompt_tokens"] = Value::Null;
        });
        let errors = verify_eval_custom(&v, &summary, &plan, &reg);
        assert!(
            errors.iter().any(|e| e.contains("cached_prompt_tokens")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_reasoning_content_leak() {
        let reg = registry();
        let (records, summary, plan) = eval_fixtures(&reg);
        let v = tamper_eval(&records, |r| {
            r["reasoning_content"] = Value::from("SECRET");
        });
        let errors = verify_eval_custom(&v, &summary, &plan, &reg);
        assert!(
            errors.iter().any(|e| e.contains("forbidden field")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_secret_field_leak() {
        let reg = registry();
        let (records, summary, plan) = eval_fixtures(&reg);
        let v = tamper_eval(&records, |r| {
            r["api_key"] = Value::from("sk-SECRET");
        });
        let errors = verify_eval_custom(&v, &summary, &plan, &reg);
        assert!(
            errors.iter().any(|e| e.contains("forbidden field")),
            "{errors:?}"
        );
    }

    // ------------------------------------------------------------------
    // Gate behavior of the summary computation (spec §40–41)
    // ------------------------------------------------------------------

    #[test]
    fn summary_conclusion_is_refuted_when_direction_fails() {
        let reg = registry();
        let cal = synthetic_calibration_records(&reg);
        let mut plan = synthetic_plan(&reg, &cal);
        // All three families calibrated, 8 reps → 6 tasks × 8 × 2 = 96
        // episodes, 48 pairs.
        for f in &mut plan.per_family {
            f.selection_status = "calibrated".to_string();
            f.s0_sanity_pass = true;
            if f.selected_stress.is_none() {
                f.selected_stress = Some("S2".to_string());
            }
        }
        plan.selected_families = FAMILIES.iter().map(|f| f.to_string()).collect();
        plan.calibrated_family_count = 3;
        plan.repetitions_per_task = 8;
        plan.heldout_tasks = reg
            .iter()
            .filter(|t| t.split == oracle::TaskSplit::HeldOut)
            .map(|t| t.id.clone())
            .collect();
        plan.proceed_to_evaluation = true;
        let records = synthetic_evaluation_records(&reg, &plan);
        let mut records = records;
        // Deterministic regression pattern: of the 48 pairs (task-major,
        // rep-ascending), the first 36 are baseline-wins/repair-failures
        // and the last 12 are repair-wins → 12 wins / 36 losses.
        let selected: Vec<&str> = plan.selected_families.iter().map(String::as_str).collect();
        let mut pair_index = 0u32;
        for t in reg.iter().filter(|t| {
            t.split == oracle::TaskSplit::HeldOut && selected.contains(&t.family.as_str())
        }) {
            for rep in 1..=8 {
                let repair_wins = pair_index >= 36;
                for r in records
                    .iter_mut()
                    .filter(|r| r.task_id == t.id && r.repetition == rep)
                {
                    r.oracle_success = if r.condition == "baseline" {
                        !repair_wins
                    } else {
                        repair_wins
                    };
                    r.oracle_failure_reasons = if r.oracle_success {
                        Vec::new()
                    } else {
                        vec!["state".to_string()]
                    };
                }
                pair_index += 1;
            }
        }
        assert_eq!(pair_index, 48);
        let summary = compute_evaluation_summary(&records, &reg, &plan);
        assert!(summary.gates.actual_informative_sufficient, "{summary:?}");
        assert!(
            summary.gates.potential_information_sufficient,
            "{summary:?}"
        );
        assert_eq!(summary.conclusion.result, "refuted");
        assert_eq!(summary.comparison.valid_pairs, 48);
        assert_eq!(summary.comparison.informative_pairs, 48);
        assert_eq!(summary.comparison.wins, 12);
        assert_eq!(summary.comparison.losses, 36);
        assert!(summary.comparison.sign_test_p <= 0.05);
        assert_eq!(summary.comparison.classification, "quality_regression");
    }

    #[test]
    fn summary_conclusion_is_inconclusive_without_potential_information() {
        let reg = registry();
        let cal = synthetic_calibration_records(&reg);
        let plan = synthetic_plan(&reg, &cal);
        let mut records = synthetic_evaluation_records(&reg, &plan);
        // A ceiling baseline: every baseline episode succeeds → 0
        // baseline failures among valid pairs → potential information
        // capacity 0 < 12 → inconclusive (not "the repair is bad").
        for r in records.iter_mut().filter(|r| r.condition == "baseline") {
            r.oracle_success = true;
            r.oracle_failure_reasons = Vec::new();
        }
        let summary = compute_evaluation_summary(&records, &reg, &plan);
        assert!(!summary.gates.potential_information_sufficient);
        assert_eq!(
            summary.potential_information.potential_information_capacity,
            0
        );
        assert_eq!(summary.conclusion.result, "inconclusive");
        assert!(
            summary
                .conclusion
                .reasons
                .iter()
                .any(|r| r.contains("potential information")),
            "{summary:?}"
        );
    }

    #[test]
    fn summary_conclusion_is_inconclusive_with_too_few_valid_pairs() {
        let reg = registry();
        let cal = synthetic_calibration_records(&reg);
        let plan = synthetic_plan(&reg, &cal);
        let mut records = synthetic_evaluation_records(&reg, &plan);
        // Five baseline episodes hit infrastructure failure → 5 excluded
        // pairs → 43 valid < 44 → inconclusive.
        let to_flip: Vec<usize> = records
            .iter()
            .enumerate()
            .filter(|(_, r)| r.condition == "baseline")
            .map(|(i, _)| i)
            .take(5)
            .collect();
        for i in to_flip {
            records[i].termination_reason = "http_error".into();
            records[i].infrastructure_failure = Some("http_error".to_string());
        }
        let summary = compute_evaluation_summary(&records, &reg, &plan);
        assert!(!summary.gates.valid_pairs_sufficient);
        assert_eq!(summary.comparison.valid_pairs, 43);
        assert_eq!(summary.conclusion.result, "inconclusive");
    }
}
