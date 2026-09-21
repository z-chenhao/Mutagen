//! Artifact types, factorial design constants, summary computation, the
//! artifact verifier, and a deterministic network-free synthetic
//! dataset (for `self-test` and verifier regression tests).
//!
//! The kernel is the authoritative execution recorder: every field of an
//! `EpisodeRecord` originates from the Rust kernel's own in-episode
//! records. Raw artifacts are never reconstructed from server logs.
//!
//! Experiment 0007 deliberately stores, per model request, the full
//! provider-reported usage block (including `null` when a field was not
//! reported) so that cache accounting is reproducible from committed
//! artifacts rather than inferred from external logs.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::oracle::{self, TaskSpec};
use crate::protocol::{TEMPERATURE, canonical_json};
use crate::stats;
use crate::tools::{StateKey, WriteAttempt};

use serde_json::json;

// ===========================================================================
// Design constants (spec §61: hardcoded, never inferred from artifacts)
// ===========================================================================

pub const EXPERIMENT_ID: &str = "0007";
pub const EXPECTED_TASKS: usize = 8;
pub const EXPECTED_CONDITIONS: usize = 3;
pub const EXPECTED_REPETITIONS: u32 = 6;
pub const EXPECTED_EPISODES: usize =
    EXPECTED_TASKS * EXPECTED_CONDITIONS * EXPECTED_REPETITIONS as usize; // 144

/// Registered conditions (baseline + the two controlled candidates).
pub const CONDITIONS: [&str; EXPECTED_CONDITIONS] =
    ["baseline", "candidate_repair", "candidate_regression"];

/// The six condition-order permutations, one per repetition
/// (spec §34). Every permutation of B/P/R appears exactly once; each
/// condition runs 2× first, 2× second, 2× third per task.
pub const PERMUTATIONS: [[&str; EXPECTED_CONDITIONS]; EXPECTED_REPETITIONS as usize] = [
    ["baseline", "candidate_repair", "candidate_regression"],
    ["candidate_repair", "candidate_regression", "baseline"],
    ["candidate_regression", "baseline", "candidate_repair"],
    ["baseline", "candidate_regression", "candidate_repair"],
    ["candidate_regression", "candidate_repair", "baseline"],
    ["candidate_repair", "baseline", "candidate_regression"],
];

/// Infrastructure-failure threshold above which the experiment is
/// inconclusive (spec §8): strictly more than 10% of all episodes.
pub const INFRA_FAILURE_RATE_THRESHOLD: f64 = 0.10;

/// Minimum valid held-out pairs per baseline/candidate comparison
/// (spec §48); below this the sign test is not run and the experiment
/// is inconclusive.
pub const MIN_VALID_HELDOUT_PAIRS: u32 = 12;

// ===========================================================================
// Termination / failure classification
// ===========================================================================

/// Terminal reason for one episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationReason {
    Completed,
    // Agent failures (candidate quality evidence):
    TurnLimit,
    ToolCallLimit,
    ParallelToolCallsUnsupported,
    UnknownTool,
    InvalidArguments,
    NoFinalAnswer,
    // Infrastructure failures (excludable, recorded separately):
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

    /// Agent-caused failures are *quality* evidence: they count as task
    /// failures and are never excluded from the candidate comparison.
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

    /// Infrastructure failures (network, server, provider) may be
    /// excluded from statistical pairs; they are recorded separately.
    /// Parallel tool calls are NOT infrastructure failures.
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
// Episode record (one JSONL line in the raw artifact)
// ===========================================================================

/// Per-request usage. Every field is `Option`: `null` when the provider
/// did not report it. `uncached_prompt_tokens` is computed (see kernel)
/// and is `null` whenever the provider did not report cached detail.
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
    /// `null` when the provider did not report cached detail.
    pub uncached_prompt_tokens: Option<u64>,
}

/// A tool call *requested* by the model. Includes rejected calls: for a
/// rejected parallel response, `requested_tool_calls` holds the model's
/// calls while `executed_tool_calls` holds none for that response.
///
/// `arguments` is the parsed JSON object; if the model emitted
/// unparseable JSON it is preserved verbatim as a JSON string.
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

/// Wall-clock measurements for one episode. All labeled approximate:
/// `kernel_overhead_estimate_ms` is a residual, not a measurement.
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

/// One persisted trajectory (a JSONL line in the raw artifact).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpisodeRecord {
    pub experiment_id: String,
    pub run_id: String,
    /// 1-based position of this episode in the run (strict sequence).
    pub sequence: u32,
    pub task_id: String,
    pub task_name: String,
    pub split: String,
    pub condition: String,
    pub repetition: u32,
    pub condition_position: u32,
    pub model: String,
    pub temperature: f64,
    pub redacted_endpoint: String,
    pub code_under_test_commit: String,
    pub baseline_prompt_sha256: String,
    pub repair_prompt_sha256: String,
    pub regression_prompt_sha256: String,
    pub task_suite_sha256: String,
    pub initial_state: Value,
    pub target_state: Value,
    pub fault_mode: Value,
    pub model_requests: Vec<ModelRequestRecord>,
    pub requested_tool_calls: Vec<RequestedToolCall>,
    pub executed_tool_calls: Vec<ExecutedToolCall>,
    /// Experiment-only fault-injection audit (never in the transcript).
    pub environment_write_attempts: Vec<WriteAttempt>,
    pub final_state: Value,
    pub final_answer: Option<String>,
    pub model_turn_count: u32,
    /// Count of executed tool calls.
    pub tool_call_count: u32,
    /// Some(agent reason) for agent-caused failures only.
    pub agent_failure: Option<String>,
    /// Some(infra reason) for infrastructure failures only.
    pub infrastructure_failure: Option<String>,
    /// Some when a request's reported usage failed arithmetic validation
    /// (e.g. cached > prompt). Fails artifact verification.
    pub usage_accounting_error: Option<String>,
    pub termination_reason: String,
    pub oracle_success: bool,
    pub oracle_failure_reasons: Vec<String>,
    pub timing: Timing,
}

/// Runner-side provenance attached by the kernel's `build_record`.
#[derive(Debug, Clone)]
pub struct RecordMeta {
    pub run_id: String,
    pub sequence: u32,
    pub task: TaskSpec,
    pub condition: String,
    pub repetition: u32,
    pub condition_position: u32,
    pub model: String,
    pub redacted_endpoint: String,
    pub code_under_test_commit: String,
    pub baseline_prompt_sha256: String,
    pub repair_prompt_sha256: String,
    pub regression_prompt_sha256: String,
    pub task_suite_sha256: String,
}

// ===========================================================================
// Summary types
// ===========================================================================

/// Aggregated cost for one condition × population. Failed episodes
/// consume resources and are never omitted from their population.
/// Token fields are `null` (never zero) when no request reported the
/// corresponding field; otherwise they are sums over reporting requests.
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
/// `cached / nominal` when both are reported; `null` otherwise (never
/// inferred from latency).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CacheMetrics {
    pub nominal_prompt_tokens: Option<u64>,
    pub cached_prompt_tokens: Option<u64>,
    pub uncached_prompt_tokens: Option<u64>,
    pub cache_hit_ratio: Option<f64>,
}

/// One row of the cache fairness audit: condition × condition position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PositionCacheRow {
    pub condition: String,
    pub position: u32,
    pub episodes: u32,
    pub nominal_prompt_tokens: Option<u64>,
    pub cached_prompt_tokens: Option<u64>,
    pub uncached_prompt_tokens: Option<u64>,
    pub cache_hit_ratio: Option<f64>,
}

/// Per-condition cost across the four registered populations. The
/// `failure` block *is* the failure resource consumption.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionSummary {
    pub condition: String,
    pub all: CostBlock,
    pub heldout: CostBlock,
    pub success: CostBlock,
    pub failure: CostBlock,
    pub cache: CacheMetrics,
    pub heldout_cache: CacheMetrics,
}

/// One candidate-versus-baseline comparison on one split, paired by
/// (task_id, repetition) after infrastructure exclusions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairComparison {
    pub baseline: String,
    pub candidate: String,
    pub split: String,
    pub expected_pairs: u32,
    pub valid_pairs: u32,
    pub excluded_infrastructure_pairs: u32,
    pub missing_pairs: u32,
    pub insufficient_pairs: bool,
    pub wins: u32,
    pub losses: u32,
    pub ties: u32,
    pub non_tied_pairs: u32,
    pub sign_test_p: f64,
    pub classification: String,
}

