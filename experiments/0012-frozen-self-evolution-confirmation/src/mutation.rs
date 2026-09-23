//! Bounded prompt-mutation generation (Experiment 0012-specific).
//!
//! The ONLY evolvable surface in this experiment is an append-only
//! system-prompt policy suffix. A candidate is:
//!
//! ```text
//! incumbent system prompt (G0, byte-frozen)
//! + one append-only generic policy suffix
//! ```
//!
//! The mutation generator is the same underlying model used for agent
//! execution: it receives a strict whitelist [`MutationInput`] packet
//! built from discovery evidence, produces exactly `candidate_count`
//! (design.json: 4) suffixes + rationales in JSON, and nothing else.
//! The model receives no tools, writes no files, and cannot influence
//! the evaluator, the selection authority, or the oracle.
//!
//! Everything in this module is experiment-local: there is deliberately
//! no `MutationEngine` trait and no production promotion.
//!
//! Anti-seeding (0012 critical requirement): no candidate from
//! Experiment 0010, no 0011 rejected candidate, and no Experiment 0009
//! repair text is copied, hard-coded, hinted, or seeded anywhere in
//! this crate — not in the mutator prompt, not in the runtime code, not
//! in the retry message. The frozen mutator prompt defines the OUTPUT
//! SCHEMA only and carries zero behavioral hints; if the fresh mutator
//! independently rediscoveries a semantically similar policy, that is
//! ACCEPTED as independent rediscovery — never rejected post hoc. The
//! similarity question is asked only AFTER the formal result is frozen
//! (post-hoc diagnostic, never part of the verdict).
//!
//! Explicit output schema (the 0012 REQUIRED FIX over 0011): the
//! 0011-r1 failure was a *structural* schema-adherence failure (the
//! model returned the wrong key / a list of bare strings because the
//! prompt said "return JSON in the required schema" without stating
//! the schema). 0012 therefore (a) states the exact structure in the
//! frozen mutator prompt, (b) validates the raw response against that
//! exact structure with SHAPE-INFORMATIVE errors ([EXPECTED_STRUCTURE]),
//! and (c) on the single registered retry (max 2 attempts) feeds back
//! the machine errors plus the expected structure — no performance,
//! selection, or cost feedback, and no manual repair of model output.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::oracle::TaskSpec;
use crate::protocol::{ToolSpec, canonical_json, sha256_bytes};
use crate::trace::{Design, DiscoveryRecord};

/// Registered suffix length bounds.
pub const MIN_SUFFIX_WORDS: usize = 5;
pub const MAX_SUFFIX_WORDS: usize = 100;
pub const MAX_SUFFIX_BYTES: usize = 800;

/// Maximum number of full mutation-generation attempts (0012: the
/// registered maximum is two; if the second response is still invalid
/// the run is INCONCLUSIVE — the protocol does not continue, patch,
/// or hand-repair the output).
pub const MAX_GENERATION_ATTEMPTS: u32 = 2;

/// The exact expected mutator output structure. This same string
/// appears in the frozen mutator prompt AND in the shape-informative
/// retry message, so the documented schema and the enforced schema are
/// one and the same (checked by `preflight`).
pub const EXPECTED_STRUCTURE: &str = r#"{\"candidates\":[{\"suffix\":\"string\",\"rationale\":\"string\"}, ... exactly four objects]}"#;

/// The incumbent is Generation 0.
pub const INCUMBENT_ID: &str = "G0";

/// Candidate generation produced by mutation.
pub const CANDIDATE_GENERATION: u32 = 1;

/// Candidate IDs are assigned by the harness in returned array order,
/// derived from the design manifest `candidate_count` (C1..C4).
pub fn candidate_ids(design: &Design) -> Vec<String> {
    design.candidate_ids()
}

/// Terms a candidate suffix must NOT contain (0012 spec §16). Compared
/// case-insensitively against the lower-cased suffix. "state_read"/
/// "state_write" ARE allowed (visible tools). Separately, a suffix
/// must not contain any registered task ID or any `V12_*` literal.
pub const FORBIDDEN_TERMS: [&str; 9] = [
    "experiment",
    "benchmark",
    "oracle",
    "selection",
    "promotion",
    "held-out",
    "dropfirstnwrites",
    "fault_reason",
    "registered_drop_index",
];

/// All registered task IDs; a suffix must not contain any of them.
pub fn registered_task_ids(registry: &[TaskSpec]) -> Vec<String> {
    registry.iter().map(|t| t.id.clone()).collect()
}

// ===========================================================================
// Candidate types (experiment-only; NOT promoted to production)
// ===========================================================================

/// One generated candidate: G0 + one append-only suffix.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateEntry {
    pub candidate_id: String,
    /// Experiment-only lineage: generation 1, parent G0.
    pub generation: u32,
    pub parent_id: String,
    /// SHA-256 (bytes) of the frozen incumbent prompt file.
    pub parent_prompt_sha256: String,
    /// SHA-256 (bytes) of the frozen `mutation-input.json` the pool
    /// was generated from.
    pub mutation_input_sha256: String,
    /// The generated suffix, verbatim as returned by the model.
    pub suffix: String,
    /// SHA-256 (bytes) of the suffix.
    pub suffix_sha256: String,
    /// SHA-256 (bytes) of `compose_full_prompt(incumbent, suffix)`.
    pub full_prompt_sha256: String,
    /// Stored for audit only. MUST NOT affect selection.
    pub rationale: String,
    pub word_count: u32,
    pub byte_count: u32,
}

