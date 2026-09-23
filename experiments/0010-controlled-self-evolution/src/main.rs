//! Experiment 0010 runner: the first controlled self-evolution loop.
//!
//! Usage:
//!   self-test
//!   discover        --trajectories P --summary P --mutation-input P [--code-commit SHA] [--endpoint REDACTED]
//!   verify-discovery --trajectories P --summary P --mutation-input P
//!   generate        --mutation-input P --generation-artifact P --candidate-pool P [--code-commit SHA] [--endpoint REDACTED]
//!   verify-mutation --generation-artifact P --candidate-pool P --mutation-input P
//!   select          --candidate-pool P --trajectories P --summary P --selected P [--code-commit SHA] [--endpoint REDACTED]
//!   verify-selection --candidate-pool P --trajectories P --summary P --selected P --mutation-input P
//!   verify-selection-freeze (same checks; the post-freeze boundary)
//!   promote         --selected P --trajectories P --summary P [--code-commit SHA] [--endpoint REDACTED]
//!   verify-promotion --selected P --candidate-pool P --trajectories P --summary P --mutation-input P
//!
//! `discover`/`generate`/`select`/`promote` require MUTAGEN_EXP_BASE_URL
//! and MUTAGEN_EXP_MODEL (MUTAGEN_EXP_API_KEY optional). If they are
//! missing, the experiment is not executed and no fallback model is
//! substituted. `--code-commit` defaults to `git rev-parse HEAD`.
//!
//! This binary is presentation/control only. None of the kernel,
//! oracle, selection, or mutation semantics lives here.

#![forbid(unsafe_code)]

mod experiment;
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

use serde_json::Value;

#[derive(Debug, Default)]
struct Flags {
    trajectories: Option<PathBuf>,
    summary: Option<PathBuf>,
    mutation_input: Option<PathBuf>,
    generation_artifact: Option<PathBuf>,
    candidate_pool: Option<PathBuf>,
    selected: Option<PathBuf>,
    code_commit: Option<String>,
    endpoint: Option<String>,
}

const KNOWN: &[&str] = &[
    "--trajectories",
    "--summary",
    "--mutation-input",
    "--generation-artifact",
    "--candidate-pool",
    "--selected",
    "--code-commit",
    "--endpoint",
];

fn parse_flags(args: &[String], required: &[&str]) -> Result<Flags, String> {
    let mut flags = Flags::default();
    let mut i = 0;
    while i < args.len() {
        if !KNOWN.contains(&args[i].as_str()) {
            return Err(format!("unknown argument {:?}", args[i]));
        }
        let v = args
            .get(i + 1)
            .ok_or_else(|| format!("{} needs a value", args[i]))?;
        let p = PathBuf::from(v);
        i += 1;
        match args[i - 1].as_str() {
            "--trajectories" => flags.trajectories = Some(p),
            "--summary" => flags.summary = Some(p),
            "--mutation-input" => flags.mutation_input = Some(p),
            "--generation-artifact" => flags.generation_artifact = Some(p),
            "--candidate-pool" => flags.candidate_pool = Some(p),
            "--selected" => flags.selected = Some(p),
            "--code-commit" => flags.code_commit = Some(p.to_string_lossy().into_owned()),
            "--endpoint" => flags.endpoint = Some(p.to_string_lossy().into_owned()),
            _ => unreachable!(),
        }
        i += 1;
    }
    for r in required {
        let set = match *r {
            "--trajectories" => flags.trajectories.is_some(),
            "--summary" => flags.summary.is_some(),
            "--mutation-input" => flags.mutation_input.is_some(),
            "--generation-artifact" => flags.generation_artifact.is_some(),
            "--candidate-pool" => flags.candidate_pool.is_some(),
            "--selected" => flags.selected.is_some(),
            _ => true,
        };
        if !set {
            return Err(format!("missing {r}"));
        }
    }
    Ok(flags)
}

fn read_lines(path: &std::path::Path, what: &str) -> Result<Vec<Value>, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {what} {}: {e}", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line)
            .map_err(|e| format!("{what}: line {}: invalid JSON: {e}", i + 1))?;
        out.push(v);
    }
    Ok(out)
}

fn read_json(path: &std::path::Path, what: &str) -> Result<Value, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {what} {}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("{what} is not valid JSON: {e}"))
}

/// File byte hashes + frozen config for the verifiers.
struct VerifyEnv {
    ctx: trace::VerifierContext,
    input_file_sha: Option<String>,
    pool_file_sha: Option<String>,
    selected_file_sha: Option<String>,
}

