//! The audited execution channel ledger (Experiment 0014).
//!
//! One canonical JSONL line per gateway request ATTEMPT — authorized or
//! not. Unauthorized attempts are never omitted: a rejected `GET
//! /v1/models` probe is a ledger row exactly like a forwarded request.
//!
//! The ledger is a hash chain:
//!
//! ```text
//! entry_hash =
//!   SHA-256(previous_entry_hash || canonical_entry_without_entry_hash)
//! ```
//!
//! where `previous_entry_hash` is the `entry_hash` of the preceding
//! row (the genesis hash from `channel-manifest.json` for row 1). The
//! canonical encoding is the key-sorted single-line JSON object with
//! the `entry_hash` field removed, so any edited / reordered /
//! deleted / truncated row breaks the chain and the verifier
//! [`verify_chain`] detects it.
//!
//! Append semantics: the ledger file is opened append-only, one line
//! per request attempt, flushed and data-synced after every entry.
//! Entries are never rewritten after A1 (the hash chain makes any
//! rewrite tamper-evident).
//!
//! This module contains no experiment stage semantics, no model I/O,
//! and no knowledge of the upstream endpoint: it is a pure
//! append/verify ledger.

use std::io::Write as _;
use std::path::Path;

use serde::Deserialize;
use serde::Serialize;

use crate::protocol::sha256_bytes;

/// One canonical ledger row: one gateway request attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelEntry {
    /// 1-based, strictly increasing sequence number.
    pub seq: u64,
    /// The run identity carried in `X-Mutagen-Run` (empty when absent).
    pub run_id: String,
    /// The stage carried in `X-Mutagen-Stage` (empty when absent).
    pub stage_claim: String,
    /// The request identity carried in `X-Mutagen-Request-Id`
    /// (empty when absent).
    pub request_id: String,
    /// The HTTP method as received.
    pub method: String,
    /// The request path as received.
    pub path: String,
    /// SHA-256 of the exact received body bytes (hex).
    pub body_sha256: String,
    /// `true` iff the request passed every channel check and was
    /// forwarded to the upstream.
    pub authorized: bool,
    /// `true` iff the request was forwarded upstream.
    pub forwarded: bool,
    /// Response status class of the forwarded request
    /// (`"2xx"` / `"3xx"` / `"4xx"` / `"5xx"`), `"0xx"` for a forward
    /// whose transport failed before any status, empty when not
    /// forwarded.
    pub status_class: String,
    /// The rejection reason (populated only for unauthorized rows, e.g.
    /// `missing_capability`, `wrong_stage`, `body_hash_mismatch`,
    /// `duplicate_request_id`, `disallowed_path`, `channel_not_armed`).
    pub reason: Option<String>,
    /// The `entry_hash` of the preceding row (genesis for row 1).
    pub previous_entry_hash: String,
    /// `SHA-256(previous_entry_hash || canonical entry without
    /// entry_hash)`.
    pub entry_hash: String,
}

/// The canonical (key-sorted, single-line) JSON encoding of an entry
/// WITHOUT the `entry_hash` field — the hash input. Deterministic:
/// `serde_json::Map` is a BTreeMap (keys sorted), so the encoding is
/// independent of insertion order.
fn canonical_without_entry_hash(e: &ChannelEntry) -> String {
    let mut map = serde_json::to_value(e).expect("entry serializes");
    if let Some(obj) = map.as_object_mut() {
        obj.remove("entry_hash");
    }
    serde_json::to_string(&map).expect("canonical encoding")
}

/// Compute the entry hash for one row given the previous row's hash.
pub fn compute_entry_hash(previous: &str, entry: &ChannelEntry) -> String {
    let mut input = String::from(previous);
    input.push('\n');
    input.push_str(&canonical_without_entry_hash(entry));
    sha256_bytes(input.as_bytes())
}

/// The complete ledger file.
pub struct Ledger {
    path: std::path::PathBuf,
    /// The genesis hash from the channel manifest (the `previous`
    /// hash of the first row).
    genesis_hash: String,
}

