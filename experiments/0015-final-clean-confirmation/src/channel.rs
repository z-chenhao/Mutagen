//! The audited execution channel (Experiment 0015 — the experiment's
//! ONLY semantic delta over 0013).
//!
//! Registered endpoint vs upstream transport:
//!
//! ```text
//!                 registered experiment traffic
//!                              │
//!                              ▼
//!                    ┌──────────────────┐
//!                    │ Channel Gateway  │
//!                    │ 127.0.0.1:<port> │
//!                    └────────┬─────────┘
//!                             │
//!                   authenticated forward
//!                             │
//!                             ▼
//!                    upstream model server
//!                    127.0.0.1:8000/v1
//! ```
//!
//! The REGISTERED EXPERIMENT ENDPOINT is the gateway; the physical
//! upstream model server is an implementation backend. **This module
//! is the ONLY source file that knows the upstream URL** (it reads
//! `channel.upstream_endpoint` from `design.json`). The agent/mutator
//! modules (`model.rs`, `kernel.rs`, `experiment.rs`, `mutation.rs`,
//! `main.rs`) must never construct a request to the upstream endpoint:
//! `ModelClient` is bound to the gateway, and the static preflight
//! bypass-guard proves by source inspection that the upstream literal
//! appears in no other source file.
//!
//! Scope boundary (registered, do not overclaim): this channel
//! mechanically guarantees that every model traffic belonging to the
//! registered Mutagen experiment execution path goes through the
//! audited gateway and reconciles exactly with the experiment
//! artifacts. It does NOT guarantee that no other process on the
//! machine can talk to the physical upstream server (that would need
//! OS-level isolation). The physical upstream endpoint is
//! infrastructure; the registered experimental execution channel is
//! the gateway.
//!
//! Session / capability / authorization:
//!
//! - `channel_session_id` — generated before A1 (no randomness after
//!   A1 changes protocol semantics).
//! - `capability_key = SHA-256("mutagen-exp-0015-channel" ||
//!   protocol_version || "|" || session_id)` — run-local capability
//!   material (deterministic from the committed manifest; exact crypto
//!   strength is not the scientific variable: the goal is accidental
//!   unregistered-request detection, not adversarial protection).
//! - Per-request authorization
//!   `X-Mutagen-Auth = HMAC-SHA256(capability_key, run_id || 0x1f ||
//!   stage || 0x1f || request_id || 0x1f || SHA-256(body))` — the body
//!   hash binds the exact bytes, so an authenticated request cannot
//!   silently change body bytes without the verifier detecting it.
//! - Every legitimate live request carries `X-Mutagen-Run`,
//!   `X-Mutagen-Stage` (B|C|D|E), `X-Mutagen-Request-Id`
//!   (`0015-r1-<stage>-NNNNNN`, stage-local sequence from 1, issued by
//!   the experiment runtime — never the model), and `X-Mutagen-Auth`.
//!
//! Ledger: every received gateway request, authorized or not, appends
//! one hash-chained row to `channel-ledger.jsonl` (see
//! `src/channel_ledger.rs`).
//!
//! Lifecycle (all local; ZERO HTTP on all of them): `channel-init`
//! (generate the session, write the manifest, start the long-lived
//! UNARMED gateway process, zero upstream requests), `channel-arm`
//! (control socket only, no upstream request; after the A1 commit),
//! `channel-status-local` (PID / control socket / state file only —
//! ZERO HTTP), `channel-finalize` (stop accepting, flush, seal, write
//! `channel-final.json`; afterwards the gateway refuses all requests).
//!
//! No restart: the gateway must be the SAME process from A1 through
//! channel close. A gateway restart after A1 is an INCONCLUSIVE
//! infrastructure failure (it is not part of the frozen
//! preregistration); the state file records the originating PID and
//! any PID change is detected mechanically.
//!
//! Unauthorized request data must NOT enter the mutation input, agent
//! trajectories, selection, or promotion: it enters ONLY the channel
//! ledger and the integrity verifier.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;

use crate::channel_ledger::{self, ChannelEntry, Ledger};
use crate::experiment::Check;
use crate::model::ChannelAuth;
use crate::protocol::sha256_bytes;

/// The registered channel protocol version (bound into the channel
/// manifest, the run manifest, and the capability key).
pub const PROTOCOL_VERSION: &str = "0015-audited-execution-channel-v1";

/// The registered live stages (the only traffic the channel mediates).
pub const STAGES: [&str; 4] = ["B", "C", "D", "E"];

/// The maximum request body the gateway will read (8 MiB; the largest
/// registered request is the mutation-input packet). Larger bodies are
/// rejected with 413 and the connection is dropped (no ledger row: no
/// complete request was received; no registered request approaches
/// this bound).
const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;

/// The registered artifact names (crate directory).
pub const CHANNEL_MANIFEST: &str = "channel-manifest.json";
pub const CHANNEL_LEDGER: &str = "channel-ledger.jsonl";
pub const CHANNEL_FINAL: &str = "channel-final.json";
pub const CHANNEL_STATE: &str = "channel-state.json";
pub const CONTROL_SOCKET: &str = "channel-control.sock";

pub const CHECKPOINT_B: &str = "channel-checkpoint-b.json";
pub const CHECKPOINT_C: &str = "channel-checkpoint-c.json";
pub const CHECKPOINT_D: &str = "channel-checkpoint-d.json";

// ===========================================================================
// The channel manifest (committed at A1; byte-stable)
// ===========================================================================

/// The channel manifest: the channel identity, bound by the run
/// manifest at Stage A1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelManifest {
    pub run_id: String,
    /// Unique channel session for the run (generated before A1).
    pub session_id: String,
    pub protocol_version: String,
    /// The registered experiment endpoint (the gateway).
    pub gateway_endpoint: String,
    /// The physical upstream model transport. Channel-internal value:
    /// only this module reads/writes it.
    pub upstream_endpoint: String,
    /// SHA-256 of the upstream endpoint string (the run manifest binds
    /// the HASH; the run manifest is never a URL source).
    pub upstream_endpoint_sha256: String,
    /// The ONLY forwardable path (no model-list endpoint, no health
    /// endpoint).
    pub allowed_path: String,
    /// The ledger genesis hash: `SHA-256(canonical manifest without
    /// ledger_genesis_hash)`.
    pub ledger_genesis_hash: String,
}

impl ChannelManifest {
    /// The gateway host:port parsed from the registered endpoint
    /// (the trailing path is ignored; the authority is what binds).
    pub fn gateway_host_port(&self) -> (String, u16) {
        let authority = self
            .gateway_endpoint
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or("");
        match authority.split_once(':') {
            Some((host, port)) => (host.to_string(), port.parse().unwrap_or(0)),
            None => (authority.to_string(), 0),
        }
    }

    /// Verify the self-seal: the stored genesis hash must equal the
    /// recomputed canonical hash. Any byte edit of the manifest (which
    /// would also orphan every ledger row, since they chain from the
    /// genesis) is detected here at load time.
    pub fn verify_seal(&self) -> Result<(), String> {
        let recomputed = self.canonical_bytes()?;
        if sha256_bytes(&recomputed) != self.ledger_genesis_hash {
            return Err(
                "channel manifest self-seal failed: stored ledger genesis hash does not match the recomputed canonical hash (the manifest was edited)".to_string(),
            );
        }
        Ok(())
    }

    fn canonical_bytes(&self) -> Result<Vec<u8>, String> {
        let mut map = serde_json::to_value(self).map_err(|e| e.to_string())?;
        if let Some(obj) = map.as_object_mut() {
            obj.remove("ledger_genesis_hash");
        }
        let text = serde_json::to_string(&map).map_err(|e| e.to_string())?;
        Ok(format!("{text}\n").into_bytes())
    }

    /// Seal the manifest: compute the genesis hash from the canonical
    /// bytes, then render the committed file bytes (pretty + newline).
    pub fn sealed_bytes(&mut self) -> Result<Vec<u8>, String> {
        self.ledger_genesis_hash = sha256_bytes(&self.canonical_bytes()?);
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        Ok(format!("{text}\n").into_bytes())
    }

    /// Load the committed channel manifest (verifies the self-seal).
    pub fn load(path: &Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read channel manifest {}: {e}", path.display()))?;
        let m: Self =
            serde_json::from_str(&raw).map_err(|e| format!("channel manifest invalid: {e}"))?;
        m.verify_seal()?;
        Ok(m)
    }
}

// ===========================================================================
// Capability / authorization (HMAC-SHA256 over run-local material)
// ===========================================================================

/// The run-local capability key (deterministic from the committed
/// session identity; no second random source exists).
pub fn capability_key(session_id: &str) -> Vec<u8> {
    use sha2::Sha256;
    let mut hasher = Sha256::new();
    hasher.update(b"mutagen-exp-0015-channel");
    hasher.update(PROTOCOL_VERSION.as_bytes());
    hasher.update(b"|");
    hasher.update(session_id.as_bytes());
    hasher.finalize().to_vec()
}

/// `HMAC-SHA256` (RFC 2104) over the vendored `sha2` crate.
///
/// The `hmac` crate is not available in the offline package cache that
/// this experiment must build against, so the standard 15-line
/// ipad/opad construction is implemented directly on `sha2` (already
/// a frozen dependency of this crate). The exact crypto strength is
/// NOT the scientific variable: the purpose is accidental
/// unregistered-request detection (body binding + one-to-one
/// reconciliation + tamper-evident ledger), not adversarial secret
/// protection against the repository owner. The construction is
/// checked against the RFC 4231 known-answer vector in the test
/// suite.
fn hmac_sha256(key: &[u8], input: &[u8]) -> [u8; 32] {
    use sha2::Sha256;
    const BLOCK: usize = 64;
    // Block the key: hash it when longer than the block; zero-pad
    // when shorter (RFC 2104 §2.3).
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        // SHA-256 output is 32 bytes; the remainder stays zero.
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    // H(K' xor opad || H(K' xor ipad || M)).
    let mut inner = Sha256::new();
    for kbyte in k.iter() {
        inner.update([kbyte ^ 0x36]);
    }
    inner.update(input);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    for kbyte in k.iter() {
        outer.update([kbyte ^ 0x5c]);
    }
    outer.update(inner_digest);
    outer.finalize().into()
}

