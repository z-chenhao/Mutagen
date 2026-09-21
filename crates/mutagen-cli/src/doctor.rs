//! `mutagen doctor`: lightweight, deterministic, local sanity checks.
//!
//! All checks run on the local machine only: no network access, no API
//! keys, no model calls. The output is stable enough to be useful in CI
//! debugging.

use mutagen_runtime::VERSION as RUNTIME_VERSION;
use mutagen_runtime::core::{EpisodeId, VERSION as CORE_VERSION};

/// Minimum rustc version required by the workspace (`rust-version`).
const MIN_RUSTC: (u16, u16) = (1, 85);

/// One named check and its outcome.
#[derive(Debug)]
struct Check {
    name: &'static str,
    passed: bool,
    detail: String,
}

impl Check {
    fn pass(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            passed: true,
            detail: detail.into(),
        }
    }

    fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            passed: false,
            detail: detail.into(),
        }
    }
}

/// Run all checks, print a deterministic table, and return the exit code.
#[must_use]
pub fn run() -> std::process::ExitCode {
    let checks = vec![
        check_toolchain(),
        check_episode_identity(),
        check_workspace_wiring(),
    ];

    println!("mutagen doctor");
    let width = checks.iter().map(|c| c.name.len()).max().unwrap_or(0) + 2;
    let mut all_passed = true;
    for check in &checks {
        all_passed &= check.passed;
        let status = if check.passed { "ok  " } else { "FAIL" };
        println!("{status} | {:width$} | {}", check.name, check.detail);
    }
    if all_passed {
        println!("All checks passed.");
        std::process::ExitCode::SUCCESS
    } else {
        println!("One or more checks failed.");
        std::process::ExitCode::FAILURE
    }
}

fn check_toolchain() -> Check {
    const NAME: &str = "toolchain";
    let Ok(output) = std::process::Command::new("rustc")
        .arg("--version")
        .output()
    else {
        return Check::fail(NAME, "rustc not found on PATH");
    };
    if !output.status.success() {
        return Check::fail(NAME, "rustc --version failed");
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // "rustc 1.85.1" → (1, 85)
    let parts: Vec<&str> = version.split_whitespace().collect();
    let Some((major, minor)) = parts.get(1).and_then(|p| parse_semver_prefix(p)) else {
        return Check::fail(NAME, format!("unparseable rustc version: {version}"));
    };
    if (major, minor) < MIN_RUSTC {
        return Check::fail(
            NAME,
            format!("rustc {major}.{minor} is older than required 1.85"),
        );
    }
    Check::pass(NAME, version)
}

fn parse_semver_prefix(s: &str) -> Option<(u16, u16)> {
    let mut parts = s.split('.');
    let major = parts.next()?.parse::<u16>().ok()?;
    let minor = parts.next()?.parse::<u16>().ok()?;
    Some((major, minor))
}

fn check_episode_identity() -> Check {
    const NAME: &str = "core: episode id";
    let id = EpisodeId::new("2026-01-15-000000-000001");
    let rendered = id.to_string();
    if rendered == "2026-01-15-000000-000001" {
        Check::pass(NAME, format!("opaque id {rendered}"))
    } else {
        Check::fail(NAME, format!("unexpected rendering: {rendered}"))
    }
}

fn check_workspace_wiring() -> Check {
    const NAME: &str = "workspace: wiring";
    // This function only compiles if mutagen-cli → mutagen-runtime →
    // mutagen-core all link. That is all it claims: a link check, not an
    // inspection of the dependency graph.
    Check::pass(
        NAME,
        format!(
            "cli v{} → runtime v{} → core v{}",
            env!("CARGO_PKG_VERSION"),
            RUNTIME_VERSION,
            CORE_VERSION
        ),
    )
}
