//! The mechanical source-freeze system (Experiment 0011).
//!
//! This is the most important change from Experiment 0010. 0010 was
//! declared inconclusive because the *executable source changed after
//! live model execution had begun* (a bugfix was committed
//! mid-selection). 0011 therefore enforces the freeze mechanically:
//!
//! 1. **Stage A0** — the frozen code commit. Every file in
//!    [`FROZEN_PATHS`] is committed as the code-under-test commit.
//! 2. **Stage A1** — `init-run` hashes every frozen file and writes
//!    `run-manifest.json` (committed before the first model request).
//! 3. **Before every live stage** (`discover` / `generate` / `select`
//!    / `promote`) the guard [`verify_freeze`] runs BEFORE the model
//!    client is constructed and BEFORE any request is built. If ANY
//!    check fails, the stage refuses to run and the run identity is
//!    permanently invalid — no patching, no continuation.
//!
//! The guard checks (all of them, always):
//! - **Current bytes**: every frozen file's current bytes hash to the
//!   SHA recorded in the run manifest.
//! - **Code-commit blob**: every frozen file matches the exact bytes
//!   stored at `code_under_test_commit` (`git show <commit>:<path>`).
//! - **No frozen-path commit after A0**: `git log <commit>..HEAD --
//!   <frozen paths>` must be EMPTY. A modify→commit→revert sequence
//!   still leaves a commit touching the path and still invalidates the
//!   run (net-diff checking alone is insufficient).
//! - **No working-tree drift**: frozen paths must have no staged or
//!   unstaged modifications (`git status --porcelain`).
//! - **Commit ancestry**: `code_under_test_commit` must be an ancestor
//!   of the current `HEAD`.
//! - **Manifest internal consistency**: the file list must be exactly
//!   the registered frozen paths, and the combined `frozen_design_sha256`
//!   must reproduce from the per-file hashes.
//!
//! There is NO bypass path: every real model request made by this
//! experiment flows through a stage command that runs this guard. The
//! network-free commands (`preflight`, `self-test`, `verify-*`) never
//! make model requests and do not need it.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::Serialize;

use crate::protocol::sha256_bytes;

/// The FROZEN DESIGN FILES. Exactly these paths may never change after
/// the Stage A0 frozen code commit. The results document is NOT frozen
/// (it is written after execution); the runtime artifacts
/// (`run-manifest.json`, `mutation-input.json`, `candidate-pool.json`,
/// `selected-candidate.json`, raw trajectories) are NOT frozen design
/// files — they are append-once run outputs.
pub const FROZEN_PATHS: &[&str] = &[
    "experiments/0011-confirmatory-self-evolution/Cargo.toml",
    "experiments/0011-confirmatory-self-evolution/Cargo.lock",
    "experiments/0011-confirmatory-self-evolution/design.json",
    "experiments/0011-confirmatory-self-evolution/tasks.json",
    "experiments/0011-confirmatory-self-evolution/stress-profile.json",
    "experiments/0011-confirmatory-self-evolution/prompts/incumbent.md",
    "experiments/0011-confirmatory-self-evolution/prompts/mutator.md",
    "experiments/0011-confirmatory-self-evolution/src/main.rs",
    "experiments/0011-confirmatory-self-evolution/src/protocol.rs",
    "experiments/0011-confirmatory-self-evolution/src/model.rs",
    "experiments/0011-confirmatory-self-evolution/src/kernel.rs",
    "experiments/0011-confirmatory-self-evolution/src/tools.rs",
    "experiments/0011-confirmatory-self-evolution/src/oracle.rs",
    "experiments/0011-confirmatory-self-evolution/src/mutation.rs",
    "experiments/0011-confirmatory-self-evolution/src/selection.rs",
    "experiments/0011-confirmatory-self-evolution/src/stats.rs",
    "experiments/0011-confirmatory-self-evolution/src/freeze.rs",
    "experiments/0011-confirmatory-self-evolution/src/trace.rs",
    "experiments/0011-confirmatory-self-evolution/src/experiment.rs",
    "docs/experiments/0011-confirmatory-self-evolution.md",
];

/// One frozen file and its byte hash at the Stage A0/A1 freeze.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenFile {
    /// Repository-relative path.
    pub path: String,
    /// SHA-256 hex of the exact file bytes.
    pub sha256: String,
}