fn verify_env(flags: &Flags) -> Result<VerifyEnv, String> {
    let dir = experiment::crate_dir();
    let incumbent = experiment::load_prompt(&dir.join("prompts/incumbent.md"))?;
    let incumbent_sha = experiment::hash_file(&dir.join("prompts/incumbent.md"))?;
    let mutator_sha = experiment::hash_file(&dir.join("prompts/mutator.md"))?;
    let registry = experiment::load_tasks(&dir.join("tasks.json"))?.tasks;
    let profile_raw = std::fs::read_to_string(dir.join("stress-profile.json"))
        .map_err(|e| format!("cannot read stress profile: {e}"))?;
    let profile: Value =
        serde_json::from_str(&profile_raw).map_err(|e| format!("stress profile invalid: {e}"))?;
    let mut env = VerifyEnv {
        ctx: trace::VerifierContext::new(incumbent, incumbent_sha, mutator_sha, registry, profile),
        input_file_sha: None,
        pool_file_sha: None,
        selected_file_sha: None,
    };
    if let Some(p) = &flags.mutation_input {
        if p.exists() {
            env.input_file_sha = Some(crate::protocol::sha256_bytes(
                &std::fs::read(p).map_err(|e| format!("cannot read mutation input: {e}"))?,
            ));
        }
    }
    if let Some(p) = &flags.candidate_pool {
        if p.exists() {
            env.pool_file_sha = Some(crate::protocol::sha256_bytes(
                &std::fs::read(p).map_err(|e| format!("cannot read candidate pool: {e}"))?,
            ));
        }
    }
    if let Some(p) = &flags.selected {
        if p.exists() {
            env.selected_file_sha = Some(crate::protocol::sha256_bytes(
                &std::fs::read(p).map_err(|e| format!("cannot read selected candidate: {e}"))?,
            ));
        }
    }
    Ok(env)
}

fn report(phase: &str, errors: &[String]) -> Result<(), String> {
    if errors.is_empty() {
        println!("{phase} verification: PASSED");
        Ok(())
    } else {
        println!("{phase} verification: FAILED ({} issues)", errors.len());
        for e in errors {
            println!("  - {e}");
        }
        Err(format!(
            "{phase} verification: {len} issue(s)",
            len = errors.len()
        ))
    }
}

fn cmd_self_test() -> Result<(), String> {
    experiment::self_test()
}

fn cmd_discover(flags: Flags) -> Result<(), String> {
    let config = experiment::DiscoverConfig {
        trajectories: flags
            .trajectories
            .ok_or("discover requires --trajectories")?,
        summary: flags.summary.ok_or("discover requires --summary")?,
        mutation_input: flags
            .mutation_input
            .ok_or("discover requires --mutation-input")?,
        code_commit: flags.code_commit,
        endpoint: flags.endpoint,
    };
    experiment::discover(&config)
}

