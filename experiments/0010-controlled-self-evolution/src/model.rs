//! Concrete synchronous client for one OpenAI-compatible chat endpoint.
//!
//! Carried over from Experiment 0006, with the Experiment 0010
//! mutation-generation request: `complete_mutator` sends a tools-less
//! request at the registered mutation temperature (0.7), while agent
//! execution keeps the registered agent request (temperature 0.2,
//! `tool_choice = auto`). One provider exists for this experiment, so
//! there is no `ModelProvider` trait and no abstraction — exactly one
//! concrete `ModelClient`. Synchronous on purpose: no async runtime.
//! The only network I/O performed by the whole experiment process is
//! this HTTP request.

use std::time::Duration;

use serde_json::Value;

use crate::protocol::{ChatRequest, Message, ModelResponse, ToolSpec, parse_response};

/// Failure to complete a model request, classified for the kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    /// Network / transport failure (connection refused, timeout, …).
    Http(String),
    /// Non-2xx status from the endpoint.
    Status(usize),
    /// 2xx response whose JSON could not be parsed into a usable
    /// assistant message.
    Parse(String),
}

/// Per-request timeout. Local Qwen generation can be slow; this is a
/// generous safety bound, not a tuning knob.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

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

    /// Agent-execution request (spec §8): the given tools, the
    /// registered agent temperature, `tool_choice = auto`. Returns the
    /// kernel-relevant parsed response.
    pub fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<ModelResponse, ModelError> {
        self.post(&ChatRequest::agent(
            &self.model,
            messages.to_vec(),
            tools.to_vec(),
        ))
    }

    /// Mutation-generation request (spec §8): the same underlying model,
    /// no tools, the registered mutation temperature. Used exactly by
    /// the mutation generator; never by agent execution.
    pub fn complete_mutator(&self, messages: &[Message]) -> Result<ModelResponse, ModelError> {
        self.post(&ChatRequest::mutator(&self.model, messages.to_vec()))
    }

    /// `POST {base_url}/chat/completions` with a ready request.
    fn post(&self, request: &ChatRequest) -> Result<ModelResponse, ModelError> {
        let body = serde_json::to_string(request)
            .map_err(|e| ModelError::Parse(format!("request serialization: {e}")))?;

        let agent = ureq::AgentBuilder::new().timeout(REQUEST_TIMEOUT).build();

        let mut req = agent
            .post(&format!(
                "{}/chat/completions",
                self.base_url.trim_end_matches('/')
            ))
            .set("Content-Type", "application/json");
        if let Some(key) = &self.api_key {
            req = req.set("Authorization", &format!("Bearer {key}"));
        }

        let response = match req.send_string(&body) {
            Ok(r) => r,
            Err(ureq::Error::Status(code, _)) => return Err(ModelError::Status(code as usize)),
            Err(ureq::Error::Transport(t)) => {
                return Err(ModelError::Http(t.to_string()));
            }
        };

        let raw: Value = response
            .into_json()
            .map_err(|e| ModelError::Parse(format!("response is not valid JSON: {e}")))?;

        parse_response(&raw).map_err(ModelError::Parse)
    }
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
        assert_eq!(
            redacted_endpoint("http://127.0.0.1:8000/v1"),
            "http://127.0.0.1:8000/v1"
        );
    }

    #[test]
    fn error_variants_carry_no_response_bodies() {
        // The struct holds only codes / transport messages; no
        // `Serialize` impl exists, so no artifact path can print bodies
        // or credentials.
        let _ = ModelError::Status(500);
        let _ = ModelError::Http("connection refused".into());
        let _ = ModelError::Parse("no choices".into());
    }
}
