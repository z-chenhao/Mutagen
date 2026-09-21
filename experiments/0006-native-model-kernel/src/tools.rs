//! Experiment-local tools: exactly two, `state_read` and `state_write`.
//!
//! Capability boundary: the model receives only these two tools. The
//! kernel provides no shell, filesystem, browser, or network tool.
//! External state is an explicit value inside `ExternalState`: no global
//! mutable state, and a fresh `ToolExecutor` is constructed per episode
//! so no state is ever reused between episodes.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;

/// The two state keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StateKey {
    X,
    Y,
}

impl StateKey {
    pub fn as_str(self) -> &'static str {
        match self {
            StateKey::X => "x",
            StateKey::Y => "y",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "x" => Some(StateKey::X),
            "y" => Some(StateKey::Y),
            _ => None,
        }
    }
}

/// The deterministic external state. Every episode starts fresh:
/// `x = "EMPTY"`, `y = "B"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalState {
    x: String,
    y: String,
}

impl ExternalState {
    /// Fresh initial state for a new episode.
    pub fn fresh() -> Self {
        Self {
            x: "EMPTY".to_string(),
            y: "B".to_string(),
        }
    }

    /// The state as a JSON object, for artifact provenance.
    pub fn as_value(&self) -> Value {
        json!({ "x": self.x, "y": self.y })
    }

    pub fn get(&self, key: StateKey) -> &str {
        match key {
            StateKey::X => &self.x,
            StateKey::Y => &self.y,
        }
    }

    pub fn set(&mut self, key: StateKey, value: &str) {
        match key {
            StateKey::X => self.x = value.to_string(),
            StateKey::Y => self.y = value.to_string(),
        }
    }
}

/// A tool execution failure that the kernel can record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// The model requested a tool that does not exist.
    UnknownTool(String),
    /// The model supplied arguments that violate the tool schema
    /// (missing key, unexpected key, wrong type, value outside enum).
    InvalidArguments { tool: String, detail: String },
    /// Internal execution failure. The two state tools cannot currently
    /// produce one; the variant keeps the execution boundary typed.
    #[allow(dead_code)]
    Execution(String),
}

/// Result of executing one tool call: a JSON payload to return to the
/// model as the tool result message.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub payload: Value,
}

/// Executes tool calls against a fresh per-episode external state.
pub struct ToolExecutor {
    state: ExternalState,
}

impl ToolExecutor {
    /// Construct an executor with fresh external state (x=EMPTY, y=B).
    pub fn fresh() -> Self {
        Self {
            state: ExternalState::fresh(),
        }
    }

    /// Current state as a JSON object (`{"x": ..., "y": ...}`).
    pub fn final_state_value(&self) -> Value {
        self.state.as_value()
    }

    /// The model-visible tool specifications. Exactly two tools.
    pub fn specs() -> Vec<crate::protocol::ToolSpec> {
        vec![
            crate::protocol::ToolSpec {
                kind: "function".into(),
                function: crate::protocol::ToolFunctionSpec {
                    name: "state_read".into(),
                    description: "Read the current value of a state key.".into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "key": {"type": "string", "enum": ["x", "y"]}
                        },
                        "required": ["key"],
                        "additionalProperties": false
                    }),
                },
            },
            crate::protocol::ToolSpec {
                kind: "function".into(),
                function: crate::protocol::ToolFunctionSpec {
                    name: "state_write".into(),
                    description: "Set a state key to a string value.".into(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "key": {"type": "string", "enum": ["x", "y"]},
                            "value": {"type": "string"}
                        },
                        "required": ["key", "value"],
                        "additionalProperties": false
                    }),
                },
            },
        ]
    }

    /// Execute a validated-by-schema-or-fail tool call.
    ///
    /// `arguments` is the parsed JSON object of the raw arguments string.
    /// Validation is explicit (no JSON-schema library): required keys,
    /// `additionalProperties: false`, enum membership.
    pub fn execute(&mut self, name: &str, arguments: &Value) -> Result<ToolResult, ToolError> {
        match name {
            "state_read" => self.read(arguments),
            "state_write" => self.write(arguments),
            other => Err(ToolError::UnknownTool(other.to_string())),
        }
    }

    fn read(&self, args: &Value) -> Result<ToolResult, ToolError> {
        if !matches!(args, Value::Object(_)) {
            return Err(ToolError::InvalidArguments {
                tool: "state_read".into(),
                detail: "arguments must be a JSON object".into(),
            });
        }
        let key = read_key(args)?;
        if let Err(detail) = reject_extra_keys(args, &["key"]) {
            return Err(ToolError::InvalidArguments {
                tool: "state_read".into(),
                detail,
            });
        }
        Ok(ToolResult {
            payload: json!({
                "key": key.as_str(),
                "value": self.state.get(key),
            }),
        })
    }

    fn write(&mut self, args: &Value) -> Result<ToolResult, ToolError> {
        if !matches!(args, Value::Object(_)) {
            return Err(ToolError::InvalidArguments {
                tool: "state_write".into(),
                detail: "arguments must be a JSON object".into(),
            });
        }
        let key = read_key(args)?;
        let value = match args.get("value").and_then(Value::as_str) {
            Some(v) => v,
            None => {
                return Err(ToolError::InvalidArguments {
                    tool: "state_write".into(),
                    detail: "missing or non-string \"value\"".into(),
                });
            }
        };
        if let Err(detail) = reject_extra_keys(args, &["key", "value"]) {
            return Err(ToolError::InvalidArguments {
                tool: "state_write".into(),
                detail,
            });
        }
        self.state.set(key, value);
        Ok(ToolResult {
            payload: json!({
                "ok": true,
                "key": key.as_str(),
                "value": value,
            }),
        })
    }
}

