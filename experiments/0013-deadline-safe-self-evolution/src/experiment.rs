//! Experiment 0013 — Frozen End-to-End Self-Evolution Confirmation:
//! command surface, live-stage orchestration, the two mechanical
//! freeze guards, preflight, and the network-free self-test.
//!
//! Every live stage (B/C/D/E) MUST be given the Stage A1 run manifest
//! (`--run-manifest`, default `run-manifest.json` in the crate dir).
//! BEFORE the model client is constructed and BEFORE any request, each
//! stage verifies BOTH:
//!   1. the source freeze (0011 mechanism, `src/freeze.rs`), and
//!   2. the previous stage's commit-level stage-artifact freeze
//!      (0013 mechanism, `src/stage_freeze.rs`):
//!      B needs only (1); C needs (1)+(B freeze); D needs (1)+(C
//!      freeze); E needs (1)+(D freeze) and a non-G0 selected
//!      candidate.
//!
//! If ANY check fails, NO model request is made and the run is
//! INCONCLUSIVE (no patching, no continuation, no restart).
//!
//! No-restart rule (0013 critical): a live stage whose output already
//! exists (the stage has emitted its first request) refuses to run
//! again — a crashed stage is INCONCLUSIVE, never resumed.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::freeze;
use crate::kernel::{self, KernelConfig};
use crate::model::{self, ModelClient, REGISTERED_ENDPOINT};
use crate::mutation::{
    self, CandidatePool, GenerationArtifact, GenerationAttempt, INCUMBENT_ID, build_mutation_input,
    build_pool, compose_full_prompt, validate_candidate_pool, validate_mutator_response,
};
use crate::oracle::{self, TaskFile, TaskSpec, TaskSplit};
use crate::protocol::{Message, sha256_bytes};
use crate::stage_freeze;
use crate::tools::{FaultMode, ToolExecutor};
use crate::trace::{
    self, Design, DiscoveryRecord, PromotionRecord, RecordProvenance, RunIdentity,
    SelectedCandidate, SelectionRecord, VerifierContext, frozen_stress, load_stress_profile,
};

// ===========================================================================
// Paths, file names, and loaders
// ===========================================================================

/// The experiment crate directory (independent of CWD).
pub fn crate_dir() -> PathBuf {
    freeze::crate_dir()
}

pub fn prompts_dir() -> PathBuf {
    crate_dir().join("prompts")
}

/// The repository root (git).
pub fn repo_root() -> Result<PathBuf, String> {
    freeze::repo_root()
}

/// The raw-artifact directory (`docs/experiments/artifacts`).
///
/// Falls back to the crate-dir/../../ layout when git metadata is
/// unavailable, so network-free commands degrade gracefully; the
/// registered layout in this repository is always the git one.
fn artifacts_dir() -> PathBuf {
    let base = repo_root().unwrap_or_else(|_| crate_dir().join("../.."));
    base.join("docs").join("experiments").join("artifacts")
}

/// Runtime evidence lives in the crate directory.
fn runtime_path(name: &str) -> PathBuf {
    crate_dir().join(name)
}

/// Raw trajectories/summaries live in `docs/experiments/artifacts`.
fn raw_path(name: &str) -> PathBuf {
    artifacts_dir().join(name)
}

/// The registered file names.
const MUTATION_INPUT: &str = "mutation-input.json";
const CANDIDATE_POOL: &str = "candidate-pool.json";
const SELECTED_CANDIDATE: &str = "selected-candidate.json";
const STAGE_B_FREEZE: &str = "stage-b-freeze.json";
const STAGE_C_FREEZE: &str = "stage-c-freeze.json";
const STAGE_D_FREEZE: &str = "stage-d-freeze.json";
const RAW_DISCOVERY_TRAJ: &str = "0013-discovery-trajectories.jsonl";
const RAW_DISCOVERY_SUMMARY: &str = "0013-discovery-summary.json";
const RAW_MUTATION_GENERATION: &str = "0013-mutation-generation.json";
const RAW_SELECTION_TRAJ: &str = "0013-selection-trajectories.jsonl";
const RAW_SELECTION_SUMMARY: &str = "0013-selection-summary.json";
const RAW_PROMOTION_TRAJ: &str = "0013-promotion-trajectories.jsonl";
const RAW_PROMOTION_SUMMARY: &str = "0013-promotion-summary.json";

/// The crate directory prefix as seen from the repository root
/// (derived from the registered frozen-path table, so it is
/// single-sourced).
fn crate_rel() -> String {
    let path = freeze::FROZEN_PATHS
        .iter()
        .find(|p| p.ends_with("/design.json"))
        .expect("the frozen design.json path is registered");
    path.rsplit_once('/')
        .map(|(dir, _)| format!("{dir}/"))
        .unwrap_or_default()
}

/// Repository-relative path of one runtime-evidence file.
fn crate_rel_path(name: &str) -> String {
    format!("{}{name}", crate_rel())
}

/// Repository-relative path of one raw artifact file.
fn raw_rel_path(name: &str) -> String {
    format!("docs/experiments/artifacts/{name}")
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
    if file.experiment_id != "0013" {
        return Err(format!(
            "task registry experiment_id {:?} does not match this experiment (0013)",
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
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Ok((text, sha256_bytes(&bytes)))
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
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| format!("cannot serialize {}: {e}", path.display()))?;
    let bytes = format!("{text}\n").into_bytes();
    std::fs::write(path, &bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(sha256_bytes(&bytes))
}

fn write_jsonl(path: &Path, rows: &[impl Serialize]) -> Result<String, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
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
// The two mechanical guards
// ===========================================================================

/// Load the Stage A1 run manifest, run the FULL source-freeze guard
/// (0011 mechanism), and return the run identity. Failure => no request
/// may occur and the run identity is permanently invalid.
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

/// Verify the PRECEDING stage's commit-level stage-artifact freeze
/// (0013 mechanism #2). Any violation ⇒ the next live stage refuses to
/// run (INCONCLUSIVE; no model request).
///
/// `manifest_name` is the runtime-evidence file name
/// (`stage-b-freeze.json` …); `expected_stage` its registered stage id.
pub fn verify_prev_stage_freeze(
    repo: &Path,
    manifest_name: &str,
    expected_stage: &str,
) -> Result<(), String> {
    let file = runtime_path(manifest_name);
    if !file.exists() {
        return Err(format!(
            "STAGE FREEZE VIOLATION — {manifest_name} does not exist: the {expected_stage} stage must be verified, manifested, and COMMITTED (artifacts + manifest together) before this live stage may run"
        ));
    }
    let f = stage_freeze::load_stage_freeze(&file)?;
    if f.stage != expected_stage {
        return Err(format!(
            "STAGE FREEZE VIOLATION — {manifest_name} has stage {:?}, expected {expected_stage}",
            f.stage
        ));
    }
    let v = stage_freeze::verify_stage_freeze(repo, &crate_rel_path(manifest_name), &f);
    if v.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "STAGE {expected_stage} FREEZE VIOLATION — the run is INCONCLUSIVE; no live stage may run. Violations:\n  - {}",
            v.join("\n  - ")
        ))
    }
}

/// No-restart rule (0013 critical): a live stage whose output file
/// already carries content has emitted its first request and must never
/// run again. A crashed stage is INCONCLUSIVE — no resume, no rerun.
fn ensure_not_started(label: &str, path: &Path) -> Result<(), String> {
    if let Ok(bytes) = std::fs::read(path) {
        if !bytes.is_empty() {
            return Err(format!(
                "{label} output {} already has content — no restart / resume / rerun is permitted; the stage is INCONCLUSIVE (record the stopping reason and stop)",
                path.display()
            ));
        }
    }
    Ok(())
}

// ===========================================================================
// Model clients (one registered endpoint, one registered model)
// ===========================================================================

/// The model client for agent execution. The endpoint URL and the model
/// are the single registered pair; there is no fallback model.
fn agent_client(design: &Design) -> ModelClient {
    let key = std::env::var("MUTAGEN_EXP_API_KEY")
        .ok()
        .filter(|k| !k.is_empty());
    ModelClient::new(REGISTERED_ENDPOINT.to_string(), design.model.clone(), key)
}

/// The model client for mutation generation (the SAME underlying model
/// as agent execution; no tools on the wire).
fn mutator_client(design: &Design) -> ModelClient {
    let key = std::env::var("MUTAGEN_EXP_API_KEY")
        .ok()
        .filter(|k| !k.is_empty());
    ModelClient::new(REGISTERED_ENDPOINT.to_string(), design.model.clone(), key)
}