fn cmd_verify_discovery(flags: Flags) -> Result<(), String> {
    let env = verify_env(&flags)?;
    let traj = flags
        .trajectories
        .ok_or("verify-discovery requires --trajectories")?;
    let sum = flags.summary.ok_or("verify-discovery requires --summary")?;
    let input = flags
        .mutation_input
        .ok_or("verify-discovery requires --mutation-input")?;
    let records_v = read_lines(&traj, "discovery trajectories")?;
    let records: Vec<trace::DiscoveryRecord> = records_v
        .iter()
        .map(|v| {
            serde_json::from_value(v.clone())
                .map_err(|e| format!("discovery record is not well-formed: {e}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let summary_v = read_json(&sum, "discovery summary")?;
    let input_v = read_json(&input, "mutation input")?;
    let input_sha = env
        .input_file_sha
        .as_deref()
        .ok_or("mutation input file is missing")?;
    let errors = trace::verify_discovery(&records, &summary_v, &input_v, input_sha, &env.ctx);
    report("discovery", &errors)
}

fn cmd_generate(flags: Flags) -> Result<(), String> {
    let dir = experiment::crate_dir();
    let config = experiment::GenerateConfig {
        mutation_input: flags
            .mutation_input
            .ok_or("generate requires --mutation-input")?,
        generation_artifact: flags
            .generation_artifact
            .ok_or("generate requires --generation-artifact")?,
        candidate_pool: flags
            .candidate_pool
            .unwrap_or_else(|| dir.join("candidate-pool.json")),
        code_commit: flags.code_commit,
        endpoint: flags.endpoint,
    };
    experiment::generate(&config)
}

fn cmd_verify_mutation(flags: Flags) -> Result<(), String> {
    let dir = experiment::crate_dir();
    let env = verify_env(&flags)?;
    let artifact = flags
        .generation_artifact
        .or_else(|| Some(dir.join("0010-mutation-generation.json")));
    let pool_path = flags
        .candidate_pool
        .or_else(|| Some(dir.join("candidate-pool.json")));
    let input_path = flags
        .mutation_input
        .or_else(|| Some(dir.join("mutation-input.json")));
    let (Some(artifact), Some(pool_path), Some(input_path)) = (artifact, pool_path, input_path)
    else {
        return Err(
            "verify-mutation requires --generation-artifact/--candidate-pool/--mutation-input"
                .to_string(),
        );
    };
    if !artifact.exists() || !pool_path.exists() {
        return Err(
            "the mutation generation artifact / candidate pool does not exist — run `generate` first (or the experiment stopped as inconclusive)".to_string(),
        );
    }
    let gen_v = read_json(&artifact, "generation artifact")?;
    let pool: mutation::CandidatePool =
        serde_json::from_value(read_json(&pool_path, "candidate pool")?)
            .map_err(|e| format!("candidate pool is not well-formed: {e}"))?;
    let input_v = read_json(&input_path, "mutation input")?;
    let input_sha = env
        .input_file_sha
        .as_deref()
        .ok_or("mutation input file is missing")?;
    let errors = trace::verify_mutation(&gen_v, &pool, &input_v, input_sha, &env.ctx);
    report("mutation-generation", &errors)
}

fn cmd_select(flags: Flags) -> Result<(), String> {
    let dir = experiment::crate_dir();
    let config = experiment::SelectConfig {
        candidate_pool: flags
            .candidate_pool
            .unwrap_or_else(|| dir.join("candidate-pool.json")),
        trajectories: flags.trajectories.ok_or("select requires --trajectories")?,
        summary: flags.summary.ok_or("select requires --summary")?,
        selected: flags
            .selected
            .unwrap_or_else(|| dir.join("selected-candidate.json")),
        code_commit: flags.code_commit,
        endpoint: flags.endpoint,
    };
    experiment::select(&config)
}

fn cmd_verify_selection(flags: Flags) -> Result<(), String> {
    let dir = experiment::crate_dir();
    let env = verify_env(&flags)?;
    let traj = flags
        .trajectories
        .ok_or("verify-selection requires --trajectories")?;
    let sum = flags.summary.ok_or("verify-selection requires --summary")?;
    let sel = flags
        .selected
        .or_else(|| Some(dir.join("selected-candidate.json")));
    let pool_path = flags
        .candidate_pool
        .or_else(|| Some(dir.join("candidate-pool.json")));
    let input_path = flags
        .mutation_input
        .or_else(|| Some(dir.join("mutation-input.json")));
    let (Some(sel), Some(pool_path), Some(_input_path)) = (sel, pool_path, input_path) else {
        return Err(
            "verify-selection requires --trajectories/--summary/--selected/--candidate-pool/--mutation-input"
                .to_string(),
        );
    };
    let records_v = read_lines(&traj, "selection trajectories")?;
    let records: Vec<trace::SelectionRecord> = records_v
        .iter()
        .map(|v| {
            serde_json::from_value(v.clone())
                .map_err(|e| format!("selection record is not well-formed: {e}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let summary_v = read_json(&sum, "selection summary")?;
    let selected_v = read_json(&sel, "selected candidate")?;
    let pool: mutation::CandidatePool =
        serde_json::from_value(read_json(&pool_path, "candidate pool")?)
            .map_err(|e| format!("candidate pool is not well-formed: {e}"))?;
    let pool_sha = env
        .pool_file_sha
        .as_deref()
        .ok_or("candidate pool file is missing")?;
    let input_sha = env
        .input_file_sha
        .as_deref()
        .ok_or("mutation input file is missing")?;
    let errors = trace::verify_selection(
        &records,
        &summary_v,
        &selected_v,
        &pool,
        pool_sha,
        input_sha,
        &env.ctx,
    );
    report("selection", &errors)
}

fn cmd_promote(flags: Flags) -> Result<(), String> {
    let dir = experiment::crate_dir();
    let config = experiment::PromoteConfig {
        selected: flags
            .selected
            .unwrap_or_else(|| dir.join("selected-candidate.json")),
        trajectories: flags
            .trajectories
            .ok_or("promote requires --trajectories")?,
        summary: flags.summary.ok_or("promote requires --summary")?,
        code_commit: flags.code_commit,
        endpoint: flags.endpoint,
    };
    experiment::promote(&config)
}

fn cmd_verify_promotion(flags: Flags) -> Result<(), String> {
    let dir = experiment::crate_dir();
    let env = verify_env(&flags)?;
    let traj = flags
        .trajectories
        .ok_or("verify-promotion requires --trajectories")?;
    let sum = flags.summary.ok_or("verify-promotion requires --summary")?;
    let sel = flags
        .selected
        .or_else(|| Some(dir.join("selected-candidate.json")));
    let pool_path = flags
        .candidate_pool
        .or_else(|| Some(dir.join("candidate-pool.json")));
    let input_path = flags
        .mutation_input
        .or_else(|| Some(dir.join("mutation-input.json")));
    let (Some(sel), Some(pool_path), Some(_input_path)) = (sel, pool_path, input_path) else {
        return Err(
            "verify-promotion requires --trajectories/--summary/--selected/--candidate-pool/--mutation-input"
                .to_string(),
        );
    };
    let records_v = read_lines(&traj, "promotion trajectories")?;
    let records: Vec<trace::PromotionRecord> = records_v
        .iter()
        .map(|v| {
            serde_json::from_value(v.clone())
                .map_err(|e| format!("promotion record is not well-formed: {e}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let summary_v = read_json(&sum, "promotion summary")?;
    let selected_v = read_json(&sel, "selected candidate")?;
    let pool: mutation::CandidatePool =
        serde_json::from_value(read_json(&pool_path, "candidate pool")?)
            .map_err(|e| format!("candidate pool is not well-formed: {e}"))?;
    let pool_sha = env
        .pool_file_sha
        .as_deref()
        .ok_or("candidate pool file is missing")?;
    let input_sha = env
        .input_file_sha
        .as_deref()
        .ok_or("mutation input file is missing")?;
    let selected_sha = env
        .selected_file_sha
        .as_deref()
        .ok_or("selected candidate file is missing")?;
    let errors = trace::verify_promotion(
        &records,
        &summary_v,
        &selected_v,
        &pool,
        pool_sha,
        input_sha,
        selected_sha,
        &env.ctx,
    );
    report("promotion", &errors)
}

fn usage() -> String {
    "usage: mutagen-exp-0010 <self-test | discover | verify-discovery | generate | verify-mutation | select | verify-selection | verify-selection-freeze | promote | verify-promotion> [flags]"
        .to_string()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();

    let required: &[&str] = match cmd {
        "self-test" => &[],
        "discover" => &["--trajectories", "--summary", "--mutation-input"],
        "verify-discovery" => &["--trajectories", "--summary", "--mutation-input"],
        "generate" => &["--mutation-input", "--generation-artifact"],
        "verify-mutation" => &[],
        "select" => &["--trajectories", "--summary"],
        "verify-selection" | "verify-selection-freeze" => &["--trajectories", "--summary"],
        "promote" => &["--trajectories", "--summary"],
        "verify-promotion" => &["--trajectories", "--summary"],
        _ => {
            eprintln!("{}", usage());
            return ExitCode::FAILURE;
        }
    };

    let outcome = match cmd {
        "self-test" => cmd_self_test(),
        "discover" => parse_flags(&rest, required).and_then(cmd_discover),
        "verify-discovery" => parse_flags(&rest, required).and_then(cmd_verify_discovery),
        "generate" => parse_flags(&rest, required).and_then(cmd_generate),
        "verify-mutation" => parse_flags(&rest, required).and_then(cmd_verify_mutation),
        "select" => parse_flags(&rest, required).and_then(cmd_select),
        "verify-selection" | "verify-selection-freeze" => {
            parse_flags(&rest, required).and_then(cmd_verify_selection)
        }
        "promote" => parse_flags(&rest, required).and_then(cmd_promote),
        "verify-promotion" => parse_flags(&rest, required).and_then(cmd_verify_promotion),
        _ => unreachable!(),
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
    fn parser_accepts_flags() {
        let flags = parse_flags(
            &[
                "--trajectories".into(),
                "t.jsonl".into(),
                "--summary".into(),
                "s.json".into(),
                "--mutation-input".into(),
                "m.json".into(),
                "--code-commit".into(),
                "a".repeat(40),
                "--endpoint".into(),
                "http://redacted/v1".into(),
            ],
            &["--trajectories", "--summary"],
        )
        .unwrap();
        assert_eq!(
            flags.trajectories.as_deref(),
            Some(std::path::Path::new("t.jsonl"))
        );
        assert_eq!(
            flags.mutation_input.as_deref(),
            Some(std::path::Path::new("m.json"))
        );
    }

    #[test]
    fn parser_reports_missing_required_flag() {
        let err = parse_flags(
            &["--trajectories".into(), "t.jsonl".into()],
            &["--trajectories", "--summary"],
        )
        .unwrap_err();
        assert!(err.contains("--summary"), "{err}");
    }

    #[test]
    fn parser_rejects_unknown_flag() {
        let err = parse_flags(&["--bogus".into(), "x".into()], &[]).unwrap_err();
        assert!(err.contains("unknown"), "{err}");
    }
}
