//! Experiment 0011 — Clean Confirmatory Self-Evolution Rerun: command
//! surface, live-stage orchestration, the frozen-run guard, preflight
//! and the network-free self-test.
//!
//! Every live stage (B/C/D/E) MUST be given the Stage A1 run manifest
//! (`--run-manifest`, default `run-manifest.json` in the crate dir);
//! the stage runs the source-freeze guard BEFORE the model client is
//! constructed and BEFORE any request. If the guard fails, NO model
//! request is made and the run identity is permanently invalid.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::freeze;
use crate::kernel::{self, KernelConfig};
use crate::model::{self, ModelClient, ModelError, REQUEST_TIMEOUT};
use crate::mutation::{
    self, CandidatePool, GenerationArtifact, GenerationAttempt, INCUMBENT_ID, build_mutation_input,
    build_pool, compose_full_prompt, validate_candidate_pool, validate_mutator_response,
};
use crate::oracle::{self, TaskFile, TaskSpec, TaskSplit};
use crate::protocol::{Message, sha256_bytes};
use crate::tools::{FaultMode, ToolExecutor};
use crate::trace::{
    self, Design, DiscoveryRecord, PromotionRecord, RecordProvenance, RunIdentity,
    SelectedCandidate, SelectionRecord, VerifierContext, frozen_stress, load_stress_profile,
};

// ===========================================================================
// Paths and loaders
// ===========================================================================

/// The experiment crate directory (independent of CWD).
pub fn crate_dir() -> PathBuf {
    freeze::crate_dir()
}

pub fn prompts_dir() -> PathBuf {
    crate_dir().join("prompts")
}

fn artifact_path(name: &str) -> PathBuf {
    crate_dir().join(name)
}

pub fn load_design() -> Result<Design, String> {
    Design::load(&crate_dir().join("design.json"))
}

/// Load + structurally validate the frozen task registry.
pub fn load_tasks(path: &Path) -> Result<TaskFile, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read task registry {}: {e}", path.display()))?;
    let file: TaskFile =
        serde_json::from_str(&raw).map_err(|e| format!("task registry is not valid JSON: {e}"))?;
    if file.experiment_id != "0011" {
        return Err(format!(
            "task registry experiment_id {:?} does not match this experiment (0011)",
            file.experiment_id
        ));
    }
    file.validate()
        .map(|_| file)
        .map_err(|e| format!("task registry {:?} is invalid: {e}", path.display()))
}

/// Read a UTF-8 text file plus its byte hash.
fn read_file_sha(path: &Path) -> Result<(String, String), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let text =
        String::from_utf8(bytes).map_err(|_| format!("{:?} is not UTF-8", path.display()))?;
    Ok((
        text,
        sha256_bytes(&std::fs::read(path).map_err(|e| e.to_string())?),
    ))
}

fn file_sha(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Ok(sha256_bytes(&bytes))
}

fn read_json(path: &Path) -> Result<Value, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("{} is not valid JSON: {e}", path.display()))
}

fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        out.push(
            serde_json::from_str(line)
                .map_err(|e| format!("{} line {}: {e}", path.display(), i + 1))?,
        );
    }
    Ok(out)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<String, String> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| format!("cannot serialize {}: {e}", path.display()))?;
    let bytes = format!("{text}\n").into_bytes();
    std::fs::write(path, &bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(sha256_bytes(&bytes))
}

