//! Artifact types, design constants, summaries, verifiers, and
//! deterministic network-free synthetic datasets for Experiment 0010
//! (the first controlled self-evolution loop).
//!
//! Three artifact families, three disjoint task splits (spec §19):
//!
//! - **Discovery** (`DiscoveryRecord`): 18 incumbent (G0) episodes —
//!   3 tasks × 6 repetitions. The mutation generator may see this
//!   evidence (whitelist packet only).
//! - **Selection** (`SelectionRecord`): 150 episodes — 3 tasks ×
//!   10 repetitions × 5 conditions (G0, C1..C4), cyclic order. Used
//!   ONLY to choose one candidate (exploratory).
//! - **Promotion** (`PromotionRecord`): 96 episodes — 6 tasks ×
//!   8 repetitions × 2 conditions (G0 + the ONE frozen selected
//!   candidate). The single confirmatory comparison.
//!
//! The kernel is the authoritative execution recorder; every execution
//! field of a record originates from the Rust kernel's own in-episode
//! records. Design constants (episode counts, orders, gates) are
//! hardcoded here and are never inferred from artifacts.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;

use crate::mutation::{
    self, CandidatePool, MutationInput, build_mutation_input, mutation_input_leak_errors,
    validate_candidate_pool,
};
use crate::oracle::{self, TaskSpec, TaskSplit};
use crate::protocol::{MUTATOR_TEMPERATURE, Message, TEMPERATURE, sha256_hex};
use crate::selection::{self, CandidateTally, SelectionOutcome};
use crate::stats;
use crate::tools::{FaultMode, ToolExecutor, WriteAttempt};

// ===========================================================================
// Design constants (hardcoded, never inferred from artifacts)
// ===========================================================================

pub const EXPERIMENT_ID: &str = "0010";

/// The three registered structural families, in registry order.
///
/// Registered task IDs per split (spec §20/§31/§44).

pub const DISCOVERY_TASKS: [&str; 3] = ["EV1", "EV2", "EV3"];
pub const SELECTION_TASKS: [&str; 3] = ["SEL1", "SEL2", "SEL3"];
pub const PROMOTION_TASKS: [&str; 6] = ["PRO1", "PRO2", "PRO3", "PRO4", "PRO5", "PRO6"];

/// Discovery (spec §20/§21): 3 tasks × 6 repetitions, incumbent only.
pub const DISCOVERY_REPETITIONS: u32 = 6;
pub const DISCOVERY_EPISODES: usize = (3 * DISCOVERY_REPETITIONS) as usize; // 18
/// Minimum valid (non-infrastructure) discovery episodes (spec §22).
pub const MIN_VALID_DISCOVERY_EPISODES: u32 = 16;

/// Selection (spec §31–34): 3 tasks × 10 repetitions × 5 conditions.
pub const SELECTION_REPETITIONS: u32 = 10;
pub const SELECTION_CONDITIONS: [&str; 5] = ["G0", "C1", "C2", "C3", "C4"];
pub const SELECTION_EPISODES: usize =
    SELECTION_TASKS.len() * SELECTION_REPETITIONS as usize * SELECTION_CONDITIONS.len(); // 150
pub const SELECTION_CELLS: u32 = (SELECTION_TASKS.len() * SELECTION_REPETITIONS as usize) as u32; // 30
/// Minimum common-valid selection cells (spec §35).
pub const MIN_COMMON_VALID_CELLS: u32 = 27;
/// Minimum G0 failures among common-valid cells (spec §36).
pub const MIN_SELECTION_POTENTIAL_INFORMATION: u32 = 12;

/// Promotion (spec §44–46): 6 tasks × 8 repetitions × 2 conditions.
pub const PROMOTION_REPETITIONS: u32 = 8;
pub const PROMOTION_PAIRS: u32 = (PROMOTION_TASKS.len() * PROMOTION_REPETITIONS as usize) as u32; // 48
pub const PROMOTION_EPISODES: usize = (PROMOTION_PAIRS * 2) as usize; // 96
/// Gate constants (spec §48–50, carrying over the 0009 power budget).
pub const MIN_VALID_PROMOTION_PAIRS: u32 = 44;
pub const MIN_PROMOTION_POTENTIAL_INFORMATION: u32 = 12;
pub const MIN_PROMOTION_INFORMATIVE_PAIRS: u32 = 12;
/// Infrastructure-failure threshold (spec §52): strictly more than 10%
/// of all 96 promotion episodes → inconclusive.
pub const MAX_PROMOTION_INFRA_FAILURES: u32 = 9;

/// The frozen carried-over family-level stress profile (spec §11): the
/// Experiment 0009 supported result. `(family, stress id, drop count)`.
pub const FROZEN_PROFILE: [(&str, &str, u32); 3] = [
    ("direct_set", "S2", 2),
    ("conditional_set", "S1", 1),
    ("replacement", "S2", 2),
];

/// The registered (family → stress) mapping of the frozen profile.
pub fn frozen_stress(family: &str) -> Option<(&'static str, u32)> {
    FROZEN_PROFILE
        .iter()
        .find(|(f, _, _)| *f == family)
        .map(|(_, s, c)| (*s, *c))
}

/// Registered cyclic condition order for one (task, repetition) cell
/// (spec §34): rep 1 → G0 C1 C2 C3 C4; each subsequent rep rotates left
/// by one; reps 6–10 repeat reps 1–5. Every condition appears at every
/// position exactly twice per task.
pub fn selection_condition_at(repetition: u32, position: u32) -> &'static str {
    let r = (repetition - 1) % (SELECTION_CONDITIONS.len() as u32);
    SELECTION_CONDITIONS[((r + position - 1) as usize) % SELECTION_CONDITIONS.len()]
}

/// Registered alternating promotion condition order (spec §47): odd
/// repetitions run G0 → selected, even run selected → G0.
pub fn promotion_condition_order(repetition: u32, selected_id: &str) -> [&str; 2] {
    if repetition % 2 == 1 {
        ["G0", selected_id]
    } else {
        [selected_id, "G0"]
    }
}

// ===========================================================================
// Termination / failure classification (carried over from 0006–0009)
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
    // Infrastructure failures (excluded from pairs, recorded separately):
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
// Shared per-episode execution records (carried over from 0006–0009)
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

// ===========================================================================
// The three record shapes (one JSONL line each)
// ===========================================================================

/// Provenance block shared by all three record shapes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordProvenance {
    pub model: String,
    pub temperature: f64,
    pub redacted_endpoint: String,
    pub code_under_test_commit: String,
    pub incumbent_prompt_sha256: String,
    pub mutator_prompt_sha256: String,
    pub task_registry_sha256: String,
    pub stress_profile_sha256: String,
}

/// One persisted discovery trajectory (spec §20–23): an incumbent (G0)
/// episode on one of the three discovery tasks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryRecord {
    pub experiment_id: String,
    pub phase: String, // "discovery"
    pub run_id: String,
    /// 1-based position of this episode in the run (strict sequence).
    pub sequence: u32,
    pub family: String,
    pub task_id: String,
    pub split: String, // "discovery"
    pub stress_level: String,
    pub drop_count: u32,
    pub fault_key: String,
    pub repetition: u32,
    /// Always "G0": no candidate exists during discovery.
    pub condition: String,
    /// G0 is generation 0.
    pub generation: u32,
    #[serde(flatten)]
    pub provenance: RecordProvenance,
    pub initial_state: Value,
    pub target_state: Value,
    pub fault_mode: Value,
    /// Model-visible transcript (system, user, assistant, tool).
    pub conversation: Vec<Message>,
    pub model_requests: Vec<ModelRequestRecord>,
    pub requested_tool_calls: Vec<RequestedToolCall>,
    pub executed_tool_calls: Vec<ExecutedToolCall>,
    /// Hidden fault audit — NEVER enters the mutation input.
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

/// One persisted selection trajectory: one of the five conditions on
/// one selection (task, repetition) cell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectionRecord {
    pub experiment_id: String,
    pub phase: String, // "selection"
    pub run_id: String,
    pub sequence: u32,
    pub family: String,
    pub task_id: String,
    pub split: String, // "selection"
    pub stress_level: String,
    pub drop_count: u32,
    pub fault_key: String,
    pub repetition: u32,
    pub condition_position: u32,
    /// "G0" | "C1".."C4".
    pub condition: String,
    /// 0 for G0, 1 for C1..C4.
    pub generation: u32,
    /// Parent candidate ID ("G0" in all cases: G0's own line and the
    /// C-candidates' single parent).
    pub parent_id: String,
    /// `null` for G0.
    pub suffix_sha256: Option<String>,
    pub full_prompt_sha256: String,
    /// Byte hash of the frozen `candidate-pool.json`.
    pub candidate_pool_sha256: String,
    /// Byte hash of the frozen `mutation-input.json`.
    pub mutation_input_sha256: String,
    #[serde(flatten)]
    pub provenance: RecordProvenance,
    pub initial_state: Value,
    pub target_state: Value,
    pub fault_mode: Value,
    pub conversation: Vec<Message>,
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

/// One persisted promotion trajectory: exactly G0 or the one frozen
/// selected candidate, on one promotion (task, repetition) cell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionRecord {
    pub experiment_id: String,
    pub phase: String, // "promotion"
    pub run_id: String,
    pub sequence: u32,
    pub family: String,
    pub task_id: String,
    pub split: String, // "promotion"
    pub stress_level: String,
    pub drop_count: u32,
    pub fault_key: String,
    pub repetition: u32,
    pub condition_position: u32,
    /// "G0" | the frozen selected candidate ID (C1..C4).
    pub condition: String,
    pub generation: u32,
    pub parent_id: String,
    pub suffix_sha256: Option<String>,
    pub full_prompt_sha256: String,
    pub candidate_pool_sha256: String,
    /// Byte hash of the frozen `selected-candidate.json`.
    pub selected_candidate_sha256: String,
    pub mutation_input_sha256: String,
    #[serde(flatten)]
    pub provenance: RecordProvenance,
    pub initial_state: Value,
    pub target_state: Value,
    pub fault_mode: Value,
    pub conversation: Vec<Message>,
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
// (continued)

/// The user message of a discovery record is the task prompt.
impl DiscoveryRecord {
    pub fn task_prompt(&self) -> &str {
        self.conversation
            .get(1)
            .and_then(|m| m.content.as_deref())
            .unwrap_or("")
    }
}

// ===========================================================================
// Cost / cache reporting (cost is diagnostic ONLY; never a selection input)
// ===========================================================================

/// Aggregated cost for one population. Token fields are `null` (never
/// zero) when no request reported the field.
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
    pub usage_reporting_requests: u32,
    pub wall_time_ms: u64,
    pub model_wait_time_ms: u64,
    pub tool_execution_time_us: u64,
}

/// Cache accounting for a population. `cache_hit_ratio` is
/// `cached / nominal` when both are reported and nominal > 0;
/// `null` otherwise. Provider-reported ratios only.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CacheMetrics {
    pub nominal_prompt_tokens: Option<u64>,
    pub cached_prompt_tokens: Option<u64>,
    pub uncached_prompt_tokens: Option<u64>,
    pub cache_hit_ratio: Option<f64>,
}

/// One row of the condition × condition-position cache audit
/// (spec §64/§65).
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

/// Per-condition cost across registered populations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionSummary {
    pub condition: String,
    pub all: CostBlock,
    pub success: CostBlock,
    pub failure: CostBlock,
    pub cache: CacheMetrics,
}

fn add_opt(slot: &mut Option<u64>, v: Option<u64>) {
    if let (Some(slot), Some(v)) = (slot, v) {
        *slot = slot.saturating_add(v);
    }
}

/// The slice of a record that cost/cache aggregation reads.
pub trait EpisodeLike {
    fn oracle_success(&self) -> bool;
    fn agent_failure(&self) -> Option<&str>;
    fn infrastructure_failure(&self) -> Option<&str>;
    fn model_requests(&self) -> &[ModelRequestRecord];
    fn executed(&self) -> &[ExecutedToolCall];
    fn timing(&self) -> &Timing;
}

impl EpisodeLike for DiscoveryRecord {
    fn oracle_success(&self) -> bool {
        self.oracle_success
    }
    fn agent_failure(&self) -> Option<&str> {
        self.agent_failure.as_deref()
    }
    fn infrastructure_failure(&self) -> Option<&str> {
        self.infrastructure_failure.as_deref()
    }
    fn model_requests(&self) -> &[ModelRequestRecord] {
        &self.model_requests
    }
    fn executed(&self) -> &[ExecutedToolCall] {
        &self.executed_tool_calls
    }
    fn timing(&self) -> &Timing {
        &self.timing
    }
}

impl EpisodeLike for SelectionRecord {
    fn oracle_success(&self) -> bool {
        self.oracle_success
    }
    fn agent_failure(&self) -> Option<&str> {
        self.agent_failure.as_deref()
    }
    fn infrastructure_failure(&self) -> Option<&str> {
        self.infrastructure_failure.as_deref()
    }
    fn model_requests(&self) -> &[ModelRequestRecord] {
        &self.model_requests
    }
    fn executed(&self) -> &[ExecutedToolCall] {
        &self.executed_tool_calls
    }
    fn timing(&self) -> &Timing {
        &self.timing
    }
}

impl EpisodeLike for PromotionRecord {
    fn oracle_success(&self) -> bool {
        self.oracle_success
    }
    fn agent_failure(&self) -> Option<&str> {
        self.agent_failure.as_deref()
    }
    fn infrastructure_failure(&self) -> Option<&str> {
        self.infrastructure_failure.as_deref()
    }
    fn model_requests(&self) -> &[ModelRequestRecord] {
        &self.model_requests
    }
    fn executed(&self) -> &[ExecutedToolCall] {
        &self.executed_tool_calls
    }
    fn timing(&self) -> &Timing {
        &self.timing
    }
}

