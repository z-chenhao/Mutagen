//! Experiment 0010 orchestration: the runner outside the kernel.
//!
//! Stage commands (each requires the frozen artifacts of the previous
//! stage to be present and hash-consistent):
//!
//!   discover  → 18 incumbent episodes; whitelist mutation input
//!   generate  → ≤2 mutation-generation attempts; frozen candidate pool
//!   select    → 150-episode five-condition tournament; frozen selected
//!               candidate
//!   promote   → 96-episode two-condition confirmatory test
//!
//! The kernel knows none of this: it records execution; the oracle
//! grades state; the selection authority is deterministic Rust
//! (`crate::selection`). Prompts, tasks, and the stress profile are
//! loaded, validated, and hashed BEFORE any model request.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use serde_json::json;

use crate::kernel::{self, KernelConfig};
use crate::model::{ModelClient, redacted_endpoint};
use crate::mutation::{
    self, CandidatePool, GenerationArtifact, GenerationAttempt, MutationInput, build_mutation_input,
};
use crate::oracle::{TaskSpec, TaskSplit};
use crate::protocol::{Message, sha256_bytes};
use crate::selection::CandidateTally;
use crate::tools::{FaultMode, ToolExecutor};
use crate::trace::{
    self, DISCOVERY_EPISODES, DISCOVERY_REPETITIONS, DiscoveryRecord, FrozenSelectionBlock,
    PROMOTION_EPISODES, PROMOTION_REPETITIONS, PromotionRecord, RecordProvenance,
    SELECTION_CONDITIONS, SELECTION_EPISODES, SELECTION_REPETITIONS, SelectedCandidate,
    SelectionRecord,
};

/// The experiment crate's directory.
pub fn crate_dir() -> PathBuf {
    std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("experiments/0010-controlled-self-evolution"))
}

// ===========================================================================
// Frozen configuration
// ===========================================================================

/// The on-disk task registry shape (`tasks.json`).
#[derive(Debug, Clone, Deserialize)]
pub struct TasksFile {
    pub families: std::collections::BTreeMap<String, FamilyFile>,
    pub tasks: Vec<TaskSpec>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FamilyFile {
    pub discovery: Vec<String>,
    pub selection: Vec<String>,
    pub promotion: Vec<String>,
}

/// Load + validate the 12-task registry (3 discovery, 3 selection,
/// 6 promotion; one or two tasks per family per split).
pub fn load_tasks(path: &Path) -> Result<TasksFile, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read tasks file {}: {e}", path.display()))?;
    let parsed: Value =
        serde_json::from_str(&raw).map_err(|e| format!("tasks file is not valid JSON: {e}"))?;
    let file: TasksFile =
        serde_json::from_value(parsed).map_err(|e| format!("invalid tasks file: {e}"))?;
    let want = |split: TaskSplit, ids: &[&str]| {
        let got: Vec<&str> = file
            .tasks
            .iter()
            .filter(|t| t.split == split)
            .map(|t| t.id.as_str())
            .collect();
        got == ids
    };
    if !want(TaskSplit::Discovery, &["EV1", "EV2", "EV3"]) {
        return Err("registry must contain exactly EV1/EV2/EV3 as discovery tasks".into());
    }
    if !want(TaskSplit::Selection, &["SEL1", "SEL2", "SEL3"]) {
        return Err("registry must contain exactly SEL1/SEL2/SEL3 as selection tasks".into());
    }
    if !want(
        TaskSplit::Promotion,
        &["PRO1", "PRO2", "PRO3", "PRO4", "PRO5", "PRO6"],
    ) {
        return Err("registry must contain exactly PRO1..PRO6 as promotion tasks".into());
    }
    for t in &file.tasks {
        if crate::tools::StateKey::parse(&t.fault_key).is_none() {
            return Err(format!("task {} has an invalid fault_key", t.id));
        }
        if trace::frozen_stress(&t.family).is_none() {
            return Err(format!(
                "task {} family {} has no frozen stress",
                t.id, t.family
            ));
        }
    }
    for (family, ff) in &file.families {
        let members: Vec<&str> = ff
            .discovery
            .iter()
            .chain(ff.selection.iter())
            .chain(ff.promotion.iter())
            .map(String::as_str)
            .collect();
        let ok = members.iter().all(|id| {
            file.tasks
                .iter()
                .find(|t| t.id == *id)
                .map(|t| t.family == *family)
                .unwrap_or(false)
        });
        if !ok {
            return Err(format!(
                "family {family} declares unknown or misfiled tasks"
            ));
        }
    }
    Ok(file)
}

/// Read a prompt file as text.
pub fn load_prompt(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read prompt {}: {e}", path.display()))
}

/// SHA-256 of a file's bytes.
pub fn hash_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Ok(sha256_bytes(&bytes))
}

/// Environment configuration. Missing required variables mean the
/// experiment is NOT executed; no fallback model is substituted.
pub fn env_config() -> Result<(String, String, Option<String>), String> {
    let base_url = std::env::var("MUTAGEN_EXP_BASE_URL").map_err(|_| {
        "MUTAGEN_EXP_BASE_URL is not set; the experiment is NOT executed (no fallback model)"
            .to_string()
    })?;
    let model = std::env::var("MUTAGEN_EXP_MODEL").map_err(|_| {
        "MUTAGEN_EXP_MODEL is not set; the experiment is NOT executed (no fallback model)"
            .to_string()
    })?;
    if base_url.trim().is_empty() || model.trim().is_empty() {
        return Err("MUTAGEN_EXP_BASE_URL / MUTAGEN_EXP_MODEL must be non-empty".into());
    }
    let api_key = std::env::var("MUTAGEN_EXP_API_KEY")
        .ok()
        .filter(|k| !k.is_empty());
    Ok((base_url, model, api_key))
}

/// The code-under-test commit (40-hex) for records.
fn resolve_code_commit(explicit: Option<&str>) -> Result<String, String> {
    let raw = match explicit {
        Some(s) => s.to_string(),
        None => {
            let output = std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .map_err(|e| format!("cannot run git rev-parse: {e}"))?;
            if !output.status.success() {
                return Err("--code-commit not given and `git rev-parse HEAD` failed".into());
            }
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
    };
    if raw.len() != 40 || !raw.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("code commit must be a 40-hex git SHA, got {raw:?}"));
    }
    Ok(raw)
}

/// All frozen inputs, validated and hashed (runs before any model call).
struct FrozenConfig {
    incumbent: String,
    mutator: String,
    incumbent_sha: String,
    mutator_sha: String,
    registry: Vec<TaskSpec>,
    registry_sha: String,
    profile_sha: String,
}

fn load_frozen_config() -> Result<FrozenConfig, String> {
    let dir = crate_dir();
    let incumbent = load_prompt(&dir.join("prompts/incumbent.md"))?;
    let mutator = load_prompt(&dir.join("prompts/mutator.md"))?;
    let registry_file = load_tasks(&dir.join("tasks.json"))?;
    let profile_raw = std::fs::read_to_string(dir.join("stress-profile.json"))
        .map_err(|e| format!("cannot read stress profile: {e}"))?;
    let profile: Value =
        serde_json::from_str(&profile_raw).map_err(|e| format!("stress profile invalid: {e}"))?;
    trace::load_stress_profile(&dir.join("stress-profile.json"))?;
    let profile_sha = trace::stress_profile_sha256(&profile);
    let registry = registry_file.tasks;
    Ok(FrozenConfig {
        incumbent,
        mutator,
        incumbent_sha: hash_file(&dir.join("prompts/incumbent.md"))?,
        mutator_sha: hash_file(&dir.join("prompts/mutator.md"))?,
        registry_sha: trace::task_registry_sha256(&registry)?,
        registry,

        profile_sha,
    })
}

fn read_json(path: &Path, what: &str) -> Result<Value, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {what} {}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("{what} is not valid JSON: {e}"))
}

fn write_json_pretty(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(value).map_err(|e| format!("serialization: {e}"))?;
    std::fs::write(path, raw + "\n").map_err(|e| format!("cannot write {}: {e}", path.display()))
}

// ===========================================================================
// Stage 1: discovery (spec §76)
// ===========================================================================

pub struct DiscoverConfig {
    pub trajectories: PathBuf,
    pub summary: PathBuf,
    pub mutation_input: PathBuf,
    pub code_commit: Option<String>,
    pub endpoint: Option<String>,
}

