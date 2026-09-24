//! Artifact types, the machine-readable design authority, summaries,
//! verifiers, and deterministic network-free synthetic datasets for
//! Experiment 0014 (the frozen end-to-end self-evolution
//! confirmation).
//!
//! Three artifact families, three disjoint task splits:
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
//! Every episode count, repetition count, gate threshold, condition
//! set, temperature, and kernel limit is DERIVED from the single
//! machine-readable design authority `design.json` (the `Design`
//! manifest below). There are no second, independently hard-coded
//! copies of these values in Rust. The only registered constants are
//! the frozen carried-over family stress profile and the structural
//! registry shape (12 tasks: 3/3/6), both of which are separately
//! verified against the frozen on-disk files.

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
use crate::protocol::{Message, sha256_hex};
use crate::selection::{self, CandidateTally, SelectionOutcome};
use crate::stats;
use crate::tools::{FaultMode, ToolExecutor, WriteAttempt};

// ===========================================================================
// The single machine-readable design authority (design.json)
// ===========================================================================

/// One split's design block. All fields are required: serde derives
/// without `#[serde(default)]`, so a missing value is a hard error
/// (never silently defaulted).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesignDiscovery {
    pub repetitions_per_task: u32,
    pub min_valid_episodes: u32,
    pub min_incumbent_failures: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesignSelection {
    pub repetitions_per_task: u32,
    pub conditions: Vec<String>,
    pub min_common_valid_cells: u32,
    pub min_potential_information: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesignPromotion {
    pub repetitions_per_task: u32,
    pub min_valid_pairs: u32,
    pub min_potential_information: u32,
    pub min_actual_informative: u32,
    pub max_infrastructure_failure_rate: f64,
    pub alpha: f64,
}

/// The registered deadline contract (0014 single source of truth).
///
/// These three values are the ONLY deadline numbers in the experiment:
/// the kernel enforces them, the verifier re-derives them, and NO other
/// Rust constant may define a duplicate value (the preflight source
/// audit asserts the old duplicate constants are gone).
/// The registered audited-execution-channel block (0014, the
/// experiment's only semantic delta over 0013).
///
/// These are the agent/mutator-visible channel settings: the protocol
/// version, the REGISTERED EXPERIMENT ENDPOINT (the channel gateway —
/// the ONLY endpoint the model client may use), and the single allowed
/// gateway path. The upstream model URL is deliberately NOT a field of
/// this struct: only `src/channel.rs` (the gateway implementation)
/// reads `channel.upstream_endpoint` from the raw `design.json`, so
/// agent/mutator code can never obtain the physical upstream endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignChannel {
    /// The channel protocol version (bound into the channel manifest
    /// and the run manifest; part of the channel identity).
    pub protocol_version: String,
    /// The registered experiment endpoint: the local channel gateway.
    /// The model client is bound to this endpoint and nothing else.
    pub gateway_endpoint: String,
    /// The ONLY path the gateway forwards: the chat-completions path.
    /// No model-list endpoint, no health endpoint, no other path.
    pub allowed_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignDeadline {
    /// Productive-execution boundary: a model response that becomes
    /// available only after this monotonic elapsed time MUST NOT
    /// influence the trajectory (it is discarded; the episode is an
    /// infrastructure failure).
    pub episode_deadline_ms: u64,
    /// Maximum per-request transport timeout the kernel may register
    /// with the model client (the kernel always clamps to the
    /// remaining episode budget, which is smaller or equal).
    pub request_timeout_ceiling_ms: u64,
    /// Small observation/teardown tolerance (0.1667% of the deadline
    /// at the registered value): it permits a blocking transport
    /// return to land slightly after the productive deadline. It is
    /// NOT additional productive agent time, and a response landing
    /// in the tolerance window must still be discarded.
    pub deadline_return_tolerance_ms: u64,
}

/// The complete design manifest. This is the ONE source of truth for
/// every executable count, threshold, and condition set in the
/// experiment (single-source-of-truth rule: executable counts come
/// from `design.json`, never from a second Rust constant).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Design {
    pub experiment_id: String,
    pub run_id: String,
    pub candidate_count: u32,
    pub discovery: DesignDiscovery,
    pub selection: DesignSelection,
    pub promotion: DesignPromotion,
    pub agent_temperature: f64,
    pub mutator_temperature: f64,
    pub max_model_turns: u32,
    pub max_tool_calls: u32,
    /// The registered deadline contract: the single source of truth for
    /// the per-episode productive deadline, the request-timeout ceiling,
    /// and the return/teardown tolerance (0014 deadline-safety).
    pub deadline: DesignDeadline,
    /// The ONE registered model (spec §10). Agent execution and the
    /// mutation generator use the SAME underlying model; there is no
    /// fallback model. In 0014 the model client is bound to the
    /// REGISTERED EXPERIMENT ENDPOINT (the channel gateway, `channel`
    /// block); the physical upstream URL is a channel-internal value
    /// known only to `src/channel.rs`.
    pub model: String,
    /// The registered audited execution channel (0014). The upstream
    /// endpoint is NOT part of this struct (see `DesignChannel`).
    pub channel: DesignChannel,
}