/// The registered endpoint for provenance (redacted).
fn endpoint_redacted() -> String {
    model::redacted_endpoint(REGISTERED_ENDPOINT)
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
/// environment. The registered hard limits (turns, calls) AND the
/// registered deadline contract (episode deadline, request-timeout
/// ceiling, return tolerance) come from the design manifest — this
/// function owns NO timing constants. The kernel enforces the deadline
/// contract itself (start check, requested timeout, finish check,
/// response acceptance, discard of late responses); this closure only
/// executes the blocking model call with the kernel-registered
/// timeout. Never panics on model/tool failure: all failures are
/// classified and recorded.
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

    let outcome = kernel::run_episode(
        |messages, tool_specs, requested_timeout_ms| {
            // The exact registered integer timeout, as computed by the
            // kernel from the design-registered deadline contract.
            client.complete(
                messages,
                tool_specs,
                temperature,
                Duration::from_millis(requested_timeout_ms),
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
        episode_id: format!("{}-disc-{seq:03}", design.run_id),
        sequence: seq,
        family: task.family.clone(),
        task_id: task.id.clone(),
        split: "discovery".into(),
        stress_level: level.to_string(),
        drop_count: count,
        fault_key: task.fault_key.clone(),
        repetition: rep,
        condition: INCUMBENT_ID.into(),
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
// Stage B — discovery (live; source-frozen; first stage after A1)
// ===========================================================================

/// Stage B runs the COMPLETE registered 18-episode discovery stage.
/// 0013 deliberately has no `--family` / `--task` live filter: any
/// subset request would be an unregistered endpoint request.
pub struct DiscoverArgs {
    pub manifest: PathBuf,
}

pub fn cmd_discover(args: DiscoverArgs) -> Result<(), String> {
    // 1. The source-freeze guard FIRST: no model client, no request, if violated.
    let (run, manifest) = frozen_run(&args.manifest)?;
    let design = load_design()?;
    let ctx = verifier_context()?;
    let registry_file = load_tasks(&crate_dir().join("tasks.json"))?;

    // 2. No-restart: a partially-written discovery stage never reruns.
    ensure_not_started("Stage B (discovery)", &raw_path(RAW_DISCOVERY_TRAJ))?;

    let client = agent_client(&design);
    let prov = base_provenance(
        &ctx,
        &manifest,
        &run,
        client.model(),
        design.agent_temperature,
        &endpoint_redacted(),
    );

    let discovery_tasks: Vec<&TaskSpec> = registry_file
        .tasks
        .iter()
        .filter(|t| t.split == TaskSplit::Discovery)
        .collect();
    if discovery_tasks.is_empty() {
        return Err("no discovery tasks registered".into());
    }
    let reps = design.discovery_repetitions();
    let all_index: std::collections::BTreeMap<String, usize> = registry_file
        .tasks
        .iter()
        .filter(|t| t.split == TaskSplit::Discovery)
        .enumerate()
        .map(|(i, t)| (t.id.clone(), i))
        .collect();

    let out = raw_path(RAW_DISCOVERY_TRAJ);
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
            // (inconclusive, non-restartable) artifact.
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

    // Mutation input (whitelist) + summary + canonical rewrite.
    let input = build_mutation_input(
        &design,
        &ctx.incumbent_text,
        &ToolExecutor::specs(),
        &records,
    );
    let input_sha = write_json(&runtime_path(MUTATION_INPUT), &input)?;
    let summary = trace::compute_discovery_summary(
        &records,
        &design,
        &ctx.registry,
        &run,
        client.model(),
        &endpoint_redacted(),
        &manifest.code_under_test_commit,
        &ctx.incumbent_file_sha,
        &ctx.task_registry_sha,
        &ctx.profile_sha,
        &input_sha,
    );
    let _ = write_json(&raw_path(RAW_DISCOVERY_SUMMARY), &summary);
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
        return Err(
            "discovery gates failed — INCONCLUSIVE: no mutation generation, no stage-B freeze; stop"
                .into(),
        );
    }
    if !violations.is_empty() {
        return Err(
            "discovery verification violations — INCONCLUSIVE: no stage-B freeze may be written; stop"
                .into(),
        );
    }

    // Gates pass + verifier clean → the stage is FROZEN: build the
    // stage-B freeze manifest (3 artifacts, repo-relative paths).
    // Committing is the next human/agent step (see printed command).
    let b_freeze = stage_freeze::build_stage_freeze(
        &design.run_id,
        "B",
        &[
            (
                raw_rel_path(RAW_DISCOVERY_TRAJ),
                raw_path(RAW_DISCOVERY_TRAJ),
            ),
            (
                raw_rel_path(RAW_DISCOVERY_SUMMARY),
                raw_path(RAW_DISCOVERY_SUMMARY),
            ),
            (crate_rel_path(MUTATION_INPUT), runtime_path(MUTATION_INPUT)),
        ],
    )?;
    stage_freeze::write_stage_freeze(&runtime_path(STAGE_B_FREEZE), &b_freeze)?;
    println!(
        "[discover] stage-B freeze manifest written: {} ({} artifacts)",
        runtime_path(STAGE_B_FREEZE).display(),
        b_freeze.artifacts.len()
    );
    println!(
        "NEXT — commit B artifacts + manifest together, then run Stage C:\n  git add {t} {s} {mi} {mf}\n  git commit -m \"exp: freeze 0013 discovery evidence\"\n  cargo run -p mutagen-exp-0013 -- generate",
        t = raw_rel_path(RAW_DISCOVERY_TRAJ),
        s = raw_rel_path(RAW_DISCOVERY_SUMMARY),
        mi = crate_rel_path(MUTATION_INPUT),
        mf = crate_rel_path(STAGE_B_FREEZE),
    );
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
// Stage C — mutation generation (live; source-frozen + B-frozen)
// ===========================================================================

pub struct GenerateArgs {
    pub manifest: PathBuf,
}

pub fn cmd_generate(args: GenerateArgs) -> Result<(), String> {
    // 1. Source freeze + the previous stage's COMMITTED freeze.
    let (run, manifest) = frozen_run(&args.manifest)?;
    let repo = freeze::repo_root()?;
    verify_prev_stage_freeze(&repo, STAGE_B_FREEZE, "B")?;
    // 2. No-restart rule.
    ensure_not_started(
        "Stage C (mutation generation)",
        &raw_path(RAW_MUTATION_GENERATION),
    )?;

    let design = load_design()?;
    let ctx = verifier_context()?;

    // The discovery gate must have passed.
    let summary_v = read_json(&raw_path(RAW_DISCOVERY_SUMMARY))?;
    let gates_ok = summary_v
        .get("proceed_to_mutation_generation")
        .and_then(Value::as_bool)
        == Some(true);
    if !gates_ok {
        return Err("the discovery gate did not pass — mutation generation must not run".into());
    }

    let input_v = read_json(&runtime_path(MUTATION_INPUT))?;
    let input_sha = file_sha(&runtime_path(MUTATION_INPUT))?;
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
        // The single registered timeout for the mutation-generation
        // request: the design-registered request-timeout ceiling
        // (Stage C is not an agent episode; the per-episode deadline
        // contract applies to Stages B/D/E agent requests).
        let result = client.complete_mutator(
            &messages,
            design.mutator_temperature,
            Duration::from_millis(design.deadline.request_timeout_ceiling_ms),
        );
        let wall = t0.elapsed().as_millis() as u64;
        let raw = result.as_ref().ok().and_then(|r| r.content.clone());
        let (mut_raw, verrs) = match &raw {
            Some(r) => validate_mutator_response(&design, r, &ctx.incumbent_text, &ctx.registry),
            None => (
                None,
                vec![
                    "mutator request failed at the transport/status level (classified; no raw content)".to_string(),
                ],
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
        let u = result.as_ref().ok().map(|r| &r.usage);
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
        // Persist after EVERY attempt: a crash mid-Stage-C leaves
        // evidence that the stage started, so the no-restart rule
        // applies mechanically.
        let artifact = GenerationArtifact {
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
            attempts: attempts.clone(),
            mutation_generation_complete: pool.is_some(),
            accepted_pool: pool.clone(),
            wall_time_ms: attempts.iter().map(|a| a.wall_time_ms).sum(),
        };
        write_json(&raw_path(RAW_MUTATION_GENERATION), &artifact)?;

        if pool.is_some() {
            break;
        }
        // The registered retry: the previous raw response stays in the
        // conversation; the retry message carries ONLY the machine
        // validation errors + the exact expected structure (no
        // performance / selection / cost feedback, no human rewrite).
        messages.push(Message::assistant_text(
            attempts
                .last()
                .and_then(|a| a.raw_response.clone())
                .unwrap_or_default(),
        ));
        messages.push(Message::user(mutation::retry_message(
            &attempts.last().unwrap().validation_errors,
        )));
    }

    let complete = pool.is_some();
    let pool_ref = pool.clone();
    let _pool_file_sha = pool_ref
        .as_ref()
        .map(|p| write_json(&runtime_path(CANDIDATE_POOL), p))
        .transpose()?;
    let total_wall: u64 = attempts.iter().map(|a| a.wall_time_ms).sum();
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
    write_json(&raw_path(RAW_MUTATION_GENERATION), &generation)?;
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
        let _ = write_json(&runtime_path(CANDIDATE_POOL), &empty);
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
            "mutation generation did not produce a fully valid pool within the registered 2 attempts — INCONCLUSIVE: no stage-C freeze, no selection; stop"
                .into(),
        );
    }
    if !violations.is_empty() {
        return Err(
            "mutation verification violations — INCONCLUSIVE: no stage-C freeze may be written; stop"
                .into(),
        );
    }

    // Complete + clean → the stage is FROZEN (stage-C freeze manifest).
    let c_freeze = stage_freeze::build_stage_freeze(
        &design.run_id,
        "C",
        &[
            (
                raw_rel_path(RAW_MUTATION_GENERATION),
                raw_path(RAW_MUTATION_GENERATION),
            ),
            (crate_rel_path(CANDIDATE_POOL), runtime_path(CANDIDATE_POOL)),
        ],
    )?;
    stage_freeze::write_stage_freeze(&runtime_path(STAGE_C_FREEZE), &c_freeze)?;
    println!(
        "[generate] stage-C freeze manifest written: {} ({} artifacts)",
        runtime_path(STAGE_C_FREEZE).display(),
        c_freeze.artifacts.len()
    );
    println!(
        "NEXT — commit C artifacts + manifest together, then run Stage D:\n  git add {} {}\n  git commit -m \"exp: freeze 0013 generated candidate pool\"\n  cargo run -p mutagen-exp-0013 -- select",
        raw_rel_path(RAW_MUTATION_GENERATION),
        crate_rel_path(CANDIDATE_POOL),
    );
    Ok(())
}

// ===========================================================================
// Stage D — selection (live; source-frozen + C-frozen)
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
        episode_id: format!("{}-sel-{seq:03}", design.run_id),
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
    // 1. Source freeze + the previous stage's COMMITTED freeze.
    let (run, manifest) = frozen_run(&args.manifest)?;
    let repo = freeze::repo_root()?;
    verify_prev_stage_freeze(&repo, STAGE_C_FREEZE, "C")?;
    // 2. No-restart rule.
    ensure_not_started("Stage D (selection)", &raw_path(RAW_SELECTION_TRAJ))?;

    let design = load_design()?;
    let ctx = verifier_context()?;

    let pool = read_json(&runtime_path(CANDIDATE_POOL))
        .and_then(|v| serde_json::from_value::<CandidatePool>(v).map_err(|e| e.to_string()))?;
    if pool.candidates.len() != design.candidate_count as usize {
        return Err("candidate pool is not complete — selection must not run".into());
    }
    if pool.mutation_input_sha256.is_empty() {
        return Err("candidate pool is missing its mutation input binding".into());
    }
    let pool_file_sha = file_sha(&runtime_path(CANDIDATE_POOL))?;
    let input_file_sha = file_sha(&runtime_path(MUTATION_INPUT))?;

    let client = agent_client(&design);
    let prov = base_provenance(
        &ctx,
        &manifest,
        &run,
        client.model(),
        design.agent_temperature,
        &endpoint_redacted(),
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
                        .open(raw_path(RAW_SELECTION_TRAJ))
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
        &endpoint_redacted(),
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
        selection_raw_sha256: file_sha(&raw_path(RAW_SELECTION_TRAJ))?,
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
    let summary_file_sha = write_json(&raw_path(RAW_SELECTION_SUMMARY), &summary)?;
    let mut selected = selected;
    selected.selection_summary_sha256 = summary_file_sha;
    let _ = write_json(&runtime_path(SELECTED_CANDIDATE), &selected);
    let _ = write_jsonl(&raw_path(RAW_SELECTION_TRAJ), &records);

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

    if !summary.gates.all() {
        return Err(
            "selection gates failed — INCONCLUSIVE: no stage-D freeze, no promotion; stop".into(),
        );
    }
    if !violations.is_empty() {
        return Err(
            "selection verification violations — INCONCLUSIVE: no stage-D freeze may be written; stop"
                .into(),
        );
    }

    // The stage completed and is clean → the stage is FROZEN.
    let d_freeze = stage_freeze::build_stage_freeze(
        &design.run_id,
        "D",
        &[
            (
                raw_rel_path(RAW_SELECTION_TRAJ),
                raw_path(RAW_SELECTION_TRAJ),
            ),
            (
                raw_rel_path(RAW_SELECTION_SUMMARY),
                raw_path(RAW_SELECTION_SUMMARY),
            ),
            (
                crate_rel_path(SELECTED_CANDIDATE),
                runtime_path(SELECTED_CANDIDATE),
            ),
        ],
    )?;
    stage_freeze::write_stage_freeze(&runtime_path(STAGE_D_FREEZE), &d_freeze)?;
    println!(
        "[select] stage-D freeze manifest written: {} ({} artifacts)",
        runtime_path(STAGE_D_FREEZE).display(),
        d_freeze.artifacts.len()
    );
    println!(
        "NEXT — commit D artifacts + manifest together:\n  git add {} {} {}\n  git commit -m \"exp: freeze 0013 selected candidate before promotion\"",
        raw_rel_path(RAW_SELECTION_TRAJ),
        raw_rel_path(RAW_SELECTION_SUMMARY),
        crate_rel_path(SELECTED_CANDIDATE),
    );
    if summary.selected.incumbent_retained {
        println!(
            "\nFORMAL RESULT AT SELECTION: REFUTED — the incumbent G0 was retained (no candidate with net margin > 0). Promotion MUST NOT run; commit the artifacts, record the result, and stop."
        );
    } else {
        println!(
            "\nCandidate {} displaced G0; Stage E (promotion) may now run after the D freeze commit.",
            summary.selected.selected_candidate_id
        );
    }
    Ok(())
}

// ===========================================================================
// Stage E — promotion (live; source-frozen + D-frozen; no next stage)
// ===========================================================================

pub struct PromoteArgs {
    pub manifest: PathBuf,
}

pub fn cmd_promote(args: PromoteArgs) -> Result<(), String> {
    // 1. Source freeze + the previous stage's COMMITTED freeze.
    let (run, manifest) = frozen_run(&args.manifest)?;
    let repo = freeze::repo_root()?;
    verify_prev_stage_freeze(&repo, STAGE_D_FREEZE, "D")?;
    // 2. No-restart rule.
    ensure_not_started("Stage E (promotion)", &raw_path(RAW_PROMOTION_TRAJ))?;

    let design = load_design()?;
    let ctx = verifier_context()?;

    let selected_v = read_json(&runtime_path(SELECTED_CANDIDATE))?;
    let selected: SelectedCandidate =
        serde_json::from_value(selected_v).map_err(|e| format!("selected candidate: {e}"))?;
    let selected_id = selected.selected_candidate_id.clone();
    if selected.incumbent_retained() {
        return Err(
            "the incumbent G0 was retained at selection — promotion must NOT run (formal result is REFUTED; commit and stop)".into(),
        );
    }
    if selected_id != INCUMBENT_ID && !design.candidate_ids().contains(&selected_id) {
        return Err(format!(
            "selected candidate {selected_id} is not in the registered candidate set"
        ));
    }
    let pool = read_json(&runtime_path(CANDIDATE_POOL))
        .and_then(|v| serde_json::from_value::<CandidatePool>(v).map_err(|e| e.to_string()))?;
    let pool_file_sha = file_sha(&runtime_path(CANDIDATE_POOL))?;
    let input_file_sha = file_sha(&runtime_path(MUTATION_INPUT))?;
    let selected_file_sha = file_sha(&runtime_path(SELECTED_CANDIDATE))?;

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
        &endpoint_redacted(),
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
                    episode_id: format!("{}-prom-{seq:03}", design.run_id),
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
                        .open(raw_path(RAW_PROMOTION_TRAJ))
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
        &endpoint_redacted(),
        &manifest.code_under_test_commit,
        &ctx.incumbent_file_sha,
        &ctx.task_registry_sha,
        &ctx.profile_sha,
        &pool_file_sha,
        &selected_file_sha,
        &input_file_sha,
    );
    let _ = write_json(&raw_path(RAW_PROMOTION_SUMMARY), &summary);
    let _ = write_jsonl(&raw_path(RAW_PROMOTION_TRAJ), &records);

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
    let manifest = freeze::build_manifest(&root, &commit, "0013-r1", "0013")?;
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
    println!(
        "AFTER THIS COMMIT: no unregistered live model requests (zero smoke/debug requests); start Stage B directly."
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
        "  git rev-parse HEAD   # → pass to: cargo run -p mutagen-exp-0013 -- init-run --commit <SHA>"
    );
}

// ===========================================================================
// Network-free verify commands
// ===========================================================================

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
        let records =
            read_jsonl::<DiscoveryRecord>(&raw_path(RAW_DISCOVERY_TRAJ)).unwrap_or_default();
        let summary = read_json(&raw_path(RAW_DISCOVERY_SUMMARY)).unwrap_or(Value::Null);
        let input = read_json(&runtime_path(MUTATION_INPUT)).unwrap_or(Value::Null);
        let input_sha = file_sha(&runtime_path(MUTATION_INPUT)).unwrap_or_default();
        let v = trace::verify_discovery(&records, &summary, &input, &input_sha, ctx);
        ("verify-discovery".to_string(), v)
    })
}