/// The frozen candidate pool. Once committed, the content must never
/// change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidatePool {
    pub experiment_id: String,
    /// The generation this pool belongs to: 1.
    pub generation: u32,
    /// The pool's common parent: "G0".
    pub parent_id: String,
    /// SHA-256 (bytes) of the frozen incumbent prompt file.
    pub parent_prompt_sha256: String,
    /// SHA-256 (bytes) of the frozen mutation input file.
    pub mutation_input_sha256: String,
    /// Exactly `candidate_count` entries, in harness-assigned order
    /// C1..C4.
    pub candidates: Vec<CandidateEntry>,
}

/// The only candidate composition rule: the incumbent text (trailing
/// newline trimmed) + one newline + the suffix. The full prompt is
/// always re-derivable from the two frozen inputs.
pub fn compose_full_prompt(incumbent: &str, suffix: &str) -> String {
    let base = incumbent.trim_end_matches('\n');
    format!("{base}\n{suffix}")
}

/// Collapse all whitespace runs to single spaces and trim. Used for
/// duplicate detection.
pub fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whitespace-delimited word count (0 for empty/whitespace-only).
pub fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

// ===========================================================================
// Suffix content validation
// ===========================================================================

/// Machine-check for the 0012 synthetic state literal pattern
/// `V12_[A-Z0-9_]+`. Returns true when any occurrence exists.
pub fn contains_v12_literal(s: &str) -> bool {
    let needle = b"V12_";
    let bytes = s.as_bytes();
    if bytes.len() < needle.len() + 1 {
        return false;
    }
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let mut j = i + needle.len();
            while j < bytes.len()
                && (bytes[j].is_ascii_uppercase() || bytes[j].is_ascii_digit() || bytes[j] == b'_')
            {
                j += 1;
            }
            if j > i + needle.len() {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// All content violations of ONE suffix. Empty list = valid.
/// The registry is used for the registered-task-ID check.
pub fn suffix_content_errors(suffix: &str, registry: &[TaskSpec]) -> Vec<String> {
    let mut errors = Vec::new();
    if suffix.trim().is_empty() {
        errors.push("suffix is empty".to_string());
        return errors;
    }
    let words = word_count(suffix);
    if words < MIN_SUFFIX_WORDS {
        errors.push(format!(
            "suffix has {words} words, fewer than the {MIN_SUFFIX_WORDS}-word minimum"
        ));
    }
    if words > MAX_SUFFIX_WORDS {
        errors.push(format!(
            "suffix has {words} words, more than the {MAX_SUFFIX_WORDS}-word maximum"
        ));
    }
    let bytes = suffix.len();
    if bytes > MAX_SUFFIX_BYTES {
        errors.push(format!(
            "suffix is {bytes} UTF-8 bytes, more than the {MAX_SUFFIX_BYTES}-byte maximum"
        ));
    }
    let lower = suffix.to_lowercase();
    for term in FORBIDDEN_TERMS {
        if lower.contains(term) {
            errors.push(format!("suffix contains forbidden term {term:?}"));
        }
    }
    if contains_v12_literal(suffix) {
        errors.push("suffix contains a synthetic V12_* state literal".to_string());
    }
    // Registered task IDs must never appear in a policy suffix:
    // they would let a policy key on task identity instead of behavior.
    for id in registered_task_ids(registry) {
        if suffix.contains(id.as_str()) {
            errors.push(format!("suffix contains registered task ID {id:?}"));
        }
    }
    errors
}

/// Validate one candidate suffix: content bounds.
pub fn validate_suffix(suffix: &str, registry: &[TaskSpec]) -> Vec<String> {
    suffix_content_errors(suffix, registry)
}

/// Full pool validation. `incumbent` is the incumbent prompt text
/// (file content); `incumbent_file_sha256` its byte hash.
pub fn validate_candidate_pool(
    pool: &CandidatePool,
    design: &Design,
    incumbent: &str,
    incumbent_file_sha256: &str,
    registry: &[TaskSpec],
) -> Vec<String> {
    let mut errors = Vec::new();
    let candidate_count = design.candidate_count as usize;
    let ids = candidate_ids(design);
    if pool.experiment_id != design.experiment_id {
        errors.push(format!(
            "pool experiment_id {:?} != {:?}",
            pool.experiment_id, design.experiment_id
        ));
    }
    if pool.generation != CANDIDATE_GENERATION {
        errors.push(format!(
            "pool generation {} != {CANDIDATE_GENERATION}",
            pool.generation
        ));
    }
    if pool.parent_id != INCUMBENT_ID {
        errors.push(format!(
            "pool parent {:?} != {INCUMBENT_ID:?}",
            pool.parent_id
        ));
    }
    if pool.parent_prompt_sha256 != incumbent_file_sha256 {
        errors.push(
            "pool parent_prompt_sha256 does not match the frozen incumbent prompt file".to_string(),
        );
    }
    if pool.candidates.len() != candidate_count {
        errors.push(format!(
            "pool has {} candidates, expected exactly {candidate_count}",
            pool.candidates.len()
        ));
    }
    let mut seen_normalized: Vec<String> = Vec::new();
    let task_ids = registered_task_ids(registry);
    for (i, c) in pool.candidates.iter().enumerate() {
        let who = if c.candidate_id.is_empty() {
            format!("candidate[{i}]")
        } else {
            c.candidate_id.clone()
        };
        if i < candidate_count && c.candidate_id != ids[i] {
            errors.push(format!(
                "candidate[{i}] has id {:?}, expected {:?}",
                c.candidate_id, ids[i]
            ));
        }
        if c.generation != CANDIDATE_GENERATION {
            errors.push(format!(
                "{who}: generation {} != {CANDIDATE_GENERATION}",
                c.generation
            ));
        }
        if c.parent_id != INCUMBENT_ID {
            errors.push(format!(
                "{who}: parent {:?} != {INCUMBENT_ID:?}",
                c.parent_id
            ));
        }
        if c.parent_prompt_sha256 != incumbent_file_sha256 {
            errors.push(format!("{who}: wrong parent prompt hash"));
        }
        if c.suffix_sha256 != sha256_bytes(c.suffix.as_bytes()) {
            errors.push(format!("{who}: suffix_sha256 does not match its suffix"));
        }
        let expected_full = sha256_bytes(compose_full_prompt(incumbent, &c.suffix).as_bytes());
        if c.full_prompt_sha256 != expected_full {
            errors.push(format!(
                "{who}: full_prompt_sha256 != sha256(incumbent + suffix)"
            ));
        }
        if c.word_count as usize != word_count(&c.suffix) {
            errors.push(format!(
                "{who}: recorded word_count {} is wrong",
                c.word_count
            ));
        }
        if c.byte_count as usize != c.suffix.len() {
            errors.push(format!(
                "{who}: recorded byte_count {} is wrong",
                c.byte_count
            ));
        }
        for e in validate_suffix(&c.suffix, registry) {
            errors.push(format!("{who}: {e}"));
        }
        // Duplicate after normalized whitespace.
        let n = normalize_whitespace(&c.suffix);
        if seen_normalized.contains(&n) {
            errors.push(format!("{who}: duplicate after whitespace normalization"));
        }
        seen_normalized.push(n);
        // Task IDs inside the suffix (registry-derived list).
        for id in &task_ids {
            if c.suffix.contains(id) {
                errors.push(format!("{who}: suffix contains task ID {id}"));
            }
        }
    }
    errors
}

// ===========================================================================
// Mutation input packet (strict whitelist)
// ===========================================================================

/// One model-visible discovery episode in the mutator packet.
///
/// WHITELIST: task prompt, initial state, model-visible conversation,
/// requested tool calls, model-visible tool results, final state,
/// termination reason, oracle success boolean. There is no field for
/// fault mode, stress id, drop count, `applied`, `fault_reason`,
/// `registered_drop_index`, hidden audit, or reasoning content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MutationInputEpisode {
    pub task_id: String,
    pub task_prompt: String,
    pub initial_state: Value,
    /// The exact message list the model saw: system (the incumbent
    /// prompt), user (the task), then assistant/tool turns.
    pub conversation: Vec<crate::protocol::Message>,
    pub requested_tool_calls: Vec<crate::trace::RequestedToolCall>,
    /// Model-visible tool results (the fault is invisible here).
    pub executed_tool_calls: Vec<crate::trace::ExecutedToolCall>,
    pub final_state: Value,
    pub termination_reason: String,
    pub oracle_success: bool,
}

/// The complete mutation-input packet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MutationInput {
    pub experiment_id: String,
    pub incumbent_system_prompt: String,
    pub tool_schemas: Vec<ToolSpec>,
    pub discovery_episodes: Vec<MutationInputEpisode>,
}