/// Run the 18 incumbent discovery episodes, then persist the whitelist
/// mutation input and the discovery summary.
pub fn discover(config: &DiscoverConfig) -> Result<(), String> {
    let (base_url, model, api_key) = env_config()?;
    let cfg = load_frozen_config()?;
    let commit = resolve_code_commit(config.code_commit.as_deref())?;
    let endpoint = config
        .endpoint
        .clone()
        .unwrap_or_else(|| redacted_endpoint(&base_url));

    if let Some(parent) = config.trajectories.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(&config.trajectories)
            .map_err(|e| format!("cannot create trajectories file: {e}"))?,
    );

    let client = ModelClient::new(base_url, model.clone(), api_key);
    let specs: Vec<crate::protocol::ToolSpec> = ToolExecutor::specs();
    let kernel_config = KernelConfig::default();
    let run_prefix = format!(
        "exp0010-disc-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );

    let discovery_tasks: Vec<&TaskSpec> = cfg
        .registry
        .iter()
        .filter(|t| t.split == TaskSplit::Discovery)
        .collect();
    let total = DISCOVERY_EPISODES;
    println!(
        "starting discovery: model={model} endpoint={endpoint} episodes={total} code_commit={commit} run_prefix={run_prefix}"
    );

    let mut records = Vec::new();
    let mut counter = 0usize;
    for task in &discovery_tasks {
        let (stress, count) = trace::frozen_stress(&task.family)
            .ok_or_else(|| format!("task {} family has no frozen stress", task.id))?;
        for rep in 1..=DISCOVERY_REPETITIONS {
            counter += 1;
            eprintln!("[{}/{total}] {} {} rep{rep}", counter, task.id, stress);
            let fault = FaultMode::for_stress(&task.fault_key, count);
            let mut tools = ToolExecutor::fresh(&task.initial_state, fault.clone());
            let outcome = kernel::run_episode(
                |messages: &[Message], specs: &[crate::protocol::ToolSpec]| {
                    client.complete(messages, specs)
                },
                &mut tools,
                &specs,
                &cfg.incumbent,
                &task.prompt,
                &kernel_config,
            );
            let c = kernel::core(outcome);
            let eval = crate::oracle::evaluate_task(
                task,
                &crate::oracle::OracleInput {
                    termination_reason: c.termination_reason.clone(),
                    initial_state: task.initial_state.clone(),
                    final_state: c.final_state.clone(),
                },
            );
            let record = DiscoveryRecord {
                experiment_id: trace::EXPERIMENT_ID.to_string(),
                phase: "discovery".to_string(),
                run_id: format!("{run_prefix}-{:03}", counter),
                sequence: counter as u32,
                family: task.family.clone(),
                task_id: task.id.clone(),
                split: "discovery".to_string(),
                stress_level: stress.to_string(),
                drop_count: count,
                fault_key: task.fault_key.clone(),
                repetition: rep,
                condition: "G0".to_string(),
                generation: 0,
                provenance: RecordProvenance {
                    model: model.clone(),
                    temperature: crate::protocol::TEMPERATURE,
                    redacted_endpoint: endpoint.clone(),
                    code_under_test_commit: commit.clone(),
                    incumbent_prompt_sha256: cfg.incumbent_sha.clone(),
                    mutator_prompt_sha256: cfg.mutator_sha.clone(),
                    task_registry_sha256: cfg.registry_sha.clone(),
                    stress_profile_sha256: cfg.profile_sha.clone(),
                },
                initial_state: task.initial_state.clone(),
                target_state: task.target_state.clone(),
                fault_mode: serde_json::to_value(&fault).unwrap_or(Value::Null),
                conversation: c.conversation,
                model_requests: c.model_requests,
                requested_tool_calls: c.requested_tool_calls,
                executed_tool_calls: c.executed_tool_calls,
                environment_write_attempts: c.write_attempts,
                final_state: c.final_state,
                final_answer: c.final_answer,
                model_turn_count: c.model_turn_count,
                tool_call_count: c.tool_call_count,
                agent_failure: c.agent_failure,
                infrastructure_failure: c.infrastructure_failure,
                usage_accounting_error: c.usage_accounting_error,
                termination_reason: c.termination_reason,
                oracle_success: eval.success,
                oracle_failure_reasons: eval.reasons,
                timing: c.timing,
            };
            let line =
                serde_json::to_string(&record).map_err(|e| format!("record serialization: {e}"))?;
            out.write_all(line.as_bytes())
                .map_err(|e| format!("write failure: {e}"))?;
            out.write_all(b"\n")
                .map_err(|e| format!("write failure: {e}"))?;
            out.flush().map_err(|e| format!("flush failure: {e}"))?;
            records.push(record.clone());
            println!(
                "  {} turns={} calls={} termination={} oracle_success={}",
                record.run_id,
                record.model_turn_count,
                record.tool_call_count,
                record.termination_reason,
                record.oracle_success
            );
        }
    }
    out.flush().map_err(|e| format!("final flush: {e}"))?;
    if records.len() != total {
        return Err(format!(
            "discovery produced {} episodes, expected {total} — the artifact is incomplete",
            records.len()
        ));
    }

    // The whitelist mutation input (spec §22/§23/§56): infrastructure
    // failures are excluded; the file bytes are frozen and hashed.
    let input = build_mutation_input(&cfg.incumbent, &specs, &records);
    let leaks = mutation::mutation_input_leak_errors(&input, &cfg.registry);
    if !leaks.is_empty() {
        return Err(format!(
            "mutation input leakage detected at construction: {leaks:?}"
        ));
    }
    write_json_pretty(
        &config.mutation_input,
        &serde_json::to_value(&input).unwrap(),
    )?;
    let input_file_sha = sha256_bytes(
        &std::fs::read(&config.mutation_input)
            .map_err(|e| format!("cannot re-read mutation input: {e}"))?,
    );

    let summary = trace::compute_discovery_summary(
        &records,
        &model,
        &endpoint,
        &commit,
        &cfg.incumbent_sha,
        &cfg.registry_sha,
        &cfg.profile_sha,
        &input_file_sha,
    );
    write_json_pretty(&config.summary, &serde_json::to_value(&summary).unwrap())?;

    println!(
        "discovery complete: {} episodes, {} valid, {} infra failures, G0 {} success / {} failure; mutation input sha256={}",
        summary.episodes_total,
        summary.valid_episodes,
        summary.infrastructure_failures,
        summary.oracle_successes,
        summary.episodes_total - summary.oracle_successes - summary.infrastructure_failures,
        input_file_sha
    );
    if !summary.proceed_to_mutation_generation {
        println!(
            "STOP: discovery gate failure — the experiment is INCONCLUSIVE; do NOT run mutation generation."
        );
    }
    Ok(())
}
// (continued)

// ===========================================================================
// Stage 2: mutation generation (spec §79)
// ===========================================================================

pub struct GenerateConfig {
    pub mutation_input: PathBuf,
    pub generation_artifact: PathBuf,
    pub candidate_pool: PathBuf,
    pub code_commit: Option<String>,
    pub endpoint: Option<String>,
}

/// Build the mutator user message: the registered schema instruction +
/// the exact frozen mutation-input bytes.
fn mutator_user_message(input: &MutationInput, errors: &[String]) -> String {
    let schema = r#"{"candidates":[{"suffix":"<string, 5-100 words>","rationale":"<string>"}]}"#;
    let mut s = String::new();
    s.push_str("The required output schema is exactly:\n");
    s.push_str(schema);
    s.push_str("\n\nReturn JSON only, with exactly four distinct candidates.\n\n");
    s.push_str("Mutation input:\n");
    s.push_str(&input.to_json_string());
    if !errors.is_empty() {
        s.push_str("\n\nYour previous attempt was structurally invalid. Machine-generated validation errors:\n");
        for e in errors {
            s.push_str("- ");
            s.push_str(e);
            s.push('\n');
        }
        s.push_str(
            "Regenerate the FULL four-candidate pool (do not preserve any previous candidate).",
        );
    }
    s
}

