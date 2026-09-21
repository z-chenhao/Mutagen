//! Artifact types, design constants, summaries, verifiers, and the
//! deterministic network-free synthetic datasets (for `self-test` and
//! verifier regression tests).
//!
//! The kernel is the authoritative execution recorder: every execution
//! field of a record originates from the Rust kernel's own in-episode
//! records. Raw artifacts are never reconstructed from server logs.
//!
//! Experiment 0008 has two artifacts with two record shapes:
//!
//! - Phase A (calibration): `CalibrationRecord` — baseline-only episodes
//!   over the three registered families × five stress levels × five
//!   repetitions, in the registered cyclic stress order.
//! - Phase B (evaluation): `EvaluationRecord` — baseline vs
//!   candidate_repair on the selected families' held-out tasks, in the
//!   registered alternating condition order.
//!
//! The design constants (episode counts, orders, gates) are hardcoded
//! here and are NEVER inferred from artifacts (the expected Phase B
//! size is derived from the committed selection manifest, spec §41).

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;

use crate::calibration::DifficultySelection;
use crate::oracle::{self, TaskSpec};
use crate::protocol::{TEMPERATURE, canonical_json};
use crate::stats;
use crate::tools::WriteAttempt;

// ===========================================================================
// Design constants (hardcoded, never inferred from artifacts)
// ===========================================================================

pub const EXPERIMENT_ID: &str = "0008";

/// The three registered structural families (spec §23), in registry order.
pub const FAMILIES: [&str; 3] = ["direct_set", "conditional_set", "replacement"];

/// Registered held-out tasks per family (spec §23).
pub const HELDOUT_TASKS_PER_FAMILY: usize = 2;

/// Phase A: 3 families × 5 stress levels × 5 repetitions = 75 episodes.
pub const CALIBRATION_STRESS_LEVELS: u32 = 5;
pub const CALIBRATION_REPETITIONS: u32 = 5;
pub const CALIBRATION_FAMILIES: u32 = 3;
pub const CALIBRATION_EPISODES: usize =
    (CALIBRATION_FAMILIES * CALIBRATION_STRESS_LEVELS * CALIBRATION_REPETITIONS) as usize; // 75

/// Phase B: 2 conditions × 6 repetitions per held-out task.
pub const CONDITIONS: [&str; 2] = ["baseline", "candidate_repair"];
pub const EVAL_REPETITIONS: u32 = 6;
/// Expected Phase B episodes for 2 or 3 calibrated families (spec §41).
pub const EVAL_EPISODES_2FAMILIES: usize =
    2 * HELDOUT_TASKS_PER_FAMILY * EVAL_REPETITIONS as usize * CONDITIONS.len(); // 48
pub const EVAL_EPISODES_3FAMILIES: usize =
    3 * HELDOUT_TASKS_PER_FAMILY * EVAL_REPETITIONS as usize * CONDITIONS.len(); // 72

/// Registered alternating condition order per repetition (spec §42):
/// each condition is first exactly three times and second exactly
/// three times.
pub fn eval_condition_order(repetition: u32) -> [&'static str; 2] {
    if repetition % 2 == 1 {
        ["baseline", "candidate_repair"]
    } else {
        ["candidate_repair", "baseline"]
    }
}

/// Registered cyclic stress order per (repetition, execution position)
/// (spec §30): every stress level occurs exactly once in every execution
/// position across the five repetitions.
pub fn calibration_stress_index(repetition: u32, execution_position: u32) -> u32 {
    (repetition - 1 + execution_position - 1) % CALIBRATION_STRESS_LEVELS
}

/// Reliable-sanity gate (spec §31): minimum S0 baseline successes of 5.
pub const S0_MIN_SUCCESS: u32 = 4;

/// Headroom band (spec §32): a selectable stress level is one whose
/// baseline calibration success count is exactly 2 or 3 out of 5.
pub const HEADROOM_SUCCESS: [u32; 2] = [2, 3];

/// Minimum calibrated families to proceed to Phase B (spec §34).
pub const MIN_CALIBRATED_FAMILIES: u32 = 2;

/// Infrastructure-failure threshold above which the experiment is
/// inconclusive (spec §51): strictly more than 10% of all episodes.
pub const INFRA_FAILURE_RATE_THRESHOLD: f64 = 0.10;

/// Minimum valid held-out pairs (spec §45).
pub const MIN_VALID_PAIRS: u32 = 24;

/// Minimum informative (non-tied) held-out pairs (spec §46): the direct
/// correction of Experiment 0007's 1-of-24 informative-pair failure.
pub const MIN_INFORMATIVE_PAIRS: u32 = 12;

/// Pre-registered held-out headroom band (spec §43–44): a broad
/// information-availability gate, not the quality test itself.
pub const HEADROOM_MIN: f64 = 0.20;
pub const HEADROOM_MAX: f64 = 0.80;

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
    /// in the paired comparison (spec §48).
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
/// did not report it (spec §56: never inferred, never zero by assumption).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelRequestRecord {
    pub turn: u32,
    pub finish_reason: Option<String>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    /// `usage.prompt_tokens_details.cached_tokens`, when reported.
    pub cached_prompt_tokens: Option<u64>,
    /// `usage.completion_tokens_details.reasoning_tokens` (a *count*,
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
// Phase A record (calibration) — spec §60
// ===========================================================================

/// One persisted calibration trajectory (a JSONL line).
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
    /// Phase A (spec §28).
    pub condition: String,
    pub model: String,
    pub temperature: f64,
    pub redacted_endpoint: String,
    pub code_under_test_commit: String,
    pub prompt_hash: String,
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
// Phase B record (evaluation) — spec §61
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
    pub selection_manifest_sha256: String,
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
/// `cached / nominal` when both are reported; `null` otherwise
/// (never inferred from latency, spec §59).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CacheMetrics {
    pub nominal_prompt_tokens: Option<u64>,
    pub cached_prompt_tokens: Option<u64>,
    pub uncached_prompt_tokens: Option<u64>,
    pub cache_hit_ratio: Option<f64>,
}

/// Phase A: one row of the cache audit, stress level × execution
/// position (spec §59).
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
/// (spec §59).
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
/// `failure` block *is* the failure resource consumption (spec §57).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionSummary {
    pub condition: String,
    pub all: CostBlock,
    pub success: CostBlock,
    pub failure: CostBlock,
    pub cache: CacheMetrics,
}

/// The baseline/candidate_repair comparison on the pooled held-out
/// pairs of the selected families, paired by (task_id, repetition)
/// after infrastructure exclusions (spec §48).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairComparison {
    pub baseline: String,
    pub candidate: String,
    pub selected_families: Vec<String>,
    pub expected_pairs: u32,
    pub valid_pairs: u32,
    pub excluded_infrastructure_pairs: u32,
    pub missing_pairs: u32,
    /// wins + losses (spec §46: discordant oracle-success status; the
    /// only accepted definition of "informative").
    pub informative_pairs: u32,
    pub wins: u32,
    pub losses: u32,
    pub ties: u32,
    pub sign_test_p: f64,
    pub classification: String,
}

/// Per-family diagnostic (spec §53–55): reported, but never the basis
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
    /// Calibration baseline success rate at the selected stress (5 reps).
    pub calibration_baseline_success_rate: f64,
    /// Held-out baseline success rate at the selected stress.
    pub heldout_baseline_success_rate: f64,
    /// heldout - calibration (descriptive only; spec §55).
    pub transfer_error: f64,
    pub abs_transfer_error: f64,
}

/// Held-out baseline headroom gate result (spec §43).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HeadroomGate {
    pub episodes: u32,
    pub successes: u32,
    pub failures: u32,
    pub success_rate: f64,
    pub in_band: bool,
}

