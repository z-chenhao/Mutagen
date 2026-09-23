//! Experiment 0011 runner: Clean Confirmatory Self-Evolution Rerun.
//!
//! Usage (all commands are relative to the repository):
//!
//! Network-free:
//!   self-test
//!   preflight [--run-manifest P]
//!   init-run [--commit <A0-SHA>] [--manifest P]
//!   verify-run-manifest --manifest P
//!   print-a0
//!   verify-discovery | verify-mutation | verify-selection | verify-promotion
//!
//! Live (each requires the Stage A1 run manifest; the source-freeze
//! guard runs BEFORE any model request):
//!   discover --run-manifest P [--family F] [--task ID]
//!   generate --run-manifest P
//!   select   --run-manifest P
//!   promote  --run-manifest P
//!
//! Ad-hoc smoke (NOT part of the experiment dataset):
//!   run [--task ID]
//!
//! Artifacts (written into the experiment crate directory):
//!   run-manifest.json, discovery-raw.jsonl, discovery-summary.json,
//!   mutation-input.json, mutation-generation.json, candidate-pool.json,
//!   selection-raw.jsonl, selection-summary.json, selected-candidate.json,
//!   promotion-raw.jsonl, promotion-summary.json
//!
//! This binary is presentation/control only. None of the kernel,
//! oracle, selection, or mutation semantics lives here.

#![forbid(unsafe_code)]

mod experiment;
mod freeze;
mod kernel;
mod model;
mod mutation;
mod oracle;
mod protocol;
mod selection;
mod stats;
mod tools;
mod trace;

use std::path::PathBuf;
use std::process::ExitCode;

fn get<'a>(flags: &'a [(String, String)], key: &str) -> Option<&'a str> {
    flags
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn usage() {
    println!(
        "mutagen-exp-0011 — Clean Confirmatory Self-Evolution Rerun\n\
         \n\
         commands:\n\
           self-test                     network-free full self-test (freeze guard, synthetic e2e, tamper suite)\n\
           preflight [--run-manifest P]  network-free readiness check (design, registry, profile, prompts, lock)\n\
           init-run [--commit SHA]       Stage A1: write run-manifest.json from the frozen files\n\
           verify-run-manifest --manifest P\n\
           print-a0                      print the Stage A0 git commands\n\
           discover --run-manifest P     Stage B: 18 incumbent episodes under the frozen 0009 stress\n\
           generate --run-manifest P     Stage C: 4 prompt-suffix candidates (same model, no tools)\n\
           select   --run-manifest P     Stage D: 150 episodes (3 tasks × 10 reps × 5 conditions)\n\
           promote  --run-manifest P     Stage E: 96 episodes (6 tasks × 8 reps × 2 conditions)\n\
           run [--task ID]               ad-hoc smoke run (never part of the experiment)\n\
         \n\
         every live stage verifies the source freeze BEFORE its first model request;\n\
         on any violation the run identity is permanently invalid (no patching, no continuation)."
    );
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut command = "help".to_string();
    let mut flags: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        if argv[i].starts_with("--") {
            let key = argv[i].trim_start_matches("--").to_string();
            if i + 1 < argv.len() && !argv[i + 1].starts_with("--") {
                flags.push((key, argv[i + 1].clone()));
                i += 1;
            } else {
                flags.push((key, String::new()));
            }
        } else if command == "help" {
            command = argv[i].clone();
        }
        i += 1;
    }
    let default_manifest = experiment::crate_dir().join("run-manifest.json");
    let manifest = |flags: &[(String, String)]| -> PathBuf {
        get(flags, "run-manifest")
            .map(PathBuf::from)
            .unwrap_or_else(|| default_manifest.clone())
    };

    let result = match command.as_str() {
        "self-test" => experiment::self_test(),
        "preflight" => {
            let m = get(&flags, "run-manifest").map(PathBuf::from);
            experiment::cmd_preflight(m.as_deref())
        }
        "init-run" => {
            let m = get(&flags, "manifest")
                .map(PathBuf::from)
                .unwrap_or_else(|| default_manifest.clone());
            experiment::cmd_init_run(experiment::InitRunArgs {
                commit: get(&flags, "commit").map(str::to_string),
                manifest: m,
            })
        }
        "verify-run-manifest" => {
            let m = get(&flags, "manifest")
                .map(PathBuf::from)
                .unwrap_or_else(|| default_manifest.clone());
            experiment::cmd_verify_run_manifest(&m)
        }
        "print-a0" => {
            experiment::print_a0_commands("exp: 0011 frozen experiment code (Stage A0)");
            Ok(())
        }
        "verify-discovery" => experiment::cmd_verify_discovery(),
        "verify-mutation" => experiment::cmd_verify_mutation(),
        "verify-selection" => experiment::cmd_verify_selection(),
        "verify-promotion" => experiment::cmd_verify_promotion(),
        "discover" => {
            let m = manifest(&flags);
            if !m.exists() {
                Err(format!(
                    "run manifest {} does not exist — run `init-run` (Stage A1) first",
                    m.display()
                ))
            } else {
                experiment::cmd_discover(experiment::DiscoverArgs {
                    manifest: m,
                    family: get(&flags, "family").map(str::to_string),
                    task: get(&flags, "task").map(str::to_string),
                })
            }
        }
        "generate" => {
            let m = manifest(&flags);
            if !m.exists() {
                Err(format!(
                    "run manifest {} does not exist — run `init-run` (Stage A1) first",
                    m.display()
                ))
            } else {
                experiment::cmd_generate(experiment::GenerateArgs { manifest: m })
            }
        }
        "select" => {
            let m = manifest(&flags);
            if !m.exists() {
                Err(format!(
                    "run manifest {} does not exist — run `init-run` (Stage A1) first",
                    m.display()
                ))
            } else {
                experiment::cmd_select(experiment::SelectArgs { manifest: m })
            }
        }
        "promote" => {
            let m = manifest(&flags);
            if !m.exists() {
                Err(format!(
                    "run manifest {} does not exist — run `init-run` (Stage A1) first",
                    m.display()
                ))
            } else {
                experiment::cmd_promote(experiment::PromoteArgs { manifest: m })
            }
        }
        "run" => experiment::cmd_run(experiment::RunArgs {
            task: get(&flags, "task").map(str::to_string),
        }),
        _ => {
            usage();
            Ok(())
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