pub fn cmd_verify_mutation() -> Result<(), String> {
    verify_command(|ctx| {
        let generation = read_json(&raw_path(RAW_MUTATION_GENERATION)).unwrap_or(Value::Null);
        let input = read_json(&runtime_path(MUTATION_INPUT)).unwrap_or(Value::Null);
        let input_sha = file_sha(&runtime_path(MUTATION_INPUT)).unwrap_or_default();
        let pool = read_json(&runtime_path(CANDIDATE_POOL))
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
        let records =
            read_jsonl::<SelectionRecord>(&raw_path(RAW_SELECTION_TRAJ)).unwrap_or_default();
        let summary = read_json(&raw_path(RAW_SELECTION_SUMMARY)).unwrap_or(Value::Null);
        let selected = read_json(&runtime_path(SELECTED_CANDIDATE)).unwrap_or(Value::Null);
        let pool = read_json(&runtime_path(CANDIDATE_POOL))
            .and_then(|v| serde_json::from_value::<CandidatePool>(v).map_err(|e| e.to_string()))
            .unwrap_or(CandidatePool {
                experiment_id: String::new(),
                generation: mutation::CANDIDATE_GENERATION,
                parent_id: INCUMBENT_ID.to_string(),
                parent_prompt_sha256: String::new(),
                mutation_input_sha256: String::new(),
                candidates: Vec::new(),
            });
        let pool_sha = file_sha(&runtime_path(CANDIDATE_POOL)).unwrap_or_default();
        let input_sha = file_sha(&runtime_path(MUTATION_INPUT)).unwrap_or_default();
        let v = trace::verify_selection(
            &records, &summary, &selected, &pool, &pool_sha, &input_sha, ctx,
        );
        ("verify-selection".to_string(), v)
    })
}