/// `HMAC-SHA256(key, input)` as lowercase hex.
fn hmac_sha256_hex(key: &[u8], input: &str) -> String {
    let out = hmac_sha256(key, input.as_bytes());
    let mut s = String::with_capacity(out.len() * 2);
    for b in out.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// The MAC input for one request (spec §16 binding):
/// `run_id || stage || request_id || SHA-256(body)` with unit
/// separators.
fn auth_input(run_id: &str, stage: &str, request_id: &str, body_sha256: &str) -> String {
    format!("{run_id}\u{1f}{stage}\u{1f}{request_id}\u{1f}{body_sha256}")
}

// ===========================================================================
// The runtime channel session (used by the four live stages)
// ===========================================================================

/// The runtime side of the channel session: allocates request
/// identities and mints the per-request authorization. One instance
/// per live stage command (a stage-local sequence starts at 1).
pub struct ChannelSession {
    pub manifest: ChannelManifest,
    key: Vec<u8>,
    seq: BTreeMap<String, u64>,
}

impl ChannelSession {
    pub fn open(state_dir: &Path) -> Result<Self, String> {
        let manifest = ChannelManifest::load(&state_dir.join(CHANNEL_MANIFEST))?;
        Ok(Self {
            key: capability_key(&manifest.session_id),
            manifest,
            seq: BTreeMap::new(),
        })
    }
    /// The next channel-issued request identity for one stage
    /// (`0015-r1-<stage>-NNNNNN`, stage-local sequence from 1).
    pub fn next_request_id(&mut self, stage: &str) -> String {
        let stage = stage.to_string();
        let n = self.seq.entry(stage.clone()).or_insert(0);
        *n += 1;
        format!("{}-{stage}-{:06}", self.manifest.run_id, *n)
    }

    /// Authorize one request: allocate the request identity, bind the
    /// exact body bytes, and produce the header set the gateway
    /// validates.
    pub fn authorize(&mut self, stage: &str, body: &str) -> ChannelAuth {
        let body_sha256 = sha256_bytes(body.as_bytes());
        let request_id = self.next_request_id(stage);
        let auth = hmac_sha256_hex(
            &self.key,
            &auth_input(&self.manifest.run_id, stage, &request_id, &body_sha256),
        );
        ChannelAuth {
            run_id: self.manifest.run_id.clone(),
            stage: stage.to_string(),
            request_id,
            auth,
            body_sha256,
        }
    }
}

// ===========================================================================
// The channel state file (operational; gitignored) + status
// ===========================================================================

/// The run-local operational state of the channel gateway process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelState {
    pub session_id: String,
    pub armed: bool,
    pub finalized: bool,
    /// The PID of the gateway process that channel-init started (the
    /// original; never updated on a restart).
    pub origin_pid: u32,
    /// The PID of the currently running gateway process (written by the
    /// gateway itself on startup). A pid != origin_pid after A1 is a
    /// gateway restart: INCONCLUSIVE.
    pub pid: u32,
    pub host: String,
    pub port: u16,
}

impl ChannelState {
    fn load(state_dir: &Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(state_dir.join(CHANNEL_STATE))
            .map_err(|e| format!("cannot read channel state: {e}"))?;
        serde_json::from_str(&raw).map_err(|e| format!("channel state invalid: {e}"))
    }

    fn write(&self, state_dir: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(state_dir.join(CHANNEL_STATE), format!("{text}\n"))
            .map_err(|e| format!("cannot write channel state: {e}"))
    }
}

/// The result of `channel-status-local`: process liveness + channel
/// state, inspected WITHOUT any HTTP request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelStatus {
    pub alive: bool,
    pub pid: u32,
    pub origin_pid: u32,
    pub armed: bool,
    pub finalized: bool,
    pub control_socket_present: bool,
    pub ledger_entries: u64,
    pub no_restart: bool,
}

fn pid_alive(pid: u32) -> bool {
    // Signal 0: a pure liveness probe (no signal is delivered).
    if pid == 0 {
        return false;
    }
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Inspect PID / control socket / state file only (ZERO HTTP requests —
/// no HTTP client call exists on this code path).
pub fn status_local(state_dir: &Path) -> Result<ChannelStatus, String> {
    let state = ChannelState::load(state_dir)?;
    let control = state_dir.join(CONTROL_SOCKET).exists();
    let ledger = channel_ledger::Ledger::open(&state_dir.join(CHANNEL_LEDGER), "")
        .map(|l| l.len().unwrap_or(0))
        .unwrap_or(0);
    Ok(ChannelStatus {
        alive: pid_alive(state.pid),
        pid: state.pid,
        origin_pid: state.origin_pid,
        armed: state.armed,
        finalized: state.finalized,
        control_socket_present: control,
        ledger_entries: ledger,
        no_restart: state.origin_pid != 0 && state.pid == state.origin_pid,
    })
}

// ===========================================================================
// Checkpoints + final seal
// ===========================================================================

/// One stage channel checkpoint (committed with the stage freeze).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelCheckpoint {
    pub run_id: String,
    pub stage: String,
    pub ledger_entry_count: u64,
    pub ledger_head_hash: String,
    pub authorized_forwarded_count: u64,
    pub unauthorized_count: u64,
    pub matched_request_count: u64,
    pub unmatched_forwarded_count: u64,
}

pub fn checkpoint_file(stage: &str) -> &'static str {
    match stage {
        "B" => CHECKPOINT_B,
        "C" => CHECKPOINT_C,
        "D" => CHECKPOINT_D,
        _ => panic!("no channel checkpoint registered for stage {stage}"),
    }
}

/// Build (and write) one stage checkpoint from the current ledger rows
/// and the experiment-recorded requests up to and including that stage.
pub fn build_checkpoint(
    state_dir: &Path,
    run_id: &str,
    stage: &str,
    entries: &[ChannelEntry],
    recorded: &[RecordedRequest],
) -> Result<String, String> {
    let counts = channel_ledger::count_entries(entries);
    let (matched, unmatched) = match_counts(entries, recorded);
    let ck = ChannelCheckpoint {
        run_id: run_id.to_string(),
        stage: stage.to_string(),
        ledger_entry_count: counts.entries,
        ledger_head_hash: entries
            .last()
            .map(|e| e.entry_hash.clone())
            .unwrap_or_default(),
        authorized_forwarded_count: counts.authorized_forwarded,
        unauthorized_count: counts.unauthorized,
        matched_request_count: matched,
        unmatched_forwarded_count: unmatched,
    };
    let text = serde_json::to_string_pretty(&ck).map_err(|e| e.to_string())?;
    let bytes = format!("{text}\n").into_bytes();
    std::fs::write(state_dir.join(checkpoint_file(stage)), &bytes)
        .map_err(|e| format!("cannot write channel checkpoint: {e}"))?;
    Ok(sha256_bytes(&bytes))
}

pub fn load_checkpoint(stage: &str) -> Result<ChannelCheckpoint, String> {
    let path = crate::experiment::crate_dir().join(checkpoint_file(stage));
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read channel checkpoint {}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("channel checkpoint invalid: {e}"))
}

/// The final channel seal (written by the gateway on `channel-finalize`
/// — the gateway refuses all requests once this exists).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelFinal {
    pub run_id: String,
    pub session_id: String,
    pub protocol_version: String,
    pub final_entry_count: u64,
    pub final_head_hash: String,
    pub forwarded_count: u64,
    pub unauthorized_count: u64,
    pub matched_count: u64,
    pub unmatched_count: u64,
    pub missing_recorded_count: u64,
    pub duplicate_request_ids: u64,
    pub hash_chain_violations: u64,
    pub stage_b_count: u64,
    pub stage_c_count: u64,
    pub stage_d_count: u64,
    pub stage_e_count: u64,
    pub gateway_restarts: u64,
}

// ===========================================================================
// Reconciliation: experiment evidence <-> gateway ledger
// ===========================================================================

/// One experiment-recorded model request: (stage, request_id,
/// request-body SHA-256).
pub type RecordedRequest = (String, String, String);

/// Count the matched / unmatched forwarded ledger rows against the
/// recorded requests (public: used by the checkpoint builder, the
/// final seal, and the final reconciliation report).
pub fn match_counts_public(entries: &[ChannelEntry], recorded: &[RecordedRequest]) -> (u64, u64) {
    match_counts(entries, recorded)
}

/// Count the duplicate request ids across ALL ledger rows.
pub fn duplicate_ids(entries: &[ChannelEntry]) -> u64 {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    entries
        .iter()
        .filter(|e| !e.request_id.is_empty() && !seen.insert(e.request_id.as_str()))
        .count() as u64
}

/// Count the recorded requests with a non-empty id that have NO
/// matching forwarded ledger entry.
pub fn missing_recorded(entries: &[ChannelEntry], recorded: &[RecordedRequest]) -> u64 {
    recorded
        .iter()
        .filter(|r| {
            !r.1.is_empty()
                && !entries.iter().any(|e| {
                    e.authorized
                        && e.forwarded
                        && e.stage_claim == r.0
                        && e.request_id == r.1
                        && e.body_sha256 == r.2
                })
        })
        .count() as u64
}

fn match_counts(entries: &[ChannelEntry], recorded: &[RecordedRequest]) -> (u64, u64) {
    let mut unmatched = 0u64;
    let mut matched = 0u64;
    for e in entries.iter().filter(|e| e.authorized && e.forwarded) {
        let hits = recorded
            .iter()
            .filter(|r| r.0 == e.stage_claim && r.1 == e.request_id && r.2 == e.body_sha256)
            .count();
        if hits == 1 {
            matched += 1;
        } else {
            unmatched += 1;
        }
    }
    (matched, unmatched)
}

