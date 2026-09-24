//! The commit-level stage-artifact freeze system (Experiment 0015).
//!
//! This is one of the two decisive 0015 mechanisms (together with the
//! 0011 source freeze carried forward in `src/freeze.rs`). Experiment
//! 0010's run was voided because the *source* changed mid-run; 0011
//! closed that gap but still left a provenance gap in the *stage
//! evidence* chain: Stage B's artifacts were hashed on disk, but the
//! hash-to-Git binding of completed-stage artifacts was never
//! mechanically verified before the next live stage began (0011-r1
//! stopped at Stage C for a different, schema reason — but the gap
//! would have applied).
//!
//! 0015 therefore freezes every completed stage at the Git level:
//!
//! 1. Each completed stage (B discovery, C mutation generation,
//!    D selection) is **verified** (its registered verifier passes),
//!    **hashed** (SHA-256 over exact bytes of every stage artifact),
//!    **manifested** (a `stage-*-freeze.json` with the path + hash
//!    pairs), and **committed together** (artifacts + manifest in one
//!    commit; the harness writes the manifest and prints the exact
//!    commit — the human/agent performs the commit, exactly like
//!    Stages A0/A1).
//! 2. The NEXT live stage runs [`verify_stage_freeze`] for the
//!    preceding stage BEFORE its first model request. The verifier
//!    requires ALL of:
//!    - the freeze manifest is committed, and the introducing commit
//!      is found via `git log -- <manifest path>`;
//!    - every artifact blob AT that commit matches the manifest hash;
//!    - every artifact's CURRENT bytes match the manifest hash (no
//!      working-tree drift);
//!    - NO commit after the introducing commit touches any stage
//!      artifact (a later edit, even a reverted one, is caught via
//!      the history, not the net diff).
//!
//! Any violation ⇒ the next stage refuses to make a single model
//! request; the run is INCONCLUSIVE and stops (no patching, no
//! continuation). This is checked in `self-test` / `preflight`
//! against real temporary git repositories.
//!
//! There is deliberately no "un-freeze" and no override flag.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::Serialize;

use crate::protocol::sha256_bytes;

/// One stage artifact and its byte hash at the stage freeze.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageFreezeArtifact {
    /// Repository-relative path of the artifact.
    pub path: String,
    /// SHA-256 hex of the exact artifact bytes.
    pub sha256: String,
}

/// One completed stage's freeze manifest
/// (`stage-b-freeze.json` / `stage-c-freeze.json` / `stage-d-freeze.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageFreeze {
    /// "B" | "C" | "D".
    pub stage: String,
    pub run_id: String,
    /// Every artifact the stage produced, sorted by path.
    pub artifacts: Vec<StageFreezeArtifact>,
}

/// Build one stage freeze from the CURRENT bytes of the given artifacts.
///
/// `repo_root` + `repo_relative` + `absolute` pairs describe each
/// artifact; hashes are computed from the absolute paths, paths are
/// recorded relative to the repository root, and the list is sorted by
/// path (deterministic file layout).
pub fn build_stage_freeze(
    run_id: &str,
    stage: &str,
    artifacts: &[(String, PathBuf)],
) -> Result<StageFreeze, String> {
    let mut rows = Vec::new();
    for (rel, abs) in artifacts {
        let bytes = std::fs::read(abs)
            .map_err(|e| format!("cannot read stage artifact {}: {e}", abs.display()))?;
        rows.push(StageFreezeArtifact {
            path: rel.clone(),
            sha256: sha256_bytes(&bytes),
        });
    }
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(StageFreeze {
        stage: stage.to_string(),
        run_id: run_id.to_string(),
        artifacts: rows,
    })
}

/// Write a stage freeze manifest (pretty JSON + trailing newline).
pub fn write_stage_freeze(path: &Path, freeze: &StageFreeze) -> Result<String, String> {
    let text = serde_json::to_string_pretty(freeze).map_err(|e| e.to_string())?;
    let bytes = format!("{text}\n").into_bytes();
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))
        .map_err(|e| format!("cannot create directory for {}: {e}", path.display()))?;
    std::fs::write(path, &bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(sha256_bytes(&bytes))
}