pub fn cmd_verify_promotion() -> Result<(), String> {
    verify_command(|ctx| {
        let records =
            read_jsonl::<PromotionRecord>(&raw_path(RAW_PROMOTION_TRAJ)).unwrap_or_default();
        let summary = read_json(&raw_path(RAW_PROMOTION_SUMMARY)).unwrap_or(Value::Null);
        let selected = read_json(&runtime_path(SELECTED_CANDIDATE)).unwrap_or(Value::Null);
        let pool = read_json(&runtime_path(CANDIDATE_POOL))
            .and_then(|v| serde_json::from_value::<CandidatePool>(v).map_err(|e| e.to_string()))
            .unwrap_or(CandidatePool {
                experiment_id: String::new(),
                generation: mutation::CANDIDATE_GENERATION,
                parent_id: INCUMBENT_ID.to_string(),
                parent_prompt_sha256: String::new(),
                mutation_input_sha256: String::new(),
                candidates: Vec::new(),
            });
        let pool_sha = file_sha(&runtime_path(CANDIDATE_POOL)).unwrap_or_default();
        let input_sha = file_sha(&runtime_path(MUTATION_INPUT)).unwrap_or_default();
        let selected_sha = file_sha(&runtime_path(SELECTED_CANDIDATE)).unwrap_or_default();
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

/// Network-free verification of the run manifest + source freeze state.
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

/// Network-free verification of every PRESENT stage-artifact freeze
/// against Git (B, then C, then D). A missing stage freeze reports
/// "not present" (the stage was not reached) but is not itself a
/// violation; a PRESENT freeze with any Git provenance violation is an
/// error.
pub fn cmd_verify_stage_freeze() -> Result<(), String> {
    let root = repo_root()?;
    let mut any_violation = false;
    for (name, stage) in [
        (STAGE_B_FREEZE, "B"),
        (STAGE_C_FREEZE, "C"),
        (STAGE_D_FREEZE, "D"),
    ] {
        let file = runtime_path(name);
        if !file.exists() {
            println!("stage-{stage} freeze: not present (stage not frozen or not reached)");
            continue;
        }
        let f = stage_freeze::load_stage_freeze(&file)?;
        let v = stage_freeze::verify_stage_freeze(&root, &crate_rel_path(name), &f);
        if v.is_empty() {
            println!(
                "stage-{stage} freeze: OK ({} artifacts, run {})",
                f.artifacts.len(),
                f.run_id
            );
        } else {
            any_violation = true;
            println!("stage-{stage} freeze: {} violation(s):", v.len());
            for x in &v {
                println!("  - {x}");
            }
        }
    }
    if any_violation {
        Err("stage-freeze violation(s) found — the run is INCONCLUSIVE".into())
    } else {
        Ok(())
    }
}

// ===========================================================================
// The mechanical checks shared by `preflight` and `self-test`
// ===========================================================================

/// One named, boolean, network-free check.
pub struct Check {
    pub name: &'static str,
    pub passed: bool,
    pub detail: String,
}

impl Check {
    fn new(name: &'static str, passed: bool, detail: String) -> Self {
        Self {
            name,
            passed,
            detail,
        }
    }
}

fn check_ok(name: &'static str, ok: bool, detail: String) -> Check {
    Check::new(name, ok, if ok { "ok".into() } else { detail })
}

/// A minimal git repo for the mechanical-check scenarios.
fn temp_repo(tag: &str, files: &[(&str, &str)]) -> (PathBuf, String) {
    let root = std::env::temp_dir().join(format!(
        "mutagen-0013-check-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    for (rel, content) in files {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, content).unwrap();
    }
    let git_in = |dir: &Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git_in(&root, &["init", "-q"]);
    git_in(&root, &["config", "user.email", "t@t.t"]);
    git_in(&root, &["config", "user.name", "t"]);
    if files.is_empty() {
        git_in(&root, &["commit", "-q", "--allow-empty", "-m", "base"]);
    } else {
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "A0"]);
    }
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&root)
        .output()
        .unwrap();
    (
        root,
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    )
}

fn git_commit(dir: &Path, msg: &str) {
    let out = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success());
    let out = std::process::Command::new("git")
        .args(["commit", "-q", "-m", msg])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The source-freeze guard exercised against real temporary git
/// repositories (0010's failure mode: mid-run source change; plus the
/// modify→commit→revert history case and the byte-drift case).
pub fn source_freeze_checks() -> Vec<Check> {
    use crate::freeze::{FrozenFile, combined_frozen_sha};

    fn mk(files: &[(&str, &str)], commit: &str) -> freeze::RunManifest {
        let mut fs = files
            .iter()
            .map(|(p, c)| FrozenFile {
                path: (*p).to_string(),
                sha256: sha256_bytes(c.as_bytes()),
            })
            .collect::<Vec<_>>();
        fs.sort_by(|a, b| a.path.cmp(&b.path));
        freeze::RunManifest {
            experiment_id: "0013".into(),
            run_id: "0013-r1".into(),
            code_under_test_commit: commit.to_string(),
            frozen_design_sha256: combined_frozen_sha(&fs),
            files: fs,
            design_sha256: String::new(),
            tasks_sha256: String::new(),
            stress_profile_sha256: String::new(),
            incumbent_prompt_sha256: String::new(),
            mutator_prompt_sha256: String::new(),
        }
    }

    let mut checks = Vec::new();

    // (a) pristine freeze passes; (b) working-tree byte change detected.
    let (root, commit) = temp_repo(
        "src-pristine",
        &[
            ("frozen/a.rs", "v1"),
            ("frozen/b.rs", "v2"),
            ("art/x.json", "z"),
        ],
    );
    let m = mk(&[("frozen/a.rs", "v1"), ("frozen/b.rs", "v2")], &commit);
    let v = freeze::verify_freeze(&root, &m, &["frozen/a.rs", "frozen/b.rs"]);
    checks.push(check_ok(
        "source freeze: pristine freeze accepted",
        v.is_empty(),
        v.join("; "),
    ));
    std::fs::write(root.join("frozen/a.rs"), "TAMPERED").unwrap();
    let v = freeze::verify_freeze(&root, &m, &["frozen/a.rs", "frozen/b.rs"]);
    checks.push(check_ok(
        "source change after A0 blocked",
        v.iter()
            .any(|x| x.contains("current bytes differ") || x.contains("working-tree drift")),
        v.join("; "),
    ));
    let _ = std::fs::remove_dir_all(&root);

    // (c) committed mid-run change detected; (d) modify→commit→revert
    // detected (history, not net diff).
    let (root, commit) = temp_repo("src-history", &[("frozen/a.rs", "v1"), ("art/x.json", "z")]);
    let m2 = mk(&[("frozen/a.rs", "v1")], &commit);
    std::fs::write(root.join("frozen/a.rs"), "v2").unwrap();
    git_commit(&root, "mid-run bugfix");
    let v = freeze::verify_freeze(&root, &m2, &["frozen/a.rs"]);
    checks.push(check_ok(
        "mid-run source commit blocked (0010 failure mode)",
        v.iter().any(|x| x.contains("commit(s) after")),
        v.join("; "),
    ));
    std::fs::write(root.join("frozen/a.rs"), "v1").unwrap();
    git_commit(&root, "revert");
    let v = freeze::verify_freeze(&root, &m2, &["frozen/a.rs"]);
    checks.push(check_ok(
        "source modify-commit-revert blocked (history, not net diff)",
        v.iter().any(|x| x.contains("commit(s) after")),
        v.join("; "),
    ));
    let _ = std::fs::remove_dir_all(&root);

    checks
}

/// The commit-level stage-freeze mechanism exercised against real
/// temporary git repositories (the 0011 Stage-B provenance gap, closed).
pub fn stage_freeze_checks() -> Vec<Check> {
    let mut checks = Vec::new();
    const MANIFEST: &str = "stage-b-freeze.json";

    fn freeze_for(artifacts: &[(&str, &str)]) -> stage_freeze::StageFreeze {
        let mut rows = Vec::new();
        for (p, c) in artifacts {
            rows.push(stage_freeze::StageFreezeArtifact {
                path: (*p).to_string(),
                sha256: sha256_bytes(c.as_bytes()),
            });
        }
        rows.sort_by(|a, b| a.path.cmp(&b.path));
        stage_freeze::StageFreeze {
            stage: "B".into(),
            run_id: "0013-r1".into(),
            artifacts: rows,
        }
    }

    // (1) pristine committed freeze verifies.
    {
        let (root, _c) = temp_repo("sf-pristine", &[]);
        let f = freeze_for(&[("a/x.json", "x1")]);
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("a/x.json"), "x1").unwrap();
        let text = serde_json::to_string_pretty(&f).unwrap();
        std::fs::write(root.join(MANIFEST), format!("{text}\n")).unwrap();
        git_commit(&root, "freeze B");
        let v = stage_freeze::verify_stage_freeze(&root, MANIFEST, &f);
        checks.push(check_ok(
            "stage freeze: pristine committed freeze verifies",
            v.is_empty(),
            v.join("; "),
        ));
        let _ = std::fs::remove_dir_all(&root);
    }
    // (2) B not committed before C → C blocked.
    {
        let (root, _c) = temp_repo("sf-uncommitted", &[]);
        let f = freeze_for(&[("a/x.json", "x1")]);
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("a/x.json"), "x1").unwrap();
        let text = serde_json::to_string_pretty(&f).unwrap();
        std::fs::write(root.join(MANIFEST), format!("{text}\n")).unwrap();
        // Nothing committed after the base commit: the manifest is untracked.
        let v = stage_freeze::verify_stage_freeze(&root, MANIFEST, &f);
        let c = check_ok(
            "B not committed before C: C blocked",
            v.iter().any(|x| x.contains("is not committed")),
            v.join("; "),
        );
        checks.push(c);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (3) B committed then changed → C blocked.
    {
        let (root, _c) = temp_repo("sf-later-edit", &[]);
        let f = freeze_for(&[("a/x.json", "x1")]);
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("a/x.json"), "x1").unwrap();
        let text = serde_json::to_string_pretty(&f).unwrap();
        std::fs::write(root.join(MANIFEST), format!("{text}\n")).unwrap();
        git_commit(&root, "freeze B");
        std::fs::write(root.join("a/x.json"), "TAMPERED").unwrap();
        let v = stage_freeze::verify_stage_freeze(&root, MANIFEST, &f);
        let c = check_ok(
            "B committed then changed: C blocked",
            v.iter().any(|x| x.contains("current bytes differ")),
            v.join("; "),
        );
        checks.push(c);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (4) later commit touching a frozen stage artifact detected (history).
    {
        let (root, _c) = temp_repo("sf-later-commit", &[]);
        let f = freeze_for(&[("a/x.json", "x1")]);
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("a/x.json"), "x1").unwrap();
        let text = serde_json::to_string_pretty(&f).unwrap();
        std::fs::write(root.join(MANIFEST), format!("{text}\n")).unwrap();
        git_commit(&root, "freeze B");
        std::fs::write(root.join("a/x.json"), "x2").unwrap();
        git_commit(&root, "later edit");
        std::fs::write(root.join("a/x.json"), "x1").unwrap();
        git_commit(&root, "revert");
        let v = stage_freeze::verify_stage_freeze(&root, MANIFEST, &f);
        let c = check_ok(
            "later edit of frozen stage artifact detected (history)",
            v.iter().any(|x| x.contains("commit(s) after")),
            v.join("; "),
        );
        checks.push(c);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (5) uncommitted previous-stage artifact blocks the next stage.
    {
        let (root, _c) = temp_repo("sf-art-missing", &[]);
        let f = freeze_for(&[("a/x.json", "x1")]);
        let text = serde_json::to_string_pretty(&f).unwrap();
        std::fs::write(root.join(MANIFEST), format!("{text}\n")).unwrap();
        git_commit(&root, "freeze B (artifact left uncommitted)");
        let v = stage_freeze::verify_stage_freeze(&root, MANIFEST, &f);
        let c = check_ok(
            "uncommitted previous-stage artifact blocks next stage",
            v.iter()
                .any(|x| x.contains("does not exist at the freeze commit")),
            v.join("; "),
        );
        checks.push(c);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (6) tampered freeze-manifest hash detected.
    {
        let (root, _c) = temp_repo("sf-tamper", &[]);
        let mut f = freeze_for(&[("a/x.json", "x1")]);
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("a/x.json"), "x1").unwrap();
        let text = serde_json::to_string_pretty(&f).unwrap();
        std::fs::write(root.join(MANIFEST), format!("{text}\n")).unwrap();
        git_commit(&root, "freeze B");
        f.artifacts[0].sha256 = "f".repeat(64);
        let v = stage_freeze::verify_stage_freeze(&root, MANIFEST, &f);
        let c = check_ok(
            "tampered stage-freeze artifact hash detected",
            v.iter()
                .any(|x| x.contains("does not match the manifest hash")),
            v.join("; "),
        );
        checks.push(c);
        let _ = std::fs::remove_dir_all(&root);
    }
    // (7) C-not-committed-before-D / D-not-committed-before-E: the same
    // mechanism with the registered stage labels, driven through the
    // stage-gate wrapper the live commands use.
    {
        let (root, _c) = temp_repo("sf-gates", &[]);
        let c_missing = verify_prev_stage_freeze_in(&root, "stage-c-freeze.json", "C").is_err();
        let d_missing = verify_prev_stage_freeze_in(&root, "stage-d-freeze.json", "D").is_err();
        checks.push(check_ok(
            "C not committed before D: D blocked",
            c_missing,
            "gate unexpectedly passed".into(),
        ));
        checks.push(check_ok(
            "D not committed before E: E blocked",
            d_missing,
            "gate unexpectedly passed".into(),
        ));
        let _ = std::fs::remove_dir_all(&root);
    }
    checks
}