fn sum_usage<T: EpisodeLike>(pop: &[T]) -> CostBlock {
    let mut cb = CostBlock {
        episodes: 0,
        oracle_successes: 0,
        agent_failures: 0,
        infrastructure_failures: 0,
        model_requests: 0,
        executed_tool_calls: 0,
        nominal_prompt_tokens: None,
        cached_prompt_tokens: None,
        uncached_prompt_tokens: None,
        completion_tokens: None,
        reasoning_tokens: None,
        usage_reporting_requests: 0,
        wall_time_ms: 0,
        model_wait_time_ms: 0,
        tool_execution_time_us: 0,
    };
    for r in pop {
        cb.episodes += 1;
        if r.oracle_success() {
            cb.oracle_successes += 1;
        }
        if r.agent_failure().is_some() {
            cb.agent_failures += 1;
        }
        if r.infrastructure_failure().is_some() {
            cb.infrastructure_failures += 1;
        }
        for q in r.model_requests() {
            cb.model_requests += 1;
            if q.prompt_tokens.is_some() {
                cb.usage_reporting_requests += 1;
                add_opt(&mut cb.nominal_prompt_tokens, q.prompt_tokens);
                add_opt(&mut cb.cached_prompt_tokens, q.cached_prompt_tokens);
                add_opt(&mut cb.uncached_prompt_tokens, q.uncached_prompt_tokens);
            }
            add_opt(&mut cb.completion_tokens, q.completion_tokens);
            add_opt(&mut cb.reasoning_tokens, q.reasoning_tokens);
        }
        cb.executed_tool_calls += r.executed().len() as u32;
        cb.wall_time_ms += r.timing().wall_time_ms;
        cb.model_wait_time_ms += r.timing().model_wait_time_ms;
        cb.tool_execution_time_us += r.timing().tool_execution_time_us;
    }
    cb
}

fn cache_metrics(cb: &CostBlock) -> CacheMetrics {
    let ratio = match (cb.nominal_prompt_tokens, cb.cached_prompt_tokens) {
        (Some(n), Some(c)) if n > 0 => Some(c as f64 / n as f64),
        _ => None,
    };
    CacheMetrics {
        nominal_prompt_tokens: cb.nominal_prompt_tokens,
        cached_prompt_tokens: cb.cached_prompt_tokens,
        uncached_prompt_tokens: cb.uncached_prompt_tokens,
        cache_hit_ratio: ratio,
    }
}

fn cache_row(cb: &CostBlock, condition: &str, position: u32) -> ConditionPositionCacheRow {
    ConditionPositionCacheRow {
        condition: condition.to_string(),
        condition_position: position,
        episodes: cb.episodes,
        nominal_prompt_tokens: cb.nominal_prompt_tokens,
        cached_prompt_tokens: cb.cached_prompt_tokens,
        uncached_prompt_tokens: cb.uncached_prompt_tokens,
        cache_hit_ratio: cb
            .nominal_prompt_tokens
            .zip(cb.cached_prompt_tokens)
            .filter(|(n, _)| *n > 0)
            .map(|(n, c)| c as f64 / n as f64),
    }
}

// ===========================================================================
// Selected-candidate freeze artifact (spec §41)
// ===========================================================================

/// The frozen selection record for the selected candidate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FrozenSelectionBlock {
    pub common_valid_cells: u32,
    pub baseline_failures: u32,
    pub wins: u32,
    pub losses: u32,
    pub ties: u32,
    pub net_margin: i64,
}

/// The frozen selected candidate (spec §41). Contains NO promotion
/// outcomes. Once committed, the content must never change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectedCandidate {
    pub experiment_id: String,
    /// "G0" (incumbent retained) or the winning candidate ID.
    pub selected_candidate_id: String,
    pub parent_id: String,
    pub candidate_pool_sha256: String,
    pub selection_raw_sha256: String,
    pub selection_summary_sha256: String,
    pub mutation_input_sha256: String,
    /// `null` when G0 is retained.
    pub suffix: Option<String>,
    pub suffix_sha256: Option<String>,
    pub full_prompt_sha256: Option<String>,
    pub selection: FrozenSelectionBlock,
    pub tie_break_path: String,
}

impl SelectedCandidate {
    pub fn incumbent_retained(&self) -> bool {
        self.selected_candidate_id == "G0"
    }
}

// ===========================================================================
// Summary types
// ===========================================================================

/// Pre-registered discovery gates (spec §52).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryGates {
    pub artifact_complete: bool,
    pub mutation_input_whitelist: bool,
    pub valid_episodes_sufficient: bool,
}

impl DiscoveryGates {
    pub fn all(&self) -> bool {
        self.artifact_complete && self.mutation_input_whitelist && self.valid_episodes_sufficient
    }
}

/// One discovery task row (diagnostic).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryTaskRow {
    pub task_id: String,
    pub family: String,
    pub stress_level: String,
    pub episodes: u32,
    pub valid: u32,
    pub infrastructure_failures: u32,
    pub oracle_successes: u32,
}

/// The discovery summary (written by the `discover` runner).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoverySummary {
    pub experiment_id: String,
    pub phase: String,
    pub model: String,
    pub redacted_endpoint: String,
    pub code_under_test_commit: String,
    pub incumbent_prompt_sha256: String,
    pub task_registry_sha256: String,
    pub stress_profile_sha256: String,
    pub episodes_total: u32,
    pub valid_episodes: u32,
    pub infrastructure_failures: u32,
    pub agent_failures: u32,
    pub oracle_successes: u32,
    /// Byte hash of the frozen `mutation-input.json`.
    pub mutation_input_sha256: String,
    pub gates: DiscoveryGates,
    pub per_task: Vec<DiscoveryTaskRow>,
    pub cost: CostBlock,
    pub proceed_to_mutation_generation: bool,
}

/// Pre-registered selection gates (spec §52).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SelectionGates {
    pub candidate_pool_frozen: bool,
    pub artifact_complete: bool,
    pub cyclic_order_balanced: bool,
    pub common_valid_sufficient: bool,
    pub potential_information_sufficient: bool,
    pub selected_candidate_consistent: bool,
}

impl SelectionGates {
    pub fn all(&self) -> bool {
        self.candidate_pool_frozen
            && self.artifact_complete
            && self.cyclic_order_balanced
            && self.common_valid_sufficient
            && self.potential_information_sufficient
            && self.selected_candidate_consistent
    }
}

/// One (task, repetition) cell of the selection design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectionCellStatus {
    pub task_id: String,
    pub repetition: u32,
    pub common_valid: bool,
}

/// The selection summary (written by the `select` runner).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectionSummary {
    pub experiment_id: String,
    pub phase: String,
    pub model: String,
    pub redacted_endpoint: String,
    pub code_under_test_commit: String,
    pub incumbent_prompt_sha256: String,
    pub task_registry_sha256: String,
    pub stress_profile_sha256: String,
    pub candidate_pool_sha256: String,
    pub mutation_input_sha256: String,
    pub episodes_total: u32,
    pub infrastructure_failures: u32,
    pub cells: Vec<SelectionCellStatus>,
    pub common_valid_cells: u32,
    pub g0_successes_in_common_valid: u32,
    /// G0 failures among common-valid cells = potential information
    /// capacity (candidate-independent).
    pub g0_failures_in_common_valid: u32,
    pub candidates: Vec<CandidateTally>,
    /// Deterministic lexicographic ordering, best first.
    pub ranked_order: Vec<String>,
    pub selected: SelectionOutcome,
    pub gates: SelectionGates,
    pub condition_cost: Vec<ConditionSummary>,
    pub cache_audit: Vec<ConditionPositionCacheRow>,
    pub conclusion: Conclusion,
}

/// One promotion task diagnostic row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionTaskRow {
    pub task_id: String,
    pub family: String,
    pub stress_level: String,
    pub pairs: u32,
    pub valid: u32,
    pub g0_successes: u32,
    pub selected_successes: u32,
}

/// Pre-registered promotion gates (spec §52).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PromotionGates {
    pub selected_candidate_frozen: bool,
    pub artifact_complete: bool,
    pub only_registered_conditions: bool,
    pub infrastructure_within_threshold: bool,
    pub valid_pairs_sufficient: bool,
    pub potential_information_sufficient: bool,
    pub actual_informative_sufficient: bool,
}

impl PromotionGates {
    pub fn all(&self) -> bool {
        self.selected_candidate_frozen
            && self.artifact_complete
            && self.only_registered_conditions
            && self.infrastructure_within_threshold
            && self.valid_pairs_sufficient
            && self.potential_information_sufficient
            && self.actual_informative_sufficient
    }
}

/// The three-way experiment conclusion (plus the selection-stage
/// "continue_promotion" state).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conclusion {
    /// "supported" | "refuted" | "inconclusive" | "continue_promotion".
    pub result: String,
    pub reason: String,
}

/// The promotion summary (written by the `promote` runner).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionSummary {
    pub experiment_id: String,
    pub phase: String,
    pub model: String,
    pub redacted_endpoint: String,
    pub code_under_test_commit: String,
    pub incumbent_prompt_sha256: String,
    pub task_registry_sha256: String,
    pub stress_profile_sha256: String,
    pub candidate_pool_sha256: String,
    pub selected_candidate_sha256: String,
    pub mutation_input_sha256: String,
    pub selected_candidate_id: String,
    pub episodes_total: u32,
    pub infrastructure_failures: u32,
    pub expected_pairs: u32,
    pub valid_pairs: u32,
    pub excluded_infrastructure_pairs: u32,
    pub missing_pairs: u32,
    pub g0_successes: u32,
    pub selected_successes: u32,
    pub g0_failures_in_valid_pairs: u32,
    /// = g0_failures_in_valid_pairs (candidate-independent capacity).
    pub potential_information_capacity: u32,
    pub informative_pairs: u32,
    pub wins: u32,
    pub losses: u32,
    pub ties: u32,
    /// Exact two-sided sign test over (wins, losses) non-tied pairs.
    pub sign_test_p: f64,
    /// quality_improvement | quality_regression | quality_inconclusive.
    pub classification: String,
    pub gates: PromotionGates,
    pub per_task: Vec<PromotionTaskRow>,
    pub condition_cost: Vec<ConditionSummary>,
    pub cache_audit: Vec<ConditionPositionCacheRow>,
    pub conclusion: Conclusion,
}
// (continued)

// ===========================================================================
// Pure summary computation (shared by runner and verifier)
// ===========================================================================

/// Compute the discovery summary from its records.
#[allow(clippy::too_many_arguments)] // the frozen provenance hashes are part of the artifact contract
pub fn compute_discovery_summary(
    records: &[DiscoveryRecord],
    model: &str,
    endpoint: &str,
    commit: &str,
    incumbent_sha: &str,
    registry_sha: &str,
    profile_sha: &str,
    input_file_sha: &str,
) -> DiscoverySummary {
    let infra: u32 = records
        .iter()
        .filter(|r| r.infrastructure_failure.is_some())
        .count() as u32;
    let valid = records.len() as u32 - infra;
    let agent: u32 = records.iter().filter(|r| r.agent_failure.is_some()).count() as u32;
    let success: u32 = records.iter().filter(|r| r.oracle_success).count() as u32;
    let per_task = DISCOVERY_TASKS
        .iter()
        .map(|id| {
            let rows: Vec<&DiscoveryRecord> = records.iter().filter(|r| r.task_id == *id).collect();
            DiscoveryTaskRow {
                task_id: (*id).to_string(),
                family: rows.first().map(|r| r.family.clone()).unwrap_or_default(),
                stress_level: rows
                    .first()
                    .map(|r| r.stress_level.clone())
                    .unwrap_or_default(),
                episodes: rows.len() as u32,
                valid: rows
                    .iter()
                    .filter(|r| r.infrastructure_failure.is_none())
                    .count() as u32,
                infrastructure_failures: rows
                    .iter()
                    .filter(|r| r.infrastructure_failure.is_some())
                    .count() as u32,
                oracle_successes: rows.iter().filter(|r| r.oracle_success).count() as u32,
            }
        })
        .collect();
    let complete = records.len() == DISCOVERY_EPISODES;
    let valid_sufficient = valid >= MIN_VALID_DISCOVERY_EPISODES;
    let gates = DiscoveryGates {
        artifact_complete: complete,
        mutation_input_whitelist: true, // file-level check in the verifier
        valid_episodes_sufficient: valid_sufficient,
    };
    DiscoverySummary {
        experiment_id: EXPERIMENT_ID.to_string(),
        phase: "discovery".to_string(),
        model: model.to_string(),
        redacted_endpoint: endpoint.to_string(),
        code_under_test_commit: commit.to_string(),
        incumbent_prompt_sha256: incumbent_sha.to_string(),
        task_registry_sha256: registry_sha.to_string(),
        stress_profile_sha256: profile_sha.to_string(),
        episodes_total: records.len() as u32,
        valid_episodes: valid,
        infrastructure_failures: infra,
        agent_failures: agent,
        oracle_successes: success,
        mutation_input_sha256: input_file_sha.to_string(),
        gates,
        per_task,
        cost: sum_usage(records),
        proceed_to_mutation_generation: gates.all(),
    }
}