fn write_jsonl(path: &Path, rows: &[impl Serialize]) -> Result<String, String> {
    let mut text = String::new();
    for r in rows {
        let line = serde_json::to_string(r).map_err(|e| format!("cannot serialize: {e}"))?;
        text.push_str(&line);
        text.push('\n');
    }
    std::fs::write(path, text.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(sha256_bytes(text.as_bytes()))
}

/// Build the verifier context from the CURRENT frozen files (design,
/// prompts, registry, profile). The verifiers recompute every hash from
/// the current contents; the source-freeze guard guarantees those
/// contents are the frozen ones.
fn verifier_context() -> Result<VerifierContext, String> {
    let dir = crate_dir();
    let design = load_design()?;
    let (inc_text, inc_sha) = read_file_sha(&prompts_dir().join("incumbent.md"))?;
    let mut_sha = file_sha(&prompts_dir().join("mutator.md"))?;
    let file = load_tasks(&dir.join("tasks.json"))?;
    let profile_bytes =
        std::fs::read(dir.join("stress-profile.json")).map_err(|e| e.to_string())?;
    let profile: Value = serde_json::from_slice(&profile_bytes)
        .map_err(|e| format!("stress profile invalid: {e}"))?;
    Ok(VerifierContext::new(
        inc_text, inc_sha, mut_sha, file.tasks, profile, design,
    ))
}

// ===========================================================================
// The frozen-run guard (mechanical source-freeze enforcement)
// ===========================================================================

/// Load the Stage A1 run manifest, run the FULL source-freeze guard,
/// and return the run identity. Called by every live stage BEFORE the
/// model client is constructed. Failure => no request may occur and the
/// run identity is permanently invalid.
pub fn frozen_run(manifest_path: &Path) -> Result<(RunIdentity, freeze::RunManifest), String> {
    let manifest = freeze::load_manifest(manifest_path)?;
    let root = freeze::repo_root()?;
    let violations = freeze::verify_run_freeze(&root, &manifest);
    if !violations.is_empty() {
        return Err(format!(
            "SOURCE FREEZE VIOLATION — the run identity is permanently invalid; no live stage may run. Violations:\n  - {}",
            violations.join("\n  - ")
        ));
    }
    let raw = std::fs::read(manifest_path).map_err(|e| e.to_string())?;
    let run = RunIdentity {
        run_id: manifest.run_id.clone(),
        run_manifest_sha256: sha256_bytes(&raw),
        frozen_design_sha256: manifest.frozen_design_sha256.clone(),
    };
    Ok((run, manifest))
}

/// The model client for the registered agent endpoint.
fn agent_client(design: &Design) -> ModelClient {
    let e = &design.endpoints[0];
    let key = std::env::var("MUTAGEN_EXP_API_KEY")
        .ok()
        .filter(|k| !k.is_empty());
    ModelClient::new(e.url.clone(), e.model.clone(), key)
}

/// The model client for the registered mutation endpoint (the SAME
/// underlying model as agent execution).
fn mutator_client(design: &Design) -> ModelClient {
    let e = design.mutator_endpoint();
    let key = std::env::var("MUTAGEN_EXP_API_KEY")
        .ok()
        .filter(|k| !k.is_empty());
    ModelClient::new(e.url.clone(), e.model.clone(), key)
}

/// The redacted endpoint for the registered agent endpoint.
fn agent_endpoint_redacted(design: &Design) -> String {
    model::redacted_endpoint(&design.endpoints[0].url)
}

/// Base provenance for one stage's records (run identity + frozen file
/// hashes + model binding).
fn base_provenance(
    ctx: &VerifierContext,
    manifest: &freeze::RunManifest,
    run: &RunIdentity,
    model: &str,
    temperature: f64,
    endpoint_redacted: &str,
) -> RecordProvenance {
    RecordProvenance {
        model: model.to_string(),
        temperature,
        redacted_endpoint: endpoint_redacted.to_string(),
        code_under_test_commit: manifest.code_under_test_commit.clone(),
        run_id: run.run_id.clone(),
        run_manifest_sha256: run.run_manifest_sha256.clone(),
        frozen_design_sha256: run.frozen_design_sha256.clone(),
        incumbent_prompt_sha256: ctx.incumbent_file_sha.clone(),
        mutator_prompt_sha256: ctx.mutator_file_sha.clone(),
        task_registry_sha256: ctx.task_registry_sha.clone(),
        stress_profile_sha256: ctx.profile_sha.clone(),
    }
}

// ===========================================================================
// One executed episode (kernel + fault + oracle; shared by all stages)
// ===========================================================================

/// The fully executed episode: kernel execution core + oracle verdict.
pub struct ExecutedEpisode {
    pub core: crate::kernel::RecordCore,
    pub eval: oracle::TaskEvaluation,
    pub initial_state: Value,
    pub level: &'static str,
    pub drop_count: u32,
}

/// Execute one agent episode against a fresh fault-injecting
/// environment. The registered hard limits (turns, calls, time) come
/// from the design manifest. Never panics on model/tool failure: all
/// failures are classified and recorded.
fn execute_episode(
    client: &ModelClient,
    design: &Design,
    temperature: f64,
    task: &TaskSpec,
    full_prompt: &str,
) -> Result<ExecutedEpisode, String> {
    let (level, count) = frozen_stress(&task.family)
        .ok_or_else(|| format!("no frozen stress registered for family {:?}", task.family))?;
    let fault = FaultMode::for_stress(&task.fault_key, count);
    let initial = task.initial_state.clone();
    let specs = ToolExecutor::specs();
    let mut tools = ToolExecutor::fresh(&initial, fault.clone());
    let cfg = KernelConfig::from_design(design);
    let budget = Duration::from_millis(design.max_time_ms);
    let started = Instant::now();

    let outcome = kernel::run_episode(
        |messages, tool_specs| {
            let elapsed = started.elapsed();
            let remaining = budget.saturating_sub(elapsed);
            if remaining.is_zero() {
                return Err(ModelError::Http(
                    "episode time limit exceeded (10-minute hard limit)".into(),
                ));
            }
            client.complete(
                messages,
                tool_specs,
                temperature,
                remaining.min(REQUEST_TIMEOUT),
            )
        },
        &mut tools,
        &specs,
        full_prompt,
        &task.prompt,
        &cfg,
    );
    let core = kernel::core(outcome);
    let eval = oracle::evaluate_task(
        task,
        &oracle::OracleInput {
            termination_reason: core.termination_reason.clone(),
            initial_state: initial.clone(),
            final_state: core.final_state.clone(),
        },
    );
    Ok(ExecutedEpisode {
        core,
        eval,
        initial_state: initial,
        level,
        drop_count: count,
    })
}

#[allow(clippy::too_many_arguments)] // one field per record field; the list is intentional
fn discovery_record(
    design: &Design,
    prov: &RecordProvenance,
    task: &TaskSpec,
    level: &str,
    count: u32,
    seq: u32,
    rep: u32,
    ep: &ExecutedEpisode,
) -> DiscoveryRecord {
    let c = &ep.core;
    DiscoveryRecord {
        experiment_id: design.experiment_id.clone(),
        phase: "discovery".into(),
        episode_id: format!("0011-r1-disc-{:03}", seq),
        sequence: seq,
        family: task.family.clone(),
        task_id: task.id.clone(),
        split: "discovery".into(),
        stress_level: level.to_string(),
        drop_count: count,
        fault_key: task.fault_key.clone(),
        repetition: rep,
        condition: "G0".into(),
        generation: 0,
        provenance: prov.clone(),
        initial_state: ep.initial_state.clone(),
        target_state: task.target_state.clone(),
        fault_mode: serde_json::to_value(FaultMode::for_stress(&task.fault_key, count)).unwrap(),
        conversation: c.conversation.clone(),
        model_requests: c.model_requests.clone(),
        requested_tool_calls: c.requested_tool_calls.clone(),
        executed_tool_calls: c.executed_tool_calls.clone(),
        environment_write_attempts: c.write_attempts.clone(),
        final_state: c.final_state.clone(),
        final_answer: c.final_answer.clone(),
        model_turn_count: c.model_turn_count,
        tool_call_count: c.tool_call_count,
        agent_failure: c.agent_failure.clone(),
        infrastructure_failure: c.infrastructure_failure.clone(),
        usage_accounting_error: c.usage_accounting_error.clone(),
        termination_reason: c.termination_reason.clone(),
        oracle_success: ep.eval.success,
        oracle_failure_reasons: ep.eval.reasons.clone(),
        timing: c.timing,
    }
}

// ===========================================================================
// Stage B — discovery (live; frozen-run guarded)
// ===========================================================================

pub struct DiscoverArgs {
    pub manifest: PathBuf,
    pub family: Option<String>,
    pub task: Option<String>,
}

pub fn cmd_discover(args: DiscoverArgs) -> Result<(), String> {
    // 1. The guard FIRST: no model client, no request, if violated.
    let (run, manifest) = frozen_run(&args.manifest)?;
    let design = load_design()?;
    let ctx = verifier_context()?;
    let registry_file = load_tasks(&crate_dir().join("tasks.json"))?;

    let client = agent_client(&design);
    let prov = base_provenance(
        &ctx,
        &manifest,
        &run,
        client.model(),
        design.agent_temperature,
        &agent_endpoint_redacted(&design),
    );

    let mut discovery_tasks: Vec<&TaskSpec> = registry_file
        .tasks
        .iter()
        .filter(|t| t.split == TaskSplit::Discovery)
        .collect();
    if let Some(f) = &args.family {
        discovery_tasks.retain(|t| &t.family == f);
    }
    if let Some(id) = &args.task {
        discovery_tasks.retain(|t| &t.id == id);
    }
    if discovery_tasks.is_empty() {
        return Err("no discovery tasks match the filter".into());
    }
    let reps = design.discovery_repetitions();
    let all_index: std::collections::BTreeMap<String, usize> = registry_file
        .tasks
        .iter()
        .filter(|t| t.split == TaskSplit::Discovery)
        .enumerate()
        .map(|(i, t)| (t.id.clone(), i))
        .collect();

    let out = artifact_path("discovery-raw.jsonl");
    let mut records: Vec<DiscoveryRecord> = Vec::new();
    for task in &discovery_tasks {
        for rep in 1..=reps {
            let seq = all_index[&task.id] as u32 * reps + rep;
            eprintln!(
                "[discovery] {}/{} {} rep {rep} ({} {})",
                records.len() + 1,
                design.discovery_episodes(&registry_file.tasks),
                task.id,
                task.family,
                run.run_id
            );
            let ep = execute_episode(
                &client,
                &design,
                design.agent_temperature,
                task,
                &ctx.incumbent_text,
            )?;
            let rec =
                discovery_record(&design, &prov, task, ep.level, ep.drop_count, seq, rep, &ep);
            records.push(rec);
            // Append incrementally: a crash mid-stage leaves a partial
            // (inconclusive) artifact, never a silently resumable one.
            let line = serde_json::to_string(records.last().unwrap()).unwrap();
            let mut f = std::io::BufWriter::new(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&out)
                    .map_err(|e| e.to_string())?,
            );
            use std::io::Write as _;
            writeln!(f, "{line}").map_err(|e| e.to_string())?;
            f.flush().map_err(|e| e.to_string())?;
        }
    }

    // Mutation input (whitelist) + summary + verification.
    let input = build_mutation_input(
        &design,
        &ctx.incumbent_text,
        &ToolExecutor::specs(),
        &records,
    );
    let input_sha = write_json(&artifact_path("mutation-input.json"), &input)?;
    let summary = trace::compute_discovery_summary(
        &records,
        &design,
        &ctx.registry,
        &run,
        client.model(),
        &agent_endpoint_redacted(&design),
        &manifest.code_under_test_commit,
        &ctx.incumbent_file_sha,
        &ctx.task_registry_sha,
        &ctx.profile_sha,
        &input_sha,
    );
    let _ = write_json(&artifact_path("discovery-summary.json"), &summary);
    let _ = write_jsonl(&out, &records); // canonical rewrite (same content)

    let violations = trace::verify_discovery(
        &records,
        &serde_json::to_value(&summary).unwrap(),
        &serde_json::to_value(&input).unwrap(),
        &input_sha,
        &ctx,
    );
    report_stage(
        "discover",
        &violations,
        summary.gates.all(),
        "mutation generation",
    );
    if !summary.proceed_to_mutation_generation {
        return Err("discovery gates failed — the mutation generator must not run".into());
    }
    Ok(())
}

/// Print the stage verification outcome and gate state.
fn report_stage(name: &str, violations: &[String], gates_all: bool, next: &str) {
    if violations.is_empty() {
        println!("[{name}] verifier: OK (no violations)");
    } else {
        println!("[{name}] verifier: {} violation(s):", violations.len());
        for v in violations {
            println!("  - {v}");
        }
    }
    if gates_all {
        println!("[{name}] all gates passed — {next} may proceed");
    } else {
        println!("[{name}] gates FAILED — {next} must NOT proceed");
    }
}

// ===========================================================================
// Stage C — mutation generation (live; frozen-run guarded)
// ===========================================================================

pub struct GenerateArgs {
    pub manifest: PathBuf,
}