fn read_key(args: &Value) -> Result<StateKey, ToolError> {
    let raw = match args.get("key").and_then(Value::as_str) {
        Some(k) => k,
        None => {
            return Err(ToolError::InvalidArguments {
                tool: "<state tool>".into(),
                detail: "missing or non-string \"key\"".into(),
            });
        }
    };
    match StateKey::parse(raw) {
        Some(k) => Ok(k),
        None => Err(ToolError::InvalidArguments {
            tool: "<state tool>".into(),
            detail: format!("\"key\" must be \"x\" or \"y\", got \"{raw}\""),
        }),
    }
}

fn reject_extra_keys(args: &Value, allowed: &[&str]) -> Result<(), String> {
    let Value::Object(map) = args else {
        return Err("arguments must be a JSON object".into());
    };
    for k in map.keys() {
        if !allowed.contains(&k.as_str()) {
            return Err(format!(
                "unexpected key \"{k}\" (additionalProperties: false)"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- tool semantics -----------------------------------------------------

    #[test]
    fn initial_state_is_x_empty_y_b() {
        let mut ex = ToolExecutor::fresh();
        let r = ex.execute("state_read", &json!({"key": "x"})).unwrap();
        assert_eq!(r.payload, json!({"key": "x", "value": "EMPTY"}));
        let r = ex.execute("state_read", &json!({"key": "y"})).unwrap();
        assert_eq!(r.payload, json!({"key": "y", "value": "B"}));
    }

    #[test]
    fn write_updates_state_and_reports_it() {
        let mut ex = ToolExecutor::fresh();
        ex.execute("state_write", &json!({"key": "x", "value": "A"}))
            .unwrap();
        let r = ex.execute("state_read", &json!({"key": "x"})).unwrap();
        assert_eq!(r.payload, json!({"key": "x", "value": "A"}));

        // write result does not implicitly verify via read
        let mut ex2 = ToolExecutor::fresh();
        let r = ex2
            .execute("state_write", &json!({"key": "x", "value": "A"}))
            .unwrap();
        assert_eq!(r.payload, json!({"ok": true, "key": "x", "value": "A"}));
    }

    // --- fresh external state per episode ------------------------------------

    #[test]
    fn executors_do_not_share_state() {
        let mut a = ToolExecutor::fresh();
        let mut b = ToolExecutor::fresh();
        a.execute("state_write", &json!({"key": "x", "value": "A"}))
            .unwrap();
        let r = b.execute("state_read", &json!({"key": "x"})).unwrap();
        assert_eq!(r.payload["value"], "EMPTY");
    }

    // --- argument validation --------------------------------------------------

    #[test]
    fn unknown_tool_is_rejected() {
        let mut ex = ToolExecutor::fresh();
        let err = ex.execute("state_delete", &json!({})).unwrap_err();
        assert!(matches!(err, ToolError::UnknownTool(ref n) if n == "state_delete"));
    }

    #[test]
    fn state_read_rejects_bad_arguments() {
        let mut ex = ToolExecutor::fresh();
        // missing key
        assert!(matches!(
            ex.execute("state_read", &json!({})).unwrap_err(),
            ToolError::InvalidArguments { .. }
        ));
        // non-string key
        assert!(matches!(
            ex.execute("state_read", &json!({"key": 1})).unwrap_err(),
            ToolError::InvalidArguments { .. }
        ));
        // out-of-enum key
        assert!(matches!(
            ex.execute("state_read", &json!({"key": "z"})).unwrap_err(),
            ToolError::InvalidArguments { .. }
        ));
        // additionalProperties: false
        assert!(matches!(
            ex.execute("state_read", &json!({"key": "x", "extra": 1}))
                .unwrap_err(),
            ToolError::InvalidArguments { .. }
        ));
        // non-object
        assert!(matches!(
            ex.execute("state_read", &json!("x")).unwrap_err(),
            ToolError::InvalidArguments { .. }
        ));
    }

    #[test]
    fn state_write_rejects_bad_arguments() {
        let mut ex = ToolExecutor::fresh();
        assert!(matches!(
            ex.execute("state_write", &json!({"key": "x"})).unwrap_err(),
            ToolError::InvalidArguments { .. }
        ));
        assert!(matches!(
            ex.execute("state_write", &json!({"key": "x", "value": 5}))
                .unwrap_err(),
            ToolError::InvalidArguments { .. }
        ));
        assert!(matches!(
            ex.execute("state_write", &json!({"key": "z", "value": "A"}))
                .unwrap_err(),
            ToolError::InvalidArguments { .. }
        ));
        assert!(matches!(
            ex.execute("state_write", &json!({"key": "x", "value": "A", "n": 1}))
                .unwrap_err(),
            ToolError::InvalidArguments { .. }
        ));
        // invalid writes must not mutate state
        ex.execute("state_write", &json!({"key": "x"})).ok();
        let r = ex.execute("state_read", &json!({"key": "x"})).unwrap();
        assert_eq!(r.payload["value"], "EMPTY");
    }

    #[test]
    fn specs_contain_exactly_the_two_state_tools() {
        let specs = ToolExecutor::specs();
        let names: Vec<&str> = specs.iter().map(|s| s.function.name.as_str()).collect();
        assert_eq!(names, vec!["state_read", "state_write"]);
        // schemas match the experiment spec
        assert_eq!(specs[0].function.parameters["required"], json!(["key"]));
        assert_eq!(
            specs[0].function.parameters["additionalProperties"],
            json!(false)
        );
        assert_eq!(
            specs[1].function.parameters["properties"]["key"]["enum"],
            json!(["x", "y"])
        );
    }
}