/// Pre-registered information-availability gates (spec §51).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Gates {
    pub phase_a_complete: bool,
    pub calibrated_families_sufficient: bool,
    pub phase_b_artifact_complete: bool,
    pub infrastructure_within_threshold: bool,
    pub headroom_in_band: bool,
    pub valid_pairs_sufficient: bool,
    pub informative_pairs_sufficient: bool,
}

/// Pre-registered experiment-level conclusion (spec §51–52).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conclusion {
    /// "supported" | "refuted" | "inconclusive"
    pub result: String,
    pub reasons: Vec<String>,
}

// ===========================================================================
// Phase A summary
// ===========================================================================

/// One family's calibration result, recomputable from the raw records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FamilyCalibrationSummary {
    pub family: String,
    pub calibration_task: String,
    pub s0_success: u32,
    pub s1_success: u32,
    pub s2_success: u32,
    pub s3_success: u32,
    pub s4_success: u32,
    pub s0_sanity_pass: bool,
    pub status: String,
    pub selected_stress: Option<String>,
    pub selection_reason: String,
}

/// One persisted calibration summary (regenerable from raw records).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalibrationSummary {
    pub experiment_id: String,
    pub phase: String,
    pub model: String,
    pub redacted_endpoint: String,
    pub temperature: f64,
    pub code_under_test_commit: String,
    pub prompt_hash: String,
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
    pub proceed_to_evaluation: bool,
    pub cache_overall: CacheMetrics,
    pub stress_position_cache_audit: Vec<StressPositionCacheRow>,
}

// ===========================================================================
// Phase B summary
// ===========================================================================

/// One persisted evaluation summary (regenerable from the immutable raw
/// artifact plus the committed selection manifest).
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
    pub selection_manifest_sha256: String,
    pub selected_families: Vec<String>,
    pub heldout_tasks: Vec<String>,
    pub episodes_expected: u32,
    pub episodes_total: u32,
    pub artifacts_complete: bool,
    pub oracle_successes: u32,
    pub agent_failures: u32,
    pub infrastructure_failures: u32,
    pub infrastructure_failure_rate: f64,
    pub agent_failure_breakdown: BTreeMap<String, u32>,
    pub infrastructure_failure_breakdown: BTreeMap<String, u32>,
    pub headroom: HeadroomGate,
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

/// One (candidate, baseline) pair outcome, from the candidate's
/// perspective (spec §48).
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
// Fault-schedule audit (spec §18, §62)
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
// Phase A summary computation
// ===========================================================================

