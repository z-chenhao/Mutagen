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

fn args_multiset(calls: &[CallRef], tool: &str) -> BTreeMap<String, usize> {
    let mut m: BTreeMap<String, usize> = BTreeMap::new();
    for c in calls.iter().filter(|c| c.tool == tool) {
        *m.entry(c.identity.clone()).or_default() += 1;
    }
    m
}

/// The relative order of writes and reads of the *same key*, projected
/// as the ordered pattern of operations on each key, for both runs.
fn state_op_pattern(calls: &[CallRef], key: StateKey) -> Vec<String> {
    calls
        .iter()
        .filter(|c| c.key == Some(key))
        .map(|c| match c.tool.as_str() {
            "state_write" => "W".to_string(),
            "state_read" => "R".to_string(),
            _ => unreachable!("only state tools exist"),
        })
        .collect()
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

    // argument_change: same tool name, different arguments across runs.
    let mut tools: BTreeSetLite = BTreeSetLite::new();
    for x in b.iter().chain(c.iter()) {
        tools.insert(x.tool.clone());
    }
    if tools
        .iter()
        .any(|tool| args_multiset(&b, tool) != args_multiset(&c, tool))
    {
        tags.push(TrajectoryTag::ArgumentChange);
    }

    // state_effect_order_change: the ordered W/R pattern on some key differs.
    if state_op_pattern(&b, StateKey::X) != state_op_pattern(&c, StateKey::X)
        || state_op_pattern(&b, StateKey::Y) != state_op_pattern(&c, StateKey::Y)
    {
        tags.push(TrajectoryTag::StateEffectOrderChange);
    }

    tags
}

/// Minimal sorted set to avoid pulling in `ordered_set`.
#[derive(Debug, Default)]
struct BTreeSetLite(BTreeMap<String, bool>);

impl BTreeSetLite {
    fn new() -> Self {
        Self::default()
    }
    fn insert(&mut self, s: String) {
        self.0.insert(s, true);
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
        conclusion,
    }
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
        EpisodeRecord {
            experiment_id: "0006".into(),
            run_id: run_id.into(),
            task_id: task.into(),
            task_name: "test".into(),
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

        // truly novel: same name, different argument
        let c2 = vec![
            call(0, 1, "state_read", "x", None),
            call(1, 2, "state_write", "y", Some("C")),
        ];
        let tags = trajectory_tags(&b, &c2);
        assert!(tags.contains(&TrajectoryTag::NovelCall));
        assert!(tags.contains(&TrajectoryTag::ArgumentChange));
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
            call(1, 2, "state_write", "x", Some("B")), // added + state-effect
            call(2, 3, "state_read", "y", None), // omitted read of x, novel-ish? no: read y absent in b => novel
        ];
        let tags = trajectory_tags(&b, &c);
        for t in [
            TrajectoryTag::ArgumentChange,
            TrajectoryTag::AddedCall,
            TrajectoryTag::OmittedCall,
            TrajectoryTag::NovelCall,
            TrajectoryTag::StateEffectOrderChange,
        ] {
            assert!(tags.contains(&t), "expected tag {t:?} in {tags:?}");
        }
        assert!(!tags.contains(&TrajectoryTag::Exact));
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
        let recs = vec![
            record(
                "x1",
                "T3",
                "baseline",
                1,
                true,
                vec![call(0, 1, "state_write", "x", Some("A"))],
            ),
            record(
                "x2",
                "T3",
                "candidate",
                1,
                true,
                vec![call(0, 1, "state_write", "x", Some("A"))],
            ),
            record(
                "x3",
                "T1",
                "baseline",
                1,
                true,
                vec![call(0, 1, "state_read", "x", None)],
            ),
            record(
                "x4",
                "T1",
                "candidate",
                1,
                true,
                vec![call(0, 1, "state_read", "x", None)],
            ),
            record(
                "x5",
                "T2",
                "baseline",
                1,
                true,
                vec![call(0, 1, "state_read", "x", None)],
            ),
            record(
                "x6",
                "T2",
                "candidate",
                1,
                true,
                vec![call(0, 1, "state_read", "x", None)],
            ),
            record(
                "x7",
                "T4",
                "baseline",
                1,
                true,
                vec![call(0, 1, "state_read", "x", None)],
            ),
            record(
                "x8",
                "T4",
                "candidate",
                1,
                true,
                vec![call(0, 1, "state_read", "x", None)],
            ),
            record(
                "x9",
                "T5",
                "baseline",
                1,
                true,
                vec![call(0, 1, "state_write", "x", Some("A"))],
            ),
            record(
                "x10",
                "T5",
                "candidate",
                1,
                true,
                vec![call(0, 1, "state_write", "x", Some("A"))],
            ),
            record(
                "x11",
                "T6",
                "baseline",
                1,
                true,
                vec![call(0, 1, "state_write", "x", Some("A"))],
            ),
            record(
                "x12",
                "T6",
                "candidate",
                1,
                true,
                vec![call(0, 1, "state_write", "x", Some("A"))],
            ),
        ];
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
        let r = record(
            "z1",
            "T1",
            "baseline",
            1,
            true,
            vec![call(0, 1, "state_read", "x", None)],
        );
        let s = compute_summary(std::slice::from_ref(&r));
        let sv = serde_json::to_value(&s).unwrap();
        // clean artifacts pass
        assert!(verify_artifacts(&raw(std::slice::from_ref(&r)), &sv).ok);

        // a raw artifact carrying a reasoning-like key must be flagged
        // (struct round-trips would silently drop such keys).
        let mut v = serde_json::to_value(&r).unwrap();
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