/// Compute the selection summary from its records + frozen pool.
#[allow(clippy::too_many_arguments)] // the frozen provenance hashes are part of the artifact contract
pub fn compute_selection_summary(
    records: &[SelectionRecord],
    pool: &CandidatePool,
    model: &str,
    endpoint: &str,
    commit: &str,
    incumbent_sha: &str,
    registry_sha: &str,
    profile_sha: &str,
    pool_file_sha: &str,
    input_file_sha: &str,
) -> SelectionSummary {
    let mut infra = 0u32;
    let mut order_ok = true;
    let mut position_counts: BTreeMap<(String, u32), u32> = BTreeMap::new();
    for r in records {
        *position_counts
            .entry((r.condition.clone(), r.condition_position))
            .or_insert(0) += 1;
        if !SELECTION_CONDITIONS.contains(&r.condition.as_str())
            || selection_condition_at(r.repetition, r.condition_position) != r.condition
        {
            order_ok = false;
        }
        if r.infrastructure_failure.is_some() {
            infra += 1;
        }
    }
    let balanced = position_counts.len() == 25 && position_counts.values().all(|&n| n == 6);
    let count_ok = records.len() == SELECTION_EPISODES;

    // Cell bookkeeping: a cell is common-valid iff NONE of the five
    // conditions has an infrastructure failure in it (spec §35).
    let mut cells: BTreeMap<(String, u32), (u8, u8)> = BTreeMap::new(); // (present, infra)
    for r in records {
        let e = cells
            .entry((r.task_id.clone(), r.repetition))
            .or_insert((0, 0));
        e.0 += 1;
        if r.infrastructure_failure.is_some() {
            e.1 += 1;
        }
    }
    let mut cell_statuses: Vec<SelectionCellStatus> = Vec::new();
    for (ti, task_id) in SELECTION_TASKS.iter().enumerate() {
        for rep in 1..=SELECTION_REPETITIONS {
            let (present, failed) = cells
                .get(&(task_id.to_string(), rep))
                .copied()
                .unwrap_or((0, 0));
            cell_statuses.push(SelectionCellStatus {
                task_id: task_id.to_string(),
                repetition: rep,
                common_valid: present == 5 && failed == 0,
            });
            let _ = ti;
        }
    }
    let common_valid = cell_statuses.iter().filter(|c| c.common_valid).count() as u32;

    let cv_set: BTreeMap<(String, u32), bool> = cell_statuses
        .iter()
        .map(|c| ((c.task_id.clone(), c.repetition), c.common_valid))
        .collect();

    // G0 baseline on common-valid cells (candidate-independent).
    let g0_records: Vec<&SelectionRecord> = records
        .iter()
        .filter(|r| {
            r.condition == "G0"
                && cv_set.get(&(r.task_id.clone(), r.repetition)) == Some(&true)
                && r.infrastructure_failure.is_none()
        })
        .collect();
    let g0_success: u32 = g0_records.iter().filter(|r| r.oracle_success).count() as u32;
    let g0_failure: u32 = g0_records.len() as u32 - g0_success;

    // Candidate tallies on the same common-valid cells.
    let g0_for = |task_id: &str, rep: u32| -> Option<&SelectionRecord> {
        records
            .iter()
            .find(|r| r.condition == "G0" && r.task_id == task_id && r.repetition == rep)
    };
    let mut tallies = Vec::new();
    for (i, c) in pool.candidates.iter().enumerate() {
        let mut wins = 0u64;
        let mut losses = 0u64;
        let mut ties = 0u64;
        for cell in &cell_statuses {
            if !cell.common_valid {
                continue;
            }
            let Some(rec) = records.iter().find(|r| {
                r.condition.as_str() == c.candidate_id.as_str()
                    && r.task_id == *cell.task_id
                    && r.repetition == cell.repetition
            }) else {
                continue;
            };
            let Some(g0) = g0_for(&cell.task_id, cell.repetition) else {
                continue;
            };
            if rec.oracle_success && !g0.oracle_success {
                wins += 1;
            } else if !rec.oracle_success && g0.oracle_success {
                losses += 1;
            } else {
                ties += 1;
            }
        }
        let net = wins as i64 - losses as i64;
        tallies.push(CandidateTally {
            candidate_id: c.candidate_id.clone(),
            ordinal: (i + 1) as u32,
            wins,
            losses,
            ties,
            net_margin: net,
            diagnostic_p_exploratory: stats::exact_sign_test_p(wins, losses),
        });
    }

    let outcome = selection::select(&tallies);
    let ranked = selection::rank_candidates(&tallies)
        .iter()
        .map(|t| t.candidate_id.clone())
        .collect();

    let common_valid_sufficient = common_valid >= MIN_COMMON_VALID_CELLS;
    let info_sufficient = g0_failure >= MIN_SELECTION_POTENTIAL_INFORMATION;
    let gates = SelectionGates {
        candidate_pool_frozen: pool.mutation_input_sha256 == input_file_sha,
        artifact_complete: count_ok,
        cyclic_order_balanced: order_ok && balanced,
        common_valid_sufficient,
        potential_information_sufficient: info_sufficient,
        selected_candidate_consistent: true, // file-level check in the verifier
    };

    let conclusion = if !gates.all() {
        Conclusion {
            result: "inconclusive".to_string(),
            reason: "selection integrity/information gate failure — no promotion".to_string(),
        }
    } else if outcome.incumbent_retained {
        Conclusion {
            result: "refuted".to_string(),
            reason: "no generated candidate had net margin > 0; the incumbent G0 was retained; the mutation generator did not produce a candidate that displaced the incumbent"
                .to_string(),
        }
    } else {
        Conclusion {
            result: "continue_promotion".to_string(),
            reason: format!(
                "candidate {} displaced G0 by the frozen lexicographic rule; promotion authorized",
                outcome.selected_candidate_id
            ),
        }
    };

    let condition_cost = SELECTION_CONDITIONS
        .iter()
        .map(|id| {
            let pop: Vec<SelectionRecord> = records
                .iter()
                .filter(|r| r.condition == *id)
                .cloned()
                .collect();
            let all = sum_usage(&pop);
            let success: Vec<SelectionRecord> =
                pop.iter().filter(|r| r.oracle_success).cloned().collect();
            let failure: Vec<SelectionRecord> =
                pop.iter().filter(|r| !r.oracle_success).cloned().collect();
            ConditionSummary {
                condition: (*id).to_string(),
                all,
                success: sum_usage(&success),
                failure: sum_usage(&failure),
                cache: cache_metrics(&all),
            }
        })
        .collect();
    let mut cache_audit = Vec::new();
    for id in SELECTION_CONDITIONS {
        for pos in 1..SELECTION_CONDITIONS.len() as u32 {
            let pop: Vec<SelectionRecord> = records
                .iter()
                .filter(|r| r.condition == id && r.condition_position == pos)
                .cloned()
                .collect();
            cache_audit.push(cache_row(&sum_usage(&pop), id, pos));
        }
    }

    SelectionSummary {
        experiment_id: EXPERIMENT_ID.to_string(),
        phase: "selection".to_string(),
        model: model.to_string(),
        redacted_endpoint: endpoint.to_string(),
        code_under_test_commit: commit.to_string(),
        incumbent_prompt_sha256: incumbent_sha.to_string(),
        task_registry_sha256: registry_sha.to_string(),
        stress_profile_sha256: profile_sha.to_string(),
        candidate_pool_sha256: pool_file_sha.to_string(),
        mutation_input_sha256: input_file_sha.to_string(),
        episodes_total: records.len() as u32,
        infrastructure_failures: infra,
        cells: cell_statuses,
        common_valid_cells: common_valid,
        g0_successes_in_common_valid: g0_success,
        g0_failures_in_common_valid: g0_failure,
        candidates: tallies,
        ranked_order: ranked,
        selected: outcome,
        gates,
        condition_cost,
        cache_audit,
        conclusion,
    }
}

/// Compute the promotion summary (the single confirmatory comparison).
#[allow(clippy::too_many_arguments)] // the frozen provenance hashes are part of the artifact contract
pub fn compute_promotion_summary(
    records: &[PromotionRecord],
    selected_id: &str,
    model: &str,
    endpoint: &str,
    commit: &str,
    incumbent_sha: &str,
    registry_sha: &str,
    profile_sha: &str,
    pool_file_sha: &str,
    selected_file_sha: &str,
    input_file_sha: &str,
) -> PromotionSummary {
    let conditions: Vec<String> = vec!["G0".to_string(), selected_id.to_string()];
    let known_conditions = records
        .iter()
        .all(|r| conditions.contains(&r.condition.clone()));
    let infra: u32 = records
        .iter()
        .filter(|r| r.infrastructure_failure.is_some())
        .count() as u32;

    let mut missing = 0u32;
    let mut valid = 0u32;
    let mut excluded_infra = 0u32;
    let mut wins = 0u32;
    let mut losses = 0u32;
    let mut ties = 0u32;
    let mut g0_success = 0u32;
    let mut sel_success = 0u32;
    let mut g0_fail_valid = 0u32;
    for t in PROMOTION_TASKS {
        for rep in 1..=PROMOTION_REPETITIONS {
            let g0 = records
                .iter()
                .find(|r| r.task_id == t && r.repetition == rep && r.condition == "G0");
            let sel = records
                .iter()
                .find(|r| r.task_id == t && r.repetition == rep && r.condition == selected_id);
            let (Some(g0), Some(sel)) = (g0, sel) else {
                missing += 1;
                continue;
            };
            if g0.infrastructure_failure.is_some() || sel.infrastructure_failure.is_some() {
                excluded_infra += 1;
                continue;
            }
            valid += 1;
            if g0.oracle_success {
                g0_success += 1;
            } else {
                g0_fail_valid += 1;
            }
            if sel.oracle_success {
                sel_success += 1;
            }
            // Selected-candidate perspective (spec §51).
            if sel.oracle_success && !g0.oracle_success {
                wins += 1;
            } else if !sel.oracle_success && g0.oracle_success {
                losses += 1;
            } else {
                ties += 1;
            }
        }
    }
    let informative = wins + losses;
    let p = stats::exact_sign_test_p(wins as u64, losses as u64);
    let class = stats::classify_quality(wins as u64, losses as u64);
    let potential = g0_fail_valid;

    let complete = records.len() == PROMOTION_EPISODES;
    let order_ok = {
        let mut seen = BTreeMap::new();
        let mut ok = true;
        for r in records {
            *seen
                .entry((
                    r.task_id.clone(),
                    r.repetition,
                    r.condition.clone(),
                    r.condition_position,
                ))
                .or_insert(0) += 1;
            let want = promotion_condition_order(r.repetition, selected_id)
                .get((r.condition_position - 1) as usize)
                .copied();
            if want != Some(r.condition.as_str()) {
                ok = false;
            }
        }
        seen.values().all(|&n| n == 1) && ok
    };
    let gates = PromotionGates {
        selected_candidate_frozen: true, // file-level check in the verifier
        artifact_complete: complete && order_ok,
        only_registered_conditions: known_conditions,
        infrastructure_within_threshold: infra <= MAX_PROMOTION_INFRA_FAILURES,
        valid_pairs_sufficient: valid >= MIN_VALID_PROMOTION_PAIRS,
        potential_information_sufficient: potential >= MIN_PROMOTION_POTENTIAL_INFORMATION,
        actual_informative_sufficient: informative >= MIN_PROMOTION_INFORMATIVE_PAIRS,
    };

    let conclusion = if !gates.all() {
        Conclusion {
            result: "inconclusive".to_string(),
            reason: "a promotion integrity/measurement gate failed — the confirmatory test is not supported".to_string(),
        }
    } else if selected_id == "G0" {
        Conclusion {
            result: "refuted".to_string(),
            reason:
                "the incumbent was retained at selection; no non-incumbent candidate to promote"
                    .to_string(),
        }
    } else if class == stats::QualityClass::Improvement {
        Conclusion {
            result: "supported".to_string(),
            reason: format!(
                "selected candidate {selected_id} achieved quality_improvement over G0 on the untouched promotion set (p={p})"
            ),
        }
    } else {
        Conclusion {
            result: "refuted".to_string(),
            reason: format!(
                "a mutation was selected and promotion had sufficient information, but the classification is {}",
                class.as_str()
            ),
        }
    };

    let per_task = PROMOTION_TASKS
        .iter()
        .map(|id| {
            let pop: Vec<&PromotionRecord> = records.iter().filter(|r| r.task_id == *id).collect();
            let g0p: Vec<&&PromotionRecord> = pop.iter().filter(|r| r.condition == "G0").collect();
            let selp: Vec<&&PromotionRecord> = pop
                .iter()
                .filter(|r| r.condition == selected_id)
                .collect();
            let mut pairs = 0u32;
            let mut valid_t = 0u32;
            for rep in 1..=PROMOTION_REPETITIONS {
                let g = g0p.iter().any(|r| r.repetition == rep);
                let s = selp.iter().any(|r| r.repetition == rep);
                if g && s {
                    pairs += 1;
                    if !matches!(g0p.iter().find(|r| r.repetition == rep), Some(r) if r.infrastructure_failure.is_some())
                        && !matches!(selp.iter().find(|r| r.repetition == rep), Some(r) if r.infrastructure_failure.is_some())
                    {
                        valid_t += 1;
                    }
                }
            }
            PromotionTaskRow {
                task_id: (*id).to_string(),
                family: pop.first().map(|r| r.family.clone()).unwrap_or_default(),
                stress_level: pop.first().map(|r| r.stress_level.clone()).unwrap_or_default(),
                pairs,
                valid: valid_t,
                g0_successes: g0p
                    .iter()
                    .filter(|r| r.oracle_success)
                    .count() as u32,
                selected_successes: selp
                    .iter()
                    .filter(|r| r.oracle_success)
                    .count() as u32,
            }
        })
        .collect();

    let condition_cost = conditions
        .iter()
        .map(|id| {
            let pop: Vec<PromotionRecord> = records
                .iter()
                .filter(|r| &r.condition == id)
                .cloned()
                .collect();
            let all = sum_usage(&pop);
            let success: Vec<PromotionRecord> =
                pop.iter().filter(|r| r.oracle_success).cloned().collect();
            let failure: Vec<PromotionRecord> =
                pop.iter().filter(|r| !r.oracle_success).cloned().collect();
            ConditionSummary {
                condition: id.clone(),
                all,
                success: sum_usage(&success),
                failure: sum_usage(&failure),
                cache: cache_metrics(&all),
            }
        })
        .collect();
    let mut cache_audit = Vec::new();
    for id in &conditions {
        for pos in 1..2u32 {
            let pop: Vec<PromotionRecord> = records
                .iter()
                .filter(|r| &r.condition == id && r.condition_position == pos)
                .cloned()
                .collect();
            cache_audit.push(cache_row(&sum_usage(&pop), id, pos));
        }
    }

    PromotionSummary {
        experiment_id: EXPERIMENT_ID.to_string(),
        phase: "promotion".to_string(),
        model: model.to_string(),
        redacted_endpoint: endpoint.to_string(),
        code_under_test_commit: commit.to_string(),
        incumbent_prompt_sha256: incumbent_sha.to_string(),
        task_registry_sha256: registry_sha.to_string(),
        stress_profile_sha256: profile_sha.to_string(),
        candidate_pool_sha256: pool_file_sha.to_string(),
        selected_candidate_sha256: selected_file_sha.to_string(),
        mutation_input_sha256: input_file_sha.to_string(),
        selected_candidate_id: selected_id.to_string(),
        episodes_total: records.len() as u32,
        infrastructure_failures: infra,
        expected_pairs: PROMOTION_PAIRS,
        valid_pairs: valid,
        excluded_infrastructure_pairs: excluded_infra,
        missing_pairs: missing,
        g0_successes: g0_success,
        selected_successes: sel_success,
        g0_failures_in_valid_pairs: g0_fail_valid,
        potential_information_capacity: potential,
        informative_pairs: informative,
        wins,
        losses,
        ties,
        sign_test_p: p,
        classification: class.as_str().to_string(),
        gates,
        per_task,
        condition_cost,
        cache_audit,
        conclusion,
    }
}
// (continued)