/// The run manifest (Stage A1): the run identity plus the exact byte
/// hashes of every frozen design file. Once committed, it is the
/// authority every live stage verifies against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunManifest {
    pub experiment_id: String,
    pub run_id: String,
    /// The Stage A0 frozen code commit (40-hex).
    pub code_under_test_commit: String,
    /// Deterministic combined hash of the frozen design (see
    /// [`combined_frozen_sha`]).
    pub frozen_design_sha256: String,
    /// Every frozen file, sorted by path.
    pub files: Vec<FrozenFile>,
    pub design_sha256: String,
    pub tasks_sha256: String,
    pub stress_profile_sha256: String,
    pub incumbent_prompt_sha256: String,
    pub mutator_prompt_sha256: String,
}

/// Deterministic combined frozen-design hash: SHA-256 over the
/// canonical (key-sorted) JSON of the sorted (path, sha256) file
/// entries. Independent of file order and key order.
pub fn combined_frozen_sha(files: &[FrozenFile]) -> String {
    let canonical = files
        .iter()
        .map(|f| {
            let mut map = serde_json::Map::new();
            map.insert(
                "path".to_string(),
                serde_json::Value::String(f.path.clone()),
            );
            map.insert(
                "sha256".to_string(),
                serde_json::Value::String(f.sha256.clone()),
            );
            serde_json::Value::Object(map)
        })
        .collect::<Vec<_>>();
    let mut sorted = canonical;
    sorted.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    crate::protocol::sha256_hex(&serde_json::Value::Array(sorted))
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_commit_hex(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Read a frozen file relative to the repository root and hash it.
fn hash_frozen_file(repo_root: &Path, path: &str) -> Result<String, String> {
    let full = repo_root.join(path);
    std::fs::read(&full)
        .map(|b| sha256_bytes(&b))
        .map_err(|e| format!("cannot read frozen file {}: {e}", full.display()))
}

/// Build the run manifest from the current frozen file bytes.
///
/// `run_id` and `experiment_id` must match the design manifest; the
/// code commit must exist in the repository.
pub fn build_manifest(
    repo_root: &Path,
    code_commit: &str,
    run_id: &str,
    experiment_id: &str,
) -> Result<RunManifest, String> {
    if !is_commit_hex(code_commit) {
        return Err(format!(
            "code commit must be a 40-hex git SHA, got {code_commit:?}"
        ));
    }
    let design_path = crate_dir().join("design.json");
    let design = crate::trace::Design::load(&design_path)?;
    if design.run_id != run_id {
        return Err(format!(
            "run_id {run_id:?} does not match the design manifest run_id {:?} — the manifest identity must equal the registered design run identity",
            design.run_id
        ));
    }
    if design.experiment_id != experiment_id {
        return Err(format!(
            "experiment_id {experiment_id:?} does not match the design manifest experiment_id {:?}",
            design.experiment_id
        ));
    }
    // The commit must exist and contain the code tree.
    git(
        repo_root,
        &[
            "rev-parse",
            "--verify",
            &format!("{code_commit}^{{commit}}"),
        ],
    )
    .map_err(|e| format!("code commit {code_commit} does not exist in this repository: {e}"))?;

    let mut files: Vec<FrozenFile> = Vec::new();
    for path in FROZEN_PATHS {
        let sha = hash_frozen_file(repo_root, path)?;
        files.push(FrozenFile {
            path: (*path).to_string(),
            sha256: sha,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let by = |name: &str| -> Result<String, String> {
        files
            .iter()
            .find(|f| f.path.ends_with(name))
            .map(|f| f.sha256.clone())
            .ok_or_else(|| format!("frozen file {name} missing from manifest"))
    };
    Ok(RunManifest {
        experiment_id: experiment_id.to_string(),
        run_id: run_id.to_string(),
        code_under_test_commit: code_commit.to_string(),
        frozen_design_sha256: combined_frozen_sha(&files),
        design_sha256: by("design.json")?,
        tasks_sha256: by("tasks.json")?,
        stress_profile_sha256: by("stress-profile.json")?,
        incumbent_prompt_sha256: by("incumbent.md")?,
        mutator_prompt_sha256: by("mutator.md")?,
        files,
    })
}

/// Load a run manifest from disk.
pub fn load_manifest(path: &Path) -> Result<RunManifest, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read run manifest {}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("run manifest is not valid JSON: {e}"))
}

/// Run git with string arguments; returns stdout (trimmed).
fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn file_sha_in_commit(root: &Path, commit: &str, path: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["show", &format!("{commit}:{path}")])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(sha256_bytes(&output.stdout))
}

fn frozen_file_path(manifest: &RunManifest, name: &str) -> Option<String> {
    manifest
        .files
        .iter()
        .find(|f| f.path.ends_with(name))
        .map(|f| f.path.clone())
}

/// The source-freeze guard. Returns the list of violations (empty =
/// the run is frozen-intact). This is the ONLY gate between the run
/// manifest and the model endpoint: if it fails, NO model request may
/// occur and the run identity is permanently invalid.
///
/// `paths` is the registered frozen-path set (in tests a synthetic set
/// is used; in production it is [`FROZEN_PATHS`]).
pub fn verify_freeze(repo_root: &Path, manifest: &RunManifest, paths: &[&str]) -> Vec<String> {
    let mut violations: Vec<String> = Vec::new();

    // --- 1. Manifest internal consistency -------------------------------
    if !is_commit_hex(&manifest.code_under_test_commit) {
        violations.push(format!(
            "run manifest code_under_test_commit {:?} is not a 40-hex SHA",
            manifest.code_under_test_commit
        ));
    }
    let mut manifest_paths: Vec<String> = manifest.files.iter().map(|f| f.path.clone()).collect();
    manifest_paths.sort();
    let mut expected_paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
    expected_paths.sort();
    if manifest_paths != expected_paths {
        violations.push(format!(
            "run manifest file set does not exactly match the registered {} frozen design paths",
            paths.len()
        ));
    }
    for f in &manifest.files {
        if !is_sha256_hex(&f.sha256) {
            violations.push(format!(
                "run manifest entry {} has a malformed sha256",
                f.path
            ));
        }
    }
    let recomputed = combined_frozen_sha(&manifest.files);
    if recomputed != manifest.frozen_design_sha256 {
        violations.push(
            "run manifest frozen_design_sha256 does not reproduce from the per-file hashes (manifest tamper)"
                .to_string(),
        );
    }
    for name in [
        ("design_sha256", "design.json"),
        ("tasks_sha256", "tasks.json"),
        ("stress_profile_sha256", "stress-profile.json"),
        ("incumbent_prompt_sha256", "incumbent.md"),
        ("mutator_prompt_sha256", "mutator.md"),
    ] {
        let field = name.0;
        let file = name.1;
        let Some(path) = frozen_file_path(manifest, file) else {
            continue;
        };
        let Some(entry) = manifest.files.iter().find(|f| f.path == path) else {
            continue;
        };
        let stored = match field {
            "design_sha256" => &manifest.design_sha256,
            "tasks_sha256" => &manifest.tasks_sha256,
            "stress_profile_sha256" => &manifest.stress_profile_sha256,
            "incumbent_prompt_sha256" => &manifest.incumbent_prompt_sha256,
            _ => &manifest.mutator_prompt_sha256,
        };
        if stored != &entry.sha256 {
            violations.push(format!(
                "run manifest {field} disagrees with its own file table for {file}"
            ));
        }
    }

    // --- 2. Current bytes vs manifest ------------------------------------
    for f in &manifest.files {
        let full = repo_root.join(&f.path);
        match std::fs::read(&full) {
            Err(e) => violations.push(format!(
                "frozen file {} is missing or unreadable on disk: {e}",
                full.display()
            )),
            Ok(bytes) => {
                let now = sha256_bytes(&bytes);
                if now != f.sha256 {
                    violations.push(format!(
                        "frozen file {} current bytes differ from the run manifest (working-tree drift or uncommitted change)",
                        f.path
                    ));
                }
            }
        }
    }

    // --- 3. Bytes vs the code-under-test commit (blob check) -------------
    if is_commit_hex(&manifest.code_under_test_commit) {
        for f in &manifest.files {
            match file_sha_in_commit(repo_root, &manifest.code_under_test_commit, &f.path) {
                None => violations.push(format!(
                    "frozen file {} does not exist at code-under-test commit {} (it was not part of the frozen code commit)",
                    f.path, manifest.code_under_test_commit
                )),
                Some(blob_sha) => {
                    if blob_sha != f.sha256 {
                        violations.push(format!(
                            "frozen file {} differs from its bytes at code-under-test commit {} — the frozen design changed relative to the frozen code",
                            f.path, manifest.code_under_test_commit
                        ));
                    }
                }
            }
        }
    }

    // --- 4. No commit after A0 touching any frozen path -------------------
    // A modify→commit→revert sequence still leaves a commit touching the
    // path, so the history check — not the net diff — is the authority.
    if is_commit_hex(&manifest.code_under_test_commit) {
        let head = match git(repo_root, &["rev-parse", "HEAD"]) {
            Ok(h) => h,
            Err(e) => {
                violations.push(format!("cannot resolve HEAD: {e}"));
                String::new()
            }
        };
        if is_commit_hex(&head) {
            let mut log_args: Vec<String> = vec!["log".into()];
            log_args.push(format!("{}..HEAD", manifest.code_under_test_commit));
            log_args.push("--".into());
            for p in paths {
                log_args.push((*p).to_string());
            }
            let log_args: Vec<&str> = log_args.iter().map(String::as_str).collect();
            match git(repo_root, &log_args) {
                Ok(out) if out.is_empty() => {
                    // Clean: no commit after A0 touches any frozen path.
                }
                Ok(out) => {
                    let count = out.lines().count();
                    violations.push(format!(
                        "{count} commit(s) after the frozen code commit touch frozen design paths: {out}"
                    ));
                }
                Err(e) => violations.push(format!("cannot inspect commit history: {e}")),
            }
            // --- 5. No working-tree drift on frozen paths -----------------
            let mut status_args: Vec<String> = vec!["status".into()];
            status_args.push("--porcelain".into());
            status_args.push("--".into());
            for p in paths {
                status_args.push((*p).to_string());
            }
            let status_args: Vec<&str> = status_args.iter().map(String::as_str).collect();
            match git(repo_root, &status_args) {
                Ok(out) if out.is_empty() => {
                    // Clean: no staged or unstaged modification.
                }
                Ok(out) => {
                    violations.push(format!("working-tree drift on frozen design paths: {out}"))
                }
                Err(e) => violations.push(format!("cannot inspect working tree: {e}")),
            }
            // --- 6. Commit ancestry ---------------------------------------
            let mut ancestor_args: Vec<String> = vec!["merge-base".into(), "--is-ancestor".into()];
            ancestor_args.push(manifest.code_under_test_commit.clone());
            ancestor_args.push(head.clone());
            let ancestor_args: Vec<&str> = ancestor_args.iter().map(String::as_str).collect();
            if let Err(e) = git(repo_root, &ancestor_args) {
                violations.push(format!(
                    "code-under-test commit {} is not an ancestor of HEAD: {e}",
                    manifest.code_under_test_commit
                ));
            }
        }
    }

    violations
}

/// Convenience wrapper over the registered frozen paths.
pub fn verify_run_freeze(repo_root: &Path, manifest: &RunManifest) -> Vec<String> {
    verify_freeze(repo_root, manifest, FROZEN_PATHS)
}

/// The repository root, resolved from the crate directory.
pub fn repo_root() -> Result<PathBuf, String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(crate_dir())
        .output()
        .map_err(|e| format!("cannot run git in {}: {e}", crate_dir().display()))?;
    if !out.status.success() {
        return Err(format!(
            "git rev-parse --show-toplevel failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    ))
}

/// The experiment crate directory.
pub fn crate_dir() -> PathBuf {
    std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("experiments/0011-confirmatory-self-evolution"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git_in(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Create a fresh temp git repo with the given files, commit them,
    /// and return (root, initial_commit).
    fn temp_repo(tag: &str, files: &[(&str, &str)]) -> (PathBuf, String) {
        let root = std::env::temp_dir().join(format!(
            "mutagen-exp-0011-freeze-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for (rel, content) in files {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, content).unwrap();
        }
        git_in(&root, &["init", "-q"]);
        git_in(&root, &["config", "user.email", "t@t.t"]);
        git_in(&root, &["config", "user.name", "t"]);
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "A0"]);
        let commit = git(&root, &["rev-parse", "HEAD"]).unwrap();
        (root, commit)
    }

    fn manifest_for(files: &[(&str, &str)], commit: &str) -> RunManifest {
        let mut fs = Vec::new();
        for (p, c) in files {
            fs.push(FrozenFile {
                path: (*p).to_string(),
                sha256: sha256_bytes(c.as_bytes()),
            });
        }
        fs.sort_by(|a, b| a.path.cmp(&b.path));
        RunManifest {
            experiment_id: "0011".into(),
            run_id: "0011-r1".into(),
            code_under_test_commit: commit.to_string(),
            frozen_design_sha256: combined_frozen_sha(&fs),
            files: fs,
            design_sha256: String::new(),
            tasks_sha256: String::new(),
            stress_profile_sha256: String::new(),
            incumbent_prompt_sha256: String::new(),
            mutator_prompt_sha256: String::new(),
        }
    }

    fn cleanup(root: &Path) {
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn combined_frozen_sha_is_deterministic() {
        let a = vec![
            FrozenFile {
                path: "b".into(),
                sha256: "2".into(),
            },
            FrozenFile {
                path: "a".into(),
                sha256: "1".into(),
            },
        ];
        let mut b = a.clone();
        b.reverse();
        assert_eq!(combined_frozen_sha(&a), combined_frozen_sha(&b));
        let mut c = a.clone();
        c[0].sha256 = "3".into();
        assert_ne!(combined_frozen_sha(&a), combined_frozen_sha(&c));
    }

    #[test]
    fn guard_passes_on_a_pristine_freeze() {
        let (root, commit) = temp_repo(
            "pristine",
            &[
                ("frozen/a.rs", "v1"),
                ("frozen/b.rs", "v2"),
                ("art/x.json", "z"),
            ],
        );
        let paths = ["frozen/a.rs", "frozen/b.rs"];
        let m = manifest_for(&[("frozen/a.rs", "v1"), ("frozen/b.rs", "v2")], &commit);
        let v = verify_freeze(&root, &m, &paths);
        assert!(v.is_empty(), "{v:?}");
        cleanup(&root);
    }

    #[test]
    fn guard_detects_current_byte_change() {
        let (root, commit) = temp_repo("bytes", &[("frozen/a.rs", "v1")]);
        let paths = ["frozen/a.rs"];
        let m = manifest_for(&[("frozen/a.rs", "v1")], &commit);
        std::fs::write(root.join("frozen/a.rs"), "TAMPERED").unwrap();
        let v = verify_freeze(&root, &m, &paths);
        assert!(
            v.iter().any(|x| x.contains("current bytes differ")),
            "{v:?}"
        );
        cleanup(&root);
    }

    #[test]
    fn guard_detects_committed_frozen_change() {
        let (root, commit) = temp_repo("committed", &[("frozen/a.rs", "v1"), ("art/x.json", "z")]);
        let paths = ["frozen/a.rs"];
        let m = manifest_for(&[("frozen/a.rs", "v1")], &commit);
        // Modify AND commit the frozen file (bytes still differ too).
        std::fs::write(root.join("frozen/a.rs"), "v2").unwrap();
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "change frozen file"]);
        let v = verify_freeze(&root, &m, &paths);
        assert!(
            v.iter()
                .any(|x| x
                    .contains("commit(s) after the frozen code commit touch frozen design paths")),
            "{v:?}"
        );
        cleanup(&root);
    }

    #[test]
    fn guard_detects_modify_commit_revert_history() {
        let (root, commit) = temp_repo("revert", &[("frozen/a.rs", "v1"), ("art/x.json", "z")]);
        let paths = ["frozen/a.rs"];
        let m = manifest_for(&[("frozen/a.rs", "v1")], &commit);
        // Modify, commit, then modify back and commit: NET bytes are
        // identical, but the history still touches the frozen path.
        std::fs::write(root.join("frozen/a.rs"), "v2").unwrap();
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "modify"]);
        std::fs::write(root.join("frozen/a.rs"), "v1").unwrap();
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "revert"]);
        let v = verify_freeze(&root, &m, &paths);
        assert!(
            v.iter()
                .any(|x| x
                    .contains("commit(s) after the frozen code commit touch frozen design paths")),
            "modify→commit→revert must invalidate the run: {v:?}"
        );
        cleanup(&root);
    }

    #[test]
    fn guard_allows_artifact_only_commits() {
        let (root, commit) = temp_repo("artifacts", &[("frozen/a.rs", "v1"), ("art/x.json", "z")]);
        let paths = ["frozen/a.rs"];
        let m = manifest_for(&[("frozen/a.rs", "v1")], &commit);
        // Commit a NON-frozen artifact only.
        std::fs::write(root.join("art/x.json"), "z2").unwrap();
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "artifact"]);
        let v = verify_freeze(&root, &m, &paths);
        assert!(v.is_empty(), "{v:?}");
        cleanup(&root);
    }

    #[test]
    fn guard_detects_manifest_hash_tamper() {
        let (root, commit) = temp_repo("tamper", &[("frozen/a.rs", "v1")]);
        let paths = ["frozen/a.rs"];
        let mut m = manifest_for(&[("frozen/a.rs", "v1")], &commit);
        m.files[0].sha256 = "f".repeat(64);
        let v = verify_freeze(&root, &m, &paths);
        assert!(
            v.iter()
                .any(|x| x.contains("frozen_design_sha256 does not reproduce")),
            "{v:?}"
        );
        cleanup(&root);
    }

    #[test]
    fn guard_detects_uncommitted_working_tree_drift() {
        let (root, commit) = temp_repo("drift", &[("frozen/a.rs", "v1")]);
        let paths = ["frozen/a.rs"];
        let m = manifest_for(&[("frozen/a.rs", "v1")], &commit);
        std::fs::write(root.join("frozen/a.rs"), "v2-dirty").unwrap();
        let v = verify_freeze(&root, &m, &paths);
        assert!(v.iter().any(|x| x.contains("working-tree drift")), "{v:?}");
        cleanup(&root);
    }
}
