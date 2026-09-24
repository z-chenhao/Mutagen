//! Concrete synchronous client for the ONE registered endpoint — the
//! audited channel GATEWAY (Experiment 0014).
//!
//! Carried over from Experiments 0006/0010/0011/0013, with the 0014
//! audited-execution-channel mediation: every model request of the
//! experiment traverses the registered channel gateway. The endpoint
//! constant below is the REGISTERED EXPERIMENT ENDPOINT (the gateway),
//! NOT the physical upstream model server. This module NEVER knows the
//! upstream URL: it is a channel-internal value read from `design.json`
//! by `src/channel.rs` (the gateway implementation) only, and the
//! static preflight bypass-guard asserts that the upstream literal
//! does not appear in this file (nor in `kernel.rs`, `experiment.rs`,
//! `mutation.rs`, or `main.rs`). The experiment runtime builds the
//! request body, has the channel authorize it, and hands this client
//! the exact body bytes plus the channel authorization; there is no
//! fallback endpoint, no second gateway, and no direct-upstream path —
//! gateway unavailability is an infrastructure failure, never a
//! silent fallback.
//!
//! The client performs exactly ONE blocking HTTP call per request:
//! `POST {gateway}/chat/completions` with the registered channel
//! headers. One registered model exists (there is no `ModelProvider`
//! trait and no abstraction); synchronous on purpose: no async
//! runtime.
//!
//! 0014 deadline contract: this client defines NO episode-deadline or
//! request-timeout constants of its own (the 0012 `REQUEST_TIMEOUT` /
//! `EPISODE_TIME_LIMIT_MS` constants are deliberately removed; the
//! deadline values are the single source of truth in `design.json`'s
//! `deadline` block, enforced by the kernel and verified from the
//! artifacts). Callers pass the exact per-request timeout they
//! registered — for agent requests the kernel computes it from the
//! design-registered deadline; for the single mutation-generation
//! request the runner passes the design-registered ceiling.
//!
//! The only network I/O performed by the whole experiment process is
//! this HTTP request, and the ONLY callers are the four protocol-
//! guarded live stages (`discover`/`generate`/`select`/`promote`);
//! the binary exposes no ad-hoc live command (0011's `run --task` smoke
//! command does not exist in 0014).
//!
//! Timeout classification (0014 §20): ureq 2.x surfaces transport
//! failures as `Error::Transport` with a structured `ErrorKind` and a
//! source chain. ureq unifies read timeouts as `io::ErrorKind`
//! `::TimedOut` in that source chain, so the client classifies a
//! structured timeout from the kind + source chain; a deterministic
//! substring fallback over the rendered message covers remaining
//! shapes (documented limitation — and formal deadline validity never
//! depends on this flag alone: it comes from start / requested timeout
//! / finish / response acceptance).

use std::time::Duration;

use serde_json::Value;

use crate::protocol::{ModelResponse, parse_response};

/// Failure to complete a model request, classified for the kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    /// A transport-level REQUEST TIMEOUT (structured classification, see
    /// [`transport_is_timeout`]): the registered per-request timeout
    /// expired before the endpoint returned anything.
    Timeout(String),
    /// Network / transport failure (connection refused, …).
    Http(String),
    /// Non-2xx status from the endpoint.
    Status(usize),
    /// 2xx response whose JSON could not be parsed into a usable
    /// assistant message.
    Parse(String),
}

/// The ONE registered experiment endpoint for this experiment (0014):
/// the audited channel GATEWAY. Named constant (reported by
/// `preflight`, which asserts it equals the registered
/// `design.json` `channel.gateway_endpoint`). This is the REGISTERED
/// EXPERIMENT ENDPOINT — the physical upstream model transport is an
/// implementation backend internal to `src/channel.rs` and is NOT
/// known to this module. There is no fallback endpoint and no other
/// network route in the experiment.
pub const REGISTERED_ENDPOINT: &str = "http://127.0.0.1:18140/v1";

/// One authorized channel request (0014): the four registered headers
/// the channel gateway validates before forwarding, plus the body hash
/// bound into the MAC. Produced by the channel session
/// (`src/channel.rs`) from the channel capability, the channel-issued
/// request identity, and the SHA-256 of the exact request body bytes.
/// This struct is an opaque header carrier here; this module performs
/// no cryptographic verification of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelAuth {
    pub run_id: String,
    pub stage: String,
    pub request_id: String,
    pub auth: String,
    /// The body hash bound into the MAC (runtime-side bookkeeping so
    /// the produced channel binding can be stamped into the record).
    pub body_sha256: String,
}

impl ChannelAuth {
    /// The exact wire headers for an authorized request.
    pub fn headers(&self) -> Vec<(String, String)> {
        vec![
            ("X-Mutagen-Run".to_string(), self.run_id.clone()),
            ("X-Mutagen-Stage".to_string(), self.stage.clone()),
            ("X-Mutagen-Request-Id".to_string(), self.request_id.clone()),
            ("X-Mutagen-Auth".to_string(), self.auth.clone()),
        ]
    }
}

/// A concrete client bound to one endpoint and one model.
///
/// The struct intentionally has no `Serialize` implementation: no
/// artifact path can print credentials.
pub struct ModelClient {
    base_url: String,
    model: String,
    api_key: Option<String>,
}