impl MutationInput {
    /// Compact single-line serialization for the mutator request body.
    pub fn to_json_string(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Build the whitelist packet from discovery records. Infrastructure
/// failures are excluded: their episodes are not behavioral evidence
/// (they are missing evidence, not G0 behavioral failures).
pub fn build_mutation_input(
    design: &Design,
    incumbent: &str,
    specs: &[ToolSpec],
    records: &[DiscoveryRecord],
) -> MutationInput {
    let episodes = records
        .iter()
        .filter(|r| r.infrastructure_failure.is_none())
        .map(|r| MutationInputEpisode {
            task_id: r.task_id.clone(),
            task_prompt: r.task_prompt().to_string(),
            initial_state: r.initial_state.clone(),
            conversation: r.conversation.clone(),
            requested_tool_calls: r.requested_tool_calls.clone(),
            executed_tool_calls: r.executed_tool_calls.clone(),
            final_state: r.final_state.clone(),
            termination_reason: r.termination_reason.clone(),
            oracle_success: r.oracle_success,
        })
        .collect();
    MutationInput {
        experiment_id: design.experiment_id.clone(),
        incumbent_system_prompt: incumbent.to_string(),
        tool_schemas: specs.to_vec(),
        discovery_episodes: episodes,
    }
}

/// Machine-level leakage verification of a mutation input.
/// Checks the RAW serialization text, not just intent:
/// - hidden fault / audit fields must never appear (field/schema check,
///   not a naive substring scan over natural-language content);
/// - selection / promotion task IDs must never appear;
/// - selection / promotion state literals must never appear.
///
/// Natural-language words such as "applied" inside model-visible
/// assistant text are NOT rejected — only hidden-field schema leakage.
pub fn mutation_input_leak_errors(input: &MutationInput, registry: &[TaskSpec]) -> Vec<String> {
    let text = canonical_json(&serde_json::to_value(input).unwrap()).to_string();
    let mut errors = Vec::new();

    // Hidden experiment / fault fields must never appear as JSON keys.
    for forbidden in [
        "DropFirstNWrites",
        "drop_first_n_writes",
        "fault_reason",
        "registered_drop_index",
        "\"applied\"",
        "fault_mode",
        "environment_write_attempts",
    ] {
        if text.contains(forbidden) {
            errors.push(format!(
                "mutation input contains hidden field {forbidden:?}"
            ));
        }
    }

    // Selection / promotion task IDs (registry-derived, not hard-coded).
    for t in registry
        .iter()
        .filter(|t| t.split != crate::oracle::TaskSplit::Discovery)
    {
        if text.contains(t.id.as_str()) {
            errors.push(format!(
                "mutation input contains {:?} task reference {:?}",
                t.split, t.id
            ));
        }
    }

    // Selection / promotion state literals (the prompts themselves are
    // covered by this, since every such prompt contains its unique
    // literal).
    let mut other_values: Vec<String> = Vec::new();
    for t in registry {
        if t.split != crate::oracle::TaskSplit::Discovery {
            for field in [t.initial_state.clone(), t.target_state.clone()] {
                if let Some(obj) = field.as_object() {
                    for v in obj.values() {
                        if let Some(s) = v.as_str() {
                            other_values.push(s.to_string());
                        }
                    }
                }
            }
        }
    }
    other_values.sort_unstable();
    other_values.dedup();
    for v in &other_values {
        if text.contains(v.as_str()) {
            errors.push(format!(
                "mutation input contains non-discovery state literal {v:?}"
            ));
        }
    }

    errors
}

// ===========================================================================
// Mutator response parsing + structural validation
// ===========================================================================

/// One raw mutator-produced candidate (before harness assignment).
/// Both fields are REQUIRED: the documented schema says every object
/// contains exactly the string fields `suffix` and `rationale`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawCandidate {
    pub suffix: String,
    pub rationale: String,
}

/// Deterministically strip one level of markdown code fences, if the
/// model wrapped its JSON in them. (The model is instructed to return
/// JSON only; fences are a known provider habit.)
pub fn strip_code_fences(raw: &str) -> String {
    let t = raw.trim();
    let fenced = t.strip_prefix("```json").or_else(|| t.strip_prefix("```"));
    let out = match fenced {
        // A well-formed fenced block: ```json<newline>JSON<newline>```
        Some(inner) => inner.trim_start().trim_end().trim_end_matches('`').trim(),
        None => t,
    };
    out.trim().to_string()
}

/// The shape-informative retry message for an invalid attempt (0012
/// REQUIRED FIX #2). It carries ONLY: the machine validation errors,
/// the instruction to return the full pool again, the exact expected
/// structure, and the no-bare-strings rule. It carries NO performance
/// feedback, NO task-success feedback, NO selection feedback, NO
/// known-good candidate, and NO human rewrite.
pub fn retry_message(errors: &[String]) -> String {
    format!(
        "Your response was rejected for structural validation errors:\n{}\n\n\nReturn the full candidate pool again.\n\nExpected structure:\n{EXPECTED_STRUCTURE}\n\nDo not return candidate strings directly.",
        errors.join("\n")
    )
}

/// Parse + structurally validate ONE mutator response against the
/// explicit documented schema.
///
/// Returns `(accepted_candidates, errors)`; `errors` empty ⇒ fully
/// valid pool. The validation is SHAPE-INFORMATIVE: structural
/// failures (wrong top-level key, non-object entries, wrong field set,
/// non-string values, wrong count) are reported with the expected
/// shape so the single registered retry can correct STRUCTURE — the
/// errors never comment on candidate content quality. String entries
/// are rejected, never manually parsed into objects (spec: "Do not
/// return a list of strings" is enforced, not emulated).
pub fn validate_mutator_response(
    design: &Design,
    raw_content: &str,
    incumbent: &str,
    registry: &[TaskSpec],
) -> (Option<Vec<RawCandidate>>, Vec<String>) {
    let text = strip_code_fences(raw_content);
    let parsed: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            return (None, vec![format!("invalid JSON: {e}")]);
        }
    };

    // --- structural validation (the 0012 explicit schema) -------------
    let mut errors = Vec::new();
    let candidates = match parsed.as_object() {
        None => {
            return (
                None,
                vec![format!(
                    "top-level value must be a JSON object with a single key \"candidates\" (got {})",
                    value_kind(&parsed)
                )],
            );
        }
        Some(obj) => {
            let mut keys: Vec<&String> = obj.keys().collect();
            keys.sort();
            if keys.len() != 1 || keys[0] != "candidates" {
                let found = keys
                    .iter()
                    .map(|k| format!("\"{k}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
                return (
                    None,
                    vec![format!(
                        "top-level object must contain exactly the key \"candidates\", found: {found}"
                    )],
                );
            }
            obj.get("candidates").cloned().unwrap_or(Value::Null)
        }
    };
    let items = match candidates.as_array() {
        None => {
            errors.push(format!(
                "\"candidates\" must be a JSON array of exactly {} objects (got {})",
                design.candidate_count,
                value_kind(&candidates)
            ));
            return (None, errors);
        }
        Some(items) => items.clone(),
    };
    if items.len() != design.candidate_count as usize {
        errors.push(format!(
            "\"candidates\" has {} entries, expected exactly {}",
            items.len(),
            design.candidate_count
        ));
    }
    let mut out: Vec<RawCandidate> = Vec::new();
    let mut seen_normalized = Vec::new();
    let task_ids = registered_task_ids(registry);
    for (i, item) in items.iter().enumerate() {
        let c = match item.as_object() {
            None => {
                errors.push(format!(
                    "candidate[{i}] must be an object with exactly the string fields \"suffix\" and \"rationale\" (got a {} — do not return candidate strings directly)",
                    value_kind(item)
                ));
                continue;
            }
            Some(obj) => {
                let mut keys: Vec<&String> = obj.keys().collect();
                keys.sort();
                let exact = keys == ["rationale", "suffix"];
                if !exact {
                    let found = keys
                        .iter()
                        .map(|k| format!("\"{k}\""))
                        .collect::<Vec<_>>()
                        .join(", ");
                    errors.push(format!(
                        "candidate[{i}] must contain exactly the fields \"suffix\" and \"rationale\", both strings (found: {found})"
                    ));
                    continue;
                }
                let suffix = match obj.get("suffix").and_then(Value::as_str) {
                    Some(s) => s.to_string(),
                    None => {
                        errors.push(format!(
                            "candidate[{i}] field \"suffix\" must be a string (got {})",
                            value_kind(obj.get("suffix").unwrap_or(&Value::Null))
                        ));
                        continue;
                    }
                };
                let rationale = match obj.get("rationale").and_then(Value::as_str) {
                    Some(s) => s.to_string(),
                    None => {
                        errors.push(format!(
                            "candidate[{i}] field \"rationale\" must be a string (got {})",
                            value_kind(obj.get("rationale").unwrap_or(&Value::Null))
                        ));
                        continue;
                    }
                };
                RawCandidate { suffix, rationale }
            }
        };
        if c.suffix.trim().is_empty() {
            errors.push(format!("candidate[{i}]: empty suffix"));
        }
        let w = word_count(&c.suffix);
        if !(MIN_SUFFIX_WORDS..=MAX_SUFFIX_WORDS).contains(&w) {
            errors.push(format!(
                "candidate[{i}]: suffix has {w} words (allowed {MIN_SUFFIX_WORDS}..={MAX_SUFFIX_WORDS})"
            ));
        }
        if c.suffix.len() > MAX_SUFFIX_BYTES {
            errors.push(format!(
                "candidate[{i}]: suffix is {} bytes (max {MAX_SUFFIX_BYTES})",
                c.suffix.len()
            ));
        }
        let n = normalize_whitespace(&c.suffix);
        if seen_normalized.contains(&n) {
            errors.push(format!("candidate[{i}]: duplicate after normalization"));
        }
        seen_normalized.push(n);
        for e in validate_suffix(&c.suffix, registry) {
            errors.push(format!("candidate[{i}]: {e}"));
        }
        for id in &task_ids {
            if c.suffix.contains(id) {
                errors.push(format!("candidate[{i}]: suffix contains task ID {id}"));
            }
        }
        // Sanity: the suffix must not reproduce the whole incumbent
        // (a "replacement" in disguise).
        if normalize_whitespace(&c.suffix) == normalize_whitespace(incumbent) {
            errors.push(format!(
                "candidate[{i}]: suffix normalizes to the incumbent itself"
            ));
        }
        out.push(c);
    }
    if errors.is_empty() {
        (Some(out), Vec::new())
    } else {
        (None, errors)
    }
}

/// A short JSON kind label for shape-informative errors.
fn value_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Build the frozen pool from one fully-valid response. Harness
/// assigns C1..C{candidate_count} in returned array order.
pub fn build_pool(
    design: &Design,
    candidates: &[RawCandidate],
    incumbent: &str,
    incumbent_file_sha256: &str,
    mutation_input_sha256: &str,
) -> CandidatePool {
    let ids = candidate_ids(design);
    let candidates = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| CandidateEntry {
            candidate_id: ids[i].clone(),
            generation: CANDIDATE_GENERATION,
            parent_id: INCUMBENT_ID.to_string(),
            parent_prompt_sha256: incumbent_file_sha256.to_string(),
            mutation_input_sha256: mutation_input_sha256.to_string(),
            suffix: c.suffix.clone(),
            suffix_sha256: sha256_bytes(c.suffix.as_bytes()),
            full_prompt_sha256: sha256_bytes(compose_full_prompt(incumbent, &c.suffix).as_bytes()),
            rationale: c.rationale.clone(),
            word_count: word_count(&c.suffix) as u32,
            byte_count: c.suffix.len() as u32,
        })
        .collect();
    CandidatePool {
        experiment_id: design.experiment_id.clone(),
        generation: CANDIDATE_GENERATION,
        parent_id: INCUMBENT_ID.to_string(),
        parent_prompt_sha256: incumbent_file_sha256.to_string(),
        mutation_input_sha256: mutation_input_sha256.to_string(),
        candidates,
    }
}

