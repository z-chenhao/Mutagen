//! OpenAI-compatible Chat Completions protocol surface for Experiment 0006.
//!
//! Scope (deliberately minimal):
//! - message roles: system, user, assistant, tool
//! - requests: `model`, `messages`, `tools`, `tool_choice = "auto"`,
//!   `temperature = 0.2`
//! - responses: a single choice whose assistant message carries either
//!   normal text (final answer) or tool calls
//!
//! NOT supported: streaming, vision, audio, logprobs, structured-output
//! frameworks, provider-specific reasoning protocols, multiple choices.
//! Unknown response fields (e.g. `reasoning`, `reasoning_content`,
//! `thinking`, `analysis`) are ignored by construction: serde does not
//! deserialize fields that are not declared, and nothing in this crate
//! stores or re-emits them.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

#[cfg(test)]
use serde_json::json;

/// Fixed experiment temperature. All runs use this value.
pub const TEMPERATURE: f64 = 0.2;

/// Message roles actually required by the experiment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A single function call requested by the model.
///
/// `arguments` is the raw JSON string exactly as the provider sent it
/// (OpenAI convention). Parsing it into `Value` happens in the kernel,
/// because argument *validity* is a tool-level concern.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// A tool call block inside an assistant message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_function")]
    kind: ToolCallKind,
    pub function: FunctionCall,
}

fn default_function() -> ToolCallKind {
    ToolCallKind::Function
}

/// Only `function` tool calls exist in this protocol surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ToolCallKind {
    #[default]
    Function,
}

/// A chat message. One struct covers the five shapes the experiment
/// uses; unused fields are omitted from the wire form.
///
/// - system:  `content`
/// - user:    `content`
/// - assistant (final): `content`, no `tool_calls`
/// - assistant (tool call): `content` (may be null), `tool_calls` (one)
/// - tool result: `tool_call_id`, `name`, `content`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self::new(Role::System, Some(content.into()))
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new(Role::User, Some(content.into()))
    }

    /// An assistant message carrying a final text answer.
    pub fn assistant_text(content: impl Into<String>) -> Self {
        Self::new(Role::Assistant, Some(content.into()))
    }

    /// An assistant message carrying a single tool call.
    /// `content` is kept (often null on the wire) so a replayed
    /// conversation matches what the provider sent.
    pub fn assistant_tool_call(
        id: impl Into<String>,
        function: FunctionCall,
        content: Option<String>,
    ) -> Self {
        Self {
            role: Role::Assistant,
            content,
            tool_calls: vec![ToolCall {
                id: id.into(),
                kind: ToolCallKind::Function,
                function,
            }],
            tool_call_id: None,
            name: None,
        }
    }

    /// A tool result message answering a previous tool call.
    pub fn tool_result(
        tool_call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            name: Some(name.into()),
        }
    }

    fn new(role: Role, content: Option<String>) -> Self {
        Self {
            role,
            content,
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }
}

/// A tool specification sent to the model in the `tools` array.
/// Experiment-local: always the OpenAI `function` shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolFunctionSpec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolFunctionSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Chat completion request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    #[serde(rename = "tool_choice")]
    pub tool_choice: String,
    pub temperature: f64,
}

impl ChatRequest {
    pub fn new(model: &str, messages: Vec<Message>, tools: Vec<ToolSpec>) -> Self {
        Self {
            model: model.to_string(),
            messages,
            tools,
            tool_choice: "auto".to_string(),
            temperature: TEMPERATURE,
        }
    }
}

/// Token usage reported by the provider, accumulated per episode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

/// The parsed, kernel-relevant part of a chat completion response.
///
/// Raw assistant message content and tool calls as sent by the provider.
/// Unknown fields on the wire are ignored (see module docs).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ModelResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
    pub usage: TokenUsage,
}

/// Classification of a parsed assistant response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseKind {
    /// Text content and no tool calls.
    FinalAnswer,
    /// Exactly one tool call.
    SingleToolCall,
    /// More than one tool call in one assistant message.
    ParallelToolCalls,
    /// Neither text nor tool calls; unusable.
    Empty,
}

impl ModelResponse {
    pub fn kind(&self) -> ResponseKind {
        match (
            self.tool_calls.is_empty(),
            self.content
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .is_empty(),
        ) {
            (true, false) => ResponseKind::FinalAnswer,
            (true, true) => ResponseKind::Empty,
            (false, _) if self.tool_calls.len() == 1 => ResponseKind::SingleToolCall,
            (false, _) => ResponseKind::ParallelToolCalls,
        }
    }
}