/// Fault-schedule audit across all episodes.
///
/// The `registered_*` fields count episodes by the fault mode registered
/// for their task. The `*_target_write` / `triggered_*` / `dropped_*`
/// fields count what the audit log actually shows. The two families are
/// deliberately distinct: a `DropFirstWrite` episode in which the model
/// never writes the registered key is registered as DropFirstWrite, has
/// zero target-key writes, and therefore a *never-triggered* fault — it
/// still conforms to the schedule (the registered one-shot drop is
/// conditional on a write occurring) and must not be reclassified as
/// Reliable. `registered_*` and `triggered_*` are not expected to be
/// equal in general.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaultAudit {
    pub episodes_checked: u32,
    pub episodes_passed: u32,
    pub violations: Vec<String>,
    pub passed: bool,
    /// Episodes whose task registered the Reliable fault mode.
    pub registered_reliable_episodes: u32,
    /// Episodes whose task registered DropFirstWrite(key).
    pub registered_drop_first_write_episodes: u32,
    /// DropFirstWrite episodes in which the model issued at least one
    /// write to the registered key.
    pub drop_mode_episodes_with_target_write: u32,
    /// DropFirstWrite episodes in which the model never wrote the
    /// registered key (fault never triggered; schedule still conforms).
    pub drop_mode_episodes_without_target_write: u32,
    /// DropFirstWrite episodes in which at least one write attempt was
    /// actually silently dropped (== with_target_write when the audit
    /// passes, because the registered drop is one-shot and fires exactly
    /// on the first target-key write).
    pub triggered_drop_episodes: u32,
    /// Total silently-dropped write attempts across all episodes (==
    /// triggered_drop_episodes when the audit passes).
    pub total_dropped_write_attempts: u32,
}

/// Pre-registered experiment-level conclusion (spec §48).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conclusion {
    /// "supported" | "refuted" | "inconclusive"
    pub result: String,
    pub reasons: Vec<String>,
}