/// The stage-gate as exercised against an arbitrary repository root
/// (the live commands call the same logic against the real repository).
fn verify_prev_stage_freeze_in(
    repo: &Path,
    manifest_name: &str,
    expected_stage: &str,
) -> Result<(), String> {
    let file = repo.join(manifest_name);
    if !file.exists() {
        return Err(format!(
            "STAGE FREEZE VIOLATION — {manifest_name} does not exist: the {expected_stage} stage must be verified, manifested, and COMMITTED (artifacts + manifest together) before this live stage may run"
        ));
    }
    let f = stage_freeze::load_stage_freeze(&file)?;
    if f.stage != expected_stage {
        return Err(format!(
            "STAGE FREEZE VIOLATION — {manifest_name} has stage {:?}, expected {expected_stage}",
            f.stage
        ));
    }
    let v = stage_freeze::verify_stage_freeze(repo, manifest_name, &f);
    if v.is_empty() {
        Ok(())
    } else {
        Err(v.join("\n"))
    }
}

/// The endpoint-discipline CLI check: the registered command table has
/// exactly the four live stages and NO ad-hoc live command.
pub fn cli_discipline_checks() -> Vec<Check> {
    let mut checks = Vec::new();
    let commands = crate::registered_commands();
    let live: Vec<&str> = commands
        .iter()
        .filter(|(_, is_live)| *is_live)
        .map(|(name, _)| *name)
        .collect();
    let names: Vec<&str> = commands.iter().map(|(name, _)| *name).collect();
    checks.push(check_ok(
        "live command set is exactly {discover,generate,select,promote}",
        live == vec!["discover", "generate", "select", "promote"]
            || (live.len() == 4
                && live.contains(&"discover")
                && live.contains(&"generate")
                && live.contains(&"select")
                && live.contains(&"promote")),
        format!("live: {live:?}"),
    ));
    let mut adhoc = Vec::new();
    for forbidden in ["run", "smoke-model", "debug-live", "probe-endpoint"] {
        if names.contains(&forbidden) {
            adhoc.push(forbidden);
        }
    }
    checks.push(check_ok(
        "no live debug CLI command exists (run / smoke-model / debug-live / probe-endpoint)",
        adhoc.is_empty(),
        format!("present: {adhoc:?}"),
    ));
    checks
}

