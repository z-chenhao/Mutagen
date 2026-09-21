//! Experiment 0007 runner: candidate quality discrimination by an
//! independent final-state oracle on repeated real-model executions.
//!
//! Usage:
//!   run --repetitions 6 --trajectories PATH --summary PATH
//!   self-test
//!   verify-artifacts --trajectories PATH --summary PATH
//!
//! `run` requires MUTAGEN_EXP_BASE_URL and MUTAGEN_EXP_MODEL
//! (MUTAGEN_EXP_API_KEY optional). If they are missing, the experiment
//! is not executed and no fallback model is substituted.
//!
//! This binary is presentation/control only: argument parsing, artifact
//! file handling, and the artifact verifier. None of that logic lives
//! in the kernel.

#![forbid(unsafe_code)]

mod experiment;
mod kernel;
mod model;
mod oracle;
mod protocol;
mod stats;
mod tools;
mod trace;

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::Value;

struct Flags {
    repetitions: Option<u32>,
    trajectories: Option<PathBuf>,
    summary: Option<PathBuf>,
}

fn parse_flags(args: &[String], want_repetitions: bool) -> Result<Flags, String> {
    let mut repetitions: Option<u32> = None;
    let mut trajectories: Option<PathBuf> = None;
    let mut summary: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--repetitions" => {
                i += 1;
                let v = args
                    .get(i)
                    .ok_or("--repetitions needs a value")?
                    .parse::<u32>()
                    .map_err(|_| {
                        format!("invalid --repetitions value: {}", args.get(i).unwrap())
                    })?;
                if v == 0 {
                    return Err("--repetitions must be >= 1".into());
                }
                repetitions = Some(v);
            }
            "--trajectories" => {
                i += 1;
                trajectories = Some(PathBuf::from(
                    args.get(i).ok_or("--trajectories needs a value")?,
                ));
            }
            "--summary" => {
                i += 1;
                summary = Some(PathBuf::from(args.get(i).ok_or("--summary needs a value")?));
            }
            other => {
                return Err(format!(
                    "unknown argument {other:?} (expected --repetitions / --trajectories / --summary)"
                ));
            }
        }
        i += 1;
    }

    if want_repetitions {
        repetitions = Some(repetitions.ok_or("missing --repetitions")?);
    }
    trajectories = Some(trajectories.ok_or("missing --trajectories")?);
    summary = Some(summary.ok_or("missing --summary")?);
    Ok(Flags {
        repetitions,
        trajectories,
        summary,
    })
}

fn cmd_run(flags: Flags) -> Result<(), String> {
    let config = experiment::RunConfig {
        repetitions: flags.repetitions.ok_or("run requires --repetitions")?,
        trajectories: flags.trajectories.ok_or("run requires --trajectories")?,
        summary: flags.summary.ok_or("run requires --summary")?,
    };
    if config.repetitions != trace::EXPECTED_REPETITIONS {
        return Err(format!(
            "the registered design uses exactly {} repetitions per task ({} episodes total); \
             reducing after observing results is not allowed",
            trace::EXPECTED_REPETITIONS,
            trace::EXPECTED_EPISODES
        ));
    }
    experiment::run(&config)
}

/// Full artifact verification from files (spec §62, §77).
fn cmd_verify_artifacts(flags: Flags) -> Result<(), String> {
    let traj_path = flags
        .trajectories
        .ok_or("verify-artifacts requires --trajectories")?;
    let summary_path = flags.summary.ok_or("verify-artifacts requires --summary")?;

    let raw = std::fs::read_to_string(&traj_path)
        .map_err(|e| format!("cannot read {}: {e}", traj_path.display()))?;
    let mut record_values = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line)
            .map_err(|e| format!("{}: line {}: invalid JSON: {e}", traj_path.display(), i + 1))?;
        record_values.push(v);
    }
    let summary_raw = std::fs::read_to_string(&summary_path)
        .map_err(|e| format!("cannot read {}: {e}", summary_path.display()))?;
    let summary_value: Value = serde_json::from_str(&summary_raw)
        .map_err(|e| format!("{}: summary is not valid JSON: {e}", summary_path.display()))?;

    let dir = experiment::crate_dir();
    let registry = experiment::load_tasks(&dir.join("tasks.json"))
        .map_err(|e| format!("cannot load task registry: {e}"))?;

    let mut errors = trace::verify_artifacts(&record_values, &summary_value, &registry);

    // Cross-check provenance hashes against the committed prompt files.
    if let Some(first) = record_values.first() {
        for (field, path) in [
            ("baseline_prompt_sha256", "prompts/baseline.md"),
            ("repair_prompt_sha256", "prompts/repair.md"),
            ("regression_prompt_sha256", "prompts/regression.md"),
        ] {
            let recorded = first.get(field).and_then(Value::as_str).unwrap_or("");
            if let Ok(h) = experiment::hash_file(&dir.join(path)) {
                if h != recorded {
                    errors.push(format!(
                        "{field} in artifact does not match committed {path} (prompt tampered)"
                    ));
                }
            } else {
                errors.push(format!("cannot hash {path}"));
            }
        }
    }

    if errors.is_empty() {
        println!("artifact verification: PASSED");
        println!("  records: {}", record_values.len());
        println!("  summary: {}", summary_path.display());
        Ok(())
    } else {
        println!("artifact verification: FAILED ({} issues)", errors.len());
        for e in &errors {
            println!("  - {e}");
        }
        Err(format!("{} issue(s) found", errors.len()))
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();

    let outcome = match cmd {
        "run" => parse_flags(&rest, true).and_then(cmd_run),
        "self-test" => experiment::self_test(),
        "verify-artifacts" => parse_flags(&rest, false).and_then(cmd_verify_artifacts),
        _ => Err("usage: mutagen-exp-0007 <run --repetitions 6 --trajectories P --summary S | self-test | verify-artifacts --trajectories P --summary S>".to_string()),
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