/// Run mutation generation: at most two full attempts (spec §27), the
/// first fully-valid pool is accepted and frozen.
pub fn generate(config: &GenerateConfig) -> Result<(), String> {
    let (base_url, model, api_key) = env_config()?;
    let cfg = load_frozen_config()?;
    let commit = resolve_code_commit(config.code_commit.as_deref())?;
    let endpoint = config
        .endpoint
        .clone()
        .unwrap_or_else(|| redacted_endpoint(&base_url));

    // The mutation generator MUST consume exactly the committed bytes
    // of mutation-input.json (spec §56).
    let input_bytes = std::fs::read(&config.mutation_input).map_err(|e| {
        format!(
            "cannot read mutation input {}: {e}",
            config.mutation_input.display()
        )
    })?;
    let input_file_sha = sha256_bytes(&input_bytes);
    let input: MutationInput =
        serde_json::from_slice(&input_bytes).map_err(|e| format!("mutation input invalid: {e}"))?;
    let leaks = mutation::mutation_input_leak_errors(&input, &cfg.registry);
    if !leaks.is_empty() {
        return Err(format!(
            "mutation input leakage verification failed BEFORE the mutator call: {leaks:?}"
        ));
    }

    // The discovery summary (if present) must authorize generation.
    let dir = crate_dir();
    let summary_path = dir.join("discovery-summary.json");
    if summary_path.exists() {
        let s = read_json(&summary_path, "discovery summary")?;
        if s.get("proceed_to_mutation_generation")
            .and_then(Value::as_bool)
            == Some(false)
        {
            return Err(
                "the frozen discovery summary does not authorize mutation generation (gate failure) — the experiment is INCONCLUSIVE"
                    .to_string(),
            );
        }
    }

    let client = ModelClient::new(base_url, model.clone(), api_key);
    let system = Message::system(cfg.mutator.clone());
    let mut attempts: Vec<GenerationAttempt> = Vec::new();
    let mut accepted: Option<CandidatePool> = None;
    let mut errors: Vec<String> = Vec::new();
    let started = std::time::Instant::now();
    let mut attempt_no = 0u32;

    loop {
        attempt_no += 1;
        let user_content = mutator_user_message(&input, &errors);
        let t = std::time::Instant::now();
        let resp = client.complete_mutator(&[system.clone(), Message::user(user_content)]);
        let wall = t.elapsed().as_millis() as u64;
        let (raw, usage) = match resp {
            Ok(r) => (
                r.content.clone(),
                (
                    r.usage.prompt_tokens,
                    r.usage.cached_prompt_tokens,
                    r.usage.completion_tokens,
                    r.usage.reasoning_tokens,
                ),
            ),
            Err(e) => {
                // Infrastructure failure of the generation call.
                return Err(format!(
                    "mutation generation request failed (infrastructure): {e:?}; the experiment is INCONCLUSIVE (do not patch and continue)"
                ));
            }
        };
        let (raw_for_validation, verrors) = match &raw {
            Some(text) => {
                let (_, e) =
                    mutation::validate_mutator_response(text, &cfg.incumbent, &cfg.registry);
                (text.clone(), e)
            }
            None => (
                String::new(),
                vec!["the provider returned no content".to_string()],
            ),
        };
        let valid = verrors.is_empty();
        if valid {
            if let Some(cands) = mutation::validate_mutator_response(
                &raw_for_validation,
                &cfg.incumbent,
                &cfg.registry,
            )
            .0
            {
                accepted = Some(mutation::build_pool(
                    &cands,
                    &cfg.incumbent,
                    &cfg.incumbent_sha,
                    &input_file_sha,
                ));
            }
        }
        attempts.push(GenerationAttempt {
            attempt: attempt_no,
            raw_response: raw.clone(),
            structurally_valid: valid,
            validation_errors: verrors.clone(),
            prompt_tokens: usage.0,
            cached_prompt_tokens: usage.1,
            completion_tokens: usage.2,
            reasoning_tokens: usage.3,
            wall_time_ms: wall,
        });
        if valid || attempt_no >= mutation::MAX_GENERATION_ATTEMPTS {
            break;
        }
        errors = verrors;
    }

    let complete = accepted.is_some();
    let artifact = GenerationArtifact {
        experiment_id: trace::EXPERIMENT_ID.to_string(),
        model: model.clone(),
        temperature: crate::protocol::MUTATOR_TEMPERATURE,
        redacted_endpoint: endpoint.clone(),
        code_under_test_commit: commit,
        mutation_input_sha256: input_file_sha.clone(),
        mutator_prompt_sha256: cfg.mutator_sha.clone(),
        parent_prompt_sha256: cfg.incumbent_sha.clone(),
        attempt_count: attempts.len() as u32,
        attempts,
        mutation_generation_complete: complete,
        accepted_pool: accepted.clone(),
        wall_time_ms: started.elapsed().as_millis() as u64,
    };
    write_json_pretty(
        &config.generation_artifact,
        &serde_json::to_value(&artifact).unwrap(),
    )?;

    match &accepted {
        Some(pool) => {
            write_json_pretty(&config.candidate_pool, &serde_json::to_value(pool).unwrap())?;
            // Audit printout (spec §80): the pool, as generated.
            println!("generated candidate pool (frozen after commit):");
            for c in &pool.candidates {
                println!(
                    "  {}  words={} bytes={} sha256={}\n      {}",
                    c.candidate_id, c.word_count, c.byte_count, c.suffix_sha256, c.suffix
                );
            }
            println!(
                "pool file sha256: {}",
                sha256_bytes(&std::fs::read(&config.candidate_pool).map_err(|e| e.to_string())?)
            );
            println!(
                "mutator cost: prompt/completion/reasoning tokens reported per attempt; wall time {} ms",
                artifact.wall_time_ms
            );
        }
        None => {
            println!(
                "STOP: no structurally valid 4-candidate pool after {} attempts — mutation_generation_complete=false; the experiment is INCONCLUSIVE; do NOT hand-author candidates.",
                artifact.attempt_count
            );
            return Ok(());
        }
    }
    Ok(())
}

// ===========================================================================
// Stage 3: selection tournament (spec §82)
// ===========================================================================

pub struct SelectConfig {
    pub candidate_pool: PathBuf,
    pub trajectories: PathBuf,
    pub summary: PathBuf,
    pub selected: PathBuf,
    pub code_commit: Option<String>,
    pub endpoint: Option<String>,
}

/// Load the frozen candidate pool + mutation input with hash checks.
fn load_frozen_pool_and_input(
    pool_path: &Path,
    input_path: &Path,
    cfg: &FrozenConfig,
) -> Result<(CandidatePool, MutationInput, String, String), String> {
    let pool_bytes = std::fs::read(pool_path)
        .map_err(|e| format!("cannot read candidate pool {}: {e}", pool_path.display()))?;
    let pool_file_sha = sha256_bytes(&pool_bytes);
    let pool: CandidatePool =
        serde_json::from_slice(&pool_bytes).map_err(|e| format!("candidate pool invalid: {e}"))?;
    let errors =
        mutation::validate_candidate_pool(&pool, &cfg.incumbent, &cfg.incumbent_sha, &cfg.registry);
    if !errors.is_empty() {
        return Err(format!("candidate pool fails validation: {errors:?}"));
    }
    let input_bytes = std::fs::read(input_path)
        .map_err(|e| format!("cannot read mutation input {}: {e}", input_path.display()))?;
    let input_file_sha = sha256_bytes(&input_bytes);
    let input: MutationInput =
        serde_json::from_slice(&input_bytes).map_err(|e| format!("mutation input invalid: {e}"))?;
    if pool.mutation_input_sha256 != input_file_sha {
        return Err(
            "candidate pool does not reference the frozen mutation input hash — STOP (candidate may have been regenerated)".to_string(),
        );
    }
    if !mutation::mutation_input_leak_errors(&input, &cfg.registry).is_empty() {
        return Err("mutation input leakage verification failed — STOP".to_string());
    }
    Ok((pool, input, pool_file_sha, input_file_sha))
}