/// Agent request wire semantics: `parallel_tool_calls = false` and
/// `tool_choice = "auto"` (one tool call per turn, frozen in 0013).
pub fn parallel_tool_check() -> Check {
    use crate::protocol::{ChatRequest, Message, ToolFunctionSpec, ToolSpec};
    let req = ChatRequest::agent(
        "m",
        vec![Message::system("s"), Message::user("u")],
        vec![ToolSpec {
            kind: "function".into(),
            function: ToolFunctionSpec {
                name: "state_read".into(),
                description: "d".into(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }],
        0.2,
    );
    let v = serde_json::to_value(&req).unwrap();
    let ok = v.get("parallel_tool_calls") == Some(&serde_json::Value::Bool(false))
        && v.get("tool_choice") == Some(&serde_json::Value::String("auto".into()));
    check_ok(
        "agent request serializes parallel_tool_calls=false + tool_choice=auto",
        ok,
        format!("{v:?}"),
    )
}

/// The 0013 deadline-contract checks (spec §59): the registered
/// values, the removed duplicate constants, the mandatory request-level
/// timing evidence, the removed aggregate wall-time rule, the §57
/// request-evidence cases, and the kernel response-discard behavior.
pub fn deadline_contract_checks() -> Vec<Check> {
    use crate::trace::{ModelRequestRecord, TerminationReason};

    let mut checks = Vec::new();
    let design =
        load_design().unwrap_or_else(|e| panic!("design must load for the deadline check: {e}"));
    let d = design.deadline;

    // 1. The registered deadline values, from design.json (the single
    // source of truth).
    checks.push(check_ok(
        "deadline contract: episode_deadline_ms = 600000",
        d.episode_deadline_ms == 600_000,
        format!("registered {}", d.episode_deadline_ms),
    ));
    checks.push(check_ok(
        "deadline contract: request_timeout_ceiling_ms = 600000",
        d.request_timeout_ceiling_ms == 600_000,
        format!("registered {}", d.request_timeout_ceiling_ms),
    ));
    checks.push(check_ok(
        "deadline contract: deadline_return_tolerance_ms = 1000",
        d.deadline_return_tolerance_ms == 1_000,
        format!("registered {}", d.deadline_return_tolerance_ms),
    ));

    // 2. No duplicate deadline constants in the model client module.
    let model_src = std::fs::read_to_string(crate_dir().join("src/model.rs")).unwrap_or_default();
    let no_duplicates = !model_src.contains("pub const REQUEST_TIMEOUT")
        && !model_src.contains("pub const EPISODE_TIME_LIMIT")
        && !model_src.contains("from_secs(600)");
    checks.push(check_ok(
        "no duplicate deadline Rust constants in src/model.rs",
        no_duplicates,
        "the 0012 REQUEST_TIMEOUT / EPISODE_TIME_LIMIT_MS constants are removed".to_string(),
    ));

    // 3. Mandatory request-level timing evidence fields exist.
    let rec_json = serde_json::to_value(&ModelRequestRecord {
        turn: 1,
        finish_reason: None,
        prompt_tokens: None,
        completion_tokens: None,
        total_tokens: None,
        cached_prompt_tokens: None,
        reasoning_tokens: None,
        uncached_prompt_tokens: None,
        request_started_elapsed_ms: 0,
        requested_timeout_ms: 0,
        request_finished_elapsed_ms: 0,
        timed_out: false,
        response_accepted: false,
    })
    .unwrap();
    let missing: Vec<&str> = [
        "request_started_elapsed_ms",
        "requested_timeout_ms",
        "request_finished_elapsed_ms",
        "timed_out",
        "response_accepted",
    ]
    .iter()
    .filter(|f| rec_json.get(f).is_none())
    .copied()
    .collect();
    checks.push(check_ok(
        "request timing fields present (start/timeout/finish/timed_out/response_accepted)",
        missing.is_empty(),
        format!("missing: {missing:?}"),
    ));

    // 4. The old 0012 aggregate wall-time integrity rule is removed.
    let trace_src = std::fs::read_to_string(crate_dir().join("src/trace.rs")).unwrap_or_default();
    checks.push(check_ok(
        "old aggregate wall-time > deadline verifier rule removed",
        !trace_src.contains("EPISODE_TIME_LIMIT_MS"),
        "the 0012 aggregate wall-time rule must not survive".to_string(),
    ));

    // 5. The §57 request-evidence cases (deterministic, network-free).
    let mk = |start: u64,
              timeout: u64,
              finish: u64,
              timed_out: bool,
              accepted: bool|
     -> ModelRequestRecord {
        ModelRequestRecord {
            turn: 1,
            finish_reason: None,
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            cached_prompt_tokens: None,
            reasoning_tokens: None,
            uncached_prompt_tokens: None,
            request_started_elapsed_ms: start,
            requested_timeout_ms: timeout,
            request_finished_elapsed_ms: finish,
            timed_out,
            response_accepted: accepted,
        }
    };
    let verify = |reqs: &[ModelRequestRecord], tr: TerminationReason| -> Vec<String> {
        crate::trace::deadline_evidence_violations(&design, "case", tr, reqs, &None, &[])
    };

    // (a) Valid normal request: PASS.
    let v = verify(
        &[mk(100, 1_000, 900, false, true)],
        TerminationReason::Completed,
    );
    checks.push(check_ok(
        "deadline case: valid normal request passes (start=100, timeout=1000, finish=900, accepted)",
        v.is_empty(),
        v.join("; "),
    ));
    // (b) Valid deadline-straddling timeout: PASS as infra, no violation.
    let v = verify(
        &[mk(599_000, 1_000, 600_002, true, false)],
        TerminationReason::HttpError,
    );
    checks.push(check_ok(
        "deadline case: valid straddling timeout is infra, no violation (599000/1000/600002)",
        v.is_empty(),
        v.join("; "),
    ));
    // (c) Valid late successful return, discarded: PASS (within tolerance).
    let v = verify(
        &[mk(599_000, 1_000, 600_100, false, false)],
        TerminationReason::EpisodeDeadlineExceeded,
    );
    checks.push(check_ok(
        "deadline case: late successful return (600100) discarded, no violation",
        v.is_empty(),
        v.join("; "),
    ));
    // (d) Invalid late start: FAIL.
    let v = verify(
        &[mk(600_000, 1_000, 600_000, false, false)],
        TerminationReason::HttpError,
    );
    checks.push(check_ok(
        "deadline case: request started at the deadline rejected (start=600000)",
        !v.is_empty() && v.iter().any(|x| x.contains("never START after")),
        v.join("; "),
    ));
    // (e) Invalid oversized timeout: FAIL.
    let v = verify(
        &[mk(590_000, 20_000, 600_000, false, true)],
        TerminationReason::Completed,
    );
    checks.push(check_ok(
        "deadline case: oversized timeout rejected (start=590000, timeout=20000 > remaining 10000)",
        !v.is_empty()
            && v.iter()
                .any(|x| x.contains("exceeds the remaining episode budget")),
        v.join("; "),
    ));
    // (f) Invalid return beyond tolerance: FAIL.
    let v = verify(
        &[mk(599_000, 1_000, 601_001, true, false)],
        TerminationReason::HttpError,
    );
    checks.push(check_ok(
        "deadline case: return beyond tolerance rejected (finish=601001)",
        !v.is_empty() && v.iter().any(|x| x.contains("beyond deadline")),
        v.join("; "),
    ));
    // (g) Invalid accepted late response: FAIL.
    let v = verify(
        &[mk(590_000, 10_000, 600_100, false, true)],
        TerminationReason::Completed,
    );
    checks.push(check_ok(
        "deadline case: accepted late response rejected (finish=600100, accepted=true)",
        !v.is_empty() && v.iter().any(|x| x.contains("must be discarded")),
        v.join("; "),
    ));
    // (h) Invalid subsequent request after a crossing: FAIL.
    let mut r2 = mk(600_002, 100, 600_100, false, true);
    r2.turn = 2;
    let v = verify(
        &[mk(599_000, 1_000, 600_002, true, false), r2],
        TerminationReason::Completed,
    );
    checks.push(check_ok(
        "deadline case: subsequent request after a crossing rejected",
        !v.is_empty() && v.iter().any(|x| x.contains("subsequent request")),
        v.join("; "),
    ));

    // 6. A late returned response cannot affect the trajectory (kernel
    // behavior, network-free): a model that returns just after a short
    // deadline is discarded — no assistant message, no tool call, no
    // final answer, no subsequent request.
    let late_check = {
        let mut tools = crate::tools::ToolExecutor::fresh(
            &serde_json::json!({"x": "EMPTY", "y": "B"}),
            crate::tools::FaultMode::Reliable,
        );
        let specs = crate::tools::ToolExecutor::specs();
        let cfg = crate::kernel::KernelConfig {
            max_model_turns: 12,
            max_tool_calls: 16,
            deadline: crate::trace::DesignDeadline {
                episode_deadline_ms: 10,
                request_timeout_ceiling_ms: 10,
                deadline_return_tolerance_ms: 1_000,
            },
        };
        let outcome = crate::kernel::run_episode(
            |_m: &[Message], _t: &[crate::protocol::ToolSpec], _to: u64| {
                std::thread::sleep(std::time::Duration::from_millis(40));
                let raw: Value = serde_json::from_str(
                    r#"{"choices":[{"message":{"role":"assistant","content":"LATE ANSWER"},"finish_reason":"stop"}]}"#,
                )
                .unwrap();
                Ok(crate::protocol::parse_response(&raw).unwrap())
            },
            &mut tools,
            &specs,
            "sys",
            "task",
            &cfg,
        );
        let q = outcome.model_requests.first().cloned().unwrap();
        outcome.termination_reason == TerminationReason::EpisodeDeadlineExceeded
            && outcome.final_answer.is_none()
            && outcome.model_turn_count == 0
            && outcome.model_requests.len() == 1
            && !q.response_accepted
            && outcome
                .conversation
                .iter()
                .all(|m| m.role != crate::protocol::Role::Assistant)
    };
    checks.push(check_ok(
        "kernel discards a late response (no assistant message / tool call / final answer)",
        late_check,
        "a response returning after the deadline must not influence the trajectory".to_string(),
    ));

    checks
}

/// Mutation-schema checks (the 0011 failure shapes, now frozen).
pub fn mutation_schema_checks() -> Vec<Check> {
    let design = load_design()
        .unwrap_or_else(|e| panic!("design must load for the mutation schema check: {e}"));
    let registry_file = load_tasks(&crate_dir().join("tasks.json"))
        .unwrap_or_else(|e| panic!("registry must load for the mutation schema check: {e}"));
    let registry = &registry_file.tasks;
    let incumbents = "line one\nline two\n";
    let good_suffixes = [
        "Always read the current value of a key before writing it, so actions follow observed state.",
        "After any action, check that the current external state matches the requested goal before completion.",
        "Verify the current external state before giving a final answer to the user.",
        "If the observed state does not match the requested goal, adjust the next action accordingly.",
    ];
    let mut checks = Vec::new();
    // (1) the documented shape is accepted.
    let body: Value = serde_json::json!({
        "candidates": good_suffixes
            .iter()
            .map(|s| serde_json::json!({"suffix": s, "rationale": "evidence"}))
            .collect::<Vec<_>>()
    });
    let (c, e) =
        mutation::validate_mutator_response(&design, &body.to_string(), incumbents, registry);
    checks.push(check_ok(
        "mutation schema: documented shape accepted",
        c.is_some() && e.is_empty(),
        format!("errors: {e:?}"),
    ));
    // (2) the 0011-r1 failure shape #1: wrong top-level key.
    let (c, e) = mutation::validate_mutator_response(
        &design,
        r#"{"suffixes":["a b c d e","f g h i j","k l m n o","p q r s t"]}"#,
        incumbents,
        registry,
    );
    checks.push(check_ok(
        "mutation schema: \"suffixes\" key rejected",
        c.is_none() && e.iter().any(|x| x.contains("\"candidates\"")),
        format!("errors: {e:?}"),
    ));
    // (3) the 0011-r1 failure shape #2: a list of bare strings.
    let (c, e) = mutation::validate_mutator_response(
        &design,
        r#"{"candidates":["a b c d e","f g h i j","k l m n o","p q r s t"]}"#,
        incumbents,
        registry,
    );
    checks.push(check_ok(
        "mutation schema: candidates array of strings rejected",
        c.is_none()
            && e.iter()
                .any(|x| x.contains("do not return candidate strings directly")),
        format!("errors: {e:?}"),
    ));
    // (4) wrong candidate count.
    let (c, e) = mutation::validate_mutator_response(
        &design,
        r#"{"candidates":[{"suffix":"a b c d e","rationale":"r"}]}"#,
        incumbents,
        registry,
    );
    checks.push(check_ok(
        "mutation schema: wrong candidate count rejected",
        c.is_none() && e.iter().any(|x| x.contains("expected exactly")),
        format!("errors: {e:?}"),
    ));
    // (5) the retry message carries the expected structural shape.
    let m = mutation::retry_message(&e);
    checks.push(check_ok(
        "retry message contains the expected structural shape",
        m.contains(mutation::EXPECTED_STRUCTURE)
            && m.starts_with("Your response was rejected for structural validation errors:"),
        m,
    ));
    checks
}

// ===========================================================================
// Preflight (network-free)
// ===========================================================================

pub fn cmd_preflight(manifest: Option<&Path>) -> Result<(), String> {
    println!("== Experiment 0013 preflight (network-free) ==");
    let mut fail = false;
    let mut report = |name: &str, passed: bool, detail: &str| {
        println!(
            "  [{}] {name} {detail}",
            if passed { "PASS" } else { "FAIL" }
        );
        if !passed {
            fail = true;
        }
    };

    // 1. Design manifest + arithmetic.
    match (load_design(), load_tasks(&crate_dir().join("tasks.json"))) {
        (Ok(design), Ok(file)) => {
            let registry = &file.tasks;
            let disc = design.discovery_episodes(registry);
            let cells = design.selection_cells(registry);
            let sel = design.selection_episodes(registry);
            let pairs = design.promotion_pairs(registry);
            let prom = design.promotion_episodes(registry);
            let n = design.selection.conditions.len() as u32;
            let sel_per_cell = cells / n; // condition × position balance
            let prom_per_cell = pairs / 2;
            let arithmetic = disc == 18
                && cells == 30
                && sel == 150
                && sel_per_cell == 6
                && pairs == 48
                && prom == 96
                && prom_per_cell == 24;
            report(
                "design loads + validates",
                true,
                &format!(
                    "experiment {} run {} model {} | discovery {} / selection {} (cells {}, 5 positions) / promotion {} (pairs {}, 2 positions) | temp {} / {} | turns {} calls {} | endpoint {} (registered)",
                    design.experiment_id,
                    design.run_id,
                    design.model,
                    disc,
                    sel,
                    cells,
                    pairs,
                    prom,
                    design.agent_temperature,
                    design.mutator_temperature,
                    design.max_model_turns,
                    design.max_tool_calls,
                    REGISTERED_ENDPOINT
                ),
            );
            report(
                "design arithmetic (18 / 30 cells / 150 / 6 per cell / 48 pairs / 96 / 24 per cell)",
                arithmetic,
                &format!(
                    "disc {disc}, cells {cells}, sel {sel} (per-cell {sel_per_cell}), pairs {pairs}, prom {prom} (per-cell {prom_per_cell})"
                ),
            );
            // The registered cyclic schedule exposes all five positions.
            let pos5 = (1..=design.selection_repetitions())
                .all(|rep| design.selection_condition_at(rep, 5).is_some());
            report(
                "selection fifth position exists for every rep",
                pos5,
                &format!("5 conditions: {:?}", design.selection.conditions),
            );
            let prom2 = design.promotion_condition_order(1, "C1")
                == ["G0".to_string(), "C1".to_string()]
                && design.promotion_condition_order(2, "C1")
                    == ["C1".to_string(), "G0".to_string()];
            report(
                "promotion two-position alternating schedule registered",
                prom2,
                "odd: G0→selected, even: selected→G0",
            );
            // 3. Explicit mutator JSON schema present in the frozen prompt.
            let mut_text = read_file_sha(&prompts_dir().join("mutator.md"))
                .map(|(t, _)| t)
                .unwrap_or_default();
            let schema = mut_text.contains("\"candidates\": [")
                && mut_text.contains("exactly four objects")
                && mut_text.contains("exactly the string fields suffix and rationale")
                && mut_text.contains("Do not return a list of strings.")
                && mut_text.contains("Return JSON only");
            report(
                "explicit mutator JSON schema present in the frozen prompt",
                schema,
                if schema {
                    "prompt documents the exact structure"
                } else {
                    "prompt lacks the explicit schema"
                },
            );
            // Fresh task split + V13 literal split-disjointness.
            let profile_bytes = std::fs::read(crate_dir().join("stress-profile.json"))
                .map_err(|e| e.to_string())?;
            let profile: Value = serde_json::from_slice(&profile_bytes)
                .map_err(|e| format!("stress profile invalid: {e}"))?;
            let mut reg_errs = Vec::new();
            trace::registry_ok(
                &trace::VerifierContext::new(
                    String::new(),
                    String::new(),
                    String::new(),
                    file.tasks.clone(),
                    profile,
                    design.clone(),
                ),
                &mut reg_errs,
            );
            let (e, s, p) = (
                registry
                    .iter()
                    .filter(|t| t.split == TaskSplit::Discovery)
                    .count(),
                registry
                    .iter()
                    .filter(|t| t.split == TaskSplit::Selection)
                    .count(),
                registry
                    .iter()
                    .filter(|t| t.split == TaskSplit::Promotion)
                    .count(),
            );
            let reg_report = if reg_errs.is_empty() {
                "clean".to_string()
            } else {
                reg_errs.join("; ")
            };
            report(
                "fresh task split (12 tasks: 3 discovery / 3 selection / 6 promotion, split-disjoint V13 literals)",
                e == 3 && s == 3 && p == 6 && reg_errs.is_empty(),
                &format!("{} {} {} — registry checks: {reg_report}", e, s, p),
            );
        }
        (Err(e), _) | (_, Err(e)) => {
            report("design loads + validates", false, &e);
        }
    }

    // 2. Candidate validator matches the documented schema + retry shape.
    for c in mutation_schema_checks() {
        report(c.name, c.passed, &c.detail);
    }
    // 4. Wire semantics.
    let c = parallel_tool_check();
    report(c.name, c.passed, &c.detail);
    // 5. CLI discipline (endpoint discipline at the binary level).
    for c in cli_discipline_checks() {
        report(c.name, c.passed, &c.detail);
    }
    // 5b. Deadline contract (the 0013 semantic delta).
    for c in deadline_contract_checks() {
        report(c.name, c.passed, &c.detail);
    }
    // 6. Source-freeze mechanism (temporary git repos).
    for c in source_freeze_checks() {
        report(c.name, c.passed, &c.detail);
    }
    // 7. Stage-freeze mechanism (temporary git repos).
    for c in stage_freeze_checks() {
        report(c.name, c.passed, &c.detail);
    }
    // 8. Stress profile (frozen 0009 profile).
    match load_stress_profile(&crate_dir().join("stress-profile.json")) {
        Ok(p) => report(
            "frozen 0009 stress profile intact (S2/2, S1/1, S2/2)",
            p.families.len() == 3,
            "no recalibration",
        ),
        Err(e) => report("frozen 0009 stress profile intact", false, &e),
    }

    // 9. Optional: the run manifest + full source freeze.
    if let Some(m) = manifest {
        match frozen_run(m) {
            Ok((run, mf)) => report(
                "run manifest + source freeze",
                true,
                &format!(
                    "run {} frozen under {} ({} files, frozen design {})",
                    run.run_id,
                    mf.code_under_test_commit,
                    mf.files.len(),
                    mf.frozen_design_sha256
                ),
            ),
            Err(e) => report("run manifest + source freeze", false, &e),
        }
    }

    if fail {
        Err("preflight: FAIL — one or more registered checks failed (see above)".into())
    } else {
        println!("preflight: PASS");
        Ok(())
    }
}

// ===========================================================================
// The complete network-free self-test
// ===========================================================================

fn self_test_pass(failures: &mut Vec<String>, name: &str, ok: bool, detail: &str) {
    let detail = if detail.is_empty() { "" } else { detail };
    println!("  [{}] {name} {detail}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        failures.push(format!("{name}: {detail}"));
    }
}

pub fn self_test() -> Result<(), String> {
    let mut failures: Vec<String> = Vec::new();
    println!("== 0013 self-test ==");

    // --- 1. Design manifest + arithmetic -------------------------------
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

    // --- 2. Frozen profile + prompts ------------------------------------
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

    // --- 3. Source-freeze guard (0010 failure modes) ---------------------
    println!("-- 3. source-freeze guard (temp git repos) --");
    for c in source_freeze_checks() {
        self_test_pass(&mut failures, c.name, c.passed, &c.detail);
    }

    // --- 4. Stage-freeze mechanism (0011 gap) ----------------------------
    println!("-- 4. stage-freeze mechanism (temp git repos) --");
    for c in stage_freeze_checks() {
        self_test_pass(&mut failures, c.name, c.passed, &c.detail);
    }

    // --- 5. Endpoint discipline + wire semantics --------------------------
    println!("-- 5. endpoint discipline + wire semantics --");
    for c in cli_discipline_checks() {
        self_test_pass(&mut failures, c.name, c.passed, &c.detail);
    }
    let c = parallel_tool_check();
    self_test_pass(&mut failures, c.name, c.passed, &c.detail);
    for c in deadline_contract_checks() {
        self_test_pass(&mut failures, c.name, c.passed, &c.detail);
    }

    // --- 6. Mutation schema (0011 failure shapes, frozen) -----------------
    println!("-- 6. mutation output schema --");
    for c in mutation_schema_checks() {
        self_test_pass(&mut failures, c.name, c.passed, &c.detail);
    }

    // --- 7. Schedules ------------------------------------------------------
    println!("-- 7. registered schedules --");
    if let Ok(design) = load_design() {
        let pos5 = (1..=design.selection_repetitions())
            .all(|rep| design.selection_condition_at(rep, 5).is_some());
        self_test_pass(
            &mut failures,
            "selection schedule has all five positions (150 episodes)",
            pos5 && design.selection.conditions.len() == 5,
            &format!("conditions: {:?}", design.selection.conditions),
        );
        // A schedule with only four conditions must reject position 5.
        let mut short = design.clone();
        short.selection.conditions = vec!["G0".into(), "C1".into(), "C2".into(), "C3".into()];
        self_test_pass(
            &mut failures,
            "selection schedule missing pos5 rejected",
            (1..=10).all(|rep| short.selection_condition_at(rep, 5).is_none()),
            "a 4-condition schedule returns None for position 5",
        );
        let prom2 = design.promotion_condition_order(1, "C1")
            == ["G0".to_string(), "C1".to_string()]
            && design.promotion_condition_order(2, "C1") == ["C1".to_string(), "G0".to_string()];
        self_test_pass(
            &mut failures,
            "promotion schedule has both positions (96 episodes)",
            prom2,
            "96 = 6 tasks x 8 reps x 2 conditions, balanced 24/24",
        );
    } else {
        self_test_pass(
            &mut failures,
            "design loads for schedule checks",
            false,
            "see above",
        );
    }

    // --- 8. Synthetic end-to-end + tamper suite ---------------------------
    println!("-- 8. synthetic end-to-end + tamper suite --");
    let synth = trace::build_synthetic_artifacts();
    let ctx = &synth.ctx;

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
        let mut iv = serde_json::to_value(&synth.mutation_input).unwrap();
        if let Some(eps) = iv
            .get_mut("discovery_episodes")
            .and_then(|v| v.as_array_mut())
        {
            if let Some(st) = eps.first_mut().and_then(|e| e.get_mut("final_state")) {
                if let Some(o) = st.as_object_mut() {
                    o.insert("x".into(), Value::String("V13_SA_0".into())); // selection literal
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
        // Unselected candidate in the promotion stage (multi-candidate
        // leakage): the synthetic winner is C2; a C3 record must be
        // rejected.
        let mut recs = serde_json::from_value::<Vec<PromotionRecord>>(
            serde_json::to_value(&synth.promotion).unwrap(),
        )
        .unwrap();
        if let Some(r) = recs.iter_mut().find(|r| r.condition != "G0") {
            r.condition = "C3".into();
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
            "unselected candidate promotion rejected",
            v.iter().any(|x| x.contains("unselected condition")),
            &v.first().cloned().unwrap_or_default(),
        );
        // Promotion schedule missing pos2: a third position is out of
        // the registered alternating schedule.
        let mut recs = serde_json::from_value::<Vec<PromotionRecord>>(
            serde_json::to_value(&synth.promotion).unwrap(),
        )
        .unwrap();
        if let Some(r) = recs.first_mut() {
            r.condition_position = 3;
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
            "promotion schedule missing pos2 rejected (out-of-schedule position)",
            v.iter().any(|x| x.contains("alternating order")),
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