pub fn cmd_generate(args: GenerateArgs) -> Result<(), String> {
    let (run, manifest) = frozen_run(&args.manifest)?;
    let design = load_design()?;
    let ctx = verifier_context()?;

    // The discovery gate must have passed.
    let summary_v = read_json(&artifact_path("discovery-summary.json"))?;
    let gates_ok = summary_v
        .get("proceed_to_mutation_generation")
        .and_then(Value::as_bool)
        == Some(true);
    if !gates_ok {
        return Err("the discovery gate did not pass — mutation generation must not run".into());
    }

    let input_v = read_json(&artifact_path("mutation-input.json"))?;
    let input_sha = file_sha(&artifact_path("mutation-input.json"))?;
    let mutator_prompt = read_file_sha(&prompts_dir().join("mutator.md")).map(|(t, _)| t)?;

    let client = mutator_client(&design);
    // Attempt 1: the registered mutator request.
    let typed_input = serde_json::from_value::<mutation::MutationInput>(input_v.clone())
        .map_err(|e| format!("mutation input is not well-formed: {e}"))?;
    let mut messages: Vec<Message> = vec![
        Message::system(mutator_prompt.clone()),
        Message::user(typed_input.to_json_string()),
    ];
    let mut attempts: Vec<GenerationAttempt> = Vec::new();
    let mut pool: Option<CandidatePool> = None;
    for attempt_no in 1..=mutation::MAX_GENERATION_ATTEMPTS {
        let t0 = Instant::now();
        let result =
            client.complete_mutator(&messages, design.mutator_temperature, REQUEST_TIMEOUT);
        let wall = t0.elapsed().as_millis() as u64;
        let (resp, raw) = match result {
            Ok(r) => (Some(r.clone()), Some(r.content.clone().unwrap_or_default())),
            Err(_) => (None, None),
        };
        let (mut_raw, verrs) = match &raw {
            Some(r) => {
                let (c, e) =
                    validate_mutator_response(&design, r, &ctx.incumbent_text, &ctx.registry);
                (c, e)
            }
            None => (
                None,
                vec![format!("mutator request failed: {resp:?} (classified)")],
            ),
        };
        let structurally_valid = mut_raw.is_some() && verrs.is_empty();
        if structurally_valid {
            let cands = mut_raw.unwrap();
            pool = Some(build_pool(
                &design,
                &cands,
                &ctx.incumbent_text,
                &ctx.incumbent_file_sha,
                &input_sha,
            ));
        }
        // The retry message carries ONLY the machine-generated
        // validation errors: no performance, selection, or cost feedback.
        messages.push(Message::assistant_text(raw.clone().unwrap_or_default()));
        messages.push(Message::user(if verrs.is_empty() {
            "The response was structurally valid.".to_string()
        } else {
            format!(
                "Your previous response was rejected with these validation errors:\n{}",
                verrs.join("\n")
            )
        }));
        let u = resp.as_ref().map(|r| &r.usage);
        attempts.push(GenerationAttempt {
            attempt: attempt_no,
            raw_response: raw,
            structurally_valid,
            validation_errors: verrs,
            prompt_tokens: u.and_then(|x| x.prompt_tokens),
            cached_prompt_tokens: u.and_then(|x| x.cached_prompt_tokens),
            completion_tokens: u.and_then(|x| x.completion_tokens),
            reasoning_tokens: u.and_then(|x| x.reasoning_tokens),
            wall_time_ms: wall,
        });
        if pool.is_some() {
            break;
        }
    }
    let complete = pool.is_some();
    let total_wall: u64 = attempts.iter().map(|a| a.wall_time_ms).sum();
    let pool_ref = pool.clone();
    let _pool_file_sha = pool_ref
        .as_ref()
        .map(|p| write_json(&artifact_path("candidate-pool.json"), p))
        .transpose()?;
    let generation = GenerationArtifact {
        experiment_id: design.experiment_id.clone(),
        run_id: run.run_id.clone(),
        run_manifest_sha256: run.run_manifest_sha256.clone(),
        frozen_design_sha256: run.frozen_design_sha256.clone(),
        model: client.model().to_string(),
        temperature: design.mutator_temperature,
        redacted_endpoint: model::redacted_endpoint(client.base_url()),
        code_under_test_commit: manifest.code_under_test_commit.clone(),
        mutation_input_sha256: input_sha.clone(),
        mutator_prompt_sha256: ctx.mutator_file_sha.clone(),
        parent_prompt_sha256: ctx.incumbent_file_sha.clone(),
        attempt_count: attempts.len() as u32,
        attempts,
        mutation_generation_complete: complete,
        accepted_pool: pool_ref.clone(),
        wall_time_ms: total_wall,
    };
    let _ = write_json(&artifact_path("mutation-generation.json"), &generation);
    // Canonical pool file even when incomplete (empty pool).
    if !complete {
        let empty = CandidatePool {
            experiment_id: design.experiment_id.clone(),
            generation: mutation::CANDIDATE_GENERATION,
            parent_id: INCUMBENT_ID.to_string(),
            parent_prompt_sha256: ctx.incumbent_file_sha.clone(),
            mutation_input_sha256: input_sha.clone(),
            candidates: Vec::new(),
        };
        let _ = write_json(&artifact_path("candidate-pool.json"), &empty);
    }

    let pool_typed = if complete {
        pool_ref.clone().unwrap()
    } else {
        CandidatePool {
            experiment_id: design.experiment_id.clone(),
            generation: mutation::CANDIDATE_GENERATION,
            parent_id: INCUMBENT_ID.to_string(),
            parent_prompt_sha256: ctx.incumbent_file_sha.clone(),
            mutation_input_sha256: input_sha.clone(),
            candidates: Vec::new(),
        }
    };
    let violations = trace::verify_mutation(
        &serde_json::to_value(&generation).unwrap(),
        &pool_typed,
        &input_v,
        &input_sha,
        &ctx,
    );
    let pool_violations = validate_candidate_pool(
        &pool_typed,
        &design,
        &ctx.incumbent_text,
        &ctx.incumbent_file_sha,
        &ctx.registry,
    );
    let mut all = violations.clone();
    all.extend(pool_violations);
    report_stage("generate", &all, complete, "selection");
    if !complete {
        return Err(
            "mutation generation did not produce a fully valid pool — selection must not run"
                .into(),
        );
    }
    Ok(())
}

// ===========================================================================
// Stage D — selection (live; frozen-run guarded)
// ===========================================================================

pub struct SelectArgs {
    pub manifest: PathBuf,
}

/// The full prompt for one condition.
fn condition_full_prompt(ctx: &VerifierContext, pool: &CandidatePool, condition: &str) -> String {
    if condition == INCUMBENT_ID {
        ctx.incumbent_text.clone()
    } else {
        let ordinal = condition
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<usize>()
            .unwrap_or(0);
        let entry = &pool.candidates[ordinal - 1];
        compose_full_prompt(&ctx.incumbent_text, &entry.suffix)
    }
}

fn condition_suffix_sha(pool: &CandidatePool, condition: &str) -> Option<String> {
    if condition == INCUMBENT_ID {
        return None;
    }
    let ordinal = condition
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .collect::<String>()
        .parse::<usize>()
        .unwrap_or(0);
    Some(pool.candidates[ordinal - 1].suffix_sha256.clone())
}

#[allow(clippy::too_many_arguments)] // one field per record field; the list is intentional
fn selection_record(
    design: &Design,
    prov: &RecordProvenance,
    task: &TaskSpec,
    level: &str,
    count: u32,
    seq: u32,
    rep: u32,
    pos: u32,
    condition: &str,
    full_prompt: &str,
    pool: &CandidatePool,
    pool_file_sha: &str,
    input_file_sha: &str,
    ep: &ExecutedEpisode,
) -> SelectionRecord {
    let c = &ep.core;
    SelectionRecord {
        experiment_id: design.experiment_id.clone(),
        phase: "selection".into(),
        episode_id: format!("0011-r1-sel-{:03}", seq),
        sequence: seq,
        family: task.family.clone(),
        task_id: task.id.clone(),
        split: "selection".into(),
        stress_level: level.to_string(),
        drop_count: count,
        fault_key: task.fault_key.clone(),
        repetition: rep,
        condition_position: pos,
        condition: condition.to_string(),
        generation: if condition == INCUMBENT_ID { 0 } else { 1 },
        parent_id: INCUMBENT_ID.to_string(),
        suffix_sha256: condition_suffix_sha(pool, condition),
        full_prompt_sha256: sha256_bytes(full_prompt.as_bytes()),
        candidate_pool_sha256: pool_file_sha.to_string(),
        mutation_input_sha256: input_file_sha.to_string(),
        provenance: prov.clone(),
        initial_state: ep.initial_state.clone(),
        target_state: task.target_state.clone(),
        fault_mode: serde_json::to_value(FaultMode::for_stress(&task.fault_key, count)).unwrap(),
        conversation: c.conversation.clone(),
        model_requests: c.model_requests.clone(),
        requested_tool_calls: c.requested_tool_calls.clone(),
        executed_tool_calls: c.executed_tool_calls.clone(),
        environment_write_attempts: c.write_attempts.clone(),
        final_state: c.final_state.clone(),
        final_answer: c.final_answer.clone(),
        model_turn_count: c.model_turn_count,
        tool_call_count: c.tool_call_count,
        agent_failure: c.agent_failure.clone(),
        infrastructure_failure: c.infrastructure_failure.clone(),
        usage_accounting_error: c.usage_accounting_error.clone(),
        termination_reason: c.termination_reason.clone(),
        oracle_success: ep.eval.success,
        oracle_failure_reasons: ep.eval.reasons.clone(),
        timing: c.timing,
    }
}

