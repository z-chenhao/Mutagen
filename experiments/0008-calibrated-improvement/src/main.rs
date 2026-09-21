//! Experiment 0008 runner: baseline-only, pre-registered
//! difficulty-calibrated improvement discrimination.
//!
//! Usage:
//!   self-test
//!   calibrate --trajectories PATH --summary PATH --selection PATH
//!   evaluate  --selection PATH --trajectories PATH --summary PATH
//!   verify-calibration --trajectories PATH --summary PATH --selection PATH
//!   verify-evaluation  --selection PATH --trajectories PATH --summary PATH
//!
//! `calibrate` and `evaluate` require MUTAGEN_EXP_BASE_URL and
//! MUTAGEN_EXP_MODEL (MUTAGEN_EXP_API_KEY optional). If they are missing,
//! the experiment is not executed and no fallback model is substituted.
//!
//! This binary is presentation/control only: argument parsing, artifact
//! file handling, and the artifact verifiers. None of that logic lives
//! in the kernel.

#![forbid(unsafe_code)]

mod calibration;
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

#[derive(Debug, Default)]
struct Flags {
    trajectories: Option<PathBuf>,
    summary: Option<PathBuf>,
    selection: Option<PathBuf>,
    code_commit: Option<String>,
    endpoint: Option<String>,
}

fn take(flags: &mut Flags, name: &str, args: &[String], i: &mut usize) -> Result<(), String> {
    let v = args
        .get(*i + 1)
        .ok_or_else(|| format!("{name} needs a value"))?;
    let p = PathBuf::from(v);
    *i += 1;
    match name {
        "--trajectories" => flags.trajectories = Some(p),
        "--summary" => flags.summary = Some(p),
        "--selection" => flags.selection = Some(p),
        "--code-commit" => flags.code_commit = Some(p.to_string_lossy().into_owned()),
        "--endpoint" => flags.endpoint = Some(p.to_string_lossy().into_owned()),
        _ => unreachable!(),
    }
    Ok(())
}

fn parse_flags(args: &[String], required: &[&str]) -> Result<Flags, String> {
    let mut flags = Flags::default();
    let mut i = 0;
    while i < args.len() {
        let known = [
                "--trajectories",
                "--summary",
                "--selection",
                "--code-commit",
                "--endpoint",
            ]
            .iter()
            .any(|r| *r == args[i]);
        if !known {
            return Err(format!("unknown argument {:?}", args[i]));
        }
        take(&mut flags, &args[i], args, &mut i)?;
        i += 1; // step over the consumed value
    }
    for r in required {
        let set = match *r {
            "--trajectories" => flags.trajectories.is_some(),
            "--summary" => flags.summary.is_some(),
            "--selection" => flags.selection.is_some(),
            "--code-commit" => flags.code_commit.is_some(),
            _ => unreachable!(),
        };
        if !set {
            return Err(format!("missing {r}"));
        }
    }
    Ok(flags)
}

fn read_lines(path: &std::path::Path) -> Result<Vec<Value>, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line)
            .map_err(|e| format!("{}: line {}: invalid JSON: {e}", path.display(), i + 1))?;
        out.push(v);
    }
    Ok(out)
}

fn read_json(path: &std::path::Path, what: &str) -> Result<Value, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {what} {}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("{what} is not valid JSON: {e}"))
}

fn cmd_calibrate(flags: Flags) -> Result<(), String> {
    let config = experiment::CalibrateConfig {
        trajectories: flags
            .trajectories
            .ok_or("calibrate requires --trajectories")?,
        summary: flags.summary.ok_or("calibrate requires --summary")?,
        selection: flags.selection.ok_or("calibrate requires --selection")?,
        code_commit: flags
            .code_commit
            .ok_or("calibrate requires --code-commit")?,
        endpoint: flags.endpoint,
    };
    experiment::calibrate(&config)
}

fn cmd_evaluate(flags: Flags) -> Result<(), String> {
    let config = experiment::EvaluateConfig {
        selection: flags.selection.ok_or("evaluate requires --selection")?,
        trajectories: flags
            .trajectories
            .ok_or("evaluate requires --trajectories")?,
        summary: flags.summary.ok_or("evaluate requires --summary")?,
        code_commit: flags.code_commit.ok_or("evaluate requires --code-commit")?,
        endpoint: flags.endpoint,
    };
    experiment::evaluate(&config)
}

fn cmd_verify_calibration(flags: Flags) -> Result<(), String> {
    let traj_path = flags
        .trajectories
        .ok_or("verify-calibration requires --trajectories")?;
    let summary_path = flags
        .summary
        .ok_or("verify-calibration requires --summary")?;
    let sel_path = flags
        .selection
        .ok_or("verify-calibration requires --selection")?;

    let record_values = read_lines(&traj_path)?;
    let summary_value = read_json(&summary_path, "summary")?;
    let manifest_value = read_json(&sel_path, "selection manifest")?;

    let dir = experiment::crate_dir();
    let registry = experiment::load_tasks(&dir.join("tasks.json"))
        .map_err(|e| format!("cannot load task registry: {e}"))?
        .tasks;
    let stress = experiment::load_stress_registry(&dir.join("stress-levels.json"))
        .map_err(|e| format!("cannot load stress registry: {e}"))?;

    let baseline_hash = experiment::hash_file(&dir.join("prompts/baseline.md"))
        .map_err(|e| format!("cannot hash baseline prompt: {e}"))?;
    let repair_hash = experiment::hash_file(&dir.join("prompts/repair.md"))
        .map_err(|e| format!("cannot hash repair prompt: {e}"))?;
    let task_hash = experiment::task_registry_sha256(&registry)
        .map_err(|e| format!("cannot hash task registry: {e}"))?;
    let stress_hash = experiment::stress_registry_sha256(&stress.levels)
        .map_err(|e| format!("cannot hash stress registry: {e}"))?;
    let cal_bytes = std::fs::read(&traj_path)
        .map_err(|e| format!("cannot read calibration trajectories: {e}"))?;
    let cal_file_sha = crate::protocol::sha256_bytes(&cal_bytes);

    let errors = trace::verify_calibration_artifacts(
        &record_values,
        &summary_value,
        &manifest_value,
        &registry,
        &baseline_hash,
        &repair_hash,
        &task_hash,
        &stress_hash,
        &cal_file_sha,
    );

    if errors.is_empty() {
        println!("calibration verification: PASSED");
        println!("  records: {}", record_values.len());
        println!("  summary: {}", summary_path.display());
        println!("  selection: {}", sel_path.display());
        Ok(())
    } else {
        println!("calibration verification: FAILED ({} issues)", errors.len());
        for e in &errors {
            println!("  - {e}");
        }
        Err(format!("{} issue(s) found", errors.len()))
    }
}