/// Parse a full provider JSON response into the kernel-relevant part.
///
/// Only the first choice is consumed (spec: multiple choices are out of
/// scope). Provider reasoning fields are not declared here and therefore
/// cannot leak into kernel state or artifacts.
pub fn parse_response(raw: &Value) -> Result<ModelResponse, String> {
    let choices = raw
        .get("choices")
        .and_then(Value::as_array)
        .ok_or_else(|| "response has no choices array".to_string())?;
    if choices.is_empty() {
        return Err("response has empty choices array".to_string());
    }
    let choice = &choices[0];
    let message = choice
        .get("message")
        .ok_or_else(|| "choice has no message".to_string())?;

    let content: Option<String> = message
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_string);

    let tool_calls: Vec<ToolCall> = match message.get("tool_calls") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                serde_json::from_value::<ToolCall>(item.clone())
                    .map_err(|e| format!("malformed tool call block: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err("tool_calls is not an array".to_string()),
    };

    let finish_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .map(str::to_string);

    let usage = raw
        .get("usage")
        .map(|u| TokenUsage {
            prompt_tokens: u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
            completion_tokens: u
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            total_tokens: u.get("total_tokens").and_then(Value::as_u64).unwrap_or(0),
        })
        .unwrap_or_default();

    Ok(ModelResponse {
        content,
        tool_calls,
        finish_reason,
        usage,
    })
}

/// Deterministically canonicalize a JSON value: object keys sorted
/// recursively, arrays preserved in order, compact separators.
///
/// Used for canonical call identity (spec: `tool_name` + normalized JSON
/// arguments, object key order normalized deterministically).
pub fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for k in keys {
                out.insert(k.clone(), canonical_json(&map[k]));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical_json).collect()),
        other => other.clone(),
    }
}

/// Canonical call identity: `tool_name` immediately followed by the
/// canonical JSON of its arguments, e.g. `state_write{"key":"x","value":"A"}`.
pub fn canonical_call_identity(tool_name: &str, arguments: &Value) -> String {
    format!("{tool_name}{}", canonical_json(arguments))
}

/// Stable SHA-256 hex digest of a JSON value in canonical form.
pub fn sha256_hex(value: &Value) -> String {
    use sha2::Digest;
    use sha2::Sha256;
    let mut hasher = Sha256::new();
    hasher.update(canonical_json(value).to_string().as_bytes());
    hex(hasher.finalize())
}

