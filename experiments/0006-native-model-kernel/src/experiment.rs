//! Experiment orchestration: the runner outside the kernel.
//!
//! Boundaries (spec §15): this module parses run configuration, loads
//! prompts/tasks from files, executes episodes through the kernel, and
//! writes artifacts. The kernel itself has no knowledge of any of it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;

use crate::kernel::{self, KernelConfig, RecordMeta};
use crate::model::{ModelClient, redacted_endpoint};
use crate::protocol::{ToolSpec, sha256_bytes, sha256_hex};
use crate::trace::{EpisodeRecord, compute_summary};

/// The one-line candidate mutation (spec §34).
pub const CANDIDATE_MUTATION: &str = "After every successful state_write, call state_read on the same key before finalizing, even if the write reported success.";

/// One experiment task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub prompt: String,
}

/// Load the on-disk task suite (`tasks.json`).
pub fn load_tasks(path: &Path) -> Result<Vec<Task>, String> {
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
            serde_json::from_value::<Task>(t.clone())
                .map_err(|e| format!("invalid task entry: {e}"))
        })
        .collect()
}

/// Read a prompt file as text.
pub fn load_prompt(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read prompt {}: {e}", path.display()))
}

/// Programmatic prompt invariant (spec §35):
/// `candidate == baseline + exactly one mutation line`.
pub fn verify_prompt_invariant(baseline: &str, candidate: &str) -> Result<(), String> {
    let b = baseline.trim_end_matches('\n');
    let c = candidate.trim_end_matches('\n');
    let expected = format!("{b}\n{CANDIDATE_MUTATION}");
    if c != expected {
        Err(
            "prompt invariant violated: candidate.md is not exactly baseline.md + the registered mutation line"
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

/// Canonical SHA-256 of the parsed task suite (key-order independent).
pub fn task_suite_sha256(tasks: &[Task]) -> Result<String, String> {
    let v: Value = serde_json::to_value(json!({ "tasks": tasks }))
        .map_err(|e| format!("task serialization: {e}"))?;
    Ok(sha256_hex(&v))
}

/// The experiment crate's directory (prompts/, tasks.json live here).
pub fn crate_dir() -> PathBuf {
    std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("experiments/0006-native-model-kernel"))
}

/// Environment configuration for a real run (spec §18, §56).
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

/// `git rev-parse HEAD` inside the repository (provenance, spec §57).
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
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub repetitions: u32,
    pub trajectories: PathBuf,
    pub summary: PathBuf,
}

/// Execute the entire experiment and write both artifacts.
///
/// Ordering (spec §40): per task, rep1 baseline→candidate,
/// rep2 candidate→baseline, rep3 baseline→candidate.
pub fn run(config: &RunConfig) -> Result<(), String> {
    let (base_url, model, api_key) = env_config()?;

    let dir = crate_dir();
    let baseline = load_prompt(&dir.join("prompts/baseline.md"))?;
    let candidate = load_prompt(&dir.join("prompts/candidate.md"))?;
    verify_prompt_invariant(&baseline, &candidate)
        .map_err(|e| format!("configuration error, experiment not run: {e}"))?;

    let tasks = load_tasks(&dir.join("tasks.json"))?;
    if tasks.len() != 6 {
        return Err(format!(
            "configuration error: expected 6 tasks, found {}",
            tasks.len()
        ));
    }

    let baseline_hash = hash_file(&dir.join("prompts/baseline.md"))?;
    let candidate_hash = hash_file(&dir.join("prompts/candidate.md"))?;
    let suite_hash = task_suite_sha256(&tasks)?;
    let commit = code_under_test_commit()?;
    let endpoint = redacted_endpoint(&base_url);

    // Start a fresh artifact file.
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

    let run_prefix = format!(
        "exp0006-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );

    let mut records: Vec<EpisodeRecord> = Vec::new();
    let mut counter = 0usize;

    println!(
        "starting run: model={model} endpoint={endpoint} episodes={} code_commit={commit}",
        tasks.len() * 2 * config.repetitions as usize
    );

    for (ti, task) in tasks.iter().enumerate() {
        for rep in 1..=config.repetitions {
            let mut order: [&str; 2] = ["baseline", "candidate"];
            if rep % 2 == 0 {
                order = ["candidate", "baseline"];
            }
            for condition in order {
                counter += 1;
                let run_id = format!("{run_prefix}-{ti}-{rep}-{counter}");
                let prompt = if condition == "baseline" {
                    &baseline
                } else {
                    &candidate
                };

                let meta = RecordMeta {
                    run_id: run_id.clone(),
                    task_id: task.id.clone(),
                    task_name: task.name.clone(),
                    condition: condition.to_string(),
                    repetition: rep,
                    model: model.clone(),
                    redacted_endpoint: endpoint.clone(),
                    code_under_test_commit: commit.clone(),
                    baseline_prompt_sha256: baseline_hash.clone(),
                    candidate_prompt_sha256: candidate_hash.clone(),
                    task_suite_sha256: suite_hash.clone(),
                };

                eprintln!(
                    "[{}/{}] {} {} rep{} …",
                    counter,
                    tasks.len() * 2 * config.repetitions as usize,
                    task.id,
                    condition,
                    rep
                );

                // Fresh tool executor per episode: no state reuse.
                let mut tools = crate::tools::ToolExecutor::fresh();
                let outcome = kernel::run_episode(
                    |messages: &[crate::protocol::Message], specs: &[ToolSpec]| {
                        client.complete(messages, specs)
                    },
                    &mut tools,
                    &specs,
                    prompt,
                    &task.prompt,
                    &kernel_config,
                );

                let record = kernel::build_record(outcome, &meta);
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
                    "  {} eligible={} turns={} calls={} reason={}",
                    run_id,
                    record.eligible,
                    record.model_turn_count,
                    record.tool_call_count,
                    record.termination_reason
                );
            }
        }
    }
    out.flush().map_err(|e| format!("final flush: {e}"))?;

    let summary = compute_summary(&records);
    if let Some(parent) = config.summary.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let summary_json = serde_json::to_string_pretty(&summary)
        .map_err(|e| format!("summary serialization: {e}"))?;
    std::fs::write(&config.summary, summary_json + "\n")
        .map_err(|e| format!("cannot write summary: {e}"))?;

    println!(
        "run complete: {} episodes ({} eligible), {} eligible pairs, conclusion={}",
        summary.total_episodes,
        summary.eligible_episodes,
        summary.eligible_pairs,
        summary.conclusion.as_str()
    );
    println!("trajectories: {}", config.trajectories.display());
    println!("summary: {}", config.summary.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASELINE: &str = "line one\nline two";

    #[test]
    fn candidate_equal_to_baseline_plus_mutation_passes() {
        // baseline ending in a newline is equivalent: the mutation line
        // is the next line of the candidate file.
        let candidate = format!("{BASELINE}\n{CANDIDATE_MUTATION}\n");
        assert!(verify_prompt_invariant(BASELINE, &candidate).is_ok());
        assert!(verify_prompt_invariant(&format!("{BASELINE}\n"), &candidate).is_ok());
    }

    #[test]
    fn candidate_with_any_other_difference_fails() {
        // extra unrelated line
        let c = format!("{BASELINE}\n{CANDIDATE_MUTATION}\nextra line\n");
        assert!(verify_prompt_invariant(BASELINE, &c).is_err());
        // missing mutation
        assert!(verify_prompt_invariant(BASELINE, BASELINE).is_err());
        // mutation present but different wording
        let c = format!("{BASELINE}\nread back after write.\n");
        assert!(verify_prompt_invariant(BASELINE, &c).is_err());
    }

    #[test]
    fn real_prompt_files_satisfy_the_invariant() {
        let dir = crate_dir();
        if !dir.join("prompts/baseline.md").exists() {
            return; // not running from a source checkout
        }
        let b = load_prompt(&dir.join("prompts/baseline.md")).unwrap();
        let c = load_prompt(&dir.join("prompts/candidate.md")).unwrap();
        assert!(verify_prompt_invariant(&b, &c).is_ok());
    }

    #[test]
    fn tasks_json_loads_six_tasks() {
        let dir = crate_dir();
        if !dir.join("tasks.json").exists() {
            return;
        }
        let tasks = load_tasks(&dir.join("tasks.json")).unwrap();
        assert_eq!(tasks.len(), 6);
        assert_eq!(tasks[0].id, "T1");
        let h = task_suite_sha256(&tasks).unwrap();
        assert_eq!(h.len(), 64);
    }
}