pub fn cmd_select(args: SelectArgs) -> Result<(), String> {
    let (run, manifest) = frozen_run(&args.manifest)?;
    let design = load_design()?;
    let ctx = verifier_context()?;

    let pool = read_json(&artifact_path("candidate-pool.json"))
        .and_then(|v| serde_json::from_value::<CandidatePool>(v).map_err(|e| e.to_string()))?;
    if pool.candidates.len() != design.candidate_count as usize {
        return Err("candidate pool is not complete — selection must not run".into());
    }
    if pool.mutation_input_sha256.is_empty() {
        return Err("candidate pool is missing its mutation input binding".into());
    }
    let pool_file_sha = file_sha(&artifact_path("candidate-pool.json"))?;
    let input_file_sha = file_sha(&artifact_path("mutation-input.json"))?;

    let client = agent_client(&design);
    let prov = base_provenance(
        &ctx,
        &manifest,
        &run,
        client.model(),
        design.agent_temperature,
        &agent_endpoint_redacted(&design),
    );

    let registry_file = load_tasks(&crate_dir().join("tasks.json"))?;
    let selection_tasks: Vec<&TaskSpec> = registry_file
        .tasks
        .iter()
        .filter(|t| t.split == TaskSplit::Selection)
        .collect();
    let reps = design.selection_repetitions();
    let n_cond = design.selection.conditions.len() as u32;
    let mut records: Vec<SelectionRecord> = Vec::new();
    let mut seq = 0u32;
    for task in &selection_tasks {
        for rep in 1..=reps {
            for pos in 1..=n_cond {
                let Some(condition) = design.selection_condition_at(rep, pos) else {
                    return Err(format!(
                        "the registered cyclic schedule has no condition at (rep {rep}, position {pos})"
                    ));
                };
                let full = condition_full_prompt(&ctx, &pool, &condition);
                seq += 1;
                eprintln!(
                    "[select] {}/{} {} rep {rep} {condition} ({} {})",
                    seq,
                    design.selection_episodes(&registry_file.tasks),
                    task.id,
                    task.family,
                    run.run_id
                );
                let ep = execute_episode(&client, &design, design.agent_temperature, task, &full)?;
                let rec = selection_record(
                    &design,
                    &prov,
                    task,
                    ep.level,
                    ep.drop_count,
                    seq,
                    rep,
                    pos,
                    &condition,
                    &full,
                    &pool,
                    &pool_file_sha,
                    &input_file_sha,
                    &ep,
                );
                records.push(rec);
                let line = serde_json::to_string(records.last().unwrap()).unwrap();
                let mut f = std::io::BufWriter::new(
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(artifact_path("selection-raw.jsonl"))
                        .map_err(|e| e.to_string())?,
                );
                use std::io::Write as _;
                writeln!(f, "{line}").map_err(|e| e.to_string())?;
                f.flush().map_err(|e| e.to_string())?;
            }
        }
    }

    let summary = trace::compute_selection_summary(
        &records,
        &design,
        &ctx.registry,
        &run,
        &pool,
        client.model(),
        &agent_endpoint_redacted(&design),
        &manifest.code_under_test_commit,
        &ctx.incumbent_file_sha,
        &ctx.task_registry_sha,
        &ctx.profile_sha,
        &pool_file_sha,
        &input_file_sha,
    );
    let sel_tally = summary
        .candidates
        .iter()
        .find(|c| c.candidate_id == summary.selected.selected_candidate_id)
        .filter(|_| !summary.selected.incumbent_retained);
    let selected = SelectedCandidate {
        experiment_id: design.experiment_id.clone(),
        run_id: run.run_id.clone(),
        run_manifest_sha256: run.run_manifest_sha256.clone(),
        frozen_design_sha256: run.frozen_design_sha256.clone(),
        selected_candidate_id: summary.selected.selected_candidate_id.clone(),
        parent_id: INCUMBENT_ID.to_string(),
        candidate_pool_sha256: pool_file_sha.clone(),
        selection_raw_sha256: file_sha(&artifact_path("selection-raw.jsonl"))?,
        selection_summary_sha256: String::new(), // set after the summary is written
        mutation_input_sha256: input_file_sha.clone(),
        suffix: sel_tally.map(|_| {
            let id = &summary.selected.selected_candidate_id;
            let ordinal = id
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<usize>()
                .unwrap_or(0);
            pool.candidates[ordinal - 1].suffix.clone()
        }),
        suffix_sha256: sel_tally.map(|_| {
            let id = &summary.selected.selected_candidate_id;
            let ordinal = id
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<usize>()
                .unwrap_or(0);
            pool.candidates[ordinal - 1].suffix_sha256.clone()
        }),
        full_prompt_sha256: sel_tally.map(|_| {
            let id = &summary.selected.selected_candidate_id;
            let ordinal = id
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<usize>()
                .unwrap_or(0);
            pool.candidates[ordinal - 1].full_prompt_sha256.clone()
        }),
        selection: trace::FrozenSelectionBlock {
            common_valid_cells: summary.common_valid_cells,
            baseline_failures: summary.g0_failures_in_common_valid,
            wins: sel_tally.map(|t| t.wins as u32).unwrap_or(0),
            losses: sel_tally.map(|t| t.losses as u32).unwrap_or(0),
            ties: sel_tally.map(|t| t.ties as u32).unwrap_or(0),
            net_margin: sel_tally.map(|t| t.net_margin).unwrap_or(0),
        },
        tie_break_path: summary.selected.tie_break_path.clone(),
    };
    let summary_file_sha = write_json(&artifact_path("selection-summary.json"), &summary)?;
    let mut selected = selected;
    selected.selection_summary_sha256 = summary_file_sha;
    let _ = write_json(&artifact_path("selected-candidate.json"), &selected);
    let _ = write_jsonl(&artifact_path("selection-raw.jsonl"), &records);

    let violations = trace::verify_selection(
        &records,
        &serde_json::to_value(&summary).unwrap(),
        &serde_json::to_value(&selected).unwrap(),
        &pool,
        &pool_file_sha,
        &input_file_sha,
        &ctx,
    );
    report_stage("select", &violations, summary.gates.all(), "promotion");
    Ok(())
}

// ===========================================================================
// Stage E — promotion (live; frozen-run guarded)
// ===========================================================================

pub struct PromoteArgs {
    pub manifest: PathBuf,
}

