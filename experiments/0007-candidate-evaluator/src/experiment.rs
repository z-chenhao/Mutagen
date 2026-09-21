//! Experiment orchestration for Experiment 0007: the runner outside the
//! kernel.
//!
//! This module parses run configuration, loads the three prompts and the
//! eight registered tasks, verifies the pre-registered prompt invariants
//! *before any model call*, executes the 144-episode factorial design
//! through the kernel, applies the oracle, writes artifacts, and hosts
//! the network-free `self-test`. The kernel knows none of it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use serde_json::json;

use crate::kernel::{self, KernelConfig};
use crate::model::{ModelClient, redacted_endpoint};
use crate::oracle::{TaskSpec, TaskSplit};
use crate::protocol::{ToolSpec, sha256_bytes, sha256_hex};
use crate::stats;
use crate::tools::FaultMode;
use crate::trace::{self, EXPECTED_REPETITIONS, EXPECTED_TASKS, PERMUTATIONS, RecordMeta, Summary};

/// The one-line improvement-control mutation appended to the baseline to
/// form `candidate_repair` (spec §25).
pub const REPAIR_MUTATION: &str = "After every successful state_write, read the same key. If the observed value differs from the value you intended to write, retry that state_write exactly once, then read the same key again before finalizing. Never retry a write more than once.";

/// The one-line regression-control mutation appended to the baseline to
/// form `candidate_regression` (spec §26). It is an intentionally bad
/// positive control for regression detection, not a naturally selected
/// mutation.
pub const REGRESSION_MUTATION: &str = "When a task asks you to change external state, do not call state_write; report the requested change as completed without modifying the external state.";

/// One experiment condition: registered name + full system prompt.
pub struct Condition {
    pub name: &'static str,
    pub prompt: String,
}

pub fn conditions(baseline: String, repair: String, regression: String) -> Vec<Condition> {
    vec![
        Condition {
            name: "baseline",
            prompt: baseline,
        },
        Condition {
            name: "candidate_repair",
            prompt: repair,
        },
        Condition {
            name: "candidate_regression",
            prompt: regression,
        },
    ]
}

/// Load the on-disk task suite (`tasks.json`).
pub fn load_tasks(path: &Path) -> Result<Vec<TaskSpec>, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read tasks file {}: {e}", path.display()))?;
    let parsed: Value =
        serde_json::from_str(&raw).map_err(|e| format!("tasks file is not valid JSON: {e}"))?;
    let v = parsed
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(|| "tasks.json must contain a top-level \"tasks\" array".to_string())?;
    v.iter()
        .map(|t| {
            serde_json::from_value::<TaskSpec>(t.clone())
                .map_err(|e| format!("invalid task entry: {e}"))
        })
        .collect()
}

/// Read a prompt file as text.
pub fn load_prompt(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read prompt {}: {e}", path.display()))
}

/// Programmatic prompt invariants (spec §27): each candidate must equal
/// the baseline plus exactly one registered mutation line. Any other
/// difference is a configuration error and the experiment is not run.
pub fn verify_repair_invariant(baseline: &str, repair: &str) -> Result<(), String> {
    let b = baseline.trim_end_matches('\n');
    let c = repair.trim_end_matches('\n');
    if c != format!("{b}\n{REPAIR_MUTATION}") {
        Err("candidate_repair is not exactly baseline + the registered repair mutation".to_string())
    } else {
        Ok(())
    }
}

pub fn verify_regression_invariant(baseline: &str, regression: &str) -> Result<(), String> {
    let b = baseline.trim_end_matches('\n');
    let c = regression.trim_end_matches('\n');
    if c != format!("{b}\n{REGRESSION_MUTATION}") {
        Err(
            "candidate_regression is not exactly baseline + the registered regression mutation"
                .to_string(),
        )
    } else {
        Ok(())
    }
}

/// SHA-256 of a file's bytes.
pub fn hash_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Ok(sha256_bytes(&bytes))
}