impl Design {
    /// Load `design.json` and validate every field. A missing field is
    /// a deserialization error; an out-of-range or inconsistent field
    /// is a validation error. There is no silent default anywhere.
    pub fn load(path: &std::path::Path) -> Result<Design, String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read design manifest {}: {e}", path.display()))?;
        let design: Design =
            serde_json::from_str(&raw).map_err(|e| format!("design manifest invalid: {e}"))?;
        design.validate()?;
        Ok(design)
    }

    /// Validate field ranges and internal consistency.
    pub fn validate(&self) -> Result<(), String> {
        let mut errors: Vec<String> = Vec::new();
        if self.experiment_id.trim().is_empty() {
            errors.push("experiment_id must be non-empty".to_string());
        }
        if self.run_id.trim().is_empty() {
            errors.push("run_id must be non-empty".to_string());
        }
        if !(1..=8).contains(&self.candidate_count) {
            errors.push(format!(
                "candidate_count {} outside the sane range 1..=8",
                self.candidate_count
            ));
        }
        if !(0.0..=2.0).contains(&self.agent_temperature) || self.agent_temperature == 0.0 {
            errors.push(format!(
                "agent_temperature {} out of range (0, 2]",
                self.agent_temperature
            ));
        }
        if !(0.0..=2.0).contains(&self.mutator_temperature) || self.mutator_temperature == 0.0 {
            errors.push(format!(
                "mutator_temperature {} out of range (0, 2]",
                self.mutator_temperature
            ));
        }
        if self.max_model_turns == 0 || self.max_tool_calls == 0 {
            errors.push("max_model_turns / max_tool_calls must be positive".to_string());
        }
        // Deadline contract.
        let d = &self.deadline;
        if d.episode_deadline_ms == 0 {
            errors.push("deadline.episode_deadline_ms must be positive".to_string());
        }
        if d.request_timeout_ceiling_ms == 0 {
            errors.push("deadline.request_timeout_ceiling_ms must be positive".to_string());
        }
        if d.deadline_return_tolerance_ms > d.episode_deadline_ms {
            errors.push(
                "deadline.deadline_return_tolerance_ms must not exceed episode_deadline_ms"
                    .to_string(),
            );
        }
        if self.model.trim().is_empty() {
            errors.push("model must be non-empty".to_string());
        }
        // Audited execution channel (0014).
        let c = &self.channel;
        if c.protocol_version.trim().is_empty() {
            errors.push("channel.protocol_version must be non-empty".to_string());
        }
        if !c.gateway_endpoint.starts_with("http://127.0.0.1:") {
            errors
                .push("channel.gateway_endpoint must be a local (127.0.0.1) endpoint".to_string());
        }
        if c.gateway_endpoint.trim().is_empty() {
            errors.push("channel.gateway_endpoint must be non-empty".to_string());
        }
        if c.allowed_path != "/v1/chat/completions" {
            errors.push(format!(
                "channel.allowed_path is {:?}, expected \"/v1/chat/completions\" (the only forwardable path; no model-list / health endpoint)",
                c.allowed_path
            ));
        }
        // Discovery gates.
        let d = &self.discovery;
        if d.repetitions_per_task == 0 {
            errors.push("discovery.repetitions_per_task must be positive".to_string());
        }
        if d.min_valid_episodes == 0 {
            errors.push("discovery.min_valid_episodes must be positive".to_string());
        }
        if d.min_valid_episodes > d.repetitions_per_task * 100 {
            errors.push(
                "discovery.min_valid_episodes unreasonably large for the repetition budget"
                    .to_string(),
            );
        }
        // Selection gates + condition set.
        let s = &self.selection;
        if s.repetitions_per_task == 0 {
            errors.push("selection.repetitions_per_task must be positive".to_string());
        }
        if s.conditions.is_empty() {
            errors.push("selection.conditions must be non-empty".to_string());
        } else {
            if s.conditions.len() != (self.candidate_count + 1) as usize {
                errors.push(format!(
                    "selection.conditions has {} entries, expected candidate_count+1 = {}",
                    s.conditions.len(),
                    self.candidate_count + 1
                ));
            }
            if s.conditions.first().map(String::as_str) != Some("G0") {
                errors.push("selection.conditions[0] must be \"G0\"".to_string());
            }
            for i in 0..s.conditions.len() {
                let want = if i == 0 {
                    "G0".to_string()
                } else {
                    format!("C{}", i)
                };
                if s.conditions[i] != want {
                    errors.push(format!(
                        "selection.conditions[{i}] is {:?}, expected {want:?}",
                        s.conditions[i]
                    ));
                }
            }
        }
        if s.min_common_valid_cells == 0 {
            errors.push("selection.min_common_valid_cells must be positive".to_string());
        }
        if s.min_potential_information == 0 {
            errors.push("selection.min_potential_information must be positive".to_string());
        }
        // Promotion gates.
        let p = &self.promotion;
        if p.repetitions_per_task == 0 {
            errors.push("promotion.repetitions_per_task must be positive".to_string());
        }
        if p.min_valid_pairs == 0 {
            errors.push("promotion.min_valid_pairs must be positive".to_string());
        }
        if p.min_potential_information == 0 {
            errors.push("promotion.min_potential_information must be positive".to_string());
        }
        if p.min_actual_informative == 0 {
            errors.push("promotion.min_actual_informative must be positive".to_string());
        }
        if !(0.0..=1.0).contains(&p.max_infrastructure_failure_rate) {
            errors.push("promotion.max_infrastructure_failure_rate outside [0, 1]".to_string());
        }
        if !(0.0..=1.0).contains(&p.alpha) || p.alpha == 0.0 {
            errors.push("promotion.alpha outside (0, 1]".to_string());
        }
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        Ok(())
    }

    // --- derived executable values (single source of truth) ------------

    pub fn discovery_repetitions(&self) -> u32 {
        self.discovery.repetitions_per_task
    }

    pub fn selection_repetitions(&self) -> u32 {
        self.selection.repetitions_per_task
    }

    pub fn promotion_repetitions(&self) -> u32 {
        self.promotion.repetitions_per_task
    }

    /// The harness-assigned candidate IDs C1..C{candidate_count}.
    pub fn candidate_ids(&self) -> Vec<String> {
        (1..=self.candidate_count)
            .map(|i| format!("C{i}"))
            .collect()
    }

    /// Number of registered tasks in one split.
    fn split_count(&self, registry: &[TaskSpec], split: TaskSplit) -> u32 {
        registry.iter().filter(|t| t.split == split).count() as u32
    }

    /// Discovery: tasks × reps.
    pub fn discovery_episodes(&self, registry: &[TaskSpec]) -> u32 {
        self.split_count(registry, TaskSplit::Discovery) * self.discovery_repetitions()
    }

    /// Selection comparison cells: tasks × reps (one cell = one
    /// (task, repetition) tuple run by ALL conditions).
    pub fn selection_cells(&self, registry: &[TaskSpec]) -> u32 {
        self.split_count(registry, TaskSplit::Selection) * self.selection_repetitions()
    }

    /// Selection: cells × conditions.
    pub fn selection_episodes(&self, registry: &[TaskSpec]) -> u32 {
        self.selection_cells(registry) * self.selection.conditions.len() as u32
    }

    /// Promotion pairs: tasks × reps.
    pub fn promotion_pairs(&self, registry: &[TaskSpec]) -> u32 {
        self.split_count(registry, TaskSplit::Promotion) * self.promotion_repetitions()
    }

    /// Promotion: pairs × 2 conditions.
    pub fn promotion_episodes(&self, registry: &[TaskSpec]) -> u32 {
        self.promotion_pairs(registry) * 2
    }

    /// Largest infrastructure-failure count within the registered
    /// failure rate over the promotion population
    /// (floor(rate × episodes); 9 for 0.10 × 96).
    pub fn max_promotion_infra_failures(&self, registry: &[TaskSpec]) -> u32 {
        let episodes = self.promotion_episodes(registry) as f64;
        (self.promotion.max_infrastructure_failure_rate * episodes).floor() as u32
    }

    /// The condition scheduled at (repetition, position) by the exact
    /// cyclic order: rep 1 → G0 C1 C2 C3 C4; each subsequent rep
    /// rotates left; reps beyond `conditions.len()` repeat the cycle.
    /// Every condition appears at every position equally often when the
    /// repetition count is a multiple of the condition count.
    ///
    /// A position beyond the registered condition count is `None` (a
    /// schedule missing the fifth position is REJECTED, not silently
    /// wrapped — the 0014 registered schedule has five conditions, so
    /// positions 1..=5 all exist).
    pub fn selection_condition_at(&self, repetition: u32, position: u32) -> Option<String> {
        if repetition == 0 || position == 0 {
            return None;
        }
        let n = self.selection.conditions.len();
        if n == 0 || position as usize > n {
            return None;
        }
        let r = (repetition - 1) % (n as u32);
        let idx = (r + position - 1) as usize % n;
        self.selection.conditions.get(idx).cloned()
    }

    /// The registered alternating promotion condition order: odd reps
    /// run G0 → selected, even reps run selected → G0. Exactly
    /// balanced: each condition-position cell holds pairs/2 episodes.
    pub fn promotion_condition_order(&self, repetition: u32, selected_id: &str) -> [String; 2] {
        if repetition % 2 == 1 {
            ["G0".to_string(), selected_id.to_string()]
        } else {
            [selected_id.to_string(), "G0".to_string()]
        }
    }
}

/// The run-identity provenance block shared by all stage records and
/// summaries: which run this evidence belongs to, and which frozen
/// design it was produced under.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunIdentity {
    /// The registered run identity (design `run_id`), e.g. "0014-r1".
    pub run_id: String,
    /// Byte hash of the committed `run-manifest.json`.
    pub run_manifest_sha256: String,
    /// The combined frozen-design hash from the run manifest.
    pub frozen_design_sha256: String,
}

// ===========================================================================
// Frozen carried-over family stress profile (registered constant,
// separately verified against the frozen stress-profile.json file)
// ===========================================================================

/// The frozen carried-over family-level stress profile (spec §35): the
/// Experiment 0009 supported result, carried over unchanged. `(family,
/// stress id, drop count)`.
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

// ===========================================================================
// Termination / failure classification (carried over from 0006–0010)
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
    /// A model response that became available only AFTER the registered
    /// productive episode deadline. The response is discarded (it must
    /// not produce an assistant message, a tool call, or a final
    /// answer); the request is a valid, contract-conformant deadline
    /// crossing, i.e. an ordinary infrastructure failure — NOT a
    /// protocol integrity violation (the verifier checks the request-
    /// level evidence, not aggregate wall time).
    EpisodeDeadlineExceeded,
    /// The episode deadline elapsed before another model request could
    /// even be started. No request is made and no request record is
    /// emitted. Valid infrastructure failure.
    EpisodeDeadlineReached,
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
            TerminationReason::EpisodeDeadlineExceeded => "episode_deadline_exceeded",
            TerminationReason::EpisodeDeadlineReached => "episode_deadline_reached",
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
            TerminationReason::MalformedResponse
                | TerminationReason::HttpError
                | TerminationReason::EpisodeDeadlineExceeded
                | TerminationReason::EpisodeDeadlineReached
        )
    }

    pub fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(Value::String(s.to_string())).ok()
    }
}