/// Run the 150-episode five-condition selection tournament, apply the
/// frozen selection rule, and persist the frozen selected candidate.
pub fn select(config: &SelectConfig) -> Result<(), String> {
    let (base_url, model, api_key) = env_config()?;
    let cfg = load_frozen_config()?;
    let commit = resolve_code_commit(config.code_commit.as_deref())?;
    let endpoint = config
        .endpoint
        .clone()
        .unwrap_or_else(|| redacted_endpoint(&base_url));
    let dir = crate_dir();
    let (pool, _input, pool_file_sha, input_file_sha) = load_frozen_pool_and_input(
        &config.candidate_pool,
        &dir.join("mutation-input.json"),
        &cfg,
    )?;

    if let Some(parent) = config.trajectories.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(&config.trajectories)
            .map_err(|e| format!("cannot create trajectories file: {e}"))?,
    );

    let client = ModelClient::new(base_url, model.clone(), api_key);
    let specs: Vec<crate::protocol::ToolSpec> = ToolExecutor::specs();
    let kernel_config = KernelConfig::default();
    let run_prefix = format!(
        "exp0010-sel-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );

    // Condition → full prompt.
    let prompts: Vec<(String, String, Option<String>)> = {
        let mut v = vec![("G0".to_string(), cfg.incumbent.clone(), None)];
        for c in &pool.candidates {
            v.push((
                c.candidate_id.clone(),
                mutation::compose_full_prompt(&cfg.incumbent, &c.suffix),
                Some(c.suffix_sha256.clone()),
            ));
        }
        v
    };

    let selection_tasks: Vec<&TaskSpec> = cfg
        .registry
        .iter()
        .filter(|t| t.split == TaskSplit::Selection)
        .collect();
    let total = SELECTION_EPISODES;
    println!(
        "starting selection tournament: model={model} endpoint={endpoint} episodes={total} conditions=5 code_commit={commit} run_prefix={run_prefix}"
    );

    let mut records = Vec::new();
    let mut counter = 0usize;
    for task in &selection_tasks {
        let (stress, count) = trace::frozen_stress(&task.family)
            .ok_or_else(|| format!("task {} family has no frozen stress", task.id))?;
        for rep in 1..=SELECTION_REPETITIONS {
            for pos in 1..SELECTION_CONDITIONS.len() as u32 {
                let cond = trace::selection_condition_at(rep, pos);
                let (prompt, suffix_sha) = prompts
                    .iter()
                    .find(|(id, _, _)| id == cond)
                    .map(|(_id, p, s)| (p.clone(), s.clone()))
                    .ok_or_else(|| format!("unregistered condition {cond}"))?;
                counter += 1;
                eprintln!(
                    "[{}/{total}] {} {} rep{} pos{} {}",
                    counter, task.id, stress, rep, pos, cond
                );
                let fault = FaultMode::for_stress(&task.fault_key, count);
                let mut tools = ToolExecutor::fresh(&task.initial_state, fault.clone());
                let outcome = kernel::run_episode(
                    |messages: &[Message], specs: &[crate::protocol::ToolSpec]| {
                        client.complete(messages, specs)
                    },
                    &mut tools,
                    &specs,
                    &prompt,
                    &task.prompt,
                    &kernel_config,
                );
                let c = kernel::core(outcome);
                let eval = crate::oracle::evaluate_task(
                    task,
                    &crate::oracle::OracleInput {
                        termination_reason: c.termination_reason.clone(),
                        initial_state: task.initial_state.clone(),
                        final_state: c.final_state.clone(),
                    },
                );
                let record = SelectionRecord {
                    experiment_id: trace::EXPERIMENT_ID.to_string(),
                    phase: "selection".to_string(),
                    run_id: format!("{run_prefix}-{:03}", counter),
                    sequence: counter as u32,
                    family: task.family.clone(),
                    task_id: task.id.clone(),
                    split: "selection".to_string(),
                    stress_level: stress.to_string(),
                    drop_count: count,
                    fault_key: task.fault_key.clone(),
                    repetition: rep,
                    condition_position: pos,
                    condition: cond.to_string(),
                    generation: if cond == "G0" { 0 } else { 1 },
                    parent_id: "G0".to_string(),
                    suffix_sha256: suffix_sha,
                    full_prompt_sha256: sha256_bytes(prompt.as_bytes()),
                    candidate_pool_sha256: pool_file_sha.clone(),
                    mutation_input_sha256: input_file_sha.clone(),
                    provenance: RecordProvenance {
                        model: model.clone(),
                        temperature: crate::protocol::TEMPERATURE,
                        redacted_endpoint: endpoint.clone(),
                        code_under_test_commit: commit.clone(),
                        incumbent_prompt_sha256: cfg.incumbent_sha.clone(),
                        mutator_prompt_sha256: cfg.mutator_sha.clone(),
                        task_registry_sha256: cfg.registry_sha.clone(),
                        stress_profile_sha256: cfg.profile_sha.clone(),
                    },
                    initial_state: task.initial_state.clone(),
                    target_state: task.target_state.clone(),
                    fault_mode: serde_json::to_value(&fault).unwrap_or(Value::Null),
                    conversation: c.conversation,
                    model_requests: c.model_requests,
                    requested_tool_calls: c.requested_tool_calls,
                    executed_tool_calls: c.executed_tool_calls,
                    environment_write_attempts: c.write_attempts,
                    final_state: c.final_state,
                    final_answer: c.final_answer,
                    model_turn_count: c.model_turn_count,
                    tool_call_count: c.tool_call_count,
                    agent_failure: c.agent_failure,
                    infrastructure_failure: c.infrastructure_failure,
                    usage_accounting_error: c.usage_accounting_error,
                    termination_reason: c.termination_reason,
                    oracle_success: eval.success,
                    oracle_failure_reasons: eval.reasons,
                    timing: c.timing,
                };
                let line = serde_json::to_string(&record)
                    .map_err(|e| format!("record serialization: {e}"))?;
                out.write_all(line.as_bytes())
                    .map_err(|e| format!("write failure: {e}"))?;
                out.write_all(b"\n")
                    .map_err(|e| format!("write failure: {e}"))?;
                out.flush().map_err(|e| format!("flush failure: {e}"))?;
                records.push(record.clone());
                println!(
                    "  {} {} turns={} calls={} termination={} oracle_success={}",
                    record.run_id,
                    cond,
                    record.model_turn_count,
                    record.tool_call_count,
                    record.termination_reason,
                    record.oracle_success
                );
            }
        }
    }
    out.flush().map_err(|e| format!("final flush: {e}"))?;
    if records.len() != total {
        return Err(format!(
            "selection produced {} episodes, expected {total} — the artifact is incomplete",
            records.len()
        ));
    }

    let raw_sha = sha256_bytes(
        &std::fs::read(&config.trajectories)
            .map_err(|e| format!("cannot re-read selection trajectories: {e}"))?,
    );
    let summary = trace::compute_selection_summary(
        &records,
        &pool,
        &model,
        &endpoint,
        &commit,
        &cfg.incumbent_sha,
        &cfg.registry_sha,
        &cfg.profile_sha,
        &pool_file_sha,
        &input_file_sha,
    );
    // The frozen selected candidate (spec §41).
    let entry = pool
        .candidates
        .iter()
        .find(|c| c.candidate_id == summary.selected.selected_candidate_id);
    let tally: Option<&CandidateTally> = summary
        .candidates
        .iter()
        .find(|t| t.candidate_id == summary.selected.selected_candidate_id);
    let sel = SelectedCandidate {
        experiment_id: trace::EXPERIMENT_ID.to_string(),
        selected_candidate_id: summary.selected.selected_candidate_id.clone(),
        parent_id: "G0".to_string(),
        candidate_pool_sha256: pool_file_sha.clone(),
        selection_raw_sha256: raw_sha,
        selection_summary_sha256: String::new(), // filled after writing the summary
        mutation_input_sha256: input_file_sha,
        suffix: entry.map(|c| c.suffix.clone()),
        suffix_sha256: entry.map(|c| c.suffix_sha256.clone()),
        full_prompt_sha256: entry.map(|c| c.full_prompt_sha256.clone()),
        selection: FrozenSelectionBlock {
            common_valid_cells: summary.common_valid_cells,
            baseline_failures: summary.g0_failures_in_common_valid,
            wins: tally.map(|t| t.wins).unwrap_or(0) as u32,
            losses: tally.map(|t| t.losses).unwrap_or(0) as u32,
            ties: tally.map(|t| t.ties).unwrap_or(0) as u32,
            net_margin: tally.map(|t| t.net_margin).unwrap_or(0),
        },
        tie_break_path: summary.selected.tie_break_path.clone(),
    };

    // Write summary, then the selected artifact with the summary hash,
    // then re-write the summary to carry its own frozen byte hash.
    let summary_value = serde_json::to_value(&summary).unwrap();
    write_json_pretty(&config.summary, &summary_value)?;
    let summary_sha = sha256_bytes(
        &std::fs::read(&config.summary).map_err(|e| format!("cannot re-read summary: {e}"))?,
    );
    let mut sel_value = serde_json::to_value(&sel).unwrap();
    sel_value["selection_summary_sha256"] = Value::String(summary_sha);
    write_json_pretty(&config.selected, &sel_value)?;

    // Report (spec §83): no subjective interpretation.
    println!("selection complete:");
    println!(
        "  common valid cells: {}/{}",
        summary.common_valid_cells,
        trace::SELECTION_CELLS
    );
    println!(
        "  G0 failures in common-valid cells: {} (potential information)",
        summary.g0_failures_in_common_valid
    );
    println!(
        "  lexicographic ranking: {}",
        summary.ranked_order.join(" > ")
    );
    println!(
        "  {:8} | {:>4} | {:>6} | {:>4} | {:>10} | selected?",
        "candidate", "wins", "losses", "ties", "net margin"
    );
    for t in &summary.candidates {
        println!(
            "  {:8} | {:>4} | {:>6} | {:>4} | {:+10} | {}",
            t.candidate_id,
            t.wins,
            t.losses,
            t.ties,
            t.net_margin,
            if t.candidate_id == sel.selected_candidate_id {
                "YES"
            } else {
                "no"
            }
        );
    }
    println!(
        "  SELECTED: {} ({})",
        sel.selected_candidate_id,
        if sel.incumbent_retained() {
            "incumbent retained"
        } else {
            "mutation"
        }
    );
    println!("  tie_break_path: {}", sel.tie_break_path);
    if summary.conclusion.result == "refuted" {
        println!(
            "EXPERIMENT CONCLUSION: REFUTED — {} Do NOT run promotion.",
            summary.conclusion.reason
        );
    } else if summary.conclusion.result == "inconclusive" {
        println!(
            "EXPERIMENT CONCLUSION: INCONCLUSIVE — {} Do NOT run promotion.",
            summary.conclusion.reason
        );
    } else {
        println!("promotion authorized — freeze the selected candidate, then run `promote`.");
    }
    Ok(())
}

