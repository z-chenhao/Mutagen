//! Experiment 0013 runner: Frozen End-to-End Self-Evolution
//! Confirmation (Evidence → Generate → Select → Promote).
//!
//! Usage (all commands are relative to the repository):
//!
//! Network-free:
//!   self-test
//!   preflight [--run-manifest P]
//!   init-run [--commit <A0-SHA>] [--manifest P]
//!   verify-run-manifest [--manifest P]
//!   verify-stage-freeze
//!   print-a0
//!   verify-discovery | verify-mutation | verify-selection | verify-promotion
//!
//! Live (each verifies the source freeze AND the previous stage's
//! commit-level freeze BEFORE its first model request):
//!   discover --run-manifest P     Stage B
//!   generate --run-manifest P     Stage C
//!   select   --run-manifest P     Stage D
//!   promote  --run-manifest P     Stage E
//!
//! Endpoint discipline (0013 formal protocol condition): these four
//! commands are the ONLY live-model commands in this binary. There is
//! NO `run --task`, NO smoke-model, NO debug-live, NO probe-endpoint:
//! after the Stage A1 commit, zero unregistered live requests may be
//! made to the registered endpoint by this binary.
//!
//! Artifacts (runtime evidence, crate directory): run-manifest.json,
//! mutation-input.json, candidate-pool.json, selected-candidate.json,
//! stage-b-freeze.json, stage-c-freeze.json, stage-d-freeze.json.
//! Raw trajectories/summaries: docs/experiments/artifacts/0013-*.
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
mod stage_freeze;
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

/// Every command the binary registers, with its live-ness. This is the
/// machine-readable command table the endpoint-discipline check
/// (preflight / self-test) asserts against: the live set is exactly
/// {discover, generate, select, promote} and no ad-hoc live command
/// (run / smoke-model / debug-live / probe-endpoint) may exist.
pub fn registered_commands() -> Vec<(&'static str, bool)> {
    vec![
        ("self-test", false),
        ("preflight", false),
        ("init-run", false),
        ("verify-run-manifest", false),
        ("verify-stage-freeze", false),
        ("print-a0", false),
        ("verify-discovery", false),
        ("verify-mutation", false),
        ("verify-selection", false),
        ("verify-promotion", false),
        ("discover", true),
        ("generate", true),
        ("select", true),
        ("promote", true),
        ("help", false),
    ]
}

fn usage() {
    println!(
        "mutagen-exp-0013 — Frozen End-to-End Self-Evolution Confirmation\n\
         \n\
         network-free commands:\n\
           self-test                     full network-free self-test (source freeze, stage freezes,\n\
                                         mutation schema, schedules, synthetic e2e, tamper suite)\n\
           preflight [--run-manifest P]  network-free readiness check (design arithmetic, explicit\n\
                                         mutator schema, CLI discipline, task splits, freeze mechanism)\n\
           init-run [--commit SHA]       Stage A1: write run-manifest.json from the frozen files\n\
           verify-run-manifest [--manifest P]\n\
           verify-stage-freeze           verify every present stage-B/C/D freeze against Git\n\
           print-a0                      print the Stage A0 git commands\n\
           verify-discovery | verify-mutation | verify-selection | verify-promotion\n\
         \n\
         live commands (the ONLY live-model commands; each is protocol-guarded):\n\
           discover --run-manifest P     Stage B: 18 incumbent episodes under the frozen 0009 stress\n\
           generate --run-manifest P     Stage C: 4 prompt-suffix candidates (explicit JSON schema, <=2 attempts)\n\
           select   --run-manifest P     Stage D: 150 episodes (3 tasks x 10 reps x 5 conditions)\n\
           promote  --run-manifest P     Stage E: 96 episodes (6 tasks x 8 reps x 2 conditions)\n\
         \n\
         endpoint discipline: after the A1 commit there is NO ad-hoc live command in this\n\
         binary (the 0011 `run --task` smoke command does not exist in 0013);\n\
         before every live stage the source freeze AND the previous stage's committed\n\
         stage-freeze are re-verified; a crashed stage is never restarted."
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

    // Endpoint discipline, enforced at dispatch: a command that is not
    // registered is unknown (no live path), and the registered live
    // set is exactly the four protocol-guarded stages.
    let known = registered_commands()
        .iter()
        .any(|(name, _)| *name == command);
    if !known {
        eprintln!(
            "unknown command: {command} (registered: self-test, preflight, init-run, verify-run-manifest, verify-stage-freeze, print-a0, verify-*, discover, generate, select, promote)"
        );
        usage();
        return ExitCode::FAILURE;
    }

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
        "verify-stage-freeze" => experiment::cmd_verify_stage_freeze(),
        "print-a0" => {
            experiment::print_a0_commands(
                "exp: preregister deadline-safe 0013 self-evolution confirmation",
            );
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
                experiment::cmd_discover(experiment::DiscoverArgs { manifest: m })
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
        "help" => {
            usage();
            Ok(())
        }
        _ => {
            eprintln!("unknown command: {command}");
            eprintln!("run: cargo run -p mutagen-exp-0013 -- help");
            Err("unknown command".to_string())
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