fn cmd_verify_evaluation(flags: Flags) -> Result<(), String> {
    let sel_path = flags
        .selection
        .ok_or("verify-evaluation requires --selection")?;
    let traj_path = flags
        .trajectories
        .ok_or("verify-evaluation requires --trajectories")?;
    let summary_path = flags
        .summary
        .ok_or("verify-evaluation requires --summary")?;

    let manifest_value = read_json(&sel_path, "selection manifest")?;
    let record_values = read_lines(&traj_path)?;
    let summary_value = read_json(&summary_path, "summary")?;

    let dir = experiment::crate_dir();
    let registry = experiment::load_tasks(&dir.join("tasks.json"))
        .map_err(|e| format!("cannot load task registry: {e}"))?
        .tasks;

    let baseline_hash = experiment::hash_file(&dir.join("prompts/baseline.md"))
        .map_err(|e| format!("cannot hash baseline prompt: {e}"))?;
    let repair_hash = experiment::hash_file(&dir.join("prompts/repair.md"))
        .map_err(|e| format!("cannot hash repair prompt: {e}"))?;
    let task_hash = experiment::task_registry_sha256(&registry)
        .map_err(|e| format!("cannot hash task registry: {e}"))?;
    let sel_bytes =
        std::fs::read(&sel_path).map_err(|e| format!("cannot read selection manifest: {e}"))?;
    let sel_file_sha = crate::protocol::sha256_bytes(&sel_bytes);

    let errors = trace::verify_evaluation_artifacts(
        &record_values,
        &summary_value,
        &manifest_value,
        &registry,
        &baseline_hash,
        &repair_hash,
        &task_hash,
        &sel_file_sha,
    );

    if errors.is_empty() {
        println!("evaluation verification: PASSED");
        println!("  records: {}", record_values.len());
        println!("  summary: {}", summary_path.display());
        Ok(())
    } else {
        println!("evaluation verification: FAILED ({} issues)", errors.len());
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
        "self-test" => experiment::self_test(),
        "calibrate" => parse_flags(
            &rest,
            &[
                "--trajectories",
                "--summary",
                "--selection",
                "--code-commit",
            ],
        )
        .and_then(cmd_calibrate),
        "evaluate" => parse_flags(
            &rest,
            &["--selection", "--trajectories", "--summary", "--code-commit"],
        )
        .and_then(cmd_evaluate),
        "verify-calibration" => parse_flags(
            &rest,
            &["--trajectories", "--summary", "--selection"],
        )
        .and_then(cmd_verify_calibration),
        "verify-evaluation" => {
            parse_flags(&rest, &["--selection", "--trajectories", "--summary"])
                .and_then(cmd_verify_evaluation)
        }
        _ => Err(
            "usage: mutagen-exp-0008 <self-test | calibrate --trajectories P --summary P --selection P --code-commit SHA [--endpoint REDACTED] | evaluate --selection P --trajectories P --summary P --code-commit SHA [--endpoint REDACTED] | verify-calibration --trajectories P --summary P --selection P | verify-evaluation --selection P --trajectories P --summary P>"
                .to_string(),
        ),
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_accepts_provenance_flags() {
        let flags = parse_flags(
            &[
                "--trajectories".into(),
                "t.jsonl".into(),
                "--summary".into(),
                "s.json".into(),
                "--selection".into(),
                "m.json".into(),
                "--code-commit".into(),
                "a".repeat(40).into(),
                "--endpoint".into(),
                "http://redacted/v1".into(),
            ],
            &["--trajectories", "--summary", "--selection", "--code-commit"],
        )
        .unwrap();
        assert_eq!(flags.trajectories.as_deref(), Some(std::path::Path::new("t.jsonl")));
        assert_eq!(flags.summary.as_deref(), Some(std::path::Path::new("s.json")));
        assert_eq!(flags.selection.as_deref(), Some(std::path::Path::new("m.json")));
        assert_eq!(flags.code_commit.as_deref(), Some("a".repeat(40).as_str()));
        assert_eq!(flags.endpoint.as_deref(), Some("http://redacted/v1"));
    }

    #[test]
    fn parser_reports_missing_required_flag() {
        let err = parse_flags(
            &["--trajectories".into(), "t.jsonl".into()],
            &["--trajectories", "--summary", "--selection", "--code-commit"],
        )
        .unwrap_err();
        assert!(err.contains("--summary"), "{err}");
    }
}