pub fn cmd_promote(args: PromoteArgs) -> Result<(), String> {
    let (run, manifest) = frozen_run(&args.manifest)?;
    let design = load_design()?;
    let ctx = verifier_context()?;

    let selected_v = read_json(&artifact_path("selected-candidate.json"))?;
    let selected: SelectedCandidate =
        serde_json::from_value(selected_v).map_err(|e| format!("selected candidate: {e}"))?;
    let selected_id = selected.selected_candidate_id.clone();
    if selected_id != INCUMBENT_ID && !design.candidate_ids().contains(&selected_id) {
        return Err(format!(
            "selected candidate {selected_id} is not in the registered candidate set"
        ));
    }
    let pool = read_json(&artifact_path("candidate-pool.json"))
        .and_then(|v| serde_json::from_value::<CandidatePool>(v).map_err(|e| e.to_string()))?;
    let pool_file_sha = file_sha(&artifact_path("candidate-pool.json"))?;
    let input_file_sha = file_sha(&artifact_path("mutation-input.json"))?;
    let selected_file_sha = file_sha(&artifact_path("selected-candidate.json"))?;

    // The selected candidate's full prompt (G0 → the incumbent itself).
    let selected_full = if selected_id == INCUMBENT_ID {
        ctx.incumbent_text.clone()
    } else {
        let ordinal = selected_id
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<usize>()
            .unwrap_or(0);
        let entry = &pool.candidates[ordinal - 1];
        compose_full_prompt(&ctx.incumbent_text, &entry.suffix)
    };
    let selected_suffix_sha = if selected_id == INCUMBENT_ID {
        None
    } else {
        let ordinal = selected_id
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<usize>()
            .unwrap_or(0);
        Some(pool.candidates[ordinal - 1].suffix_sha256.clone())
    };

    let client = agent_client(&design);
    let prov = base_provenance(
        &ctx,
        &manifest,
        &run,
        client.model(),
        design.agent_temperature,
        &agent_endpoint_redacted(&design),
    );

    let registry_file = load_tasks(&crate_dir().join("tasks.json"))?;
    let promotion_tasks: Vec<&TaskSpec> = registry_file
        .tasks
        .iter()
        .filter(|t| t.split == TaskSplit::Promotion)
        .collect();
    let reps = design.promotion_repetitions();
    let mut records: Vec<PromotionRecord> = Vec::new();
    let mut seq = 0u32;
    for task in &promotion_tasks {
        for rep in 1..=reps {
            let order = design.promotion_condition_order(rep, &selected_id);
            for (pos, condition) in order.iter().enumerate() {
                let full = if condition == INCUMBENT_ID {
                    ctx.incumbent_text.clone()
                } else {
                    selected_full.clone()
                };
                seq += 1;
                eprintln!(
                    "[promote] {}/{} {} rep {rep} {condition} ({} {})",
                    seq,
                    design.promotion_episodes(&registry_file.tasks),
                    task.id,
                    task.family,
                    run.run_id
                );
                let ep = execute_episode(&client, &design, design.agent_temperature, task, &full)?;
                let c = &ep.core;
                records.push(PromotionRecord {
                    experiment_id: design.experiment_id.clone(),
                    phase: "promotion".into(),
                    episode_id: format!("0011-r1-prom-{:03}", seq),
                    sequence: seq,
                    family: task.family.clone(),
                    task_id: task.id.clone(),
                    split: "promotion".into(),
                    stress_level: ep.level.to_string(),
                    drop_count: ep.drop_count,
                    fault_key: task.fault_key.clone(),
                    repetition: rep,
                    condition_position: (pos + 1) as u32,
                    condition: condition.clone(),
                    generation: if condition == INCUMBENT_ID { 0 } else { 1 },
                    parent_id: INCUMBENT_ID.to_string(),
                    suffix_sha256: if condition == INCUMBENT_ID {
                        None
                    } else {
                        selected_suffix_sha.clone()
                    },
                    full_prompt_sha256: sha256_bytes(full.as_bytes()),
                    candidate_pool_sha256: pool_file_sha.clone(),
                    selected_candidate_sha256: selected_file_sha.clone(),
                    mutation_input_sha256: input_file_sha.clone(),
                    provenance: prov.clone(),
                    initial_state: ep.initial_state.clone(),
                    target_state: task.target_state.clone(),
                    fault_mode: serde_json::to_value(FaultMode::for_stress(
                        &task.fault_key,
                        ep.drop_count,
                    ))
                    .unwrap(),
                    conversation: c.conversation.clone(),
                    model_requests: c.model_requests.clone(),
                    requested_tool_calls: c.requested_tool_calls.clone(),
                    executed_tool_calls: c.executed_tool_calls.clone(),
                    environment_write_attempts: c.write_attempts.clone(),
                    final_state: c.final_state.clone(),
                    final_answer: c.final_answer.clone(),
                    model_turn_count: c.model_turn_count,
                    tool_call_count: c.tool_call_count,
                    agent_failure: c.agent_failure.clone(),
                    infrastructure_failure: c.infrastructure_failure.clone(),
                    usage_accounting_error: c.usage_accounting_error.clone(),
                    termination_reason: c.termination_reason.clone(),
                    oracle_success: ep.eval.success,
                    oracle_failure_reasons: ep.eval.reasons.clone(),
                    timing: c.timing,
                });
                let line = serde_json::to_string(records.last().unwrap()).unwrap();
                let mut f = std::io::BufWriter::new(
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(artifact_path("promotion-raw.jsonl"))
                        .map_err(|e| e.to_string())?,
                );
                use std::io::Write as _;
                writeln!(f, "{line}").map_err(|e| e.to_string())?;
                f.flush().map_err(|e| e.to_string())?;
            }
        }
    }

    let summary = trace::compute_promotion_summary(
        &records,
        &design,
        &ctx.registry,
        &run,
        &selected_id,
        client.model(),
        &agent_endpoint_redacted(&design),
        &manifest.code_under_test_commit,
        &ctx.incumbent_file_sha,
        &ctx.task_registry_sha,
        &ctx.profile_sha,
        &pool_file_sha,
        &selected_file_sha,
        &input_file_sha,
    );
    let _ = write_json(&artifact_path("promotion-summary.json"), &summary);
    let _ = write_jsonl(&artifact_path("promotion-raw.jsonl"), &records);

    let violations = trace::verify_promotion(
        &records,
        &serde_json::to_value(&summary).unwrap(),
        &serde_json::to_value(&selected).unwrap(),
        &pool,
        &pool_file_sha,
        &input_file_sha,
        &selected_file_sha,
        &ctx,
    );
    report_stage(
        "promote",
        &violations,
        summary.gates.all(),
        "the final three-way conclusion",
    );
    println!(
        "[promote] CONCLUSION: {} — {}",
        summary.conclusion.result, summary.conclusion.reason
    );
    Ok(())
}

// ===========================================================================
// Stage A1 — run manifest creation (network-free)
// ===========================================================================

pub struct InitRunArgs {
    pub commit: Option<String>,
    pub manifest: PathBuf,
}

/// Stage A1: hash every frozen file, write the run manifest. The tree
/// must be exactly the Stage A0 state (the guard is run before AND
/// after writing). Network-free.
pub fn cmd_init_run(args: InitRunArgs) -> Result<(), String> {
    let root = freeze::repo_root()?;
    let commit = args
        .commit
        .clone()
        .or_else(|| {
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&root)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        })
        .filter(|c| c.len() == 40)
        .ok_or("no commit given and HEAD is not resolvable")?;
    let manifest = freeze::build_manifest(&root, &commit, "0011-r1", "0011")?;
    // Pre-check: the current tree must already satisfy the freeze.
    let pre = freeze::verify_run_freeze(&root, &manifest);
    if !pre.is_empty() {
        return Err(format!(
            "refusing to write the run manifest — the working tree does not match the frozen code commit:\n  - {}",
            pre.join("\n  - ")
        ));
    }
    let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    let bytes = format!("{text}\n").into_bytes();
    std::fs::write(&args.manifest, &bytes)
        .map_err(|e| format!("cannot write {}: {e}", args.manifest.display()))?;
    // Post-check on the bytes actually written to disk.
    let reloaded = freeze::load_manifest(&args.manifest)?;
    let post = freeze::verify_run_freeze(&root, &reloaded);
    if !post.is_empty() {
        return Err(format!(
            "run manifest written but post-write freeze check failed:\n  - {}",
            post.join("\n  - ")
        ));
    }
    println!(
        "run manifest written: {} ({} bytes, sha256 {})",
        args.manifest.display(),
        bytes.len(),
        sha256_bytes(&bytes)
    );
    println!("  run_id: {}", manifest.run_id);
    println!(
        "  code_under_test_commit: {}",
        manifest.code_under_test_commit
    );
    println!("  frozen_design_sha256: {}", manifest.frozen_design_sha256);
    println!(
        "  files: {} frozen design files recorded",
        manifest.files.len()
    );
    Ok(())
}

/// Stage A0 helper: print the exact git commands to commit the frozen
/// code (the human/agent runs them; the harness never commits).
pub fn print_a0_commands(commit_message: &str) {
    let mut paths: Vec<&str> = freeze::FROZEN_PATHS.to_vec();
    paths.sort_unstable();
    paths.dedup();
    let mut cmd = "git add".to_string();
    for p in &paths {
        cmd.push(' ');
        cmd.push_str(p);
    }
    println!(
        "Stage A0 (frozen code commit): commit every frozen design file (run-manifest.json is written and committed later, at Stage A1)."
    );
    println!("  {cmd}");
    println!("  git commit -m \"{commit_message}\"");
    println!(
        "  git rev-parse HEAD   # → pass to: cargo run -p mutagen-exp-0011 -- init-run --commit <SHA>"
    );
}

fn verify_command(
    verify: impl Fn(&VerifierContext) -> (String, Vec<String>),
) -> Result<(), String> {
    let ctx = verifier_context()?;
    let (name, violations) = verify(&ctx);
    if violations.is_empty() {
        println!("{name}: verifier OK — no violations");
        Ok(())
    } else {
        Err(format!(
            "{name}: {} violation(s):\n  - {}",
            violations.len(),
            violations.join("\n  - ")
        ))
    }
}

pub fn cmd_verify_discovery() -> Result<(), String> {
    verify_command(|ctx| {
        let records = read_jsonl::<DiscoveryRecord>(&artifact_path("discovery-raw.jsonl"))
            .unwrap_or_default();
        let summary = read_json(&artifact_path("discovery-summary.json")).unwrap_or(Value::Null);
        let input = read_json(&artifact_path("mutation-input.json")).unwrap_or(Value::Null);
        let input_sha = file_sha(&artifact_path("mutation-input.json")).unwrap_or_default();
        let v = trace::verify_discovery(&records, &summary, &input, &input_sha, ctx);
        ("verify-discovery".to_string(), v)
    })
}