// ===========================================================================
// Provenance / configuration helpers
// ===========================================================================

/// Canonical (key-order independent) SHA-256 of the loaded task registry.
pub fn task_registry_sha256(registry: &[TaskSpec]) -> Result<String, String> {
    let v: Value = serde_json::to_value(json!({ "tasks": registry })).map_err(|e| e.to_string())?;
    Ok(sha256_hex(&v))
}

/// Canonical SHA-256 of the frozen stress profile (parsed JSON).
pub fn stress_profile_sha256(profile: &Value) -> String {
    sha256_hex(&crate::protocol::canonical_json(profile))
}

/// Load + validate the frozen stress profile (spec §11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FamilyStress {
    pub stress: String,
    pub drop_first_n_writes_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StressProfile {
    pub profile: String,
    pub note: String,
    pub families: BTreeMap<String, FamilyStress>,
}

pub fn load_stress_profile(path: &std::path::Path) -> Result<StressProfile, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read stress profile {}: {e}", path.display()))?;
    let p: StressProfile =
        serde_json::from_str(&raw).map_err(|e| format!("stress profile is not valid: {e}"))?;
    if p.families.len() != 3 {
        return Err(format!(
            "stress profile must contain 3 families, found {}",
            p.families.len()
        ));
    }
    for (family, stress, count) in FROZEN_PROFILE {
        let Some(fs) = p.families.get(family) else {
            return Err(format!("stress profile is missing family {family}"));
        };
        if fs.stress != stress || fs.drop_first_n_writes_count != count {
            return Err(format!(
                "family {family} profile ({}, {}) does not match the frozen Experiment 0009 stresses ({}, {count})",
                fs.stress, fs.drop_first_n_writes_count, stress
            ));
        }
    }
    Ok(p)
}

// ===========================================================================
// Artifact verifiers (typed; used by `verify-*` and `self-test`)
// ===========================================================================

/// Frozen configuration context shared by all verifiers.
#[derive(Clone)]
pub struct VerifierContext {
    pub incumbent_text: String,
    pub incumbent_file_sha: String,
    pub mutator_file_sha: String,
    pub registry: Vec<TaskSpec>,
    pub task_registry_sha: String,
    pub profile: Value,
    pub profile_sha: String,
}

impl VerifierContext {
    pub fn new(
        incumbent_text: String,
        incumbent_file_sha: String,
        mutator_file_sha: String,
        registry: Vec<TaskSpec>,
        profile: Value,
    ) -> Self {
        let registry_sha = task_registry_sha256(&registry).unwrap_or_default();
        let profile_sha = stress_profile_sha256(&profile);
        Self {
            incumbent_text,
            incumbent_file_sha,
            mutator_file_sha,
            registry,
            task_registry_sha: registry_sha,
            profile,
            profile_sha,
        }
    }

    pub fn task(&self, id: &str) -> Option<&TaskSpec> {
        self.registry.iter().find(|t| t.id == id)
    }
}

fn registry_ok(ctx: &VerifierContext, errors: &mut Vec<String>) {
    // The stress profile file must still be the frozen Experiment 0009
    // profile (the constants alone would let a swapped file pass).
    let families = ctx.profile.get("families").and_then(Value::as_object);
    for (family, stress, count) in FROZEN_PROFILE {
        let Some(fs) = families.and_then(|f| f.get(family)) else {
            errors.push(format!("stress profile is missing family {family}"));
            continue;
        };
        if fs.get("stress").and_then(Value::as_str) != Some(stress)
            || fs.get("drop_first_n_writes_count").and_then(Value::as_u64) != Some(u64::from(count))
        {
            errors.push(format!(
                "stress profile disagrees with the frozen Experiment 0009 profile for {family}"
            ));
        }
    }
    if ctx.registry.len() != 12 {
        errors.push(format!(
            "registry must contain 12 tasks, found {}",
            ctx.registry.len()
        ));
        return;
    }
    let expected = [
        "EV1", "EV2", "EV3", "SEL1", "SEL2", "SEL3", "PRO1", "PRO2", "PRO3", "PRO4", "PRO5", "PRO6",
    ];
    for (i, (t, id)) in ctx.registry.iter().zip(expected.iter()).enumerate() {
        if t.id != *id {
            errors.push(format!("registry task[{i}] is {}, expected {id}", t.id));
        }
    }
    for t in &ctx.registry {
        let want_split = match t.id.chars().next().unwrap_or(' ') {
            'E' => TaskSplit::Discovery,
            'S' => TaskSplit::Selection,
            'P' => TaskSplit::Promotion,
            _ => TaskSplit::Discovery,
        };
        if t.split != want_split {
            errors.push(format!(
                "task {} split {:?} inconsistent with its ID",
                t.id, t.split
            ));
        }
        let want_family = match t.id.as_str() {
            "EV1" | "SEL1" | "PRO1" | "PRO2" => "direct_set",
            "EV2" | "SEL2" | "PRO3" | "PRO4" => "conditional_set",
            _ => "replacement",
        };
        if t.family != want_family {
            errors.push(format!(
                "task {} family {:?} != {want_family:?}",
                t.id, t.family
            ));
        }
    }
}

/// The slice of a record that `verify_record_core` reads, so the three
/// typed records share one core verifier.
pub trait RecordCoreLike {
    fn termination_reason(&self) -> &str;
    fn agent_failure(&self) -> &Option<String>;
    fn infrastructure_failure(&self) -> &Option<String>;
    fn usage_accounting_error(&self) -> &Option<String>;
    fn model_turn_count(&self) -> u32;
    fn tool_call_count(&self) -> u32;
    fn model_requests(&self) -> &[ModelRequestRecord];
    fn write_attempts(&self) -> &[WriteAttempt];
    fn executed(&self) -> &[ExecutedToolCall];
    fn final_state(&self) -> &Value;
    fn final_answer(&self) -> &Option<String>;
    fn conversation(&self) -> &[Message];
    fn fault_mode(&self) -> &Value;
    fn initial_state(&self) -> &Value;
    fn target_state(&self) -> &Value;
    fn oracle_success(&self) -> bool;
    fn oracle_failure_reasons(&self) -> &[String];
}

impl RecordCoreLike for DiscoveryRecord {
    fn termination_reason(&self) -> &str {
        &self.termination_reason
    }
    fn agent_failure(&self) -> &Option<String> {
        &self.agent_failure
    }
    fn infrastructure_failure(&self) -> &Option<String> {
        &self.infrastructure_failure
    }
    fn usage_accounting_error(&self) -> &Option<String> {
        &self.usage_accounting_error
    }
    fn model_turn_count(&self) -> u32 {
        self.model_turn_count
    }
    fn tool_call_count(&self) -> u32 {
        self.tool_call_count
    }
    fn model_requests(&self) -> &[ModelRequestRecord] {
        &self.model_requests
    }
    fn write_attempts(&self) -> &[WriteAttempt] {
        &self.environment_write_attempts
    }
    fn executed(&self) -> &[ExecutedToolCall] {
        &self.executed_tool_calls
    }
    fn final_state(&self) -> &Value {
        &self.final_state
    }
    fn final_answer(&self) -> &Option<String> {
        &self.final_answer
    }
    fn conversation(&self) -> &[Message] {
        &self.conversation
    }
    fn fault_mode(&self) -> &Value {
        &self.fault_mode
    }
    fn initial_state(&self) -> &Value {
        &self.initial_state
    }
    fn target_state(&self) -> &Value {
        &self.target_state
    }
    fn oracle_success(&self) -> bool {
        self.oracle_success
    }
    fn oracle_failure_reasons(&self) -> &[String] {
        &self.oracle_failure_reasons
    }
}

impl RecordCoreLike for SelectionRecord {
    fn termination_reason(&self) -> &str {
        &self.termination_reason
    }
    fn agent_failure(&self) -> &Option<String> {
        &self.agent_failure
    }
    fn infrastructure_failure(&self) -> &Option<String> {
        &self.infrastructure_failure
    }
    fn usage_accounting_error(&self) -> &Option<String> {
        &self.usage_accounting_error
    }
    fn model_turn_count(&self) -> u32 {
        self.model_turn_count
    }
    fn tool_call_count(&self) -> u32 {
        self.tool_call_count
    }
    fn model_requests(&self) -> &[ModelRequestRecord] {
        &self.model_requests
    }
    fn write_attempts(&self) -> &[WriteAttempt] {
        &self.environment_write_attempts
    }
    fn executed(&self) -> &[ExecutedToolCall] {
        &self.executed_tool_calls
    }
    fn final_state(&self) -> &Value {
        &self.final_state
    }
    fn final_answer(&self) -> &Option<String> {
        &self.final_answer
    }
    fn conversation(&self) -> &[Message] {
        &self.conversation
    }
    fn fault_mode(&self) -> &Value {
        &self.fault_mode
    }
    fn initial_state(&self) -> &Value {
        &self.initial_state
    }
    fn target_state(&self) -> &Value {
        &self.target_state
    }
    fn oracle_success(&self) -> bool {
        self.oracle_success
    }
    fn oracle_failure_reasons(&self) -> &[String] {
        &self.oracle_failure_reasons
    }
}

impl RecordCoreLike for PromotionRecord {
    fn termination_reason(&self) -> &str {
        &self.termination_reason
    }
    fn agent_failure(&self) -> &Option<String> {
        &self.agent_failure
    }
    fn infrastructure_failure(&self) -> &Option<String> {
        &self.infrastructure_failure
    }
    fn usage_accounting_error(&self) -> &Option<String> {
        &self.usage_accounting_error
    }
    fn model_turn_count(&self) -> u32 {
        self.model_turn_count
    }
    fn tool_call_count(&self) -> u32 {
        self.tool_call_count
    }
    fn model_requests(&self) -> &[ModelRequestRecord] {
        &self.model_requests
    }
    fn write_attempts(&self) -> &[WriteAttempt] {
        &self.environment_write_attempts
    }
    fn executed(&self) -> &[ExecutedToolCall] {
        &self.executed_tool_calls
    }
    fn final_state(&self) -> &Value {
        &self.final_state
    }
    fn final_answer(&self) -> &Option<String> {
        &self.final_answer
    }
    fn conversation(&self) -> &[Message] {
        &self.conversation
    }
    fn fault_mode(&self) -> &Value {
        &self.fault_mode
    }
    fn initial_state(&self) -> &Value {
        &self.initial_state
    }
    fn target_state(&self) -> &Value {
        &self.target_state
    }
    fn oracle_success(&self) -> bool {
        self.oracle_success
    }
    fn oracle_failure_reasons(&self) -> &[String] {
        &self.oracle_failure_reasons
    }
}

