//! Experiment orchestration for Experiment 0009: the runner outside
//! the kernel.
//!
//! This module loads the two frozen prompts, the task registry
//! (`tasks.json`), and the stress registry (`stress-levels.json`),
//! verifies the pre-registered prompt invariants *before any model
//! call* (spec §6), executes the two-phase design through the kernel
//! (Phase A: baseline-only family-level calibration over two
//! development tasks per family; then a frozen 48-pair evaluation plan;
//! Phase B: held-out evaluation under the frozen selected stresses),
//! applies the oracle, writes artifacts, and hosts the network-free
//! `self-test`. The kernel knows none of it.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::calibration::EvaluationPlan;
use crate::kernel::{self, KernelConfig};
use crate::model::{ModelClient, redacted_endpoint};
use crate::oracle::{TaskSpec, TaskSplit};
use crate::protocol::{ToolSpec, sha256_bytes, sha256_hex};
use crate::tools::{FaultMode, RawStressLevel, StressLevel, StressRegistry, ToolExecutor};
use crate::trace::{self, CalibrationRecord, EVAL_EPISODES, EvaluationRecord, PLANNED_PAIRS};

/// The one-line robustness repair appended to the baseline to form
/// `repair` (spec §13). Frozen BEFORE calibration.
pub const REPAIR_MUTATION: &str = "After every successful state_write, read the same key. If the observed value differs from the value you intended to write, write it again and re-read it. Repeat until the value matches, but make at most five state_write attempts for that requested key.";

/// One experiment condition: registered name + full system prompt.
pub struct Condition {
    pub name: &'static str,
    pub prompt: String,
}

/// The two Phase B conditions (spec §37).
pub fn conditions(baseline: String, repair: String) -> Vec<Condition> {
    vec![
        Condition {
            name: "baseline",
            prompt: baseline,
        },
        Condition {
            name: "repair",
            prompt: repair,
        },
    ]
}

/// The on-disk task registry shape (`tasks.json`).
#[derive(Debug, Clone, Deserialize)]
pub struct TasksFile {
    pub families: Vec<FamilyFile>,
    pub tasks: Vec<TaskSpec>,
}

/// One registered family (spec §15): two development + two held-out
/// tasks.
#[derive(Debug, Clone, Deserialize)]
pub struct FamilyFile {
    pub id: String,
    pub development: Vec<String>,
    pub heldout: Vec<String>,
}

/// Load and structurally validate the task registry.
pub fn load_tasks(path: &Path) -> Result<TasksFile, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read tasks file {}: {e}", path.display()))?;
    let parsed: Value =
        serde_json::from_str(&raw).map_err(|e| format!("tasks file is not valid JSON: {e}"))?;
    let file: TasksFile =
        serde_json::from_value(parsed).map_err(|e| format!("invalid tasks file: {e}"))?;

    // Registered design: exactly three families, two development +
    // two held-out tasks each (spec §15).
    if file.families.len() != trace::FAMILIES.len() {
        return Err(format!(
            "registry must contain {} families, found {}",
            trace::FAMILIES.len(),
            file.families.len()
        ));
    }
    for (i, f) in file.families.iter().enumerate() {
        if trace::FAMILIES[i] != f.id {
            return Err(format!(
                "family registry order mismatch: expected {}, found {}",
                trace::FAMILIES[i],
                f.id
            ));
        }
        if f.development.len() != trace::DEVELOPMENT_TASKS_PER_FAMILY
            || f.heldout.len() != trace::HELDOUT_TASKS_PER_FAMILY
        {
            return Err(format!(
                "family {} must register exactly {} development + {} held-out tasks",
                f.id,
                trace::DEVELOPMENT_TASKS_PER_FAMILY,
                trace::HELDOUT_TASKS_PER_FAMILY
            ));
        }
    }
    if file.tasks.len() != 12 {
        return Err(format!(
            "registry must contain 12 tasks, found {}",
            file.tasks.len()
        ));
    }
    let ids: Vec<&str> = file.tasks.iter().map(|t| t.id.as_str()).collect();
    let expected: Vec<&str> = vec![
        "D1", "D2", "H1", "H2", "D3", "D4", "H3", "H4", "D5", "D6", "H5", "H6",
    ];
    if ids != expected {
        return Err(format!(
            "task registry must contain exactly {expected:?} in order, found {ids:?}"
        ));
    }
    // Each family's declared task ids must exist and match.
    for f in &file.families {
        for id in f.development.iter().chain(f.heldout.iter()) {
            let t = file
                .tasks
                .iter()
                .find(|t| t.id == *id)
                .ok_or_else(|| format!("family {} declares unknown task {id}", f.id))?;
            if t.family != f.id {
                return Err(format!("task {id} belongs to {}, not {}", t.family, f.id));
            }
            if !crate::tools::StateKey::parse(&t.fault_key).is_some() {
                return Err(format!("task {id} has invalid fault_key {:?}", t.fault_key));
            }
        }
    }
    for t in &file.tasks {
        let want_split = if t.id.starts_with('D') {
            TaskSplit::Development
        } else {
            TaskSplit::HeldOut
        };
        if t.split != want_split {
            return Err(format!(
                "task {} has split {:?}, expected {want_split:?}",
                t.id, t.split
            ));
        }
    }
    Ok(file)
}