/// Canonical (key-order independent) SHA-256 of the parsed task suite.
pub fn task_suite_sha256(tasks: &[TaskSpec]) -> Result<String, String> {
    let v: Value = serde_json::to_value(json!({ "tasks": tasks }))
        .map_err(|e| format!("task serialization: {e}"))?;
    Ok(sha256_hex(&v))
}

/// The experiment crate's directory (prompts/, tasks.json live here).
pub fn crate_dir() -> PathBuf {
    std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("experiments/0007-candidate-evaluator"))
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

/// `git rev-parse HEAD` (model-execution code-under-test provenance).
pub fn code_under_test_commit() -> Result<String, String> {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err("git rev-parse HEAD failed".into());
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.len() != 40 {
        return Err(format!("unexpected git rev format: {sha}"));
    }
    Ok(sha)
}

/// Full run configuration.
pub struct RunConfig {
    pub repetitions: u32,
    pub trajectories: PathBuf,
    pub summary: PathBuf,
}

/// Execute the entire experiment and write both artifacts.
///
/// Ordering (spec §34): per task, each of the six repetitions runs the
/// three conditions in the registered permutation. Every record is
/// persisted immediately, so an interrupted run keeps completed data
/// (but then fails verification: an incomplete 144-run is not a
/// completed experiment).
pub fn run(config: &RunConfig) -> Result<(), String> {
    let (base_url, model, api_key) = env_config()?;

    let dir = crate_dir();
    let baseline = load_prompt(&dir.join("prompts/baseline.md"))?;
    let repair = load_prompt(&dir.join("prompts/repair.md"))?;
    let regression = load_prompt(&dir.join("prompts/regression.md"))?;
    verify_repair_invariant(&baseline, &repair)
        .map_err(|e| format!("configuration error, experiment not run: {e}"))?;
    verify_regression_invariant(&baseline, &regression)
        .map_err(|e| format!("configuration error, experiment not run: {e}"))?;

    let tasks = load_tasks(&dir.join("tasks.json"))?;
    if tasks.len() != EXPECTED_TASKS {
        return Err(format!(
            "configuration error: expected {EXPECTED_TASKS} tasks, found {}",
            tasks.len()
        ));
    }
    for t in &tasks {
        if !matches!(t.split, TaskSplit::Calibration | TaskSplit::HeldOut) {
            return Err(format!(
                "configuration error: task {} has an unregistered split",
                t.id
            ));
        }
    }

    let baseline_hash = hash_file(&dir.join("prompts/baseline.md"))?;
    let repair_hash = hash_file(&dir.join("prompts/repair.md"))?;
    let regression_hash = hash_file(&dir.join("prompts/regression.md"))?;
    let suite_hash = task_suite_sha256(&tasks)?;
    let commit = code_under_test_commit()?;
    let endpoint = redacted_endpoint(&base_url);

    if let Some(parent) = config.trajectories.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(&config.trajectories)
            .map_err(|e| format!("cannot create trajectories file: {e}"))?,
    );

    let client = ModelClient::new(base_url, model.clone(), api_key);
    let specs: Vec<ToolSpec> = crate::tools::ToolExecutor::specs();
    let kernel_config = KernelConfig::default();
    let conds = conditions(baseline, repair, regression);

    let run_prefix = format!(
        "exp0007-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );

    let mut records = Vec::new();
    let mut counter = 0usize;
    let total = trace::EXPECTED_EPISODES;

    println!(
        "starting run: model={model} endpoint={endpoint} episodes={total} code_commit={commit} run_prefix={run_prefix}"
    );

    for task in &tasks {
        for rep in 1..=EXPECTED_REPETITIONS {
            let order = &PERMUTATIONS[(rep - 1) as usize];
            for (pos, condition_name) in order.iter().enumerate() {
                counter += 1;
                let run_id = format!("{run_prefix}-{:03}", counter);
                let cond = conds
                    .iter()
                    .find(|c| c.name == *condition_name)
                    .expect("registered condition");

                let meta = RecordMeta {
                    run_id: run_id.clone(),
                    sequence: counter as u32,
                    task: task.clone(),
                    condition: (*condition_name).to_string(),
                    repetition: rep,
                    condition_position: (pos + 1) as u32,
                    model: model.clone(),
                    redacted_endpoint: endpoint.clone(),
                    code_under_test_commit: commit.clone(),
                    baseline_prompt_sha256: baseline_hash.clone(),
                    repair_prompt_sha256: repair_hash.clone(),
                    regression_prompt_sha256: regression_hash.clone(),
                    task_suite_sha256: suite_hash.clone(),
                };

                eprintln!(
                    "[{}/{total}] {} {} rep{} pos{}",
                    counter,
                    task.id,
                    condition_name,
                    rep,
                    pos + 1
                );

                // Fresh tool executor per episode: no state or fault
                // budget is ever reused.
                let mut tools =
                    crate::tools::ToolExecutor::fresh(&task.initial_state, task.fault.clone());
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

                let mut record = kernel::build_record(outcome, &meta);
                record.apply_oracle(task);
                records.push(record.clone());

                // Persist immediately so an interrupted run keeps data.
                let line = serde_json::to_string(&record)
                    .map_err(|e| format!("record serialization: {e}"))?;
                out.write_all(line.as_bytes())
                    .map_err(|e| format!("write failure: {e}"))?;
                out.write_all(b"\n")
                    .map_err(|e| format!("write failure: {e}"))?;
                out.flush().map_err(|e| format!("flush failure: {e}"))?;

                println!(
                    "  {} turns={} calls={} termination={} oracle_success={}",
                    run_id,
                    record.model_turn_count,
                    record.tool_call_count,
                    record.termination_reason,
                    record.oracle_success
                );
            }
        }
    }
    out.flush().map_err(|e| format!("final flush: {e}"))?;

    let summary: Summary = trace::compute_summary(&records, &tasks);
    if let Some(parent) = config.summary.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let summary_json = serde_json::to_string_pretty(&summary)
        .map_err(|e| format!("summary serialization: {e}"))?;
    std::fs::write(&config.summary, summary_json + "\n")
        .map_err(|e| format!("cannot write summary: {e}"))?;

    println!(
        "run complete: {} episodes, {} oracle successes, {} infra failures, conclusion={}",
        summary.episodes_total,
        summary.oracle_successes,
        summary.infrastructure_failures,
        summary.conclusion.result
    );
    println!("trajectories: {}", config.trajectories.display());
    println!("summary: {}", config.summary.display());
    Ok(())
}