/// Load a stage freeze manifest from disk.
pub fn load_stage_freeze(path: &Path) -> Result<StageFreeze, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read stage freeze {}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("stage freeze is not valid JSON: {e}"))
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

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// The oldest commit that introduced (or last modified) `path` in the
/// current branch history.
fn introducing_commit(root: &Path, path: &str) -> Option<String> {
    let out = git(root, &["log", "--reverse", "--format=%H", "--", path]).ok()?;
    out.lines().next().map(str::to_string)
}

/// Read a blob at a commit and hash it; `None` when the path does not
/// exist at that commit.
fn blob_sha_in_commit(root: &Path, commit: &str, path: &str) -> Option<String> {
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

/// Verify one completed stage's freeze against Git. Returns the list of
/// violations (empty = the stage freeze is intact and the next live
/// stage may proceed).
///
/// `manifest_path` is the repository-relative path of the
/// `stage-*-freeze.json` file itself; every entry of
/// [`StageFreeze::artifacts`] is checked as described in the module
/// documentation. All paths are repository-relative.
pub fn verify_stage_freeze(
    repo_root: &Path,
    manifest_path: &str,
    freeze: &StageFreeze,
) -> Vec<String> {
    let mut violations: Vec<String> = Vec::new();
    let stage_label = if freeze.stage.is_empty() {
        "this stage"
    } else {
        &freeze.stage
    };

    // --- 1. Internal consistency --------------------------------------
    if freeze.run_id.trim().is_empty() {
        violations.push("stage freeze manifest has an empty run_id".to_string());
    }
    if freeze.artifacts.is_empty() {
        violations.push(
            "stage freeze manifest has no artifacts — an empty stage cannot be frozen".to_string(),
        );
    }
    let mut seen: Vec<&str> = Vec::new();
    for a in &freeze.artifacts {
        if a.path.trim().is_empty() {
            violations.push("stage freeze artifact with an empty path".to_string());
        }
        if !is_sha256_hex(&a.sha256) {
            violations.push(format!(
                "stage freeze artifact {} has a malformed sha256",
                a.path
            ));
        }
        if seen.contains(&a.path.as_str()) {
            violations.push(format!("stage freeze lists {} twice", a.path));
        }
        seen.push(a.path.as_str());
    }

    // --- 2. The manifest must be committed -----------------------------
    // An uncommitted manifest (or an untracked file) means the stage was
    // never frozen: the next live stage must be blocked.
    let Some(commit) = introducing_commit(repo_root, manifest_path) else {
        violations.push(format!(
            "stage {stage_label} freeze manifest {manifest_path} is not committed — the stage must be committed (artifacts + manifest together) before the next live stage"
        ));
        return violations;
    };
    if blob_sha_in_commit(repo_root, &commit, manifest_path).is_none() {
        violations.push(format!(
            "stage {stage_label} freeze manifest {manifest_path} is missing at its introducing commit {commit}"
        ));
        return violations;
    }
    // The manifest bytes at that commit must be what is on disk now:
    // the committed manifest is the authority.
    if let Some(committed_sha) = blob_sha_in_commit(repo_root, &commit, manifest_path) {
        let current = std::fs::read(repo_root.join(manifest_path))
            .map(|b| sha256_bytes(&b))
            .unwrap_or_default();
        if current != committed_sha {
            violations.push(format!(
                "stage {stage_label} freeze manifest current bytes differ from its committed bytes (the committed manifest is the authority)"
            ));
        }
    }

    // --- 3. Every artifact at the freeze commit -------------------------
    for a in &freeze.artifacts {
        match blob_sha_in_commit(repo_root, &commit, &a.path) {
            None => violations.push(format!(
                "stage {stage_label} artifact {} does not exist at the freeze commit {commit} (artifacts must be committed together with the manifest)",
                a.path
            )),
            Some(blob) => {
                if blob != a.sha256 {
                    violations.push(format!(
                        "stage {stage_label} artifact {} at freeze commit {commit} does not match the manifest hash ({} != {})",
                        a.path,
                        a.sha256,
                        blob
                    ));
                }
            }
        }
    }

    // --- 4. Current bytes ------------------------------------------------
    for a in &freeze.artifacts {
        let full = repo_root.join(&a.path);
        match std::fs::read(&full) {
            Err(e) => violations.push(format!(
                "stage {stage_label} artifact {} is missing on disk: {e}",
                a.path
            )),
            Ok(bytes) => {
                let now = sha256_bytes(&bytes);
                if now != a.sha256 {
                    violations.push(format!(
                        "stage {stage_label} artifact {} current bytes differ from the stage freeze manifest (uncommitted change or later edit)",
                        a.path
                    ));
                }
            }
        }
    }

    // --- 5. No later commit may touch any stage artifact -----------------
    // History — not the net diff — is the authority: a later
    // modify→commit→revert still invalidates the stage chain.
    let head = match git(repo_root, &["rev-parse", "HEAD"]) {
        Ok(h) => h,
        Err(e) => {
            violations.push(format!("cannot resolve HEAD: {e}"));
            return violations;
        }
    };
    if head == commit {
        // Nothing after the freeze commit: clean by construction.
        return violations;
    }
    let mut log_args: Vec<String> = vec!["log".into()];
    log_args.push(format!("{commit}..HEAD"));
    log_args.push("--".into());
    for a in &freeze.artifacts {
        log_args.push(a.path.clone());
    }
    let log_args: Vec<&str> = log_args.iter().map(String::as_str).collect();
    match git(repo_root, &log_args) {
        Ok(out) if out.is_empty() => {
            // Clean: no commit after the freeze touches any artifact.
        }
        Ok(out) => {
            let count = out.lines().count();
            violations.push(format!(
                "{count} commit(s) after the stage {stage_label} freeze commit touch stage artifacts: {out}"
            ));
        }
        Err(e) => violations.push(format!("cannot inspect commit history: {e}")),
    }

    violations
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

    /// Fresh temp git repo with the given files committed (empty initial
    /// commit when no files).
    fn temp_repo(tag: &str, files: &[(&str, &str)]) -> (PathBuf, String) {
        let root = std::env::temp_dir().join(format!(
            "mutagen-exp-0015-stagefreeze-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        for (rel, content) in files {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, content).unwrap();
        }
        git_in(&root, &["init", "-q"]);
        git_in(&root, &["config", "user.email", "t@t.t"]);
        git_in(&root, &["config", "user.name", "t"]);
        if files.is_empty() {
            git_in(&root, &["commit", "-q", "--allow-empty", "-m", "base"]);
        } else {
            git_in(&root, &["add", "-A"]);
            git_in(&root, &["commit", "-q", "-m", "base"]);
        }
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&root)
            .output()
            .unwrap();
        (
            root,
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )
    }

    fn cleanup(root: &Path) {
        let _ = std::fs::remove_dir_all(root);
    }

    fn freeze_for(artifacts: &[(&str, &str)]) -> StageFreeze {
        let mut rows = Vec::new();
        for (p, c) in artifacts {
            rows.push(StageFreezeArtifact {
                path: (*p).to_string(),
                sha256: sha256_bytes(c.as_bytes()),
            });
        }
        rows.sort_by(|a, b| a.path.cmp(&b.path));
        StageFreeze {
            stage: "B".into(),
            run_id: "0015-r1".into(),
            artifacts: rows,
        }
    }

    fn write_manifest(root: &Path, freeze: &StageFreeze) {
        let text = serde_json::to_string_pretty(freeze).unwrap();
        std::fs::write(root.join("stage-b-freeze.json"), format!("{text}\n")).unwrap();
    }

    const MANIFEST: &str = "stage-b-freeze.json";

    #[test]
    fn pristine_committed_freeze_verifies() {
        let (root, _commit) = temp_repo("pristine", &[("a/x.json", "x1"), ("a/y.json", "y1")]);
        let f = freeze_for(&[("a/x.json", "x1"), ("a/y.json", "y1")]);
        write_manifest(&root, &f);
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "freeze B"]);
        let v = verify_stage_freeze(&root, MANIFEST, &f);
        assert!(v.is_empty(), "{v:?}");
        cleanup(&root);
    }

    #[test]
    fn uncommitted_freeze_blocks_the_next_stage() {
        let (root, _commit) = temp_repo("uncommitted", &[("a/x.json", "x1")]);
        let f = freeze_for(&[("a/x.json", "x1")]);
        write_manifest(&root, &f);
        // Manifest + (already-committed) artifact, but the manifest file
        // itself is untracked: the stage was never frozen in Git.
        let v = verify_stage_freeze(&root, MANIFEST, &f);
        assert!(v.iter().any(|x| x.contains("is not committed")), "{v:?}");
        cleanup(&root);
    }

    #[test]
    fn uncommitted_artifact_blocks_the_next_stage() {
        // The artifact is NOT in the base commit; the freeze commit
        // records its hash, but the artifact blob is absent at that
        // commit → the "committed together" requirement fails.
        let (root, _commit) = temp_repo("uncommitted-art", &[]);
        let f = freeze_for(&[("a/x.json", "x1")]);
        write_manifest(&root, &f);
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("a/x.json"), "x1").unwrap();
        git_in(&root, &["add", MANIFEST]);
        git_in(
            &root,
            &["commit", "-q", "-m", "freeze B (artifact left out)"],
        );
        let v = verify_stage_freeze(&root, MANIFEST, &f);
        assert!(
            v.iter()
                .any(|x| x.contains("does not exist at the freeze commit")),
            "{v:?}"
        );
        cleanup(&root);
    }

    #[test]
    fn committed_then_modified_artifact_blocks_the_next_stage() {
        let (root, _commit) = temp_repo("tamper", &[]);
        let f = freeze_for(&[("a/x.json", "x1")]);
        write_manifest(&root, &f);
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("a/x.json"), "x1").unwrap();
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "freeze B"]);
        let v = verify_stage_freeze(&root, MANIFEST, &f);
        assert!(v.is_empty(), "pristine must verify: {v:?}");
        // Later edit (uncommitted): current bytes drift from the freeze.
        std::fs::write(root.join("a/x.json"), "TAMPERED").unwrap();
        let v = verify_stage_freeze(&root, MANIFEST, &f);
        assert!(
            v.iter().any(|x| x.contains("current bytes differ")),
            "{v:?}"
        );
        cleanup(&root);
    }

    #[test]
    fn later_commit_touching_an_artifact_blocks_the_next_stage() {
        let (root, _commit) = temp_repo("later", &[]);
        let f = freeze_for(&[("a/x.json", "x1")]);
        write_manifest(&root, &f);
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("a/x.json"), "x1").unwrap();
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "freeze B"]);
        // modify → commit → revert: NET bytes identical, but a commit
        // after the freeze touches the artifact → blocked.
        std::fs::write(root.join("a/x.json"), "x2").unwrap();
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "edit"]);
        std::fs::write(root.join("a/x.json"), "x1").unwrap();
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "revert"]);
        let v = verify_stage_freeze(&root, MANIFEST, &f);
        assert!(
            v.iter().any(|x| x.contains("commit(s) after the stage")),
            "modify→commit→revert must invalidate the stage chain: {v:?}"
        );
        cleanup(&root);
    }

    #[test]
    fn manifest_hash_tamper_is_detected() {
        let (root, _commit) = temp_repo("manipulated", &[]);
        let mut f = freeze_for(&[("a/x.json", "x1")]);
        write_manifest(&root, &f);
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("a/x.json"), "x1").unwrap();
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "freeze B"]);
        f.artifacts[0].sha256 = "f".repeat(64);
        let v = verify_stage_freeze(&root, MANIFEST, &f);
        assert!(
            v.iter()
                .any(|x| x.contains("does not match the manifest hash")),
            "{v:?}"
        );
        cleanup(&root);
    }
}