/// Load the stress registry (`stress-levels.json`).
pub fn load_stress_registry(path: &Path) -> Result<StressRegistry, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read stress registry {}: {e}", path.display()))?;
    let registry: StressRegistry =
        serde_json::from_str(&raw).map_err(|e| format!("stress registry is not valid: {e}"))?;
    if registry.levels.len() != 5 {
        return Err(format!(
            "stress registry must contain 5 levels, found {}",
            registry.levels.len()
        ));
    }
    for (i, raw_level) in registry.levels.iter().enumerate() {
        let registered = StressLevel::by_index(i).ok_or("registered stress ladder is shorter")?;
        if !registered.matches_registry(raw_level) {
            return Err(format!(
                "stress registry level {} does not match the registered ladder ({registered:?} != {raw_level:?})",
                i + 1
            ));
        }
    }
    Ok(registry)
}

/// Read a prompt file as text.
pub fn load_prompt(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read prompt {}: {e}", path.display()))
}

/// Programmatic prompt invariant (spec §13): the repair candidate must
/// equal the baseline plus exactly the registered mutation line. Any
/// other difference is a configuration error and the experiment is not
/// run (calibration must not be used to tune the prompt).
pub fn verify_repair_invariant(baseline: &str, repair: &str) -> Result<(), String> {
    let b = baseline.trim_end_matches('\n');
    let c = repair.trim_end_matches('\n');
    if c != format!("{b}\n{REPAIR_MUTATION}") {
        Err("repair is not exactly baseline + the registered repair mutation".to_string())
    } else {
        Ok(())
    }
}

/// SHA-256 of a file's bytes.
pub fn hash_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Ok(sha256_bytes(&bytes))
}

/// Canonical (key-order independent) SHA-256 of the loaded task
/// registry. Hashes the parsed task list; the families block of the
/// file is validated structurally by `load_tasks` (its content is
/// derivable from the tasks).
pub fn task_registry_sha256(registry: &[TaskSpec]) -> Result<String, String> {
    let v: Value = serde_json::to_value(serde_json::json!({ "tasks": registry }))
        .map_err(|e| format!("task serialization: {e}"))?;
    Ok(sha256_hex(&v))
}

/// Canonical SHA-256 of the loaded stress registry.
pub fn stress_registry_sha256(levels: &[RawStressLevel]) -> Result<String, String> {
    let registry = StressRegistry {
        levels: levels.to_vec(),
    };
    let v: Value =
        serde_json::to_value(&registry).map_err(|e| format!("stress serialization: {e}"))?;
    Ok(sha256_hex(&v))
}

/// The experiment crate's directory (prompts/, tasks.json, and
/// stress-levels.json live here).
pub fn crate_dir() -> PathBuf {
    std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("experiments/0009-power-aware-evaluation"))
}

/// Environment configuration for a real run. Missing required variables
/// mean the experiment is NOT executed; no fallback model is substituted.
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

/// Resolve the code commit recorded in records: the explicit
/// `--code-commit` flag wins; otherwise the current `git rev-parse HEAD`
/// is used. The value must be a 40-hex SHA.
fn resolve_code_commit(explicit: Option<&str>) -> Result<String, String> {
    let raw = match explicit {
        Some(s) => s.to_string(),
        None => {
            let output = std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .map_err(|e| format!("cannot run git rev-parse: {e}"))?;
            if !output.status.success() {
                return Err("--code-commit not given and `git rev-parse HEAD` failed".to_string());
            }
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
    };
    if raw.len() != 40 || !raw.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("code commit must be a 40-hex git SHA, got {raw:?}"));
    }
    Ok(raw)
}

/// Full frozen-configuration load shared by both phases.
struct FrozenConfig {
    baseline: String,
    repair: String,
    baseline_hash: String,
    repair_hash: String,
    tasks: Vec<TaskSpec>,
    task_registry_sha256: String,
    stress_registry_sha256: String,
}

/// Load, validate, and hash every frozen input. This runs BEFORE any
/// model request in either phase (spec §6): prompts, tasks, stress
/// ladder, and the repair prompt invariant must all be committed and
/// stable before Phase A starts.
fn load_frozen_config() -> Result<FrozenConfig, String> {
    let dir = crate_dir();
    let baseline = load_prompt(&dir.join("prompts/baseline.md"))?;
    let repair = load_prompt(&dir.join("prompts/repair.md"))?;
    verify_repair_invariant(&baseline, &repair)
        .map_err(|e| format!("configuration error, experiment not run: {e}"))?;

    let tasks_file = load_tasks(&dir.join("tasks.json"))?;
    let stress = load_stress_registry(&dir.join("stress-levels.json"))?;
    Ok(FrozenConfig {
        baseline_hash: hash_file(&dir.join("prompts/baseline.md"))?,
        repair_hash: hash_file(&dir.join("prompts/repair.md"))?,
        baseline,
        repair,
        task_registry_sha256: task_registry_sha256(&tasks_file.tasks)?,
        stress_registry_sha256: stress_registry_sha256(&stress.levels)?,
        tasks: tasks_file.tasks,
    })
}