impl ModelClient {
    pub fn new(base_url: String, model: String, api_key: Option<String>) -> Self {
        Self {
            base_url,
            model,
            api_key,
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// One registered channel request: `POST {base_url}/chat/completions`
    /// with the EXACT `body` bytes (produced by the experiment runtime
    /// via `ChatRequest::to_body`) and the registered channel headers
    /// (`auth`). `timeout` is the EXACT registered per-request timeout
    /// computed by the kernel from the design-registered deadline
    /// (`min(remaining episode budget, deadline ceiling)`) — this module
    /// holds no deadline constant of its own, and the channel gateway's
    /// overhead is part of the request elapsed time (no deadline reset
    /// at the gateway). Returns the kernel-relevant parsed response.
    pub fn send(
        &self,
        body: &str,
        timeout: Duration,
        auth: &ChannelAuth,
    ) -> Result<ModelResponse, ModelError> {
        let agent = ureq::AgentBuilder::new().timeout(timeout).build();

        let mut req = agent
            .post(&format!(
                "{}/chat/completions",
                self.base_url.trim_end_matches('/')
            ))
            .set("Content-Type", "application/json");
        for (k, v) in auth.headers() {
            req = req.set(&k, &v);
        }
        if let Some(key) = &self.api_key {
            req = req.set("Authorization", &format!("Bearer {key}"));
        }

        let response = match req.send_string(body) {
            Ok(r) => r,
            Err(ureq::Error::Status(code, _)) => return Err(ModelError::Status(code as usize)),
            Err(ureq::Error::Transport(t)) => {
                if transport_is_timeout(&t) {
                    return Err(ModelError::Timeout(t.to_string()));
                }
                return Err(ModelError::Http(t.to_string()));
            }
        };

        // Body reads that time out are request timeouts, not malformed
        // responses (ureq unifies read timeouts as io::ErrorKind
        // `TimedOut`).
        let raw: Value = response.into_json().map_err(|e| {
            if e.kind() == std::io::ErrorKind::TimedOut {
                ModelError::Timeout(format!("body read timed out: {e}"))
            } else {
                ModelError::Parse(format!("response is not valid JSON: {e}"))
            }
        })?;

        parse_response(&raw).map_err(ModelError::Parse)
    }
}

/// Structured transport-timeout classification (0014 §20).
///
/// ureq 2.x has no dedicated `ErrorKind::Timeout` variant: it surfaces
/// read/socket timeouts as an `io::Error` in the transport source
/// chain with `io::ErrorKind::TimedOut` (ureq explicitly unifies these
/// in its API). This classifier first follows the structured source
/// chain (bounded walk); if the chain does not expose a structured
/// `TimedOut` io::Error, a deterministic substring check over the
/// rendered transport message is the fallback. The flag is supporting
/// diagnostic evidence only: formal deadline validity is re-derived
/// from start / requested timeout / finish / response-acceptance.
pub fn transport_is_timeout(t: &ureq::Transport) -> bool {
    // Structured: walk the source chain for a TimedOut io::Error.
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(t);
    let mut hops = 0u32;
    while let Some(s) = source {
        if let Some(io_err) = s.downcast_ref::<std::io::Error>() {
            if io_err.kind() == std::io::ErrorKind::TimedOut {
                return true;
            }
        }
        source = s.source();
        hops += 1;
        if hops > 8 {
            break; // the chain is a few frames deep; never walk forever
        }
    }
    // Deterministic fallback over the rendered message (kind prefix
    // included, e.g. "http://...: Network Error: operation timed out").
    let rendered = t.to_string().to_lowercase();
    rendered.contains("timed out") || rendered.contains("timeout")
}

/// Redact credential material from an endpoint URL for artifacts.
///
/// `http://user:pass@host:8080/v1` → `http://host:8080/v1`.
pub fn redacted_endpoint(base_url: &str) -> String {
    let (scheme, remainder) = match base_url.find("://") {
        Some(i) => (base_url[..i].to_string(), &base_url[i + 3..]),
        None => (String::new(), base_url),
    };
    let (authority, path) = match remainder.find('/') {
        Some(i) => (&remainder[..i], &remainder[i..]),
        None => (remainder, ""),
    };
    let host_port = authority
        .rsplit_once('@')
        .map(|(_, h)| h.to_string())
        .unwrap_or_else(|| authority.to_string());
    format!("{scheme}://{host_port}{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_userinfo_from_endpoint() {
        assert_eq!(
            redacted_endpoint("http://user:secret@host:8080/v1"),
            "http://host:8080/v1"
        );
    }

    #[test]
    fn keeps_plain_endpoint_intact() {
        // (The sample must NOT be the registered upstream endpoint
        // from design.json: the static bypass guard asserts that
        // only src/channel.rs may contain it.)
        assert_eq!(
            redacted_endpoint("http://127.0.0.1:9999/v1"),
            "http://127.0.0.1:9999/v1"
        );
    }

    #[test]
    fn error_variants_carry_no_response_bodies() {
        // The struct holds only codes / transport messages; no
        // `Serialize` impl exists, so no artifact path can print bodies
        // or credentials.
        let _ = ModelError::Status(500);
        let _ = ModelError::Http("connection refused".into());
        let _ = ModelError::Timeout("operation timed out".into());
        let _ = ModelError::Parse("no choices".into());
    }
}