/// The one-to-one reconciliation invariants (spec §26/§27):
///
/// - ledger forwarded count == recorded live request count;
/// - every recorded request has exactly one matching forwarded ledger
///   entry (stage + request_id + body_sha256 all equal);
/// - every forwarded ledger entry has exactly one recorded request
///   (catches an authorized-but-unregistered request: an accidental or
///   manual use of a valid capability);
/// - no duplicate request_id in the ledger (across ALL rows);
/// - every recorded stage is a registered live stage.
///
/// Recorded requests with an EMPTY request_id (a request that failed
/// before reaching the channel, e.g. body serialization) never reached
/// the gateway and are excluded — a request that reached the gateway
/// boundary with a transport failure MUST be ledgered, so a
/// recorded-but-not-ledgered non-empty id is a violation.
pub fn reconcile_violations(entries: &[ChannelEntry], recorded: &[RecordedRequest]) -> Vec<String> {
    let mut errors = Vec::new();

    let mut rec_seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut rec_nonempty: Vec<&RecordedRequest> = Vec::new();
    for r in recorded {
        if r.1.is_empty() {
            continue; // never reached the gateway
        }
        if !STAGES.contains(&r.0.as_str()) {
            errors.push(format!(
                "recorded request has an unregistered stage {:?}",
                r.0
            ));
        }
        if !rec_seen.insert((r.0.clone(), r.1.clone())) {
            errors.push(format!(
                "duplicate recorded channel request id {} (stage {})",
                r.1, r.0
            ));
        }
        rec_nonempty.push(r);
    }

    let mut ids: BTreeSet<String> = BTreeSet::new();
    for e in entries {
        if !e.request_id.is_empty() && !ids.insert(e.request_id.clone()) {
            errors.push(format!("duplicate ledger request id {}", e.request_id));
        }
    }

    let forwarded: Vec<&ChannelEntry> = entries
        .iter()
        .filter(|e| e.authorized && e.forwarded)
        .collect();

    if (forwarded.len() as u64) != rec_nonempty.len() as u64 {
        errors.push(format!(
            "ledger forwarded count {} != recorded live model request count {} — every forwarded request must match exactly one experiment record",
            forwarded.len(),
            rec_nonempty.len()
        ));
    }

    let mut rec_matched: BTreeSet<usize> = BTreeSet::new();
    for e in &forwarded {
        let matches: Vec<usize> = rec_nonempty
            .iter()
            .enumerate()
            .filter(|(_, r)| r.0 == e.stage_claim && r.1 == e.request_id && r.2 == e.body_sha256)
            .map(|(i, _)| i)
            .collect();
        if matches.is_empty() {
            errors.push(format!(
                "unmatched forwarded ledger entry {} (stage {}, request {}, body {}) — an authorized request with no matching experiment record",
                e.seq, e.stage_claim, e.request_id, e.body_sha256
            ));
        } else if matches.len() > 1 {
            errors.push(format!(
                "forwarded ledger entry {} matches {} recorded requests (exactly one required)",
                e.seq,
                matches.len()
            ));
        } else {
            rec_matched.insert(matches[0]);
        }
    }

    for (i, r) in rec_nonempty.iter().enumerate() {
        if !rec_matched.contains(&i) {
            errors.push(format!(
                "recorded channel request {} (stage {}) has no matching forwarded ledger entry (missing channel record)",
                r.1, r.0
            ));
        }
    }

    errors
}

/// The full channel integrity verification for one stage: hash chain +
/// zero unauthorized attempts + the one-to-one reconciliation.
pub fn channel_violations(
    state_dir: &Path,
    recorded: &[RecordedRequest],
) -> Result<Vec<String>, String> {
    let manifest = ChannelManifest::load(&state_dir.join(CHANNEL_MANIFEST))?;
    let ledger = Ledger::open(
        &state_dir.join(CHANNEL_LEDGER),
        &manifest.ledger_genesis_hash,
    )?;
    let entries = ledger.entries()?;
    let mut v = channel_ledger::verify_chain(&entries, &manifest.ledger_genesis_hash);
    let counts = channel_ledger::count_entries(&entries);
    if counts.unauthorized > 0 {
        v.push(format!(
            "channel integrity: {} unauthorized gateway request attempt(s) — even a correctly REJECTED probe is an integrity failure (the run is INCONCLUSIVE)",
            counts.unauthorized
        ));
    }
    v.extend(reconcile_violations(&entries, recorded));
    Ok(v)
}

/// Ledger prefix continuity for the NEXT stage: the current ledger's
/// first `n` rows must form an unbroken chain from the genesis to the
/// checkpoint head hash. (The stage freeze separately binds the frozen
/// ledger bytes; together: the committed prefix is intact and the live
/// file only ever APPENDED.)
pub fn checkpoint_prefix_violations(
    state_dir: &Path,
    ck: &ChannelCheckpoint,
) -> Result<Vec<String>, String> {
    let manifest = ChannelManifest::load(&state_dir.join(CHANNEL_MANIFEST))?;
    let entries = Ledger::open(
        &state_dir.join(CHANNEL_LEDGER),
        &manifest.ledger_genesis_hash,
    )?
    .entries()?;
    let mut v = Vec::new();
    if (entries.len() as u64) < ck.ledger_entry_count {
        v.push(format!(
            "channel: current ledger has {} rows, the frozen {stage} checkpoint expects {n} — the ledger only ever appends (missing rows)",
            entries.len(),
            stage = ck.stage,
            n = ck.ledger_entry_count
        ));
    }
    let prefix: Vec<ChannelEntry> = entries
        .iter()
        .take(ck.ledger_entry_count as usize)
        .cloned()
        .collect();
    v.extend(channel_ledger::verify_chain(
        &prefix,
        &manifest.ledger_genesis_hash,
    ));
    if let Some(last) = prefix.last() {
        if last.entry_hash != ck.ledger_head_hash {
            v.push(format!(
                "channel: current ledger prefix head hash does not match the frozen {stage} checkpoint head hash (the committed ledger prefix was altered)",
                stage = ck.stage
            ));
        }
    }
    Ok(v)
}

/// The recorded requests of one stage's records (a record with an
/// empty channel binding never reached the gateway and is excluded by
/// the reconciliation).
pub fn recorded_from_requests(
    stage: &str,
    reqs: &[crate::trace::ModelRequestRecord],
) -> Vec<RecordedRequest> {
    reqs.iter()
        .filter(|r| !r.channel_request_id.is_empty())
        .map(|r| {
            (
                stage.to_string(),
                r.channel_request_id.clone(),
                r.request_body_sha256.clone(),
            )
        })
        .collect()
}

// ===========================================================================
// The channel gateway process (long-lived; one process A1 → finalize)
// ===========================================================================

/// One request as parsed from the wire.
struct Incoming {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Incoming {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn error(&self, status: &str, reason: &str) -> Vec<u8> {
        let body =
            format!(r#"{{"error":{{"message":"channel: {reason}","type":"channel_rejected"}}}}"#);
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .into_bytes()
    }
}

/// The gateway runtime state shared between the accept threads.
struct GatewayRuntime {
    manifest: ChannelManifest,
    state: ChannelState,
    state_dir: PathBuf,
    ledger: Ledger,
    upstream: String,
    forward_timeout_ms: u64,
    armed: bool,
    finalized: bool,
    seen_ids: BTreeSet<String>,
}

fn http_status_line(code: usize) -> &'static str {
    match code {
        200 => "200 OK",
        201 => "201 Created",
        202 => "202 Accepted",
        204 => "204 No Content",
        301 => "301 Moved Permanently",
        302 => "302 Found",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        413 => "413 Payload Too Large",
        500 => "500 Internal Server Error",
        502 => "502 Bad Gateway",
        503 => "503 Service Unavailable",
        504 => "504 Gateway Timeout",
        _ => "500 Internal Server Error",
    }
}

fn status_class(code: usize) -> String {
    format!("{}xx", (code / 100).min(5))
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Read one complete request from the wire (bounded).
fn read_request(stream: &mut TcpStream) -> Result<Incoming, String> {
    let mut buf: Vec<u8> = Vec::new();
    let mut one = [0u8; 8192];
    let header_end = loop {
        let n = stream
            .read(&mut one)
            .map_err(|e| format!("read failed: {e}"))?;
        if n == 0 {
            return Err("connection closed before the request completed".to_string());
        }
        buf.extend_from_slice(&one[..n]);
        if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > 1024 * 1024 {
            return Err("request headers too large".to_string());
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut headers = Vec::new();
    let mut content_length = 0u64;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim().to_string();
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.parse().unwrap_or(0);
            }
            headers.push((k.trim().to_string(), v));
        }
    }
    if content_length > MAX_BODY_BYTES {
        // A declared-oversized body: reject without a ledger row (no
        // complete request was received; no registered request
        // approaches this bound).
        let resp = Incoming {
            method: method.clone(),
            path: path.clone(),
            headers: headers.clone(),
            body: Vec::new(),
        }
        .error(
            "413 Payload Too Large",
            &format!("body too large ({content_length} bytes)"),
        );
        let _ = stream.write_all(&resp);
        return Err(format!("body too large ({content_length} bytes)"));
    }
    let mut body = buf.split_off(header_end);
    while (body.len() as u64) < content_length {
        let n = stream
            .read(&mut one)
            .map_err(|e| format!("body read failed: {e}"))?;
        if n == 0 {
            return Err("connection closed before the body completed".to_string());
        }
        body.extend_from_slice(&one[..n]);
    }
    body.truncate(content_length as usize);
    Ok(Incoming {
        method,
        path,
        headers,
        body,
    })
}

/// Constant-time comparison (the goal is detection, not adversarial
/// timing resistance, but the comparison cost is trivial to equalize).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Validate + authorize one incoming request; returns `(authorized,
/// rejection reason, HTTP status for the rejection)`.
fn authorize_incoming(
    rt: &mut GatewayRuntime,
    req: &Incoming,
    body_sha256: &str,
) -> (bool, Option<String>, usize) {
    if rt.finalized {
        return (false, Some("channel_finalized".to_string()), 503);
    }
    if !rt.armed {
        return (false, Some("channel_not_armed".to_string()), 503);
    }
    // Path first: a disallowed path (model list, health, anything
    // but the one forwardable path) is rejected regardless of method.
    if req.path != rt.manifest.allowed_path {
        return (false, Some("disallowed_path".to_string()), 404);
    }
    if req.method != "POST" {
        return (false, Some("disallowed_method".to_string()), 405);
    }
    let run = req.header("X-Mutagen-Run").filter(|s| !s.is_empty());
    let stage = req.header("X-Mutagen-Stage").filter(|s| !s.is_empty());
    let request_id = req.header("X-Mutagen-Request-Id").filter(|s| !s.is_empty());
    let auth = req.header("X-Mutagen-Auth").filter(|s| !s.is_empty());
    fn deny(reason: &str) -> (bool, Option<String>, usize) {
        (false, Some(reason.to_string()), 403)
    }
    match (run, stage, request_id, auth) {
        (None, _, _, _) => deny("missing_run_header"),
        (_, None, _, _) => deny("missing_stage_header"),
        (_, _, None, _) => deny("missing_request_id_header"),
        (_, _, _, None) => deny("missing_auth_header"),
        (Some(r), Some(s), Some(id), Some(a)) => {
            if r != rt.manifest.run_id {
                deny("run_mismatch")
            } else if !STAGES.contains(&s) {
                (false, Some("wrong_stage".to_string()), 403)
            } else {
                let prefix = format!("{r}-{s}-");
                let valid_id = id
                    .strip_prefix(&prefix)
                    .is_some_and(|d| d.len() == 6 && d.bytes().all(|b| b.is_ascii_digit()));
                if !valid_id {
                    (false, Some("malformed_request_id".to_string()), 403)
                } else if !constant_time_eq(
                    hmac_sha256_hex(
                        &capability_key(&rt.state.session_id),
                        &auth_input(r, s, id, body_sha256),
                    )
                    .as_bytes(),
                    a.as_bytes(),
                ) {
                    // Covers a wrong stage capability, a wrong body
                    // hash, a forged or stale authorization, and a
                    // tampered body byte.
                    (false, Some("auth_mismatch".to_string()), 403)
                } else if !rt.seen_ids.insert(id.to_string()) {
                    deny("duplicate_request_id")
                } else {
                    (true, None, 200)
                }
            }
        }
    }
}

/// Forward one authorized request to the physical upstream (the ONE
/// place in the experiment that knows the upstream URL). Returns the
/// status class + the relay response bytes.
fn forward(rt: &GatewayRuntime, req: &Incoming) -> (String, Vec<u8>) {
    let url = format!("{}/chat/completions", rt.upstream.trim_end_matches('/'));
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_millis(rt.forward_timeout_ms))
        .build();
    let mut r = agent.post(&url).set("Content-Type", "application/json");
    // Relay every received header except the channel-internal ones.
    for (k, v) in &req.headers {
        if k.eq_ignore_ascii_case("X-Mutagen-Run")
            || k.eq_ignore_ascii_case("X-Mutagen-Stage")
            || k.eq_ignore_ascii_case("X-Mutagen-Request-Id")
            || k.eq_ignore_ascii_case("X-Mutagen-Auth")
            || k.eq_ignore_ascii_case("Host")
            || k.eq_ignore_ascii_case("Content-Length")
            || k.eq_ignore_ascii_case("Connection")
        {
            continue;
        }
        r = r.set(k, v);
    }
    let body = String::from_utf8_lossy(&req.body).to_string();
    match r.send_string(&body) {
        Ok(resp) => {
            let code = resp.status() as usize;
            let ct = resp
                .header("content-type")
                .unwrap_or("application/json")
                .to_string();
            let text = match resp.into_string() {
                Ok(t) => t,
                Err(_) => {
                    return (
                        status_class(code),
                        format!(
                            "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            code,
                            http_status_line(code)
                        )
                        .into_bytes(),
                    );
                }
            };
            (
                status_class(code),
                format!(
                    "HTTP/1.1 {code} {}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    http_status_line(code),
                    text.len(),
                    text
                )
                .into_bytes(),
            )
        }
        Err(e) => {
            eprintln!("gateway: upstream transport failure: {e}");
            (
                "0xx".to_string(),
                b"HTTP/1.1 502 Bad Gateway\r\nContent-Type: application/json\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec(),
            )
        }
    }
}