// ===========================================================================
// Shared per-episode execution records (carried over from 0006–0010)
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
    // --- 0014 request-level deadline evidence (mandatory on every real
    // agent request; monotonic milliseconds relative to the episode
    // start, never wall-clock timestamps) ---
    /// Monotonic elapsed ms at the instant immediately before the model
    /// request was started. Must be strictly < the registered episode
    /// deadline (a request whose start meets or exceeds the deadline
    /// must never be made).
    pub request_started_elapsed_ms: u64,
    /// The exact integer timeout (ms) registered with the model client
    /// for this request: `min(remaining_budget, ceiling)`, always > 0
    /// and never larger than `deadline - start`.
    pub requested_timeout_ms: u64,
    /// Monotonic elapsed ms at the instant the blocking model call
    /// returned (success or transport failure). Must be within
    /// `deadline + return_tolerance`.
    pub request_finished_elapsed_ms: u64,
    /// Structured transport-timeout classification (supporting
    /// diagnostic evidence; formal deadline validity comes from
    /// start/timeout/finish/acceptance, not from this flag alone).
    pub timed_out: bool,
    // --- 0014 audited-execution-channel evidence (mandatory on every
    // real request; empty strings in synthetic/network-free records) ---
    /// The channel-issued request identity, allocated by the experiment
    /// runtime (never by the model): `0014-r1-<STAGE>-NNNNNN` with a
    /// stage-local sequence starting at 1.
    pub channel_request_id: String,
    /// SHA-256 of the exact serialized request body bytes the channel
    /// gateway received; the one-to-one reconciliation key with the
    /// channel ledger.
    pub request_body_sha256: String,
    /// True only when the response was usable AND returned at or
    /// before the productive episode deadline, and was actually
    /// passed into the agent kernel. A response that returns after
    /// the deadline (even inside the return tolerance) is discarded
    /// and must be recorded as `response_accepted = false`.
    pub response_accepted: bool,
}

/// The per-request audited-channel binding (0014): the channel-issued
/// request identity plus the hash of the exact body bytes that
/// traversed the registered gateway. The kernel stamps it into every
/// [`ModelRequestRecord`]; the runtime (via `src/channel.rs`) produces
/// it. Synthetic (network-free) records carry an empty binding.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelBinding {
    pub request_id: String,
    pub body_sha256: String,
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
///
/// Every real episode records: experiment_id, run identity, the
/// code-under-test commit, the run-manifest hash, the frozen-design
/// hash, the model, and the temperature (spec §46).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordProvenance {
    pub model: String,
    pub temperature: f64,
    pub redacted_endpoint: String,
    pub code_under_test_commit: String,
    pub run_id: String,
    pub run_manifest_sha256: String,
    pub frozen_design_sha256: String,
    pub incumbent_prompt_sha256: String,
    pub mutator_prompt_sha256: String,
    pub task_registry_sha256: String,
    pub stress_profile_sha256: String,
}

/// One persisted discovery trajectory: an incumbent (G0) episode on one
/// of the three discovery tasks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryRecord {
    pub experiment_id: String,
    pub phase: String, // "discovery"
    /// One episode's identity within the run (sequence-labeled).
    pub episode_id: String,
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
    pub episode_id: String,
    pub sequence: u32,
    pub family: String,
    pub task_id: String,
    pub split: String, // "selection"
    pub stress_level: String,
    pub drop_count: u32,
    pub fault_key: String,
    pub repetition: u32,
    pub condition_position: u32,
    /// "G0" | "C1".."C4" (from the design condition set).
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
    pub episode_id: String,
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
    /// Aggregate wall time is DIAGNOSTIC / cost evidence only: it is
    /// never a deadline-integrity input (the 0012 aggregate
    /// wall-time > deadline rule is deliberately removed).
    pub wall_time_ms: u64,
    pub model_wait_time_ms: u64,
    pub tool_execution_time_us: u64,
    // --- 0014 deadline summary metrics (re-derived from the
    // request-level evidence; diagnostic reporting, not gates) ---
    /// Episodes whose termination is a registered deadline
    /// infrastructure reason (deadline exceeded / reached) or a
    /// transport-level request failure (including timeouts).
    pub deadline_infrastructure_failures: u32,
    /// Model responses that became available after the productive
    /// episode deadline and were discarded (counted per request).
    pub deadline_crossing_discarded_responses: u32,
    /// Max observed return overshoot past the productive deadline (ms;
    /// 0 when no recorded response crossed the deadline).
    pub max_return_overshoot_ms: u64,
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

