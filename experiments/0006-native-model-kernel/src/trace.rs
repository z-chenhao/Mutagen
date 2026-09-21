//! Trajectory recording, tags, verification metric, summary, and
//! artifact verification for Experiment 0006.
//!
//! The kernel is the authoritative execution recorder: every event here
//! originates from the Rust kernel's own `Vec<TrajectoryEvent>` (see
//! `kernel.rs`). Trajectories are never reconstructed from logs.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;

use crate::protocol::{canonical_call_identity, canonical_json};
use crate::tools::StateKey;

/// One typed kernel event. Deliberately a small closed set; there is no
/// general event bus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrajectoryEvent {
    EpisodeStarted,
    ModelTurnStarted {
        turn: usize,
    },
    ModelResponseReceived {
        turn: usize,
        response_kind: String,
    },
    ToolCallRequested {
        turn: usize,
        tool_call_id: String,
        tool_name: String,
        arguments: Value,
    },
    ToolExecutionStarted {
        tool_call_id: String,
        tool_name: String,
    },
    ToolExecutionFinished {
        tool_call_id: String,
        tool_name: String,
        result: Value,
        duration_us: u64,
    },
    FinalAnswer {
        text: String,
    },
    EpisodeFinished {
        eligible: bool,
        termination_reason: String,
    },
    EpisodeFailed {
        reason: String,
    },
}

/// Canonical record of one tool interaction in execution (request) order.
/// Persisted in artifacts as `tool_calls`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub sequence: usize,
    pub turn: usize,
    pub tool_call_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub result: Value,
}

/// Terminal reason for an episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationReason {
    Completed,
    TurnLimit,
    ToolCallLimit,
    ParallelToolCallsUnsupported,
    HttpError,
    ParseError,
    UnknownTool,
    InvalidArguments,
    EpisodeFailed,
}

impl TerminationReason {
    pub fn as_str(self) -> &'static str {
        match self {
            TerminationReason::Completed => "completed",
            TerminationReason::TurnLimit => "turn_limit",
            TerminationReason::ToolCallLimit => "tool_call_limit",
            TerminationReason::ParallelToolCallsUnsupported => "parallel_tool_calls_unsupported",
            TerminationReason::HttpError => "http_error",
            TerminationReason::ParseError => "parse_error",
            TerminationReason::UnknownTool => "unknown_tool",
            TerminationReason::InvalidArguments => "invalid_arguments",
            TerminationReason::EpisodeFailed => "episode_failed",
        }
    }
}

/// Wall-clock measurements for one episode. All labeled approximate:
/// `kernel_overhead_estimate_ms` is a residual, not a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Timing {
    pub total_wall_time_ms: u64,
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
    pub task_id: String,
    pub task_name: String,
    pub condition: String,
    pub repetition: u32,
    pub model: String,
    pub temperature: f64,
    pub redacted_endpoint: String,
    pub code_under_test_commit: String,
    pub baseline_prompt_sha256: String,
    pub candidate_prompt_sha256: String,
    pub task_suite_sha256: String,
    pub initial_state: Value,
    pub final_state: Value,
    pub tool_calls: Vec<ToolCallRecord>,
    pub final_answer: Option<String>,
    pub model_turn_count: usize,
    pub tool_call_count: usize,
    pub usage: UsageRecord,
    pub timing: Timing,
    pub eligible: bool,
    pub termination_reason: String,
    pub error: Option<String>,
}

/// Token usage for one episode (zeros when the provider reported none).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct UsageRecord {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl UsageRecord {
    /// Accumulate usage from one model response.
    pub fn add(&mut self, other: &crate::protocol::TokenUsage) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.total_tokens += other.total_tokens;
    }
}

impl EpisodeRecord {
    /// Canonical identities of this episode's tool calls, in order.
    /// Used by tests and by divergence analysis tooling.
    #[allow(dead_code)]
    pub fn canonical_identities(&self) -> Vec<String> {
        self.tool_calls
            .iter()
            .map(|c| canonical_call_identity(&c.tool_name, &c.arguments))
            .collect()
    }
}

// ===========================================================================
// Trajectory tags (spec §45)
// ===========================================================================

/// The eight independent trajectory tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrajectoryTag {
    Exact,
    AddedCall,
    OmittedCall,
    NovelCall,
    Reordered,
    RepeatedCountChange,
    ArgumentChange,
    StateEffectOrderChange,
}

impl TrajectoryTag {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::AddedCall => "added_call",
            Self::OmittedCall => "omitted_call",
            Self::NovelCall => "novel_call",
            Self::Reordered => "reordered",
            Self::RepeatedCountChange => "repeated_count_change",
            Self::ArgumentChange => "argument_change",
            Self::StateEffectOrderChange => "state_effect_order_change",
        }
    }

    pub fn all() -> [Self; 8] {
        [
            Self::Exact,
            Self::AddedCall,
            Self::OmittedCall,
            Self::NovelCall,
            Self::Reordered,
            Self::RepeatedCountChange,
            Self::ArgumentChange,
            Self::StateEffectOrderChange,
        ]
    }
}

/// A call sequence with both the canonical identity and the raw
/// tool name / key (for the state-effect tag).
struct CallRef {
    identity: String,
    tool: String,
    key: Option<StateKey>,
}

impl CallRef {
    fn from_record(r: &ToolCallRecord) -> Self {
        let key = r
            .arguments
            .get("key")
            .and_then(Value::as_str)
            .and_then(StateKey::parse);
        Self {
            identity: canonical_call_identity(&r.tool_name, &r.arguments),
            tool: r.tool_name.clone(),
            key,
        }
    }
}

fn counts(calls: &[CallRef]) -> BTreeMap<String, usize> {
    let mut m: BTreeMap<String, usize> = BTreeMap::new();
    for c in calls {
        *m.entry(c.identity.clone()).or_default() += 1;
    }
    m
}

/// The *unique* canonical argument identities issued for one tool name,
/// as a set: insensitive to call order and to call multiplicity.
fn unique_args(calls: &[CallRef], tool: &str) -> BTreeSetLite {
    let mut s = BTreeSetLite::new();
    for c in calls.iter().filter(|c| c.tool == tool) {
        s.insert(c.identity.clone());
    }
    s
}

/// The write/read *relation* on one key, projected as a 2-bit signature
/// `(write_before_read, read_before_write)`: whether the trajectory
/// contains at least one write of the key before at least one read of
/// the key, and vice versa.
///
/// Returns `None` for keys on which the trajectory performs fewer than
/// one write or fewer than one read: without both operations the
/// write/read relation is undefined, and presence/count differences are
/// the domain of `added_call` / `omitted_call` /
/// `repeated_count_change`, not of this tag.
fn state_relations(calls: &[CallRef], key: StateKey) -> Option<(bool, bool)> {
    let mut writes = 0usize;
    let mut reads = 0usize;
    let mut write_before_read = false;
    let mut read_before_write = false;
    for c in calls.iter().filter(|c| c.key == Some(key)) {
        if c.tool == "state_write" {
            // A write after an earlier read of the same key establishes
            // read-before-write.
            if reads > 0 {
                read_before_write = true;
            }
            writes += 1;
        } else {
            // A read after an earlier write of the same key establishes
            // write-before-read.
            if writes > 0 {
                write_before_read = true;
            }
            reads += 1;
        }
    }
    if writes > 0 && reads > 0 {
        Some((write_before_read, read_before_write))
    } else {
        None
    }
}