// ===========================================================================
// Generation artifact
// ===========================================================================

/// One mutation-generation attempt, persisted for audit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerationAttempt {
    pub attempt: u32,
    /// The exact model-visible content the provider returned
    /// (null when the provider gave none). NO hidden reasoning
    /// content: the protocol layer does not store reasoning fields.
    pub raw_response: Option<String>,
    pub structurally_valid: bool,
    /// Machine-generated validation errors (empty when valid).
    pub validation_errors: Vec<String>,
    pub prompt_tokens: Option<u64>,
    pub cached_prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub wall_time_ms: u64,
}

/// The complete mutation-generation artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerationArtifact {
    pub experiment_id: String,
    pub run_id: String,
    pub run_manifest_sha256: String,
    pub frozen_design_sha256: String,
    pub model: String,
    pub temperature: f64,
    pub redacted_endpoint: String,
    pub code_under_test_commit: String,
    pub mutation_input_sha256: String,
    pub mutator_prompt_sha256: String,
    pub parent_prompt_sha256: String,
    pub attempt_count: u32,
    pub attempts: Vec<GenerationAttempt>,
    /// `null` until the first fully-valid pool is accepted.
    pub mutation_generation_complete: bool,
    /// Accepted candidate pool (the FIRST fully-valid pool).
    pub accepted_pool: Option<CandidatePool>,
    pub wall_time_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolExecutor;
    use serde_json::json;

    fn design() -> Design {
        crate::experiment::load_design().expect("0012 design manifest loads and validates")
    }

    fn registry() -> Vec<TaskSpec> {
        let file =
            crate::experiment::load_tasks(&crate::experiment::crate_dir().join("tasks.json"))
                .unwrap();
        file.tasks
    }

    fn valid_pool() -> CandidatePool {
        let incumbents = "line one\nline two\n";
        // The pool is built with placeholder sha "3…"; validation against
        // a different sha must fail the parent_prompt_sha256 check.
        let c1 = RawCandidate {
            suffix: "Always observe the current external state before acting on it, so that every decision follows what is actually present."
                .to_string(),
            rationale: "r1".to_string(),
        };
        let c2 = RawCandidate {
            suffix: "Check that requested external state changes are actually present before declaring the task complete."
                .to_string(),
            rationale: "r2".to_string(),
        };
        let c3 = RawCandidate {
            suffix: "When a requested change is not observed after an action, take another action based on the observed state."
                .to_string(),
            rationale: "r3".to_string(),
        };
        let c4 = RawCandidate {
            suffix: "Keep your plan simple and base each step only on information you have directly observed."
                .to_string(),
            rationale: "r4".to_string(),
        };
        let d = design();
        let (cands, errors) = validate_mutator_response(
            &d,
            &json!({"candidates":[c1.clone(), c2.clone(), c3.clone(), c4.clone()]}).to_string(),
            incumbents,
            &registry(),
        );
        assert!(errors.is_empty(), "{errors:?}");
        let cands = cands.unwrap();
        build_pool(&d, &cands, incumbents, &"3".repeat(64), &"2".repeat(64))
    }

    #[test]
    fn compose_is_incumbent_plus_suffix_only() {
        let full = compose_full_prompt("a\nb\n", "S");
        assert_eq!(full, "a\nb\nS");
        assert!(full.starts_with("a\nb\n"));
    }

    #[test]
    fn v12_literal_detection() {
        assert!(contains_v12_literal("use V12_DA_0 here"));
        assert!(contains_v12_literal("V12_X"));
        assert!(!contains_v12_literal("no literals"));
        assert!(!contains_v12_literal("the word V12_ alone"));
        assert!(!contains_v12_literal(
            "old V_D1_0 literal is not a 0012 literal"
        ));
    }

    #[test]
    fn suffix_bounds_enforced() {
        let reg = registry();
        assert!(!validate_suffix("too short", &reg).is_empty()); // 2 words
        assert!(
            validate_suffix(&"word ".repeat(101), &reg)
                .iter()
                .any(|e| e.contains("maximum"))
        );
        assert!(
            validate_suffix(&format!("a b c d e {}", "f".repeat(810)), &reg)
                .iter()
                .any(|e| e.contains("bytes"))
        );
        let ok = "Always observe the current external state before acting on it.";
        assert!(validate_suffix(ok, &reg).is_empty(), "ok");
    }

    #[test]
    fn forbidden_terms_and_task_ids_rejected() {
        let reg = registry();
        for bad in [
            "Always check the experiment results before acting on state.",
            "Always check the Experiment results before acting on state.",
            "Benchmark your actions before observing state each time.",
            "Never trust the oracle when it tells you about state.",
            "Avoid the promotion set whenever you can observe state.",
            "Ignore held-out evidence when you observe state values.",
            "Mind the selection outcome before you act on each state key.",
            "Check DropFirstNWrites behavior while you observe each state key once.",
            "Read fault_reason after each write to the current state.",
            "Remember registered_drop_index after each state write occurs.",
            "Set V12_PA_1 whenever the state appears to need a change now.",
        ] {
            let errs = suffix_content_errors(bad, &reg);
            assert!(!errs.is_empty(), "not rejected: {bad}");
        }
        // Registered task IDs are rejected against the registry.
        for id in ["E12A", "S12B", "P12F"] {
            let bad = format!("Never touch {id} because the task registry tells you so.");
            let errs = validate_suffix(&bad, &reg);
            assert!(!errs.is_empty(), "not rejected: {bad}");
        }
    }

    #[test]
    fn pool_validation_rejects_bad_pools() {
        let d = design();
        let reg = registry();
        let incumbents = "line one\nline two\n";
        let pool = valid_pool();
        assert!(
            validate_candidate_pool(&pool, &d, incumbents, &"1".repeat(64), &reg)
                .iter()
                .any(|e| e.contains("parent_prompt_sha256"))
        );
        // Wrong hash fails; right hash passes.
        let mut p2 = pool.clone();
        p2.parent_prompt_sha256 = "1".repeat(64);
        for c in &mut p2.candidates {
            c.parent_prompt_sha256 = "1".repeat(64);
        }
        assert!(validate_candidate_pool(&p2, &d, incumbents, &"1".repeat(64), &reg).is_empty());
        // Tamper a suffix → hash mismatch + possibly content errors.
        let mut p3 = p2.clone();
        p3.candidates[0].suffix = "TAMPERED SUFFIX".to_string();
        assert!(
            validate_candidate_pool(&p3, &d, incumbents, &"1".repeat(64), &reg)
                .iter()
                .any(|e| e.contains("suffix_sha256"))
        );
        // Wrong count.
        let mut p4 = p2.clone();
        p4.candidates.pop();
        let e4 = validate_candidate_pool(&p4, &d, incumbents, &"1".repeat(64), &reg);
        assert!(e4.iter().any(|e| e.contains("candidates")), "{e4:?}");
        // Duplicate after normalization.
        let mut p5 = p2.clone();
        p5.candidates[1].suffix = normalize_whitespace(&p5.candidates[0].suffix);
        p5.candidates[1].suffix_sha256 = sha256_bytes(p5.candidates[1].suffix.as_bytes());
        p5.candidates[1].full_prompt_sha256 =
            sha256_bytes(compose_full_prompt(incumbents, &p5.candidates[1].suffix).as_bytes());
        p5.candidates[1].word_count = word_count(&p5.candidates[1].suffix) as u32;
        p5.candidates[1].byte_count = p5.candidates[1].suffix.len() as u32;
        assert!(
            validate_candidate_pool(&p5, &d, incumbents, &"1".repeat(64), &reg)
                .iter()
                .any(|e| e.contains("duplicate"))
        );
    }

    #[test]
    fn mutator_response_validation_counts_and_fences() {
        let d = design();
        let reg = registry();
        let incumbents = "line one\n";
        let raw = json!({"candidates":[{"suffix":"a b c d e f","rationale":"r"}]});
        let (c, e) = validate_mutator_response(&d, &raw.to_string(), incumbents, &reg);
        assert!(c.is_none());
        assert!(e.iter().any(|x| x.contains("expected exactly")), "{e:?}");
        // Fences are stripped deterministically (including the trailing
        // newline between the closing fence and the end of the message);
        // a fully-conformant fenced pool is accepted.
        let body = r#"{"candidates":[{"suffix":"a b c d e f","rationale":"r1"},{"suffix":"g h i j k l","rationale":"r2"},{"suffix":"m n o p q r","rationale":"r3"},{"suffix":"s t u v w x","rationale":"r4"}]}"#;
        let (c, e) =
            validate_mutator_response(&d, &format!("```json\n{body}\n```"), incumbents, &reg);
        assert!(c.is_some() && e.is_empty(), "{e:?}");
        // The 0011 failure shapes are rejected with shape-informative
        // errors, never manually repaired.
        // (a) wrong top-level key.
        let raw = json!({"suffixes":["one two three four five"]});
        let (c, e) = validate_mutator_response(&d, &raw.to_string(), incumbents, &reg);
        assert!(c.is_none());
        assert!(e.iter().any(|x| x.contains("\"candidates\"")), "{e:?}");
        // (b) a list of bare strings (the 0011-r1 observed failure).
        let raw = json!({"candidates":["one two three four five","six seven eight nine ten","eleven twelve thirteen fourteen fifteen","sixteen seventeen eighteen nineteen twenty"]});
        let (c, e) = validate_mutator_response(&d, &raw.to_string(), incumbents, &reg);
        assert!(c.is_none());
        assert!(
            e.iter()
                .any(|x| x.contains("do not return candidate strings directly")),
            "{e:?}"
        );
        // (c) an object missing the rationale field.
        let raw = json!({"candidates":[{"suffix":"a b c d e f"}]});
        let (c, e) = validate_mutator_response(&d, &raw.to_string(), incumbents, &reg);
        assert!(c.is_none());
        assert!(
            e.iter()
                .any(|x| x.contains("must contain exactly the fields")),
            "{e:?}"
        );
        // (d) an extra field on a candidate object.
        let raw = json!({"candidates":[{"suffix":"a b c d e f","rationale":"r","extra":1}]});
        let (c, e) = validate_mutator_response(&d, &raw.to_string(), incumbents, &reg);
        assert!(c.is_none());
        assert!(
            e.iter()
                .any(|x| x.contains("must contain exactly the fields")),
            "{e:?}"
        );
        // (e) top-level value that is not an object.
        let (c, e) = validate_mutator_response(&d, "[1, 2, 3, 4]", incumbents, &reg);
        assert!(c.is_none());
        assert!(
            e.iter().any(|x| x.contains("must be a JSON object")),
            "{e:?}"
        );
    }

    #[test]
    fn retry_message_carries_shape_not_feedback() {
        let errs = vec![
            "\"candidates\" has 3 entries, expected exactly 4".to_string(),
            "candidate[0] must be an object ...".to_string(),
        ];
        let m = retry_message(&errs);
        assert!(m.starts_with("Your response was rejected for structural validation errors:"));
        assert!(m.contains(&errs[0]) && m.contains(&errs[1]));
        assert!(m.contains(EXPECTED_STRUCTURE));
        assert!(m.contains("Do not return candidate strings directly."));
        // No performance / selection / cost feedback words.
        for forbidden in ["success rate", "you did well", "score", "tokens", "cost"] {
            assert!(!m.to_lowercase().contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn frozen_prompt_documents_the_explicit_schema() {
        // The 0012 fix: the frozen mutator prompt must state the exact
        // structure (the 0011 prompt only said "required schema").
        let prompt =
            std::fs::read_to_string(crate::experiment::crate_dir().join("prompts/mutator.md"))
                .unwrap();
        assert!(prompt.contains("\"candidates\": ["));
        assert!(prompt.contains("\"suffix\""));
        assert!(prompt.contains("\"rationale\""));
        assert!(prompt.contains("The top-level key must be exactly candidates."));
        assert!(prompt.contains("exactly four objects"));
        assert!(prompt.contains("exactly the string fields suffix and rationale"));
        assert!(prompt.contains("Do not return a list of strings."));
        // The prompt must NOT carry behavioral hints toward any
        // particular policy (schema only, zero mutation guidance).
        for hint in ["verify", "retry", "read-back", "silent", "drop"] {
            assert!(
                !prompt.to_lowercase().contains(hint),
                "mutator prompt carries a behavioral hint: {hint}"
            );
        }
    }

    #[test]
    fn mutation_input_whitelist_and_leak_check() {
        let d = design();
        let reg = registry();
        let incumbents = "line one\n";
        let mut input = MutationInput {
            experiment_id: d.experiment_id.clone(),
            incumbent_system_prompt: incumbents.into(),
            tool_schemas: ToolExecutor::specs(),
            discovery_episodes: Vec::new(),
        };
        assert!(mutation_input_leak_errors(&input, &reg).is_empty());
        // Inject a selection task prompt → must be caught.
        let sel = reg.iter().find(|t| t.id == "S12A").unwrap();
        input.discovery_episodes.push(MutationInputEpisode {
            task_id: "E12A".into(),
            task_prompt: sel.prompt.clone(),
            initial_state: sel.initial_state.clone(),
            conversation: Vec::new(),
            requested_tool_calls: Vec::new(),
            executed_tool_calls: Vec::new(),
            final_state: json!({"x": "V12_DA_0", "y": "V12_KEEP_DA"}),
            termination_reason: "completed".into(),
            oracle_success: false,
        });
        let errs = mutation_input_leak_errors(&input, &reg);
        assert!(errs.iter().any(|e| e.contains("V12_SA_0")), "{errs:?}");
        // A selection task ID in the input is a hard leak.
        let mut input2 = input.clone();
        input2.discovery_episodes[0].task_id = "S12A".into();
        let errs2 = mutation_input_leak_errors(&input2, &reg);
        assert!(errs2.iter().any(|e| e.contains("S12A")), "{errs2:?}");
    }

    #[test]
    fn known_0009_repair_text_is_structurally_valid_but_not_injectable() {
        // The Experiment 0009 repair line is a structurally valid
        // suffix. It must NEVER be produced by harness code: injection
        // is caught because pool suffixes must match the raw mutator
        // response verbatim (see trace tamper tests), not because the
        // text is banned. (No Experiment 0010 candidate text is
        // referenced anywhere in this crate — spec: do not hard-code
        // the old candidate.)
        let repair = "After every successful state_write, read the same key. If the observed value differs from the value you intended to write, write it again and re-read it. Repeat until the value matches, but make at most five state_write attempts for that requested key.";
        let reg = registry();
        assert!(
            validate_suffix(repair, &reg).is_empty(),
            "the 0009 repair must be structurally valid: {:?}",
            validate_suffix(repair, &reg)
        );
    }

    #[test]
    fn design_candidate_count_is_the_source_of_truth() {
        let d = design();
        assert_eq!(d.candidate_count, 4);
        assert_eq!(
            candidate_ids(&d),
            vec![
                "C1".to_string(),
                "C2".to_string(),
                "C3".to_string(),
                "C4".to_string()
            ]
        );
    }
}