/// Stable SHA-256 hex digest of raw bytes.
pub fn sha256_bytes(data: &[u8]) -> String {
    use sha2::Digest;
    use sha2::Sha256;
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex(hasher.finalize())
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    // --- message parsing -------------------------------------------------

    #[test]
    fn serializes_system_and_user_messages() {
        let m = Message::system("be an agent");
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v, json!({"role": "system", "content": "be an agent"}));

        let u = Message::user("do the task");
        let v = serde_json::to_value(&u).unwrap();
        assert_eq!(v, json!({"role": "user", "content": "do the task"}));
    }

    #[test]
    fn serializes_assistant_tool_call_and_result() {
        let a = Message::assistant_tool_call(
            "call_1",
            FunctionCall {
                name: "state_write".into(),
                arguments: "{\"key\":\"x\",\"value\":\"A\"}".into(),
            },
            None,
        );
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["role"], "assistant");
        assert!(v["content"].is_null());
        assert_eq!(v["tool_calls"][0]["id"], "call_1");
        assert_eq!(v["tool_calls"][0]["type"], "function");
        assert_eq!(v["tool_calls"][0]["function"]["name"], "state_write");

        let t = Message::tool_result("call_1", "state_write", "{\"ok\":true}");
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["role"], "tool");
        assert_eq!(v["tool_call_id"], "call_1");
        assert_eq!(v["name"], "state_write");
    }

    // --- single tool-call parsing ----------------------------------------

    #[test]
    fn parses_single_tool_call_response() {
        let raw = fixture(
            r#"{
              "id": "cmpl-1", "object": "chat.completion", "model": "m",
              "choices": [{
                "index": 0,
                "message": {
                  "role": "assistant",
                  "content": null,
                  "tool_calls": [{
                    "id": "call_9", "type": "function",
                    "function": {"name": "state_read", "arguments": "{\"key\":\"y\"}"}
                  }]
                },
                "finish_reason": "tool_calls"
              }],
              "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
            }"#,
        );
        let r = parse_response(&raw).unwrap();
        assert_eq!(r.kind(), ResponseKind::SingleToolCall);
        assert_eq!(r.tool_calls[0].id, "call_9");
        assert_eq!(r.tool_calls[0].function.name, "state_read");
        assert_eq!(r.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!(r.usage.total_tokens, 15);
    }

    // --- parallel tool-call rejection -------------------------------------

    #[test]
    fn parses_parallel_tool_calls_as_distinct_kind() {
        let raw = fixture(
            r#"{
              "choices": [{
                "message": {
                  "role": "assistant",
                  "content": "",
                  "tool_calls": [
                    {"id": "c1", "type": "function",
                     "function": {"name": "state_read", "arguments": "{\"key\":\"x\"}"}},
                    {"id": "c2", "type": "function",
                     "function": {"name": "state_read", "arguments": "{\"key\":\"y\"}"}}
                  ]
                },
                "finish_reason": "tool_calls"
              }]
            }"#,
        );
        let r = parse_response(&raw).unwrap();
        assert_eq!(r.kind(), ResponseKind::ParallelToolCalls);
        assert_eq!(r.tool_calls.len(), 2);
    }

    // --- final answer parsing ---------------------------------------------

    #[test]
    fn parses_final_answer_without_tool_calls() {
        let raw = fixture(
            r#"{
              "choices": [{
                "message": {"role": "assistant", "content": "x is A."},
                "finish_reason": "stop"
              }],
              "usage": {"prompt_tokens": 3, "completion_tokens": 4, "total_tokens": 7}
            }"#,
        );
        let r = parse_response(&raw).unwrap();
        assert_eq!(r.kind(), ResponseKind::FinalAnswer);
        assert_eq!(r.content.as_deref(), Some("x is A."));
        assert!(r.tool_calls.is_empty());
        assert_eq!(r.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn response_without_text_or_calls_is_empty() {
        let raw = fixture(
            r#"{"choices": [{"message": {"role": "assistant", "content": null},
              "finish_reason": "stop"}]}"#,
        );
        assert_eq!(parse_response(&raw).unwrap().kind(), ResponseKind::Empty);
    }

    #[test]
    fn response_without_choices_fails_to_parse() {
        let raw = fixture(r#"{"object": "chat.completion"}"#);
        assert!(parse_response(&raw).is_err());
    }

    // --- reasoning fields are ignored -------------------------------------

    #[test]
    fn reasoning_style_fields_are_ignored() {
        let raw = fixture(
            r#"{
              "choices": [{
                "message": {
                  "role": "assistant",
                  "content": "x is A.",
                  "reasoning": "SECRET-CHAIN-OF-THOUGHT",
                  "reasoning_content": "SECRET-CHAIN-OF-THOUGHT",
                  "thinking": "SECRET-THINKING",
                  "analysis": "SECRET-ANALYSIS"
                },
                "finish_reason": "stop"
              }]
            }"#,
        );
        let r = parse_response(&raw).unwrap();
        // Only declared fields survive into kernel state.
        assert_eq!(r.content.as_deref(), Some("x is A."));
        let serialized = serde_json::to_string(&json!({
            "content": r.content,
            "tool_calls": r.tool_calls,
        }))
        .unwrap();
        for forbidden in ["reasoning", "thinking", "analysis", "SECRET"] {
            assert!(
                !serialized.contains(forbidden),
                "reasoning-like field leaked: {forbidden}"
            );
        }
    }

    // --- canonicalization ---------------------------------------------------

    #[test]
    fn canonical_identity_is_independent_of_key_order() {
        let a: Value = fixture(r#"{"key":"x","value":"A"}"#);
        let b: Value = fixture(r#"{"value":"A","key":"x"}"#);
        assert_eq!(
            canonical_call_identity("state_write", &a),
            canonical_call_identity("state_write", &b)
        );
        assert_eq!(
            canonical_call_identity("state_write", &a),
            r#"state_write{"key":"x","value":"A"}"#
        );
    }

    #[test]
    fn canonical_json_sorts_nested_keys_and_keeps_arrays() {
        let v: Value = fixture(r#"{"z": 1, "a": {"y": [1, 2], "b": true}}"#);
        let c = canonical_json(&v);
        assert_eq!(c.to_string(), r#"{"a":{"b":true,"y":[1,2]},"z":1}"#);
    }

    #[test]
    fn sha256_of_identical_json_is_stable() {
        let a: Value = fixture(r#"{"key":"x","value":"A"}"#);
        let b: Value = fixture(r#"{"value":"A","key":"x"}"#);
        assert_eq!(sha256_hex(&a), sha256_hex(&b));
        assert_eq!(sha256_hex(&a).len(), 64);
    }

    #[test]
    fn tool_spec_round_trips() {
        let spec = ToolSpec {
            kind: "function".into(),
            function: ToolFunctionSpec {
                name: "state_read".into(),
                description: "Read the current value of a state key.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {"key": {"type": "string", "enum": ["x", "y"]}},
                    "required": ["key"],
                    "additionalProperties": false
                }),
            },
        };
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v["type"], "function");
        assert_eq!(v["function"]["name"], "state_read");
        assert_eq!(v["function"]["parameters"]["required"], json!(["key"]));
    }
}