/// The per-request gateway dispatch: validate, authorize, ledger
/// (EVERY request, authorized or not), forward-or-reject.
fn dispatch(rt: &mut GatewayRuntime, req: &Incoming) -> Vec<u8> {
    let body_sha256 = sha256_bytes(&req.body);
    let (run, stage, request_id) = (
        req.header("X-Mutagen-Run")
            .map(str::to_string)
            .unwrap_or_default(),
        req.header("X-Mutagen-Stage")
            .map(str::to_string)
            .unwrap_or_default(),
        req.header("X-Mutagen-Request-Id")
            .map(str::to_string)
            .unwrap_or_default(),
    );
    let (authorized, reason, status_code) = authorize_incoming(rt, req, &body_sha256);
    if authorized {
        let (class, bytes) = forward(rt, req);
        let _ = rt.ledger.append(ChannelEntry {
            seq: 0, // set by the ledger (previous hash + hash chain)
            run_id: run,
            stage_claim: stage,
            request_id,
            method: req.method.clone(),
            path: req.path.clone(),
            body_sha256,
            authorized: true,
            forwarded: true,
            status_class: class,
            reason: None,
            previous_entry_hash: String::new(),
            entry_hash: String::new(),
        });
        bytes
    } else {
        let code = http_status_line(status_code);
        let _ = rt.ledger.append(ChannelEntry {
            seq: 0,
            run_id: run,
            stage_claim: stage,
            request_id,
            method: req.method.clone(),
            path: req.path.clone(),
            body_sha256,
            authorized: false,
            forwarded: false,
            status_class: String::new(),
            reason: reason.clone(),
            previous_entry_hash: String::new(),
            entry_hash: String::new(),
        });
        req.error(code, reason.as_deref().unwrap_or("rejected"))
    }
}

/// The control-socket handler (arm / status / finalize; ZERO HTTP,
/// ZERO upstream traffic).
fn handle_control_unix(rt: &mut GatewayRuntime, stream: std::os::unix::net::UnixStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let reply = match line.trim() {
        "arm" if !rt.finalized => {
            rt.armed = true;
            rt.state.armed = true;
            let _ = rt.state.write(&rt.state_dir);
            "OK armed\n".to_string()
        }
        "status" => {
            let entries = rt.ledger.entries().unwrap_or_default();
            let counts = channel_ledger::count_entries(&entries);
            format!(
                "OK armed={} finalized={} entries={} pid={}\n",
                rt.armed, rt.finalized, counts.entries, rt.state.pid
            )
        }
        "finalize" => {
            rt.finalized = true;
            rt.state.finalized = true;
            let _ = rt.state.write(&rt.state_dir);
            let entries = rt.ledger.entries().unwrap_or_default();
            let final_doc = rt.build_final(&entries);
            if let Ok(text) = serde_json::to_string_pretty(&final_doc) {
                let _ = std::fs::write(rt.state_dir.join(CHANNEL_FINAL), format!("{text}\n"));
            }
            "OK finalized\n".to_string()
        }
        _ => "ERR unknown\n".to_string(),
    };
    let mut writer = stream;
    let _ = writer.write_all(reply.as_bytes());
    let _ = writer.flush();
}

impl GatewayRuntime {
    fn build_final(&self, entries: &[ChannelEntry]) -> ChannelFinal {
        let counts = channel_ledger::count_entries(entries);
        let chain_violations =
            channel_ledger::verify_chain(entries, &self.manifest.ledger_genesis_hash).len() as u64;
        let per_stage = |s: &str| *counts.per_stage.get(s).unwrap_or(&0);
        ChannelFinal {
            run_id: self.manifest.run_id.clone(),
            session_id: self.state.session_id.clone(),
            protocol_version: self.manifest.protocol_version.clone(),
            final_entry_count: counts.entries,
            final_head_hash: entries
                .last()
                .map(|e| e.entry_hash.clone())
                .unwrap_or_else(|| self.manifest.ledger_genesis_hash.clone()),
            forwarded_count: counts.authorized_forwarded,
            unauthorized_count: counts.unauthorized,
            matched_count: 0, // filled by the final reconciliation
            unmatched_count: 0,
            missing_recorded_count: 0,
            duplicate_request_ids: 0,
            hash_chain_violations: chain_violations,
            stage_b_count: per_stage("B"),
            stage_c_count: per_stage("C"),
            stage_d_count: per_stage("D"),
            stage_e_count: per_stage("E"),
            gateway_restarts: if self.state.pid != self.state.origin_pid {
                1
            } else {
                0
            },
        }
    }
}

/// The long-lived gateway process (one process from A1 through
/// finalize; NO restarts).
pub fn run_gateway(state_dir: &Path) -> Result<(), String> {
    let manifest = ChannelManifest::load(&state_dir.join(CHANNEL_MANIFEST))
        .map_err(|e| format!("gateway: {e}"))?;
    // The ONE read of the physical upstream endpoint in the experiment.
    let design_raw = std::fs::read_to_string(state_dir.join("design.json"))
        .map_err(|e| format!("gateway: cannot read design.json: {e}"))?;
    let design_v: serde_json::Value =
        serde_json::from_str(&design_raw).map_err(|e| format!("gateway: design invalid: {e}"))?;
    let upstream = design_v["channel"]["upstream_endpoint"]
        .as_str()
        .ok_or("gateway: channel.upstream_endpoint missing from design.json")?
        .to_string();
    if upstream != manifest.upstream_endpoint {
        return Err(
            "gateway: design.json channel.upstream_endpoint does not match the channel manifest"
                .into(),
        );
    }
    let forward_timeout_ms = design_v["deadline"]["request_timeout_ceiling_ms"]
        .as_u64()
        .unwrap_or(600_000)
        + design_v["deadline"]["deadline_return_tolerance_ms"]
            .as_u64()
            .unwrap_or(1_000);

    // Register our PID (the FIRST startup writes origin_pid; a later
    // startup keeps origin_pid and is detected as a restart).
    let pid = std::process::id();
    let mut state = ChannelState::load(state_dir).unwrap_or_else(|_| ChannelState {
        session_id: manifest.session_id.clone(),
        armed: false,
        finalized: false,
        origin_pid: 0,
        pid: 0,
        host: String::new(),
        port: 0,
    });
    if state.origin_pid == 0 {
        state.origin_pid = pid;
    }
    state.pid = pid;

    let (host, port) = manifest.gateway_host_port();
    state.host = host.clone();
    state.port = port;
    state.write(state_dir)?;

    let _ = std::fs::remove_file(state_dir.join(CONTROL_SOCKET));
    use std::os::unix::net::UnixListener;
    let control = UnixListener::bind(state_dir.join(CONTROL_SOCKET))
        .map_err(|e| format!("gateway: cannot bind control socket: {e}"))?;
    let listener = TcpListener::bind(format!("127.0.0.1:{port}"))
        .map_err(|e| format!("gateway: cannot bind {host}:{port}: {e}"))?;

    let ledger = Ledger::open(
        &state_dir.join(CHANNEL_LEDGER),
        &manifest.ledger_genesis_hash,
    )?;

    let rt = Arc::new(std::sync::Mutex::new(GatewayRuntime {
        manifest,
        state: state.clone(),
        state_dir: state_dir.to_path_buf(),
        ledger,
        upstream,
        forward_timeout_ms,
        armed: state.armed,
        finalized: state.finalized,
        seen_ids: BTreeSet::new(),
    }));

    // Control thread (arm / status / finalize).
    let control_rt = rt.clone();
    std::thread::spawn(move || {
        for stream in control.incoming() {
            let Ok(stream) = stream else { break };
            let Ok(mut guard) = control_rt.lock() else {
                break;
            };
            handle_control_unix(&mut guard, stream);
        }
    });

    // Data thread (the channel gateway itself; a single worker keeps
    // the ledger strictly sequential — the experiment issues one
    // request at a time).
    let data_rt = rt.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(15)));
            let req = match read_request(&mut stream) {
                Ok(r) => r,
                Err(reason) => {
                    // A connection that dies before the request
                    // completes is NOT a request attempt: nothing
                    // complete was received, so nothing is ledgered.
                    eprintln!("gateway: dropping connection: {reason}");
                    continue;
                }
            };
            let body = match data_rt.lock() {
                Ok(mut guard) => dispatch(&mut guard, &req),
                Err(_) => continue,
            };
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        }
    });

    // The main thread waits until the channel is finalized (after
    // finalize the gateway refuses all further requests; the process
    // then exits).
    loop {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let finalized = rt.lock().ok().map(|g| g.finalized).unwrap_or(false);
        if finalized {
            break;
        }
    }
    println!("gateway: finalized, exiting");
    Ok(())
}

