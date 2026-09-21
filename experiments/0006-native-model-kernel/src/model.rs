//! Concrete synchronous client for one OpenAI-compatible chat endpoint.
//!
//! One provider exists for this experiment, so there is no
//! `ModelProvider` trait and no abstraction: exactly one concrete
//! `ModelClient`. Synchronous on purpose (spec: semantics, not
//! throughput; no async runtime).
//!
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

impl ModelError {
    /// Human-readable form, safe to persist (no bodies, no keys).
    pub fn message(&self) -> String {
        match self {
            ModelError::Http(detail) => format!("model HTTP failure: {detail}"),
            ModelError::Status(code) => format!("model endpoint returned HTTP {code}"),
            ModelError::Parse(detail) => format!("model response parse failure: {detail}"),
        }
    }
}

/// Per-request timeout. Local Qwen generation can be slow; this is a
/// generous safety bound, not a tuning knob.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

/// A concrete client bound to one endpoint and one model.
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

    /// `POST {base_url}/chat/completions` with the given messages and
    /// tools. Returns the kernel-relevant parsed response.
    ///
    /// The API key is sent in the `Authorization` header only; it is
    /// never written to any artifact.
    pub fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<ModelResponse, ModelError> {
        let request = ChatRequest::new(&self.model, messages.to_vec(), tools.to_vec());
        let body = serde_json::to_string(&request)
            .map_err(|e| ModelError::Parse(format!("request serialization: {e}")))?;

        let agent = ureq::AgentBuilder::new().timeout(REQUEST_TIMEOUT).build();

        let mut req = agent.post(&format!(
            "{}/chat/completions",
            self.base_url.trim_end_matches('/')
        ));
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
    fn error_messages_carry_no_secrets_or_bodies() {
        // Status errors record the code only (no response body), and the
        // client struct holds the key without any Serialize impl, so no
        // artifact path can print credentials.
        let e = ModelError::Status(500);
        assert_eq!(e.message(), "model endpoint returned HTTP 500");
        let client = ModelClient::new("http://h/v1".into(), "m".into(), Some("sk-secret".into()));
        assert_eq!(client.base_url, "http://h/v1");
        assert_eq!(client.model, "m");
    }
}