/// Verify kernel / fault / oracle / usage invariants for ONE record
/// against its registered task + frozen stress. Shared by the three
/// phase verifiers.
fn verify_record_core<R: RecordCoreLike>(
    errors: &mut Vec<String>,
    who: &str,
    task: &TaskSpec,
    drop_count: u32,
    expected_full_prompt: &str,
    record: &R,
) {
    let tr = TerminationReason::parse(record.termination_reason());
    let Some(tr) = tr else {
        errors.push(format!(
            "{who}: unregistered termination reason {:?}",
            record.termination_reason()
        ));
        return;
    };
    if tr.is_agent_failure() && record.agent_failure().as_deref() != Some(tr.as_str()) {
        errors.push(format!(
            "{who}: agent_failure inconsistent with termination {tr:?}"
        ));
    }
    if !tr.is_agent_failure() && record.agent_failure().is_some() {
        errors.push(format!(
            "{who}: agent_failure set without an agent-failure termination"
        ));
    }
    if tr.is_infrastructure() && record.infrastructure_failure().as_deref() != Some(tr.as_str()) {
        errors.push(format!(
            "{who}: infrastructure_failure inconsistent with termination {tr:?}"
        ));
    }
    if !tr.is_infrastructure() && record.infrastructure_failure().is_some() {
        errors.push(format!(
            "{who}: infrastructure_failure set without an infrastructure termination"
        ));
    }
    let expected_len = record.model_turn_count() + if tr.is_infrastructure() { 1 } else { 0 };
    if (record.model_requests().len() as u32) != expected_len {
        errors.push(format!(
            "{who}: {} model requests, expected {expected_len}",
            record.model_requests().len()
        ));
    }
    for (i, q) in record.model_requests().iter().enumerate() {
        if q.turn != i as u32 + 1 {
            errors.push(format!("{who}: request turn numbering broken at {i}"));
        }
        let expected_uncached = match (q.prompt_tokens, q.cached_prompt_tokens) {
            (Some(p), Some(c)) if c <= p => Some(p - c),
            _ => None,
        };
        if q.uncached_prompt_tokens != expected_uncached
            && record.usage_accounting_error().is_none()
        {
            errors.push(format!("{who}: request {i} usage accounting inconsistent"));
        }
    }
    if record.usage_accounting_error().is_some() {
        errors.push(format!(
            "{who}: usage accounting error recorded: {:?}",
            record.usage_accounting_error()
        ));
    }
    if record.model_turn_count() > 12 {
        errors.push(format!(
            "{who}: model_turn_count {} exceeds the 12-turn kernel limit",
            record.model_turn_count()
        ));
    }
    if record.tool_call_count() > 16 {
        errors.push(format!(
            "{who}: tool_call_count {} exceeds the 16-call kernel limit",
            record.tool_call_count()
        ));
    }
    if record.tool_call_count() as usize != record.executed().len() {
        errors.push(format!(
            "{who}: tool_call_count {} != {} executed calls",
            record.tool_call_count(),
            record.executed().len()
        ));
    }
    // Fault audit + replay (spec §12): the hidden audit must be exactly
    // what the registered fault schedule dictates, and the replay of
    // applied writes must reproduce the final state.
    let mut state = task.initial_state.clone();
    let mut target_ordinal = 0u32;
    for (i, w) in record.write_attempts().iter().enumerate() {
        if w.sequence != i as u32 {
            errors.push(format!("{who}: write attempt sequence broken at {i}"));
        }
        let is_target = w.key == task.fault_key;
        if is_target {
            target_ordinal += 1;
            if w.registered_drop_index != Some(target_ordinal) {
                errors.push(format!(
                    "{who}: write {i} registered_drop_index {:?} != ordinal {target_ordinal}",
                    w.registered_drop_index
                ));
            }
        } else if w.registered_drop_index.is_some() {
            errors.push(format!(
                "{who}: non-target write {i} carries a drop ordinal"
            ));
        }
        let expected_applied = if is_target {
            target_ordinal > drop_count
        } else {
            true
        };
        if w.applied != expected_applied {
            errors.push(format!(
                "{who}: write {i} applied={} but the fault schedule requires {expected_applied}",
                w.applied
            ));
        }
        if w.fault_reason != (!w.applied).then(|| "drop_first_n_writes".to_string()) {
            errors.push(format!(
                "{who}: write {i} fault_reason inconsistent with applied={}",
                w.applied
            ));
        }
        if w.applied {
            if let Some(obj) = state.as_object_mut() {
                obj.insert(w.key.clone(), Value::String(w.requested_value.clone()));
            }
        }
    }
    if state != *record.final_state() {
        errors.push(format!(
            "{who}: final state {:?} != write-replay {:?} (audit and state disagree)",
            record.final_state(),
            state
        ));
    }
    // Executed-call results must be exactly the model-visible contract.
    let mut replay = task.initial_state.clone();
    for c in record.executed() {
        match c.tool_name.as_str() {
            "state_write" => {
                let expected = json!({
                    "ok": true,
                    "key": c.arguments.get("key"),
                    "value": c.arguments.get("value"),
                });
                if c.result != expected {
                    errors.push(format!(
                        "{who}: state_write result for call {} diverges from the model-visible contract",
                        c.sequence
                    ));
                }
            }
            "state_read" => {
                let k = c.arguments.get("key").and_then(Value::as_str).unwrap_or("");
                let expected = json!({ "key": k, "value": replay.get(k) });
                if c.result != expected {
                    errors.push(format!(
                        "{who}: state_read result for call {} diverges from replay state",
                        c.sequence
                    ));
                }
            }
            other => {
                errors.push(format!("{who}: unknown executed tool {other:?}"));
            }
        }
        // Replay the applied writes (read state from the audit).
        if c.tool_name == "state_write" {
            let k = c.arguments.get("key").and_then(Value::as_str).unwrap_or("");
            let v = c
                .arguments
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or("");
            let applied = record
                .write_attempts()
                .iter()
                .any(|w| w.key == k && w.requested_value == v && w.applied);
            if applied {
                if let Some(obj) = replay.as_object_mut() {
                    obj.insert(k.to_string(), Value::String(v.to_string()));
                }
            }
        }
    }
    // Conversation invariants.
    if let (Some(sys), Some(user)) = (record.conversation().first(), record.conversation().get(1)) {
        if sys.role != crate::protocol::Role::System
            || sys.content.as_deref() != Some(expected_full_prompt)
        {
            errors.push(format!(
                "{who}: conversation[0] is not the expected full system prompt"
            ));
        }
        if user.role != crate::protocol::Role::User || user.content.as_deref() != Some(&task.prompt)
        {
            errors.push(format!(
                "{who}: conversation[1] is not the registered task prompt"
            ));
        }
    } else {
        errors.push(format!(
            "{who}: conversation does not start with system+user"
        ));
    }
    let tool_msgs: Vec<&Message> = record
        .conversation()
        .iter()
        .filter(|m| m.role == crate::protocol::Role::Tool)
        .collect();
    if tool_msgs.len() != record.executed().len() {
        errors.push(format!(
            "{who}: {} tool messages, {} executed calls",
            tool_msgs.len(),
            record.executed().len()
        ));
    }
    for (tm, ex) in tool_msgs.iter().zip(record.executed().iter()) {
        if tm.tool_call_id.as_deref() != Some(ex.tool_call_id.as_str())
            || tm.name.as_deref() != Some(ex.tool_name.as_str())
            || tm.content.as_deref() != Some(ex.result.to_string().as_str())
        {
            errors.push(format!(
                "{who}: tool message does not mirror executed call {}",
                ex.sequence
            ));
        }
    }
    if tr == TerminationReason::Completed {
        let ok = matches!(
            record.conversation().last(),
            Some(m)
                if m.role == crate::protocol::Role::Assistant
                    && m.tool_calls.is_empty()
                    && m.content.as_deref() == record.final_answer().as_deref()
        );
        if !ok || record.final_answer().is_none() {
            errors.push(format!(
                "{who}: a completed episode must end with its final answer"
            ));
        }
    }
    // Oracle recomputation (candidate-blind).
    let eval = oracle::evaluate_task(
        task,
        &oracle::OracleInput {
            termination_reason: record.termination_reason().to_string(),
            initial_state: task.initial_state.clone(),
            final_state: record.final_state().clone(),
        },
    );
    if eval.success != record.oracle_success() || eval.reasons != record.oracle_failure_reasons() {
        errors.push(format!(
            "{who}: oracle recomputation diverges (success {} vs recorded {})",
            eval.success,
            record.oracle_success()
        ));
    }
    if record.fault_mode()
        != &serde_json::to_value(FaultMode::for_stress(&task.fault_key, drop_count)).unwrap()
    {
        errors.push(format!(
            "{who}: fault_mode does not match (fault_key, frozen stress)"
        ));
    }
    if record.initial_state() != &task.initial_state || record.target_state() != &task.target_state
    {
        errors.push(format!("{who}: recorded states diverge from the registry"));
    }
}
// (continued)