// (continued)

// ===========================================================================
// Stage 4: promotion (spec §86)
// ===========================================================================

pub struct PromoteConfig {
    pub selected: PathBuf,
    pub trajectories: PathBuf,
    pub summary: PathBuf,
    pub code_commit: Option<String>,
    pub endpoint: Option<String>,
}

/// Verify the promotion leakage boundary (spec §43): the selected
/// candidate must be frozen (hash-consistent) before any promotion
/// model request.
fn verify_promotion_boundary(
    sel: &SelectedCandidate,
    pool: &CandidatePool,
    cfg: &FrozenConfig,
    selected_file_sha: &str,
    pool_file_sha: &str,
    input_file_sha: &str,
) -> Result<(), String> {
    if sel.selected_candidate_id == "G0" {
        return Err(
            "the frozen selected candidate is G0 (incumbent retained) — promotion must not run; the experiment is REFUTED".to_string(),
        );
    }
    if sel.candidate_pool_sha256 != pool_file_sha {
        return Err(
            "selected-candidate pool hash does not match the on-disk pool — promotion refuses to start".to_string(),
        );
    }
    if sel.mutation_input_sha256 != input_file_sha {
        return Err(
            "selected-candidate mutation input hash does not match the frozen input — promotion refuses to start".to_string(),
        );
    }
    let Some(entry) = pool
        .candidates
        .iter()
        .find(|c| c.candidate_id == sel.selected_candidate_id)
    else {
        return Err("the frozen selected candidate is not in the frozen pool".to_string());
    };
    if sel.suffix.as_deref() != Some(entry.suffix.as_str())
        || sel.suffix_sha256.as_deref() != Some(entry.suffix_sha256.as_str())
        || sel.full_prompt_sha256.as_deref() != Some(entry.full_prompt_sha256.as_str())
    {
        return Err(
            "selected-candidate content differs from the frozen pool — promotion refuses to start (candidate changed after freeze)"
                .to_string(),
        );
    }
    // The full prompt must re-compose from the frozen incumbent + suffix.
    let full = mutation::compose_full_prompt(&cfg.incumbent, &entry.suffix);
    if sha256_bytes(full.as_bytes()) != sel.full_prompt_sha256.clone().unwrap_or_default() {
        return Err(
            "selected full prompt does not re-compose from the frozen incumbent + suffix"
                .to_string(),
        );
    }
    if sel.selection_summary_sha256.trim().is_empty() {
        return Err(
            "selected-candidate is missing the frozen selection-summary hash (selection freeze not committed)".to_string(),
        );
    }
    let _ = selected_file_sha;
    Ok(())
}

/// Run the 96-episode two-condition confirmatory promotion test.
pub fn promote(config: &PromoteConfig) -> Result<(), String> {
    let (base_url, model, api_key) = env_config()?;
    let cfg = load_frozen_config()?;
    let commit = resolve_code_commit(config.code_commit.as_deref())?;
    let endpoint = config
        .endpoint
        .clone()
        .unwrap_or_else(|| redacted_endpoint(&base_url));
    let dir = crate_dir();

    let sel_bytes = std::fs::read(&config.selected).map_err(|e| {
        format!(
            "cannot read selected candidate {}: {e}",
            config.selected.display()
        )
    })?;
    let sel_file_sha = sha256_bytes(&sel_bytes);
    let sel: SelectedCandidate = serde_json::from_slice(&sel_bytes)
        .map_err(|e| format!("selected artifact invalid: {e}"))?;
    let (pool, _input, pool_file_sha, input_file_sha) = load_frozen_pool_and_input(
        &dir.join("candidate-pool.json"),
        &dir.join("mutation-input.json"),
        &cfg,
    )?;

    // The promotion leakage boundary (spec §43).
    verify_promotion_boundary(
        &sel,
        &pool,
        &cfg,
        &sel_file_sha,
        &pool_file_sha,
        &input_file_sha,
    )?;

    if let Some(parent) = config.trajectories.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(&config.trajectories)
            .map_err(|e| format!("cannot create trajectories file: {e}"))?,
    );

    let client = ModelClient::new(base_url, model.clone(), api_key);
    let specs: Vec<crate::protocol::ToolSpec> = ToolExecutor::specs();
    let kernel_config = KernelConfig::default();
    let selected_id = sel.selected_candidate_id.clone();
    let entry = pool
        .candidates
        .iter()
        .find(|c| c.candidate_id == selected_id)
        .ok_or("selected candidate not in pool")?;
    let selected_full = mutation::compose_full_prompt(&cfg.incumbent, &entry.suffix);

    let run_prefix = format!(
        "exp0010-prom-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    let promotion_tasks: Vec<&TaskSpec> = cfg
        .registry
        .iter()
        .filter(|t| t.split == TaskSplit::Promotion)
        .collect();
    let total = PROMOTION_EPISODES;
    println!(
        "starting promotion: model={model} endpoint={endpoint} episodes={total} conditions=2 (G0 + {selected_id}) code_commit={commit} run_prefix={run_prefix}"
    );

    let mut records = Vec::new();
    let mut counter = 0usize;
    for task in &promotion_tasks {
        let (stress, count) = trace::frozen_stress(&task.family)
            .ok_or_else(|| format!("task {} family has no frozen stress", task.id))?;
        for rep in 1..=PROMOTION_REPETITIONS {
            for (pos, cond) in trace::promotion_condition_order(rep, &selected_id)
                .into_iter()
                .enumerate()
            {
                counter += 1;
                let (prompt, suffix_sha) = if cond == "G0" {
                    (cfg.incumbent.clone(), None)
                } else {
                    (selected_full.clone(), Some(entry.suffix_sha256.clone()))
                };
                eprintln!(
                    "[{}/{total}] {} {} rep{} pos{} {}",
                    counter,
                    task.id,
                    stress,
                    rep,
                    pos + 1,
                    cond
                );
                let fault = FaultMode::for_stress(&task.fault_key, count);
                let mut tools = ToolExecutor::fresh(&task.initial_state, fault.clone());
                let outcome = kernel::run_episode(
                    |messages: &[Message], specs: &[crate::protocol::ToolSpec]| {
                        client.complete(messages, specs)
                    },
                    &mut tools,
                    &specs,
                    &prompt,
                    &task.prompt,
                    &kernel_config,
                );
                let c = kernel::core(outcome);
                let eval = crate::oracle::evaluate_task(
                    task,
                    &crate::oracle::OracleInput {
                        termination_reason: c.termination_reason.clone(),
                        initial_state: task.initial_state.clone(),
                        final_state: c.final_state.clone(),
                    },
                );
                let record = PromotionRecord {
                    experiment_id: trace::EXPERIMENT_ID.to_string(),
                    phase: "promotion".to_string(),
                    run_id: format!("{run_prefix}-{:03}", counter),
                    sequence: counter as u32,
                    family: task.family.clone(),
                    task_id: task.id.clone(),
                    split: "promotion".to_string(),
                    stress_level: stress.to_string(),
                    drop_count: count,
                    fault_key: task.fault_key.clone(),
                    repetition: rep,
                    condition_position: (pos + 1) as u32,
                    condition: cond.to_string(),
                    generation: if cond == "G0" { 0 } else { 1 },
                    parent_id: "G0".to_string(),
                    suffix_sha256: suffix_sha,
                    full_prompt_sha256: sha256_bytes(prompt.as_bytes()),
                    candidate_pool_sha256: pool_file_sha.clone(),
                    selected_candidate_sha256: sel_file_sha.clone(),
                    mutation_input_sha256: input_file_sha.clone(),
                    provenance: RecordProvenance {
                        model: model.clone(),
                        temperature: crate::protocol::TEMPERATURE,
                        redacted_endpoint: endpoint.clone(),
                        code_under_test_commit: commit.clone(),
                        incumbent_prompt_sha256: cfg.incumbent_sha.clone(),
                        mutator_prompt_sha256: cfg.mutator_sha.clone(),
                        task_registry_sha256: cfg.registry_sha.clone(),
                        stress_profile_sha256: cfg.profile_sha.clone(),
                    },
                    initial_state: task.initial_state.clone(),
                    target_state: task.target_state.clone(),
                    fault_mode: serde_json::to_value(&fault).unwrap_or(Value::Null),
                    conversation: c.conversation,
                    model_requests: c.model_requests,
                    requested_tool_calls: c.requested_tool_calls,
                    executed_tool_calls: c.executed_tool_calls,
                    environment_write_attempts: c.write_attempts,
                    final_state: c.final_state,
                    final_answer: c.final_answer,
                    model_turn_count: c.model_turn_count,
                    tool_call_count: c.tool_call_count,
                    agent_failure: c.agent_failure,
                    infrastructure_failure: c.infrastructure_failure,
                    usage_accounting_error: c.usage_accounting_error,
                    termination_reason: c.termination_reason,
                    oracle_success: eval.success,
                    oracle_failure_reasons: eval.reasons,
                    timing: c.timing,
                };
                let line = serde_json::to_string(&record)
                    .map_err(|e| format!("record serialization: {e}"))?;
                out.write_all(line.as_bytes())
                    .map_err(|e| format!("write failure: {e}"))?;
                out.write_all(b"\n")
                    .map_err(|e| format!("write failure: {e}"))?;
                out.flush().map_err(|e| format!("flush failure: {e}"))?;
                records.push(record.clone());
                println!(
                    "  {} {} turns={} calls={} termination={} oracle_success={}",
                    record.run_id,
                    cond,
                    record.model_turn_count,
                    record.tool_call_count,
                    record.termination_reason,
                    record.oracle_success
                );
            }
        }
    }
    out.flush().map_err(|e| format!("final flush: {e}"))?;
    if records.len() != total {
        return Err(format!(
            "promotion produced {} episodes, expected {total} — the artifact is incomplete",
            records.len()
        ));
    }

    let summary = trace::compute_promotion_summary(
        &records,
        &selected_id,
        &model,
        &endpoint,
        &commit,
        &cfg.incumbent_sha,
        &cfg.registry_sha,
        &cfg.profile_sha,
        &pool_file_sha,
        &sel_file_sha,
        &input_file_sha,
    );
    write_json_pretty(&config.summary, &serde_json::to_value(&summary).unwrap())?;

    println!("promotion complete:");
    println!(
        "  G0 successes: {}  selected ({selected_id}) successes: {}",
        summary.g0_successes, summary.selected_successes
    );
    println!(
        "  valid pairs: {}  potential information (G0 failures in valid pairs): {}  informative pairs: {}",
        summary.valid_pairs, summary.potential_information_capacity, summary.informative_pairs
    );
    println!(
        "  wins: {}  losses: {}  ties: {}  sign-test p: {}  classification: {}",
        summary.wins, summary.losses, summary.ties, summary.sign_test_p, summary.classification
    );
    println!(
        "  EXPERIMENT CONCLUSION: {} — {}",
        summary.conclusion.result.to_uppercase(),
        summary.conclusion.reason
    );
    Ok(())
}
// (continued)