/// One row of the condition × condition-position cache audit.
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
    fn termination_reason(&self) -> &str;
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
    fn termination_reason(&self) -> &str {
        &self.termination_reason
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
    fn termination_reason(&self) -> &str {
        &self.termination_reason
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
    fn termination_reason(&self) -> &str {
        &self.termination_reason
    }
}

/// The registered productive episode deadline (design authority) —
/// the sole deadline input to cost/deadline aggregation.
pub fn registered_deadline_ms(design: &Design) -> u64 {
    design.deadline.episode_deadline_ms
}

fn sum_usage<T: EpisodeLike>(pop: &[T], deadline_ms: u64) -> CostBlock {
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
        deadline_infrastructure_failures: 0,
        deadline_crossing_discarded_responses: 0,
        max_return_overshoot_ms: 0,
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
        if matches!(
            TerminationReason::parse(r.termination_reason()),
            Some(
                TerminationReason::EpisodeDeadlineExceeded
                    | TerminationReason::EpisodeDeadlineReached
                    | TerminationReason::HttpError
            )
        ) {
            cb.deadline_infrastructure_failures += 1;
        }
        for q in r.model_requests() {
            if q.request_finished_elapsed_ms > deadline_ms {
                cb.deadline_crossing_discarded_responses += 1;
                let overshoot = q.request_finished_elapsed_ms - deadline_ms;
                if overshoot > cb.max_return_overshoot_ms {
                    cb.max_return_overshoot_ms = overshoot;
                }
            }
        }
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
// Selected-candidate freeze artifact
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

/// The frozen selected candidate. Contains NO promotion outcomes.
/// Once committed, the content must never change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectedCandidate {
    pub experiment_id: String,
    pub run_id: String,
    pub run_manifest_sha256: String,
    pub frozen_design_sha256: String,
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

/// Pre-registered discovery gates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryGates {
    pub artifact_complete: bool,
    pub mutation_input_whitelist: bool,
    pub valid_episodes_sufficient: bool,
    /// The mutator must receive MEANINGFUL failure evidence: the
    /// registered minimum of incumbent failures among valid episodes.
    pub incumbent_failures_sufficient: bool,
}

impl DiscoveryGates {
    pub fn all(&self) -> bool {
        self.artifact_complete
            && self.mutation_input_whitelist
            && self.valid_episodes_sufficient
            && self.incumbent_failures_sufficient
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
    pub run_id: String,
    pub run_manifest_sha256: String,
    pub frozen_design_sha256: String,
    pub incumbent_prompt_sha256: String,
    pub task_registry_sha256: String,
    pub stress_profile_sha256: String,
    pub episodes_total: u32,
    pub valid_episodes: u32,
    pub infrastructure_failures: u32,
    pub agent_failures: u32,
    pub oracle_successes: u32,
    /// G0 failures among VALID episodes (agent failures) — the
    /// evidence the mutation generator is allowed to see.
    pub incumbent_failures: u32,
    /// Byte hash of the frozen `mutation-input.json`.
    pub mutation_input_sha256: String,
    pub gates: DiscoveryGates,
    pub per_task: Vec<DiscoveryTaskRow>,
    pub cost: CostBlock,
    pub proceed_to_mutation_generation: bool,
}

/// Pre-registered selection gates.
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
    pub run_id: String,
    pub run_manifest_sha256: String,
    pub frozen_design_sha256: String,
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

/// Pre-registered promotion gates.
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
    pub run_id: String,
    pub run_manifest_sha256: String,
    pub frozen_design_sha256: String,
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
// Pure summary computation (shared by runner and verifier; every count
// and gate derives from the Design manifest)
// ===========================================================================

/// Compute the discovery summary from its records.
#[allow(clippy::too_many_arguments)] // the frozen provenance hashes are part of the artifact contract
pub fn compute_discovery_summary(
    records: &[DiscoveryRecord],
    design: &Design,
    registry: &[TaskSpec],
    run: &RunIdentity,
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
    let incumbent_failures = valid - success;
    let per_task = registry
        .iter()
        .filter(|t| t.split == TaskSplit::Discovery)
        .map(|t| {
            let rows: Vec<&DiscoveryRecord> =
                records.iter().filter(|r| r.task_id == t.id).collect();
            DiscoveryTaskRow {
                task_id: t.id.clone(),
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
    let expected = design.discovery_episodes(registry);
    let complete = records.len() as u32 == expected;
    let valid_sufficient = valid >= design.discovery.min_valid_episodes;
    let failures_sufficient = incumbent_failures >= design.discovery.min_incumbent_failures;
    let gates = DiscoveryGates {
        artifact_complete: complete,
        mutation_input_whitelist: true, // file-level check in the verifier
        valid_episodes_sufficient: valid_sufficient,
        incumbent_failures_sufficient: failures_sufficient,
    };
    DiscoverySummary {
        experiment_id: design.experiment_id.clone(),
        phase: "discovery".to_string(),
        model: model.to_string(),
        redacted_endpoint: endpoint.to_string(),
        code_under_test_commit: commit.to_string(),
        run_id: run.run_id.clone(),
        run_manifest_sha256: run.run_manifest_sha256.clone(),
        frozen_design_sha256: run.frozen_design_sha256.clone(),
        incumbent_prompt_sha256: incumbent_sha.to_string(),
        task_registry_sha256: registry_sha.to_string(),
        stress_profile_sha256: profile_sha.to_string(),
        episodes_total: records.len() as u32,
        valid_episodes: valid,
        infrastructure_failures: infra,
        agent_failures: agent,
        oracle_successes: success,
        incumbent_failures,
        mutation_input_sha256: input_file_sha.to_string(),
        gates,
        per_task,
        cost: sum_usage(records, registered_deadline_ms(design)),
        proceed_to_mutation_generation: gates.all(),
    }
}

/// Compute the selection summary from its records + frozen pool.
#[allow(clippy::too_many_arguments)] // the frozen provenance hashes are part of the artifact contract
pub fn compute_selection_summary(
    records: &[SelectionRecord],
    design: &Design,
    registry: &[TaskSpec],
    run: &RunIdentity,
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
    let conditions = &design.selection.conditions;
    let mut infra = 0u32;
    let mut order_ok = true;
    let mut position_counts: BTreeMap<(String, u32), u32> = BTreeMap::new();
    for r in records {
        *position_counts
            .entry((r.condition.clone(), r.condition_position))
            .or_insert(0) += 1;
        let want = design.selection_condition_at(r.repetition, r.condition_position);
        if !conditions.contains(&r.condition.clone())
            || want.as_deref() != Some(r.condition.as_str())
        {
            order_ok = false;
        }
        if r.infrastructure_failure.is_some() {
            infra += 1;
        }
    }
    let sel_tasks: Vec<&TaskSpec> = registry
        .iter()
        .filter(|t| t.split == TaskSplit::Selection)
        .collect();
    let expected_per_cell =
        (sel_tasks.len() as u32 * design.selection_repetitions()) / conditions.len() as u32;
    let balanced = position_counts.len() == conditions.len() * conditions.len()
        && position_counts.values().all(|&n| n == expected_per_cell);
    let count_ok = records.len() as u32 == design.selection_episodes(registry);

    // Cell bookkeeping: a cell is common-valid iff NONE of the conditions
    // has an infrastructure failure in it.
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
    for task in &sel_tasks {
        for rep in 1..=design.selection_repetitions() {
            let (present, failed) = cells
                .get(&(task.id.to_string(), rep))
                .copied()
                .unwrap_or((0, 0));
            cell_statuses.push(SelectionCellStatus {
                task_id: task.id.clone(),
                repetition: rep,
                common_valid: present == conditions.len() as u8 && failed == 0,
            });
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

    let common_valid_sufficient = common_valid >= design.selection.min_common_valid_cells;
    let info_sufficient = g0_failure >= design.selection.min_potential_information;
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

    let condition_cost = conditions
        .iter()
        .map(|id| {
            let pop: Vec<SelectionRecord> = records
                .iter()
                .filter(|r| r.condition == *id)
                .cloned()
                .collect();
            let all = sum_usage(&pop, registered_deadline_ms(design));
            let success: Vec<SelectionRecord> =
                pop.iter().filter(|r| r.oracle_success).cloned().collect();
            let failure: Vec<SelectionRecord> =
                pop.iter().filter(|r| !r.oracle_success).cloned().collect();
            ConditionSummary {
                condition: id.clone(),
                all,
                success: sum_usage(&success, registered_deadline_ms(design)),
                failure: sum_usage(&failure, registered_deadline_ms(design)),
                cache: cache_metrics(&all),
            }
        })
        .collect();
    let mut cache_audit = Vec::new();
    for id in conditions {
        for pos in 1..=conditions.len() as u32 {
            let pop: Vec<SelectionRecord> = records
                .iter()
                .filter(|r| r.condition.as_str() == id && r.condition_position == pos)
                .cloned()
                .collect();
            cache_audit.push(cache_row(
                &sum_usage(&pop, registered_deadline_ms(design)),
                id,
                pos,
            ));
        }
    }

    SelectionSummary {
        experiment_id: design.experiment_id.clone(),
        phase: "selection".to_string(),
        model: model.to_string(),
        redacted_endpoint: endpoint.to_string(),
        code_under_test_commit: commit.to_string(),
        run_id: run.run_id.clone(),
        run_manifest_sha256: run.run_manifest_sha256.clone(),
        frozen_design_sha256: run.frozen_design_sha256.clone(),
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
    design: &Design,
    registry: &[TaskSpec],
    run: &RunIdentity,
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
    for t in registry.iter().filter(|t| t.split == TaskSplit::Promotion) {
        for rep in 1..=design.promotion_repetitions() {
            let g0 = records
                .iter()
                .find(|r| r.task_id == t.id && r.repetition == rep && r.condition == "G0");
            let sel = records
                .iter()
                .find(|r| r.task_id == t.id && r.repetition == rep && r.condition == selected_id);
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
            // Selected-candidate perspective.
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
    let class = stats::classify_quality(wins as u64, losses as u64, design.promotion.alpha);
    let potential = g0_fail_valid;

    let complete = records.len() as u32 == design.promotion_episodes(registry);
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
            let want = design
                .promotion_condition_order(r.repetition, selected_id)
                .get((r.condition_position - 1) as usize)
                .cloned();
            if want.as_deref() != Some(r.condition.as_str()) {
                ok = false;
            }
        }
        seen.values().all(|&n| n == 1) && ok
    };
    let infra_ok = records.is_empty()
        || infra as f64
            <= design.promotion.max_infrastructure_failure_rate * records.len() as f64 + 1e-9;
    let gates = PromotionGates {
        selected_candidate_frozen: true, // file-level check in the verifier
        artifact_complete: complete && order_ok,
        only_registered_conditions: known_conditions,
        infrastructure_within_threshold: infra_ok,
        valid_pairs_sufficient: valid >= design.promotion.min_valid_pairs,
        potential_information_sufficient: potential >= design.promotion.min_potential_information,
        actual_informative_sufficient: informative >= design.promotion.min_actual_informative,
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

    let per_task = registry
        .iter()
        .filter(|t| t.split == TaskSplit::Promotion)
        .map(|t| {
            let pop: Vec<&PromotionRecord> = records.iter().filter(|r| r.task_id == t.id).collect();
            let g0p: Vec<&&PromotionRecord> =
                pop.iter().filter(|r| r.condition == "G0").collect();
            let selp: Vec<&&PromotionRecord> =
                pop.iter().filter(|r| r.condition == selected_id).collect();
            let mut pairs = 0u32;
            let mut valid_t = 0u32;
            for rep in 1..=design.promotion_repetitions() {
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
                task_id: t.id.clone(),
                family: pop.first().map(|r| r.family.clone()).unwrap_or_default(),
                stress_level: pop.first().map(|r| r.stress_level.clone()).unwrap_or_default(),
                pairs,
                valid: valid_t,
                g0_successes: g0p.iter().filter(|r| r.oracle_success).count() as u32,
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
            let all = sum_usage(&pop, registered_deadline_ms(design));
            let success: Vec<PromotionRecord> =
                pop.iter().filter(|r| r.oracle_success).cloned().collect();
            let failure: Vec<PromotionRecord> =
                pop.iter().filter(|r| !r.oracle_success).cloned().collect();
            ConditionSummary {
                condition: id.clone(),
                all,
                success: sum_usage(&success, registered_deadline_ms(design)),
                failure: sum_usage(&failure, registered_deadline_ms(design)),
                cache: cache_metrics(&all),
            }
        })
        .collect();
    let mut cache_audit = Vec::new();
    for id in &conditions {
        for pos in 1..=2u32 {
            let pop: Vec<PromotionRecord> = records
                .iter()
                .filter(|r| &r.condition == id && r.condition_position == pos)
                .cloned()
                .collect();
            cache_audit.push(cache_row(
                &sum_usage(&pop, registered_deadline_ms(design)),
                id,
                pos,
            ));
        }
    }

    PromotionSummary {
        experiment_id: design.experiment_id.clone(),
        phase: "promotion".to_string(),
        model: model.to_string(),
        redacted_endpoint: endpoint.to_string(),
        code_under_test_commit: commit.to_string(),
        run_id: run.run_id.clone(),
        run_manifest_sha256: run.run_manifest_sha256.clone(),
        frozen_design_sha256: run.frozen_design_sha256.clone(),
        incumbent_prompt_sha256: incumbent_sha.to_string(),
        task_registry_sha256: registry_sha.to_string(),
        stress_profile_sha256: profile_sha.to_string(),
        candidate_pool_sha256: pool_file_sha.to_string(),
        selected_candidate_sha256: selected_file_sha.to_string(),
        mutation_input_sha256: input_file_sha.to_string(),
        selected_candidate_id: selected_id.to_string(),
        episodes_total: records.len() as u32,
        infrastructure_failures: infra,
        expected_pairs: design.promotion_pairs(registry),
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

/// Load + validate the frozen stress profile (spec §35: the validated
/// 0009 family stresses, carried over unchanged — no calibration).
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
    /// The design manifest: every executable count and gate the
    /// verifiers check comes from here.
    pub design: Design,
}

impl VerifierContext {
    pub fn new(
        incumbent_text: String,
        incumbent_file_sha: String,
        mutator_file_sha: String,
        registry: Vec<TaskSpec>,
        profile: Value,
        design: Design,
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
            design,
        }
    }

    pub fn task(&self, id: &str) -> Option<&TaskSpec> {
        self.registry.iter().find(|t| t.id == id)
    }
}

/// Structural registry checks. Task IDs and state literals come from
/// the frozen `tasks.json` (a frozen design file guarded by the source
/// freeze); records bind themselves to the registry through the
/// provenance hash and the per-record state/prompt checks in
/// `verify_record_core`, so the registry cannot be swapped out
/// silently.
pub fn registry_ok(ctx: &VerifierContext, errors: &mut Vec<String>) {
    // The stress profile file must still be the frozen 0009 profile
    // (the constants alone would let a swapped file pass).
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
    // Registered registry shape: 12 tasks, 3/3/6 splits, unique IDs.
    if ctx.registry.len() != 12 {
        errors.push(format!(
            "registry must contain 12 tasks, found {}",
            ctx.registry.len()
        ));
        return;
    }
    let mut seen_ids = BTreeMap::new();
    for t in &ctx.registry {
        *seen_ids.entry(t.id.clone()).or_insert(0) += 1;
    }
    for (id, n) in &seen_ids {
        if *n != 1 {
            errors.push(format!("registry task ID {id} appears {n} times"));
        }
    }
    for split in [
        TaskSplit::Discovery,
        TaskSplit::Selection,
        TaskSplit::Promotion,
    ] {
        let (want, label) = match split {
            TaskSplit::Discovery => (3u32, "discovery"),
            TaskSplit::Selection => (3u32, "selection"),
            TaskSplit::Promotion => (6u32, "promotion"),
        };
        let got = ctx.registry.iter().filter(|t| t.split == split).count() as u32;
        if got != want {
            errors.push(format!("registry has {got} {label} tasks, expected {want}"));
        }
    }
    // ID prefix consistency and registered family membership.
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
        if !FROZEN_PROFILE.iter().any(|(f, _, _)| *f == t.family) {
            errors.push(format!(
                "task {} family {:?} is not a registered family",
                t.id, t.family
            ));
        }
    }
    // State literals must be split-disjoint: a literal from one split
    // must not appear in another split's states (task leakage would
    // otherwise be invisible in the raw files).
    let mut literals: BTreeMap<String, TaskSplit> = BTreeMap::new();
    for t in &ctx.registry {
        for field in [&t.initial_state, &t.target_state] {
            if let Some(obj) = field.as_object() {
                for v in obj.values() {
                    if let Some(s) = v.as_str() {
                        match literals.entry(s.to_string()) {
                            std::collections::btree_map::Entry::Vacant(e) => {
                                e.insert(t.split);
                            }
                            std::collections::btree_map::Entry::Occupied(e) => {
                                if *e.get() != t.split {
                                    errors.push(format!(
                                        "state literal {s} is shared across splits ({:?} and {:?})",
                                        e.get(),
                                        t.split
                                    ));
                                }
                            }
                        }
                    }
                }
            }
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
    fn timing(&self) -> &Timing;
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
    fn timing(&self) -> &Timing {
        &self.timing
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
    fn timing(&self) -> &Timing {
        &self.timing
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
    fn timing(&self) -> &Timing {
        &self.timing
    }
}

/// The 0014 registered request-level deadline contract (spec §28).
///
/// For EVERY recorded agent model request of one episode, the request
/// evidence MUST satisfy (all in monotonic ms against the episode
/// start, from the design-registered deadline contract):
///
/// ```text
/// start < deadline
/// timeout > 0
/// timeout <= ceiling
/// timeout <= deadline - start
/// finish >= start
/// finish <= deadline + return_tolerance
/// if finish > deadline:
///     response_accepted == false
///     episode termination is an infrastructure failure
///     no subsequent request exists
///     no tool call or final answer is derived from that request
/// ```
///
/// A contract-conformant crossing (finish in (deadline,
/// deadline + tolerance]) is a VALID infrastructure failure — the
/// response was discarded. A return beyond the tolerance (e.g.
/// deadline + tolerance + 1) is a hard integrity violation.
/// Aggregate wall time is deliberately NOT checked against the
/// deadline (the 0012 rule is removed; the request-level contract is
/// authoritative).
pub fn deadline_evidence_violations(
    design: &Design,
    who: &str,
    tr: TerminationReason,
    model_requests: &[ModelRequestRecord],
    final_answer: &Option<String>,
    executed: &[ExecutedToolCall],
) -> Vec<String> {
    let mut errors = Vec::new();
    let d = &design.deadline;
    let deadline = d.episode_deadline_ms;
    let ceiling = d.request_timeout_ceiling_ms;
    let tolerance = d.deadline_return_tolerance_ms;
    for (i, q) in model_requests.iter().enumerate() {
        let label = format!("{who} request {}", q.turn);
        if q.request_started_elapsed_ms >= deadline {
            errors.push(format!(
                "{label}: request started at the productive deadline or later ({}) — a request may never START after the deadline",
                q.request_started_elapsed_ms
            ));
        }
        if q.requested_timeout_ms == 0 {
            errors.push(format!("{label}: registered timeout is zero"));
        }
        if q.requested_timeout_ms > ceiling {
            errors.push(format!(
                "{label}: requested timeout {} exceeds the registered ceiling {}",
                q.requested_timeout_ms, ceiling
            ));
        }
        let remaining = deadline.saturating_sub(q.request_started_elapsed_ms);
        if q.requested_timeout_ms > remaining {
            errors.push(format!(
                "{label}: requested timeout {} exceeds the remaining episode budget {} (deadline {} - start {})",
                q.requested_timeout_ms, remaining, deadline, q.request_started_elapsed_ms
            ));
        }
        if q.request_finished_elapsed_ms < q.request_started_elapsed_ms {
            errors.push(format!(
                "{label}: finish ({}) precedes start ({})",
                q.request_finished_elapsed_ms, q.request_started_elapsed_ms
            ));
        }
        if q.request_finished_elapsed_ms > deadline + tolerance {
            errors.push(format!(
                "{label}: request returned at {} ms — beyond deadline {} + return tolerance {} (hard timing-integrity violation)",
                q.request_finished_elapsed_ms, deadline, tolerance
            ));
        }
        if q.request_finished_elapsed_ms > deadline {
            if q.response_accepted {
                errors.push(format!(
                    "{label}: response returned after the productive deadline ({deadline} ms) but was recorded as ACCEPTED — a late response must be discarded",
                ));
            }
            if i + 1 < model_requests.len() {
                errors.push(format!(
                    "{label}: crossed the episode deadline but a subsequent request (turn {}) exists — no request may follow a deadline crossing",
                    model_requests[i + 1].turn
                ));
            }
            if !tr.is_infrastructure() {
                errors.push(format!(
                    "{label}: crossed the episode deadline but the episode termination {:?} is not an infrastructure failure",
                    tr
                ));
            }
            if final_answer.is_some() {
                errors.push(format!(
                    "{label}: crossed the episode deadline but a final answer is recorded — a discarded response must not produce a final answer"
                ));
            }
            if executed.iter().any(|c| c.turn > q.turn) {
                errors.push(format!(
                    "{label}: crossed the episode deadline but a later tool call is recorded — a discarded response must not execute a tool call"
                ));
            }
        }
    }
    errors
}

/// Verify kernel / fault / oracle / usage invariants for ONE record
/// against its registered task + frozen stress. Shared by the three
/// phase verifiers. Kernel limits come from the design manifest.
fn verify_record_core<R: RecordCoreLike>(
    errors: &mut Vec<String>,
    who: &str,
    design: &Design,
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
    // A request that was made always leaves a record; the exception is
    // `episode_deadline_reached`, where the deadline elapsed BEFORE a
    // new request could start, so no request record is emitted.
    let infra_request = tr.is_infrastructure() && tr != TerminationReason::EpisodeDeadlineReached;
    let expected_len = record.model_turn_count() + if infra_request { 1 } else { 0 };
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
    if record.model_turn_count() > design.max_model_turns {
        errors.push(format!(
            "{who}: model_turn_count {} exceeds the registered {}-turn kernel limit",
            record.model_turn_count(),
            design.max_model_turns
        ));
    }
    // 0014: the request-level deadline contract is the authoritative
    // deadline integrity check. The 0012 aggregate wall-time rule is
    // deliberately removed: a contract-conformant crossing whose
    // aggregate wall time slightly exceeds the deadline (within the
    // registered return tolerance) must NOT be a violation. Aggregate
    // wall time remains diagnostic/cost evidence only.
    errors.extend(deadline_evidence_violations(
        design,
        who,
        tr,
        record.model_requests(),
        record.final_answer(),
        record.executed(),
    ));
    // Loose record-corruption sanity bound (registered values only, not
    // a second deadline definition): an episode wall time beyond
    // deadline + tolerance + another full deadline indicates a corrupt
    // record, since a conformant crossing can overshoot by at most the
    // tolerance.
    if record.timing().wall_time_ms
        > design.deadline.episode_deadline_ms
            + design.deadline.deadline_return_tolerance_ms
            + design.deadline.episode_deadline_ms
    {
        errors.push(format!(
            "{who}: aggregate wall time {} ms implausibly exceeds the registered deadline contract",
            record.timing().wall_time_ms
        ));
    }
    if record.tool_call_count() > design.max_tool_calls {
        errors.push(format!(
            "{who}: tool_call_count {} exceeds the registered {}-call kernel limit",
            record.tool_call_count(),
            design.max_tool_calls
        ));
    }
    if record.tool_call_count() as usize != record.executed().len() {
        errors.push(format!(
            "{who}: tool_call_count {} != {} executed calls",
            record.tool_call_count(),
            record.executed().len()
        ));
    }
    // Fault audit + replay: the hidden audit must be exactly what the
    // registered fault schedule dictates, and the replay of applied
    // writes must reproduce the final state.
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
    // Executed-call results must be exactly what the true state says.
    let mut replay = task.initial_state.clone();
    let mut write_idx = 0usize;
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
        if c.tool_name == "state_write" {
            let applied = record
                .write_attempts()
                .get(write_idx)
                .is_some_and(|w| w.applied);
            write_idx += 1;
            if applied {
                let k = c.arguments.get("key").and_then(Value::as_str).unwrap_or("");
                let v = c
                    .arguments
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if let Some(obj) = replay.as_object_mut() {
                    obj.insert(k.to_string(), Value::String(v.to_string()));
                }
            }
        }
    }
    if write_idx != record.write_attempts().len() {
        errors.push(format!(
            "{who}: {} state_write calls vs {} audit entries (positions diverge)",
            write_idx,
            record.write_attempts().len()
        ));
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

/// Verify the discovery artifacts (spec §84/§78: tamper detection).
pub fn verify_discovery(
    records: &[DiscoveryRecord],
    summary: &Value,
    mutation_input: &Value,
    mutation_input_file_sha: &str,
    ctx: &VerifierContext,
) -> Vec<String> {
    let mut errors = Vec::new();
    registry_ok(ctx, &mut errors);
    let design = &ctx.design;
    let expected = design.discovery_episodes(&ctx.registry);
    if records.len() as u32 != expected {
        errors.push(format!(
            "discovery has {} episodes, expected {expected}",
            records.len()
        ));
    }
    let mut seen_cells: BTreeMap<(String, u32), u32> = BTreeMap::new();
    for r in records {
        let who = r.episode_id.clone();
        if r.experiment_id != design.experiment_id || r.phase != "discovery" {
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
        if (r.provenance.temperature - design.agent_temperature).abs() > 1e-9 {
            errors.push(format!(
                "{who}: agent temperature {} != design {}",
                r.provenance.temperature, design.agent_temperature
            ));
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
        verify_record_core(
            &mut errors,
            &who,
            design,
            task,
            count,
            &ctx.incumbent_text,
            r,
        );
    }
    for task in ctx
        .registry
        .iter()
        .filter(|t| t.split == TaskSplit::Discovery)
    {
        for rep in 1..=design.discovery_repetitions() {
            if seen_cells
                .get(&(task.id.to_string(), rep))
                .copied()
                .unwrap_or(0)
                != 1
            {
                errors.push(format!(
                    "discovery cell ({}, rep {rep}) does not appear exactly once",
                    task.id
                ));
            }
        }
    }
    if seen_cells.len() as u32 != expected {
        errors.push(format!(
            "discovery covers {} distinct cells, expected {expected}",
            seen_cells.len()
        ));
    }
    let valid = records
        .iter()
        .filter(|r| r.infrastructure_failure.is_none())
        .count() as u32;
    if valid < design.discovery.min_valid_episodes {
        errors.push(format!(
            "only {valid} valid discovery episodes (< {}) — mutation generation must not run",
            design.discovery.min_valid_episodes
        ));
    }
    let valid_failures = records
        .iter()
        .filter(|r| r.infrastructure_failure.is_none() && !r.oracle_success)
        .count() as u32;
    if valid_failures < design.discovery.min_incumbent_failures {
        errors.push(format!(
            "only {valid_failures} incumbent failures among valid episodes (< {}) — the mutator lacks meaningful failure evidence",
            design.discovery.min_incumbent_failures
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
    let run = RunIdentity {
        run_id: str_of("run_id"),
        run_manifest_sha256: str_of("run_manifest_sha256"),
        frozen_design_sha256: str_of("frozen_design_sha256"),
    };
    for r in records {
        if r.provenance.run_id != run.run_id
            || r.provenance.run_manifest_sha256 != run.run_manifest_sha256
            || r.provenance.frozen_design_sha256 != run.frozen_design_sha256
        {
            errors.push(format!(
                "{}: run-identity provenance diverges from the summary freeze",
                r.episode_id
            ));
        }
    }
    let recomputed = compute_discovery_summary(
        records,
        design,
        &ctx.registry,
        &run,
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
        if typed_input.experiment_id != design.experiment_id {
            errors.push(
                "mutation input experiment_id does not match the design manifest".to_string(),
            );
        }
        errors.extend(mutation_input_leak_errors(&typed_input, &ctx.registry));
    } else {
        errors.push("mutation input is not well-formed against the whitelist schema".to_string());
    }
    let expected_input =
        build_mutation_input(design, &ctx.incumbent_text, &ToolExecutor::specs(), records);
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

/// Verify the mutation-generation artifact + candidate pool.
pub fn verify_mutation(
    generation: &Value,
    pool: &CandidatePool,
    mutation_input: &Value,
    mutation_input_file_sha: &str,
    ctx: &VerifierContext,
) -> Vec<String> {
    let mut errors = Vec::new();
    registry_ok(ctx, &mut errors);
    let design = &ctx.design;

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
    if (generation
        .get("temperature")
        .and_then(Value::as_f64)
        .unwrap_or(f64::NAN)
        - design.mutator_temperature)
        .abs()
        > 1e-9
    {
        errors.push(format!(
            "generation temperature != design mutator_temperature {}",
            design.mutator_temperature
        ));
    }
    if generation
        .get("run_id")
        .and_then(Value::as_str)
        .map(|s| s.trim().is_empty())
        .unwrap_or(true)
    {
        errors.push(
            "generation artifact is missing its run identity (run_id / manifest / frozen-design provenance)".to_string(),
        );
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
        // or post-acceptance editing.
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
            mutation::validate_mutator_response(design, raw, &ctx.incumbent_text, &ctx.registry);
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
        design,
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

/// Verify the selection artifacts.
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
    let design = &ctx.design;
    let conditions = &design.selection.conditions;
    if records.len() as u32 != design.selection_episodes(&ctx.registry) {
        errors.push(format!(
            "selection has {} episodes, expected {}",
            records.len(),
            design.selection_episodes(&ctx.registry)
        ));
    }
    let mut position_counts: BTreeMap<(String, u32), u32> = BTreeMap::new();
    let mut seen: BTreeMap<(String, u32, String), u32> = BTreeMap::new();
    for r in records {
        let who = r.episode_id.clone();
        if r.experiment_id != design.experiment_id || r.phase != "selection" {
            errors.push(format!("{who}: wrong experiment id/phase"));
        }
        if (r.provenance.temperature - design.agent_temperature).abs() > 1e-9 {
            errors.push(format!(
                "{who}: temperature {} != design agent temperature {}",
                r.provenance.temperature, design.agent_temperature
            ));
        }
        *position_counts
            .entry((r.condition.clone(), r.condition_position))
            .or_insert(0) += 1;
        *seen
            .entry((r.task_id.clone(), r.repetition, r.condition.clone()))
            .or_insert(0) += 1;
        if !conditions.contains(&r.condition.clone()) {
            errors.push(format!("{who}: unregistered condition {:?}", r.condition));
            continue;
        }
        if design
            .selection_condition_at(r.repetition, r.condition_position)
            .as_deref()
            != Some(r.condition.as_str())
        {
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
        verify_record_core(&mut errors, &who, design, task, count, &expected_full, r);
    }
    let sel_count = ctx
        .registry
        .iter()
        .filter(|t| t.split == TaskSplit::Selection)
        .count() as u32;
    let expected_per_cell = sel_count * design.selection_repetitions() / conditions.len() as u32;
    for ((cond, pos), n) in &position_counts {
        if *n != expected_per_cell {
            errors.push(format!(
                "condition {cond} position {pos} has {n} episodes, expected {expected_per_cell} (registered cyclic balance)"
            ));
        }
    }
    if position_counts.len() != conditions.len() * conditions.len() {
        errors.push(format!(
            "selection covers {} condition×position cells, expected {}",
            position_counts.len(),
            conditions.len() * conditions.len()
        ));
    }
    for t in ctx
        .registry
        .iter()
        .filter(|t| t.split == TaskSplit::Selection)
    {
        for rep in 1..=design.selection_repetitions() {
            for c in conditions {
                if seen
                    .get(&(t.id.to_string(), rep, c.to_string()))
                    .copied()
                    .unwrap_or(0)
                    != 1
                {
                    errors.push(format!(
                        "selection cell ({}, rep {rep}, {c}) does not appear exactly once",
                        t.id
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
    let run = RunIdentity {
        run_id: str_of("run_id"),
        run_manifest_sha256: str_of("run_manifest_sha256"),
        frozen_design_sha256: str_of("frozen_design_sha256"),
    };
    for r in records {
        if r.provenance.run_id != run.run_id
            || r.provenance.run_manifest_sha256 != run.run_manifest_sha256
            || r.provenance.frozen_design_sha256 != run.frozen_design_sha256
        {
            errors.push(format!(
                "{}: run-identity provenance diverges from the summary freeze",
                r.episode_id
            ));
        }
    }
    let recomputed = compute_selection_summary(
        records,
        design,
        &ctx.registry,
        &run,
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
    if sel.run_id != run.run_id
        || sel.run_manifest_sha256 != run.run_manifest_sha256
        || sel.frozen_design_sha256 != run.frozen_design_sha256
    {
        errors
            .push("selected-candidate run identity diverges from the selection freeze".to_string());
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

/// Verify the promotion artifacts.
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
    let design = &ctx.design;
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
    if records.len() as u32 != design.promotion_episodes(&ctx.registry) {
        errors.push(format!(
            "promotion has {} episodes, expected {}",
            records.len(),
            design.promotion_episodes(&ctx.registry)
        ));
    }
    let mut pair_counts: BTreeMap<(String, u32, String), u32> = BTreeMap::new();
    let mut position_counts: BTreeMap<(String, u32), u32> = BTreeMap::new();
    for r in records {
        let who = r.episode_id.clone();
        if r.experiment_id != design.experiment_id || r.phase != "promotion" {
            errors.push(format!("{who}: wrong experiment id/phase"));
        }
        if (r.provenance.temperature - design.agent_temperature).abs() > 1e-9 {
            errors.push(format!(
                "{who}: temperature {} != design agent temperature {}",
                r.provenance.temperature, design.agent_temperature
            ));
        }
        *pair_counts
            .entry((r.task_id.clone(), r.repetition, r.condition.clone()))
            .or_insert(0) += 1;
        *position_counts
            .entry((r.condition.clone(), r.condition_position))
            .or_insert(0) += 1;
        let allowed = r.condition == "G0" || r.condition == selected_id;
        if !allowed {
            errors.push(format!(
                "{who}: unselected condition {:?} in promotion (multi-candidate leakage)",
                r.condition
            ));
            continue;
        }
        let want = design
            .promotion_condition_order(r.repetition, &selected_id)
            .get((r.condition_position - 1) as usize)
            .cloned();
        if want.as_deref() != Some(r.condition.as_str()) {
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
        verify_record_core(&mut errors, &who, design, task, count, &expected_full, r);
    }
    for t in ctx
        .registry
        .iter()
        .filter(|t| t.split == TaskSplit::Promotion)
    {
        for rep in 1..=design.promotion_repetitions() {
            for c in ["G0", selected_id.as_str()] {
                if pair_counts
                    .get(&(t.id.to_string(), rep, c.to_string()))
                    .copied()
                    .unwrap_or(0)
                    != 1
                {
                    errors.push(format!(
                        "promotion cell ({}, rep {rep}, {c}) does not appear exactly once",
                        t.id
                    ));
                }
            }
        }
    }
    let prom_count = ctx
        .registry
        .iter()
        .filter(|t| t.split == TaskSplit::Promotion)
        .count() as u32;
    let expected_per_cell = prom_count * design.promotion_repetitions() / 2;
    for ((cond, pos), n) in &position_counts {
        if *n != expected_per_cell {
            errors.push(format!(
                "promotion condition {cond} position {pos} has {n} episodes, expected {expected_per_cell} (registered alternating balance)"
            ));
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
    let run = RunIdentity {
        run_id: str_of("run_id"),
        run_manifest_sha256: str_of("run_manifest_sha256"),
        frozen_design_sha256: str_of("frozen_design_sha256"),
    };
    for r in records {
        if r.provenance.run_id != run.run_id
            || r.provenance.run_manifest_sha256 != run.run_manifest_sha256
            || r.provenance.frozen_design_sha256 != run.frozen_design_sha256
        {
            errors.push(format!(
                "{}: run-identity provenance diverges from the summary freeze",
                r.episode_id
            ));
        }
    }
    let recomputed = compute_promotion_summary(
        records,
        design,
        &ctx.registry,
        &run,
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
        let turn = requests.len() as u32 + 1;
        requests.push(ModelRequestRecord {
            turn,
            finish_reason: Some("tool_calls".to_string()),
            prompt_tokens: Some(100),
            completion_tokens: Some(5),
            total_tokens: Some(105),
            cached_prompt_tokens: Some(0),
            reasoning_tokens: Some(0),
            uncached_prompt_tokens: Some(100),
            // Synthetic (network-free) request-level timing evidence:
            // deterministic values far inside the registered deadline
            // contract (the fields may be synthetic in tests).
            request_started_elapsed_ms: 50 * turn as u64,
            requested_timeout_ms: 5_000,
            request_finished_elapsed_ms: 50 * turn as u64 + 80,
            timed_out: false,
            response_accepted: true,
            // Synthetic (network-free) record: empty channel binding
            // (no channel traffic is or can be simulated here).
            channel_request_id: String::new(),
            request_body_sha256: String::new(),
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
    let turn = requests.len() as u32 + 1;
    requests.push(ModelRequestRecord {
        turn,
        finish_reason: Some("stop".to_string()),
        prompt_tokens: Some(100),
        completion_tokens: Some(5),
        total_tokens: Some(105),
        cached_prompt_tokens: Some(0),
        reasoning_tokens: Some(0),
        uncached_prompt_tokens: Some(100),
        request_started_elapsed_ms: 50 * turn as u64,
        requested_timeout_ms: 5_000,
        request_finished_elapsed_ms: 50 * turn as u64 + 80,
        timed_out: false,
        response_accepted: true,
        // Synthetic (network-free) record: empty channel binding.
        channel_request_id: String::new(),
        request_body_sha256: String::new(),
    });

    let final_state = if success {
        task.target_state.clone()
    } else {
        task.initial_state.clone()
    };
    (conv, requests, requested, executed, attempts, final_state)
}

/// A provenance block for synthetic records.
fn synth_provenance(ctx: &VerifierContext, run: &RunIdentity) -> RecordProvenance {
    RecordProvenance {
        model: "synthetic-model".to_string(),
        temperature: ctx.design.agent_temperature,
        redacted_endpoint: "http://synthetic.invalid/v1".to_string(),
        code_under_test_commit: SHA_0.to_string(),
        run_id: run.run_id.clone(),
        run_manifest_sha256: run.run_manifest_sha256.clone(),
        frozen_design_sha256: run.frozen_design_sha256.clone(),
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
/// - discovery: 18 episodes, 6 successes / 12 failures, no infra
///   (meets both discovery gates: >= 16 valid, >= 8 incumbent failures);
/// - selection: G0 fails 13/30 cells; C2 wins all 13 (selected);
///   C1 and C3 tie exactly (ordinal tie-break); C4 negative;
/// - promotion: 30 wins / 2 losses / 16 ties → quality_improvement.
pub fn build_synthetic_artifacts() -> SyntheticArtifacts {
    let dir = crate::experiment::crate_dir();
    let design = Design::load(&dir.join("design.json")).expect("0014 design manifest loads");
    let incumbent = SYNTH_INCUMBENT.to_string();
    let registry_file =
        crate::experiment::load_tasks(&dir.join("tasks.json")).expect("registry loads");
    let profile: Value = serde_json::from_str(
        std::fs::read_to_string(dir.join("stress-profile.json"))
            .as_deref()
            .unwrap_or("{}"),
    )
    .unwrap_or(Value::Null);
    let ctx = VerifierContext::new(
        incumbent.clone(),
        SHA_A.to_string(),
        SHA_B.to_string(),
        registry_file.tasks.clone(),
        profile.clone(),
        design.clone(),
    );
    let run = RunIdentity {
        run_id: "0014-r1".to_string(),
        run_manifest_sha256: SHA_0.to_string(),
        frozen_design_sha256: SHA_0.to_string(),
    };
    let prov = synth_provenance(&ctx, &run);
    let registry = &ctx.registry;

    // ---------- discovery ----------
    // The synthetic design must satisfy BOTH discovery gates:
    // >= 16 valid episodes AND >= 8 incumbent failures.  reps 1-2
    // succeed, reps 3-6 fail (the single write lands in the drop
    // window): 6 successes / 12 failures per 18 episodes.
    let mut discovery = Vec::new();
    let mut counter = 0usize;
    for task in registry.iter().filter(|t| t.split == TaskSplit::Discovery) {
        let (_, count) = frozen_stress(&task.family).unwrap();
        for rep in 1..=design.discovery_repetitions() {
            counter += 1;
            let success = rep <= 2;
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
                experiment_id: design.experiment_id.clone(),
                phase: "discovery".to_string(),
                episode_id: format!("synth-disc-{counter:03}"),
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
            debug_assert!(
                r.model_turn_count <= design.max_model_turns
                    && r.tool_call_count <= design.max_tool_calls
            );
        }
    }
    let input_file_sha = SHA_C;
    let input = build_mutation_input(&design, &incumbent, &ToolExecutor::specs(), &discovery);
    let discovery_summary = compute_discovery_summary(
        &discovery,
        &design,
        &ctx.registry,
        &run,
        "synthetic-model",
        "http://synthetic.invalid/v1",
        SHA_0,
        &ctx.incumbent_file_sha,
        &ctx.task_registry_sha,
        &ctx.profile_sha,
        input_file_sha,
    );
    assert!(
        discovery_summary.proceed_to_mutation_generation,
        "synthetic discovery must pass both gates, got {discovery_summary:?}"
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
        &design,
        &serde_json::to_value(json!({ "candidates": raw_candidates }))
            .unwrap()
            .to_string(),
        &incumbent,
        registry,
    );
    assert!(
        verrs.is_empty(),
        "synthetic suffixes must validate: {verrs:?}"
    );
    let pool = crate::mutation::build_pool(
        &design,
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
        experiment_id: design.experiment_id.clone(),
        run_id: run.run_id.clone(),
        run_manifest_sha256: run.run_manifest_sha256.clone(),
        frozen_design_sha256: run.frozen_design_sha256.clone(),
        model: "synthetic-model".to_string(),
        temperature: design.mutator_temperature,
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
            channel_request_id: String::new(),
            request_body_sha256: String::new(),
        }],
        mutation_generation_complete: true,
        accepted_pool: Some(pool.clone()),
        wall_time_ms: 1500,
    };

    // ---------- selection ----------
    // 30 cells (3 tasks × 10 reps, c = ti*10 + rep-1). G0 fails on 13
    // (task0 reps 0-4, task1 reps 0-4, task2 reps 0-2). C2 always
    // succeeds → 13 wins / 0 losses / 17 ties → selected (the design
    // winner). C1/C3 each fail on exactly two G0-success cells
    // (13W/2L, exact tie broken by ordinal). C4 fails on two G0-failure
    // cells (ties) plus two G0-success cells (losses) → 11W/2L.
    let g0_fail = |c: usize| -> bool {
        let ti = c / 10;
        let r = c % 10;
        (ti < 2 && r < 5) || (ti == 2 && r < 3)
    };
    let c1_fail = |c: usize| -> bool { c == 6 || c == 16 };
    let c3_fail = |c: usize| -> bool { c == 7 || c == 17 };
    let c4_fail = |c: usize| -> bool { c == 0 || c == 10 || c == 8 || c == 18 };
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
    // The synthetic set encodes the full 3×10×5 = 150 design; use the
    // design repetition count so the record count always matches the
    // registered selection episodes.
    let synth_reps: u32 = design.selection_repetitions();
    for (ti, task) in registry
        .iter()
        .filter(|t| t.split == TaskSplit::Selection)
        .enumerate()
    {
        let (_, count) = frozen_stress(&task.family).unwrap();
        for rep in 1..=synth_reps {
            for pos in 1..=design.selection.conditions.len() as u32 {
                let cond = design
                    .selection_condition_at(rep, pos)
                    .expect("synthetic schedule is registered");
                let c = ti * synth_reps as usize + (rep - 1) as usize;
                counter += 1;
                let success = cond_success(c, &cond);
                let (suffix_sha, full) = if cond == "G0" {
                    (None, incumbent.clone())
                } else {
                    let ordinal = cond.as_bytes()[1] as usize - b'0' as usize;
                    let e = &pool.candidates[ordinal - 1];
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
                    experiment_id: design.experiment_id.clone(),
                    phase: "selection".to_string(),
                    episode_id: format!("synth-sel-{counter:03}"),
                    sequence: counter as u32,
                    family: task.family.clone(),
                    task_id: task.id.clone(),
                    split: "selection".to_string(),
                    stress_level: frozen_stress(&task.family).unwrap().0.to_string(),
                    drop_count: count,
                    fault_key: task.fault_key.clone(),
                    repetition: rep,
                    condition_position: pos,
                    condition: cond.clone(),
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
        &design,
        &ctx.registry,
        &run,
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
        experiment_id: design.experiment_id.clone(),
        run_id: run.run_id.clone(),
        run_manifest_sha256: run.run_manifest_sha256.clone(),
        frozen_design_sha256: run.frozen_design_sha256.clone(),
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
        .iter()
        .filter(|t| t.split == TaskSplit::Promotion)
        .enumerate()
    {
        let (_, count) = frozen_stress(&task.family).unwrap();
        for rep in 1..=design.promotion_repetitions() {
            for (pos, cond) in design
                .promotion_condition_order(rep, "C2")
                .into_iter()
                .enumerate()
            {
                let p = ti * design.promotion_repetitions() as usize + (rep - 1) as usize;
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
                    experiment_id: design.experiment_id.clone(),
                    phase: "promotion".to_string(),
                    episode_id: format!("synth-prom-{counter:03}"),
                    sequence: counter as u32,
                    family: task.family.clone(),
                    task_id: task.id.clone(),
                    split: "promotion".to_string(),
                    stress_level: frozen_stress(&task.family).unwrap().0.to_string(),
                    drop_count: count,
                    fault_key: task.fault_key.clone(),
                    repetition: rep,
                    condition_position: (pos + 1) as u32,
                    condition: cond.clone(),
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
        &design,
        &ctx.registry,
        &run,
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