impl Ledger {
    /// Open (creating if absent) a ledger file in append-only usage.
    pub fn open(path: &Path, genesis_hash: &str) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        Ok(Self {
            path: path.to_path_buf(),
            genesis_hash: genesis_hash.to_string(),
        })
    }

    /// The next row number (the file must not be rewritten; the count
    /// is derived from the existing rows).
    pub fn len(&self) -> Result<u64, String> {
        self.entries().map(|v| v.len() as u64)
    }

    /// The hash of the last row (genesis when the ledger is empty).
    pub fn head_hash(&self) -> Result<String, String> {
        Ok(self
            .entries()?
            .last()
            .map(|e| e.entry_hash.clone())
            .unwrap_or_else(|| self.genesis_hash.clone()))
    }

    /// Read every row of the ledger file (all lines, in order).
    pub fn entries(&self) -> Result<Vec<ChannelEntry>, String> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let raw = std::fs::read_to_string(&self.path)
            .map_err(|e| format!("cannot read ledger {}: {e}", self.path.display()))?;
        let mut out = Vec::new();
        for (i, line) in raw.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            out.push(
                serde_json::from_str::<ChannelEntry>(line)
                    .map_err(|e| format!("ledger line {} is not a valid entry: {e}", i + 1))?,
            );
        }
        Ok(out)
    }

    /// Append one row: compute the hash from the previous row, write
    /// exactly one line, flush, and data-sync (one line = one request
    /// attempt; never rewritten afterwards).
    pub fn append(&self, mut entry: ChannelEntry) -> Result<ChannelEntry, String> {
        let prev = self.head_hash()?;
        entry.seq = self.len()? + 1;
        entry.previous_entry_hash = prev.clone();
        entry.entry_hash = compute_entry_hash(&prev, &entry);
        let line = serde_json::to_string(&entry).map_err(|e| e.to_string())?;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("cannot open ledger for append: {e}"))?;
        f.write_all(line.as_bytes())
            .and_then(|_| f.write_all(b"\n"))
            .and_then(|_| f.flush())
            .map_err(|e| format!("ledger append failed: {e}"))?;
        f.sync_data()
            .map_err(|e| format!("ledger sync failed: {e}"))?;
        Ok(entry)
    }
}

/// Verify the ENTIRE hash chain. Returns violations (empty = intact):
/// row ordering / sequence, row linkage, and recomputed row hashes.
/// Any missing row, reordered row, edited row, or truncated middle
/// produces a violation.
pub fn verify_chain(entries: &[ChannelEntry], genesis_hash: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let mut prev = genesis_hash.to_string();
    for (i, e) in entries.iter().enumerate() {
        if e.seq != i as u64 + 1 {
            errors.push(format!(
                "ledger row {} has seq {}, expected {} (missing or reordered row)",
                i + 1,
                e.seq,
                i as u64 + 1
            ));
        }
        if e.previous_entry_hash != prev {
            errors.push(format!(
                "ledger row {} previous_entry_hash does not link to the preceding row (chain break)",
                e.seq
            ));
        }
        let recomputed = compute_entry_hash(&e.previous_entry_hash, e);
        if recomputed != e.entry_hash {
            errors.push(format!(
                "ledger row {} entry_hash does not reproduce from its canonical bytes (edited or corrupted row)",
                e.seq
            ));
        }
        prev = e.entry_hash.clone();
    }
    errors
}

/// The aggregate counters the checkpoints and the final seal report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerCounts {
    pub entries: u64,
    pub authorized_forwarded: u64,
    pub unauthorized: u64,
    pub per_stage: std::collections::BTreeMap<String, u64>,
}

/// Compute the aggregate counters over one set of ledger rows
/// (`per_stage` counts forwarded rows by their claimed stage).
pub fn count_entries(entries: &[ChannelEntry]) -> LedgerCounts {
    let mut c = LedgerCounts::default();
    for e in entries {
        c.entries += 1;
        if e.authorized && e.forwarded {
            c.authorized_forwarded += 1;
            *c.per_stage.entry(e.stage_claim.clone()).or_insert(0) += 1;
        } else {
            c.unauthorized += 1;
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        seq: u64,
        run: &str,
        stage: &str,
        id: &str,
        body: &str,
        ok: bool,
        f: bool,
    ) -> ChannelEntry {
        ChannelEntry {
            seq,
            run_id: run.into(),
            stage_claim: stage.into(),
            request_id: id.into(),
            method: "POST".into(),
            path: "/v1/chat/completions".into(),
            body_sha256: body.into(),
            authorized: ok,
            forwarded: f,
            status_class: if f { "2xx".into() } else { String::new() },
            reason: if ok { None } else { Some("test".into()) },
            previous_entry_hash: String::new(),
            entry_hash: String::new(),
        }
    }

    #[test]
    fn chain_verifies_clean_and_catches_tamper() {
        let dir = std::env::temp_dir().join(format!(
            "mutagen-exp-0014-ledger-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let ledger = Ledger::open(&dir.join("channel-ledger.jsonl"), &"g".repeat(64)).unwrap();
        let e1 = ledger
            .append(entry(
                1,
                "0014-r1",
                "B",
                "0014-r1-B-000001",
                "b1",
                true,
                true,
            ))
            .unwrap();
        let e2 = ledger
            .append(entry(
                2,
                "0014-r1",
                "B",
                "0014-r1-B-000002",
                "b2",
                true,
                true,
            ))
            .unwrap();
        let mut rows = ledger.entries().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(verify_chain(&rows, &"g".repeat(64)).is_empty());

        // Edited row.
        rows[0].body_sha256 = "TAMPERED".into();
        assert!(!verify_chain(&rows, &"g".repeat(64)).is_empty());
        rows[0].body_sha256 = e1.body_sha256.clone();

        // Reordered rows.
        let mut r = rows.clone();
        r.reverse();
        assert!(!verify_chain(&r, &"g".repeat(64)).is_empty());

        // Deleted middle row.
        let mut r = rows.clone();
        r.remove(0);
        assert!(!verify_chain(&r, &"g".repeat(64)).is_empty());

        // Wrong genesis.
        assert!(!verify_chain(&rows, &"x".repeat(64)).is_empty());
        let _ = e2;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