// ===========================================================================
// Network-free self-test (component checks + verifiers + tamper suite)
// ===========================================================================

struct Check {
    name: &'static str,
    ok: bool,
    detail: String,
}

/// The value-form of one synthetic artifact bundle, cloned per tamper.
struct Bundle {
    discovery: Vec<Value>,
    discovery_summary: Value,
    mutation_input: Value,
    generation: Value,
    pool: Value,
    selection: Vec<Value>,
    selection_summary: Value,
    selected: Value,
    promotion: Vec<Value>,
    promotion_summary: Value,
}

fn clone_bundle(b: &Bundle) -> Bundle {
    Bundle {
        discovery: b.discovery.clone(),
        discovery_summary: b.discovery_summary.clone(),
        mutation_input: b.mutation_input.clone(),
        generation: b.generation.clone(),
        pool: b.pool.clone(),
        selection: b.selection.clone(),
        selection_summary: b.selection_summary.clone(),
        selected: b.selected.clone(),
        promotion: b.promotion.clone(),
        promotion_summary: b.promotion_summary.clone(),
    }
}

/// Run every stage verifier on a bundle; returns a combined error list.
fn verify_all(
    b: &Bundle,
    ctx: &trace::VerifierContext,
    input_sha: &str,
    pool_sha: &str,
    selected_sha: &str,
) -> Vec<String> {
    let mut errors = Vec::new();
    let d: Vec<trace::DiscoveryRecord> = match b
        .discovery
        .iter()
        .map(|v| serde_json::from_value::<trace::DiscoveryRecord>(v.clone()))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(v) => v,
        Err(_) => {
            errors.push("discovery records do not deserialize (schema tamper)".to_string());
            return errors;
        }
    };
    {
        let s_val = b.discovery_summary.clone();
        let in_val = b.mutation_input.clone();
        let errs = trace::verify_discovery(&d, &s_val, &in_val, input_sha, ctx);
        if !errs.is_empty() {
            errors.extend(errs.into_iter().map(|e| format!("discovery: {e}")));
        }
    }
    if let (Ok(pool), Ok(generation_v)) = (
        serde_json::from_value::<CandidatePool>(b.pool.clone()),
        serde_json::from_value::<Value>(b.generation.clone()),
    ) {
        let in_val = b.mutation_input.clone();
        let errs = trace::verify_mutation(&generation_v, &pool, &in_val, input_sha, ctx);
        if !errs.is_empty() {
            errors.extend(errs.into_iter().map(|e| format!("mutation: {e}")));
        }
    } else {
        errors.push("mutation artifacts do not deserialize (schema tamper)".to_string());
    }
    if let (Ok(sel_recs), Ok(pool)) = (
        b.selection
            .iter()
            .map(|v| serde_json::from_value::<trace::SelectionRecord>(v.clone()))
            .collect::<Result<Vec<_>, _>>(),
        serde_json::from_value::<CandidatePool>(b.pool.clone()),
    ) {
        let errs = trace::verify_selection(
            &sel_recs,
            &b.selection_summary,
            &b.selected,
            &pool,
            pool_sha,
            input_sha,
            ctx,
        );
        if !errs.is_empty() {
            errors.extend(errs.into_iter().map(|e| format!("selection: {e}")));
        }
    } else {
        errors.push("selection records do not deserialize (schema tamper)".to_string());
    }
    if let (Ok(prom_recs), Ok(pool)) = (
        b.promotion
            .iter()
            .map(|v| serde_json::from_value::<trace::PromotionRecord>(v.clone()))
            .collect::<Result<Vec<_>, _>>(),
        serde_json::from_value::<CandidatePool>(b.pool.clone()),
    ) {
        let errs = trace::verify_promotion(
            &prom_recs,
            &b.promotion_summary,
            &b.selected,
            &pool,
            pool_sha,
            input_sha,
            selected_sha,
            ctx,
        );
        if !errs.is_empty() {
            errors.extend(errs.into_iter().map(|e| format!("promotion: {e}")));
        }
    } else {
        errors.push("promotion records do not deserialize (schema tamper)".to_string());
    }
    errors
}

/// One tamper case: name + mutation closure + which verifier output to check.
struct TamperCase {
    name: &'static str,
    f: Box<dyn Fn(&mut Bundle)>,
}