/// Recompute the Phase A summary from raw records and the registry.
/// Selection is recomputed through the pure
/// `calibration::select_family_stress` — candidate data is not an input.
pub fn compute_calibration_summary(
    records: &[CalibrationRecord],
    registry: &[TaskSpec],
) -> CalibrationSummary {
    let first = records.first();
    let (model, endpoint, temp, commit, prompt_hash, task_hash, stress_hash) = match first {
        Some(r) => (
            r.model.clone(),
            r.redacted_endpoint.clone(),
            r.temperature,
            r.code_under_test_commit.clone(),
            r.prompt_hash.clone(),
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

    // Per-family: success counts per stress, then the pre-registered
    // pure selection rule.
    let mut per_family: Vec<FamilyCalibrationSummary> = Vec::new();
    let mut calibrated = 0u32;
    for family in FAMILIES {
        let cal_task = registry
            .iter()
            .find(|t| t.family == family && t.split == oracle::TaskSplit::Calibration)
            .map(|t| t.id.clone())
            .unwrap_or_default();

        let mut counts = [0u32; 5];
        for r in records.iter().filter(|r| r.family == family) {
            if let Some(i) = crate::tools::StressLevel::by_id(&r.stress_level)
                .map(|l| l.drop_count as usize)
                .filter(|i| *i < 5)
            {
                if r.oracle_success {
                    counts[i] += 1;
                }
            }
        }

        let outcomes: Vec<crate::calibration::CalibrationOutcome> = records
            .iter()
            .filter(|r| r.family == family)
            .map(|r| crate::calibration::CalibrationOutcome {
                stress_id: r.stress_level.clone(),
                repetition: r.repetition,
                oracle_success: r.oracle_success,
            })
            .collect();
        let sel = crate::calibration::select_family_stress(family, &outcomes);
        if sel.status == crate::calibration::SelectionStatus::Calibrated {
            calibrated += 1;
        }

        per_family.push(FamilyCalibrationSummary {
            family: family.to_string(),
            calibration_task: cal_task,
            s0_success: counts[0],
            s1_success: counts[1],
            s2_success: counts[2],
            s3_success: counts[3],
            s4_success: counts[4],
            s0_sanity_pass: sel.s0_sanity_pass,
            status: sel.status.as_str().to_string(),
            selected_stress: sel.selected_stress.clone(),
            selection_reason: sel.selection_reason.clone(),
        });
    }

    let complete = phase_a_artifact_complete(records, registry);

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
        prompt_hash,
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
        proceed_to_evaluation: calibrated >= MIN_CALIBRATED_FAMILIES,
        cache_overall,
        stress_position_cache_audit,
    }
}

// ===========================================================================
// Phase A / Phase B artifact completeness
// ===========================================================================

/// Phase A artifact completeness: exact count, exact tuple coverage,
/// registered cyclic stress order, registry consistency, fault audit,
/// usage arithmetic, oracle recomputation.
pub fn phase_a_artifact_complete(records: &[CalibrationRecord], registry: &[TaskSpec]) -> bool {
    if registry.len() != 9 || records.len() != CALIBRATION_EPISODES {
        return false;
    }
    let by_id: BTreeMap<&str, &TaskSpec> = registry.iter().map(|t| (t.id.as_str(), t)).collect();

    for (i, r) in records.iter().enumerate() {
        if r.sequence != (i + 1) as u32 {
            return false;
        }
        if !FAMILIES.contains(&r.family.as_str()) || r.condition != "baseline" {
            return false; // Phase A is baseline-only (spec §28).
        }
        if !(1..=CALIBRATION_REPETITIONS).contains(&r.repetition)
            || !(1..=CALIBRATION_STRESS_LEVELS).contains(&r.execution_position)
        {
            return false;
        }
        match by_id.get(r.task_id.as_str()) {
            Some(t) => {
                if t.family != r.family || t.split != oracle::TaskSplit::Calibration {
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
        // Registered cyclic stress order (spec §30).
        let expected = crate::tools::STRESS_LEVELS
            [calibration_stress_index(r.repetition, r.execution_position) as usize];
        if r.stress_level != expected.id || r.drop_count != expected.drop_count {
            return false;
        }
    }

    let mut tuples: BTreeMap<(String, u32, u32), u32> = BTreeMap::new();
    for r in records {
        *tuples
            .entry((r.family.clone(), r.repetition, r.execution_position))
            .or_insert(0) += 1;
    }
    for f in FAMILIES {
        for rep in 1..=CALIBRATION_REPETITIONS {
            for pos in 1..=CALIBRATION_STRESS_LEVELS {
                if tuples.get(&((*f).to_string(), rep, pos)) != Some(&1) {
                    return false;
                }
            }
        }
    }
    true
}

/// Phase B artifact completeness (spec §64): exact count from the
/// manifest, exact tuple coverage, registered alternating order, pair
/// stress consistency, registry consistency, fault audit.
pub fn phase_b_artifact_complete(
    records: &[EvaluationRecord],
    registry: &[TaskSpec],
    selection: &DifficultySelection,
    expected: usize,
) -> bool {
    if records.len() != expected || expected == 0 || registry.len() != 9 {
        return false;
    }
    let selected: BTreeSet<&str> = selection
        .per_family
        .iter()
        .filter(|f| f.status == crate::calibration::SelectionStatus::Calibrated.as_str())
        .map(|f| f.family.as_str())
        .collect();
    if selected.is_empty() || selected.len() > 3 {
        return false;
    }
    let by_id: BTreeMap<&str, &TaskSpec> = registry.iter().map(|t| (t.id.as_str(), t)).collect();

    for (i, r) in records.iter().enumerate() {
        if r.sequence != (i + 1) as u32 {
            return false;
        }
        if !CONDITIONS.contains(&r.condition.as_str())
            || !(1..=EVAL_REPETITIONS).contains(&r.repetition)
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
        let manifest_stress = selection
            .per_family
            .iter()
            .find(|f| f.family == r.family)
            .and_then(|f| f.selected_stress.clone())
            .unwrap_or_default();
        if manifest_stress != r.selected_stress {
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

    let mut tuples: BTreeSet<(String, String, u32)> = BTreeSet::new();
    for r in records {
        tuples.insert((r.task_id.clone(), r.condition.clone(), r.repetition));
    }
    for f in &selected {
        for t in registry
            .iter()
            .filter(|t| t.family == *f && t.split == oracle::TaskSplit::HeldOut)
        {
            for rep in 1..=EVAL_REPETITIONS {
                for c in CONDITIONS {
                    if !tuples.contains(&(t.id.clone(), c.to_string(), rep)) {
                        return false;
                    }
                }
            }
        }
    }
    for t in registry
        .iter()
        .filter(|t| t.split == oracle::TaskSplit::HeldOut)
    {
        if !selected.contains(t.family.as_str()) {
            continue;
        }
        for rep in 1..=EVAL_REPETITIONS {
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
/// the committed selection manifest (selected families, expected episode
/// count, per-family transfer diagnostic).
pub fn compute_evaluation_summary(
    records: &[EvaluationRecord],
    registry: &[TaskSpec],
    selection: &DifficultySelection,
) -> EvaluationSummary {
    let first = records.first();
    let (model, endpoint, temp, commit, bh, rh, task_hash, cal_raw, man_hash) = match first {
        Some(r) => (
            r.model.clone(),
            r.redacted_endpoint.clone(),
            r.temperature,
            r.final_eval_commit.clone(),
            r.baseline_prompt_sha256.clone(),
            r.repair_prompt_sha256.clone(),
            r.task_registry_sha256.clone(),
            r.calibration_raw_sha256.clone(),
            r.selection_manifest_sha256.clone(),
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

    let selected_families: Vec<String> = selection
        .per_family
        .iter()
        .filter(|f| f.status == crate::calibration::SelectionStatus::Calibrated.as_str())
        .map(|f| f.family.clone())
        .collect();

    let heldout_tasks: Vec<String> = selected_families
        .iter()
        .flat_map(|f| {
            registry
                .iter()
                .filter(|t| t.family == *f && t.split == oracle::TaskSplit::HeldOut)
                .map(|t| t.id.clone())
        })
        .collect();

    let calibrated = selection.calibrated_family_count as usize;
    let expected = if calibrated == 2 {
        EVAL_EPISODES_2FAMILIES
    } else if calibrated == 3 {
        EVAL_EPISODES_3FAMILIES
    } else {
        0
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

    // Headroom gate (spec §43): across ALL selected-family held-out
    // baseline episodes. Infra-failed episodes count as baseline
    // failures here.
    let baseline_records: Vec<&EvaluationRecord> = records
        .iter()
        .filter(|r| r.condition == "baseline")
        .collect();
    let base_total = baseline_records.len() as u32;
    let base_successes = baseline_records.iter().filter(|r| r.oracle_success).count() as u32;
    let base_rate = if base_total > 0 {
        base_successes as f64 / base_total as f64
    } else {
        0.0
    };
    let headroom = HeadroomGate {
        episodes: base_total,
        successes: base_successes,
        failures: base_total - base_successes,
        success_rate: base_rate,
        in_band: base_total > 0 && (HEADROOM_MIN..=HEADROOM_MAX).contains(&base_rate),
    };

    // Pooled comparison (spec §48–50): pair by (task_id, repetition).
    let mut wins = 0u32;
    let mut losses = 0u32;
    let mut ties = 0u32;
    let mut valid = 0u32;
    let mut excluded = 0u32;
    let mut missing = 0u32;
    let mut expected_pairs = 0u32;
    for t in registry
        .iter()
        .filter(|t| t.split == oracle::TaskSplit::HeldOut && selected_families.contains(&t.family))
    {
        for rep in 1..=EVAL_REPETITIONS {
            expected_pairs += 1;
            match (
                find_record(records, &t.id, "candidate_repair", rep),
                find_record(records, &t.id, "baseline", rep),
            ) {
                (Some(c), Some(b)) => {
                    if c.infrastructure_failure.is_some() || b.infrastructure_failure.is_some() {
                        excluded += 1;
                    } else {
                        valid += 1;
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
        candidate: "candidate_repair".to_string(),
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

    // Per-family diagnostic (spec §53–55). The calibration baseline rate
    // comes from the *committed* selection manifest (frozen before
    // Phase B), never re-derived from the mutable workspace.
    let per_family_diagnostics: Vec<FamilyDiagnostic> = selected_families
        .iter()
        .map(|f| {
            let fam_heldout: Vec<String> = registry
                .iter()
                .filter(|t| t.family == *f && t.split == oracle::TaskSplit::HeldOut)
                .map(|t| t.id.clone())
                .collect();
            let base: Vec<&EvaluationRecord> = records
                .iter()
                .filter(|r| r.family == *f && r.condition == "baseline")
                .collect();
            let rep: Vec<&EvaluationRecord> = records
                .iter()
                .filter(|r| r.family == *f && r.condition == "candidate_repair")
                .collect();
            let mut fw = 0u32;
            let mut fl = 0u32;
            let mut ft = 0u32;
            for t in registry
                .iter()
                .filter(|t| t.family == *f && t.split == oracle::TaskSplit::HeldOut)
            {
                for r in 1..=EVAL_REPETITIONS {
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
            let cal_rate = selection
                .per_family
                .iter()
                .find(|pf| pf.family == *f)
                .and_then(|pf| {
                    let s = pf.selected_stress.as_ref()?;
                    let c = match s.as_str() {
                        "S0" => pf.s0_success,
                        "S1" => pf.s1_success,
                        "S2" => pf.s2_success,
                        "S3" => pf.s3_success,
                        "S4" => pf.s4_success,
                        _ => return None,
                    };
                    Some(c as f64 / CALIBRATION_REPETITIONS as f64)
                })
                .unwrap_or(0.0);
            let selected = selection
                .per_family
                .iter()
                .find(|pf| pf.family == *f)
                .and_then(|pf| pf.selected_stress.clone())
                .unwrap_or_default();
            let te = base_rate_fam - cal_rate;
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
                calibration_baseline_success_rate: cal_rate,
                heldout_baseline_success_rate: base_rate_fam,
                transfer_error: te,
                abs_transfer_error: te.abs(),
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

    let complete = phase_b_artifact_complete(records, registry, selection, expected);

    // Pre-registered conclusion rule (spec §51–52): information gates
    // first; direction only once the measurement had sufficient
    // information.
    let valid_ok = valid >= MIN_VALID_PAIRS;
    let informative_ok = informative >= MIN_INFORMATIVE_PAIRS;
    let gates = Gates {
        phase_a_complete: selection.proceed_to_evaluation,
        calibrated_families_sufficient: selection.calibrated_family_count
            >= MIN_CALIBRATED_FAMILIES,
        phase_b_artifact_complete: complete,
        infrastructure_within_threshold: infra_within,
        headroom_in_band: headroom.in_band,
        valid_pairs_sufficient: valid_ok,
        informative_pairs_sufficient: informative_ok,
    };
    let mut reasons: Vec<String> = Vec::new();
    let mut result = "supported".to_string();
    if !gates.phase_a_complete {
        result = "inconclusive".into();
        reasons
            .push("Phase A artifact incomplete (calibration did not pass its gates)".to_string());
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
    if !gates.headroom_in_band {
        result = "inconclusive".into();
        reasons.push(format!(
            "held-out baseline success rate {base_rate:.4} outside the pre-registered headroom band [{HEADROOM_MIN:.2}, {HEADROOM_MAX:.2}]"
        ));
    }
    if !gates.valid_pairs_sufficient {
        result = "inconclusive".into();
        reasons.push(format!("valid pairs {valid} < {MIN_VALID_PAIRS}"));
    }
    if !gates.informative_pairs_sufficient {
        result = "inconclusive".into();
        reasons.push(format!(
            "informative (non-tied) pairs {informative} < {MIN_INFORMATIVE_PAIRS}"
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
        selection_manifest_sha256: man_hash,
        selected_families,
        heldout_tasks,
        episodes_expected: expected as u32,
        episodes_total: total,
        artifacts_complete: complete,
        oracle_successes: successes,
        agent_failures: agent,
        infrastructure_failures: infra,
        infrastructure_failure_rate: infra_rate,
        agent_failure_breakdown: agent_breakdown,
        infrastructure_failure_breakdown: infra_breakdown,
        headroom,
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
// Artifact verifiers (spec §62–65)
// ===========================================================================

/// Keys that must never appear anywhere in a raw artifact record.
/// Reasoning *content* and credential material are redacted by
/// construction; the candidate-side keys guard the anti-leakage
/// requirement (spec §5): the difficulty selector never sees the
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

/// Per-record structural checks shared by both verifiers (spec §62/§64):
/// termination consistency, usage arithmetic, requested/executed
/// integrity, model-visible result shapes, write-audit consistency,
/// fault-schedule audit, oracle recomputation.
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

/// Provenance of a selection manifest: the code commit plus the five
/// frozen hashes (spec §36, §38). The repair prompt hash is a *config*
/// hash committed before calibration; the candidate was never executed
/// in Phase A, so the manifest carries no candidate outcome data.
#[derive(Debug, Clone, PartialEq)]
pub struct ManifestProvenance {
    pub code_under_test_commit: String,
    pub baseline_prompt_sha256: String,
    pub repair_prompt_sha256: String,
    pub task_registry_sha256: String,
    pub stress_registry_sha256: String,
}

/// Recompute the difficulty selection manifest from raw calibration
/// records (spec §63). Pure: records + registry + frozen provenance —
/// no candidate data. The caller must fill `calibration_raw_sha256`
/// from the artifact file's byte hash before comparing.
pub fn recompute_selection(
    records: &[CalibrationRecord],
    registry: &[TaskSpec],
    prov: &ManifestProvenance,
) -> DifficultySelection {
    let mut per_family = Vec::new();
    for family in FAMILIES {
        let cal_task = registry
            .iter()
            .find(|t| t.family == family && t.split == oracle::TaskSplit::Calibration)
            .map(|t| t.id.clone())
            .unwrap_or_default();
        let mut counts = [0u32; 5];
        for r in records.iter().filter(|r| r.family == family) {
            let i = crate::tools::StressLevel::by_id(&r.stress_level)
                .map(|l| l.drop_count as usize)
                .unwrap_or(0);
            if i < 5 && r.oracle_success {
                counts[i] += 1;
            }
        }
        let outcomes: Vec<crate::calibration::CalibrationOutcome> = records
            .iter()
            .filter(|r| r.family == family)
            .map(|r| crate::calibration::CalibrationOutcome {
                stress_id: r.stress_level.clone(),
                repetition: r.repetition,
                oracle_success: r.oracle_success,
            })
            .collect();
        let sel = crate::calibration::select_family_stress(family, &outcomes);
        per_family.push(crate::calibration::ManifestFamilyEntry {
            family: family.to_string(),
            calibration_task: cal_task,
            s0_success: counts[0],
            s1_success: counts[1],
            s2_success: counts[2],
            s3_success: counts[3],
            s4_success: counts[4],
            status: sel.status.as_str().to_string(),
            selected_stress: sel.selected_stress.clone(),
            selection_reason: sel.selection_reason.clone(),
        });
    }
    let calibrated = per_family
        .iter()
        .filter(|f| f.status == crate::calibration::SelectionStatus::Calibrated.as_str())
        .count() as u32;
    DifficultySelection {
        experiment_id: EXPERIMENT_ID.to_string(),
        calibration_code_commit: prov.code_under_test_commit.clone(),
        calibration_raw_sha256: String::new(), // filled by the caller from the file hash
        baseline_prompt_sha256: prov.baseline_prompt_sha256.clone(),
        repair_prompt_sha256: prov.repair_prompt_sha256.clone(),
        task_registry_sha256: prov.task_registry_sha256.clone(),
        stress_registry_sha256: prov.stress_registry_sha256.clone(),
        per_family,
        calibrated_family_count: calibrated,
        proceed_to_evaluation: calibrated >= MIN_CALIBRATED_FAMILIES,
    }
}

/// Full Phase A verification (spec §62, §63).
#[allow(clippy::too_many_arguments)]
pub fn verify_calibration_artifacts(
    record_values: &[Value],
    summary_value: &Value,
    manifest_value: &Value,
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

    if registry.len() != 9 {
        errors.push(format!(
            "expected 9 registered tasks, found {}",
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
                if t.family != r.family || t.split != oracle::TaskSplit::Calibration {
                    errors.push(format!(
                        "{tag}: task {} does not belong to calibration family {:?}",
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
        if r.prompt_hash != baseline_prompt_hash {
            errors.push(format!(
                "{tag}: prompt_hash does not match the committed baseline prompt"
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

        // Registered cyclic stress order (spec §30).
        let expected_stress = crate::tools::STRESS_LEVELS
            [calibration_stress_index(r.repetition, r.execution_position) as usize];
        if r.stress_level != expected_stress.id {
            errors.push(format!(
                "{tag}: cyclic order violation — position {} of rep {} must be {}",
                r.execution_position, r.repetition, expected_stress.id
            ));
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
    consistent(|r| &r.prompt_hash, "prompt_hash");
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

    // --- selection manifest: recompute from raw records, deep-compare ---
    if !records.is_empty() {
        let first = &records[0];
        let prov = ManifestProvenance {
            code_under_test_commit: first.code_under_test_commit.clone(),
            baseline_prompt_sha256: first.prompt_hash.clone(),
            repair_prompt_sha256: repair_prompt_hash.to_string(),
            task_registry_sha256: first.task_registry_sha256.clone(),
            stress_registry_sha256: first.stress_registry_sha256.clone(),
        };
        let mut recomputed = recompute_selection(&records, registry, &prov);
        recomputed.calibration_raw_sha256 = calibration_file_sha256.to_string();
        if canonical_json(&serde_json::to_value(&recomputed).unwrap())
            != canonical_json(manifest_value)
        {
            errors.push(
                "selected-difficulty manifest does not deep-equal the selection recomputed from raw calibration records"
                    .to_string(),
            );
        }
    }
    // Manifest frozen provenance vs on-disk files + raw artifact hash.
    if manifest_value
        .get("repair_prompt_sha256")
        .and_then(Value::as_str)
        != Some(repair_prompt_hash)
    {
        errors.push(
            "manifest repair_prompt_sha256 does not match the committed repair prompt".to_string(),
        );
    }
    if manifest_value
        .get("baseline_prompt_sha256")
        .and_then(Value::as_str)
        != Some(baseline_prompt_hash)
    {
        errors.push(
            "manifest baseline_prompt_sha256 does not match the committed baseline prompt"
                .to_string(),
        );
    }
    if manifest_value
        .get("task_registry_sha256")
        .and_then(Value::as_str)
        != Some(task_registry_hash)
    {
        errors.push(
            "manifest task_registry_sha256 does not match the registered task suite".to_string(),
        );
    }
    if manifest_value
        .get("stress_registry_sha256")
        .and_then(Value::as_str)
        != Some(stress_registry_hash)
    {
        errors.push(
            "manifest stress_registry_sha256 does not match the registered stress ladder"
                .to_string(),
        );
    }
    if manifest_value
        .get("calibration_raw_sha256")
        .and_then(Value::as_str)
        != Some(calibration_file_sha256)
    {
        errors.push(
            "manifest calibration_raw_sha256 does not match the calibration artifact".to_string(),
        );
    }

    errors
}

/// Convert a parsed manifest `Value` into a typed selection (falls back
/// to an empty selection on parse failure, which then forces a summary
/// mismatch in verification).
impl From<Value> for DifficultySelection {
    fn from(v: Value) -> Self {
        serde_json::from_value(v).unwrap_or_else(|_| DifficultySelection {
            experiment_id: EXPERIMENT_ID.to_string(),
            calibration_code_commit: String::new(),
            calibration_raw_sha256: String::new(),
            baseline_prompt_sha256: String::new(),
            repair_prompt_sha256: String::new(),
            task_registry_sha256: String::new(),
            stress_registry_sha256: String::new(),
            per_family: Vec::new(),
            calibrated_family_count: 0,
            proceed_to_evaluation: false,
        })
    }
}

/// Full Phase B verification (spec §64, §65).
#[allow(clippy::too_many_arguments)]
pub fn verify_evaluation_artifacts(
    record_values: &[Value],
    summary_value: &Value,
    manifest_value: &Value,
    registry: &[TaskSpec],
    baseline_prompt_hash: &str,
    repair_prompt_hash: &str,
    task_registry_hash: &str,
    manifest_file_sha256: &str,
) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();

    if registry.len() != 9 {
        errors.push(format!(
            "expected 9 registered tasks, found {}",
            registry.len()
        ));
    }
    let by_id: BTreeMap<&str, &TaskSpec> = registry.iter().map(|t| (t.id.as_str(), t)).collect();

    // --- selection manifest frozen provenance vs on-disk files ---
    for (field, file_hash, label) in [
        (
            "baseline_prompt_sha256",
            baseline_prompt_hash,
            "baseline prompt",
        ),
        ("repair_prompt_sha256", repair_prompt_hash, "repair prompt"),
        ("task_registry_sha256", task_registry_hash, "task registry"),
    ] {
        if manifest_value.get(field).and_then(Value::as_str) != Some(file_hash) {
            errors.push(format!(
                "manifest {field} does not match the committed {label} file (pre-calibration hash changed)"
            ));
        }
    }
    if manifest_value
        .get("proceed_to_evaluation")
        .and_then(Value::as_bool)
        != Some(true)
    {
        errors.push(
            "selection manifest does not authorize Phase B (proceed_to_evaluation != true)"
                .to_string(),
        );
    }
    let selected: Vec<&str> = manifest_value
        .get("per_family")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|f| f.get("status").and_then(Value::as_str) == Some("calibrated"))
                .map(|f| f.get("family").and_then(Value::as_str).unwrap_or(""))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if selected.is_empty() || selected.len() > 3 {
        errors.push(format!(
            "selection manifest must list 2 or 3 calibrated families (found {})",
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

    // --- field-level tampering: forbidden keys (candidate must be unseen) ---
    for (i, v) in record_values.iter().enumerate() {
        scan_forbidden_keys(v, &format!("_records[{i}]"), &mut errors);
    }

    // --- expected record count derived from the committed manifest ---
    let calibrated = manifest_value
        .get("calibrated_family_count")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let expected = if calibrated == 2 {
        EVAL_EPISODES_2FAMILIES
    } else if calibrated == 3 {
        EVAL_EPISODES_3FAMILIES
    } else {
        0
    };
    if record_values.len() != expected {
        errors.push(format!(
            "expected {expected} evaluation records (derived from the committed selection manifest: {calibrated} calibrated families), found {}",
            record_values.len()
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
        if !(1..=EVAL_REPETITIONS).contains(&r.repetition)
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
        if r.selection_manifest_sha256 != manifest_file_sha256 {
            errors.push(format!(
                "{tag}: selection_manifest_sha256 does not match the committed manifest"
            ));
        }
        if r.task_registry_sha256 != task_registry_hash {
            errors.push(format!(
                "{tag}: task_registry_sha256 does not match the registered task suite"
            ));
        }
        if r.calibration_raw_sha256
            != manifest_value
                .get("calibration_raw_sha256")
                .and_then(Value::as_str)
                .unwrap_or("")
        {
            errors.push(format!(
                "{tag}: calibration_raw_sha256 does not match the selection manifest"
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
        for rep in 1..=EVAL_REPETITIONS {
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
                    "task {} rep {rep}: baseline and candidate_repair ran under different selected stresses",
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
    consistent(
        |r| &r.selection_manifest_sha256,
        "selection_manifest_sha256",
    );
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
        let selection = manifest_value.clone().into();
        let recomputed = compute_evaluation_summary(&records, registry, &selection);
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
/// `write_value` to the fault key, each followed by a read, then a final
/// answer turn. Returns consistent requested/executed/write-audit/
/// final-state records. The write plan stays inside the registered
/// repair budget: `write_count ≤ 5` ≤ max_tool_calls (10 calls ≤ 16).
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

/// Synthetic calibration success schedule. Deterministic; designed so
/// that two of the three families calibrate and one does not:
///
/// - direct_set:      S0 5/5, S1 4/5, S2 3/5, S3 3/5, S4 2/5 → select S2
/// - conditional_set: S0 5/5, S1 3/5, S2 2/5, S3 1/5, S4 0/5 → select S1
/// - replacement:     S0 3/5 → S0 sanity gate fails → calibration_invalid
fn synthetic_calibration_success(family: &str, stress_index: u32, repetition: u32) -> bool {
    let r = repetition - 1; // 0..4
    match family {
        "direct_set" => match stress_index {
            0 => true,
            1 => r < 4,
            2 => r < 3,
            3 => r < 3,
            4 => r < 2,
            _ => false,
        },
        "conditional_set" => match stress_index {
            0 => true,
            1 => r < 3,
            2 => r < 2,
            3 => r == 0,
            4 => false,
            _ => false,
        },
        _ => r < 3, // replacement: 3/5 at every level → S0 sanity fails
    }
}

/// Build the full deterministic 75-record synthetic calibration dataset.
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
        let task = registry
            .iter()
            .find(|t| t.family == family && t.split == oracle::TaskSplit::Calibration)
            .cloned()
            .unwrap_or_else(|| panic!("registry must contain a calibration task for {family}"));
        for rep in 1..=CALIBRATION_REPETITIONS {
            for pos in 1..=CALIBRATION_STRESS_LEVELS {
                seq += 1;
                let level =
                    crate::tools::STRESS_LEVELS[calibration_stress_index(rep, pos) as usize];
                let success = synthetic_calibration_success(family, level.drop_count, rep);
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
                    run_id: format!("exp0008-selftest-cal-{seq:03}"),
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
                    prompt_hash: "1".repeat(64),
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
    records
}

/// Synthetic evaluation success schedule (per family/task/rep/condition).
///
/// - baseline: succeed on reps 1..3 of a family's FIRST held-out task and
///   reps 1..2 of its SECOND. 2 families → 10/24 = 0.4167 → headroom
///   in-band; 3 families → 15/30 = 0.50 → in-band.
/// - candidate_repair: everything the baseline got right stays right,
///   plus it wins all of the first task's reps 4..6; it loses exactly
///   one pair per family (second task, rep 1). No infrastructure
///   failures: every pair is valid (24 or 36 ≥ 24).
fn synthetic_eval_outcome(task_id: &str, repetition: u32, condition: &str) -> bool {
    let is_first = matches!(task_id, "H1" | "H3" | "H5");
    let limit = if is_first { 3 } else { 2 };
    if condition == "baseline" {
        repetition <= limit
    } else {
        // candidate_repair
        !(matches!(task_id, "H2" | "H4" | "H6") && repetition == 1)
    }
}

/// Build the full synthetic Phase B dataset for the given manifest.
pub fn synthetic_evaluation_records(
    registry: &[TaskSpec],
    selection: &DifficultySelection,
) -> Vec<EvaluationRecord> {
    let task_hash = crate::experiment::task_registry_sha256(registry).unwrap_or_default();
    let selected: Vec<&str> = selection
        .per_family
        .iter()
        .filter(|f| f.status == "calibrated")
        .map(|f| f.family.as_str())
        .collect();
    let mut records = Vec::new();
    let mut seq = 0u32;
    for family in FAMILIES {
        if !selected.contains(&family) {
            continue;
        }
        let sel_stress = selection
            .per_family
            .iter()
            .find(|f| f.family == family)
            .and_then(|f| f.selected_stress.clone())
            .unwrap_or_else(|| "S2".to_string());
        let level =
            crate::tools::StressLevel::by_id(&sel_stress).unwrap_or(crate::tools::STRESS_LEVELS[2]);
        for task in registry
            .iter()
            .filter(|t| t.family == family && t.split == oracle::TaskSplit::HeldOut)
        {
            for rep in 1..=EVAL_REPETITIONS {
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
                        synthetic_run(task, &level, &write_value, write_count);
                    let requests = synthetic_requests(turns);
                    let eval = oracle::evaluate_task(
                        task,
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
                        run_id: format!("exp0008-selftest-eval-{seq:03}"),
                        sequence: seq,
                        family: family.to_string(),
                        task_id: task.id.clone(),
                        selected_stress: level.id.to_string(),
                        drop_count: level.drop_count,
                        condition: condition.to_string(),
                        repetition: rep,
                        condition_position: (pos + 1) as u32,
                        calibration_raw_sha256: "a".repeat(64),
                        selection_manifest_sha256: "b".repeat(64),
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
// Tests (spec §65 tamper matrix, both phases)
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

    fn synthetic_manifest(
        reg: &[TaskSpec],
        records: &[CalibrationRecord],
    ) -> (DifficultySelection, String, String) {
        let (bh, th, sh) = hash_inputs(reg);
        let prov = ManifestProvenance {
            code_under_test_commit: "0".repeat(40),
            baseline_prompt_sha256: bh,
            repair_prompt_sha256: "2".repeat(64),
            task_registry_sha256: th,
            stress_registry_sha256: sh,
        };
        let mut s = recompute_selection(records, reg, &prov);
        s.calibration_raw_sha256 = "a".repeat(64);
        (s, "a".repeat(64), "b".repeat(64))
    }

    fn verify_cal(
        records: &[CalibrationRecord],
        summary: &CalibrationSummary,
        manifest: &DifficultySelection,
        reg: &[TaskSpec],
    ) -> Vec<String> {
        let (bh, th, sh) = hash_inputs(reg);
        verify_calibration_artifacts(
            &cal_values(records),
            &serde_json::to_value(summary).unwrap(),
            &serde_json::to_value(manifest).unwrap(),
            reg,
            &bh,
            &"2".repeat(64), // records carry "1"*64 as baseline prompt hash;
            // the manifest's repair hash is checked against the frozen value.
            &th,
            &sh,
            &"a".repeat(64),
        )
    }

    fn verify_eval(
        records: &[EvaluationRecord],
        summary: &EvaluationSummary,
        manifest: &DifficultySelection,
        reg: &[TaskSpec],
    ) -> Vec<String> {
        let (_bh, th, _sh) = hash_inputs(reg);
        verify_evaluation_artifacts(
            &eval_values(records),
            &serde_json::to_value(summary).unwrap(),
            &serde_json::to_value(manifest).unwrap(),
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
        let (manifest, _, _) = synthetic_manifest(&reg, &records);
        assert_eq!(manifest.calibrated_family_count, 2);
        assert!(manifest.proceed_to_evaluation);
        let direct = manifest
            .per_family
            .iter()
            .find(|f| f.family == "direct_set")
            .unwrap();
        assert_eq!(direct.selected_stress.as_deref(), Some("S2"));
        let cond = manifest
            .per_family
            .iter()
            .find(|f| f.family == "conditional_set")
            .unwrap();
        assert_eq!(cond.selected_stress.as_deref(), Some("S1"));
        let rep = manifest
            .per_family
            .iter()
            .find(|f| f.family == "replacement")
            .unwrap();
        assert_eq!(rep.status, "calibration_invalid");
        assert_eq!(rep.selected_stress, None);
    }

    #[test]
    fn synthetic_calibration_verifier_passes() {
        let reg = registry();
        let records = synthetic_calibration_records(&reg);
        let summary = compute_calibration_summary(&records, &reg);
        assert!(summary.artifacts_complete);
        assert!(summary.proceed_to_evaluation);
        let (manifest, _, _) = synthetic_manifest(&reg, &records);
        let errors = verify_cal(&records, &summary, &manifest, &reg);
        assert!(errors.is_empty(), "unexpected: {errors:?}");
    }

    #[test]
    fn verifier_fails_on_calibration_count() {
        let reg = registry();
        let mut records = synthetic_calibration_records(&reg);
        records.pop(); // 74 records
        let summary = compute_calibration_summary(&records, &reg);
        let (manifest, _, _) = synthetic_manifest(&reg, &records);
        let errors = verify_cal(&records, &summary, &manifest, &reg);
        assert!(errors.iter().any(|e| e.contains("expected exactly")));
    }

    #[test]
    fn verifier_fails_on_candidate_inserted_into_calibration() {
        let reg = registry();
        let mut records = synthetic_calibration_records(&reg);
        records.last_mut().unwrap().condition = "candidate_repair".into();
        let summary = compute_calibration_summary(&records, &reg);
        let (manifest, _, _) = synthetic_manifest(&reg, &records);
        let errors = verify_cal(&records, &summary, &manifest, &reg);
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
        let (manifest, _, _) = synthetic_manifest(&reg, &records);
        let mut v = cal_values(&records);
        v.last_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("candidate_success".into(), Value::Bool(true));
        let errors = verify_calibration_artifacts(
            &v,
            &serde_json::to_value(&summary).unwrap(),
            &serde_json::to_value(&manifest).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &hash_inputs(&reg).2,
            &"a".repeat(64),
        );
        assert!(
            errors.iter().any(|e| e.contains("forbidden field")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_calibration_outcome_tamper() {
        let reg = registry();
        let records = synthetic_calibration_records(&reg);
        let summary = compute_calibration_summary(&records, &reg);
        let (manifest, _, _) = synthetic_manifest(&reg, &records);
        // Tamper the raw records after the summary/manifest were made.
        let mut v = cal_values(&records);
        v.last_mut().unwrap()["oracle_success"] =
            Value::from(!v.last().unwrap()["oracle_success"].as_bool().unwrap());
        let errors = verify_calibration_artifacts(
            &v,
            &serde_json::to_value(&summary).unwrap(),
            &serde_json::to_value(&manifest).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &hash_inputs(&reg).2,
            &"a".repeat(64),
        );
        assert!(!errors.is_empty(), "tampered calibration must not verify");
        assert!(
            errors
                .iter()
                .any(|e| e.contains("oracle_success") || e.contains("deep-equal")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_changed_selection_severity() {
        let reg = registry();
        let records = synthetic_calibration_records(&reg);
        let summary = compute_calibration_summary(&records, &reg);
        let mut manifest = recompute_selection(
            &records,
            &reg,
            &ManifestProvenance {
                code_under_test_commit: "0".repeat(40),
                baseline_prompt_sha256: "1".repeat(64),
                repair_prompt_sha256: "2".repeat(64),
                task_registry_sha256: hash_inputs(&reg).1,
                stress_registry_sha256: hash_inputs(&reg).2,
            },
        );
        manifest.calibration_raw_sha256 = "a".repeat(64);
        if let Some(d) = manifest
            .per_family
            .iter_mut()
            .find(|f| f.family == "direct_set")
        {
            d.selected_stress = Some("S4".into());
        }
        let errors = verify_cal(&records, &summary, &manifest, &reg);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("manifest does not deep-equal")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_cyclic_order_violation() {
        let reg = registry();
        let mut records = synthetic_calibration_records(&reg);
        // Swap the stress labels of two records of the same family/rep.
        let a = records[0].clone();
        let b = records[1].clone();
        let tmp = a.stress_level.clone();
        records[0].stress_level = b.stress_level.clone();
        records[0].drop_count = b.drop_count;
        records[1].stress_level = tmp;
        let summary = compute_calibration_summary(&records, &reg);
        let (manifest, _, _) = synthetic_manifest(&reg, &records);
        let errors = verify_cal(&records, &summary, &manifest, &reg);
        assert!(
            errors.iter().any(|e| e.contains("cyclic order violation")),
            "{errors:?}"
        );
    }

    // ------------------------------------------------------------------
    // Phase B
    // ------------------------------------------------------------------

    fn eval_fixtures(
        reg: &[TaskSpec],
    ) -> (
        Vec<EvaluationRecord>,
        EvaluationSummary,
        DifficultySelection,
    ) {
        let cal = synthetic_calibration_records(reg);
        let (manifest, _, _) = synthetic_manifest(reg, &cal);
        let records = synthetic_evaluation_records(reg, &manifest);
        let summary = compute_evaluation_summary(&records, reg, &manifest);
        (records, summary, manifest)
    }

    #[test]
    fn synthetic_evaluation_design() {
        let reg = registry();
        let (records, summary, _) = eval_fixtures(&reg);
        assert_eq!(records.len(), EVAL_EPISODES_2FAMILIES);
        assert!(summary.artifacts_complete);
        let h = &summary.headroom;
        assert!(h.in_band, "headroom: {h:?}");
        let c = &summary.comparison;
        assert_eq!(c.valid_pairs, 24);
        assert_eq!(c.informative_pairs, 16); // 14 wins + 2 losses
        assert_eq!(c.wins, 14);
        assert_eq!(c.losses, 2);
        assert_eq!(c.classification, "quality_improvement");
        assert!(summary.gates.headroom_in_band);
        assert!(summary.gates.valid_pairs_sufficient);
        assert!(summary.gates.informative_pairs_sufficient);
        assert_eq!(summary.conclusion.result, "supported");
    }

    #[test]
    fn synthetic_evaluation_verifier_passes() {
        let reg = registry();
        let (records, summary, manifest) = eval_fixtures(&reg);
        let errors = verify_eval(&records, &summary, &manifest, &reg);
        assert!(errors.is_empty(), "unexpected: {errors:?}");
    }

    fn tamper_eval(records: &[EvaluationRecord], f: impl Fn(&mut Value)) -> Vec<Value> {
        let mut v = eval_values(records);
        f(v.last_mut().unwrap());
        v
    }

    #[test]
    fn verifier_fails_on_missing_record() {
        let reg = registry();
        let (records, summary, manifest) = eval_fixtures(&reg);
        let mut v = eval_values(&records);
        v.pop();
        let errors = verify_evaluation_artifacts(
            &v,
            &serde_json::to_value(&summary).unwrap(),
            &serde_json::to_value(&manifest).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &"b".repeat(64),
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("expected") || e.contains("deep-equal")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_duplicate_record() {
        let reg = registry();
        let (records, summary, manifest) = eval_fixtures(&reg);
        let mut v = eval_values(&records);
        v.push(serde_json::to_value(&records[0]).unwrap());
        let errors = verify_evaluation_artifacts(
            &v,
            &serde_json::to_value(&summary).unwrap(),
            &serde_json::to_value(&manifest).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &"b".repeat(64),
        );
        assert!(errors.iter().any(|e| e.contains("expected")), "{errors:?}");
    }

    #[test]
    fn verifier_fails_on_heldout_task_changed_after_calibration() {
        let reg = registry();
        let (records, summary, manifest) = eval_fixtures(&reg);
        let v = tamper_eval(&records, |r| {
            r["initial_state"]["x"] = Value::from("TAMPERED");
        });
        let errors = verify_evaluation_artifacts(
            &v,
            &serde_json::to_value(&summary).unwrap(),
            &serde_json::to_value(&manifest).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &"b".repeat(64),
        );
        assert!(
            errors.iter().any(|e| e.contains("does not match registry")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_repair_prompt_hash_change() {
        let reg = registry();
        let (records, summary, manifest) = eval_fixtures(&reg);
        let v = tamper_eval(&records, |r| {
            r["repair_prompt_sha256"] = Value::from("3".repeat(64));
        });
        let errors = verify_evaluation_artifacts(
            &v,
            &serde_json::to_value(&summary).unwrap(),
            &serde_json::to_value(&manifest).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &"b".repeat(64),
        );
        assert!(
            errors.iter().any(|e| e.contains("repair_prompt_sha256")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_wrong_selected_stress_in_one_condition() {
        let reg = registry();
        let (records, summary, manifest) = eval_fixtures(&reg);
        let v = tamper_eval(&records, |r| {
            // The last record's condition gets the *other* family's stress.
            r["selected_stress"] = Value::from(if r["selected_stress"].as_str().unwrap() == "S1" {
                "S2"
            } else {
                "S1"
            });
        });
        let errors = verify_evaluation_artifacts(
            &v,
            &serde_json::to_value(&summary).unwrap(),
            &serde_json::to_value(&manifest).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &"b".repeat(64),
        );
        assert!(
            !errors.is_empty(),
            "a tampered selected stress must not verify"
        );
    }

    #[test]
    fn verifier_fails_on_different_stress_between_conditions() {
        let reg = registry();
        let (records, summary, manifest) = eval_fixtures(&reg);
        // Change the stress of one member of the last pair only.
        let v = {
            let mut v = eval_values(&records);
            let last = v.last().unwrap();
            let rep = last["repetition"].clone();
            let task = last["task_id"].clone();
            v.iter_mut().for_each(|x| {
                if x["task_id"] == task
                    && x["repetition"] == rep
                    && x["condition"].as_str() == Some("baseline")
                {
                    x["selected_stress"] = Value::from("S4");
                }
            });
            v
        };
        let errors = verify_evaluation_artifacts(
            &v,
            &serde_json::to_value(&summary).unwrap(),
            &serde_json::to_value(&manifest).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &"b".repeat(64),
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("different selected stresses") || e.contains("inconsistent")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_wrong_condition_order() {
        let reg = registry();
        let (records, summary, manifest) = eval_fixtures(&reg);
        let v = tamper_eval(&records, |r| {
            // Flip the last record's condition to its pair member's.
            r["condition"] = Value::from(if r["condition"].as_str().unwrap() == "baseline" {
                "candidate_repair"
            } else {
                "baseline"
            });
        });
        let errors = verify_evaluation_artifacts(
            &v,
            &serde_json::to_value(&summary).unwrap(),
            &serde_json::to_value(&manifest).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &"b".repeat(64),
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("registered alternating order")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_oracle_tamper() {
        let reg = registry();
        let (records, summary, manifest) = eval_fixtures(&reg);
        let v = tamper_eval(&records, |r| {
            r["oracle_success"] = Value::from(!r["oracle_success"].as_bool().unwrap());
        });
        let errors = verify_evaluation_artifacts(
            &v,
            &serde_json::to_value(&summary).unwrap(),
            &serde_json::to_value(&manifest).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &"b".repeat(64),
        );
        assert!(
            errors.iter().any(|e| e.contains("oracle_success")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_fault_audit_tamper() {
        let reg = registry();
        let (records, summary, manifest) = eval_fixtures(&reg);
        let v = tamper_eval(&records, |r| {
            let attempts = r["environment_write_attempts"].as_array_mut().unwrap();
            if let Some(a) = attempts.first_mut() {
                a["applied"] = Value::from(!a["applied"].as_bool().unwrap());
            }
        });
        let errors = verify_evaluation_artifacts(
            &v,
            &serde_json::to_value(&summary).unwrap(),
            &serde_json::to_value(&manifest).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &"b".repeat(64),
        );
        assert!(
            errors.iter().any(|e| e.contains("fault audit")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_when_cache_exceeds_prompt() {
        let reg = registry();
        let (records, summary, manifest) = eval_fixtures(&reg);
        let v = tamper_eval(&records, |r| {
            let reqs = r["model_requests"].as_array_mut().unwrap();
            let q = reqs.first_mut().unwrap();
            let p = q["prompt_tokens"].as_u64().unwrap();
            q["cached_prompt_tokens"] = Value::from(p + 5);
            q["uncached_prompt_tokens"] = Value::Null;
        });
        let errors = verify_evaluation_artifacts(
            &v,
            &serde_json::to_value(&summary).unwrap(),
            &serde_json::to_value(&manifest).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &"b".repeat(64),
        );
        assert!(
            errors.iter().any(|e| e.contains("cached_prompt_tokens")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_reasoning_content_leak() {
        let reg = registry();
        let (records, summary, manifest) = eval_fixtures(&reg);
        let v = tamper_eval(&records, |r| {
            r["reasoning_content"] = Value::from("SECRET");
        });
        let errors = verify_evaluation_artifacts(
            &v,
            &serde_json::to_value(&summary).unwrap(),
            &serde_json::to_value(&manifest).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &"b".repeat(64),
        );
        assert!(
            errors.iter().any(|e| e.contains("forbidden field")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_secret_field_leak() {
        let reg = registry();
        let (records, summary, manifest) = eval_fixtures(&reg);
        let v = tamper_eval(&records, |r| {
            r["api_key"] = Value::from("sk-SECRET");
        });
        let errors = verify_evaluation_artifacts(
            &v,
            &serde_json::to_value(&summary).unwrap(),
            &serde_json::to_value(&manifest).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &"b".repeat(64),
        );
        assert!(
            errors.iter().any(|e| e.contains("forbidden field")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_conclusion_tamper() {
        let reg = registry();
        let (records, summary, manifest) = eval_fixtures(&reg);
        let mut sv = serde_json::to_value(&summary).unwrap();
        sv["conclusion"]["result"] = Value::from("supported");
        let errors = verify_evaluation_artifacts(
            &eval_values(&records),
            &sv,
            &serde_json::to_value(&manifest).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &"b".repeat(64),
        );
        // The synthetic conclusion is "supported", so tamper the opposite:
        let mut sv2 = serde_json::to_value(&summary).unwrap();
        sv2["conclusion"]["result"] = Value::from("refuted");
        let errors2 = verify_evaluation_artifacts(
            &eval_values(&records),
            &sv2,
            &serde_json::to_value(&manifest).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &"b".repeat(64),
        );
        assert!(
            errors.is_empty(),
            "untampered summary must verify: {errors:?}"
        );
        assert!(
            errors2.iter().any(|e| e.contains("deep-equal")),
            "{errors2:?}"
        );
    }

    #[test]
    fn verifier_fails_on_informative_pair_count_tamper() {
        let reg = registry();
        let (records, summary, manifest) = eval_fixtures(&reg);
        let mut sv = serde_json::to_value(&summary).unwrap();
        sv["comparison"]["informative_pairs"] = Value::from(99u64);
        let errors = verify_evaluation_artifacts(
            &eval_values(&records),
            &sv,
            &serde_json::to_value(&manifest).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &"b".repeat(64),
        );
        assert!(
            errors.iter().any(|e| e.contains("deep-equal")),
            "{errors:?}"
        );
    }

    #[test]
    fn verifier_fails_on_headroom_value_tamper() {
        let reg = registry();
        let (records, summary, manifest) = eval_fixtures(&reg);
        let mut sv = serde_json::to_value(&summary).unwrap();
        sv["headroom"]["success_rate"] = Value::from(0.01);
        let errors = verify_evaluation_artifacts(
            &eval_values(&records),
            &sv,
            &serde_json::to_value(&manifest).unwrap(),
            &reg,
            &"1".repeat(64),
            &"2".repeat(64),
            &hash_inputs(&reg).1,
            &"b".repeat(64),
        );
        assert!(
            errors.iter().any(|e| e.contains("deep-equal")),
            "{errors:?}"
        );
    }

    // ------------------------------------------------------------------
    // Gate behavior of the summary computation (information-availability)
    // ------------------------------------------------------------------

    #[test]
    fn summary_conclusion_is_refuted_when_direction_fails() {
        let reg = registry();
        let cal = synthetic_calibration_records(&reg);
        let (manifest, _, _) = synthetic_manifest(&reg, &cal);
        // Deterministic regression pattern on all three families (36
        // pairs): the first 28 (task, rep) pairs are
        // baseline-wins/repair-failures, the last 8 the opposite.
        // Headroom 28/36 = 0.78 stays in band; all 36 pairs are valid and
        // informative; the exact sign test classifies quality_regression,
        // so the pre-registered rule must conclude "refuted".
        let mut manifest = manifest;
        for f in &mut manifest.per_family {
            f.status = "calibrated".to_string();
            if f.selected_stress.is_none() {
                f.selected_stress = Some("S2".to_string());
            }
        }
        manifest.calibrated_family_count = 3;
        manifest.proceed_to_evaluation = true;
        let records = synthetic_evaluation_records(&reg, &manifest);
        let mut records = records;
        let selected: Vec<&str> = manifest
            .per_family
            .iter()
            .filter(|f| f.status == "calibrated")
            .map(|f| f.family.as_str())
            .collect();
        let mut pair_index = 0u32;
        for t in reg.iter().filter(|t| {
            t.split == crate::oracle::TaskSplit::HeldOut && selected.contains(&t.family.as_str())
        }) {
            for rep in 1..=EVAL_REPETITIONS {
                let repair = pair_index >= 28; // repair wins the last 8 pairs
                for r in records
                    .iter_mut()
                    .filter(|r| r.task_id == t.id && r.repetition == rep)
                {
                    r.oracle_success = if r.condition == "baseline" {
                        !repair
                    } else {
                        repair
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
        let summary = compute_evaluation_summary(&records, &reg, &manifest);
        assert!(summary.gates.informative_pairs_sufficient, "{summary:?}");
        assert_eq!(summary.conclusion.result, "refuted");
        assert_eq!(summary.comparison.valid_pairs, 36);
        assert_eq!(summary.comparison.informative_pairs, 36);
        assert!(
            summary.comparison.losses > summary.comparison.wins,
            "{:?}",
            summary.comparison
        );
        assert!(summary.comparison.sign_test_p <= 0.05);
        assert_eq!(summary.comparison.classification, "quality_regression");
    }
}