/// Compute all tags that apply to one baseline/candidate pair.
/// Multiple tags may apply; they are independent observations.
pub fn trajectory_tags(
    baseline: &[ToolCallRecord],
    candidate: &[ToolCallRecord],
) -> Vec<TrajectoryTag> {
    let b: Vec<CallRef> = baseline.iter().map(CallRef::from_record).collect();
    let c: Vec<CallRef> = candidate.iter().map(CallRef::from_record).collect();

    let mut tags = Vec::new();

    let b_seq: Vec<&str> = b.iter().map(|x| x.identity.as_str()).collect();
    let c_seq: Vec<&str> = c.iter().map(|x| x.identity.as_str()).collect();
    let b_multiset = counts(&b);
    let c_multiset = counts(&c);

    // exact: sequences identical (order + identity).
    if b_seq == c_seq {
        tags.push(TrajectoryTag::Exact);
    }

    // added_call: some identity occurs more often in candidate.
    if c_multiset
        .iter()
        .any(|(id, n)| b_multiset.get(id).copied().unwrap_or(0) < *n)
    {
        tags.push(TrajectoryTag::AddedCall);
    }

    // omitted_call: some identity occurs less often in candidate.
    if b_multiset
        .iter()
        .any(|(id, n)| c_multiset.get(id).copied().unwrap_or(0) < *n)
    {
        tags.push(TrajectoryTag::OmittedCall);
    }

    // novel_call: candidate has an identity absent from baseline.
    if c_multiset
        .iter()
        .any(|(id, n)| *n > 0 && !b_multiset.contains_key(id))
    {
        tags.push(TrajectoryTag::NovelCall);
    }

    // reordered: same multiset, different order (and therefore non-exact).
    if b_multiset == c_multiset && b_seq != c_seq {
        tags.push(TrajectoryTag::Reordered);
    }

    // repeated_count_change: some identity present in both with different count.
    if b_multiset.iter().chain(c_multiset.iter()).any(|(id, _)| {
        let bn = b_multiset.get(id).copied().unwrap_or(0);
        let cn = c_multiset.get(id).copied().unwrap_or(0);
        bn > 0 && cn > 0 && bn != cn
    }) {
        tags.push(TrajectoryTag::RepeatedCountChange);
    }

    // argument_change: for a tool name present in *both* trajectories,
    // the set of distinct arguments differs across runs. Deliberately
    // insensitive to call order and to multiplicity: a count difference
    // is the domain of `added_call` / `omitted_call` /
    // `repeated_count_change`, and a tool absent from one run (0 -> n)
    // is the domain of `added_call` / `novel_call`.
    let mut common_tools = BTreeSetLite::new();
    let b_tools: BTreeSetLite = {
        let mut s = BTreeSetLite::new();
        for x in &b {
            s.insert(x.tool.clone());
        }
        s
    };
    let c_tools: BTreeSetLite = {
        let mut s = BTreeSetLite::new();
        for x in &c {
            s.insert(x.tool.clone());
        }
        s
    };
    for t in b_tools.iter() {
        if c_tools.contains(t) {
            common_tools.insert(t.clone());
        }
    }
    if common_tools
        .iter()
        .any(|tool| unique_args(&b, tool) != unique_args(&c, tool))
    {
        tags.push(TrajectoryTag::ArgumentChange);
    }

    // state_effect_order_change: for a key on which *both* trajectories
    // perform at least one read AND at least one write, the write/read
    // relation differs (e.g. W-then-R vs R-then-W, or an inserted read
    // that inverts part of the order). Keys where one side lacks reads
    // or writes are not characterized: pure presence or multiplicity
    // changes belong to the count tags, so this tag is independent of
    // `added_call` / `repeated_count_change` for those cases.
    for key in [StateKey::X, StateKey::Y] {
        if let (Some(rb), Some(rc)) = (state_relations(&b, key), state_relations(&c, key)) {
            if rb != rc {
                tags.push(TrajectoryTag::StateEffectOrderChange);
                break;
            }
        }
    }

    tags
}

/// Minimal sorted set to avoid pulling in `ordered_set`.
#[derive(Debug, Default, PartialEq)]
struct BTreeSetLite(BTreeMap<String, bool>);

impl BTreeSetLite {
    fn new() -> Self {
        Self::default()
    }
    fn insert(&mut self, s: String) {
        self.0.insert(s, true);
    }
    fn contains(&self, s: &str) -> bool {
        self.0.contains_key(s)
    }
    fn iter(&self) -> impl Iterator<Item = &String> {
        self.0.keys()
    }
}

// ===========================================================================
// Verification metric (spec §46)
// ===========================================================================

/// Verification of writes within one episode's call sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VerificationCount {
    pub successful_writes: usize,
    pub verified_writes: usize,
}

/// For each successful `state_write(key, value)` at position i, a write
/// is *verified* if some later `state_read(key)` (i' > i) occurs before
/// the final answer. All recorded tool calls occur before the final
/// answer by construction, so "later" means later in the sequence.
pub fn verification_of(record: &EpisodeRecord) -> VerificationCount {
    let mut successful = 0;
    let mut verified = 0;
    for (i, call) in record.tool_calls.iter().enumerate() {
        if call.tool_name != "state_write" {
            continue;
        }
        let ok = call.result.get("ok").and_then(Value::as_bool) == Some(true);
        if !ok {
            continue;
        }
        let key = match call
            .arguments
            .get("key")
            .and_then(Value::as_str)
            .and_then(StateKey::parse)
        {
            Some(k) => k,
            None => continue,
        };
        successful += 1;
        let is_verified = record.tool_calls[i + 1..].iter().any(|later| {
            later.tool_name == "state_read"
                && later
                    .arguments
                    .get("key")
                    .and_then(Value::as_str)
                    .and_then(StateKey::parse)
                    == Some(key)
        });
        if is_verified {
            verified += 1;
        }
    }
    VerificationCount {
        successful_writes: successful,
        verified_writes: verified,
    }
}

/// Aggregate verification over many episodes. Rate is `null` (None)
/// when there are zero writes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerificationSummary {
    pub successful_writes: usize,
    pub verified_writes: usize,
    pub verification_rate: Option<f64>,
}

/// Token usage accounting for the whole run (audit §24).
///
/// The raw artifact persists `usage = {prompt_tokens, completion_tokens,
/// total_tokens}` per episode (summed over that episode's model
/// requests). The 0006 response parser discarded every other usage
/// field the provider sent, so categories the artifact does not contain
/// are `null` with an explicit note — never estimated.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UsageSummary {
    /// Nominal prompt tokens over all model requests of all episodes.
    /// Per the serving stack's semantics this counts the *full* prompt
    /// of every request, including any prefix it served from its prefix
    /// cache; it is not the amount of prefill compute performed.
    pub prompt_tokens: u64,
    /// Output tokens over all model requests. Includes reasoning and
    /// visible text; the 0006 parser did not separate the two, so the
    /// reasoning share is not recoverable from this artifact.
    pub completion_tokens: u64,
    /// `prompt_tokens + completion_tokens` as reported per request.
    pub total_tokens: u64,
    /// Number of model requests issued (one per model turn).
    pub model_requests: u64,
    /// Episodes covered (all 36, eligible and ineligible alike).
    pub episodes: usize,
    /// Cached-prefix tokens as reported by the provider: `null` — the
    /// 0006 parser did not persist the provider's cache accounting
    /// (`usage.prompt_tokens_details.cached_tokens`), so the prefix-cache
    /// hit rate during the run is not quantifiable from this artifact.
    pub cached_prefix_tokens: Option<u64>,
    /// Effective uncached prefill work: `null` — not computable without
    /// the cached-prefix accounting above.
    pub effective_uncached_prefill_tokens: Option<u64>,
    /// Explicit statement of what the artifact does and does not contain.
    pub note: String,
}

pub fn aggregate_verification(records: &[EpisodeRecord]) -> VerificationSummary {
    let mut successful = 0;
    let mut verified = 0;
    for r in records {
        if r.eligible {
            let v = verification_of(r);
            successful += v.successful_writes;
            verified += v.verified_writes;
        }
    }
    VerificationSummary {
        verification_rate: if successful == 0 {
            None
        } else {
            Some(verified as f64 / successful as f64)
        },
        successful_writes: successful,
        verified_writes: verified,
    }
}

// ===========================================================================
// Summary (spec §61) and conclusion (spec §50)
// ===========================================================================

/// One baseline/candidate pair (task + repetition).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairRecord {
    pub task_id: String,
    pub task_name: String,
    pub repetition: u32,
    pub eligible: bool,
    pub tags: Vec<TrajectoryTag>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Conclusion {
    Supported,
    Refuted,
    Inconclusive,
}

