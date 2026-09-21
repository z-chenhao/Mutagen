//! Experiment 0006 runner.
//!
//! Usage:
//!   run --repetitions 3 --trajectories PATH --summary PATH
//!   verify-artifacts --trajectories PATH --summary PATH
//!   recompute-summary --trajectories PATH --summary PATH
//!
//! `recompute-summary` regenerates the derived summary from the immutable
//! raw trajectories (no model calls); it writes only the summary file.
//!
//! `run` requires MUTAGEN_EXP_BASE_URL and MUTAGEN_EXP_MODEL
//! (MUTAGEN_EXP_API_KEY optional). If they are missing, the experiment
//! is not executed and no fallback model is substituted.
//!
//! This binary is presentation/control: argument parsing, artifact
//! file handling, and the artifact verifier. None of that logic lives
//! in the kernel.

#![forbid(unsafe_code)]

mod experiment;
mod kernel;
mod model;
mod protocol;
mod tools;
mod trace;

use std::path::PathBuf;
use std::process::ExitCode;

struct Flags {
    repetitions: u32,
    trajectories: Option<PathBuf>,
    summary: Option<PathBuf>,
}

fn parse_flags(args: &[String]) -> Result<Flags, String> {
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

    let repetitions = repetitions.ok_or("missing --repetitions")?;
    let trajectories = trajectories.ok_or("missing --trajectories")?;
    let summary = summary.ok_or("missing --summary")?;

    Ok(Flags {
        repetitions,
        trajectories: Some(trajectories),
        summary: Some(summary),
    })
}

fn print_usage() {
    eprintln!(
        "usage:\n  mutagen-exp-0006 run --repetitions N --trajectories PATH --summary PATH\n  mutagen-exp-0006 verify-artifacts --trajectories PATH --summary PATH\n  mutagen-exp-0006 recompute-summary --trajectories PATH --summary PATH\n\nenvironment (run):\n  MUTAGEN_EXP_BASE_URL  required (OpenAI-compatible endpoint, e.g. http://127.0.0.1:8000/v1)\n  MUTAGEN_EXP_MODEL     required (Qwen model id)\n  MUTAGEN_EXP_API_KEY   optional"
    );
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        print_usage();
        return ExitCode::from(2);
    }

    let result = match args[0].as_str() {
        "run" => {
            let flags = match parse_flags(&args[1..]) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("argument error: {e}");
                    print_usage();
                    return ExitCode::from(2);
                }
            };
            experiment::run(&experiment::RunConfig {
                repetitions: flags.repetitions,
                trajectories: flags.trajectories.unwrap(),
                summary: flags.summary.unwrap(),
            })
        }
        "verify-artifacts" => verify_artifacts_cmd(&args[1..]),
        "recompute-summary" => recompute_summary_cmd(&args[1..]),
        other => {
            eprintln!("unknown command {other:?}");
            print_usage();
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

/// Regenerate the derived summary from the immutable raw trajectories.
/// No model calls; the trajectories file is read, never written.
fn recompute_summary_cmd(args: &[String]) -> Result<(), String> {
    let mut trajectories: Option<String> = None;
    let mut summary: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--trajectories" => {
                i += 1;
                trajectories = Some(args[i].clone());
            }
            "--summary" => {
                i += 1;
                summary = Some(args[i].clone());
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
        i += 1;
    }
    let trajectories = trajectories.ok_or("missing --trajectories")?;
    let summary = summary.ok_or("missing --summary")?;

    let raw = std::fs::read_to_string(&trajectories)
        .map_err(|e| format!("cannot read {trajectories}: {e}"))?;
    let mut values = Vec::new();
    for (n, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value =
            serde_json::from_str(line).map_err(|e| format!("line {}: {e}", n + 1))?;
        values.push(v);
    }
    if values.is_empty() {
        return Err("trajectories file contains no records".into());
    }
    let records: Vec<trace::EpisodeRecord> = values
        .iter()
        .map(|v| serde_json::from_value(v.clone()))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("record does not match the 0006 schema: {e}"))?;

    let s = trace::compute_summary(&records);
    let json = serde_json::to_string_pretty(&s).map_err(|e| e.to_string())?;
    std::fs::write(&summary, json + "\n").map_err(|e| format!("cannot write {summary}: {e}"))?;

    println!(
        "recomputed summary for {} records: conclusion={} eligible={}/{} pairs={}/{} prompt_tokens={} completion_tokens={}",
        s.total_episodes,
        s.conclusion.as_str(),
        s.eligible_episodes,
        s.total_episodes,
        s.eligible_pairs,
        s.eligible_pairs + s.ineligible_pairs,
        s.usage.prompt_tokens,
        s.usage.completion_tokens
    );
    Ok(())
}

fn verify_artifacts_cmd(args: &[String]) -> Result<(), String> {
    let mut trajectories: Option<String> = None;
    let mut summary: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--trajectories" => {
                i += 1;
                trajectories = Some(args[i].clone());
            }
            "--summary" => {
                i += 1;
                summary = Some(args[i].clone());
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
        i += 1;
    }
    let trajectories = trajectories.ok_or("missing --trajectories")?;
    let summary = summary.ok_or("missing --summary")?;

    let raw = std::fs::read_to_string(&trajectories)
        .map_err(|e| format!("cannot read {trajectories}: {e}"))?;
    let mut values = Vec::new();
    for (n, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value =
            serde_json::from_str(line).map_err(|e| format!("line {}: {e}", n + 1))?;
        values.push(v);
    }
    let summary_value: serde_json::Value = std::fs::read_to_string(&summary)
        .map_err(|e| format!("cannot read {summary}: {e}"))?
        .parse()
        .map_err(|e| format!("summary is not valid JSON: {e}"))?;

    let result = trace::verify_artifacts(&values, &summary_value);
    for (name, ok, detail) in &result.checks {
        let mark = if *ok { "PASS" } else { "FAIL" };
        let suffix = if detail.is_empty() {
            String::new()
        } else {
            format!("  ({detail})")
        };
        println!("{mark}  {name}{suffix}");
    }
    if result.ok {
        println!(
            "artifact verification: PASS ({} checks)",
            result.checks.len()
        );
        Ok(())
    } else {
        println!("artifact verification: FAIL");
        Err("one or more artifact checks failed".into())
    }
}