// ===========================================================================
// Phase A: baseline-only family-level calibration
// ===========================================================================

pub struct CalibrateConfig {
    pub trajectories: PathBuf,
    pub summary: PathBuf,
    pub plan: PathBuf,
    /// Frozen code-under-test commit (40-hex); falls back to the
    /// current `git rev-parse HEAD`.
    pub code_commit: Option<String>,
    /// Optional explicit redacted endpoint; falls back to the (redacted)
    /// configured base URL when absent.
    pub endpoint: Option<String>,
}

/// Execute Phase A (spec §19): 150 baseline-only episodes over
/// 3 families × 2 development tasks × 5 stress levels × 5 repetitions,
/// then the pure family-level selection rule, then the frozen evaluation
/// plan (`evaluation-plan.json`). The repair candidate is never
/// executed.
pub fn calibrate(config: &CalibrateConfig) -> Result<(), String> {
    let (base_url, model, api_key) = env_config()?;
    let cfg = load_frozen_config()?;
    let commit = resolve_code_commit(config.code_commit.as_deref())?;
    let endpoint = config
        .endpoint
        .clone()
        .unwrap_or_else(|| redacted_endpoint(&base_url));

    let client = ModelClient::new(base_url, model.clone(), api_key);
    let specs: Vec<ToolSpec> = ToolExecutor::specs();
    let kernel_config = KernelConfig::default();

    for parent in [
        config.trajectories.parent(),
        config.summary.parent(),
        config.plan.parent(),
    ]
    .into_iter()
    .flatten()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(&config.trajectories)
            .map_err(|e| format!("cannot create trajectories file: {e}"))?,
    );

    let run_prefix = format!(
        "exp0009-cal-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );

    let mut records = Vec::new();
    let mut counter = 0usize;
    let total = trace::CALIBRATION_EPISODES;

    println!(
        "starting Phase A calibration: model={model} endpoint={endpoint} episodes={total} code_commit={commit} run_prefix={run_prefix}"
    );

    // Families in registry order; both development tasks per family;
    // the registered cyclic stress order inside each
    // (repetition, execution_position) cell.
    for family in trace::FAMILIES.iter() {
        let dev_tasks: Vec<TaskSpec> = cfg
            .tasks
            .iter()
            .filter(|t| t.family == *family && t.split == TaskSplit::Development)
            .cloned()
            .collect();
        if dev_tasks.len() != trace::DEVELOPMENT_TASKS_PER_FAMILY {
            return Err(format!(
                "family {family} must have exactly {} registered development tasks",
                trace::DEVELOPMENT_TASKS_PER_FAMILY
            ));
        }
        for cal_task in &dev_tasks {
            for rep in 1..=trace::CALIBRATION_REPETITIONS {
                for pos in 1..=trace::CALIBRATION_STRESS_LEVELS {
                    counter += 1;
                    let level = crate::tools::STRESS_LEVELS
                        [trace::calibration_stress_index(rep, pos) as usize];

                    eprintln!(
                        "[{}/{}] {} {} {} rep{} pos{}",
                        counter, total, family, cal_task.id, level.id, rep, pos
                    );

                    // Fresh tool executor per episode; fault from (key, stress).
                    let fault = FaultMode::for_stress(&cal_task.fault_key, level.drop_count);
                    let mut tools = ToolExecutor::fresh(&cal_task.initial_state, fault.clone());
                    let outcome = kernel::run_episode(
                        |messages: &[crate::protocol::Message], specs: &[ToolSpec]| {
                            client.complete(messages, specs)
                        },
                        &mut tools,
                        &specs,
                        &cfg.baseline, // Phase A is baseline-only.
                        &cal_task.prompt,
                        &kernel_config,
                    );
                    let c = kernel::core(outcome);
                    let eval = crate::oracle::evaluate_task(
                        cal_task,
                        &crate::oracle::OracleInput {
                            termination_reason: c.termination_reason.clone(),
                            initial_state: cal_task.initial_state.clone(),
                            final_state: c.final_state.clone(),
                        },
                    );

                    let record = CalibrationRecord {
                        experiment_id: trace::EXPERIMENT_ID.to_string(),
                        phase: "calibration".to_string(),
                        run_id: format!("{run_prefix}-{:03}", counter),
                        sequence: counter as u32,
                        family: family.to_string(),
                        task_id: cal_task.id.clone(),
                        stress_level: level.id.to_string(),
                        drop_count: level.drop_count,
                        repetition: rep,
                        execution_position: pos,
                        condition: "baseline".to_string(),
                        model: model.clone(),
                        temperature: crate::protocol::TEMPERATURE,
                        redacted_endpoint: endpoint.clone(),
                        code_under_test_commit: commit.clone(),
                        baseline_prompt_sha256: cfg.baseline_hash.clone(),
                        repair_prompt_sha256: cfg.repair_hash.clone(),
                        task_registry_sha256: cfg.task_registry_sha256.clone(),
                        stress_registry_sha256: cfg.stress_registry_sha256.clone(),
                        initial_state: cal_task.initial_state.clone(),
                        target_state: cal_task.target_state.clone(),
                        fault_mode: serde_json::to_value(&fault).unwrap_or(Value::Null),
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
                    records.push(record.clone());

                    let line = serde_json::to_string(&record)
                        .map_err(|e| format!("record serialization: {e}"))?;
                    out.write_all(line.as_bytes())
                        .map_err(|e| format!("write failure: {e}"))?;
                    out.write_all(b"\n")
                        .map_err(|e| format!("write failure: {e}"))?;
                    out.flush().map_err(|e| format!("flush failure: {e}"))?;

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
        }
    }
    out.flush().map_err(|e| format!("final flush: {e}"))?;

    if records.len() != total {
        return Err(format!(
            "calibration produced {} episodes, expected {total} — the artifact is incomplete",
            records.len()
        ));
    }

    let summary = trace::compute_calibration_summary(&records, &cfg.tasks);
    let summary_json = serde_json::to_string_pretty(&summary)
        .map_err(|e| format!("summary serialization: {e}"))?;
    std::fs::write(&config.summary, summary_json + "\n")
        .map_err(|e| format!("cannot write summary: {e}"))?;

    // The frozen evaluation plan (spec §45). Its raw-artifact hash is
    // the byte hash of the immutable calibration trajectories file.
    let raw_bytes = std::fs::read(&config.trajectories)
        .map_err(|e| format!("cannot re-read trajectories for hashing: {e}"))?;
    let raw_sha = sha256_bytes(&raw_bytes);
    let mut plan = trace::recompute_evaluation_plan(
        &records,
        &cfg.tasks,
        &trace::PlanProvenance {
            code_under_test_commit: commit.clone(),
            baseline_prompt_sha256: cfg.baseline_hash.clone(),
            repair_prompt_sha256: cfg.repair_hash.clone(),
            task_registry_sha256: cfg.task_registry_sha256.clone(),
            stress_registry_sha256: cfg.stress_registry_sha256.clone(),
        },
    );
    plan.calibration_raw_sha256 = raw_sha.clone();
    let plan_json =
        serde_json::to_string_pretty(&plan).map_err(|e| format!("plan serialization: {e}"))?;
    std::fs::write(&config.plan, plan_json + "\n")
        .map_err(|e| format!("cannot write evaluation plan: {e}"))?;

    println!(
        "Phase A complete: {} episodes; {} of {} families calibrated; planned_pairs={}; repetitions_per_task={}; proceed_to_evaluation={}",
        summary.episodes_total,
        summary.calibrated_family_count,
        trace::FAMILIES.len(),
        summary.planned_pairs,
        summary.repetitions_per_task,
        summary.proceed_to_evaluation
    );
    println!("calibration raw SHA-256: {raw_sha}");
    println!("  plan:    {}", config.plan.display());
    println!("  summary: {}", config.summary.display());
    if !summary.proceed_to_evaluation {
        println!(
            "STOP: fewer than {} families calibrated — the experiment conclusion is \
             INCONCLUSIVE; do NOT run Phase B and do NOT tune anything.",
            trace::MIN_CALIBRATED_FAMILIES
        );
    }
    Ok(())
}

// ===========================================================================
// Phase B: held-out evaluation
// ===========================================================================

pub struct EvaluateConfig {
    pub plan: PathBuf,
    pub trajectories: PathBuf,
    pub summary: PathBuf,
    /// Final-evaluation execution commit (40-hex); falls back to the
    /// current `git rev-parse HEAD`.
    pub code_commit: Option<String>,
    /// Optional explicit redacted endpoint; falls back to the (redacted)
    /// configured base URL when absent.
    pub endpoint: Option<String>,
}

/// Load the frozen evaluation plan (committed before Phase B).
fn load_plan(path: &Path) -> Result<EvaluationPlan, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read evaluation plan {}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("evaluation plan is not valid JSON: {e}"))
}

/// Execute Phase B (spec §26–28, §65): baseline vs repair on every
/// selected family's held-out tasks at its frozen selected stress —
/// always exactly 48 pairs / 96 episodes, regardless of whether two
/// or three families calibrated.
pub fn evaluate(config: &EvaluateConfig) -> Result<(), String> {
    let (base_url, model, api_key) = env_config()?;
    let cfg = load_frozen_config()?;
    let plan = load_plan(&config.plan)?;

    // The plan must authorize Phase B (spec §25).
    if !plan.proceed_to_evaluation {
        return Err("the frozen evaluation plan does not authorize Phase B \
             (proceed_to_evaluation != true) — the experiment stopped after calibration"
            .to_string());
    }
    let calibrated = plan.calibrated_family_count;
    let reps = plan.repetitions_per_task;
    if trace::eval_repetitions_for(calibrated) != Some(reps)
        || reps == 0
        || plan.heldout_tasks.len() as u32 * reps != PLANNED_PAIRS
    {
        return Err(format!(
            "the frozen evaluation plan is internally inconsistent ({} calibrated families, {} reps, {} held-out tasks)",
            calibrated,
            reps,
            plan.heldout_tasks.len()
        ));
    }

    // No-tuning check (spec §6): the prompts on disk must still be the
    // exact pre-calibration hashes frozen in the plan.
    if cfg.repair_hash != plan.repair_prompt_sha256 {
        return Err(
            "the repair prompt on disk does not hash to the pre-calibration value \
             frozen in the evaluation plan — STOP, the candidate was tuned after calibration"
                .to_string(),
        );
    }
    if cfg.baseline_hash != plan.baseline_prompt_sha256 {
        return Err(
            "the baseline prompt on disk does not hash to the value frozen in the \
             evaluation plan — STOP"
                .to_string(),
        );
    }
    if cfg.task_registry_sha256 != plan.task_registry_sha256 {
        return Err(
            "the task registry on disk does not hash to the value frozen in the \
             evaluation plan — STOP"
                .to_string(),
        );
    }
    if plan.selected_families.len() as u32 != calibrated
        || calibrated < trace::MIN_CALIBRATED_FAMILIES
    {
        return Err(format!(
            "evaluation plan lists {} selected families / calibrated_family_count {calibrated}; the registered design requires ≥ {}",
            plan.selected_families.len(),
            trace::MIN_CALIBRATED_FAMILIES
        ));
    }

    let commit = resolve_code_commit(config.code_commit.as_deref())?; // final-evaluation execution commit
    let endpoint = config
        .endpoint
        .clone()
        .unwrap_or_else(|| redacted_endpoint(&base_url));

    let plan_bytes = std::fs::read(&config.plan)
        .map_err(|e| format!("cannot read evaluation plan for hashing: {e}"))?;
    let plan_sha = sha256_bytes(&plan_bytes);

    if let Some(parent) = config.trajectories.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(&config.trajectories)
            .map_err(|e| format!("cannot create trajectories file: {e}"))?,
    );

    let client = ModelClient::new(base_url, model.clone(), api_key);
    let specs: Vec<ToolSpec> = ToolExecutor::specs();
    let kernel_config = KernelConfig::default();
    let conds = conditions(cfg.baseline.clone(), cfg.repair.clone());

    let run_prefix = format!(
        "exp0009-eval-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );

    let mut records = Vec::new();
    let mut counter = 0usize;
    let expected = EVAL_EPISODES;

    println!(
        "starting Phase B evaluation: model={model} endpoint={endpoint} episodes={expected} pairs={PLANNED_PAIRS} code_commit={commit} run_prefix={run_prefix} selected_families={:?} reps/task={reps}",
        plan.selected_families
    );

    for family in trace::FAMILIES.iter() {
        if !plan.selected_families.iter().any(|f| f == family) {
            continue;
        }
        let selected_stress = plan
            .per_family
            .iter()
            .find(|f| f.family == *family)
            .and_then(|f| f.selected_stress.clone())
            .ok_or_else(|| format!("family {family} is selected but has no selected stress"))?;
        let level = StressLevel::by_id(&selected_stress).ok_or_else(|| {
            format!("selected stress {selected_stress} is not a registered level")
        })?;
        if level.drop_count == 0 {
            return Err(format!(
                "selected stress {selected_stress} is S0 (the sanity control) — S0 may not be a final evaluation stress"
            ));
        }
        for task in cfg
            .tasks
            .iter()
            .filter(|t| t.family == *family && t.split == TaskSplit::HeldOut)
        {
            for rep in 1..=reps {
                for (pos, condition_name) in trace::eval_condition_order(rep).iter().enumerate() {
                    counter += 1;
                    let cond = conds
                        .iter()
                        .find(|c| c.name == *condition_name)
                        .expect("registered condition");
                    let fault = FaultMode::for_stress(&task.fault_key, level.drop_count);

                    eprintln!(
                        "[{}/{}] {} {} {} rep{} pos{}",
                        counter,
                        expected,
                        family,
                        task.id,
                        condition_name,
                        rep,
                        pos + 1
                    );

                    let mut tools = ToolExecutor::fresh(&task.initial_state, fault.clone());
                    let outcome = kernel::run_episode(
                        |messages: &[crate::protocol::Message], specs: &[ToolSpec]| {
                            client.complete(messages, specs)
                        },
                        &mut tools,
                        &specs,
                        &cond.prompt,
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

                    let record = EvaluationRecord {
                        experiment_id: trace::EXPERIMENT_ID.to_string(),
                        phase: "evaluation".to_string(),
                        run_id: format!("{run_prefix}-{:03}", counter),
                        sequence: counter as u32,
                        family: family.to_string(),
                        task_id: task.id.clone(),
                        selected_stress: level.id.to_string(),
                        drop_count: level.drop_count,
                        condition: condition_name.to_string(),
                        repetition: rep,
                        condition_position: (pos + 1) as u32,
                        calibration_raw_sha256: plan.calibration_raw_sha256.clone(),
                        evaluation_plan_sha256: plan_sha.clone(),
                        model: model.clone(),
                        temperature: crate::protocol::TEMPERATURE,
                        redacted_endpoint: endpoint.clone(),
                        final_eval_commit: commit.clone(),
                        baseline_prompt_sha256: cfg.baseline_hash.clone(),
                        repair_prompt_sha256: cfg.repair_hash.clone(),
                        task_registry_sha256: cfg.task_registry_sha256.clone(),
                        initial_state: task.initial_state.clone(),
                        target_state: task.target_state.clone(),
                        fault_mode: serde_json::to_value(&fault).unwrap_or(Value::Null),
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
                    records.push(record.clone());

                    let line = serde_json::to_string(&record)
                        .map_err(|e| format!("record serialization: {e}"))?;
                    out.write_all(line.as_bytes())
                        .map_err(|e| format!("write failure: {e}"))?;
                    out.write_all(b"\n")
                        .map_err(|e| format!("write failure: {e}"))?;
                    out.flush().map_err(|e| format!("flush failure: {e}"))?;

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
        }
    }
    out.flush().map_err(|e| format!("final flush: {e}"))?;

    if records.len() != expected {
        return Err(format!(
            "evaluation produced {} episodes, expected {expected} — the artifact is incomplete",
            records.len()
        ));
    }

    let summary = trace::compute_evaluation_summary(&records, &cfg.tasks, &plan);
    let summary_json = serde_json::to_string_pretty(&summary)
        .map_err(|e| format!("summary serialization: {e}"))?;
    if let Some(parent) = config.summary.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(&config.summary, summary_json + "\n")
        .map_err(|e| format!("cannot write summary: {e}"))?;

    println!(
        "Phase B complete: {} episodes, {} oracle successes, {} infra failures, \
         potential_info={} ({}), valid_pairs={}, informative_pairs={}, \
         wins={}/losses={}/ties={}, p={}, classification={}, conclusion={}",
        summary.episodes_total,
        summary.oracle_successes,
        summary.infrastructure_failures,
        summary.potential_information.potential_information_capacity,
        summary.potential_information.sufficient,
        summary.comparison.valid_pairs,
        summary.comparison.informative_pairs,
        summary.comparison.wins,
        summary.comparison.losses,
        summary.comparison.ties,
        summary.comparison.sign_test_p,
        summary.comparison.classification,
        summary.conclusion.result
    );
    println!("trajectories: {}", config.trajectories.display());
    println!("summary:     {}", config.summary.display());
    Ok(())
}

// ===========================================================================
// Network-free self-test (spec §58)
// ===========================================================================

/// Network-free self-test. Runs every component check and BOTH full
/// verifiers against deterministic synthetic datasets. Makes no model
/// calls.
pub fn self_test() -> Result<(), String> {
    struct Check {
        name: &'static str,
        ok: bool,
        detail: String,
    }
    let mut checks: Vec<Check> = Vec::new();
    let mut add = |name: &'static str, ok: bool, detail: String| {
        checks.push(Check { name, ok, detail });
    };

    let dir = crate_dir();

    // 1. Prompt invariants (real committed files; repair frozen before
    // calibration, spec §6/§13).
    match (
        load_prompt(&dir.join("prompts/baseline.md")),
        load_prompt(&dir.join("prompts/repair.md")),
    ) {
        (Ok(b), Ok(r)) => {
            let res = verify_repair_invariant(&b, &r);
            add(
                "prompt invariants",
                res.is_ok(),
                match res {
                    Ok(()) => "repair == baseline + exactly the registered mutation (frozen before calibration)".to_string(),
                    Err(e) => e,
                },
            );
        }
        _ => add(
            "prompt invariants",
            false,
            "prompt files missing".to_string(),
        ),
    }

    // 2. Twelve-task registry with the registered families.
    match load_tasks(&dir.join("tasks.json")) {
        Ok(file) => {
            let dev: usize = file
                .tasks
                .iter()
                .filter(|t| t.split == TaskSplit::Development)
                .count();
            let held: usize = file
                .tasks
                .iter()
                .filter(|t| t.split == TaskSplit::HeldOut)
                .count();
            add(
                "12-task registry (3 families)",
                file.tasks.len() == 12 && dev == 6 && held == 6,
                format!(
                    "{} tasks ({} development / {} held-out), families {:?}",
                    file.tasks.len(),
                    dev,
                    held,
                    file.families
                        .iter()
                        .map(|f| f.id.as_str())
                        .collect::<Vec<_>>()
                ),
            );
        }
        Err(e) => add("12-task registry (3 families)", false, e),
    }

    // 3. Stress ladder registry.
    match load_stress_registry(&dir.join("stress-levels.json")) {
        Ok(reg) => {
            let counts: Vec<u32> = reg.levels.iter().map(|l| l.drop_count).collect();
            add(
                "stress ladder S0..S4",
                counts == vec![0, 1, 2, 3, 4],
                format!("registered drop counts {counts:?}"),
            );
        }
        Err(e) => add("stress ladder S0..S4", false, e),
    }

    // 4. DropFirstNWrites fault semantics.
    {
        let mut exec = ToolExecutor::fresh(
            &serde_json::json!({"x": "EMPTY", "y": "B"}),
            FaultMode::DropFirstNWrites {
                key: "x".into(),
                count: 2,
            },
        );
        exec.execute(
            "state_write",
            &serde_json::json!({"key": "x", "value": "A"}),
        )
        .unwrap();
        let after_first = exec.final_state_value()["x"].to_string();
        exec.execute(
            "state_write",
            &serde_json::json!({"key": "x", "value": "A"}),
        )
        .unwrap();
        let after_second = exec.final_state_value()["x"].to_string();
        exec.execute(
            "state_write",
            &serde_json::json!({"key": "x", "value": "A"}),
        )
        .unwrap();
        let after_third = exec.final_state_value()["x"].to_string();
        add(
            "DropFirstNWrites fault semantics",
            after_first == "\"EMPTY\""
                && after_second == "\"EMPTY\""
                && after_third == "\"A\""
                && !exec.write_attempts()[0].applied
                && !exec.write_attempts()[1].applied
                && exec.write_attempts()[2].applied
                && exec.write_attempts()[0].registered_drop_index == Some(1)
                && exec.write_attempts()[1].registered_drop_index == Some(2)
                && exec.write_attempts()[2].registered_drop_index == Some(3),
            "first N target writes silently dropped and audited with ordinal indices; later writes apply".to_string(),
        );
    }

    // 5. Oracle independence + pass/fail.
    {
        let task = TaskSpec {
            id: "H1".into(),
            name: "heldout_direct_set_x".into(),
            family: "direct_set".into(),
            split: TaskSplit::HeldOut,
            prompt: "Set x to R.".into(),
            initial_state: serde_json::json!({"x": "Q", "y": "B"}),
            target_state: serde_json::json!({"x": "R", "y": "B"}),
            fault_key: "x".into(),
        };
        let pass = crate::oracle::evaluate_task(
            &task,
            &crate::oracle::OracleInput {
                termination_reason: "completed".into(),
                initial_state: serde_json::json!({"x": "Q", "y": "B"}),
                final_state: serde_json::json!({"x": "R", "y": "B"}),
            },
        );
        let fail_state = crate::oracle::evaluate_task(
            &task,
            &crate::oracle::OracleInput {
                termination_reason: "completed".into(),
                initial_state: serde_json::json!({"x": "Q", "y": "B"}),
                final_state: serde_json::json!({"x": "Q", "y": "B"}),
            },
        );
        let fail_protocol = crate::oracle::evaluate_task(
            &task,
            &crate::oracle::OracleInput {
                termination_reason: "turn_limit".into(),
                initial_state: serde_json::json!({"x": "Q", "y": "B"}),
                final_state: serde_json::json!({"x": "R", "y": "B"}),
            },
        );
        add(
            "oracle independence + pass/fail",
            pass.success && !fail_state.success && !fail_protocol.success,
            format!(
                "correct-state+completed = pass; wrong-state = fail; correct-state+protocol-failure = fail ({} reasons)",
                fail_protocol.reasons.len()
            ),
        );
    }

    // 6. Sign test + classification + power.
    {
        let p73 = crate::stats::exact_sign_test_p(7, 3);
        add(
            "exact sign test",
            (p73 - 0.34375).abs() < 1e-12 && crate::stats::exact_sign_test_p(0, 0) == 1.0,
            format!("p(7 wins, 3 losses) = {p73}"),
        );
        let a = crate::stats::classify_quality(14, 2);
        let b = crate::stats::classify_quality(2, 14);
        add(
            "quality classification",
            a == crate::stats::QualityClass::Improvement
                && b == crate::stats::QualityClass::Regression
                && crate::stats::classify_quality(8, 8) == crate::stats::QualityClass::Inconclusive,
            format!("(14,2)={a:?}, (2,14)={b:?}, (8,8)=inconclusive"),
        );
        let power_at_min = crate::power::conditional_detection_power(12, 0.90);
        add(
            "power design constants",
            power_at_min >= crate::power::MIN_CONDITIONAL_DETECTION_POWER,
            format!(
                "Power(12 informative, θ=0.90) = {power_at_min:.6} (minimum {})",
                crate::power::MIN_CONDITIONAL_DETECTION_POWER
            ),
        );
    }

    // 7. Family-level two-variant selection rule (pure): BOTH
    // development variants must be within the ≤40% headroom at the same
    // stress. D1 is 2/5 at S1, D2 is 3/5 at S1 → S1 ineligible; the
    // rule must therefore fall through (to S2 at 2/2 in this pattern)
    // rather than calibrating on a single task.
    {
        let mut outcomes = Vec::new();
        for (dc, level) in crate::tools::STRESS_LEVELS.iter().enumerate() {
            for rep in 1..=5 {
                for (task, succ_at_s1, succ_at_s2) in [("D1", 2u32, 2u32), ("D2", 3u32, 2u32)] {
                    let count = match dc {
                        0 => 5, // S0: 5/5 on both variants
                        1 => succ_at_s1,
                        2 => succ_at_s2,
                        _ => 0,
                    };
                    outcomes.push(crate::calibration::BaselineDevelopmentOutcome {
                        task_id: task.into(),
                        stress_id: level.id.to_string(),
                        repetition: rep,
                        oracle_success: rep <= count,
                    });
                }
            }
        }
        let sel = crate::calibration::select_family_stress(
            "direct_set",
            &["D1".to_string(), "D2".to_string()],
            &outcomes,
        );
        add(
            "family-level selection rule (two variants, ≤40% headroom)",
            sel.status == crate::calibration::SelectionStatus::Calibrated
                && sel.selected_stress.as_deref() == Some("S2"),
            format!(
                "D1 2/5 + D2 3/5 at S1 is NOT eligible (both variants must be ≤ 2/5); selected {:?} ({})",
                sel.selected_stress, sel.selection_reason
            ),
        );
    }

    // 8. Usage parsing.
    {
        use crate::protocol::parse_response;
        let with: Value = serde_json::from_str(
            r#"{"choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],
                 "usage":{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105,
                          "prompt_tokens_details":{"cached_tokens":40},
                          "completion_tokens_details":{"reasoning_tokens":2}}}"#,
        )
        .unwrap();
        let without: Value = serde_json::from_str(
            r#"{"choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}"#,
        )
        .unwrap();
        let rw = parse_response(&with).unwrap();
        let rn = parse_response(&without).unwrap();
        add(
            "usage parsing",
            rw.usage.cached_prompt_tokens == Some(40)
                && rw.usage.reasoning_tokens == Some(2)
                && rn.usage.cached_prompt_tokens.is_none()
                && rn.usage.prompt_tokens.is_none(),
            "reported details parsed; absent details stay null (never zero)".to_string(),
        );
    }

    // 9-13. Synthetic datasets through the full verifiers.
    let reg = match load_tasks(&dir.join("tasks.json")) {
        Ok(file) => file.tasks,
        Err(e) => return Err(format!("self-test: cannot load registry: {e}")),
    };
    let cal_records = trace::synthetic_calibration_records(&reg);
    add(
        "150-record calibration design",
        cal_records.len() == trace::CALIBRATION_EPISODES,
        format!("{} synthetic calibration episodes", cal_records.len()),
    );

    let cal_summary = trace::compute_calibration_summary(&cal_records, &reg);
    add(
        "calibration family selection (synthetic)",
        cal_summary.calibrated_family_count == 2 && cal_summary.proceed_to_evaluation,
        format!(
            "families: {} planned_pairs={} reps/task={}",
            cal_summary
                .per_family
                .iter()
                .map(|f| format!(
                    "{}={}/{}",
                    f.family,
                    f.status.as_str(),
                    f.selected_stress.clone().unwrap_or_else(|| "-".into())
                ))
                .collect::<Vec<_>>()
                .join(", "),
            cal_summary.planned_pairs,
            cal_summary.repetitions_per_task
        ),
    );

    let (bh, th, sh) = (
        "1".repeat(64),
        task_registry_sha256(&reg).unwrap_or_default(),
        stress_registry_sha256(
            &crate::tools::STRESS_LEVELS
                .iter()
                .map(|l| RawStressLevel {
                    id: l.id.to_string(),
                    drop_count: l.drop_count,
                })
                .collect::<Vec<_>>(),
        )
        .unwrap_or_default(),
    );
    let prov = trace::PlanProvenance {
        code_under_test_commit: "0".repeat(40),
        baseline_prompt_sha256: bh.clone(),
        repair_prompt_sha256: "2".repeat(64),
        task_registry_sha256: th.clone(),
        stress_registry_sha256: sh.clone(),
    };
    let mut plan = trace::recompute_evaluation_plan(&cal_records, &reg, &prov);
    plan.calibration_raw_sha256 = "a".repeat(64);

    let cal_values: Vec<Value> = cal_records
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let cal_errors = trace::verify_calibration_artifacts(
        &cal_values,
        &serde_json::to_value(&cal_summary).unwrap(),
        &serde_json::to_value(&plan).unwrap(),
        &reg,
        &bh,
        &"2".repeat(64),
        &th,
        &sh,
        &"a".repeat(64),
    );
    add(
        "calibration artifact verification (synthetic)",
        cal_errors.is_empty(),
        if cal_errors.is_empty() {
            "all 150 records verify: design, cyclic order, audit, oracle, usage, summary, plan, power"
                .to_string()
        } else {
            cal_errors
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        },
    );

    let eval_records = trace::synthetic_evaluation_records(&reg, &plan);
    add(
        "96-record evaluation design (48 pairs, fixed budget)",
        eval_records.len() == EVAL_EPISODES,
        format!("{} synthetic evaluation episodes", eval_records.len()),
    );
    let eval_summary = trace::compute_evaluation_summary(&eval_records, &reg, &plan);
    let eval_values: Vec<Value> = eval_records
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let eval_errors = trace::verify_evaluation_artifacts(
        &eval_values,
        &serde_json::to_value(&eval_summary).unwrap(),
        &serde_json::to_value(&plan).unwrap(),
        &reg,
        &bh,
        &"2".repeat(64),
        &th,
        &"b".repeat(64),
    );
    add(
        "evaluation artifact verification (synthetic)",
        eval_errors.is_empty(),
        if eval_errors.is_empty() {
            format!(
                "all records verify; potential_info={} valid={} informative={} → {}",
                eval_summary
                    .potential_information
                    .potential_information_capacity,
                eval_summary.comparison.valid_pairs,
                eval_summary.comparison.informative_pairs,
                eval_summary.conclusion.result
            )
        } else {
            eval_errors
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        },
    );

    // Report.
    let mut all_ok = true;
    for c in &checks {
        if !c.ok {
            all_ok = false;
        }
        println!(
            "  [{}] {} — {}",
            if c.ok { "PASS" } else { "FAIL" },
            c.name,
            c.detail
        );
    }
    if all_ok {
        println!("self-test: ALL CHECKS PASSED");
        Ok(())
    } else {
        Err("self-test: some checks failed".to_string())
    }
}