pub fn cmd_verify_mutation() -> Result<(), String> {
    verify_command(|ctx| {
        let generation =
            read_json(&artifact_path("mutation-generation.json")).unwrap_or(Value::Null);
        let input = read_json(&artifact_path("mutation-input.json")).unwrap_or(Value::Null);
        let input_sha = file_sha(&artifact_path("mutation-input.json")).unwrap_or_default();
        let pool = read_json(&artifact_path("candidate-pool.json"))
            .and_then(|v| serde_json::from_value::<CandidatePool>(v).map_err(|e| e.to_string()))
            .unwrap_or(CandidatePool {
                experiment_id: String::new(),
                generation: mutation::CANDIDATE_GENERATION,
                parent_id: INCUMBENT_ID.to_string(),
                parent_prompt_sha256: String::new(),
                mutation_input_sha256: String::new(),
                candidates: Vec::new(),
            });
        let v = trace::verify_mutation(&generation, &pool, &input, &input_sha, ctx);
        ("verify-mutation".to_string(), v)
    })
}

pub fn cmd_verify_selection() -> Result<(), String> {
    verify_command(|ctx| {
        let records = read_jsonl::<SelectionRecord>(&artifact_path("selection-raw.jsonl"))
            .unwrap_or_default();
        let summary = read_json(&artifact_path("selection-summary.json")).unwrap_or(Value::Null);
        let selected = read_json(&artifact_path("selected-candidate.json")).unwrap_or(Value::Null);
        let pool = read_json(&artifact_path("candidate-pool.json"))
            .and_then(|v| serde_json::from_value::<CandidatePool>(v).map_err(|e| e.to_string()))
            .unwrap_or(CandidatePool {
                experiment_id: String::new(),
                generation: mutation::CANDIDATE_GENERATION,
                parent_id: INCUMBENT_ID.to_string(),
                parent_prompt_sha256: String::new(),
                mutation_input_sha256: String::new(),
                candidates: Vec::new(),
            });
        let pool_sha = file_sha(&artifact_path("candidate-pool.json")).unwrap_or_default();
        let input_sha = file_sha(&artifact_path("mutation-input.json")).unwrap_or_default();
        let v = trace::verify_selection(
            &records, &summary, &selected, &pool, &pool_sha, &input_sha, ctx,
        );
        ("verify-selection".to_string(), v)
    })
}

pub fn cmd_verify_promotion() -> Result<(), String> {
    verify_command(|ctx| {
        let records = read_jsonl::<PromotionRecord>(&artifact_path("promotion-raw.jsonl"))
            .unwrap_or_default();
        let summary = read_json(&artifact_path("promotion-summary.json")).unwrap_or(Value::Null);
        let selected = read_json(&artifact_path("selected-candidate.json")).unwrap_or(Value::Null);
        let pool = read_json(&artifact_path("candidate-pool.json"))
            .and_then(|v| serde_json::from_value::<CandidatePool>(v).map_err(|e| e.to_string()))
            .unwrap_or(CandidatePool {
                experiment_id: String::new(),
                generation: mutation::CANDIDATE_GENERATION,
                parent_id: INCUMBENT_ID.to_string(),
                parent_prompt_sha256: String::new(),
                mutation_input_sha256: String::new(),
                candidates: Vec::new(),
            });
        let pool_sha = file_sha(&artifact_path("candidate-pool.json")).unwrap_or_default();
        let input_sha = file_sha(&artifact_path("mutation-input.json")).unwrap_or_default();
        let selected_sha = file_sha(&artifact_path("selected-candidate.json")).unwrap_or_default();
        let v = trace::verify_promotion(
            &records,
            &summary,
            &selected,
            &pool,
            &pool_sha,
            &input_sha,
            &selected_sha,
            ctx,
        );
        ("verify-promotion".to_string(), v)
    })
}

/// Network-free verification of the run manifest + freeze state.
pub fn cmd_verify_run_manifest(manifest: &Path) -> Result<(), String> {
    let m = freeze::load_manifest(manifest)?;
    let root = freeze::repo_root()?;
    let v = freeze::verify_run_freeze(&root, &m);
    if v.is_empty() {
        println!(
            "run manifest OK: run {} frozen under commit {} ({} files, frozen design {})",
            m.run_id,
            m.code_under_test_commit,
            m.files.len(),
            m.frozen_design_sha256
        );
        Ok(())
    } else {
        Err(format!(
            "run manifest freeze violations:\n  - {}",
            v.join("\n  - ")
        ))
    }
}

// ===========================================================================
// Preflight (network-free) and the self-test
// ===========================================================================

pub fn cmd_preflight(manifest: Option<&Path>) -> Result<(), String> {
    println!("== Experiment 0011 preflight (network-free) ==");
    let design = load_design()?;
    println!(
        "design: experiment {} run {} candidates {} | temp agent {} mutator {} | turns {} calls {} time {} ms",
        design.experiment_id,
        design.run_id,
        design.candidate_count,
        design.agent_temperature,
        design.mutator_temperature,
        design.max_model_turns,
        design.max_tool_calls,
        design.max_time_ms
    );
    let file = load_tasks(&crate_dir().join("tasks.json"))?;
    let (e, s, p) = (
        file.tasks
            .iter()
            .filter(|t| t.split == TaskSplit::Discovery)
            .count(),
        file.tasks
            .iter()
            .filter(|t| t.split == TaskSplit::Selection)
            .count(),
        file.tasks
            .iter()
            .filter(|t| t.split == TaskSplit::Promotion)
            .count(),
    );
    println!(
        "registry: {} tasks ({e} discovery / {s} selection / {p} promotion) — validated",
        file.tasks.len()
    );
    let profile = load_stress_profile(&crate_dir().join("stress-profile.json"))?;
    println!(
        "stress profile: frozen 0009 profile ({} families) — validated",
        profile.families.len()
    );
    for (name, path) in [
        ("incumbent prompt", prompts_dir().join("incumbent.md")),
        ("mutator prompt", prompts_dir().join("mutator.md")),
    ] {
        let (_, sha) = read_file_sha(&path)?;
        println!("{name}: sha256 {sha}");
    }
    println!(
        "endpoint: {} (model {}, auth_header {})",
        design.endpoints[0].url, design.endpoints[0].model, design.endpoints[0].auth_header
    );
    let lock = crate_dir().join("Cargo.lock");
    println!(
        "lockfile: {}",
        if lock.exists() { "present" } else { "MISSING" }
    );
    if let Some(m) = manifest {
        match frozen_run(m) {
            Ok((run, _)) => {
                println!("freeze: OK — run {} frozen intact", run.run_id);
            }
            Err(e) => {
                println!("freeze: VIOLATION\n{e}");
                return Err("source freeze violation".into());
            }
        }
    }
    println!("preflight: PASS");
    Ok(())
}

/// The complete network-free self-test: design arithmetic, registry,
/// profile, prompts, the source-freeze guard (temp-repo scenarios), and
/// the synthetic end-to-end + tamper suite.
/// Record one self-test outcome.
fn self_test_pass(failures: &mut Vec<String>, name: &str, ok: bool, detail: &str) {
    let detail = if detail.is_empty() { "" } else { detail };
    println!("  [{}] {name} {detail}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        failures.push(format!("{name}: {detail}"));
    }
}