/// Verify the discovery artifacts (spec §67/§70).
pub fn verify_discovery(
    records: &[DiscoveryRecord],
    summary: &Value,
    mutation_input: &Value,
    mutation_input_file_sha: &str,
    ctx: &VerifierContext,
) -> Vec<String> {
    let mut errors = Vec::new();
    registry_ok(ctx, &mut errors);
    if records.len() != DISCOVERY_EPISODES {
        errors.push(format!(
            "discovery has {} episodes, expected {DISCOVERY_EPISODES}",
            records.len()
        ));
    }
    let mut seen_cells: BTreeMap<(String, u32), u32> = BTreeMap::new();
    for r in records {
        let who = r.run_id.clone();
        if r.experiment_id != EXPERIMENT_ID || r.phase != "discovery" {
            errors.push(format!("{who}: wrong experiment id/phase"));
        }
        if r.condition != "G0" {
            errors.push(format!(
                "{who}: discovery must run G0 only, found {:?}",
                r.condition
            ));
        }
        if r.generation != 0 {
            errors.push(format!("{who}: discovery generation must be 0"));
        }
        if r.provenance.temperature != TEMPERATURE {
            errors.push(format!("{who}: agent temperature != {TEMPERATURE}"));
        }
        if r.sequence == 0 {
            errors.push(format!("{who}: sequence must be 1-based"));
        }
        *seen_cells
            .entry((r.task_id.clone(), r.repetition))
            .or_insert(0) += 1;
        let Some(task) = ctx.task(&r.task_id) else {
            errors.push(format!("{who}: unknown task {}", r.task_id));
            continue;
        };
        if task.split != TaskSplit::Discovery {
            errors.push(format!("{who}: task {} is not a discovery task", task.id));
        }
        let Some((stress, count)) = frozen_stress(&task.family) else {
            continue;
        };
        if r.stress_level != stress || r.drop_count != count {
            errors.push(format!(
                "{who}: stress ({}, {}) != frozen ({stress}, {count})",
                r.stress_level, r.drop_count
            ));
        }
        if r.provenance.incumbent_prompt_sha256 != ctx.incumbent_file_sha
            || r.provenance.task_registry_sha256 != ctx.task_registry_sha
            || r.provenance.stress_profile_sha256 != ctx.profile_sha
        {
            errors.push(format!("{who}: provenance hash mismatch"));
        }
        if !r.conversation.is_empty()
            && r.conversation[0].content.as_deref() != Some(&ctx.incumbent_text)
        {
            errors.push(format!(
                "{who}: conversation[0] is not the frozen G0 prompt text"
            ));
        }
        verify_record_core(&mut errors, &who, task, count, &ctx.incumbent_text, r);
    }
    for (task_id, task_id2) in DISCOVERY_TASKS.iter().enumerate() {
        let _ = task_id;
        for rep in 1..=DISCOVERY_REPETITIONS {
            if seen_cells
                .get(&(task_id2.to_string(), rep))
                .copied()
                .unwrap_or(0)
                != 1
            {
                errors.push(format!(
                    "discovery cell ({task_id2}, rep {rep}) does not appear exactly once"
                ));
            }
        }
    }
    if seen_cells.len() != DISCOVERY_EPISODES {
        errors.push(format!(
            "discovery covers {} distinct cells, expected {DISCOVERY_EPISODES}",
            seen_cells.len()
        ));
    }
    let valid = records
        .iter()
        .filter(|r| r.infrastructure_failure.is_none())
        .count() as u32;
    if valid < MIN_VALID_DISCOVERY_EPISODES {
        errors.push(format!(
            "only {valid} valid discovery episodes (< {MIN_VALID_DISCOVERY_EPISODES}) — mutation generation must not run"
        ));
    }
    // Summary deep-compare against recomputation.
    let str_of = |k: &str| {
        summary
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let recomputed = compute_discovery_summary(
        records,
        &str_of("model"),
        &str_of("redacted_endpoint"),
        &str_of("code_under_test_commit"),
        &ctx.incumbent_file_sha,
        &ctx.task_registry_sha,
        &ctx.profile_sha,
        mutation_input_file_sha,
    );
    if summary != &serde_json::to_value(&recomputed).unwrap() {
        errors.push("discovery summary diverges from recomputation".to_string());
    }
    // Mutation input: whitelist construction, leakage, file hash.
    if let Ok(typed_input) = serde_json::from_value::<MutationInput>(mutation_input.clone()) {
        errors.extend(mutation_input_leak_errors(&typed_input, &ctx.registry));
    } else {
        errors.push("mutation input is not well-formed against the whitelist schema".to_string());
    }
    let expected_input = build_mutation_input(&ctx.incumbent_text, &ToolExecutor::specs(), records);
    if serde_json::to_value(&expected_input).unwrap() != *mutation_input {
        errors.push(
            "mutation input diverges from the discovery records (whitelist construction)"
                .to_string(),
        );
    }
    if summary.get("mutation_input_sha256").and_then(Value::as_str) != Some(mutation_input_file_sha)
    {
        errors.push("summary mutation_input_sha256 != frozen file hash".to_string());
    }
    errors
}

/// Verify the mutation-generation artifact + candidate pool
/// (spec §66/§70/§71).
pub fn verify_mutation(
    generation: &Value,
    pool: &CandidatePool,
    mutation_input: &Value,
    mutation_input_file_sha: &str,
    ctx: &VerifierContext,
) -> Vec<String> {
    let mut errors = Vec::new();
    registry_ok(ctx, &mut errors);

    if generation
        .get("mutation_input_sha256")
        .and_then(Value::as_str)
        != Some(mutation_input_file_sha)
    {
        errors.push(
            "generation artifact mutation_input_sha256 != frozen mutation input hash".to_string(),
        );
    }
    if generation
        .get("parent_prompt_sha256")
        .and_then(Value::as_str)
        != Some(&ctx.incumbent_file_sha)
    {
        errors
            .push("generation artifact parent_prompt_sha256 != frozen incumbent hash".to_string());
    }
    if generation
        .get("mutator_prompt_sha256")
        .and_then(Value::as_str)
        != Some(&ctx.mutator_file_sha)
    {
        errors.push(
            "generation artifact mutator_prompt_sha256 != frozen mutator prompt hash".to_string(),
        );
    }
    if generation.get("temperature").and_then(Value::as_f64) != Some(MUTATOR_TEMPERATURE) {
        errors.push(format!("generation temperature != {MUTATOR_TEMPERATURE}"));
    }
    let Some(attempts) = generation.get("attempts").and_then(Value::as_array) else {
        errors.push("generation artifact has no attempts".to_string());
        return errors;
    };
    if attempts.len() > mutation::MAX_GENERATION_ATTEMPTS as usize {
        errors.push(format!(
            "generation used {} attempts (> {})",
            attempts.len(),
            mutation::MAX_GENERATION_ATTEMPTS
        ));
    }
    if generation.get("attempt_count").and_then(Value::as_u64) != Some(attempts.len() as u64) {
        errors.push("generation attempt_count inconsistent with attempts".to_string());
    }
    let complete = generation
        .get("mutation_generation_complete")
        .and_then(Value::as_bool)
        == Some(true);
    if complete {
        let Some(accepted) = generation.get("accepted_pool") else {
            errors.push("generation marked complete but has no accepted pool".to_string());
            return errors;
        };
        // Every accepted suffix must equal the RAW model response
        // verbatim: the mechanical guard against harness-side injection
        // (e.g. the known 0009 repair) or post-acceptance editing.
        let Some(idx) = attempts
            .iter()
            .rposition(|a| a.get("structurally_valid").and_then(Value::as_bool) == Some(true))
        else {
            errors.push("no structurally valid attempt recorded".to_string());
            return errors;
        };
        let raw = attempts[idx]
            .get("raw_response")
            .and_then(Value::as_str)
            .unwrap_or("");
        let (parsed, verrs) =
            mutation::validate_mutator_response(raw, &ctx.incumbent_text, &ctx.registry);
        if !verrs.is_empty() {
            errors.push(format!("accepted attempt fails re-validation: {verrs:?}"));
        }
        let Some(parsed) = parsed else {
            return errors;
        };
        let accepted_pool: CandidatePool = match serde_json::from_value(accepted.clone()) {
            Ok(p) => p,
            Err(e) => {
                errors.push(format!("accepted pool is not well-formed: {e}"));
                return errors;
            }
        };
        if accepted_pool != *pool {
            errors.push(
                "candidate pool differs from the generation artifact's accepted pool".to_string(),
            );
        }
        for (p, raw_c) in accepted_pool.candidates.iter().zip(parsed.iter()) {
            if p.suffix != raw_c.suffix {
                errors.push(format!(
                    "candidate {}: pool suffix does not equal the raw model response verbatim (harness-side modification)",
                    p.candidate_id
                ));
            }
        }
    } else if generation.get("accepted_pool").is_some() {
        errors.push("an incomplete generation must not carry an accepted pool".to_string());
    }

    errors.extend(validate_candidate_pool(
        pool,
        &ctx.incumbent_text,
        &ctx.incumbent_file_sha,
        &ctx.registry,
    ));
    if pool.mutation_input_sha256 != mutation_input_file_sha {
        errors.push("pool mutation_input_sha256 != frozen mutation input hash".to_string());
    }
    if let Ok(typed_input) = serde_json::from_value::<MutationInput>(mutation_input.clone()) {
        errors.extend(mutation_input_leak_errors(&typed_input, &ctx.registry));
    }
    errors
}

/// Verify the selection artifacts (spec §68/§70/§71).
pub fn verify_selection(
    records: &[SelectionRecord],
    summary: &Value,
    selected: &Value,
    pool: &CandidatePool,
    pool_file_sha: &str,
    mutation_input_file_sha: &str,
    ctx: &VerifierContext,
) -> Vec<String> {
    let mut errors = Vec::new();
    registry_ok(ctx, &mut errors);
    if records.len() != SELECTION_EPISODES {
        errors.push(format!(
            "selection has {} episodes, expected {SELECTION_EPISODES}",
            records.len()
        ));
    }
    let mut position_counts: BTreeMap<(String, u32), u32> = BTreeMap::new();
    let mut seen: BTreeMap<(String, u32, String), u32> = BTreeMap::new();
    for r in records {
        let who = r.run_id.clone();
        if r.experiment_id != EXPERIMENT_ID || r.phase != "selection" {
            errors.push(format!("{who}: wrong experiment id/phase"));
        }
        if r.provenance.temperature != TEMPERATURE {
            errors.push(format!("{who}: temperature != {TEMPERATURE}"));
        }
        *position_counts
            .entry((r.condition.clone(), r.condition_position))
            .or_insert(0) += 1;
        *seen
            .entry((r.task_id.clone(), r.repetition, r.condition.clone()))
            .or_insert(0) += 1;
        if !SELECTION_CONDITIONS.contains(&r.condition.as_str()) {
            errors.push(format!("{who}: unregistered condition {:?}", r.condition));
            continue;
        }
        if selection_condition_at(r.repetition, r.condition_position) != r.condition {
            errors.push(format!(
                "{who}: condition {} at (rep {}, pos {}) violates the registered cyclic order",
                r.condition, r.repetition, r.condition_position
            ));
        }
        if r.candidate_pool_sha256 != pool_file_sha {
            errors.push(format!("{who}: pool hash mismatch"));
        }
        if r.mutation_input_sha256 != mutation_input_file_sha {
            errors.push(format!("{who}: mutation input hash mismatch"));
        }
        if r.provenance.incumbent_prompt_sha256 != ctx.incumbent_file_sha
            || r.provenance.task_registry_sha256 != ctx.task_registry_sha
            || r.provenance.stress_profile_sha256 != ctx.profile_sha
        {
            errors.push(format!("{who}: provenance hash mismatch"));
        }
        let Some(task) = ctx.task(&r.task_id) else {
            errors.push(format!("{who}: unknown task {}", r.task_id));
            continue;
        };
        if task.split != TaskSplit::Selection {
            errors.push(format!(
                "{who}: promotion/discovery task in the selection artifact ({})",
                task.id
            ));
        }
        let Some((stress, count)) = frozen_stress(&task.family) else {
            continue;
        };
        if r.stress_level != stress || r.drop_count != count {
            errors.push(format!("{who}: frozen stress mismatch"));
        }
        let (expected_full, expected_suffix_sha) = if r.condition == "G0" {
            (ctx.incumbent_text.clone(), None)
        } else {
            match pool
                .candidates
                .iter()
                .find(|c| c.candidate_id == r.condition)
            {
                Some(e) => (
                    mutation::compose_full_prompt(&ctx.incumbent_text, &e.suffix),
                    Some(e.suffix_sha256.clone()),
                ),
                None => {
                    errors.push(format!(
                        "{who}: unknown candidate condition {:?}",
                        r.condition
                    ));
                    (ctx.incumbent_text.clone(), None)
                }
            }
        };
        if r.full_prompt_sha256 != crate::protocol::sha256_bytes(expected_full.as_bytes()) {
            errors.push(format!("{who}: full_prompt_sha256 mismatch"));
        }
        if r.condition == "G0" {
            if r.generation != 0 || r.suffix_sha256.is_some() {
                errors.push(format!("{who}: G0 must be generation 0 with null suffix"));
            }
        } else if r.suffix_sha256 != expected_suffix_sha {
            errors.push(format!("{who}: suffix hash does not match the frozen pool"));
        } else if r.generation != 1 {
            errors.push(format!("{who}: candidate must be generation 1"));
        }
        verify_record_core(&mut errors, &who, task, count, &expected_full, r);
    }
    for ((cond, pos), n) in &position_counts {
        if *n != 6 {
            errors.push(format!(
                "condition {cond} position {pos} has {n} episodes, expected 6 (registered cyclic balance)"
            ));
        }
    }
    if position_counts.len() != 25 {
        errors.push(format!(
            "selection covers {} condition×position cells, expected 25",
            position_counts.len()
        ));
    }
    for t in SELECTION_TASKS {
        for rep in 1..=SELECTION_REPETITIONS {
            for c in SELECTION_CONDITIONS {
                if seen
                    .get(&(t.to_string(), rep, c.to_string()))
                    .copied()
                    .unwrap_or(0)
                    != 1
                {
                    errors.push(format!(
                        "selection cell ({t}, rep {rep}, {c}) does not appear exactly once"
                    ));
                }
            }
        }
    }
    // Summary deep-compare.
    let str_of = |k: &str| {
        summary
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let recomputed = compute_selection_summary(
        records,
        pool,
        &str_of("model"),
        &str_of("redacted_endpoint"),
        &str_of("code_under_test_commit"),
        &ctx.incumbent_file_sha,
        &ctx.task_registry_sha,
        &ctx.profile_sha,
        pool_file_sha,
        mutation_input_file_sha,
    );
    if summary != &serde_json::to_value(&recomputed).unwrap() {
        errors.push("selection summary diverges from recomputation".to_string());
    }
    // Selected-candidate artifact vs the deterministic rule.
    let Ok(sel) = serde_json::from_value::<SelectedCandidate>(selected.clone()) else {
        errors.push("selected-candidate artifact is not well-formed".to_string());
        return errors;
    };
    if sel.selected_candidate_id != recomputed.selected.selected_candidate_id
        || sel.incumbent_retained() != recomputed.selected.incumbent_retained
    {
        errors
            .push("selected-candidate artifact disagrees with the deterministic rule".to_string());
    }
    if sel.candidate_pool_sha256 != pool_file_sha {
        errors.push("selected-candidate pool hash mismatch".to_string());
    }
    if sel.mutation_input_sha256 != mutation_input_file_sha {
        errors.push("selected-candidate mutation input hash mismatch".to_string());
    }
    if sel.selected_candidate_id == "G0" {
        if sel.suffix.is_some() || sel.suffix_sha256.is_some() || sel.full_prompt_sha256.is_some() {
            errors.push("retained G0 must carry null suffix fields".to_string());
        }
    } else {
        let entry = pool
            .candidates
            .iter()
            .find(|c| c.candidate_id == sel.selected_candidate_id);
        let Some(entry) = entry else {
            errors.push("selected candidate is not in the frozen pool".to_string());
            return errors;
        };
        if sel.suffix.as_deref() != Some(entry.suffix.as_str())
            || sel.suffix_sha256.as_deref() != Some(entry.suffix_sha256.as_str())
            || sel.full_prompt_sha256.as_deref() != Some(entry.full_prompt_sha256.as_str())
        {
            errors.push(
                "selected-candidate suffix/prompt do not match the frozen pool entry".to_string(),
            );
        }
    }
    // Rule consistency against the recomputed tallies.
    if recomputed.gates.all() && !recomputed.selected.incumbent_retained {
        let best = recomputed
            .candidates
            .iter()
            .find(|t| t.candidate_id == recomputed.selected.selected_candidate_id)
            .map(|t| t.net_margin)
            .unwrap_or(i64::MIN);
        if best <= 0 {
            errors
                .push("a candidate with net margin <= 0 was selected (rule violation)".to_string());
        }
    }
    if recomputed.gates.all()
        && recomputed.selected.incumbent_retained
        && recomputed.candidates.iter().any(|t| t.net_margin > 0)
    {
        errors.push(
            "G0 was retained although a candidate had net margin > 0 (rule violation)".to_string(),
        );
    }
    errors
}

/// Verify the promotion artifacts (spec §69/§72).
#[allow(clippy::too_many_arguments)] // the frozen provenance hashes are part of the artifact contract
pub fn verify_promotion(
    records: &[PromotionRecord],
    summary: &Value,
    selected: &Value,
    pool: &CandidatePool,
    pool_file_sha: &str,
    mutation_input_file_sha: &str,
    selected_file_sha: &str,
    ctx: &VerifierContext,
) -> Vec<String> {
    let mut errors = Vec::new();
    registry_ok(ctx, &mut errors);
    let Ok(sel) = serde_json::from_value::<SelectedCandidate>(selected.clone()) else {
        return vec!["selected-candidate artifact is not well-formed".to_string()];
    };
    let selected_id = sel.selected_candidate_id.clone();
    if !sel.incumbent_retained()
        && !pool
            .candidates
            .iter()
            .any(|c| c.candidate_id == selected_id)
    {
        errors.push("promotion runs a candidate that is not the frozen selected one".to_string());
    }
    let sel_entry = pool
        .candidates
        .iter()
        .find(|c| c.candidate_id == selected_id);
    if records.len() != PROMOTION_EPISODES {
        errors.push(format!(
            "promotion has {} episodes, expected {PROMOTION_EPISODES}",
            records.len()
        ));
    }
    let mut pair_counts: BTreeMap<(String, u32, String), u32> = BTreeMap::new();
    for r in records {
        let who = r.run_id.clone();
        if r.experiment_id != EXPERIMENT_ID || r.phase != "promotion" {
            errors.push(format!("{who}: wrong experiment id/phase"));
        }
        if r.provenance.temperature != TEMPERATURE {
            errors.push(format!("{who}: temperature != {TEMPERATURE}"));
        }
        *pair_counts
            .entry((r.task_id.clone(), r.repetition, r.condition.clone()))
            .or_insert(0) += 1;
        let allowed = r.condition == "G0" || r.condition == selected_id;
        if !allowed {
            errors.push(format!(
                "{who}: unselected condition {:?} in promotion (multi-candidate leakage)",
                r.condition
            ));
            continue;
        }
        let want = promotion_condition_order(r.repetition, &selected_id)
            .get((r.condition_position - 1) as usize)
            .copied();
        if want != Some(r.condition.as_str()) {
            errors.push(format!(
                "{who}: condition {} at position {} violates the registered alternating order",
                r.condition, r.condition_position
            ));
        }
        if r.candidate_pool_sha256 != pool_file_sha {
            errors.push(format!("{who}: pool hash mismatch"));
        }
        if r.mutation_input_sha256 != mutation_input_file_sha {
            errors.push(format!("{who}: mutation input hash mismatch"));
        }
        if r.selected_candidate_sha256 != selected_file_sha {
            errors.push(format!(
                "{who}: selected-candidate hash does not match the frozen selected-candidate file"
            ));
        }
        if r.provenance.incumbent_prompt_sha256 != ctx.incumbent_file_sha
            || r.provenance.task_registry_sha256 != ctx.task_registry_sha
            || r.provenance.stress_profile_sha256 != ctx.profile_sha
        {
            errors.push(format!("{who}: provenance hash mismatch"));
        }
        let Some(task) = ctx.task(&r.task_id) else {
            errors.push(format!("{who}: unknown task {}", r.task_id));
            continue;
        };
        if task.split != TaskSplit::Promotion {
            errors.push(format!(
                "{who}: non-promotion task ({}) in the promotion artifact",
                task.id
            ));
        }
        let Some((stress, count)) = frozen_stress(&task.family) else {
            continue;
        };
        if r.stress_level != stress || r.drop_count != count {
            errors.push(format!("{who}: frozen stress mismatch"));
        }
        let expected_full = if r.condition == "G0" {
            ctx.incumbent_text.clone()
        } else {
            match sel_entry {
                Some(e) => mutation::compose_full_prompt(&ctx.incumbent_text, &e.suffix),
                None => ctx.incumbent_text.clone(),
            }
        };
        if r.full_prompt_sha256 != crate::protocol::sha256_bytes(expected_full.as_bytes()) {
            errors.push(format!(
                "{who}: full_prompt_sha256 mismatch (candidate text changed after freeze?)"
            ));
        }
        if r.condition == "G0" {
            if r.suffix_sha256.is_some() || r.generation != 0 {
                errors.push(format!("{who}: G0 must be generation 0 with null suffix"));
            }
        } else if let Some(e) = sel_entry {
            if r.suffix_sha256.as_deref() != Some(e.suffix_sha256.as_str()) {
                errors.push(format!(
                    "{who}: suffix hash does not match the frozen selected candidate"
                ));
            }
        }
        verify_record_core(&mut errors, &who, task, count, &expected_full, r);
    }
    for t in PROMOTION_TASKS {
        for rep in 1..=PROMOTION_REPETITIONS {
            for c in ["G0", selected_id.as_str()] {
                if pair_counts
                    .get(&(t.to_string(), rep, c.to_string()))
                    .copied()
                    .unwrap_or(0)
                    != 1
                {
                    errors.push(format!(
                        "promotion cell ({t}, rep {rep}, {c}) does not appear exactly once"
                    ));
                }
            }
        }
    }
    // Summary deep-compare.
    let str_of = |k: &str| {
        summary
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let recomputed = compute_promotion_summary(
        records,
        &selected_id,
        &str_of("model"),
        &str_of("redacted_endpoint"),
        &str_of("code_under_test_commit"),
        &ctx.incumbent_file_sha,
        &ctx.task_registry_sha,
        &ctx.profile_sha,
        pool_file_sha,
        &str_of("selected_candidate_sha256"),
        mutation_input_file_sha,
    );
    if summary != &serde_json::to_value(&recomputed).unwrap() {
        errors.push("promotion summary diverges from recomputation".to_string());
    }
    if sel.candidate_pool_sha256 != pool_file_sha {
        errors.push("selected-candidate pool hash mismatch".to_string());
    }
    if sel.mutation_input_sha256 != mutation_input_file_sha {
        errors.push("selected-candidate mutation input hash mismatch".to_string());
    }
    if summary
        .get("selected_candidate_sha256")
        .and_then(Value::as_str)
        != Some(selected_file_sha)
    {
        errors.push(
            "promotion summary selected-candidate hash does not match the frozen file (results recorded against a different freeze)".to_string(),
        );
    }
    errors
}
// (continued)

// ===========================================================================
// Synthetic datasets + tamper suite (for `self-test`; network-free)
// ===========================================================================

/// A fake incumbent used only by the network-free synthetic data.
pub const SYNTH_INCUMBENT: &str =
    "You are a synthetic test agent.\nObserve and act on state carefully.\n";

/// Synthetic candidate suffixes (all must pass the content validator).
pub const SYNTH_SUFFIXES: [&str; 4] = [
    "Always read the current value of a key before writing it, so your actions follow observed state.",
    "After any action, check that the current external state matches the requested goal before declaring completion.",
    "Verify the current external state before giving a final answer to the user's task.",
    "If the observed state does not match the requested goal, adjust your next action accordingly.",
];

const SHA_A: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const SHA_B: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const SHA_C: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const SHA_D: &str = "4444444444444444444444444444444444444444444444444444444444444444";
const SHA_E: &str = "5555555555555555555555555555555555555555555555555555555555555555";
const SHA_F: &str = "6666666666666666666666666666666666666666666666666666666666666666";
const SHA_G: &str = "7777777777777777777777777777777777777777777777777777777777777777";
const SHA_0: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// One fully-consistent synthetic episode (conversation, calls, audit,
/// usage, timing) for a registered task under one frozen stress.
/// One synthetic episode's six streams (type alias: clippy type_complexity).
/// Note: the six tuple fields are (conversation, requests, requested,
/// executed, write attempts, final state) in that fixed order.
type EpisodeParts = (
    Vec<Message>,
    Vec<ModelRequestRecord>,
    Vec<RequestedToolCall>,
    Vec<ExecutedToolCall>,
    Vec<WriteAttempt>,
    Value,
);

fn synth_episode(
    task: &TaskSpec,
    full_prompt: &str,
    success: bool,
    drop_count: u32,
) -> EpisodeParts {
    use crate::protocol::ToolCall;
    let key = task.fault_key.clone();
    let target_value = task
        .target_state
        .get(&key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| "SYNTH".to_string());
    let mut conv: Vec<Message> = vec![Message::system(full_prompt), Message::user(&task.prompt)];
    let mut requests: Vec<ModelRequestRecord> = Vec::new();
    let mut requested: Vec<RequestedToolCall> = Vec::new();
    let mut executed: Vec<ExecutedToolCall> = Vec::new();
    let mut attempts: Vec<WriteAttempt> = Vec::new();

    let pushes = |conv: &mut Vec<Message>,
                  requests: &mut Vec<ModelRequestRecord>,
                  requested: &mut Vec<RequestedToolCall>,
                  executed: &mut Vec<ExecutedToolCall>,
                  attempts: &mut Vec<WriteAttempt>,
                  key: &str,
                  value: &str,
                  ordinal: u32,
                  applied: bool| {
        let args = json!({ "key": key, "value": value });
        let tc_id = format!("tc{}", conv.len());
        conv.push(Message::assistant_tool_calls(
            vec![ToolCall::function(
                tc_id.clone(),
                "state_write",
                args.to_string(),
            )],
            None,
        ));
        let result = json!({ "ok": true, "key": key, "value": value });
        conv.push(Message::tool_result(
            tc_id.clone(),
            "state_write".to_string(),
            result.to_string(),
        ));
        requests.push(ModelRequestRecord {
            turn: requests.len() as u32 + 1,
            finish_reason: Some("tool_calls".to_string()),
            prompt_tokens: Some(100),
            completion_tokens: Some(5),
            total_tokens: Some(105),
            cached_prompt_tokens: Some(0),
            reasoning_tokens: Some(0),
            uncached_prompt_tokens: Some(100),
        });
        requested.push(RequestedToolCall {
            sequence: requested.len() as u32,
            turn: requested.len() as u32,
            tool_call_id: tc_id.clone(),
            tool_name: "state_write".to_string(),
            arguments: args,
        });
        executed.push(ExecutedToolCall {
            sequence: executed.len() as u32,
            turn: executed.len() as u32,
            tool_call_id: tc_id,
            tool_name: "state_write".to_string(),
            arguments: json!({ "key": key, "value": value }),
            result,
            duration_us: 1,
        });
        attempts.push(WriteAttempt {
            sequence: attempts.len() as u32,
            key: key.to_string(),
            requested_value: value.to_string(),
            applied,
            fault_reason: (!applied).then(|| "drop_first_n_writes".to_string()),
            registered_drop_index: Some(ordinal),
        });
    };

    if success {
        // count + 1 writes to the fault key beat the drop window.
        for n in 1..=drop_count + 1 {
            pushes(
                &mut conv,
                &mut requests,
                &mut requested,
                &mut executed,
                &mut attempts,
                &key,
                &target_value,
                n,
                n > drop_count,
            );
        }
    } else {
        // Exactly one write to the fault key, inside the drop window
        // (drop_count >= 1 for every registered family), so it is
        // silently dropped, the final state stays at the initial state,
        // and the model gives a (false) completion answer.
        pushes(
            &mut conv,
            &mut requests,
            &mut requested,
            &mut executed,
            &mut attempts,
            &key,
            &target_value,
            1,
            false,
        );
    }
    conv.push(Message::assistant_text(
        "The requested external state has been achieved.",
    ));
    requests.push(ModelRequestRecord {
        turn: requests.len() as u32 + 1,
        finish_reason: Some("stop".to_string()),
        prompt_tokens: Some(100),
        completion_tokens: Some(5),
        total_tokens: Some(105),
        cached_prompt_tokens: Some(0),
        reasoning_tokens: Some(0),
        uncached_prompt_tokens: Some(100),
    });

    let final_state = if success {
        task.target_state.clone()
    } else {
        task.initial_state.clone()
    };
    (conv, requests, requested, executed, attempts, final_state)
}

/// A provenance block for synthetic records.
fn synth_provenance(ctx: &VerifierContext) -> RecordProvenance {
    RecordProvenance {
        model: "synthetic-model".to_string(),
        temperature: TEMPERATURE,
        redacted_endpoint: "http://synthetic.invalid/v1".to_string(),
        code_under_test_commit: SHA_0.to_string(),
        incumbent_prompt_sha256: ctx.incumbent_file_sha.clone(),
        mutator_prompt_sha256: ctx.mutator_file_sha.clone(),
        task_registry_sha256: ctx.task_registry_sha.clone(),
        stress_profile_sha256: ctx.profile_sha.clone(),
    }
}

fn synth_timing() -> Timing {
    Timing {
        wall_time_ms: 1000,
        model_wait_time_ms: 900,
        tool_execution_time_us: 100,
        kernel_overhead_estimate_ms: 100,
        approximate: true,
    }
}

/// The complete synthetic artifact set for the network-free self-test
/// and the tamper suite.
pub struct SyntheticArtifacts {
    pub ctx: VerifierContext,
    pub discovery: Vec<DiscoveryRecord>,
    pub discovery_summary: DiscoverySummary,
    pub mutation_input: MutationInput,
    pub generation: crate::mutation::GenerationArtifact,
    pub pool: CandidatePool,
    pub selection: Vec<SelectionRecord>,
    pub selection_summary: SelectionSummary,
    pub selected: SelectedCandidate,
    pub promotion: Vec<PromotionRecord>,
    pub promotion_summary: PromotionSummary,
    // Deterministic synthetic file hashes.
    pub input_file_sha: &'static str,
    pub pool_file_sha: &'static str,
    pub selected_file_sha: &'static str,
}

/// Build the full synthetic artifact set. The design encodes:
/// - discovery: 18 episodes, 12 successes / 6 failures, no infra;
/// - selection: G0 fails 13/30 cells; C2 wins all 13 (selected);
///   C1 and C3 tie exactly (ordinal tie-break); C4 negative;
/// - promotion: 30 wins / 2 losses / 16 ties → quality_improvement.
pub fn build_synthetic_artifacts() -> SyntheticArtifacts {
    let incumbent = SYNTH_INCUMBENT.to_string();
    let registry =
        crate::experiment::load_tasks(&crate::experiment::crate_dir().join("tasks.json"))
            .expect("registry loads");
    let profile: Value = serde_json::from_str(
        std::fs::read_to_string(crate::experiment::crate_dir().join("stress-profile.json"))
            .as_deref()
            .unwrap_or("{}"),
    )
    .unwrap_or(Value::Null);
    let ctx = VerifierContext::new(
        incumbent.clone(),
        SHA_A.to_string(),
        SHA_B.to_string(),
        registry.tasks.clone(),
        profile.clone(),
    );
    let prov = synth_provenance(&ctx);

    // ---------- discovery ----------
    let mut discovery = Vec::new();
    let mut counter = 0usize;
    for (ti, task) in registry
        .tasks
        .iter()
        .filter(|t| t.split == TaskSplit::Discovery)
        .enumerate()
    {
        let (_, count) = frozen_stress(&task.family).unwrap();
        for rep in 1..=DISCOVERY_REPETITIONS {
            counter += 1;
            let success = rep <= 3;
            let (conv, req, rqc, exc, att, final_state) =
                synth_episode(task, &incumbent, success, count);
            let eval = oracle::evaluate_task(
                task,
                &oracle::OracleInput {
                    termination_reason: "completed".to_string(),
                    initial_state: task.initial_state.clone(),
                    final_state: final_state.clone(),
                },
            );
            discovery.push(DiscoveryRecord {
                experiment_id: EXPERIMENT_ID.to_string(),
                phase: "discovery".to_string(),
                run_id: format!("synth-disc-{counter:03}"),
                sequence: counter as u32,
                family: task.family.clone(),
                task_id: task.id.clone(),
                split: "discovery".to_string(),
                stress_level: frozen_stress(&task.family).unwrap().0.to_string(),
                drop_count: count,
                fault_key: task.fault_key.clone(),
                repetition: rep,
                condition: "G0".to_string(),
                generation: 0,
                provenance: prov.clone(),
                initial_state: task.initial_state.clone(),
                target_state: task.target_state.clone(),
                fault_mode: serde_json::to_value(FaultMode::for_stress(&task.fault_key, count))
                    .unwrap(),
                conversation: conv,
                model_requests: req,
                requested_tool_calls: rqc,
                executed_tool_calls: exc,
                environment_write_attempts: att,
                final_state,
                final_answer: Some("The requested external state has been achieved.".to_string()),
                model_turn_count: 0, // filled below
                tool_call_count: 0,  // filled below
                agent_failure: None,
                infrastructure_failure: None,
                usage_accounting_error: None,
                termination_reason: "completed".to_string(),
                oracle_success: eval.success,
                oracle_failure_reasons: eval.reasons,
                timing: synth_timing(),
            });
            let r = &mut discovery[counter - 1];
            // Turns: one model turn per successful request, including the final answer.
            r.model_turn_count = r.model_requests.len() as u32;
            r.tool_call_count = r.executed_tool_calls.len() as u32;
            debug_assert!(r.model_turn_count <= 12 && r.tool_call_count <= 16);
        }
        let _ = ti;
    }
    let input_file_sha = SHA_C;
    let input = build_mutation_input(&incumbent, &ToolExecutor::specs(), &discovery);
    let discovery_summary = compute_discovery_summary(
        &discovery,
        "synthetic-model",
        "http://synthetic.invalid/v1",
        SHA_0,
        &ctx.incumbent_file_sha,
        &ctx.task_registry_sha,
        &ctx.profile_sha,
        input_file_sha,
    );

    // ---------- candidate pool ----------
    let raw_candidates: Vec<crate::mutation::RawCandidate> = SYNTH_SUFFIXES
        .iter()
        .map(|s| crate::mutation::RawCandidate {
            suffix: s.to_string(),
            rationale: "synthetic rationale".to_string(),
        })
        .collect();
    let (validated, verrs) = crate::mutation::validate_mutator_response(
        &serde_json::to_value(json!({ "candidates": raw_candidates }))
            .unwrap()
            .to_string(),
        &incumbent,
        &registry.tasks,
    );
    assert!(
        verrs.is_empty(),
        "synthetic suffixes must validate: {verrs:?}"
    );
    let pool = crate::mutation::build_pool(
        &validated.unwrap(),
        &incumbent,
        &ctx.incumbent_file_sha,
        input_file_sha,
    );
    let pool_file_sha = SHA_D;
    let raw_json = serde_json::to_value(json!({ "candidates": raw_candidates }))
        .unwrap()
        .to_string();
    let generation = crate::mutation::GenerationArtifact {
        experiment_id: EXPERIMENT_ID.to_string(),
        model: "synthetic-model".to_string(),
        temperature: MUTATOR_TEMPERATURE,
        redacted_endpoint: "http://synthetic.invalid/v1".to_string(),
        code_under_test_commit: SHA_0.to_string(),
        mutation_input_sha256: input_file_sha.to_string(),
        mutator_prompt_sha256: ctx.mutator_file_sha.clone(),
        parent_prompt_sha256: ctx.incumbent_file_sha.clone(),
        attempt_count: 1,
        attempts: vec![crate::mutation::GenerationAttempt {
            attempt: 1,
            raw_response: Some(raw_json),
            structurally_valid: true,
            validation_errors: Vec::new(),
            prompt_tokens: Some(1000),
            cached_prompt_tokens: Some(0),
            completion_tokens: Some(200),
            reasoning_tokens: None,
            wall_time_ms: 1500,
        }],
        mutation_generation_complete: true,
        accepted_pool: Some(pool.clone()),
        wall_time_ms: 1500,
    };

    // ---------- selection ----------
    // G0 fails on 13 of 30 cells: c%10 ∈ {0,1,2} (9) plus {5,6,7,24} (4).
    let g0_fail = |c: usize| -> bool { c % 10 < 3 || c == 5 || c == 6 || c == 7 || c == 24 };
    let c1_fail =
        |c: usize| -> bool { !matches!(c, 0 | 10 | 20) && (g0_fail(c) || c == 3 || c == 13) };
    let c3_fail =
        |c: usize| -> bool { !matches!(c, 1 | 11 | 21) && (g0_fail(c) || c == 4 || c == 14) };
    let c4_fail =
        |c: usize| -> bool { !matches!(c, 0 | 10) && (g0_fail(c) || c == 3 || c == 4 || c == 13) };
    let cond_success = |c: usize, cond: &str| -> bool {
        match cond {
            "G0" => !g0_fail(c),
            "C1" => !c1_fail(c),
            "C2" => true,
            "C3" => !c3_fail(c),
            "C4" => !c4_fail(c),
            _ => unreachable!(),
        }
    };
    let mut selection = Vec::new();
    let mut counter = 0usize;
    for (ti, task) in registry
        .tasks
        .iter()
        .filter(|t| t.split == TaskSplit::Selection)
        .enumerate()
    {
        let (_, count) = frozen_stress(&task.family).unwrap();
        for rep in 1..=SELECTION_REPETITIONS {
            for pos in 1..=SELECTION_CONDITIONS.len() as u32 {
                let cond = selection_condition_at(rep, pos);
                let c = ti * SELECTION_REPETITIONS as usize + (rep - 1) as usize;
                counter += 1;
                let success = cond_success(c, cond);
                let (suffix_sha, full) = if cond == "G0" {
                    (None, incumbent.clone())
                } else {
                    let e = &pool.candidates[(cond.as_bytes()[1] as usize - b'0' as usize) - 1];
                    (
                        Some(e.suffix_sha256.clone()),
                        crate::mutation::compose_full_prompt(&incumbent, &e.suffix),
                    )
                };
                let (conv, req, rqc, exc, att, final_state) =
                    synth_episode(task, &full, success, count);
                let eval = oracle::evaluate_task(
                    task,
                    &oracle::OracleInput {
                        termination_reason: "completed".to_string(),
                        initial_state: task.initial_state.clone(),
                        final_state: final_state.clone(),
                    },
                );
                let mut rec = SelectionRecord {
                    experiment_id: EXPERIMENT_ID.to_string(),
                    phase: "selection".to_string(),
                    run_id: format!("synth-sel-{counter:03}"),
                    sequence: counter as u32,
                    family: task.family.clone(),
                    task_id: task.id.clone(),
                    split: "selection".to_string(),
                    stress_level: frozen_stress(&task.family).unwrap().0.to_string(),
                    drop_count: count,
                    fault_key: task.fault_key.clone(),
                    repetition: rep,
                    condition_position: pos,
                    condition: cond.to_string(),
                    generation: if cond == "G0" { 0 } else { 1 },
                    parent_id: "G0".to_string(),
                    suffix_sha256: suffix_sha,
                    full_prompt_sha256: crate::protocol::sha256_bytes(full.as_bytes()),
                    candidate_pool_sha256: pool_file_sha.to_string(),
                    mutation_input_sha256: input_file_sha.to_string(),
                    provenance: prov.clone(),
                    initial_state: task.initial_state.clone(),
                    target_state: task.target_state.clone(),
                    fault_mode: serde_json::to_value(FaultMode::for_stress(&task.fault_key, count))
                        .unwrap(),
                    conversation: conv,
                    model_requests: req,
                    requested_tool_calls: rqc,
                    executed_tool_calls: exc,
                    environment_write_attempts: att,
                    final_state,
                    final_answer: Some(
                        "The requested external state has been achieved.".to_string(),
                    ),
                    model_turn_count: 0,
                    tool_call_count: 0,
                    agent_failure: None,
                    infrastructure_failure: None,
                    usage_accounting_error: None,
                    termination_reason: "completed".to_string(),
                    oracle_success: eval.success,
                    oracle_failure_reasons: eval.reasons,
                    timing: synth_timing(),
                };
                rec.model_turn_count = rec.model_requests.len() as u32;
                rec.tool_call_count = rec.executed_tool_calls.len() as u32;
                selection.push(rec);
            }
        }
    }
    let selection_summary = compute_selection_summary(
        &selection,
        &pool,
        "synthetic-model",
        "http://synthetic.invalid/v1",
        SHA_0,
        &ctx.incumbent_file_sha,
        &ctx.task_registry_sha,
        &ctx.profile_sha,
        pool_file_sha,
        input_file_sha,
    );
    // Sanity: the synthetic design must select C2.
    assert_eq!(
        selection_summary.selected.selected_candidate_id, "C2",
        "synthetic selection design must select C2, got {:?}",
        selection_summary.selected
    );
    let selected = SelectedCandidate {
        experiment_id: EXPERIMENT_ID.to_string(),
        selected_candidate_id: selection_summary.selected.selected_candidate_id.clone(),
        parent_id: "G0".to_string(),
        candidate_pool_sha256: pool_file_sha.to_string(),
        selection_raw_sha256: SHA_F.to_string(),
        selection_summary_sha256: SHA_G.to_string(),
        mutation_input_sha256: input_file_sha.to_string(),
        suffix: Some(pool.candidates[1].suffix.clone()),
        suffix_sha256: Some(pool.candidates[1].suffix_sha256.clone()),
        full_prompt_sha256: Some(pool.candidates[1].full_prompt_sha256.clone()),
        selection: FrozenSelectionBlock {
            common_valid_cells: selection_summary.common_valid_cells,
            baseline_failures: selection_summary.g0_failures_in_common_valid,
            wins: 13,
            losses: 0,
            ties: 17,
            net_margin: 13,
        },
        tie_break_path: selection_summary.selected.tie_break_path.clone(),
    };

    // ---------- promotion ----------
    // G0 fails on 32/48 pairs (group t0 all 8, groups t1..t4 six each,
    // t5 none); C2 fails exactly on pairs {26,28,30,31} — two of them
    // inside G0 failures (both fail → ties) and two inside G0 successes
    // (losses). Yields 30 wins / 2 losses / 16 ties.
    let g0_fail_p = |p: usize| -> bool {
        let t = p / 8;
        let r = p % 8;
        match t {
            0 => true,
            1..=4 => r < 6,
            _ => false,
        }
    };
    let c2_fail_p = |p: usize| -> bool { matches!(p, 26 | 28 | 30 | 31) };
    let mut promotion = Vec::new();
    let mut counter = 0usize;
    for (ti, task) in registry
        .tasks
        .iter()
        .filter(|t| t.split == TaskSplit::Promotion)
        .enumerate()
    {
        let (_, count) = frozen_stress(&task.family).unwrap();
        for rep in 1..=PROMOTION_REPETITIONS {
            for (pos, cond) in promotion_condition_order(rep, "C2").into_iter().enumerate() {
                let p = ti * PROMOTION_REPETITIONS as usize + (rep - 1) as usize;
                counter += 1;
                let success = if cond == "G0" {
                    !g0_fail_p(p)
                } else {
                    !c2_fail_p(p)
                };
                let (suffix_sha, full) = if cond == "G0" {
                    (None, incumbent.clone())
                } else {
                    (
                        Some(pool.candidates[1].suffix_sha256.clone()),
                        crate::mutation::compose_full_prompt(
                            &incumbent,
                            &pool.candidates[1].suffix,
                        ),
                    )
                };
                let (conv, req, rqc, exc, att, final_state) =
                    synth_episode(task, &full, success, count);
                let eval = oracle::evaluate_task(
                    task,
                    &oracle::OracleInput {
                        termination_reason: "completed".to_string(),
                        initial_state: task.initial_state.clone(),
                        final_state: final_state.clone(),
                    },
                );
                let mut rec = PromotionRecord {
                    experiment_id: EXPERIMENT_ID.to_string(),
                    phase: "promotion".to_string(),
                    run_id: format!("synth-prom-{counter:03}"),
                    sequence: counter as u32,
                    family: task.family.clone(),
                    task_id: task.id.clone(),
                    split: "promotion".to_string(),
                    stress_level: frozen_stress(&task.family).unwrap().0.to_string(),
                    drop_count: count,
                    fault_key: task.fault_key.clone(),
                    repetition: rep,
                    condition_position: (pos + 1) as u32,
                    condition: cond.to_string(),
                    generation: if cond == "G0" { 0 } else { 1 },
                    parent_id: "G0".to_string(),
                    suffix_sha256: suffix_sha,
                    full_prompt_sha256: crate::protocol::sha256_bytes(full.as_bytes()),
                    candidate_pool_sha256: pool_file_sha.to_string(),
                    selected_candidate_sha256: SHA_E.to_string(),
                    mutation_input_sha256: input_file_sha.to_string(),
                    provenance: prov.clone(),
                    initial_state: task.initial_state.clone(),
                    target_state: task.target_state.clone(),
                    fault_mode: serde_json::to_value(FaultMode::for_stress(&task.fault_key, count))
                        .unwrap(),
                    conversation: conv,
                    model_requests: req,
                    requested_tool_calls: rqc,
                    executed_tool_calls: exc,
                    environment_write_attempts: att,
                    final_state,
                    final_answer: Some(
                        "The requested external state has been achieved.".to_string(),
                    ),
                    model_turn_count: 0,
                    tool_call_count: 0,
                    agent_failure: None,
                    infrastructure_failure: None,
                    usage_accounting_error: None,
                    termination_reason: "completed".to_string(),
                    oracle_success: eval.success,
                    oracle_failure_reasons: eval.reasons,
                    timing: synth_timing(),
                };
                rec.model_turn_count = rec.model_requests.len() as u32;
                rec.tool_call_count = rec.executed_tool_calls.len() as u32;
                promotion.push(rec);
            }
        }
    }
    let promotion_summary = compute_promotion_summary(
        &promotion,
        "C2",
        "synthetic-model",
        "http://synthetic.invalid/v1",
        SHA_0,
        &ctx.incumbent_file_sha,
        &ctx.task_registry_sha,
        &ctx.profile_sha,
        pool_file_sha,
        SHA_E,
        input_file_sha,
    );
    assert_eq!(
        promotion_summary.conclusion.result, "supported",
        "synthetic promotion must be supported, got {:?}",
        promotion_summary.conclusion
    );
    assert_eq!(
        (
            promotion_summary.wins,
            promotion_summary.losses,
            promotion_summary.ties
        ),
        (30, 2, 16),
        "synthetic promotion design mismatch: {:?} {:?}",
        promotion_summary.wins,
        promotion_summary.losses
    );

    SyntheticArtifacts {
        ctx,
        discovery,
        discovery_summary,
        mutation_input: input,
        generation,
        pool,
        selection,
        selection_summary,
        selected,
        promotion,
        promotion_summary,
        input_file_sha: SHA_C,
        pool_file_sha: SHA_D,
        selected_file_sha: SHA_E,
    }
}