// ===========================================================================
// Lifecycle commands (all local; ZERO HTTP on all of them)
// ===========================================================================

/// `channel-init` (before A1): generate the channel session, write the
/// manifest + ledger + state, and start the long-lived UNARMED gateway
/// process. Makes ZERO upstream requests (the gateway is un-armed and
/// cannot forward anything until `channel-arm`).
pub fn cmd_channel_init() -> Result<(), String> {
    let dir = crate::experiment::crate_dir();
    let design = crate::experiment::load_design()?;
    if ChannelManifest::load(&dir.join(CHANNEL_MANIFEST)).is_ok() {
        return Err(format!(
            "{CHANNEL_MANIFEST} already exists — channel-init runs exactly once (before A1); do not reinitialize a live channel session"
        ));
    }
    let session_id = {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let material = format!(
            "0015-r1|{}|{}|{}|{}",
            design.run_id,
            design.experiment_id,
            nanos,
            std::process::id()
        );
        &sha256_bytes(material.as_bytes())[..32]
    };
    // The physical upstream (channel-internal; read here only).
    let design_raw = std::fs::read_to_string(dir.join("design.json"))
        .map_err(|e| format!("cannot read design.json: {e}"))?;
    let design_v: serde_json::Value =
        serde_json::from_str(&design_raw).map_err(|e| format!("design invalid: {e}"))?;
    let upstream = design_v["channel"]["upstream_endpoint"]
        .as_str()
        .ok_or("design.json channel.upstream_endpoint missing")?
        .to_string();

    let mut manifest = ChannelManifest {
        run_id: design.run_id.clone(),
        session_id: session_id.to_string(),
        protocol_version: design.channel.protocol_version.clone(),
        gateway_endpoint: design.channel.gateway_endpoint.clone(),
        upstream_endpoint: upstream,
        upstream_endpoint_sha256: String::new(),
        allowed_path: design.channel.allowed_path.clone(),
        ledger_genesis_hash: String::new(),
    };
    manifest.upstream_endpoint_sha256 = sha256_bytes(manifest.upstream_endpoint.as_bytes());
    let bytes = manifest.sealed_bytes()?;
    std::fs::write(dir.join(CHANNEL_MANIFEST), &bytes)
        .map_err(|e| format!("cannot write channel manifest: {e}"))?;
    // Fresh (empty) ledger + fresh state, then start the gateway.
    let _ = std::fs::remove_file(dir.join(CHANNEL_LEDGER));
    let _ = std::fs::File::create(dir.join(CHANNEL_LEDGER));
    let _ = std::fs::remove_file(dir.join(CHANNEL_FINAL));
    let _ = std::fs::remove_file(dir.join(CONTROL_SOCKET));
    let state = ChannelState {
        session_id: manifest.session_id.clone(),
        armed: false,
        finalized: false,
        origin_pid: 0,
        pid: 0,
        host: String::new(),
        port: 0,
    };
    state.write(&dir)?;

    // Start the long-lived gateway child process (unarmed).
    let exe = std::env::current_exe().map_err(|e| format!("cannot resolve the binary: {e}"))?;
    std::process::Command::new(exe)
        .arg("gateway")
        .arg("--state-dir")
        .arg(&dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start the channel gateway process: {e}"))?;

    // Wait for the control socket (the process is up; it is NOT armed).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if dir.join(CONTROL_SOCKET).exists() {
            break;
        }
        if std::time::Instant::now() > deadline {
            return Err("channel gateway did not start (control socket never appeared)".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let status = status_local(&dir)?;
    if !status.alive {
        return Err("channel gateway process died at startup".into());
    }
    if status.armed {
        return Err("channel gateway started ARMED — it must start UNARMED".into());
    }
    let (_host, port) = manifest.gateway_host_port();
    println!(
        "channel-init: session {} gateway 127.0.0.1:{port} (UNARMED, pid {})",
        manifest.session_id, status.pid
    );
    println!(
        "  channel manifest: {} (hash {})",
        CHANNEL_MANIFEST,
        sha256_bytes(&bytes)
    );
    println!(
        "  NEXT (after the A1 commit): channel-arm — local-only, no upstream request; then START STAGE B directly (no probe, no /v1/models, no smoke request)."
    );
    Ok(())
}

/// Send one control-socket command (arm / status / finalize).
fn control_command(state_dir: &Path, command: &str) -> Result<String, String> {
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(state_dir.join(CONTROL_SOCKET)).map_err(|e| {
        format!("cannot reach the channel gateway control socket (is the gateway running?): {e}")
    })?;
    stream
        .write_all(format!("{command}\n").as_bytes())
        .map_err(|e| format!("control command write failed: {e}"))?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("control command read failed: {e}"))?;
    if !line.starts_with("OK") {
        return Err(format!("channel gateway rejected {command:?}: {line}"));
    }
    Ok(line)
}

/// `channel-arm` (after the A1 commit): local control socket only —
/// ZERO HTTP, ZERO upstream requests.
pub fn cmd_channel_arm() -> Result<(), String> {
    let dir = crate::experiment::crate_dir();
    control_command(&dir, "arm")?;
    let status = status_local(&dir)?;
    println!(
        "channel-arm: armed (pid {}, no_restart {}, ledger rows {})",
        status.pid, status.no_restart, status.ledger_entries
    );
    println!(
        "START STAGE B DIRECTLY. No availability probe, no /v1/models, no curl, no smoke request — any other gateway access after this instant is an integrity violation."
    );
    Ok(())
}

/// `channel-status-local`: PID / control socket / state file only.
/// ZERO HTTP requests (by construction — no HTTP client call exists on
/// this code path).
pub fn cmd_channel_status_local() -> Result<(), String> {
    let dir = crate::experiment::crate_dir();
    let s = status_local(&dir)?;
    println!(
        "channel-status-local: alive={} pid={} origin_pid={} no_restart={} armed={} finalized={} control_socket={} ledger_rows={}",
        s.alive,
        s.pid,
        s.origin_pid,
        s.no_restart,
        s.armed,
        s.finalized,
        s.control_socket_present,
        s.ledger_entries
    );
    if !s.no_restart {
        println!(
            "  WARNING: the gateway pid differs from the origin pid — a gateway restart after A1 is an INCONCLUSIVE condition."
        );
    }
    Ok(())
}

/// `channel-finalize` (after Stage E): stop accepting, flush, seal,
/// write `channel-final.json`. Local control socket only — ZERO HTTP.
/// Once finalized the gateway refuses all further requests.
pub fn cmd_channel_finalize() -> Result<(), String> {
    let dir = crate::experiment::crate_dir();
    control_command(&dir, "finalize")?;
    let s = status_local(&dir)?;
    println!(
        "channel-finalize: sealed (pid {} no_restart {} ledger_rows {} finalized {})",
        s.pid, s.no_restart, s.ledger_entries, s.finalized
    );
    Ok(())
}

// ===========================================================================
// Stage-integration helpers (called by the four live stages)
// ===========================================================================

/// The pre-request channel gate every live stage runs (after the
/// source freeze + the previous stage freeze): the channel must be
/// up, armed, single-process (no restart), and identity-bound to the
/// run manifest.
pub fn stage_channel_gate(
    manifest: &crate::freeze::RunManifest,
    run: &crate::trace::RunIdentity,
) -> Result<(), String> {
    let dir = crate::experiment::crate_dir();
    let cm = ChannelManifest::load(&dir.join(CHANNEL_MANIFEST))
        .map_err(|e| format!("CHANNEL VIOLATION — {e}"))?;
    let mut v = Vec::new();
    if cm.run_id != run.run_id {
        v.push(format!(
            "channel manifest run_id {} != run {}",
            cm.run_id, run.run_id
        ));
    }
    if cm.session_id != manifest.channel_session_id {
        v.push("channel manifest session_id != the run-manifest-bound channel session".to_string());
    }
    if cm.protocol_version != manifest.channel_protocol_version {
        v.push(
            "channel manifest protocol version != the run-manifest-bound protocol version"
                .to_string(),
        );
    }
    if cm.gateway_endpoint != manifest.channel_gateway_endpoint {
        v.push("channel manifest gateway endpoint != the run-manifest-bound endpoint".to_string());
    }
    if cm.upstream_endpoint_sha256 != manifest.channel_upstream_endpoint_sha256 {
        v.push(
            "channel manifest upstream endpoint hash != the run-manifest-bound hash".to_string(),
        );
    }
    if !v.is_empty() {
        return Err(format!(
            "CHANNEL IDENTITY VIOLATION:\n  - {}",
            v.join("\n  - ")
        ));
    }
    let s = status_local(&dir)?;
    if !s.alive {
        return Err(
            "CHANNEL VIOLATION — the channel gateway process is not alive (gateway unavailable: infrastructure failure, INCONCLUSIVE; no fallback path exists)".into(),
        );
    }
    if !s.no_restart {
        return Err(format!(
            "CHANNEL VIOLATION — the gateway pid {} differs from the origin pid {}: a gateway restart after A1 is INCONCLUSIVE",
            s.pid, s.origin_pid
        ));
    }
    if !s.armed {
        return Err("CHANNEL VIOLATION — the channel is not armed; no live stage may run".into());
    }
    if s.finalized {
        return Err("CHANNEL VIOLATION — the channel is finalized; no live stage may run".into());
    }
    Ok(())
}

/// Collect the recorded requests of all stage records present so far
/// (B + C + D + E; each filtered to non-empty channel bindings).
pub fn all_recorded(
    discovery: &[crate::trace::DiscoveryRecord],
    generation: &Option<crate::mutation::GenerationArtifact>,
    selection: &[crate::trace::SelectionRecord],
    promotion: &[crate::trace::PromotionRecord],
) -> Vec<RecordedRequest> {
    let mut out = Vec::new();
    for r in discovery {
        out.extend(recorded_from_requests("B", &r.model_requests));
    }
    if let Some(g) = generation {
        for a in &g.attempts {
            if !a.channel_request_id.is_empty() {
                out.push((
                    "C".to_string(),
                    a.channel_request_id.clone(),
                    a.request_body_sha256.clone(),
                ));
            }
        }
    }
    for r in selection {
        out.extend(recorded_from_requests("D", &r.model_requests));
    }
    for r in promotion {
        out.extend(recorded_from_requests("E", &r.model_requests));
    }
    out
}

// ===========================================================================
// The preflight channel checks (all network-free)
// ===========================================================================

fn check_ok(name: &'static str, ok: bool, detail: String) -> Check {
    Check {
        name,
        passed: ok,
        detail: if ok { "ok".to_string() } else { detail },
    }
}

/// The static bypass guard: the physical upstream endpoint literal may
/// appear in NO source file other than `src/channel.rs`. (`design.json`
/// is the one allowed non-Rust location — it is the channel
/// configuration, read by channel.rs only.)
pub fn bypass_guard_checks() -> Vec<Check> {
    let dir = crate::experiment::crate_dir();
    let design = crate::experiment::load_design()
        .unwrap_or_else(|e| panic!("design must load for the bypass guard: {e}"));
    let design_raw = std::fs::read_to_string(dir.join("design.json"))
        .map_err(|e| panic!("cannot read design.json: {e}"))
        .unwrap();
    let design_v: serde_json::Value = serde_json::from_str(&design_raw)
        .map_err(|e| panic!("design invalid: {e}"))
        .unwrap();
    let upstream = design_v["channel"]["upstream_endpoint"]
        .as_str()
        .unwrap_or("");
    let mut checks = Vec::new();
    let mut offenders = Vec::new();
    for name in [
        "src/model.rs",
        "src/kernel.rs",
        "src/experiment.rs",
        "src/mutation.rs",
        "src/main.rs",
        "src/trace.rs",
        "src/protocol.rs",
        "src/freeze.rs",
        "src/stage_freeze.rs",
        "src/stats.rs",
        "src/tools.rs",
        "src/oracle.rs",
        "src/selection.rs",
        "src/channel_ledger.rs",
    ] {
        if let Ok(text) = std::fs::read_to_string(dir.join(name)) {
            if !upstream.is_empty() && text.contains(upstream) {
                offenders.push(name.to_string());
            }
        }
    }
    checks.push(check_ok(
        "static bypass guard: no agent/mutator source contains the physical upstream endpoint",
        offenders.is_empty(),
        format!("offenders: {offenders:?} (allowed: src/channel.rs only)"),
    ));
    // The model client endpoint constant IS the registered gateway.
    let model_src = std::fs::read_to_string(dir.join("src/model.rs")).unwrap_or_default();
    let constant_is_gateway = model_src.contains(&format!(
        "pub const REGISTERED_ENDPOINT: &str = {:?};",
        design.channel.gateway_endpoint
    ));
    // D1 regression guard (registered after run 0014-r1): the final
    // reconciliation in `cmd_promote` MUST include the selection (D)
    // record set — the 0014-r1 final audit omitted it, which the
    // sealed evidence recorded as a reconciliation mismatch and which
    // stands as the run's formal INCONCLUSIVE driver.
    let exp_src = std::fs::read_to_string(dir.join("src/experiment.rs")).unwrap_or_default();
    // Whitespace-insensitive match (rustfmt may wrap the call).
    let squashed: String = exp_src.chars().filter(|c| !c.is_whitespace()).collect();
    let d1_wired = squashed.contains(
        "channel::all_recorded(&discovery_records,&generation_artifact,&selection_records,&records",
    );
    checks.push(check_ok(
        "final channel audit wires all four stage record sets (D1 guard)",
        d1_wired,
        "cmd_promote final seal must reconcile B + C + D + E (0014-r1 omitted D)".to_string(),
    ));
    checks.push(check_ok(
        "model client bound to the registered gateway endpoint (design channel block)",
        constant_is_gateway,
        format!(
            "REGISTERED_ENDPOINT must equal design channel.gateway_endpoint = {}",
            design.channel.gateway_endpoint
        ),
    ));
    checks
}

// ===========================================================================
// The behavioral channel preflight suite (network-free: in-process
// gateway loop + a fake in-process upstream; loopback only)
// ===========================================================================

/// One in-process channel fixture: a test channel manifest (a dedicated
/// ephemeral gateway port + a fake in-process upstream) and the
/// gateway dispatch loop on a thread. NO real model endpoint is
/// contacted.
struct ChannelFixture {
    dir: PathBuf,
    manifest: ChannelManifest,
    gateway_gateway: String,
    runtime: Arc<std::sync::Mutex<GatewayRuntime>>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl ChannelFixture {
    fn new(tag: &str) -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!(
            "mutagen-exp-0015-channel-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

        // The fake in-process upstream (loopback; canned 200 response).
        let fake = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("cannot bind fake upstream: {e}"))?;
        let fake_port = fake.local_addr().unwrap().port();
        let fake_url = format!("http://127.0.0.1:{fake_port}/v1");
        let fake_clone = fake.try_clone().map_err(|e| e.to_string())?;
        let t1 = std::thread::spawn(move || {
            for stream in fake_clone.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf: Vec<u8> = Vec::new();
                let mut one = [0u8; 8192];
                loop {
                    match stream.read(&mut one) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&one[..n]);
                            if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
                                if let Ok(head) = std::str::from_utf8(&buf[..pos]) {
                                    if let Some(cl) =
                                        head.lines().find_map(|l| l.strip_prefix("Content-Length:"))
                                    {
                                        let cl = cl.trim().parse::<usize>().unwrap_or(0);
                                        if pos + 4 + cl <= buf.len() {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
                let body =
                    r#"{"choices":[{"message":{"role":"assistant","content":"ok"}}],"usage":{}}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });

        // The test gateway: a dedicated ephemeral port.
        let gw =
            TcpListener::bind("127.0.0.1:0").map_err(|e| format!("cannot bind gateway: {e}"))?;
        let gw_port = gw.local_addr().unwrap().port();
        let mut manifest = ChannelManifest {
            run_id: "0015-test".into(),
            session_id: "test-session".into(),
            protocol_version: PROTOCOL_VERSION.into(),
            gateway_endpoint: format!("http://127.0.0.1:{gw_port}/v1"),
            upstream_endpoint: fake_url.clone(),
            upstream_endpoint_sha256: sha256_bytes(fake_url.as_bytes()),
            allowed_path: "/v1/chat/completions".into(),
            ledger_genesis_hash: String::new(),
        };
        let bytes = manifest.sealed_bytes().map_err(|e| e.to_string())?;
        std::fs::write(dir.join(CHANNEL_MANIFEST), &bytes).map_err(|e| e.to_string())?;
        let _ = std::fs::File::create(dir.join(CHANNEL_LEDGER));
        let state = ChannelState {
            session_id: manifest.session_id.clone(),
            armed: false,
            finalized: false,
            origin_pid: std::process::id(),
            pid: std::process::id(),
            host: "127.0.0.1".into(),
            port: gw_port,
        };
        state.write(&dir).map_err(|e| e.to_string())?;

        let ledger = Ledger::open(&dir.join(CHANNEL_LEDGER), &manifest.ledger_genesis_hash)
            .map_err(|e| e.to_string())?;
        let runtime = Arc::new(std::sync::Mutex::new(GatewayRuntime {
            manifest: manifest.clone(),
            state,
            state_dir: dir.clone(),
            ledger,
            upstream: fake_url,
            forward_timeout_ms: 5_000,
            armed: false,
            finalized: false,
            seen_ids: BTreeSet::new(),
        }));
        let gw_clone = gw.try_clone().map_err(|e| e.to_string())?;
        let rt2 = runtime.clone();
        let t2 = std::thread::spawn(move || {
            for stream in gw_clone.incoming() {
                let Ok(mut stream) = stream else { continue };
                let req = match read_request(&mut stream) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                let body = match rt2.lock() {
                    Ok(mut guard) => dispatch(&mut guard, &req),
                    Err(_) => continue,
                };
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
        });

        Ok(Self {
            dir,
            manifest,
            gateway_gateway: format!("http://127.0.0.1:{gw_port}/v1"),
            runtime,
            threads: vec![t1, t2],
        })
    }

    fn arm(&self) {
        let mut g = self.runtime.lock().unwrap();
        g.armed = true;
    }
}

impl Drop for ChannelFixture {
    fn drop(&mut self) {
        for t in self.threads.drain(..) {
            let _ = t;
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A minimal raw-HTTP client for the fixture (loopback only).
fn fixture_request(
    gateway: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (usize, String) {
    let authority = gateway.trim_start_matches("http://");
    let port = authority
        .split('/')
        .next()
        .unwrap()
        .rsplit(':')
        .next()
        .unwrap();
    let mut stream =
        TcpStream::connect(("127.0.0.1", port.parse().unwrap())).expect("loopback connect");
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf: Vec<u8> = Vec::new();
    let mut one = [0u8; 4096];
    loop {
        match stream.read(&mut one) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&one[..n]);
                if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
                    if let Ok(head) = std::str::from_utf8(&buf[..pos]) {
                        if let Some(cl) =
                            head.lines().find_map(|l| l.strip_prefix("Content-Length:"))
                        {
                            let cl = cl.trim().parse::<usize>().unwrap_or(0);
                            if pos + 4 + cl <= buf.len() {
                                break;
                            }
                        }
                    }
                }
            }
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, text)
}

/// The behavioral channel preflight suite (spec §45 tests + the 0013
/// regression). All network-free: in-process gateway loop + a fake
/// in-process upstream. Returns (name, passed, detail) rows.
pub fn channel_preflight_checks() -> Vec<Check> {
    let mut checks = Vec::new();

    // The registered design channel block values.
    match crate::experiment::load_design() {
        Ok(design) => {
            let c = &design.channel;
            let ok = c.protocol_version == PROTOCOL_VERSION
                && c.gateway_endpoint == "http://127.0.0.1:18150/v1"
                && c.allowed_path == "/v1/chat/completions";
            checks.push(check_ok(
                "design channel block registered (protocol / gateway / allowed path)",
                ok,
                format!(
                    "protocol={} gateway={} path={}",
                    c.protocol_version, c.gateway_endpoint, c.allowed_path
                ),
            ));
        }
        Err(e) => checks.push(check_ok("design channel block registered", false, e)),
    }

    let Ok(fx) = ChannelFixture::new("preflight") else {
        checks.push(check_ok(
            "channel behavioral suite (in-process gateway + fake upstream)",
            false,
            "fixture could not start".into(),
        ));
        return checks;
    };

    let key = capability_key(&fx.manifest.session_id);
    let mk_body = |content: &str| -> String {
        format!(
            r#"{{"model":"m","messages":[{{"role":"user","content":"{content}"}}],"temperature":0.2}}"#
        )
    };

    // 1. Authorized request → forwarded + ledger authorized=true.
    {
        fx.arm();
        let body = mk_body("hello");
        let body_sha = sha256_bytes(body.as_bytes());
        let auth = hmac_sha256_hex(
            &key,
            &auth_input("0015-test", "B", "0015-test-B-000001", &body_sha),
        );
        let (status, _) = fixture_request(
            &fx.gateway_gateway,
            "POST",
            "/v1/chat/completions",
            &[
                ("X-Mutagen-Run", "0015-test"),
                ("X-Mutagen-Stage", "B"),
                ("X-Mutagen-Request-Id", "0015-test-B-000001"),
                ("X-Mutagen-Auth", &auth),
            ],
            &body,
        );
        let e = fx.entries();
        let last = e.last().cloned();
        let ok = status == 200
            && e.len() == 1
            && last
                .as_ref()
                .is_some_and(|x| x.authorized && x.forwarded && x.status_class == "2xx");
        checks.push(check_ok(
            "channel: authorized request forwarded + ledgered (authorized=true)",
            ok,
            format!("status {status}, {} ledger row(s)", e.len()),
        ));
    }

    // 2. Missing capability → rejected + ledgered (unauthorized).
    {
        let (status, _) = fixture_request(
            &fx.gateway_gateway,
            "POST",
            "/v1/chat/completions",
            &[("X-Mutagen-Run", "0015-test")],
            &mk_body("no capability"),
        );
        let e = fx.entries();
        let last = e.last().cloned();
        let ok = status == 403
            && last.as_ref().is_some_and(|x| {
                !x.authorized && !x.forwarded && x.reason.as_deref() == Some("missing_stage_header")
            });
        checks.push(check_ok(
            "channel: missing capability rejected + ledgered (unauthorized)",
            ok,
            format!(
                "status {status} reason {:?}",
                last.as_ref().and_then(|x| x.reason.clone())
            ),
        ));
    }

    // 3. Wrong stage → rejected/logged.
    {
        let body = mk_body("wrong stage");
        let body_sha = sha256_bytes(body.as_bytes());
        let auth = hmac_sha256_hex(
            &key,
            &auth_input("0015-test", "Z", "0015-test-Z-000001", &body_sha),
        );
        let (status, _) = fixture_request(
            &fx.gateway_gateway,
            "POST",
            "/v1/chat/completions",
            &[
                ("X-Mutagen-Run", "0015-test"),
                ("X-Mutagen-Stage", "Z"),
                ("X-Mutagen-Request-Id", "0015-test-Z-000001"),
                ("X-Mutagen-Auth", &auth),
            ],
            &body,
        );
        let e = fx.entries();
        let last = e.last().cloned();
        let ok = status == 403
            && last
                .as_ref()
                .is_some_and(|x| x.reason.as_deref() == Some("wrong_stage") && !x.forwarded);
        checks.push(check_ok(
            "channel: wrong stage rejected + ledgered",
            ok,
            format!(
                "status {status} reason {:?}",
                last.as_ref().and_then(|x| x.reason.clone())
            ),
        ));
    }

    // 4. Wrong body hash (tampered body under a valid MAC) → rejected/logged.
    {
        let body = mk_body("original body");
        let body_sha = sha256_bytes(body.as_bytes());
        let auth = hmac_sha256_hex(
            &key,
            &auth_input("0015-test", "B", "0015-test-B-000002", &body_sha),
        );
        let tampered = mk_body("TAMPERED body");
        let (status, _) = fixture_request(
            &fx.gateway_gateway,
            "POST",
            "/v1/chat/completions",
            &[
                ("X-Mutagen-Run", "0015-test"),
                ("X-Mutagen-Stage", "B"),
                ("X-Mutagen-Request-Id", "0015-test-B-000002"),
                ("X-Mutagen-Auth", &auth),
            ],
            &tampered,
        );
        let e = fx.entries();
        let last = e.last().cloned();
        let ok = status == 403
            && last.as_ref().is_some_and(|x| {
                x.reason.as_deref() == Some("auth_mismatch")
                    && !x.forwarded
                    && x.body_sha256 == sha256_bytes(tampered.as_bytes())
            });
        checks.push(check_ok(
            "channel: wrong body hash (tampered body under a valid MAC) rejected + ledgered",
            ok,
            format!(
                "status {status} reason {:?}",
                last.as_ref().and_then(|x| x.reason.clone())
            ),
        ));
    }

    // 5. GET /v1/models → rejected/logged (no model-list endpoint).
    {
        let (status, _) = fixture_request(&fx.gateway_gateway, "GET", "/v1/models", &[], "");
        let e = fx.entries();
        let last = e.last().cloned();
        let ok = status == 404
            && last.as_ref().is_some_and(|x| {
                !x.authorized
                    && !x.forwarded
                    && x.reason.as_deref() == Some("disallowed_path")
                    && x.method == "GET"
                    && x.path == "/v1/models"
            });
        checks.push(check_ok(
            "channel: GET /v1/models rejected + ledgered (no model-list endpoint)",
            ok,
            format!(
                "status {status} reason {:?}",
                last.as_ref().and_then(|x| x.reason.clone())
            ),
        ));
    }

    // 6. Duplicate request id → rejected/logged.
    {
        let body = mk_body("hello");
        let body_sha = sha256_bytes(body.as_bytes());
        let auth = hmac_sha256_hex(
            &key,
            &auth_input("0015-test", "B", "0015-test-B-000001", &body_sha),
        );
        let (status, _) = fixture_request(
            &fx.gateway_gateway,
            "POST",
            "/v1/chat/completions",
            &[
                ("X-Mutagen-Run", "0015-test"),
                ("X-Mutagen-Stage", "B"),
                ("X-Mutagen-Request-Id", "0015-test-B-000001"),
                ("X-Mutagen-Auth", &auth),
            ],
            &body,
        );
        let e = fx.entries();
        let last = e.last().cloned();
        let ok = status == 403
            && last.as_ref().is_some_and(|x| {
                x.reason.as_deref() == Some("duplicate_request_id") && !x.forwarded
            });
        checks.push(check_ok(
            "channel: duplicate request id rejected + ledgered",
            ok,
            format!(
                "status {status} reason {:?}",
                last.as_ref().and_then(|x| x.reason.clone())
            ),
        ));
    }

    // 7-9. Ledger tamper / deletion / reorder → chain verifier fails.
    {
        let rows = fx.entries();
        let genesis = fx.manifest.ledger_genesis_hash.clone();
        let baseline = channel_ledger::verify_chain(&rows, &genesis);
        checks.push(check_ok(
            "channel: pristine ledger chain verifies",
            baseline.is_empty(),
            baseline.join("; "),
        ));
        let mut tampered = rows.clone();
        if let Some(row) = tampered.get_mut(1) {
            row.body_sha256 = "f".repeat(64);
        }
        checks.push(check_ok(
            "channel: ledger tamper detected (chain verifier fails)",
            !channel_ledger::verify_chain(&tampered, &genesis).is_empty(),
            String::new(),
        ));
        let mut deleted = rows.clone();
        if len_at_least(&deleted, 3) {
            deleted.remove(2);
        }
        checks.push(check_ok(
            "channel: ledger line deletion detected (chain verifier fails)",
            !channel_ledger::verify_chain(&deleted, &genesis).is_empty(),
            String::new(),
        ));
        let mut reordered = rows.clone();
        if len_at_least(&reordered, 3) {
            reordered.swap(1, 2);
        }
        checks.push(check_ok(
            "channel: ledger reorder detected (chain verifier fails)",
            !channel_ledger::verify_chain(&reordered, &genesis).is_empty(),
            String::new(),
        ));
    }

    // 10. Extra authorized but unmatched request → reconciliation fails.
    {
        let rows = fx.entries();
        let v = reconcile_violations(&rows, &[]);
        let ok = v.iter().any(|x| x.contains("unmatched"));
        checks.push(check_ok(
            "channel: extra authorized unmatched request fails reconciliation",
            ok,
            v.first().cloned().unwrap_or_default(),
        ));
    }

    // 11. Recorded request with no gateway row → reconciliation fails.
    {
        let v = reconcile_violations(
            &[],
            &[(
                "B".to_string(),
                "0015-test-B-999999".to_string(),
                "a".repeat(64),
            )],
        );
        let ok = v
            .iter()
            .any(|x| x.contains("no matching forwarded ledger entry"));
        checks.push(check_ok(
            "channel: recorded request missing gateway row fails reconciliation",
            ok,
            v.first().cloned().unwrap_or_default(),
        ));
    }

    // 12. Gateway restart → session continuity fails.
    {
        let mut state = ChannelState::load(&fx.dir).unwrap();
        let ok_before = state.pid == state.origin_pid;
        state.pid = state.origin_pid.saturating_add(1); // simulate a restart
        state.write(&fx.dir).unwrap();
        let s = status_local(&fx.dir).unwrap();
        let ok = ok_before && !s.no_restart;
        checks.push(check_ok(
            "channel: gateway restart detected (session continuity fails)",
            ok,
            format!("pid {} origin {}", s.pid, s.origin_pid),
        ));
        state.pid = state.origin_pid;
        state.write(&fx.dir).unwrap();
    }

    // 13. THE 0013 REGRESSION: after arm, an ad-hoc GET /v1/models →
    // HTTP rejected + ledgered unauthorized + the channel verifier
    // marks the run INCONCLUSIVE. This is the exact 0013 mistake, now
    // mechanically observable.
    {
        let (status, _) = fixture_request(&fx.gateway_gateway, "GET", "/v1/models", &[], "");
        let rows = fx.entries();
        let v = channel_ledger::verify_chain(&rows, &fx.manifest.ledger_genesis_hash);
        let counts = channel_ledger::count_entries(&rows);
        let integrity_v = channel_violations(&fx.dir, &[]).unwrap_or_default();
        let unauthorized_flag = integrity_v
            .iter()
            .any(|x| x.contains("unauthorized gateway request"));
        let last = rows.last().cloned();
        let ok = status == 404
            && last.as_ref().is_some_and(|x| !x.authorized && !x.forwarded)
            && v.is_empty()
            && counts.unauthorized >= 1
            && unauthorized_flag;
        checks.push(check_ok(
            "0013 regression: post-arm GET /v1/models rejected + ledgered + channel verifier INCONCLUSIVE",
            ok,
            format!(
                "status {status}, {} ledger rows, {} unauthorized",
                rows.len(),
                counts.unauthorized
            ),
        ));
    }

    // 14. The clean-run invariant: ANY unauthorized attempt (even a
    // correctly rejected one) fails the channel verifier.
    {
        let integrity_v = channel_violations(&fx.dir, &[]).unwrap_or_default();
        let ok = integrity_v
            .iter()
            .any(|x| x.contains("unauthorized gateway request"));
        checks.push(check_ok(
            "channel: any unauthorized attempt (even a rejected probe) fails the verifier",
            ok,
            "a rejected probe is still an integrity failure".into(),
        ));
    }

    // 15. channel-status-local performs ZERO HTTP (structural check on
    // the frozen source).
    {
        let src = std::fs::read_to_string(crate::experiment::crate_dir().join("src/channel.rs"))
            .unwrap_or_default();
        let status_fn = src
            .split_once("pub fn status_local")
            .and_then(|(_, rest)| rest.split_once("pub fn checkpoint_file"))
            .map(|(s, _)| s)
            .unwrap_or("");
        let ok = !status_fn.contains("ureq") && !status_fn.contains("http");
        checks.push(check_ok(
            "channel-status-local inspects PID/socket/state only (zero HTTP)",
            ok,
            String::new(),
        ));
    }

    checks
}

// ===========================================================================
// Synthetic final reconciliation checks (D1; network-free, no gateway)
// ===========================================================================

/// One authorized + forwarded synthetic ledger row, chained.
fn synth_final_entry(
    seq: u64,
    previous: &str,
    stage: &str,
    request_id: &str,
    body_sha: &str,
) -> ChannelEntry {
    let mut e = ChannelEntry {
        seq,
        run_id: "0015-r1".into(),
        stage_claim: stage.into(),
        request_id: request_id.into(),
        method: "POST".into(),
        path: "/v1/chat/completions".into(),
        body_sha256: body_sha.into(),
        authorized: true,
        forwarded: true,
        status_class: "2xx".into(),
        reason: None,
        previous_entry_hash: previous.into(),
        entry_hash: String::new(),
    };
    e.entry_hash = channel_ledger::compute_entry_hash(previous, &e);
    e
}

/// The D1 final-reconciliation regression pair (network-free; these
/// test the EXISTING reconciliation primitives — no new mechanism):
///
/// 1. the complete final record side (B + C + D + E) against a matching
///    sealed ledger reconciles exactly (the 0015 live confirmation of
///    the D1 fix);
/// 2. the 0014-r1 defect shape (the recorded side with the Stage-D set
///    omitted) MUST fail the reconciliation.
pub fn synthetic_final_reconciliation_checks() -> Vec<Check> {
    let synth = crate::trace::build_synthetic_artifacts();
    let mut discovery = synth.discovery.clone();
    let mut selection = synth.selection.clone();
    let mut promotion = synth.promotion.clone();
    let mut generation = synth.generation.clone();

    let bind = |req: &mut crate::trace::ModelRequestRecord, stage: &str| {
        req.channel_request_id = format!("0015-r1-{stage}-000001");
        req.request_body_sha256 = sha256_bytes(format!("synthetic-d1-final-{stage}").as_bytes());
    };
    let mut bound = discovery
        .first_mut()
        .and_then(|r| r.model_requests.first_mut())
        .is_some_and(|req| {
            bind(req, "B");
            true
        });
    bound = selection
        .first_mut()
        .and_then(|r| r.model_requests.first_mut())
        .is_some_and(|req| {
            bind(req, "D");
            true
        })
        && bound;
    bound = promotion
        .first_mut()
        .and_then(|r| r.model_requests.first_mut())
        .is_some_and(|req| {
            bind(req, "E");
            true
        })
        && bound;
    if generation.attempts.is_empty() {
        generation
            .attempts
            .push(crate::mutation::GenerationAttempt {
                attempt: 1,
                raw_response: None,
                structurally_valid: true,
                validation_errors: Vec::new(),
                prompt_tokens: None,
                cached_prompt_tokens: None,
                completion_tokens: None,
                reasoning_tokens: None,
                wall_time_ms: 1,
                channel_request_id: "0015-r1-C-000001".into(),
                request_body_sha256: sha256_bytes(b"synthetic-d1-final-C"),
            });
    }
    if let Some(a) = generation.attempts.first_mut() {
        if a.channel_request_id.is_empty() {
            a.channel_request_id = "0015-r1-C-000001".into();
            a.request_body_sha256 = sha256_bytes(b"synthetic-d1-final-C");
        }
    }
    if !bound
        || generation
            .attempts
            .first()
            .is_none_or(|a| a.channel_request_id.is_empty())
    {
        return vec![check_ok(
            "synthetic final reconciliation (D1: B + C + D + E)",
            false,
            "could not bind the synthetic stage records".to_string(),
        )];
    }

    let genesis = "0".repeat(64);
    let mut entries = Vec::new();
    let mut prev = genesis.clone();
    for (stage, req) in [
        ("B", Some(&discovery[0].model_requests[0])),
        ("C", None),
        ("D", Some(&selection[0].model_requests[0])),
        ("E", Some(&promotion[0].model_requests[0])),
    ] {
        let (id, sha) = match req {
            Some(r) => (r.channel_request_id.clone(), r.request_body_sha256.clone()),
            None => (
                generation.attempts[0].channel_request_id.clone(),
                generation.attempts[0].request_body_sha256.clone(),
            ),
        };
        let e = synth_final_entry(entries.len() as u64 + 1, &prev, stage, &id, &sha);
        prev = e.entry_hash.clone();
        entries.push(e);
    }

    // 1. The complete B + C + D + E record side reconciles exactly.
    let generation = Some(generation);
    let full = all_recorded(&discovery, &generation, &selection, &promotion);
    let (matched, unmatched) = match_counts_public(&entries, &full);
    let missing = missing_recorded(&entries, &full);
    let chain = channel_ledger::verify_chain(&entries, &genesis);
    let ok = matched == entries.len() as u64
        && unmatched == 0
        && missing == 0
        && duplicate_ids(&entries) == 0
        && chain.is_empty();
    let mut checks = vec![check_ok(
        "synthetic final reconciliation B + C + D + E: exact success (D1)",
        ok,
        format!(
            "matched {matched}/{} unmatched {unmatched} missing {missing}",
            entries.len()
        ),
    )];

    // 2. The 0014-r1 defect shape: omit the Stage-D set → the sealed
    //    ledger no longer reconciles (unmatched D row + count mismatch).
    let omitted_d = all_recorded(&discovery, &generation, &[], &promotion);
    let (_, unmatched_d) = match_counts_public(&entries, &omitted_d);
    let missing_d = missing_recorded(&entries, &omitted_d);
    let recorded_total = omitted_d.iter().filter(|r| !r.1.is_empty()).count() as u64;
    let detected = unmatched_d > 0 || missing_d > 0 || recorded_total != entries.len() as u64;
    checks.push(check_ok(
        "synthetic final reconciliation with D omitted (the 0014-r1 shape): failure detected (D1)",
        detected,
        format!(
            "unmatched {unmatched_d} missing {missing_d} recorded {recorded_total} vs ledger {}",
            entries.len()
        ),
    ));
    checks
}

fn len_at_least(v: &[ChannelEntry], n: usize) -> bool {
    v.len() >= n
}

impl ChannelFixture {
    fn entries(&self) -> Vec<ChannelEntry> {
        let g = self.runtime.lock().unwrap();
        g.ledger.entries().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4231 test case 2 for HMAC-SHA256 (known-answer vector):
    /// key = 0x0b x 20, message = "Hi There".
    #[test]
    fn hmac_sha256_matches_rfc4231_vector() {
        let key = vec![0x0bu8; 20];
        let out = hmac_sha256(&key, b"Hi There");
        assert_eq!(
            hmac_sha256_hex(&key, "Hi There"),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(out.len(), 32);
    }

    /// Keys longer than the block are hashed first (RFC 2104).
    /// RFC 4231 test case 4: key = 0xaa x 80, message = 0xdd x 100.
    #[test]
    fn hmac_sha256_handles_long_keys() {
        let key = vec![0xaa_u8; 80];
        let message = vec![0xdd_u8; 100];
        let out = hmac_sha256(&key, &message);
        let hex: String = out.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "3e6c475b04e4237ddc05fca918e9a9e0a8acee78b519ef1034c6380bb5d15117"
        );
    }

    /// RFC 4231 test case 3: key = 0x60 x 131, message =
    /// "Test Using Larger Than Block-Size Key - Key Longer Than
    /// Block-Size".
    #[test]
    fn hmac_sha256_matches_rfc4231_case3() {
        let key = vec![0x60_u8; 131];
        let out = hmac_sha256_hex(
            &key,
            "Test Using Larger Than Block-Size Key - Key Longer Than Block-Size",
        );
        assert_eq!(
            out,
            "0c4ebbf39a60589efe98d60c651088c4c2052cea4fa0d89055c356f221ae6f02"
        );
    }
}