/// Derived summary (regenerable from the immutable raw artifact).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub experiment_id: String,
    pub model: String,
    pub redacted_endpoint: String,
    pub temperature: f64,
    pub code_under_test_commit: String,
    pub baseline_prompt_sha256: String,
    pub repair_prompt_sha256: String,
    pub regression_prompt_sha256: String,
    pub task_suite_sha256: String,
    pub episodes_expected: u32,
    pub episodes_total: u32,
    /// Exactly 144 records, every registered task/condition/repetition
    /// tuple present exactly once, and the fault audit passing.
    pub artifacts_complete: bool,
    pub oracle_successes: u32,
    pub agent_failures: u32,
    pub infrastructure_failures: u32,
    pub infrastructure_failure_rate: f64,
    pub infrastructure_failure_rate_exceeds_threshold: bool,
    pub agent_failure_breakdown: BTreeMap<String, u32>,
    pub infrastructure_failure_breakdown: BTreeMap<String, u32>,
    pub conditions: Vec<ConditionSummary>,
    pub cache_overall: CacheMetrics,
    pub cache_heldout: CacheMetrics,
    /// True when at least one request reported cached-token detail.
    pub cache_observable: bool,
    pub position_cache_audit: Vec<PositionCacheRow>,
    pub heldout_comparisons: Vec<PairComparison>,
    pub calibration_comparisons: Vec<PairComparison>,
    pub fault_audit: FaultAudit,
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
    fn add_record(&mut self, r: &EpisodeRecord) {
        self.episodes += 1;
        if r.oracle_success {
            self.successes += 1;
        }
        if r.agent_failure.is_some() {
            self.agent_failures += 1;
        }
        if r.infrastructure_failure.is_some() {
            self.infra_failures += 1;
        }
        self.requests += r.model_requests.len() as u32;
        self.calls += r.executed_tool_calls.len() as u32;
        for q in &r.model_requests {
            self.nominal.add(q.prompt_tokens);
            self.cached.add(q.cached_prompt_tokens);
            self.uncached.add(q.uncached_prompt_tokens);
            self.completion.add(q.completion_tokens);
            self.reasoning.add(q.reasoning_tokens);
            if q.prompt_tokens.is_some() {
                self.usage_reporting += 1;
            }
        }
        self.wall_ms += r.timing.wall_time_ms;
        self.model_wait_ms += r.timing.model_wait_time_ms;
        self.tool_exec_us += r.timing.tool_execution_time_us;
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

fn accumulate(records: &[EpisodeRecord], keep: impl Fn(&EpisodeRecord) -> bool) -> CostBlock {
    let mut acc = CostAccum::default();
    for r in records {
        if keep(r) {
            acc.add_record(r);
        }
    }
    acc.finish()
}

fn cache_metrics(records: &[EpisodeRecord], keep: impl Fn(&EpisodeRecord) -> bool) -> CacheMetrics {
    let mut nominal = TokenAcc::default();
    let mut cached = TokenAcc::default();
    let mut uncached = TokenAcc::default();
    for r in records.iter().filter(|r| keep(r)) {
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
    CacheMetrics {
        nominal_prompt_tokens: n,
        cached_prompt_tokens: c,
        uncached_prompt_tokens: u,
        cache_hit_ratio: ratio,
    }
}

/// One (candidate, baseline) pair outcome, from the candidate's
/// perspective.
fn pair_outcome(candidate_success: bool, baseline_success: bool) -> &'static str {
    match (candidate_success, baseline_success) {
        (true, false) => "win",
        (false, true) => "loss",
        _ => "tie",
    }
}

/// Compute one comparison. Pairs are matched by (task_id, repetition).
/// A pair is *valid* (enters the sign test) only when neither episode
/// suffered an infrastructure failure; agent failures are ordinary task
/// failures and remain valid pairs.
fn compare(
    records: &[EpisodeRecord],
    registry: &[TaskSpec],
    split: &str,
    candidate: &str,
) -> PairComparison {
    let mut wins = 0u32;
    let mut losses = 0u32;
    let mut ties = 0u32;
    let mut valid = 0u32;
    let mut excluded = 0u32;
    let mut missing = 0u32;

    let find_index =
        |records: &[EpisodeRecord], task_id: &str, condition: &str, rep: u32| -> Option<usize> {
            records.iter().position(|x| {
                x.task_id == task_id && x.condition == condition && x.repetition == rep
            })
        };
    let find = |i: Option<usize>| i.and_then(|i| records.get(i));

    let mut expected = 0u32;
    for t in registry.iter().filter(|t| t.split.as_str() == split) {
        expected += EXPECTED_REPETITIONS;
        for rep in 1..=EXPECTED_REPETITIONS {
            let (c, b) = (
                find(find_index(records, &t.id, candidate, rep)),
                find(find_index(records, &t.id, "baseline", rep)),
            );
            match (c, b) {
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

    let non_tied = wins + losses;
    let p = stats::exact_sign_test_p(wins as u64, losses as u64);
    let class = stats::classify_quality(wins as u64, losses as u64);

    PairComparison {
        baseline: "baseline".to_string(),
        candidate: candidate.to_string(),
        split: split.to_string(),
        expected_pairs: expected,
        valid_pairs: valid,
        excluded_infrastructure_pairs: excluded,
        missing_pairs: missing,
        insufficient_pairs: valid < MIN_VALID_HELDOUT_PAIRS,
        wins,
        losses,
        ties,
        non_tied_pairs: non_tied,
        sign_test_p: p,
        classification: class.as_str().to_string(),
    }
}

/// Fault-schedule audit for one record. Returns a violation message or
/// `None` when the schedule executed exactly as registered.
pub fn audit_record(r: &EpisodeRecord) -> Option<String> {
    let mode = r.fault_mode.get("mode").and_then(Value::as_str)?;
    match mode {
        "reliable" => {
            for a in &r.environment_write_attempts {
                if !a.applied || a.fault_reason.is_some() {
                    return Some(format!(
                        "reliable mode: write attempt {} not applied cleanly",
                        a.sequence
                    ));
                }
            }
            None
        }
        "drop_first_write" => {
            let key = r
                .fault_mode
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let target: Vec<&WriteAttempt> = r
                .environment_write_attempts
                .iter()
                .filter(|a| a.key == key)
                .collect();
            let dropped: Vec<&WriteAttempt> = r
                .environment_write_attempts
                .iter()
                .filter(|a| !a.applied)
                .collect();
            if dropped.len() > 1 {
                return Some(format!(
                    "drop_first_write: {} dropped writes (at most one allowed)",
                    dropped.len()
                ));
            }
            if let Some(first) = target.first() {
                if first.applied {
                    return Some(format!(
                        "drop_first_write({key}): first write was applied (expected dropped)"
                    ));
                }
                if first.fault_reason.as_deref() != Some("drop_first_write") {
                    return Some(format!(
                        "drop_first_write({key}): dropped write missing fault_reason"
                    ));
                }
            }
            for a in target.iter().skip(1) {
                if !a.applied {
                    return Some(format!(
                        "drop_first_write({key}): write {} after the first was dropped",
                        a.sequence
                    ));
                }
            }
            for a in r.environment_write_attempts.iter().filter(|a| a.key != key) {
                if !a.applied {
                    return Some(format!(
                        "drop_first_write({key}): unregistered key {} was dropped",
                        a.key
                    ));
                }
            }
            None
        }
        other => Some(format!("unknown fault mode {other:?}")),
    }
}

/// Recompute the derived summary from raw records and the registry.
pub fn compute_summary(records: &[EpisodeRecord], registry: &[TaskSpec]) -> Summary {
    let first = records.first();
    let (model, endpoint, temp, commit, bh, rh, gh, suite_h) = match first {
        Some(r) => (
            r.model.clone(),
            r.redacted_endpoint.clone(),
            r.temperature,
            r.code_under_test_commit.clone(),
            r.baseline_prompt_sha256.clone(),
            r.repair_prompt_sha256.clone(),
            r.regression_prompt_sha256.clone(),
            r.task_suite_sha256.clone(),
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

    let conditions: Vec<ConditionSummary> = CONDITIONS
        .iter()
        .map(|c| ConditionSummary {
            condition: (*c).to_string(),
            all: accumulate(records, |r| r.condition == *c),
            heldout: accumulate(records, |r| r.condition == *c && r.split == "heldout"),
            success: accumulate(records, |r| r.condition == *c && r.oracle_success),
            failure: accumulate(records, |r| r.condition == *c && !r.oracle_success),
            cache: cache_metrics(records, |r| r.condition == *c),
            heldout_cache: cache_metrics(records, |r| r.condition == *c && r.split == "heldout"),
        })
        .collect();

    let heldout_all = |r: &EpisodeRecord| r.split == "heldout";
    let cache_overall = cache_metrics(records, |_| true);
    let cache_heldout = cache_metrics(records, heldout_all);
    let cache_observable = records.iter().any(|r| {
        r.model_requests
            .iter()
            .any(|q| q.cached_prompt_tokens.is_some())
    });

    let position_cache_audit: Vec<PositionCacheRow> = CONDITIONS
        .iter()
        .flat_map(|c| {
            (1..=EXPECTED_CONDITIONS as u32).map(move |pos| {
                let rows = cache_metrics(records, |r| {
                    r.condition == *c && r.condition_position == pos
                });
                PositionCacheRow {
                    condition: (*c).to_string(),
                    position: pos,
                    episodes: records
                        .iter()
                        .filter(|r| r.condition == *c && r.condition_position == pos)
                        .count() as u32,
                    nominal_prompt_tokens: rows.nominal_prompt_tokens,
                    cached_prompt_tokens: rows.cached_prompt_tokens,
                    uncached_prompt_tokens: rows.uncached_prompt_tokens,
                    cache_hit_ratio: rows.cache_hit_ratio,
                }
            })
        })
        .collect();

    let heldout_comparisons: Vec<PairComparison> = ["candidate_repair", "candidate_regression"]
        .iter()
        .map(|c| compare(records, registry, "heldout", c))
        .collect();
    let calibration_comparisons: Vec<PairComparison> = ["candidate_repair", "candidate_regression"]
        .iter()
        .map(|c| compare(records, registry, "calibration", c))
        .collect();

    let mut violations = Vec::new();
    for r in records {
        if let Some(v) = audit_record(r) {
            violations.push(format!("{}: {v}", r.run_id));
        }
    }
    let passed = violations.is_empty();
    let passed_episodes = total - violations.len() as u32;
    // Machine-derived fault accounting, all values computed from the
    // immutable records: registered mode vs actually-triggered fault are
    // reported as separate quantities (see FaultAudit docs).
    let mut reg_reliable = 0u32;
    let mut reg_drop = 0u32;
    let mut drop_with_write = 0u32;
    let mut drop_without_write = 0u32;
    let mut triggered = 0u32;
    let mut total_dropped = 0u32;
    for r in records {
        let mode = r
            .fault_mode
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("");
        let dropped = r
            .environment_write_attempts
            .iter()
            .filter(|a| !a.applied)
            .count() as u32;
        total_dropped += dropped;
        match mode {
            "reliable" => reg_reliable += 1,
            "drop_first_write" => {
                reg_drop += 1;
                let key = r
                    .fault_mode
                    .get("key")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if r.environment_write_attempts.iter().any(|a| a.key == key) {
                    drop_with_write += 1;
                } else {
                    drop_without_write += 1;
                }
                if dropped > 0 {
                    triggered += 1;
                }
            }
            _ => {}
        }
    }
    let fault_audit = FaultAudit {
        episodes_checked: total,
        episodes_passed: passed_episodes,
        violations,
        passed,
        registered_reliable_episodes: reg_reliable,
        registered_drop_first_write_episodes: reg_drop,
        drop_mode_episodes_with_target_write: drop_with_write,
        drop_mode_episodes_without_target_write: drop_without_write,
        triggered_drop_episodes: triggered,
        total_dropped_write_attempts: total_dropped,
    };

    // Artifact completeness: exactly the registered factorial design and
    // a clean fault audit.
    let mut tuple_count: BTreeMap<(String, String, u32), u32> = BTreeMap::new();
    for r in records {
        *tuple_count
            .entry((r.task_id.clone(), r.condition.clone(), r.repetition))
            .or_insert(0) += 1;
    }
    let mut complete = records.len() == EXPECTED_EPISODES && fault_audit.passed;
    for ((task_id, condition, rep), n) in &tuple_count {
        if *n != 1
            || !registry.iter().any(|t| &t.id == task_id)
            || !CONDITIONS.contains(&condition.as_str())
            || !((1..=EXPECTED_REPETITIONS).contains(rep))
        {
            complete = false;
        }
    }
    if registry.len() == EXPECTED_TASKS {
        for t in registry {
            for c in CONDITIONS {
                for rep in 1..=EXPECTED_REPETITIONS {
                    if !tuple_count.contains_key(&(t.id.clone(), c.to_string(), rep)) {
                        complete = false;
                    }
                }
            }
        }
    } else {
        complete = false;
    }

    // Pre-registered experiment-level conclusion (spec §48).
    let mut reasons = Vec::new();
    let mut result = "supported".to_string();
    let infra_exceeds = infra_rate > INFRA_FAILURE_RATE_THRESHOLD;
    if !complete {
        result = "inconclusive".to_string();
        reasons.push("artifact completeness failed".to_string());
    }
    if infra_exceeds {
        result = "inconclusive".to_string();
        reasons.push(format!(
            "infrastructure failure rate {infra_rate:.4} exceeds {INFRA_FAILURE_RATE_THRESHOLD:.2}"
        ));
    }
    if !infra_exceeds && complete {
        let mut insufficient = Vec::new();
        for cmp in &heldout_comparisons {
            if cmp.insufficient_pairs {
                insufficient.push(format!(
                    "{}: {} valid held-out pairs (< {MIN_VALID_HELDOUT_PAIRS})",
                    cmp.candidate, cmp.valid_pairs
                ));
            }
        }
        if !insufficient.is_empty() {
            result = "inconclusive".to_string();
            reasons.extend(insufficient);
        }
    }
    if result == "supported" {
        let repair_ok = heldout_comparisons.iter().any(|c| {
            c.candidate == "candidate_repair" && c.classification == "quality_improvement"
        });
        let regression_ok = heldout_comparisons.iter().any(|c| {
            c.candidate == "candidate_regression" && c.classification == "quality_regression"
        });
        if repair_ok && regression_ok {
            reasons.push(
                "held-out: repair classified quality_improvement, regression classified \
                 quality_regression"
                    .to_string(),
            );
        } else {
            result = "refuted".to_string();
            for c in &heldout_comparisons {
                reasons.push(format!(
                    "held-out: {} = {} (expected {})",
                    c.candidate,
                    c.classification,
                    if c.candidate == "candidate_repair" {
                        "quality_improvement"
                    } else {
                        "quality_regression"
                    }
                ));
            }
        }
    }
    if reasons.is_empty() {
        reasons.push("pre-registered inconclusive criteria not triggered".to_string());
    }

    Summary {
        experiment_id: EXPERIMENT_ID.to_string(),
        model,
        redacted_endpoint: endpoint,
        temperature: temp,
        code_under_test_commit: commit,
        baseline_prompt_sha256: bh,
        repair_prompt_sha256: rh,
        regression_prompt_sha256: gh,
        task_suite_sha256: suite_h,
        episodes_expected: EXPECTED_EPISODES as u32,
        episodes_total: total,
        artifacts_complete: complete,
        oracle_successes: successes,
        agent_failures: agent,
        infrastructure_failures: infra,
        infrastructure_failure_rate: infra_rate,
        infrastructure_failure_rate_exceeds_threshold: infra_exceeds,
        agent_failure_breakdown: agent_breakdown,
        infrastructure_failure_breakdown: infra_breakdown,
        conditions,
        cache_overall,
        cache_heldout,
        cache_observable,
        position_cache_audit,
        heldout_comparisons,
        calibration_comparisons,
        fault_audit,
        conclusion: Conclusion { result, reasons },
    }
}

// ===========================================================================
// Artifact verifier (spec §62)
// ===========================================================================

/// Keys that must never appear anywhere in a raw artifact record.
/// Reasoning *content* and credential material are redacted by
/// construction; this check guards against regressions.
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
/// the schema cannot validate but which is a known agent failure).
fn check_call_schema(name: &str, args: &Value) -> Option<String> {
    let obj = match name {
        "state_read" | "state_write" => {
            if args.is_string() {
                return None; // verbatim unparseable raw arguments
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
        .and_then(StateKey::parse)
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

/// Full artifact verification (spec §62). Operates on the raw JSON
/// values so that field-level tampering (e.g. leaked
/// `reasoning_content`) is detectable, then on the typed records.
///
/// Returns the list of verification errors; empty means the artifacts
/// are complete and self-consistent.
pub fn verify_artifacts(
    record_values: &[Value],
    summary_value: &Value,
    registry: &[TaskSpec],
) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();

    // --- record count (hardcoded design, never inferred) ---
    if record_values.len() != EXPECTED_EPISODES {
        errors.push(format!(
            "expected exactly {EXPECTED_EPISODES} records, found {}",
            record_values.len()
        ));
    }

    // --- field-level tampering: forbidden keys ---
    for (i, v) in record_values.iter().enumerate() {
        scan_forbidden_keys(v, &format!("_records[{i}]"), &mut errors);
    }

    // --- deserialize ---
    let mut records: Vec<EpisodeRecord> = Vec::with_capacity(record_values.len());
    for (i, v) in record_values.iter().enumerate() {
        match serde_json::from_value::<EpisodeRecord>(v.clone()) {
            Ok(r) => records.push(r),
            Err(e) => errors.push(format!("record[{i}] does not deserialize: {e}")),
        }
    }

    let by_id: BTreeMap<&str, &TaskSpec> = registry.iter().map(|t| (t.id.as_str(), t)).collect();

    // --- registration + factorial tuples ---
    let mut tuples: BTreeMap<(String, String, u32), u32> = BTreeMap::new();
    for (i, r) in records.iter().enumerate() {
        let tag = format!("record[{}]({})", i, r.run_id);
        if r.experiment_id != EXPERIMENT_ID {
            errors.push(format!("{tag}: experiment_id is not {EXPERIMENT_ID:?}"));
        }
        match by_id.get(r.task_id.as_str()) {
            Some(t) => {
                if r.task_name != t.name {
                    errors.push(format!("{tag}: task_name does not match registry"));
                }
                if r.split != t.split.as_str() {
                    errors.push(format!("{tag}: split does not match registry"));
                }
                if r.initial_state != t.initial_state {
                    errors.push(format!("{tag}: initial_state does not match registry"));
                }
                if r.target_state != t.target_state {
                    errors.push(format!("{tag}: target_state does not match registry"));
                }
                let fv: Value = serde_json::to_value(&t.fault).unwrap_or(Value::Null);
                if r.fault_mode != fv {
                    errors.push(format!("{tag}: fault_mode does not match registry"));
                }
            }
            None => errors.push(format!("{tag}: unknown task {}", r.task_id)),
        }
        if !CONDITIONS.contains(&r.condition.as_str()) {
            errors.push(format!("{tag}: unknown condition {}", r.condition));
        }
        if !(1..=EXPECTED_REPETITIONS).contains(&r.repetition) {
            errors.push(format!(
                "{tag}: repetition {} outside 1..={EXPECTED_REPETITIONS}",
                r.repetition
            ));
        }
        if !((1..=EXPECTED_CONDITIONS as u32).contains(&r.condition_position)) {
            errors.push(format!(
                "{tag}: condition_position {} out of range",
                r.condition_position
            ));
        }
        *tuples
            .entry((r.task_id.clone(), r.condition.clone(), r.repetition))
            .or_insert(0) += 1;
    }
    for ((task_id, condition, rep), n) in &tuples {
        if *n > 1 {
            errors.push(format!(
                "duplicate tuple: task {task_id}, condition {condition}, rep {rep} ×{n}"
            ));
        }
    }
    if registry.len() == EXPECTED_TASKS {
        for t in registry {
            for c in CONDITIONS {
                for rep in 1..=EXPECTED_REPETITIONS {
                    if !tuples.contains_key(&(t.id.clone(), c.to_string(), rep)) {
                        errors.push(format!(
                            "missing tuple: task {}, condition {c}, rep {rep}",
                            t.id
                        ));
                    }
                }
            }
        }
    }

    // --- registered six-permutation order + position balance ---
    for t in registry {
        for rep in 1..=EXPECTED_REPETITIONS {
            let got: Vec<&str> = records
                .iter()
                .filter(|r| r.task_id == t.id && r.repetition == rep)
                .map(|r| r.condition.as_str())
                .collect();
            let expected: Vec<&str> = PERMUTATIONS[(rep - 1) as usize].to_vec();
            if got.len() != EXPECTED_CONDITIONS || got != expected {
                errors.push(format!(
                    "task {} rep {rep}: condition order {:?} does not match registered {:?}",
                    t.id, got, expected
                ));
            }
        }
        for c in CONDITIONS {
            let mut pos: [u32; EXPECTED_CONDITIONS] = [0; EXPECTED_CONDITIONS];
            let mut known = true;
            for r in records
                .iter()
                .filter(|r| r.task_id == t.id && r.condition == c)
            {
                if (1..=EXPECTED_CONDITIONS as u32).contains(&r.condition_position) {
                    pos[(r.condition_position - 1) as usize] += 1;
                } else {
                    known = false;
                }
            }
            if !known || pos.iter().any(|&n| n != 2) {
                errors.push(format!(
                    "task {}: condition {} position balance not 2/2/2 (got {pos:?})",
                    t.id, c
                ));
            }
        }
    }

    // --- single run prefix + strict sequence ---
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
        let p = r.run_id.rsplit_once('-').map(|(p, _)| p);
        match p {
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
    let suite_hash = crate::experiment::task_suite_sha256(registry)
        .unwrap_or_else(|e| format!("__suite_hash_error__:{e}"));
    let mut consistent = |get: fn(&EpisodeRecord) -> &str, name: &str| {
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
    consistent(|r| &r.regression_prompt_sha256, "regression_prompt_sha256");
    consistent(|r| &r.task_suite_sha256, "task_suite_sha256");
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
        if records[0].task_suite_sha256 != suite_hash {
            errors.push("task_suite_sha256 does not match the registered task suite".to_string());
        }
    }

    // --- per-record structural checks ---
    for (i, r) in records.iter().enumerate() {
        let tag = format!("record[{}]", i);

        // Termination / failure consistency.
        match TerminationReason::parse(&r.termination_reason) {
            Some(term) => {
                let agent_expected = term.is_agent_failure();
                let infra_expected = term.is_infrastructure();
                if r.agent_failure.is_some() && r.infrastructure_failure.is_some() {
                    errors.push(format!("{tag}: both agent and infrastructure failure set"));
                }
                if agent_expected != r.agent_failure.is_some() {
                    errors.push(format!(
                        "{tag}: agent_failure {} does not match termination {:?}",
                        r.agent_failure.as_deref().unwrap_or("<none>"),
                        r.termination_reason
                    ));
                }
                if let Some(a) = &r.agent_failure {
                    if a != term.as_str() {
                        errors.push(format!("{tag}: agent_failure {a:?} mismatches termination"));
                    }
                }
                if infra_expected != r.infrastructure_failure.is_some() {
                    errors.push(format!(
                        "{tag}: infrastructure_failure {} does not match termination {:?}",
                        r.infrastructure_failure.as_deref().unwrap_or("<none>"),
                        r.termination_reason
                    ));
                }
                if let Some(x) = &r.infrastructure_failure {
                    if x != term.as_str() {
                        errors.push(format!(
                            "{tag}: infrastructure_failure {x:?} mismatches termination"
                        ));
                    }
                }
            }
            None => errors.push(format!(
                "{tag}: unregistered termination_reason {:?}",
                r.termination_reason
            )),
        }

        // Per-request usage arithmetic.
        for q in &r.model_requests {
            let qtag = format!("{tag} request turn {}", q.turn);
            if let (Some(p), Some(c)) = (q.prompt_tokens, q.cached_prompt_tokens) {
                if c > p {
                    errors.push(format!(
                        "{qtag}: cached_prompt_tokens {c} > prompt_tokens {p}"
                    ));
                }
            }
            let expected_uncached = match (q.prompt_tokens, q.cached_prompt_tokens) {
                (Some(p), Some(c)) if c <= p => Some(p - c),
                _ => None,
            };
            if q.uncached_prompt_tokens != expected_uncached {
                errors.push(format!(
                    "{qtag}: uncached_prompt_tokens {:?} inconsistent with prompt/cached {:?}/{:?}",
                    q.uncached_prompt_tokens, q.prompt_tokens, q.cached_prompt_tokens
                ));
            }
            if let (Some(p), Some(c), Some(t)) =
                (q.prompt_tokens, q.completion_tokens, q.total_tokens)
            {
                if t != p + c {
                    errors.push(format!(
                        "{qtag}: total_tokens {t} != prompt {p} + completion {c}"
                    ));
                }
            }
        }
        for (a, b) in r.model_requests.iter().zip(r.model_requests.iter().skip(1)) {
            if b.turn != a.turn + 1 {
                errors.push(format!("{tag}: model_requests turns are not sequential"));
                break;
            }
        }

        // Requested / executed integrity: executed calls are an
        // order-preserving, identity-equal prefix of requested calls.
        for (k, c) in r.requested_tool_calls.iter().enumerate() {
            if c.sequence != k as u32 {
                errors.push(format!(
                    "{tag}: requested_tool_calls sequence not contiguous"
                ));
                break;
            }
            if c.turn == 0 || c.turn > r.model_turn_count || c.turn > r.model_requests.len() as u32
            {
                errors.push(format!(
                    "{tag}: requested call turn {} inconsistent with model turns",
                    c.turn
                ));
            }
            if let Some(e) = check_call_schema(&c.tool_name, &c.arguments) {
                errors.push(format!("{tag}: requested call: {e}"));
            }
        }
        if r.tool_call_count != r.executed_tool_calls.len() as u32 {
            errors.push(format!(
                "{tag}: tool_call_count {} != executed_tool_calls len {}",
                r.tool_call_count,
                r.executed_tool_calls.len()
            ));
        }
        for (k, e) in r.executed_tool_calls.iter().enumerate() {
            if e.sequence != k as u32 {
                errors.push(format!(
                    "{tag}: executed_tool_calls sequence not contiguous"
                ));
                break;
            }
            let req = r.requested_tool_calls.get(k);
            match req {
                None => {
                    errors.push(format!(
                        "{tag}: executed call has no matching requested call"
                    ));
                }
                Some(q) => {
                    if q.turn != e.turn
                        || q.tool_call_id != e.tool_call_id
                        || q.tool_name != e.tool_name
                        || canonical_json(&q.arguments) != canonical_json(&e.arguments)
                    {
                        errors.push(format!(
                            "{tag}: executed call {k} does not match its requested call"
                        ));
                    }
                }
            }
            // Model-visible result shapes (fault must stay invisible).
            match e.tool_name.as_str() {
                "state_write" => {
                    let res_key = e.result.get("key").and_then(Value::as_str);
                    let arg_key = e.arguments.get("key").and_then(Value::as_str);
                    let res_value = e.result.get("value").and_then(Value::as_str);
                    let arg_value = e.arguments.get("value").and_then(Value::as_str);
                    if e.result.get("ok") != Some(&Value::Bool(true))
                        || res_key != arg_key
                        || res_value != arg_value
                    {
                        errors.push(format!(
                            "{tag}: write result does not mirror the requested write"
                        ));
                    }
                    if let Some(obj) = e.result.as_object() {
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
                    if e.result.get("ok").is_some() {
                        errors.push(format!("{tag}: read result has unexpected shape"));
                    }
                }
                other => errors.push(format!("{tag}: executed call with unknown tool {other:?}")),
            }
        }

        // Write-attempt audit vs executed writes.
        let writes: Vec<&ExecutedToolCall> = r
            .executed_tool_calls
            .iter()
            .filter(|e| e.tool_name == "state_write")
            .collect();
        if writes.len() != r.environment_write_attempts.len() {
            errors.push(format!(
                "{tag}: {} executed writes vs {} write attempts",
                writes.len(),
                r.environment_write_attempts.len()
            ));
        } else {
            for (w, a) in writes.iter().zip(r.environment_write_attempts.iter()) {
                if a.key != w.arguments.get("key").and_then(Value::as_str).unwrap_or("")
                    || a.requested_value
                        != w.arguments
                            .get("value")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                {
                    errors.push(format!(
                        "{tag}: write attempt does not match executed write"
                    ));
                }
            }
        }
        for (k, a) in r.environment_write_attempts.iter().enumerate() {
            if a.sequence != k as u32 {
                errors.push(format!("{tag}: write attempt sequence not contiguous"));
                break;
            }
        }

        // Fault schedule executed exactly as registered.
        if let Some(v) = audit_record(r) {
            errors.push(format!("{tag}: fault audit: {v}"));
        }

        // Oracle recomputation (anti-circularity: task + input only).
        let t = by_id.get(r.task_id.as_str());
        if let Some(t) = t {
            let eval = oracle::evaluate_task(
                t,
                &oracle::OracleInput {
                    termination_reason: r.termination_reason.clone(),
                    initial_state: r.initial_state.clone(),
                    final_state: r.final_state.clone(),
                },
            );
            if eval.success != r.oracle_success {
                errors.push(format!(
                    "{tag}: stored oracle_success {} != recomputed {}",
                    r.oracle_success, eval.success
                ));
            }
            if eval.reasons != r.oracle_failure_reasons {
                errors.push(format!(
                    "{tag}: oracle failure reasons mismatch recomputation"
                ));
            }
            if eval.success && !r.oracle_failure_reasons.is_empty() {
                errors.push(format!("{tag}: successful episode has failure reasons"));
            }
        }
    }

    // --- summary deep-equality ---
    let recomputed = compute_summary(&records, registry);
    // Post-hoc fix (recorded in the experiment document): serde_json 1.0.151
    // has a 1 ulp inaccuracy in its decimal-to-f64 parser, so the summary
    // parsed from the on-disk file can differ from the in-memory
    // recomputation in the low bit of f64 fields (e.g. `cache_hit_ratio`)
    // even though both derive from identical integer sums. Canonicalize the
    // recomputed summary through the same serialize-then-parse pipeline as
    // the on-disk file before the deep comparison. This changes no
    // measurement; the raw artifacts remain exactly as written.
    let recomputed_value: Value =
        serde_json::from_str(&serde_json::to_string(&recomputed).unwrap()).unwrap_or(Value::Null);
    if canonical_json(&recomputed_value) != canonical_json(summary_value) {
        errors.push(
            "summary does not deep-equal the recomputed summary from raw records".to_string(),
        );
    }

    errors
}

// ===========================================================================
// Synthetic network-free dataset (self-test + verifier regression tests)
// ===========================================================================

struct Behavior {
    termination: TerminationReason,
    /// (tool, key, value) for each executed call, in order. For reads,
    /// the value is decorative: recorded read results are recomputed
    /// from the simulated state.
    calls: Vec<(String, String, String)>,
    final_answer: bool,
    infra: bool,
}

/// The scripted behavior of each (condition, task, rep) in the
/// synthetic dataset. Encodes the hypothesized qualitative pattern
/// (spec §59) as deterministic data — it is not a prediction about the
/// real model.
fn synthetic_behavior(task: &TaskSpec, condition: &str, rep: u32) -> Behavior {
    let is_drop = !task.fault.is_reliable();
    let target_key = task
        .target_state
        .as_object()
        .and_then(|t| {
            let mut keys: Vec<String> = t.keys().cloned().collect();
            keys.sort();
            keys.into_iter().find(|k| {
                task.initial_state
                    .get(k)
                    .map(|v| *v != t[k])
                    .unwrap_or(false)
            })
        })
        .unwrap_or_else(|| "x".to_string());
    let init_x = task
        .initial_state
        .get("x")
        .and_then(Value::as_str)
        .unwrap_or("EMPTY")
        .to_string();
    let init_y = task
        .initial_state
        .get("y")
        .and_then(Value::as_str)
        .unwrap_or("B")
        .to_string();
    let target_x = task
        .target_state
        .get("x")
        .and_then(Value::as_str)
        .unwrap_or(init_x.as_str())
        .to_string();
    let target_y = task
        .target_state
        .get("y")
        .and_then(Value::as_str)
        .unwrap_or(init_y.as_str())
        .to_string();

    // The key being changed, and its initial / target values.
    let changed = target_key;
    let (changed_init, changed_target) = if changed == "x" {
        (init_x, target_x)
    } else {
        (init_y, target_y)
    };
    let w = |v: &str| ("state_write".to_string(), changed.clone(), v.to_string());
    let r = |v: &str| ("state_read".to_string(), changed.clone(), v.to_string());
    let wt = || w(changed_target.as_str());
    let it = || r(changed_init.as_str());
    let rt = || r(changed_target.as_str());
    if condition == "candidate_regression" {
        Behavior {
            termination: TerminationReason::Completed,
            calls: Vec::new(),
            final_answer: true,
            infra: false,
        }
    } else if condition == "candidate_repair" {
        if is_drop {
            // write (dropped), read (stale), retry write (applied), read
            Behavior {
                termination: TerminationReason::Completed,
                calls: vec![wt(), it(), wt(), rt()],
                final_answer: true,
                infra: false,
            }
        } else {
            // write (applied), read (confirms)
            Behavior {
                termination: TerminationReason::Completed,
                calls: vec![wt(), rt()],
                final_answer: true,
                infra: false,
            }
        }
    } else {
        // baseline
        if !is_drop {
            Behavior {
                termination: TerminationReason::Completed,
                calls: vec![wt()],
                final_answer: true,
                infra: false,
            }
        } else if rep <= 3 {
            // The model independently verifies and recovers.
            Behavior {
                termination: TerminationReason::Completed,
                calls: vec![wt(), it(), wt(), rt()],
                final_answer: true,
                infra: false,
            }
        } else if rep <= 5 {
            // The dropped write is never detected: state stays initial.
            Behavior {
                termination: TerminationReason::Completed,
                calls: vec![wt()],
                final_answer: true,
                infra: false,
            }
        } else if task.id == "C2" && rep == 6 {
            // The single infrastructure failure in the synthetic set:
            // one request, no usable response.
            Behavior {
                termination: TerminationReason::HttpError,
                calls: Vec::new(),
                final_answer: false,
                infra: true,
            }
        } else {
            // Turn limit: the model keeps retrying; six writes means the
            // state eventually reaches the target, but without a final
            // answer the episode still fails.
            let mut calls = Vec::new();
            for _ in 0..6 {
                calls.push(wt());
                calls.push(rt());
            }
            Behavior {
                termination: TerminationReason::TurnLimit,
                calls,
                final_answer: false,
                infra: false,
            }
        }
    }
}

/// Build the full deterministic 144-record synthetic dataset for the
/// given registry. Satisfies every verifier rule, including the fault
/// audit, the permutation design, and usage arithmetic.
pub fn synthetic_records(registry: &[TaskSpec]) -> Vec<EpisodeRecord> {
    let suite_hash = crate::experiment::task_suite_sha256(registry).unwrap_or_default();
    let hashes: [(&str, &str); 3] = [
        ("baseline", &"1".repeat(64)),
        ("candidate_repair", &"2".repeat(64)),
        ("candidate_regression", &"3".repeat(64)),
    ];

    let mut records = Vec::with_capacity(EXPECTED_EPISODES);
    let mut seq = 0u32;
    for task in registry {
        for rep in 1..=EXPECTED_REPETITIONS {
            for (pos, condition) in PERMUTATIONS[(rep - 1) as usize].iter().enumerate() {
                seq += 1;
                let behavior = synthetic_behavior(task, condition, rep);

                // Simulate the fault schedule for write attempts.
                let drop_key = task.fault.target_key().map(|k| k.as_str().to_string());
                let mut drop_fired = false;
                let mut write_attempts = Vec::new();
                for (tool, key, value) in &behavior.calls {
                    if *tool == "state_write" {
                        let dropped = drop_key.as_deref() == Some(key.as_str()) && !drop_fired;
                        if dropped {
                            drop_fired = true;
                        }
                        write_attempts.push(WriteAttempt {
                            sequence: write_attempts.len() as u32,
                            key: key.clone(),
                            requested_value: value.clone(),
                            applied: !dropped,
                            fault_reason: dropped.then_some("drop_first_write".to_string()),
                        });
                    }
                }

                // Final state: apply the executed writes to the initial
                // state under the simulated fault.
                let mut final_state = task.initial_state.clone();
                drop_fired = false;
                for (tool, key, value) in &behavior.calls {
                    if *tool == "state_write" {
                        let dropped = drop_key.as_deref() == Some(key.as_str()) && !drop_fired;
                        if dropped {
                            drop_fired = true;
                        }
                        if !dropped {
                            if let Some(obj) = final_state.as_object_mut() {
                                obj.insert(key.clone(), Value::String(value.clone()));
                            }
                        }
                    }
                }

                // Model requests: one per executed turn, plus the final
                // answer turn when the episode completed.
                let completed = behavior.termination == TerminationReason::Completed;
                let turns = if behavior.infra {
                    0
                } else {
                    behavior.calls.len() + if completed { 1 } else { 0 }
                };
                let mut model_requests = Vec::new();
                for turn in 1..=turns.max(1) as u32 {
                    // The single synthetic infra episode: one request
                    // with no usable response and no reported usage.
                    if behavior.infra {
                        model_requests.push(ModelRequestRecord {
                            turn: 1,
                            finish_reason: None,
                            prompt_tokens: None,
                            completion_tokens: None,
                            total_tokens: None,
                            cached_prompt_tokens: None,
                            reasoning_tokens: None,
                            uncached_prompt_tokens: None,
                        });
                        break;
                    }
                    let prompt = 500u64 + 100u64 * turn as u64;
                    let cached = if turn == 1 { 0u64 } else { 300 };
                    let completion = 10u64 * turn as u64;
                    model_requests.push(ModelRequestRecord {
                        turn,
                        finish_reason: Some(if turn <= behavior.calls.len() as u32 {
                            "tool_calls".to_string()
                        } else {
                            "stop".to_string()
                        }),
                        prompt_tokens: Some(prompt),
                        completion_tokens: Some(completion),
                        total_tokens: Some(prompt + completion),
                        cached_prompt_tokens: Some(cached),
                        reasoning_tokens: if turn == turns as u32 { Some(5) } else { None },
                        uncached_prompt_tokens: Some(prompt - cached),
                    });
                }

                // Requested / executed tool calls.
                let mut requested = Vec::new();
                let mut executed = Vec::new();
                let mut sim_state = task.initial_state.clone();
                drop_fired = false;
                for (i, (tool, key, value)) in behavior.calls.iter().enumerate() {
                    let turn = (i + 1) as u32;
                    let id = format!("call-{turn}");
                    let args = if *tool == "state_write" {
                        json!({ "key": key, "value": value })
                    } else {
                        json!({ "key": key })
                    };
                    requested.push(RequestedToolCall {
                        sequence: i as u32,
                        turn,
                        tool_call_id: id.clone(),
                        tool_name: tool.clone(),
                        arguments: args.clone(),
                    });
                    // Read results reflect the simulated current state.
                    let current = if *tool == "state_read" {
                        sim_state.get(key).cloned().unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    };
                    if *tool == "state_write" {
                        let dropped = drop_key.as_deref() == Some(key.as_str()) && !drop_fired;
                        if dropped {
                            drop_fired = true;
                        }
                        if !dropped {
                            if let Some(obj) = sim_state.as_object_mut() {
                                obj.insert(key.clone(), Value::String(value.clone()));
                            }
                        }
                    }
                    let result = if *tool == "state_write" {
                        json!({ "ok": true, "key": key, "value": value })
                    } else {
                        json!({ "key": key, "value": current })
                    };
                    executed.push(ExecutedToolCall {
                        sequence: i as u32,
                        turn,
                        tool_call_id: id,
                        tool_name: tool.clone(),
                        arguments: args,
                        result,
                        duration_us: 1_000,
                    });
                }

                let term = behavior.termination;
                let agent_failure = term.is_agent_failure().then(|| term.as_str().to_string());
                let infrastructure_failure =
                    term.is_infrastructure().then(|| term.as_str().to_string());

                // Model-visible final answer, when one exists.
                let final_answer = behavior
                    .final_answer
                    .then(|| "The requested external state has been achieved.".to_string());

                let mut record = EpisodeRecord {
                    experiment_id: EXPERIMENT_ID.to_string(),
                    run_id: format!("exp0007-selftest-{seq:03}"),
                    sequence: seq,
                    task_id: task.id.clone(),
                    task_name: task.name.clone(),
                    split: task.split.as_str().to_string(),
                    condition: condition.to_string(),
                    repetition: rep,
                    condition_position: (pos + 1) as u32,
                    model: "selftest-synthetic".to_string(),
                    temperature: TEMPERATURE,
                    redacted_endpoint: "http://selftest.invalid/v1".to_string(),
                    code_under_test_commit: "0".repeat(40),
                    baseline_prompt_sha256: hashes[0].1.to_string(),
                    repair_prompt_sha256: hashes[1].1.to_string(),
                    regression_prompt_sha256: hashes[2].1.to_string(),
                    task_suite_sha256: suite_hash.clone(),
                    initial_state: task.initial_state.clone(),
                    target_state: task.target_state.clone(),
                    fault_mode: serde_json::to_value(&task.fault).unwrap_or(Value::Null),
                    model_requests,
                    requested_tool_calls: requested,
                    executed_tool_calls: executed,
                    environment_write_attempts: write_attempts,
                    final_state,
                    final_answer,
                    model_turn_count: turns as u32,
                    tool_call_count: behavior.calls.len() as u32,
                    agent_failure,
                    infrastructure_failure,
                    usage_accounting_error: None,
                    termination_reason: term.as_str().to_string(),
                    oracle_success: false,
                    oracle_failure_reasons: Vec::new(),
                    timing: Timing {
                        wall_time_ms: 1_000 + 500 * turns as u64,
                        model_wait_time_ms: 800 * turns as u64,
                        tool_execution_time_us: 1_000 * behavior.calls.len() as u64,
                        kernel_overhead_estimate_ms: 100 * turns as u64,
                        approximate: true,
                    },
                };

                let eval = oracle::evaluate_task(
                    task,
                    &oracle::OracleInput {
                        termination_reason: record.termination_reason.clone(),
                        initial_state: record.initial_state.clone(),
                        final_state: record.final_state.clone(),
                    },
                );
                record.oracle_success = eval.success;
                record.oracle_failure_reasons = eval.reasons;
                records.push(record);
            }
        }
    }
    records
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experiment;

    fn registry() -> Vec<TaskSpec> {
        experiment::load_tasks(&crate::experiment::crate_dir().join("tasks.json")).unwrap()
    }

    fn values(records: &[EpisodeRecord]) -> Vec<Value> {
        records
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn verify_all(records: &[EpisodeRecord], registry: &[TaskSpec]) -> Vec<String> {
        let summary = compute_summary(records, registry);
        verify_artifacts(
            &values(records),
            &serde_json::to_value(&summary).unwrap(),
            registry,
        )
    }

    #[test]
    fn synthetic_dataset_passes_the_full_verifier() {
        let reg = registry();
        let records = synthetic_records(&reg);
        assert_eq!(records.len(), EXPECTED_EPISODES);
        let errors = verify_all(&records, &reg);
        assert!(errors.is_empty(), "unexpected: {errors:?}");
    }

    #[test]
    fn synthetic_summary_conclusion_is_supported() {
        let reg = registry();
        let records = synthetic_records(&reg);
        let s = compute_summary(&records, &reg);
        assert_eq!(s.conclusion.result, "supported");
        assert!(s.artifacts_complete);
        assert!(!s.infrastructure_failure_rate_exceeds_threshold);
        // One infra episode (synthetic baseline C2 rep 6).
        assert_eq!(s.infrastructure_failures, 1);
    }

    #[test]
    fn sign_test_and_classification_in_summary() {
        let reg = registry();
        let records = synthetic_records(&reg);
        let s = compute_summary(&records, &reg);
        let repair = s
            .heldout_comparisons
            .iter()
            .find(|c| c.candidate == "candidate_repair")
            .unwrap();
        assert_eq!(repair.wins, 6);
        assert_eq!(repair.losses, 0);
        assert_eq!(repair.ties, 18);
        assert!((repair.sign_test_p - 0.03125).abs() < 1e-12);
        assert_eq!(repair.classification, "quality_improvement");
        let regression = s
            .heldout_comparisons
            .iter()
            .find(|c| c.candidate == "candidate_regression")
            .unwrap();
        assert_eq!(regression.losses, 18);
        assert_eq!(regression.wins, 0);
        assert_eq!(regression.classification, "quality_regression");
    }

    fn tamper(records: &[EpisodeRecord], f: impl Fn(&mut Value)) -> Vec<Value> {
        let mut v = values(records);
        f(v.last_mut().unwrap());
        v
    }

    #[test]
    fn verifier_fails_on_143_records() {
        let reg = registry();
        let mut records = synthetic_records(&reg);
        records.pop();
        let s = compute_summary(&records, &reg);
        let errors = verify_artifacts(&values(&records), &serde_json::to_value(&s).unwrap(), &reg);
        assert!(errors.iter().any(|e| e.contains("expected exactly 144")));
    }

    #[test]
    fn verifier_fails_on_145_records() {
        let reg = registry();
        let mut records = synthetic_records(&reg);
        records.push(records[0].clone());
        let s = compute_summary(&records, &reg);
        let errors = verify_artifacts(&values(&records), &serde_json::to_value(&s).unwrap(), &reg);
        assert!(errors.iter().any(|e| e.contains("expected exactly 144")));
        assert!(errors.iter().any(|e| e.contains("duplicate")));
    }

    #[test]
    fn verifier_fails_on_missing_tuple() {
        let reg = registry();
        let mut records = synthetic_records(&reg);
        records.remove(0);
        let s = compute_summary(&records, &reg);
        let errors = verify_artifacts(&values(&records), &serde_json::to_value(&s).unwrap(), &reg);
        assert!(errors.iter().any(|e| e.contains("missing tuple")));
    }

    #[test]
    fn verifier_fails_on_duplicate_replacing_tuple() {
        let reg = registry();
        let mut records = synthetic_records(&reg);
        records[10] = records[3].clone();
        let s = compute_summary(&records, &reg);
        let errors = verify_artifacts(&values(&records), &serde_json::to_value(&s).unwrap(), &reg);
        assert!(errors.iter().any(|e| e.contains("duplicate")));
        assert!(errors.iter().any(|e| e.contains("missing")));
    }

    #[test]
    fn verifier_fails_on_unknown_task() {
        let reg = registry();
        let records = synthetic_records(&reg);
        let v = tamper(&records, |v| {
            v["task_id"] = Value::from("X9");
        });
        let s = compute_summary(&records, &reg);
        let errors = verify_artifacts(&v, &serde_json::to_value(&s).unwrap(), &reg);
        assert!(errors.iter().any(|e| e.contains("unknown task")));
    }

    #[test]
    fn verifier_fails_on_unknown_condition() {
        let reg = registry();
        let records = synthetic_records(&reg);
        let v = tamper(&records, |v| {
            v["condition"] = Value::from("candidate_unknown");
        });
        let s = compute_summary(&records, &reg);
        let errors = verify_artifacts(&v, &serde_json::to_value(&s).unwrap(), &reg);
        assert!(errors.iter().any(|e| e.contains("unknown condition")));
    }

    #[test]
    fn verifier_fails_on_rep_7() {
        let reg = registry();
        let records = synthetic_records(&reg);
        let v = tamper(&records, |v| {
            v["repetition"] = Value::from(7);
        });
        let s = compute_summary(&records, &reg);
        let errors = verify_artifacts(&v, &serde_json::to_value(&s).unwrap(), &reg);
        assert!(errors.iter().any(|e| e.contains("repetition 7")));
    }

    #[test]
    fn verifier_fails_on_wrong_split() {
        let reg = registry();
        let records = synthetic_records(&reg);
        let v = tamper(&records, |v| {
            v["split"] = Value::from("calibration");
        });
        let s = compute_summary(&records, &reg);
        let errors = verify_artifacts(&v, &serde_json::to_value(&s).unwrap(), &reg);
        assert!(errors.iter().any(|e| e.contains("split")));
    }

    #[test]
    fn verifier_fails_on_wrong_target_state() {
        let reg = registry();
        let records = synthetic_records(&reg);
        let v = tamper(&records, |v| {
            v["target_state"]["x"] = Value::from("WRONG");
        });
        let s = compute_summary(&records, &reg);
        let errors = verify_artifacts(&v, &serde_json::to_value(&s).unwrap(), &reg);
        assert!(errors.iter().any(|e| e.contains("target_state")));
    }

    #[test]
    fn verifier_fails_on_wrong_fault_mode() {
        let reg = registry();
        let records = synthetic_records(&reg);
        let v = tamper(&records, |v| {
            v["fault_mode"] = json!({"mode": "reliable"});
        });
        let s = compute_summary(&records, &reg);
        let errors = verify_artifacts(&v, &serde_json::to_value(&s).unwrap(), &reg);
        assert!(errors.iter().any(|e| e.contains("fault_mode")));
    }

    #[test]
    fn verifier_fails_on_wrong_condition_order() {
        let reg = registry();
        let records = synthetic_records(&reg);
        // The last record is the last task's rep 6, third position. Swap
        // its condition with the second-position record of the same rep.
        let v = tamper(&records, |v| {
            v["condition"] = Value::from("candidate_repair");
            v["condition_position"] = Value::from(2);
        });
        let s = compute_summary(&records, &reg);
        let errors = verify_artifacts(&v, &serde_json::to_value(&s).unwrap(), &reg);
        assert!(!errors.is_empty());
    }

    #[test]
    fn verifier_fails_when_cache_exceeds_prompt() {
        let reg = registry();
        let records = synthetic_records(&reg);
        let v = tamper(&records, |v| {
            let reqs = v["model_requests"].as_array_mut().unwrap();
            let q = reqs.first_mut().unwrap();
            q["cached_prompt_tokens"] = Value::from(q["prompt_tokens"].as_u64().unwrap() + 5);
            q["uncached_prompt_tokens"] = Value::Null;
        });
        let s = compute_summary(&records, &reg);
        let errors = verify_artifacts(&v, &serde_json::to_value(&s).unwrap(), &reg);
        assert!(errors.iter().any(|e| e.contains("cached_prompt_tokens")));
    }

    #[test]
    fn verifier_fails_on_tampered_oracle_success() {
        let reg = registry();
        let records = synthetic_records(&reg);
        let v = tamper(&records, |v| {
            v["oracle_success"] = Value::from(!v["oracle_success"].as_bool().unwrap());
        });
        let s = compute_summary(&records, &reg);
        let errors = verify_artifacts(&v, &serde_json::to_value(&s).unwrap(), &reg);
        assert!(errors.iter().any(|e| e.contains("oracle_success")));
    }

    #[test]
    fn verifier_fails_on_tampered_summary() {
        let reg = registry();
        let records = synthetic_records(&reg);
        let mut sv = serde_json::to_value(compute_summary(&records, &reg)).unwrap();
        sv["conclusion"]["result"] = Value::from("supported");
        sv["oracle_successes"] = Value::from(999u64);
        let errors = verify_artifacts(&values(&records), &sv, &reg);
        assert!(errors.iter().any(|e| e.contains("deep-equal")));
    }

    #[test]
    fn verifier_fails_on_reasoning_content_leak() {
        let reg = registry();
        let records = synthetic_records(&reg);
        let v = tamper(&records, |v| {
            v["reasoning_content"] = Value::from("SECRET");
        });
        let s = compute_summary(&records, &reg);
        let errors = verify_artifacts(&v, &serde_json::to_value(&s).unwrap(), &reg);
        assert!(errors.iter().any(|e| e.contains("forbidden field")));
    }

    #[test]
    fn verifier_fails_on_secret_field_leak() {
        let reg = registry();
        let records = synthetic_records(&reg);
        let v = tamper(&records, |v| {
            v["api_key"] = Value::from("sk-SECRET");
        });
        let s = compute_summary(&records, &reg);
        let errors = verify_artifacts(&v, &serde_json::to_value(&s).unwrap(), &reg);
        assert!(errors.iter().any(|e| e.contains("forbidden field")));
    }

    #[test]
    fn verifier_fails_on_fault_audit_violation() {
        let reg = registry();
        let mut records = synthetic_records(&reg);
        // Record 127 (1-based 127 = task H4, rep 1, position 1 = baseline)
        // has four write attempts under DropFirstWrite(x).
        for a in &mut records[126].environment_write_attempts {
            a.applied = false;
        }
        let v: Vec<Value> = records
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let s = compute_summary(&records, &reg);
        let errors = verify_artifacts(&v, &serde_json::to_value(&s).unwrap(), &reg);
        assert!(errors.iter().any(|e| e.contains("fault audit")));
    }

    // --- machine-derived fault accounting (registered vs triggered) ---

    #[test]
    fn fault_accounting_registered_counts_match_design() {
        let reg = registry();
        let records = synthetic_records(&reg);
        let s = compute_summary(&records, &reg);
        // 8 tasks: C1/C3/H1/H3 reliable, C2/C4/H2/H4 DropFirstWrite;
        // 3 conditions x 6 repetitions = 18 episodes per task.
        assert_eq!(s.fault_audit.registered_reliable_episodes, 72);
        assert_eq!(s.fault_audit.registered_drop_first_write_episodes, 72);
        assert_eq!(
            s.fault_audit.registered_reliable_episodes
                + s.fault_audit.registered_drop_first_write_episodes,
            144
        );
    }

    #[test]
    fn fault_accounting_derives_triggered_from_records() {
        let reg = registry();
        let records = synthetic_records(&reg);
        let s = compute_summary(&records, &reg);
        // In the synthetic set every drop-mode episode except the 24
        // no-write regression episodes and the single no-write
        // infrastructure episode (C2 rep 6 baseline) writes the
        // registered key.
        assert_eq!(s.fault_audit.drop_mode_episodes_with_target_write, 47);
        assert_eq!(s.fault_audit.drop_mode_episodes_without_target_write, 25);
        assert_eq!(
            s.fault_audit.drop_mode_episodes_with_target_write
                + s.fault_audit.drop_mode_episodes_without_target_write,
            72
        );
        // Invariants that hold whenever the per-record audit passes.
        assert!(s.fault_audit.passed);
        assert_eq!(s.fault_audit.triggered_drop_episodes, 47);
        assert_eq!(s.fault_audit.total_dropped_write_attempts, 47);
        // The one-shot schedule: every triggered drop is exactly one
        // dropped attempt, so triggered episodes == total dropped writes.
        assert_eq!(
            s.fault_audit.triggered_drop_episodes,
            s.fault_audit.total_dropped_write_attempts
        );
    }

    #[test]
    fn fault_accounting_triggered_is_distinct_from_registered() {
        let reg = registry();
        let records = synthetic_records(&reg);
        let s = compute_summary(&records, &reg);
        // Registered 72 DropFirstWrite episodes vs 47 triggered drops:
        // the 25 episodes in which the model never wrote the registered
        // key left the registered fault untriggered and must not inflate
        // either the reliable count or the trigger count.
        assert!(
            s.fault_audit.triggered_drop_episodes
                < s.fault_audit.registered_drop_first_write_episodes
        );
        assert_eq!(s.fault_audit.registered_reliable_episodes, 72);
    }

    #[test]
    fn fault_accounting_no_write_drop_episode_stays_drop_mode() {
        let reg = registry();
        let records = synthetic_records(&reg);
        // A DropFirstWrite episode whose model never writes the registered
        // key (synthetic regression episodes have no tool calls).
        let idx = records
            .iter()
            .position(|r| {
                r.fault_mode.get("mode").and_then(Value::as_str) == Some("drop_first_write")
                    && r.environment_write_attempts.is_empty()
            })
            .expect("synthetic set contains a no-write drop-mode episode");
        let r = &records[idx];
        // The per-record audit passes: the registered one-shot drop is
        // conditional on a write occurring; nothing to violate.
        assert!(audit_record(r).is_none());
        let key = r
            .fault_mode
            .get("key")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        assert!(!r.environment_write_attempts.iter().any(|a| a.key == key));
        // And the summary accounts it as registered DropFirstWrite /
        // untriggered — never as Reliable.
        let s = compute_summary(&records, &reg);
        assert_eq!(s.fault_audit.registered_drop_first_write_episodes, 72);
        assert_eq!(s.fault_audit.registered_reliable_episodes, 72);
        assert!(s.fault_audit.drop_mode_episodes_without_target_write > 0);
        assert_eq!(
            s.fault_audit.drop_mode_episodes_with_target_write
                + s.fault_audit.drop_mode_episodes_without_target_write,
            72
        );
    }

    #[test]
    fn fault_accounting_reclassifying_drop_as_reliable_is_detected() {
        let reg = registry();
        let mut records = synthetic_records(&reg);
        // Tamper: a no-write DropFirstWrite record relabeled as Reliable
        // while keeping no dropped writes would pass the *per-write* rules
        // of either mode — the summary must still reflect what the records
        // actually say, and the registry cross-check must fail.
        let idx = records
            .iter()
            .position(|r| {
                r.fault_mode.get("mode").and_then(Value::as_str) == Some("drop_first_write")
                    && r.environment_write_attempts.is_empty()
            })
            .unwrap();
        records[idx].fault_mode = json!({ "mode": "reliable" });
        let v = values(&records);
        let s = compute_summary(&records, &reg);
        // Derived from records: now 73 reliable / 71 drop.
        assert_eq!(s.fault_audit.registered_reliable_episodes, 73);
        assert_eq!(s.fault_audit.registered_drop_first_write_episodes, 71);
        // And the verifier catches the registry mismatch.
        let errors = verify_artifacts(&v, &serde_json::to_value(&s).unwrap(), &reg);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("fault_mode does not match registry"))
        );
    }
}