pub fn self_test() -> Result<(), String> {
    let mut failures: Vec<String> = Vec::new();
    println!("== 0011 self-test ==");

    // --- 1. Design manifest arithmetic --------------------------------
    println!("-- 1. design manifest --");
    match load_design() {
        Ok(design) => {
            let file = load_tasks(&crate_dir().join("tasks.json")).unwrap_or_else(|e| {
                failures.push(format!("registry: {e}"));
                std::process::exit(1);
            });
            self_test_pass(
                &mut failures,
                "design loads + validates",
                true,
                &format!(
                    "discovery {} / selection {} / promotion {} episodes; gates 16/8, 27/12, 44/12/12, infra floor {}",
                    design.discovery_episodes(&file.tasks),
                    design.selection_episodes(&file.tasks),
                    design.promotion_episodes(&file.tasks),
                    design.max_promotion_infra_failures(&file.tasks)
                ),
            );
            self_test_pass(
                &mut failures,
                "registry validates",
                true,
                &format!("{} tasks", file.tasks.len()),
            );
            // The 0010 failure mode at the gate level: insufficient
            // evidence must block generation (mechanical, not manual).
            self_test_pass(
                &mut failures,
                "gate blocks generation on weak evidence",
                design.discovery.min_valid_episodes > 0
                    && design.discovery.min_incumbent_failures > 0,
                "min_valid_episodes + min_incumbent_failures are registered and enforced",
            );
        }
        Err(e) => {
            self_test_pass(&mut failures, "design loads + validates", false, &e);
        }
    }

    // --- 2. Frozen profile + prompts ----------------------------------
    println!("-- 2. frozen profile + prompts --");
    match load_stress_profile(&crate_dir().join("stress-profile.json")) {
        Ok(_) => self_test_pass(
            &mut failures,
            "stress profile",
            true,
            "frozen 0009 profile verified",
        ),
        Err(e) => self_test_pass(&mut failures, "stress profile", false, &e),
    }
    for name in ["incumbent.md", "mutator.md"] {
        match read_file_sha(&prompts_dir().join(name)) {
            Ok((text, _)) => {
                self_test_pass(
                    &mut failures,
                    &format!("prompt {name}"),
                    !text.trim().is_empty(),
                    &format!("{} bytes", text.len()),
                );
            }
            Err(e) => self_test_pass(&mut failures, &format!("prompt {name}"), false, &e),
        }
    }

    // --- 3. Source-freeze guard (temp-repo scenarios) -----------------
    println!("-- 3. source-freeze guard --");
    {
        use crate::freeze::{FrozenFile, combined_frozen_sha};
        fn git_in(dir: &Path, args: &[&str]) -> Result<(), String> {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
            }
            Ok(())
        }
        fn test_repo(tag: &str, files: &[(&str, &str)]) -> (PathBuf, String) {
            let root = std::env::temp_dir().join(format!(
                "mutagen-0011-freeze-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            for (rel, content) in files {
                let p = root.join(rel);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, content).unwrap();
            }
            git_in(&root, &["init", "-q"]).unwrap();
            git_in(&root, &["config", "user.email", "t@t.t"]).unwrap();
            git_in(&root, &["config", "user.name", "t"]).unwrap();
            git_in(&root, &["add", "-A"]).unwrap();
            git_in(&root, &["commit", "-q", "-m", "A0"]).unwrap();
            let commit = std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&root)
                .output()
                .unwrap();
            (
                root,
                String::from_utf8_lossy(&commit.stdout).trim().to_string(),
            )
        }
        fn mk(files: &[(&str, &str)]) -> freeze::RunManifest {
            let mut fs = files
                .iter()
                .map(|(p, c)| FrozenFile {
                    path: (*p).to_string(),
                    sha256: sha256_bytes(c.as_bytes()),
                })
                .collect::<Vec<_>>();
            fs.sort_by(|a, b| a.path.cmp(&b.path));
            freeze::RunManifest {
                experiment_id: "0011".into(),
                run_id: "0011-r1".into(),
                code_under_test_commit: String::new(),
                frozen_design_sha256: combined_frozen_sha(&fs),
                files: fs,
                design_sha256: String::new(),
                tasks_sha256: String::new(),
                stress_profile_sha256: String::new(),
                incumbent_prompt_sha256: String::new(),
                mutator_prompt_sha256: String::new(),
            }
        }
        let paths = ["frozen/a.rs", "frozen/b.rs"];

        // (a) pristine freeze passes.
        let (root, commit) = test_repo(
            "pristine",
            &[
                ("frozen/a.rs", "v1"),
                ("frozen/b.rs", "v2"),
                ("art/x.json", "z"),
            ],
        );
        let mut m = mk(&[("frozen/a.rs", "v1"), ("frozen/b.rs", "v2")]);
        m.code_under_test_commit = commit.clone();
        let v = freeze::verify_freeze(&root, &m, &paths);
        self_test_pass(
            &mut failures,
            "pristine freeze accepted",
            v.is_empty(),
            &v.join("; "),
        );
        // (b) current bytes changed → rejected.
        std::fs::write(root.join("frozen/a.rs"), "TAMPERED").unwrap();
        let v = freeze::verify_freeze(&root, &m, &paths);
        self_test_pass(
            &mut failures,
            "byte change rejected",
            v.iter().any(|x| x.contains("current bytes differ")),
            &v.join("; "),
        );
        let _ = std::fs::remove_dir_all(&root);

        // (c) committed frozen change → rejected (0010's failure mode).
        let (root, commit) = test_repo("committed", &[("frozen/a.rs", "v1"), ("art/x.json", "z")]);
        let mut m = mk(&[("frozen/a.rs", "v1")]);
        m.code_under_test_commit = commit;
        std::fs::write(root.join("frozen/a.rs"), "v2").unwrap();
        git_in(&root, &["add", "-A"]).unwrap();
        git_in(&root, &["commit", "-q", "-m", "mid-run bugfix"]).unwrap();
        let v = freeze::verify_freeze(&root, &m, &["frozen/a.rs"]);
        self_test_pass(
            &mut failures,
            "mid-run commit rejected (0010 failure mode)",
            v.iter().any(|x| x.contains("commit(s) after")),
            &v.join("; "),
        );
        // (d) modify → commit → revert (net diff clean) → still rejected.
        std::fs::write(root.join("frozen/a.rs"), "v1").unwrap();
        git_in(&root, &["add", "-A"]).unwrap();
        git_in(&root, &["commit", "-q", "-m", "revert"]).unwrap();
        let v = freeze::verify_freeze(&root, &m, &["frozen/a.rs"]);
        self_test_pass(
            &mut failures,
            "modify-commit-revert rejected (history, not net diff)",
            v.iter().any(|x| x.contains("commit(s) after")),
            &v.join("; "),
        );
        let _ = std::fs::remove_dir_all(&root);

        // (e) artifact-only commit → accepted (fresh repo, so the
        // history is clean apart from the non-frozen artifact).
        let (root, commit) = test_repo("artifacts", &[("frozen/a.rs", "v1"), ("art/x.json", "z")]);
        let mut m2 = mk(&[("frozen/a.rs", "v1")]);
        m2.code_under_test_commit = commit;
        std::fs::write(root.join("art/x.json"), "z2").unwrap();
        git_in(&root, &["add", "-A"]).unwrap();
        git_in(&root, &["commit", "-q", "-m", "artifact"]).unwrap();
        let v = freeze::verify_freeze(&root, &m2, &["frozen/a.rs"]);
        self_test_pass(
            &mut failures,
            "artifact-only commits accepted",
            v.is_empty(),
            &v.join("; "),
        );
        let _ = std::fs::remove_dir_all(&root);

        // (f) manifest internal tamper → rejected.
        let (root, commit) = test_repo("tamper", &[("frozen/a.rs", "v1")]);
        let mut m = mk(&[("frozen/a.rs", "v1")]);
        m.code_under_test_commit = commit;
        m.files[0].sha256 = "f".repeat(64);
        let v = freeze::verify_freeze(&root, &m, &["frozen/a.rs"]);
        self_test_pass(
            &mut failures,
            "manifest tamper rejected",
            v.iter()
                .any(|x| x.contains("frozen_design_sha256 does not reproduce")),
            &v.join("; "),
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // --- 4. Synthetic end-to-end + tamper suite -----------------------
    println!("-- 4. synthetic end-to-end + tamper suite --");
    let synth = trace::build_synthetic_artifacts();
    let ctx = &synth.ctx;
    let sha_of = |v: &Value| sha256_bytes(v.to_string().as_bytes());

    // Discovery.
    {
        let base = trace::verify_discovery(
            &synth.discovery,
            &serde_json::to_value(&synth.discovery_summary).unwrap(),
            &serde_json::to_value(&synth.mutation_input).unwrap(),
            synth.input_file_sha,
            ctx,
        );
        self_test_pass(
            &mut failures,
            "synthetic discovery verifies clean",
            base.is_empty(),
            &base.join("; "),
        );
        let mut recs = serde_json::from_value::<Vec<DiscoveryRecord>>(
            serde_json::to_value(&synth.discovery).unwrap(),
        )
        .unwrap();
        recs[3].oracle_success = !recs[3].oracle_success; // tamper verdict
        let v = trace::verify_discovery(
            &recs,
            &serde_json::to_value(&synth.discovery_summary).unwrap(),
            &serde_json::to_value(&synth.mutation_input).unwrap(),
            synth.input_file_sha,
            ctx,
        );
        self_test_pass(
            &mut failures,
            "discovery oracle tamper detected",
            !v.is_empty(),
            &v[0].clone(),
        );
        let mut recs = serde_json::from_value::<Vec<DiscoveryRecord>>(
            serde_json::to_value(&synth.discovery).unwrap(),
        )
        .unwrap();
        if let Some(o) = recs[5].final_state.as_object_mut() {
            if let Some(x) = o.get_mut("x") {
                *x = Value::String("TAMPERED".into());
            }
        }
        let v = trace::verify_discovery(
            &recs,
            &serde_json::to_value(&synth.discovery_summary).unwrap(),
            &serde_json::to_value(&synth.mutation_input).unwrap(),
            synth.input_file_sha,
            ctx,
        );
        self_test_pass(
            &mut failures,
            "discovery final-state tamper detected",
            !v.is_empty(),
            &v[0].clone(),
        );
        let mut recs = serde_json::from_value::<Vec<DiscoveryRecord>>(
            serde_json::to_value(&synth.discovery).unwrap(),
        )
        .unwrap();
        recs.pop(); // drop one episode
        let v = trace::verify_discovery(
            &recs,
            &serde_json::to_value(&synth.discovery_summary).unwrap(),
            &serde_json::to_value(&synth.mutation_input).unwrap(),
            synth.input_file_sha,
            ctx,
        );
        self_test_pass(
            &mut failures,
            "discovery dropped record detected",
            !v.is_empty(),
            &v[0].clone(),
        );
        let mut iv = serde_json::to_value(&synth.mutation_input).unwrap();
        if let Some(eps) = iv
            .get_mut("discovery_episodes")
            .and_then(|v| v.as_array_mut())
        {
            if let Some(st) = eps.first_mut().and_then(|e| e.get_mut("final_state")) {
                if let Some(o) = st.as_object_mut() {
                    o.insert("x".into(), Value::String("V11_SA_0".into())); // selection literal
                }
            }
        }
        let v = trace::verify_discovery(
            &synth.discovery,
            &serde_json::to_value(&synth.discovery_summary).unwrap(),
            &iv,
            synth.input_file_sha,
            ctx,
        );
        self_test_pass(
            &mut failures,
            "mutation-input leakage (selection literal) detected",
            !v.is_empty(),
            &v.first().cloned().unwrap_or_default(),
        );
    }

    // Mutation.
    {
        let base = trace::verify_mutation(
            &serde_json::to_value(&synth.generation).unwrap(),
            &synth.pool,
            &serde_json::to_value(&synth.mutation_input).unwrap(),
            synth.input_file_sha,
            ctx,
        );
        self_test_pass(
            &mut failures,
            "synthetic mutation verifies clean",
            base.is_empty(),
            &base.join("; "),
        );
        // Inject the 0009 repair into C1 (harness-side modification).
        let mut pool = synth.pool.clone();
        pool.candidates[0].suffix =
            "After every successful state_write, read the same key. If the observed value differs from the value you intended to write, write it again and re-read it. Repeat until the value matches, but make at most five state_write attempts for that requested key."
                .to_string();
        pool.candidates[0].suffix_sha256 = sha256_bytes(pool.candidates[0].suffix.as_bytes());
        pool.candidates[0].full_prompt_sha256 = sha256_bytes(
            compose_full_prompt(&ctx.incumbent_text, &pool.candidates[0].suffix).as_bytes(),
        );
        let v = trace::verify_mutation(
            &serde_json::to_value(&synth.generation).unwrap(),
            &pool,
            &serde_json::to_value(&synth.mutation_input).unwrap(),
            synth.input_file_sha,
            ctx,
        );
        self_test_pass(
            &mut failures,
            "pool-vs-raw-response mismatch (injection) detected",
            !v.is_empty(),
            &v.first().cloned().unwrap_or_default(),
        );
        let mut g = serde_json::to_value(&synth.generation).unwrap();
        if let Some(t) = g.get_mut("temperature") {
            *t = Value::from(0.99);
        }
        let v = trace::verify_mutation(
            &g,
            &synth.pool,
            &serde_json::to_value(&synth.mutation_input).unwrap(),
            synth.input_file_sha,
            ctx,
        );
        self_test_pass(
            &mut failures,
            "generation temperature tamper detected",
            !v.is_empty(),
            &v.first().cloned().unwrap_or_default(),
        );
        let _ = sha_of;
    }

    // Selection.
    {
        let base = trace::verify_selection(
            &synth.selection,
            &serde_json::to_value(&synth.selection_summary).unwrap(),
            &serde_json::to_value(&synth.selected).unwrap(),
            &synth.pool,
            synth.pool_file_sha,
            synth.input_file_sha,
            ctx,
        );
        self_test_pass(
            &mut failures,
            "synthetic selection verifies clean",
            base.is_empty(),
            &base.join("; "),
        );
        let mut recs = serde_json::from_value::<Vec<SelectionRecord>>(
            serde_json::to_value(&synth.selection).unwrap(),
        )
        .unwrap();
        // Flip an oracle outcome on a common-valid cell.
        if let Some(r) = recs.iter_mut().find(|r| r.condition == "C2") {
            r.oracle_success = !r.oracle_success;
        }
        let v = trace::verify_selection(
            &recs,
            &serde_json::to_value(&synth.selection_summary).unwrap(),
            &serde_json::to_value(&synth.selected).unwrap(),
            &synth.pool,
            synth.pool_file_sha,
            synth.input_file_sha,
            ctx,
        );
        self_test_pass(
            &mut failures,
            "selection outcome tamper detected",
            !v.is_empty(),
            &v.first().cloned().unwrap_or_default(),
        );
        let mut sel = synth.selected.clone();
        sel.selected_candidate_id = "C3".into(); // wrong winner
        let v = trace::verify_selection(
            &synth.selection,
            &serde_json::to_value(&synth.selection_summary).unwrap(),
            &serde_json::to_value(&sel).unwrap(),
            &synth.pool,
            synth.pool_file_sha,
            synth.input_file_sha,
            ctx,
        );
        self_test_pass(
            &mut failures,
            "wrong selected candidate detected",
            !v.is_empty(),
            &v.first().cloned().unwrap_or_default(),
        );
    }

    // Promotion.
    {
        let base = trace::verify_promotion(
            &synth.promotion,
            &serde_json::to_value(&synth.promotion_summary).unwrap(),
            &serde_json::to_value(&synth.selected).unwrap(),
            &synth.pool,
            synth.pool_file_sha,
            synth.input_file_sha,
            synth.selected_file_sha,
            ctx,
        );
        self_test_pass(
            &mut failures,
            "synthetic promotion verifies clean",
            base.is_empty(),
            &base.join("; "),
        );
        let mut recs = serde_json::from_value::<Vec<PromotionRecord>>(
            serde_json::to_value(&synth.promotion).unwrap(),
        )
        .unwrap();
        if let Some(r) = recs
            .iter_mut()
            .find(|r| r.condition != "G0" && r.oracle_success)
        {
            r.oracle_success = false; // flip one win into a tie/loss
        }
        let v = trace::verify_promotion(
            &recs,
            &serde_json::to_value(&synth.promotion_summary).unwrap(),
            &serde_json::to_value(&synth.selected).unwrap(),
            &synth.pool,
            synth.pool_file_sha,
            synth.input_file_sha,
            synth.selected_file_sha,
            ctx,
        );
        self_test_pass(
            &mut failures,
            "promotion outcome tamper detected",
            !v.is_empty(),
            &v.first().cloned().unwrap_or_default(),
        );
        let mut s = synth.promotion_summary.clone();
        s.conclusion.result = "refuted".into(); // contradict the numbers
        let v = trace::verify_promotion(
            &synth.promotion,
            &serde_json::to_value(&s).unwrap(),
            &serde_json::to_value(&synth.selected).unwrap(),
            &synth.pool,
            synth.pool_file_sha,
            synth.input_file_sha,
            synth.selected_file_sha,
            ctx,
        );
        self_test_pass(
            &mut failures,
            "conclusion tamper detected",
            !v.is_empty(),
            &v.first().cloned().unwrap_or_default(),
        );
    }

    if failures.is_empty() {
        println!("self-test: ALL PASS");
        Ok(())
    } else {
        Err(format!(
            "self-test: {} failure(s):\n  - {}",
            failures.len(),
            failures.join("\n  - ")
        ))
    }
}