impl Conclusion {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Refuted => "refuted",
            Self::Inconclusive => "inconclusive",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelFailures {
    pub http_failures: usize,
    pub parse_failures: usize,
    pub unknown_tools: usize,
    pub invalid_arguments: usize,
    pub parallel_tool_calls: usize,
    pub turn_limit: usize,
    pub tool_call_limit: usize,
    pub eligible_episodes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TimingSummary {
    pub total_wall_time_ms: u64,
    pub model_wait_time_ms: u64,
    pub tool_execution_time_us: u64,
    pub kernel_overhead_estimate_ms: u64,
    pub episodes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagSummary {
    pub count: usize,
    pub task_ids: Vec<String>,
    pub pairs: Vec<PairRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerTaskSummary {
    pub task_id: String,
    pub task_name: String,
    pub eligible_pairs: usize,
    pub tags: Vec<TrajectoryTag>,
    pub non_exact_pairs: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub experiment_id: String,
    pub code_under_test_commit: String,
    pub model: String,
    pub redacted_endpoint: String,
    pub temperature: f64,
    pub baseline_prompt_sha256: String,
    pub candidate_prompt_sha256: String,
    pub task_suite_sha256: String,
    pub total_episodes: usize,
    pub eligible_episodes: usize,
    pub ineligible_episodes: usize,
    pub eligible_pairs: usize,
    pub ineligible_pairs: usize,
    pub baseline: VerificationSummary,
    pub candidate: VerificationSummary,
    pub baseline_total_tool_calls: usize,
    pub candidate_total_tool_calls: usize,
    pub trajectory_tags: BTreeMap<String, TagSummary>,
    pub per_task: Vec<PerTaskSummary>,
    pub read_only_control_write_count: usize,
    pub read_only_control_write_occurrences: Vec<ReadOnlyWriteOccurrence>,
    pub kernel_failures: KernelFailures,
    pub timing: TimingSummary,
    #[serde(default)]
    pub usage: UsageSummary,
    pub conclusion: Conclusion,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadOnlyWriteOccurrence {
    pub run_id: String,
    pub task_id: String,
    pub condition: String,
    pub sequence: usize,
    pub key: String,
    pub value: String,
}

/// Pre-registered read-only / write-containing task split (spec §47).
pub const READ_ONLY_TASKS: &[&str] = &["T1", "T2"];
pub const WRITE_TASKS: &[&str] = &["T3", "T4", "T5", "T6"];

/// Recompute the full summary from raw trajectories. This is the single
/// source of the summary logic: the runner writes it, the verifier
/// re-derives it and compares.
pub fn compute_summary(records: &[EpisodeRecord]) -> Summary {
    let eligible: Vec<&EpisodeRecord> = records.iter().filter(|r| r.eligible).collect();
    let ineligible = records.len() - eligible.len();

    let baseline: Vec<&EpisodeRecord> = records
        .iter()
        .filter(|r| r.condition == "baseline")
        .collect();
    let candidate: Vec<&EpisodeRecord> = records
        .iter()
        .filter(|r| r.condition == "candidate")
        .collect();

    // Pairing: by task_id + repetition; a pair is eligible only if both
    // of its runs are eligible.
    let mut pairs: Vec<PairRecord> = Vec::new();
    for task in unique_task_ids(records) {
        let rep = records
            .iter()
            .filter(|r| r.task_id == task)
            .map(|r| r.repetition)
            .max()
            .unwrap_or(0);
        for r in 1..=rep {
            let b = records
                .iter()
                .find(|x| x.task_id == task && x.repetition == r && x.condition == "baseline");
            let c = records
                .iter()
                .find(|x| x.task_id == task && x.repetition == r && x.condition == "candidate");
            let (Some(b), Some(c)) = (b, c) else {
                continue;
            };
            let name = b.task_name.clone();
            let pair_eligible = b.eligible && c.eligible;
            let tags = if pair_eligible {
                trajectory_tags(&b.tool_calls, &c.tool_calls)
            } else {
                Vec::new()
            };
            pairs.push(PairRecord {
                task_id: task.clone(),
                task_name: name,
                repetition: r,
                eligible: pair_eligible,
                tags,
            });
        }
    }

    let eligible_pairs = pairs.iter().filter(|p| p.eligible).count();

    // Per tag.
    let mut trajectory_tags: BTreeMap<String, TagSummary> = BTreeMap::new();
    for tag in TrajectoryTag::all() {
        let matching: Vec<&PairRecord> = pairs
            .iter()
            .filter(|p| p.eligible && p.tags.contains(&tag))
            .collect();
        let mut task_ids = BTreeSetLite::new();
        let mut pair_list = Vec::new();
        for p in &matching {
            task_ids.insert(p.task_id.clone());
            pair_list.push((*p).clone());
        }
        trajectory_tags.insert(
            tag.as_str().to_string(),
            TagSummary {
                count: matching.len(),
                task_ids: task_ids.iter().cloned().collect(),
                pairs: pair_list,
            },
        );
    }

    // Per task.
    let mut per_task = Vec::new();
    for task in unique_task_ids(records) {
        let task_pairs = pairs
            .iter()
            .filter(|p| p.task_id == task)
            .collect::<Vec<_>>();
        let eligible_pairs = task_pairs.iter().filter(|p| p.eligible).count();
        let mut tags = BTreeSetLite::new();
        let non_exact = task_pairs
            .iter()
            .filter(|p| p.eligible && !p.tags.contains(&TrajectoryTag::Exact))
            .count();
        for p in task_pairs.iter().filter(|p| p.eligible) {
            for t in &p.tags {
                tags.insert(t.as_str().to_string());
            }
        }
        let name = records
            .iter()
            .find(|r| r.task_id == task)
            .map(|r| r.task_name.clone())
            .unwrap_or_default();
        per_task.push(PerTaskSummary {
            task_id: task.clone(),
            task_name: name,
            eligible_pairs,
            tags: tags
                .iter()
                .filter_map(|t| serde_json::from_value::<TrajectoryTag>(json!(t)).ok())
                .collect(),
            non_exact_pairs: non_exact,
        });
    }

    // Read-only control: unexpected state_write calls in T1/T2 runs.
    let mut ro_count = 0;
    let mut ro_occurrences = Vec::new();
    for r in records
        .iter()
        .filter(|r| READ_ONLY_TASKS.contains(&r.task_id.as_str()))
    {
        for call in &r.tool_calls {
            if call.tool_name == "state_write" {
                ro_count += 1;
                ro_occurrences.push(ReadOnlyWriteOccurrence {
                    run_id: r.run_id.clone(),
                    task_id: r.task_id.clone(),
                    condition: r.condition.clone(),
                    sequence: call.sequence,
                    key: call
                        .arguments
                        .get("key")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    value: call
                        .arguments
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                });
            }
        }
    }

    // Kernel failures, by termination reason over all episodes.
    let by_reason = |reason: TerminationReason| -> usize {
        records
            .iter()
            .filter(|r| r.termination_reason == reason.as_str())
            .count()
    };
    let kernel_failures = KernelFailures {
        http_failures: by_reason(TerminationReason::HttpError),
        parse_failures: by_reason(TerminationReason::ParseError),
        unknown_tools: by_reason(TerminationReason::UnknownTool),
        invalid_arguments: by_reason(TerminationReason::InvalidArguments),
        parallel_tool_calls: by_reason(TerminationReason::ParallelToolCallsUnsupported),
        turn_limit: by_reason(TerminationReason::TurnLimit),
        tool_call_limit: by_reason(TerminationReason::ToolCallLimit),
        eligible_episodes: eligible.len(),
    };

    // Condition aggregates (eligible episodes only).
    let baseline_summary =
        aggregate_verification(&baseline.iter().map(|b| (*b).clone()).collect::<Vec<_>>());
    let candidate_summary =
        aggregate_verification(&candidate.iter().map(|c| (*c).clone()).collect::<Vec<_>>());

    let total_tool_calls = |conds: &[&EpisodeRecord]| -> usize {
        conds
            .iter()
            .filter(|r| r.eligible)
            .map(|r| r.tool_call_count)
            .sum()
    };

    // Timing (all episodes).
    let timing = TimingSummary {
        total_wall_time_ms: records.iter().map(|r| r.timing.total_wall_time_ms).sum(),
        model_wait_time_ms: records.iter().map(|r| r.timing.model_wait_time_ms).sum(),
        tool_execution_time_us: records
            .iter()
            .map(|r| r.timing.tool_execution_time_us)
            .sum(),
        kernel_overhead_estimate_ms: records
            .iter()
            .map(|r| r.timing.kernel_overhead_estimate_ms)
            .sum(),
        episodes: records.len(),
    };

    // Token usage accounting over all episodes (audit §24).
    let usage = UsageSummary {
        prompt_tokens: records.iter().map(|r| r.usage.prompt_tokens).sum(),
        completion_tokens: records.iter().map(|r| r.usage.completion_tokens).sum(),
        total_tokens: records.iter().map(|r| r.usage.total_tokens).sum(),
        model_requests: records.iter().map(|r| r.model_turn_count as u64).sum(),
        episodes: records.len(),
        cached_prefix_tokens: None,
        effective_uncached_prefill_tokens: None,
        note: USAGE_NOTE.to_string(),
    };

    // Pre-registered conclusion rule (spec §50), applied mechanically.
    let conclusion = decide_conclusion(&pairs, &baseline_summary, &candidate_summary);

    Summary {
        experiment_id: "0006".into(),
        code_under_test_commit: records
            .first()
            .map(|r| r.code_under_test_commit.clone())
            .unwrap_or_default(),
        model: records.first().map(|r| r.model.clone()).unwrap_or_default(),
        redacted_endpoint: records
            .first()
            .map(|r| r.redacted_endpoint.clone())
            .unwrap_or_default(),
        temperature: records.first().map(|r| r.temperature).unwrap_or(0.2),
        baseline_prompt_sha256: records
            .first()
            .map(|r| r.baseline_prompt_sha256.clone())
            .unwrap_or_default(),
        candidate_prompt_sha256: records
            .first()
            .map(|r| r.candidate_prompt_sha256.clone())
            .unwrap_or_default(),
        task_suite_sha256: records
            .first()
            .map(|r| r.task_suite_sha256.clone())
            .unwrap_or_default(),
        total_episodes: records.len(),
        eligible_episodes: eligible.len(),
        ineligible_episodes: ineligible,
        eligible_pairs,
        ineligible_pairs: pairs.len() - eligible_pairs,
        baseline: baseline_summary,
        candidate: candidate_summary,
        baseline_total_tool_calls: total_tool_calls(&baseline),
        candidate_total_tool_calls: total_tool_calls(&candidate),
        trajectory_tags,
        per_task,
        read_only_control_write_count: ro_count,
        read_only_control_write_occurrences: ro_occurrences,
        kernel_failures,
        timing,
        usage,
        conclusion,
    }
}

/// Explicit statement of what the 0006 usage accounting does and does not
/// contain (audit §24): categories the artifact lacks are `null`, never
/// estimated.
const USAGE_NOTE: &str = concat!(
    "The raw artifact persists only prompt/completion/total tokens per episode. The ",
    "provider's prefix-cache accounting and its reasoning/visible-text split were not ",
    "persisted by the 0006 parser, so cached_prefix_tokens and effective_uncached_",
    "prefill_tokens are null (not zero): the run-time prefix-cache hit rate is UNKNOWN ",
    "from this artifact. completion_tokens includes reasoning and cannot be decomposed. ",
    "prompt_tokens is nominal (full prompt per request), not uncached prefill work. The ",
    "one-line candidate mutation is a system-prompt change; it does not alter the ",
    "recorded token accounting mechanism.",
);

/// The registered task suite of Experiment 0006 (audit §30).
const CANONICAL_TASKS: [(&str, &str); 6] = [
    ("T1", "read_x"),
    ("T2", "read_both"),
    ("T3", "write_x"),
    ("T4", "conditional_write"),
    ("T5", "unrelated_read_after_write"),
    ("T6", "two_writes"),
];

/// The registered per-task condition order for repetitions 1..3
/// (audit §25, §30): file order must be baseline, candidate, candidate,
/// baseline, baseline, candidate.
const COUNTERBALANCE: [&str; 6] = [
    "baseline",
    "candidate",
    "candidate",
    "baseline",
    "baseline",
    "candidate",
];

/// Registered design size. These are fixed constants of the registered
/// Experiment 0006 protocol — they are NEVER inferred from the artifact
/// (a complete 2-repetition / 24-record matrix must fail, not pass).
pub const EXPECTED_REPETITIONS: u32 = 3;
pub const EXPECTED_CONDITIONS: [&str; 2] = ["baseline", "candidate"];
pub const EXPECTED_EPISODES: usize = 36;

/// The registered matrix must be exactly 6 tasks x 2 conditions x 3 reps.
const _: () = assert!(
    EXPECTED_EPISODES
        == CANONICAL_TASKS.len() * EXPECTED_CONDITIONS.len() * EXPECTED_REPETITIONS as usize
);

/// Strict factorial completeness of the registered design.
///
/// FAILS unless the artifact is exactly the registered matrix: 36
/// records; `task_id` in T1..T6 with its registered name; `condition`
/// in {baseline, candidate}; `repetition` in 1..3; every
/// (task, condition, repetition) tuple exactly once; 18/18 condition
/// balance; 6 records per task; 3 records per task-condition. Nothing
/// is derived from the file.
pub fn check_factorial_completeness(records: &[EpisodeRecord]) -> (bool, String) {
    if records.len() != EXPECTED_EPISODES {
        return (
            false,
            format!(
                "{} records; the registered 0006 design requires exactly {} (6 tasks x 2 conditions x {} repetitions)",
                records.len(),
                EXPECTED_EPISODES,
                EXPECTED_REPETITIONS
            ),
        );
    }

    // Registered value domains, record by record.
    for r in records {
        match CANONICAL_TASKS.iter().find(|(id, _)| *id == r.task_id) {
            None => {
                return (
                    false,
                    format!("{}: unregistered task id {}", r.run_id, r.task_id),
                );
            }
            Some((_, name)) => {
                if *name != r.task_name {
                    return (
                        false,
                        format!(
                            "{}: task {} has unregistered name {:?} (expected {:?})",
                            r.run_id, r.task_id, r.task_name, name
                        ),
                    );
                }
            }
        }
        if !EXPECTED_CONDITIONS.contains(&r.condition.as_str()) {
            return (
                false,
                format!("{}: unexpected condition {:?}", r.run_id, r.condition),
            );
        }
        if !(1..=EXPECTED_REPETITIONS).contains(&r.repetition) {
            return (
                false,
                format!(
                    "{}: repetition {} outside the registered 1..={}",
                    r.run_id, r.repetition, EXPECTED_REPETITIONS
                ),
            );
        }
    }

    // Every registered tuple exactly once.
    let mut cells: BTreeMap<String, usize> = BTreeMap::new();
    for r in records {
        *cells
            .entry(format!("{}|{}|{}", r.task_id, r.condition, r.repetition))
            .or_default() += 1;
    }
    let duplicated = cells
        .iter()
        .filter(|(_, n)| **n > 1)
        .map(|(k, _)| k.clone())
        .collect::<Vec<_>>();
    if !duplicated.is_empty() {
        return (
            false,
            format!(
                "{} duplicated tuple(s): {}",
                duplicated.len(),
                duplicated.join(", ")
            ),
        );
    }
    let mut missing = Vec::new();
    for (id, _) in CANONICAL_TASKS {
        for condition in EXPECTED_CONDITIONS {
            for rep in 1..=EXPECTED_REPETITIONS {
                let cell = format!("{id}|{condition}|{rep}");
                if !cells.contains_key(&cell) {
                    missing.push(cell);
                }
            }
        }
    }
    if !missing.is_empty() {
        return (
            false,
            format!("{} missing tuple(s): {}", missing.len(), missing.join(", ")),
        );
    }

    // Exact balance: 18/18 conditions, 6 per task, 3 per task-condition.
    let expected_half = EXPECTED_EPISODES / EXPECTED_CONDITIONS.len();
    let baseline = records
        .iter()
        .filter(|r| r.condition == EXPECTED_CONDITIONS[0])
        .count();
    let candidate = records
        .iter()
        .filter(|r| r.condition == EXPECTED_CONDITIONS[1])
        .count();
    if baseline != expected_half || candidate != expected_half {
        return (
            false,
            format!("condition balance {baseline}/{candidate} != {expected_half}/{expected_half}"),
        );
    }
    let expected_per_task = EXPECTED_EPISODES / CANONICAL_TASKS.len();
    for (id, _) in CANONICAL_TASKS {
        let n = records.iter().filter(|r| r.task_id == *id).count();
        if n != expected_per_task {
            return (
                false,
                format!("task {id}: {n} records, expected {expected_per_task}"),
            );
        }
        for condition in EXPECTED_CONDITIONS {
            let n = records
                .iter()
                .filter(|r| r.task_id == *id && r.condition == condition)
                .count();
            if n != EXPECTED_REPETITIONS as usize {
                return (
                    false,
                    format!("task {id} {condition}: {n} records, expected {EXPECTED_REPETITIONS}"),
                );
            }
        }
    }
    (
        true,
        format!(
            "{} records = 6 tasks x 2 conditions x {} repetitions; every registered tuple exactly once; balance {expected_half}/{expected_half}, {expected_per_task} per task, {} per task-condition",
            EXPECTED_EPISODES, EXPECTED_REPETITIONS, EXPECTED_REPETITIONS
        ),
    )
}

/// Registered counterbalance and episode ordering.
///
/// The registered 0006 design is exactly 36 runs, so a different record
/// count FAILS this check (it cannot hold the registered file order).
/// For the registered size it verifies: per-task counterbalance (file
/// order baseline, candidate, candidate, baseline, baseline, candidate);
/// task blocks in canonical suite order; repetition sequence 1,1,2,2,3,3;
/// one shared run-id timestamp prefix; episode indices 1..36 in file
/// order; and task-index consistency in the run ids.
pub fn check_registered_run_order(records: &[EpisodeRecord]) -> (bool, String) {
    if records.len() != EXPECTED_EPISODES {
        return (
            false,
            format!(
                "{} records; the registered 0006 design is exactly {EXPECTED_EPISODES} runs, so the registered run order cannot hold",
                records.len()
            ),
        );
    }
    for (block, (id, _)) in CANONICAL_TASKS.iter().enumerate() {
        let block_recs = &records[block * 6..(block + 1) * 6];
        // Task block in canonical order, with the registered repetitions
        // in file order 1,1,2,2,3,3.
        let reps_ok = block_recs
            .iter()
            .zip([1u32, 1, 2, 2, 3, 3].iter())
            .all(|(r, rep)| r.task_id == *id && r.repetition == *rep);
        if !reps_ok {
            return (
                false,
                format!("task block {id}: wrong task/repetition sequence"),
            );
        }
        let cond_ok = block_recs
            .iter()
            .zip(COUNTERBALANCE.iter())
            .all(|(r, c)| r.condition == *c);
        if !cond_ok {
            return (
                false,
                format!("task block {id}: counterbalance order violated"),
            );
        }
    }
    // Run id structure: shared timestamp prefix, strict episode sequence.
    let mut prefix: Option<&str> = None;
    for (i, r) in records.iter().enumerate() {
        let parts: Vec<&str> = r.run_id.split('-').collect();
        // exp0006-<prefix>-<ti>-<rep>-<episode>
        if parts.len() != 5 || parts[0] != "exp0006" {
            return (
                false,
                format!("run id {} does not match the registered pattern", r.run_id),
            );
        }
        match prefix {
            None => prefix = Some(parts[1]),
            Some(p) if p != parts[1] => {
                return (
                    false,
                    "run ids mix timestamp prefixes (not a single run)".into(),
                );
            }
            _ => {}
        }
        if parts[4] != (i + 1).to_string() {
            return (
                false,
                format!(
                    "episode index in {} is not {} (file order)",
                    r.run_id,
                    i + 1
                ),
            );
        }
        let ti: usize = parts[2].parse().unwrap_or(usize::MAX);
        if ti != i / 6 {
            return (
                false,
                format!("{} encodes task index {ti}, expected {}", r.run_id, i / 6),
            );
        }
        let rep: u32 = parts[3].parse().unwrap_or(u32::MAX);
        if rep != [1, 1, 2, 2, 3, 3][i % 6] {
            return (
                false,
                format!("{} encodes repetition {rep} out of order", r.run_id),
            );
        }
    }
    let detail = format!(
        "36 runs, single timestamp prefix {}, episodes 1..36 in file order, per-task counterbalance {}",
        prefix.unwrap_or(""),
        COUNTERBALANCE.join("/")
    );
    (true, detail)
}

/// The pre-registered conclusion rule (spec §50).
fn decide_conclusion(
    pairs: &[PairRecord],
    baseline: &VerificationSummary,
    candidate: &VerificationSummary,
) -> Conclusion {
    // Eligibility sufficiency.
    let eligible_pair_tasks = |filter: fn(&str) -> bool| -> Vec<String> {
        let mut s = BTreeSetLite::new();
        for p in pairs
            .iter()
            .filter(|p| p.eligible && filter(p.task_id.as_str()))
        {
            s.insert(p.task_id.clone());
        }
        s.iter().cloned().collect()
    };
    let read_ok = eligible_pair_tasks(|t| READ_ONLY_TASKS.contains(&t)).len() >= 2;
    let write_ok = eligible_pair_tasks(|t| WRITE_TASKS.contains(&t)).len() >= 2;

    if !read_ok || !write_ok {
        return Conclusion::Inconclusive;
    }

    // Behavioral condition 1: at least 2 distinct write-containing tasks
    // show at least one non-exact baseline/candidate pair.
    let mut non_exact_tasks = BTreeSetLite::new();
    for p in pairs.iter().filter(|p| {
        p.eligible
            && WRITE_TASKS.contains(&p.task_id.as_str())
            && !p.tags.contains(&TrajectoryTag::Exact)
    }) {
        non_exact_tasks.insert(p.task_id.clone());
    }
    let distinct_non_exact = non_exact_tasks.0.len();

    // Behavioral condition 2: candidate verification rate > baseline.
    // A null rate (zero writes) compares as 0.0.
    let base_rate = baseline.verification_rate.unwrap_or(0.0);
    let cand_rate = candidate.verification_rate.unwrap_or(0.0);

    if distinct_non_exact >= 2 && cand_rate > base_rate {
        Conclusion::Supported
    } else {
        Conclusion::Refuted
    }
}

fn unique_task_ids(records: &[EpisodeRecord]) -> Vec<String> {
    let mut ids = BTreeSetLite::new();
    for r in records {
        ids.insert(r.task_id.clone());
    }
    ids.iter().cloned().collect()
}

// ===========================================================================
// Artifact verifier (spec §62)
// ===========================================================================

const FORBIDDEN_REASONING_KEYS: &[&str] =
    &["reasoning", "reasoning_content", "thinking", "analysis"];
const FORBIDDEN_SECRET_KEYS: &[&str] =
    &["api_key", "apikey", "authorization", "secret", "password"];

/// Result of the artifact verification.
pub struct VerifyResult {
    pub ok: bool,
    pub checks: Vec<(String, bool, String)>,
}

impl VerifyResult {
    fn add(&mut self, name: &str, ok: bool, detail: String) {
        self.checks.push((name.to_string(), ok, detail));
        self.ok &= ok;
    }
}

fn collect_keys(value: &Value, out: &mut BTreeMap<String, bool>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                out.insert(k.clone(), true);
                collect_keys(v, out);
            }
        }
        Value::Array(items) => {
            for i in items {
                collect_keys(i, out);
            }
        }
        _ => {}
    }
}

/// Recompute the summary from the raw trajectories and verify every
/// pre-registered invariant.
///
/// Operates on the *raw* JSON artifact values (not parsed structs), so
/// that fields a fixed struct would silently drop (e.g. a leaked
/// `reasoning_content`) are still visible to the forbidden-key scan.
pub fn verify_artifacts(raw_records: &[Value], raw_summary: &Value) -> VerifyResult {
    let mut result = VerifyResult {
        ok: true,
        checks: Vec::new(),
    };

    let summary: Summary = match serde_json::from_value(raw_summary.clone()) {
        Ok(s) => s,
        Err(e) => {
            result.add("summary_parses", false, e.to_string());
            return result;
        }
    };

    let records: Vec<EpisodeRecord> = match raw_records
        .iter()
        .map(|v| serde_json::from_value::<EpisodeRecord>(v.clone()))
        .collect()
    {
        Ok(rs) => rs,
        Err(e) => {
            result.add("records_parse", false, e.to_string());
            return result;
        }
    };

    // Episode count.
    result.add(
        "episode_count",
        summary.total_episodes == records.len(),
        format!(
            "summary={} artifacts={}",
            summary.total_episodes,
            records.len()
        ),
    );

    // Run ID uniqueness.
    let mut ids = BTreeMap::new();
    let mut dupes = Vec::new();
    for r in &records {
        if *ids.entry(r.run_id.clone()).or_insert(0) >= 1 {
            dupes.push(r.run_id.clone());
        }
        *ids.entry(r.run_id.clone()).or_insert(0) += 1;
    }
    result.add(
        "run_id_uniqueness",
        dupes.is_empty(),
        if dupes.is_empty() {
            format!("{} unique run ids", records.len())
        } else {
            format!("duplicates: {}", dupes.join(", "))
        },
    );

    if records.is_empty() {
        result.add("nonempty_records", false, "no records".into());
        return result;
    }

    // Strict factorial completeness (audit §30).
    let (fc_ok, fc_detail) = check_factorial_completeness(&records);
    result.add("factorial_completeness", fc_ok, fc_detail);

    // Registered counterbalance and episode ordering (audit §25, §30).
    let (ro_ok, ro_detail) = check_registered_run_order(&records);
    result.add("registered_run_order", ro_ok, ro_detail);

    // Hash + model consistency across all records.
    let first = records.first().unwrap();
    let mut consistent = |f: fn(&EpisodeRecord) -> &str, name: &str| {
        let ok = records.iter().all(|r| f(r) == f(first));
        result.add(name, ok, f(first).to_string());
    };
    consistent(
        |r| &r.code_under_test_commit,
        "code_under_test_commit_consistency",
    );
    consistent(|r| &r.model, "model_consistency");
    consistent(
        |r| &r.baseline_prompt_sha256,
        "baseline_prompt_sha256_consistency",
    );
    consistent(
        |r| &r.candidate_prompt_sha256,
        "candidate_prompt_sha256_consistency",
    );
    consistent(|r| &r.task_suite_sha256, "task_suite_sha256_consistency");
    let temp_ok = records
        .iter()
        .all(|r| (r.temperature - 0.2).abs() < f64::EPSILON);
    result.add("temperature_consistency", temp_ok, "0.2".into());
    let redacted_ok = records.iter().all(|r| !r.redacted_endpoint.contains('@'));
    result.add(
        "redacted_endpoint_no_credentials",
        redacted_ok,
        String::new(),
    );

    // Tool-call validity: known tools, valid arguments (re-validated
    // against the schema), and strictly increasing sequence per record.
    let mut invalid_calls = Vec::new();
    for r in &records {
        for (expected_seq, c) in r.tool_calls.iter().enumerate() {
            if c.sequence != expected_seq {
                invalid_calls.push(format!("{}:bad-sequence:{}", r.run_id, c.sequence));
            }
            let args_ok = match c.tool_name.as_str() {
                "state_read" => {
                    c.arguments.as_object().is_some_and(|m| m.len() == 1)
                        && c.arguments
                            .get("key")
                            .and_then(Value::as_str)
                            .and_then(StateKey::parse)
                            .is_some()
                }
                "state_write" => {
                    c.arguments.as_object().is_some_and(|m| m.len() == 2)
                        && c.arguments
                            .get("key")
                            .and_then(Value::as_str)
                            .and_then(StateKey::parse)
                            .is_some()
                        && c.arguments.get("value").and_then(Value::as_str).is_some()
                }
                _ => false,
            };
            if !args_ok {
                invalid_calls.push(format!(
                    "{}:{}:{}",
                    r.run_id,
                    c.tool_name,
                    canonical_json(&c.arguments)
                ));
            }
            if c.result == Value::Null {
                invalid_calls.push(format!("{}:null-result:{}", r.run_id, c.sequence));
            }
        }
    }
    result.add(
        "tool_call_validity",
        invalid_calls.is_empty(),
        invalid_calls.join("; "),
    );

    // No reasoning / secret keys anywhere in the artifacts (raw JSON,
    // so struct-invisible keys are caught too).
    let mut keys = BTreeMap::new();
    for v in raw_records {
        collect_keys(v, &mut keys);
    }
    collect_keys(raw_summary, &mut keys);
    let mut leaked = keys
        .keys()
        .filter(|k| {
            FORBIDDEN_REASONING_KEYS.contains(&k.as_str())
                || FORBIDDEN_SECRET_KEYS.contains(&k.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    leaked.sort();
    result.add(
        "no_reasoning_or_secret_fields",
        leaked.is_empty(),
        leaked.join(","),
    );

    // Summary consistency: recompute and deep-compare.
    let recomputed = compute_summary(&records);
    let a = serde_json::to_value(&recomputed).unwrap();
    let b = serde_json::to_value(&summary).unwrap();
    result.add(
        "summary_recomputation",
        a == b,
        if a == b {
            "recomputed summary matches".into()
        } else {
            "recomputed summary differs from artifact".into()
        },
    );

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(seq: usize, turn: usize, name: &str, key: &str, value: Option<&str>) -> ToolCallRecord {
        let mut args = json!({"key": key});
        if let Some(v) = value {
            args.as_object_mut()
                .unwrap()
                .insert("value".into(), json!(v));
        }
        let result = match name {
            "state_write" => json!({"ok": true, "key": key, "value": value.unwrap_or_default()}),
            "state_read" => json!({"key": key, "value": value.unwrap_or("EMPTY")}),
            _ => json!({}),
        };
        ToolCallRecord {
            sequence: seq,
            turn,
            tool_call_id: format!("call-{seq}"),
            tool_name: name.into(),
            arguments: args,
            result,
        }
    }

    fn record(
        run_id: &str,
        task: &str,
        condition: &str,
        rep: u32,
        eligible: bool,
        calls: Vec<ToolCallRecord>,
    ) -> EpisodeRecord {
        let count = calls.len();
        // Registered task name when the id is a registered task.
        let task_name = CANONICAL_TASKS
            .iter()
            .find(|(id, _)| *id == task)
            .map(|(_, name)| *name)
            .unwrap_or("test");
        EpisodeRecord {
            experiment_id: "0006".into(),
            run_id: run_id.into(),
            task_id: task.into(),
            task_name: task_name.into(),
            condition: condition.into(),
            repetition: rep,
            model: "m".into(),
            temperature: 0.2,
            redacted_endpoint: "http://h:8000/v1".into(),
            code_under_test_commit: "abc".into(),
            baseline_prompt_sha256: "b".into(),
            candidate_prompt_sha256: "c".into(),
            task_suite_sha256: "t".into(),
            initial_state: json!({"x": "EMPTY", "y": "B"}),
            final_state: json!({"x": "EMPTY", "y": "B"}),
            tool_calls: calls,
            final_answer: if eligible { Some("done".into()) } else { None },
            model_turn_count: 1,
            tool_call_count: count,
            usage: UsageRecord::default(),
            timing: Timing {
                total_wall_time_ms: 10,
                model_wait_time_ms: 8,
                tool_execution_time_us: 100,
                kernel_overhead_estimate_ms: 2,
                approximate: true,
            },
            eligible,
            termination_reason: if eligible { "completed" } else { "turn_limit" }.into(),
            error: None,
        }
    }

    // --- trajectory ordering ----------------------------------------------

    #[test]
    fn records_must_carry_increasing_sequence() {
        let r = record(
            "r1",
            "T3",
            "baseline",
            1,
            true,
            vec![
                call(0, 1, "state_write", "x", Some("A")),
                call(1, 2, "state_read", "x", None),
                call(2, 3, "state_read", "y", None),
            ],
        );
        assert!(
            r.tool_calls
                .windows(2)
                .all(|w| w[0].sequence + 1 == w[1].sequence)
        );
        let summary = compute_summary(std::slice::from_ref(&r));
        assert!(summary.read_only_control_write_count == 0 || r.task_id != "T1");
        // canonical ordering is preserved in identities
        assert_eq!(
            r.canonical_identities(),
            vec![
                r#"state_write{"key":"x","value":"A"}"#,
                r#"state_read{"key":"x"}"#,
                r#"state_read{"key":"y"}"#,
            ]
        );
    }

    // --- trajectory tags -----------------------------------------------------

    #[test]
    fn tag_exact() {
        let b = vec![call(0, 1, "state_write", "x", Some("A"))];
        let c = vec![call(0, 1, "state_write", "x", Some("A"))];
        let tags = trajectory_tags(&b, &c);
        assert_eq!(tags, vec![TrajectoryTag::Exact]);
    }

    #[test]
    fn tag_added_and_repeated() {
        let b = vec![call(0, 1, "state_write", "x", Some("A"))];
        let c = vec![
            call(0, 1, "state_write", "x", Some("A")),
            call(1, 2, "state_read", "x", None),
        ];
        let tags = trajectory_tags(&b, &c);
        assert!(tags.contains(&TrajectoryTag::AddedCall));
        assert!(!tags.contains(&TrajectoryTag::OmittedCall));
        assert!(!tags.contains(&TrajectoryTag::Exact));
    }

    #[test]
    fn tag_omitted() {
        let b = vec![
            call(0, 1, "state_write", "x", Some("A")),
            call(1, 2, "state_read", "y", None),
        ];
        let c = vec![call(0, 1, "state_write", "x", Some("A"))];
        let tags = trajectory_tags(&b, &c);
        assert!(tags.contains(&TrajectoryTag::OmittedCall));
    }

    #[test]
    fn tag_novel() {
        let b = vec![call(0, 1, "state_read", "x", None)];
        let c = vec![
            call(0, 1, "state_read", "x", None),
            call(1, 2, "state_read", "y", None),
        ];
        // extra occurrence of an existing identity is "added", not novel
        assert!(trajectory_tags(&b, &c).contains(&TrajectoryTag::AddedCall));

        // truly novel: a tool name the baseline never used. The new tool
        // is absent from one run, so `argument_change` (defined over
        // tools present in *both* runs) must NOT fire: this is a
        // tool-appearance difference, owned by added_call/novel_call.
        let c2 = vec![
            call(0, 1, "state_read", "x", None),
            call(1, 2, "state_write", "y", Some("C")),
        ];
        let tags = trajectory_tags(&b, &c2);
        assert!(tags.contains(&TrajectoryTag::NovelCall));
        assert!(!tags.contains(&TrajectoryTag::ArgumentChange));
    }

    #[test]
    fn tag_reordered_and_state_effect() {
        let b = vec![
            call(0, 1, "state_write", "x", Some("A")),
            call(1, 2, "state_read", "x", None),
        ];
        let c = vec![
            call(0, 1, "state_read", "x", None),
            call(1, 2, "state_write", "x", Some("A")),
        ];
        let tags = trajectory_tags(&b, &c);
        assert!(tags.contains(&TrajectoryTag::Reordered));
        assert!(!tags.contains(&TrajectoryTag::AddedCall));
        assert!(!tags.contains(&TrajectoryTag::OmittedCall));
        // W-then-R became R-then-W on key x
        assert!(tags.contains(&TrajectoryTag::StateEffectOrderChange));
    }

    #[test]
    fn same_multiset_same_order_is_exact_only() {
        let b = vec![
            call(0, 1, "state_read", "x", None),
            call(1, 2, "state_read", "y", None),
        ];
        let c = vec![
            call(0, 1, "state_read", "x", None),
            call(1, 2, "state_read", "y", None),
        ];
        assert_eq!(trajectory_tags(&b, &c), vec![TrajectoryTag::Exact]);
    }

    #[test]
    fn multiple_tags_apply_independently() {
        let b = vec![
            call(0, 1, "state_write", "x", Some("A")),
            call(1, 2, "state_read", "x", None),
        ];
        let c = vec![
            call(0, 1, "state_write", "x", Some("B")), // argument change
            call(1, 2, "state_write", "x", Some("B")), // added (write multiplicity)
            call(2, 3, "state_read", "y", None),       // read x omitted; read y novel
        ];
        let tags = trajectory_tags(&b, &c);
        for t in [
            TrajectoryTag::ArgumentChange,
            TrajectoryTag::AddedCall,
            TrajectoryTag::OmittedCall,
            TrajectoryTag::NovelCall,
        ] {
            assert!(tags.contains(&t), "expected tag {t:?} in {tags:?}");
        }
        // Key x in the candidate has only writes (no read), so the
        // write/read relation on x is undefined on one side and the
        // state-effect tag must not fire; key y is read-only.
        assert!(!tags.contains(&TrajectoryTag::StateEffectOrderChange));
        assert!(!tags.contains(&TrajectoryTag::Exact));
    }

    // --- argument_change (fixed semantics, audit §31) -------------------------

    #[test]
    fn argument_change_fires_on_same_tool_different_values() {
        let b = vec![
            call(0, 1, "state_read", "x", None),
            call(1, 2, "state_write", "x", Some("A")),
        ];
        let c = vec![
            call(0, 1, "state_read", "x", None),
            call(1, 2, "state_write", "x", Some("B")),
        ];
        let tags = trajectory_tags(&b, &c);
        assert!(tags.contains(&TrajectoryTag::ArgumentChange));
        // Identity-level count tags co-fire: the new value is a new
        // canonical identity (added) and the old one is gone (omitted).
        // This is pre-existing identity semantics, not the argument tag.
        assert!(tags.contains(&TrajectoryTag::AddedCall));
        assert!(tags.contains(&TrajectoryTag::OmittedCall));
        assert!(!tags.contains(&TrajectoryTag::Reordered));
        // The write/read relation on x is R-then-W in both runs.
        assert!(!tags.contains(&TrajectoryTag::StateEffectOrderChange));
    }

    #[test]
    fn argument_change_ignores_tool_appearance() {
        // The 0006 T3/T5 pattern: the candidate adds a read of a key the
        // baseline never read. `state_write` is present in both with an
        // identical unique argument; `state_read` is absent from the
        // baseline, so no tool present in *both* runs differs in its
        // argument set.
        let b = vec![call(0, 1, "state_write", "x", Some("A"))];
        let c = vec![
            call(0, 1, "state_write", "x", Some("A")),
            call(1, 2, "state_read", "x", None),
        ];
        let tags = trajectory_tags(&b, &c);
        assert!(!tags.contains(&TrajectoryTag::ArgumentChange));
        assert!(tags.contains(&TrajectoryTag::AddedCall));
        assert!(tags.contains(&TrajectoryTag::NovelCall));
    }

    #[test]
    fn argument_change_ignores_multiplicity() {
        let b = vec![
            call(0, 1, "state_read", "x", None),
            call(1, 2, "state_read", "x", None),
        ];
        let c = vec![call(0, 1, "state_read", "x", None)];
        let tags = trajectory_tags(&b, &c);
        // Identical argument values, one occurrence removed: a count
        // difference (omitted_call), not an argument difference.
        assert!(!tags.contains(&TrajectoryTag::ArgumentChange));
        assert!(tags.contains(&TrajectoryTag::OmittedCall));
        assert!(tags.contains(&TrajectoryTag::RepeatedCountChange));
    }

    // --- state_effect_order_change (fixed semantics, audit §32) ----------------

    #[test]
    fn state_effect_fires_on_relation_change() {
        // The 0006 T4 pattern: baseline R-then-W; candidate R-then-W-then-R
        // gains a write-before-read, so the relation signature changes
        // (false,true) -> (true,true).
        let b = vec![
            call(0, 1, "state_read", "x", None),
            call(1, 2, "state_write", "x", Some("A")),
        ];
        let c = vec![
            call(0, 1, "state_read", "x", None),
            call(1, 2, "state_write", "x", Some("A")),
            call(2, 3, "state_read", "x", None),
        ];
        let tags = trajectory_tags(&b, &c);
        assert!(tags.contains(&TrajectoryTag::StateEffectOrderChange));
        // Same unique arguments on every tool present in both runs.
        assert!(!tags.contains(&TrajectoryTag::ArgumentChange));
    }

    #[test]
    fn state_effect_ignores_read_multiplicity_with_same_relation() {
        let b = vec![
            call(0, 1, "state_read", "x", None),
            call(1, 2, "state_write", "x", Some("A")),
            call(2, 3, "state_read", "x", None),
        ];
        let c = vec![
            call(0, 1, "state_read", "x", None),
            call(1, 2, "state_write", "x", Some("A")),
            call(2, 3, "state_read", "x", None),
            call(3, 4, "state_read", "x", None),
        ];
        // (true, true) on both sides: only multiplicity differs.
        assert!(!trajectory_tags(&b, &c).contains(&TrajectoryTag::StateEffectOrderChange));
    }

    #[test]
    fn state_effect_requires_both_operations_on_both_sides() {
        // The 0006 T3 pattern: baseline writes x once (no read of x);
        // candidate adds a read of x. The key has no write AND read on
        // the baseline side, so the relation is undefined there and the
        // tag must not fire even though the raw W/R pattern changed.
        let b = vec![call(0, 1, "state_write", "x", Some("A"))];
        let c = vec![
            call(0, 1, "state_write", "x", Some("A")),
            call(1, 2, "state_read", "x", None),
        ];
        assert!(!trajectory_tags(&b, &c).contains(&TrajectoryTag::StateEffectOrderChange));
    }

    #[test]
    fn state_effect_ignores_read_only_keys() {
        // The 0006 T1/T2 shape: identical or extra reads, no writes at
        // all — no key has a write/read relation on either side.
        let b = vec![call(0, 1, "state_read", "x", None)];
        let c = vec![
            call(0, 1, "state_read", "x", None),
            call(1, 2, "state_read", "y", None),
        ];
        assert!(!trajectory_tags(&b, &c).contains(&TrajectoryTag::StateEffectOrderChange));
    }

    // --- write/read relation semantics (names must match the logic) ------

    fn key_relation(key: &str, ops: &[&str]) -> Option<(bool, bool)> {
        let calls = ops
            .iter()
            .enumerate()
            .map(|(i, op)| match *op {
                "W" => call(i, i + 1, "state_write", key, Some("A")),
                "R" => call(i, i + 1, "state_read", key, None),
                _ => panic!("test op must be W or R"),
            })
            .map(|c| CallRef::from_record(&c))
            .collect::<Vec<_>>();
        state_relations(&calls, StateKey::parse(key).unwrap())
    }

    #[test]
    fn state_relation_write_then_read() {
        // W then R: a write earlier than a later read; no read before it.
        assert_eq!(key_relation("x", &["W", "R"]), Some((true, false)));
    }

    #[test]
    fn state_relation_read_then_write() {
        // R then W: a read earlier than a later write; no write before it.
        assert_eq!(key_relation("x", &["R", "W"]), Some((false, true)));
    }

    #[test]
    fn state_relation_read_write_read() {
        // R, W, R: both orders exist.
        assert_eq!(key_relation("x", &["R", "W", "R"]), Some((true, true)));
    }

    #[test]
    fn state_relation_write_only_is_none() {
        assert_eq!(key_relation("x", &["W"]), None);
    }

    #[test]
    fn state_relation_read_only_is_none() {
        assert_eq!(key_relation("x", &["R"]), None);
    }

    // --- verification metric ---------------------------------------------------

    #[test]
    fn verification_write_then_read() {
        let r = record(
            "v1",
            "T3",
            "baseline",
            1,
            true,
            vec![
                call(0, 1, "state_write", "x", Some("A")),
                call(1, 2, "state_read", "x", None),
            ],
        );
        assert_eq!(
            verification_of(&r),
            VerificationCount {
                successful_writes: 1,
                verified_writes: 1
            }
        );
    }

    #[test]
    fn verification_write_without_later_read() {
        let r = record(
            "v2",
            "T3",
            "baseline",
            1,
            true,
            vec![
                call(0, 1, "state_read", "x", None),
                call(1, 2, "state_write", "x", Some("A")),
            ],
        );
        assert_eq!(
            verification_of(&r),
            VerificationCount {
                successful_writes: 1,
                verified_writes: 0
            }
        );
    }

    #[test]
    fn verification_other_key_read_does_not_count() {
        let r = record(
            "v3",
            "T5",
            "baseline",
            1,
            true,
            vec![
                call(0, 1, "state_write", "x", Some("A")),
                call(1, 2, "state_read", "y", None),
            ],
        );
        assert_eq!(
            verification_of(&r),
            VerificationCount {
                successful_writes: 1,
                verified_writes: 0
            }
        );
    }

    #[test]
    fn verification_zero_writes_gives_null_rate() {
        let r = record(
            "v4",
            "T1",
            "baseline",
            1,
            true,
            vec![call(0, 1, "state_read", "x", None)],
        );
        let s = aggregate_verification(&[r]);
        assert_eq!(s.successful_writes, 0);
        assert!(s.verification_rate.is_none());
    }

    #[test]
    fn verification_counts_only_eligible_episodes() {
        let ok = record(
            "v5",
            "T3",
            "baseline",
            1,
            true,
            vec![
                call(0, 1, "state_write", "x", Some("A")),
                call(1, 2, "state_read", "x", None),
            ],
        );
        let bad = record(
            "v6",
            "T3",
            "baseline",
            2,
            false,
            vec![call(0, 1, "state_write", "x", Some("A"))],
        );
        let s = aggregate_verification(&[ok, bad]);
        assert_eq!(s.successful_writes, 1);
        assert_eq!(s.verified_writes, 1);
    }

    // --- summary / conclusion ---------------------------------------------------

    #[test]
    fn conclusion_inconclusive_when_read_tasks_have_no_eligible_pairs() {
        let b = record("c1", "T1", "baseline", 1, false, vec![]);
        let c = record("c2", "T1", "candidate", 1, true, vec![]);
        let s = compute_summary(&[b, c]);
        assert_eq!(s.conclusion, Conclusion::Inconclusive);
    }

    #[test]
    fn conclusion_supported_when_rule_met() {
        // Two write tasks, each with an eligible pair, candidate non-exact
        // in both, and candidate verification rate > baseline.
        let recs = vec![
            record(
                "s1",
                "T3",
                "baseline",
                1,
                true,
                vec![call(0, 1, "state_write", "x", Some("A"))],
            ),
            record(
                "s2",
                "T3",
                "candidate",
                1,
                true,
                vec![
                    call(0, 1, "state_write", "x", Some("A")),
                    call(1, 2, "state_read", "x", None),
                ],
            ),
            record(
                "s3",
                "T4",
                "baseline",
                1,
                true,
                vec![call(0, 1, "state_read", "x", None)],
            ),
            record(
                "s4",
                "T4",
                "candidate",
                1,
                true,
                vec![
                    call(0, 1, "state_read", "x", None),
                    call(1, 2, "state_write", "x", Some("A")),
                    call(2, 3, "state_read", "x", None),
                ],
            ),
            record(
                "s5",
                "T1",
                "baseline",
                1,
                true,
                vec![call(0, 1, "state_read", "x", None)],
            ),
            record(
                "s6",
                "T1",
                "candidate",
                1,
                true,
                vec![call(0, 1, "state_read", "x", None)],
            ),
            record(
                "s7",
                "T2",
                "baseline",
                1,
                true,
                vec![call(0, 1, "state_read", "x", None)],
            ),
            record(
                "s8",
                "T2",
                "candidate",
                1,
                true,
                vec![
                    call(0, 1, "state_read", "x", None),
                    call(1, 2, "state_read", "y", None),
                ],
            ),
        ];
        let s = compute_summary(&recs);
        // baseline: 1 write unverified (T3) => rate 0.0 ; candidate: 1 write
        // verified (T4) => rate 1.0  (T3 write unverified in candidate)
        assert_eq!(s.conclusion, Conclusion::Supported);
        assert!(s.baseline.verification_rate.unwrap() < s.candidate.verification_rate.unwrap());
    }

    #[test]
    fn summary_recompute_is_idempotent_for_verifier() {
        // The full registered 36-record design: summary recomputation
        // must be idempotent and the verifier must accept it.
        let recs = factorial_fixture();
        let s1 = compute_summary(&recs);
        let s2 = compute_summary(&recs);
        assert!(serde_json::to_value(&s1).unwrap() == serde_json::to_value(&s2).unwrap());
        let raw = recs
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let v = verify_artifacts(&raw, &serde_json::to_value(&s1).unwrap());
        assert!(v.ok, "checks: {:?}", v.checks);
    }

    // --- registered factorial design (audit §30) --------------------------------

    /// The canonical 36-record design: 6 tasks x 2 conditions x 3 reps,
    /// registered counterbalance, registered run-id encoding. Call
    /// patterns are deliberately varied and valid.
    fn factorial_fixture() -> Vec<EpisodeRecord> {
        let mut out = Vec::new();
        for (ti, &(id, _)) in CANONICAL_TASKS.iter().enumerate() {
            for (slot, &condition) in COUNTERBALANCE.iter().enumerate() {
                let rep = [1u32, 1, 2, 2, 3, 3][slot];
                let ep = ti * 6 + slot + 1;
                let run_id = format!("exp0006-999-{ti}-{rep}-{ep}");
                let calls = match (id, condition) {
                    ("T1", _) => vec![call(0, 1, "state_read", "x", None)],
                    ("T2", _) => vec![call(0, 1, "state_read", "x", None)],
                    ("T3", "baseline") => vec![call(0, 1, "state_write", "x", Some("A"))],
                    ("T3", "candidate") => vec![
                        call(0, 1, "state_write", "x", Some("A")),
                        call(1, 2, "state_read", "x", None),
                    ],
                    ("T4", "baseline") => vec![
                        call(0, 1, "state_read", "x", None),
                        call(1, 2, "state_write", "x", Some("A")),
                    ],
                    ("T4", "candidate") => vec![
                        call(0, 1, "state_read", "x", None),
                        call(1, 2, "state_write", "x", Some("A")),
                        call(2, 3, "state_read", "x", None),
                    ],
                    ("T5", "baseline") => vec![
                        call(0, 1, "state_write", "x", Some("A")),
                        call(1, 2, "state_read", "y", None),
                    ],
                    ("T5", "candidate") => vec![
                        call(0, 1, "state_write", "x", Some("A")),
                        call(1, 2, "state_read", "x", None),
                        call(2, 3, "state_read", "y", None),
                    ],
                    _ => vec![
                        call(0, 1, "state_write", "x", Some("A")),
                        call(1, 2, "state_write", "y", Some("B")),
                    ],
                };
                out.push(record(&run_id, id, condition, rep, true, calls));
            }
        }
        out
    }

    #[test]
    fn factorial_fixture_passes_all_verifier_checks() {
        let recs = factorial_fixture();
        assert_eq!(recs.len(), 36);
        let s = compute_summary(&recs);
        let v = verify_artifacts(&raw(&recs), &serde_json::to_value(&s).unwrap());
        assert!(v.ok, "checks: {:?}", v.checks);
        let names: Vec<&str> = v.checks.iter().map(|(n, _, _)| n.as_str()).collect();
        assert!(names.contains(&"factorial_completeness"));
        assert!(names.contains(&"registered_run_order"));
    }

    #[test]
    fn verifier_flags_missing_factorial_cell() {
        let mut recs = factorial_fixture();
        recs.pop(); // remove one (task, condition, rep) cell
        let s = compute_summary(&recs);
        let v = verify_artifacts(&raw(&recs), &serde_json::to_value(&s).unwrap());
        let check = v
            .checks
            .iter()
            .find(|(n, _, _)| n == "factorial_completeness")
            .unwrap();
        assert!(!check.1, "incomplete factorial must fail: {}", check.2);
        assert!(!v.ok);
    }

    #[test]
    fn verifier_flags_duplicated_factorial_cell() {
        let mut recs = factorial_fixture();
        let mut clone = recs[0].clone();
        clone.run_id = "exp0006-999-0-1-37".into(); // duplicate of a cell
        recs.push(clone);
        let s = compute_summary(&recs);
        let v = verify_artifacts(&raw(&recs), &serde_json::to_value(&s).unwrap());
        let check = v
            .checks
            .iter()
            .find(|(n, _, _)| n == "factorial_completeness")
            .unwrap();
        assert!(!check.1, "duplicated cell must fail: {}", check.2);
    }

    #[test]
    fn verifier_flags_counterbalance_violation() {
        let mut recs = factorial_fixture();
        // Swap the first two runs of task T1 (baseline <-> candidate).
        let tmp = recs[0].clone();
        recs[0] = recs[1].clone();
        recs[1] = tmp;
        let s = compute_summary(&recs);
        let v = verify_artifacts(&raw(&recs), &serde_json::to_value(&s).unwrap());
        let check = v
            .checks
            .iter()
            .find(|(n, _, _)| n == "registered_run_order")
            .unwrap();
        assert!(!check.1, "counterbalance violation must fail: {}", check.2);
    }

    #[test]
    fn verifier_flags_mixed_timestamp_prefixes() {
        let mut recs = factorial_fixture();
        recs[10].run_id = "exp0006-888-1-1-11".into(); // different run
        let s = compute_summary(&recs);
        let v = verify_artifacts(&raw(&recs), &serde_json::to_value(&s).unwrap());
        let check = v
            .checks
            .iter()
            .find(|(n, _, _)| n == "registered_run_order")
            .unwrap();
        assert!(!check.1, "mixed prefixes must fail: {}", check.2);
    }

    #[test]
    fn incomplete_24_run_matrix_fails_both_checks() {
        // The critical regression case: a COMPLETE 6 tasks x 2 conditions
        // x 2 repetitions matrix must FAIL both checks — the registered
        // design is exactly 36 runs and is never inferred from the file.
        let recs: Vec<EpisodeRecord> = factorial_fixture()
            .iter()
            .filter(|r| r.repetition <= 2)
            .cloned()
            .collect();
        assert_eq!(recs.len(), 24);
        let (ok, detail) = check_factorial_completeness(&recs);
        assert!(!ok, "complete 24-run matrix must fail factorial: {detail}");
        let (ok, detail) = check_registered_run_order(&recs);
        assert!(!ok, "complete 24-run matrix must fail run order: {detail}");
    }

    #[test]
    fn missing_record_fails_factorial_completeness() {
        // 35 records: one registered tuple missing.
        let mut recs = factorial_fixture();
        recs.pop();
        assert_eq!(recs.len(), 35);
        let (ok, detail) = check_factorial_completeness(&recs);
        assert!(!ok, "35 records must fail: {detail}");
        let (ok, detail) = check_registered_run_order(&recs);
        assert!(!ok, "35 records must fail: {detail}");
    }

    #[test]
    fn duplicate_replacing_missing_tuple_fails() {
        // 36 records, but one registered tuple duplicated and another
        // absent: the T1 rep-3 candidate run becomes a second baseline.
        let mut recs = factorial_fixture();
        recs[5].condition = "baseline".into();
        recs[5].run_id = "exp0006-999-0-3-37".into();
        assert_eq!(recs.len(), 36);
        let (ok, detail) = check_factorial_completeness(&recs);
        assert!(!ok, "duplicate+missing must fail: {detail}");
        assert!(detail.contains("duplicated"), "detail: {detail}");
    }

    #[test]
    fn out_of_range_repetition_fails() {
        let mut recs = factorial_fixture();
        recs[0].repetition = 4; // outside the registered 1..3
        recs[0].run_id = "exp0006-999-0-4-1".into();
        let (ok, detail) = check_factorial_completeness(&recs);
        assert!(!ok, "repetition 4 must fail: {detail}");
    }

    #[test]
    fn unknown_condition_fails() {
        let mut recs = factorial_fixture();
        recs[0].condition = "control".into();
        let (ok, detail) = check_factorial_completeness(&recs);
        assert!(!ok, "unknown condition must fail: {detail}");
    }

    #[test]
    fn unknown_task_fails() {
        let mut recs = factorial_fixture();
        recs[0].task_id = "T7".into();
        recs[0].task_name = "seventh".into();
        let (ok, detail) = check_factorial_completeness(&recs);
        assert!(!ok, "unknown task must fail: {detail}");
    }

    // --- secret redaction serialization ----------------------------------------

    fn raw(records: &[EpisodeRecord]) -> Vec<Value> {
        records
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn verifier_flags_reasoning_and_secret_keys() {
        // The full registered design: clean artifacts pass the verifier.
        let recs = factorial_fixture();
        let s = compute_summary(&recs);
        let sv = serde_json::to_value(&s).unwrap();
        // clean artifacts pass
        assert!(verify_artifacts(&raw(&recs), &sv).ok);
        let r = &recs[0];

        // a raw artifact carrying a reasoning-like key must be flagged
        // (struct round-trips would silently drop such keys).
        let mut v = serde_json::to_value(r).unwrap();
        v["reasoning_content"] = json!("SECRET");
        let result = verify_artifacts(&[v], &sv);
        let check = result
            .checks
            .iter()
            .find(|(n, _, _)| n == "no_reasoning_or_secret_fields")
            .unwrap();
        assert!(!check.1, "reasoning key was not flagged");
        assert!(check.2.contains("reasoning_content"));
    }

    #[test]
    fn verifier_flags_duplicate_run_ids() {
        let a = record("dup", "T1", "baseline", 1, true, vec![]);
        let b = {
            let mut b = a.clone();
            b.condition = "candidate".into();
            b
        };
        let s = compute_summary(&[a.clone(), b.clone()]);
        let v = verify_artifacts(&raw(&[a, b]), &serde_json::to_value(&s).unwrap());
        assert!(
            !v.checks
                .iter()
                .find(|(n, _, _)| n == "run_id_uniqueness")
                .unwrap()
                .1
        );
    }
}