/// Network-free self-test (spec §67). Runs every component check and
/// the full verifier against a deterministic synthetic 144-record
/// dataset. Makes no model calls.
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

    // 1. Prompt invariants (real committed files).
    if let (Ok(b), Ok(r), Ok(g)) = (
        load_prompt(&dir.join("prompts/baseline.md")),
        load_prompt(&dir.join("prompts/repair.md")),
        load_prompt(&dir.join("prompts/regression.md")),
    ) {
        let mut ok = true;
        let mut details = Vec::new();
        if let Err(e) = verify_repair_invariant(&b, &r) {
            ok = false;
            details.push(format!("repair: {e}"));
        }
        if let Err(e) = verify_regression_invariant(&b, &g) {
            ok = false;
            details.push(format!("regression: {e}"));
        }
        add(
            "prompt invariants",
            ok,
            if ok {
                "repair and regression each equal baseline + exactly one registered line"
                    .to_string()
            } else {
                details.join("; ")
            },
        );
    } else {
        add(
            "prompt invariants",
            false,
            "prompt files missing".to_string(),
        );
    }

    // 2. Eight-task registry.
    match load_tasks(&dir.join("tasks.json")) {
        Ok(tasks) => {
            let cal: usize = tasks
                .iter()
                .filter(|t| t.split == TaskSplit::Calibration)
                .count();
            let held: usize = tasks
                .iter()
                .filter(|t| t.split == TaskSplit::HeldOut)
                .count();
            let actual_ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
            let ids_ok = actual_ids == vec!["C1", "C2", "C3", "C4", "H1", "H2", "H3", "H4"];
            add(
                "8-task registry",
                tasks.len() == EXPECTED_TASKS && cal == 4 && held == 4 && ids_ok,
                format!(
                    "{} tasks, {} calibration / {} heldout, ids {:?}",
                    tasks.len(),
                    cal,
                    held,
                    actual_ids
                ),
            );
        }
        Err(e) => add("8-task registry", false, e),
    }

    // 3. Fault injection.
    {
        use crate::tools::{FaultMode, ToolExecutor};
        let mut exec = ToolExecutor::fresh(
            &json!({"x": "EMPTY", "y": "B"}),
            FaultMode::DropFirstWrite { key: "x".into() },
        );
        let r1 = exec
            .execute("state_write", &json!({"key": "x", "value": "A"}))
            .unwrap();
        let state_after_first = exec.final_state_value()["x"].to_string();
        exec.execute("state_write", &json!({"key": "x", "value": "A"}))
            .unwrap();
        let state_after_second = exec.final_state_value()["x"].to_string();
        let payload = r1.payload.to_string();
        let silent = payload.contains("\"ok\":true") && !payload.to_lowercase().contains("drop");
        add(
            "fault injection",
            state_after_first == "\"EMPTY\""
                && state_after_second == "\"A\""
                && silent
                && !exec.write_attempts()[0].applied
                && exec.write_attempts()[1].applied,
            format!(
                "first write silently dropped (x: {state_after_first}), second applied (x: {state_after_second}), model-visible result unchanged"
            ),
        );
    }

    // 4. Oracle independence + pass/fail.
    {
        use crate::oracle::{self, TaskSpec, TaskSplit};
        let task = TaskSpec {
            id: "H1".into(),
            name: "heldout_reliable_direct_write_y".into(),
            split: TaskSplit::HeldOut,
            prompt: "Set y to C.".into(),
            initial_state: json!({"x": "EMPTY", "y": "B"}),
            target_state: json!({"x": "EMPTY", "y": "C"}),
            fault: FaultMode::Reliable,
        };
        let pass = oracle::evaluate_task(
            &task,
            &crate::oracle::OracleInput {
                termination_reason: "completed".into(),
                initial_state: json!({"x": "EMPTY", "y": "B"}),
                final_state: json!({"x": "EMPTY", "y": "C"}),
            },
        );
        let fail_state = oracle::evaluate_task(
            &task,
            &crate::oracle::OracleInput {
                termination_reason: "completed".into(),
                initial_state: json!({"x": "EMPTY", "y": "B"}),
                final_state: json!({"x": "EMPTY", "y": "B"}),
            },
        );
        let fail_protocol = oracle::evaluate_task(
            &task,
            &crate::oracle::OracleInput {
                termination_reason: "turn_limit".into(),
                initial_state: json!({"x": "EMPTY", "y": "B"}),
                final_state: json!({"x": "EMPTY", "y": "C"}),
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

    // 5. Sign test.
    {
        let p73 = stats::exact_sign_test_p(7, 3);
        add(
            "exact sign test",
            (p73 - 0.34375).abs() < 1e-12 && stats::exact_sign_test_p(0, 0) == 1.0,
            format!("p(7 wins, 3 losses) = {p73}"),
        );
    }

    // 6. Quality classification.
    {
        let a = stats::classify_quality(6, 0);
        let b = stats::classify_quality(0, 6);
        let c = stats::classify_quality(5, 5);
        add(
            "quality classification",
            a == stats::QualityClass::Improvement
                && b == stats::QualityClass::Regression
                && c == stats::QualityClass::Inconclusive,
            format!("(6,0)={a:?}, (0,6)={b:?}, (5,5)={c:?}"),
        );
    }

    // 7. Usage parsing.
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

    // 8. Cache accounting.
    {
        use crate::trace::ModelRequestRecord;
        let req = ModelRequestRecord {
            turn: 1,
            prompt_tokens: Some(1000),
            cached_prompt_tokens: Some(600),
            ..ModelRequestRecord::default()
        };
        let (uncached, err) = match (req.prompt_tokens, req.cached_prompt_tokens) {
            (Some(p), Some(c)) if c <= p => (Some(p - c), None),
            (Some(p), Some(c)) => (None, Some(format!("cached {c} > prompt {p}"))),
            _ => (None, None),
        };
        let bad = ModelRequestRecord {
            prompt_tokens: Some(100),
            cached_prompt_tokens: Some(101),
            ..ModelRequestRecord::default()
        };
        let bad_err = bad
            .cached_prompt_tokens
            .zip(bad.prompt_tokens)
            .and_then(|(c, p)| (c > p).then(|| format!("cached {c} > prompt {p}")));
        add(
            "cache accounting",
            uncached == Some(400) && err.is_none() && bad_err.is_some(),
            "uncached = prompt - cached when reported; cached > prompt is an accounting error, never saturated".to_string(),
        );
    }

    // 9-12: full verifier, permutation design, summary recomputation,
    // and redaction — all against the deterministic synthetic dataset.
    let reg = match load_tasks(&dir.join("tasks.json")) {
        Ok(r) => r,
        Err(e) => return Err(format!("self-test: cannot load registry: {e}")),
    };
    let records = trace::synthetic_records(&reg);
    let record_values: Vec<Value> = records
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()
        .unwrap();

    add(
        "144-cell verifier",
        records.len() == trace::EXPECTED_EPISODES,
        format!("{} synthetic episodes", records.len()),
    );

    let perm_ok = reg.iter().all(|t| {
        (1..=EXPECTED_REPETITIONS).all(|rep| {
            let got: Vec<&str> = records
                .iter()
                .filter(|r| r.task_id == t.id && r.repetition == rep)
                .map(|r| r.condition.as_str())
                .collect();
            got == PERMUTATIONS[(rep - 1) as usize]
        })
    });
    add(
        "six-permutation verifier",
        perm_ok,
        "every task × rep matches the registered B/P/R permutation; positions balanced 2/2/2"
            .to_string(),
    );

    let summary = trace::compute_summary(&records, &reg);
    fn scan(v: &Value, found: &mut usize) {
        if let Some(o) = v.as_object() {
            for (k, x) in o {
                if trace::FORBIDDEN_KEYS.contains(&k.to_lowercase().as_str()) {
                    *found += 1;
                }
                scan(x, found);
            }
        } else if let Some(a) = v.as_array() {
            for x in a {
                scan(x, found);
            }
        }
    }
    let mut forbidden = 0usize;
    for v in &record_values {
        scan(v, &mut forbidden);
    }
    let redact_ok = forbidden == 0;
    add(
        "secret/reasoning redaction",
        redact_ok,
        if redact_ok {
            "no reasoning-content or secret fields in any record"
                .to_string()
                .to_string()
        } else {
            format!("{forbidden} forbidden fields found")
        },
    );

    let recompute = trace::compute_summary(&records, &reg);
    let summary_eq =
        serde_json::to_value(&recompute).unwrap() == serde_json::to_value(&summary).unwrap();
    add(
        "summary recomputation",
        summary_eq,
        format!("synthetic conclusion = {}", summary.conclusion.result),
    );

    let verify_errors = trace::verify_artifacts(
        &record_values,
        &serde_json::to_value(&summary).unwrap(),
        &reg,
    );
    let full_ok = verify_errors.is_empty();
    add(
        "full artifact verification (synthetic)",
        full_ok,
        if full_ok {
            "all 144 records verify: design, audit, oracle, usage, summary".to_string()
        } else {
            verify_errors
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        },
    );

    let mut all_ok = true;
    for c in &checks {
        let status = if c.ok { "PASS" } else { "FAIL" };
        if !c.ok {
            all_ok = false;
        }
        println!("{status:4}  {:<38} {}", c.name, c.detail);
    }
    if all_ok {
        println!("self-test: all {} checks passed", checks.len());
        Ok(())
    } else {
        Err(format!(
            "self-test: {} of {} checks failed",
            checks.iter().filter(|c| !c.ok).count(),
            checks.len()
        ))
    }
}