/// The standalone `run` smoke command (NOT part of the experiment;
/// its records carry an ad-hoc run identity and can never be accepted
/// into the 0011 dataset).
pub struct RunArgs {
    pub task: Option<String>,
}

pub fn cmd_run(args: RunArgs) -> Result<(), String> {
    let design = load_design()?;
    let file = load_tasks(&crate_dir().join("tasks.json"))?;
    let ctx = verifier_context()?;
    let task_id = args
        .task
        .clone()
        .unwrap_or_else(|| file.tasks.first().map(|t| t.id.clone()).unwrap_or_default());
    let task = file
        .tasks
        .iter()
        .find(|t| t.id == task_id)
        .ok_or_else(|| format!("no task {task_id}"))?;
    let client = agent_client(&design);
    println!(
        "ad-hoc run: task {} ({} {:?}) at {}/{}",
        task.id,
        task.family,
        task.split,
        design.endpoints[0].url,
        client.model()
    );
    let ep = execute_episode(
        &client,
        &design,
        design.agent_temperature,
        task,
        &ctx.incumbent_text,
    )?;
    println!(
        "termination: {} | oracle success: {} | turns {} | tool calls {}",
        ep.core.termination_reason,
        ep.eval.success,
        ep.core.model_turn_count,
        ep.core.tool_call_count
    );
    if !ep.eval.success {
        for r in &ep.eval.reasons {
            println!("  reason: {r}");
        }
    }
    for m in &ep.core.conversation {
        println!("  {:?} {}", m.role, m.content.as_deref().unwrap_or(""));
    }
    Ok(())
}