/// The full tamper suite (spec §70/§71/§72).
fn tamper_cases() -> Vec<TamperCase> {
    let mut c = Vec::new();
    // --- leakage (§70) ---
    c.push(TamperCase {
        name: "leak: selection task in mutation input",
        f: Box::new(|b: &mut Bundle| {
            if let Some(ep) = b
                .mutation_input
                .get_mut("discovery_episodes")
                .and_then(|a| a.get_mut(0))
            {
                ep["task_id"] = Value::String("SEL1".to_string());
                ep["final_state"] = json!({"x": "V_KEEP_S1", "y": "V_S1_0"});
            }
        }),
    });
    c.push(TamperCase {
        name: "leak: promotion task in mutation input",
        f: Box::new(|b: &mut Bundle| {
            if let Some(ep) = b
                .mutation_input
                .get_mut("discovery_episodes")
                .and_then(|a| a.get_mut(0))
            {
                ep["task_id"] = Value::String("PRO1".to_string());
                ep["final_state"] = json!({"x": "V_P1_0", "y": "V_KEEP_P1"});
            }
        }),
    });
    c.push(TamperCase {
        name: "leak: hidden fault field in mutation input",
        f: Box::new(|b: &mut Bundle| {
            if let Some(ep) = b
                .mutation_input
                .get_mut("discovery_episodes")
                .and_then(|a| a.get_mut(0))
            {
                ep["fault_reason"] = Value::String("drop_first_n_writes".to_string());
            }
        }),
    });
    c.push(TamperCase {
        name: "leak: fault audit \"applied\" in mutation input",
        f: Box::new(|b: &mut Bundle| {
            if let Some(ep) = b
                .mutation_input
                .get_mut("discovery_episodes")
                .and_then(|a| a.get_mut(0))
            {
                ep["applied"] = Value::Bool(false);
            }
        }),
    });
    // --- mutation / pool (§70/§71) ---
    c.push(TamperCase {
        name: "mutation: known 0009 repair injected into the pool by code",
        f: Box::new(|b: &mut Bundle| {
            let repair = "After every successful state_write, read the same key. If the observed value differs from the value you intended to write, write it again and re-read it. Repeat until the value matches, but make at most five state_write attempts for that requested key.";
            if let Some(cand) = b.pool.get_mut("candidates").and_then(|a| a.get_mut(0)) {
                cand["suffix"] = Value::String(repair.to_string());
            }
        }),
    });
    c.push(TamperCase {
        name: "mutation: wrong candidate count",
        f: Box::new(|b: &mut Bundle| {
            if let Some(cands) = b.pool.get_mut("candidates").and_then(Value::as_array_mut) {
                let extra = cands[0].clone();
                cands.push(extra);
            }
        }),
    });
    c.push(TamperCase {
        name: "selection: candidate suffix changed (vs raw response)",
        f: Box::new(|b: &mut Bundle| {
            if let Some(cand) = b.pool.get_mut("candidates").and_then(|a| a.get_mut(2)) {
                let s = cand
                    .get("suffix")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                cand["suffix"] = Value::String(format!("{s} and verify again"));
            }
        }),
    });
    c.push(TamperCase {
        name: "selection: candidate hash changed",
        f: Box::new(|b: &mut Bundle| {
            if let Some(cand) = b.pool.get_mut("candidates").and_then(|a| a.get_mut(3)) {
                cand["suffix_sha256"] = Value::String("f".repeat(64));
            }
        }),
    });
    // --- selection rule (§71) ---
    c.push(TamperCase {
        name: "selection: selection score (wins) tampered in summary",
        f: Box::new(|b: &mut Bundle| {
            if let Some(cand) = b
                .selection_summary
                .get_mut("candidates")
                .and_then(|a| a.get_mut(1))
            {
                let w = cand.get("wins").and_then(Value::as_u64).unwrap_or(0) + 1;
                cand["wins"] = Value::from(w);
            }
        }),
    });
    c.push(TamperCase {
        name: "selection: tie-break ordering tampered in summary",
        f: Box::new(|b: &mut Bundle| {
            if let Some(order) = b
                .selection_summary
                .get_mut("ranked_order")
                .and_then(Value::as_array_mut)
            {
                order.swap(0, 1);
            }
        }),
    });
    c.push(TamperCase {
        name: "selection: higher-ID candidate selected despite exact tie",
        f: Box::new(|b: &mut Bundle| {
            b.selected["selected_candidate_id"] = Value::String("C3".to_string());
            b.selected["suffix"] = Value::Null;
        }),
    });
    c.push(TamperCase {
        name: "selection: candidate selected with net margin <= 0",
        f: Box::new(|b: &mut Bundle| {
            b.selected["selected_candidate_id"] = Value::String("C4".to_string());
            b.selected["suffix"] = Value::Null;
        }),
    });
    c.push(TamperCase {
        name: "selection: G0 retained despite a positive-net candidate",
        f: Box::new(|b: &mut Bundle| {
            b.selected["selected_candidate_id"] = Value::String("G0".to_string());
            b.selected["suffix"] = Value::Null;
        }),
    });
    c.push(TamperCase {
        name: "selection: cost tampered (cost is not a selection input)",
        f: Box::new(|b: &mut Bundle| {
            if let Some(c) = b
                .selection_summary
                .get_mut("condition_cost")
                .and_then(|a| a.get_mut(0))
            {
                let n = c
                    .get("all")
                    .and_then(|a| a.get("nominal_prompt_tokens"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    + 100_000;
                if let Some(all) = c.get_mut("all") {
                    all["nominal_prompt_tokens"] = Value::from(n);
                }
            }
        }),
    });
    // --- promotion (§70/§72) ---
    c.push(TamperCase {
        name: "promotion: unselected candidate appears",
        f: Box::new(|b: &mut Bundle| {
            if let Some(r) = b.promotion.get_mut(3) {
                r["condition"] = Value::String("C1".to_string());
            }
        }),
    });
    c.push(TamperCase {
        name: "promotion: candidate text changed after freeze",
        f: Box::new(|b: &mut Bundle| {
            if let Some(conv) = b
                .promotion
                .get_mut(1)
                .and_then(|r| r.get_mut("conversation"))
                .and_then(Value::as_array_mut)
                .and_then(|a| a.get_mut(0))
            {
                let s = conv
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                conv["content"] = Value::String(format!("{s} TAMPERED"));
            }
        }),
    });
    c.push(TamperCase {
        name: "promotion: wrong promotion candidate (condition)",
        f: Box::new(|b: &mut Bundle| {
            if let Some(r) = b.promotion.get_mut(5) {
                r["condition"] = Value::String("C3".to_string());
            }
        }),
    });
    c.push(TamperCase {
        name: "promotion: extra candidate condition (episode count)",
        f: Box::new(|b: &mut Bundle| {
            if let Some(r) = b.promotion.first() {
                b.promotion.push(r.clone());
            }
        }),
    });
    c.push(TamperCase {
        name: "promotion: missing pair",
        f: Box::new(|b: &mut Bundle| {
            b.promotion.pop();
        }),
    });
    c.push(TamperCase {
        name: "promotion: duplicate pair",
        f: Box::new(|b: &mut Bundle| {
            // Copy record 1 over record 2's cell (same task, rep, condition).
            if let (Some(src), Some(dst)) = (b.promotion.get(1).cloned(), b.promotion.get_mut(2)) {
                *dst = src;
            }
        }),
    });
    c.push(TamperCase {
        name: "promotion: wrong stress",
        f: Box::new(|b: &mut Bundle| {
            if let Some(r) = b.promotion.get_mut(7) {
                r["stress_level"] = Value::String("S3".to_string());
            }
        }),
    });
    c.push(TamperCase {
        name: "promotion: oracle tampered",
        f: Box::new(|b: &mut Bundle| {
            if let Some(r) = b.promotion.get_mut(10) {
                r["oracle_success"] = Value::Bool(
                    !r.get("oracle_success")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                );
            }
        }),
    });
    c.push(TamperCase {
        name: "promotion: potential information tampered",
        f: Box::new(|b: &mut Bundle| {
            let v = b
                .promotion_summary
                .get("potential_information_capacity")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                + 1;
            b.promotion_summary["potential_information_capacity"] = Value::from(v);
        }),
    });
    c.push(TamperCase {
        name: "promotion: informative count tampered",
        f: Box::new(|b: &mut Bundle| {
            let v = b
                .promotion_summary
                .get("informative_pairs")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                + 1;
            b.promotion_summary["informative_pairs"] = Value::from(v);
        }),
    });
    c.push(TamperCase {
        name: "promotion: sign-test p tampered",
        f: Box::new(|b: &mut Bundle| {
            b.promotion_summary["sign_test_p"] = Value::from(0.9);
        }),
    });
    c.push(TamperCase {
        name: "promotion: classification tampered",
        f: Box::new(|b: &mut Bundle| {
            b.promotion_summary["classification"] = Value::String("quality_regression".to_string());
        }),
    });
    c.push(TamperCase {
        name: "promotion: conclusion tampered",
        f: Box::new(|b: &mut Bundle| {
            if let Some(conc) = b.promotion_summary.get_mut("conclusion") {
                conc["result"] = Value::String("refuted".to_string());
            }
        }),
    });
    c
}

/// The full network-free self-test (spec §75).
pub fn self_test() -> Result<(), String> {
    let mut checks: Vec<Check> = Vec::new();
    let mut add = |name: &'static str, ok: bool, detail: String| {
        checks.push(Check { name, ok, detail });
    };

    // 1. Registry.
    match load_tasks(&crate_dir().join("tasks.json")) {
        Ok(file) => {
            let ok = file.tasks.len() == 12;
            add(
                "12-task registry (3/3/6 splits)",
                ok,
                format!("{} tasks", file.tasks.len()),
            );
        }
        Err(e) => add("12-task registry (3/3/6 splits)", false, e),
    }
    // 2. Frozen stress profile.
    match trace::load_stress_profile(&crate_dir().join("stress-profile.json")) {
        Ok(p) => add(
            "frozen 0009 stress profile (S2/S1/S2)",
            p.families.len() == 3,
            "direct_set=S2 c=2, conditional_set=S1 c=1, replacement=S2 c=2".to_string(),
        ),
        Err(e) => add("frozen 0009 stress profile (S2/S1/S2)", false, e),
    }
    // 3. Drop fault semantics.
    {
        let mut exec = crate::tools::ToolExecutor::fresh(
            &json!({"x": "A0", "y": "B"}),
            FaultMode::DropFirstNWrites {
                key: "x".into(),
                count: 2,
            },
        );
        for _ in 0..3 {
            exec.execute("state_write", &json!({"key": "x", "value": "A1"}))
                .unwrap();
        }
        let ok = exec.final_state_value()["x"] == "A1"
            && !exec.write_attempts()[0].applied
            && !exec.write_attempts()[1].applied
            && exec.write_attempts()[2].applied;
        add(
            "DropFirstNWrites fault semantics",
            ok,
            "first N target writes silently dropped and audited".to_string(),
        );
    }
    // 4. Oracle independence.
    {
        let task = TaskSpec {
            id: "PRO1".into(),
            name: "t".into(),
            family: "direct_set".into(),
            split: TaskSplit::Promotion,
            prompt: "p".into(),
            initial_state: json!({"x": "A", "y": "B"}),
            target_state: json!({"x": "C", "y": "B"}),
            fault_key: "x".into(),
        };
        let pass = crate::oracle::evaluate_task(
            &task,
            &crate::oracle::OracleInput {
                termination_reason: "completed".into(),
                initial_state: json!({"x": "A", "y": "B"}),
                final_state: json!({"x": "C", "y": "B"}),
            },
        );
        let fail = crate::oracle::evaluate_task(
            &task,
            &crate::oracle::OracleInput {
                termination_reason: "completed".into(),
                initial_state: json!({"x": "A", "y": "B"}),
                final_state: json!({"x": "A", "y": "B"}),
            },
        );
        add(
            "oracle independence + pass/fail",
            pass.success && !fail.success,
            "state-only judging".to_string(),
        );
    }
    // 5. Sign test. n=32, k=min(30,2)=2 → p = 2·(Σᵢ₌₀..₂ C(32,ᵢ))/2³².
    // Σ = 1 + 32 + 496 = 529 → p = 1058/2³². (The pre-registration
    // derives this from the observed (w,l); the self-test pins the
    // synthetic design's value 30/2/16 → n=32.)
    {
        let p = crate::stats::exact_sign_test_p(30, 2);
        let want = 1058.0 / 2f64.powi(32);
        add(
            "exact sign test",
            (p - want).abs() < 1e-18,
            format!("p(30,2) = {p}"),
        );
    }
    // 6. Selection rule (pure).
    {
        let t = |id: &str, o: u32, w: u64, l: u64| CandidateTally {
            candidate_id: id.into(),
            ordinal: o,
            wins: w,
            losses: l,
            ties: 0,
            net_margin: w as i64 - l as i64,
            diagnostic_p_exploratory: crate::stats::exact_sign_test_p(w, l),
        };
        let sel = crate::selection::select(&[
            t("C1", 1, 3, 2),
            t("C2", 2, 13, 0),
            t("C3", 3, 3, 2),
            t("C4", 4, 2, 3),
        ]);
        let retained = crate::selection::select(&[
            t("C1", 1, 1, 1),
            t("C2", 2, 0, 1),
            t("C3", 3, 1, 1),
            t("C4", 4, 0, 0),
        ]);
        add(
            "frozen selection rule (net margin > 0 required; ordinal tie-break)",
            sel.selected_candidate_id == "C2"
                && retained.selected_candidate_id == "G0"
                && retained.incumbent_retained,
            format!("best=C2 retained={}", retained.incumbent_retained),
        );
    }
    // 7. Mutation validation.
    {
        let reg = load_tasks(&crate_dir().join("tasks.json"))
            .ok()
            .map(|f| f.tasks)
            .unwrap_or_default();
        let bad = "Always check the experiment before acting on state.";
        let good = "Always read a key before writing it, so actions follow observed state.";
        add(
            "candidate suffix validator",
            mutation::validate_suffix(good, &reg).is_empty()
                && !mutation::validate_suffix(bad, &reg).is_empty(),
            "generic suffix accepted; experiment-referencing suffix rejected".to_string(),
        );
    }

    // 8. Synthetic artifacts through the full verifiers.
    let art = trace::build_synthetic_artifacts();
    let ctx = art.ctx.clone();
    let input_sha = art.input_file_sha;
    let pool_sha = art.pool_file_sha;
    let selected_sha = art.selected_file_sha;
    let bundle = Bundle {
        discovery: art
            .discovery
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?,
        discovery_summary: serde_json::to_value(&art.discovery_summary)
            .map_err(|e| e.to_string())?,
        mutation_input: serde_json::to_value(&art.mutation_input).map_err(|e| e.to_string())?,
        generation: serde_json::to_value(&art.generation).map_err(|e| e.to_string())?,
        pool: serde_json::to_value(&art.pool).map_err(|e| e.to_string())?,
        selection: art
            .selection
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?,
        selection_summary: serde_json::to_value(&art.selection_summary)
            .map_err(|e| e.to_string())?,
        selected: serde_json::to_value(&art.selected).map_err(|e| e.to_string())?,
        promotion: art
            .promotion
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?,
        promotion_summary: serde_json::to_value(&art.promotion_summary)
            .map_err(|e| e.to_string())?,
    };
    let clean_errors = verify_all(&bundle, &ctx, input_sha, pool_sha, selected_sha);
    add(
        "all four stage verifiers (synthetic artifacts)",
        clean_errors.is_empty(),
        if clean_errors.is_empty() {
            "discovery/mutation/selection/promotion all verify clean".to_string()
        } else {
            clean_errors
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        },
    );

    // 9. Tamper suite: every case MUST fail at least one verifier.
    let cases = tamper_cases();
    let mut tamper_ok = true;
    for case in &cases {
        let mut b = clone_bundle(&bundle);
        (case.f)(&mut b);
        let errs = verify_all(&b, &ctx, input_sha, pool_sha, selected_sha);
        let ok = !errs.is_empty();
        tamper_ok &= ok;
        add(
            case.name,
            ok,
            if ok {
                format!("{} issue(s) caught", errs.len())
            } else {
                "TAMPER NOT DETECTED".to_string()
            },
        );
    }

    // Report.
    let mut failures = 0usize;
    for c in &checks {
        println!(
            "[{}] {:55} {}",
            if c.ok { "ok" } else { "FAIL" },
            c.name,
            c.detail
        );
        if !c.ok {
            failures += 1;
        }
    }
    println!(
        "self-test: {} checks, {} failed, {} tamper cases, all detected: {tamper_ok}",
        checks.len(),
        failures,
        cases.len()
    );
    if failures > 0 {
        return Err(format!("{failures} self-test check(s) failed"));
    }
    Ok(())
}
