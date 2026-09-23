//! Experiment-local tools: exactly two, `state_read` and `state_write`,
//! plus the Experiment 0009 deterministic fault-injection environment.
//!
//! Capability boundary: the model receives only these two tools. No
//! shell, filesystem, browser, or network tool exists. External state is
//! an explicit value inside `ExternalState`; no global mutable state.
//! Each episode receives a **fresh** `ToolExecutor` constructed from the
//! task's registered initial state — no state is ever reused between
//! episodes.
//!
//! Fault injection (Experiment 0009): a registered deterministic stress
//! ladder of *repeated silent write drops*, `DropFirstNWrites { key,
//! count }` with `count ∈ {1, 2, 3, 4}` (plus `Reliable` as the S0
//! sanity control). The first `count` writes to the registered key are
//! silently dropped — each still returns the *identical* model-visible
//! success response — while writes from the (count+1)-th onward apply
//! normally, as do writes to other keys. The fault is invisible to the
//! model (it can only discover it by reading state) and is recorded in
//! an experiment-only `WriteAttempt` audit log that never enters the LLM
//! transcript.

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

/// The deterministic external state for one episode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalState {
    x: String,
    y: String,
}

impl ExternalState {
    /// Build from a registered initial-state object `{"x": ..., "y": ...}`.
    pub fn from_value(v: &Value) -> Self {
        let get = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        Self {
            x: get("x"),
            y: get("y"),
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

/// Registered experiment fault semantics (spec §15).
///
/// Serializes as `{"mode": "reliable"}` or
/// `{"mode": "drop_first_n_writes", "key": "x", "count": 2}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum FaultMode {
    /// Every valid `state_write` updates external state immediately.
    /// This is stress level S0.
    Reliable,
    /// The first `count` writes to the registered key are silently
    /// dropped (count ∈ {1,2,3,4}); the (count+1)-th and later writes
    /// apply normally, as do all writes to other keys.
    DropFirstNWrites { key: String, count: u32 },
}

impl FaultMode {
    /// Derive the fault mode for a (fault key, stress level) pair.
    /// `drop_count == 0` is S0 and maps to `Reliable`.
    pub fn for_stress(key: &str, drop_count: u32) -> Self {
        if drop_count == 0 {
            FaultMode::Reliable
        } else {
            FaultMode::DropFirstNWrites {
                key: key.to_string(),
                count: drop_count,
            }
        }
    }

    /// The registered target key, if the mode has one and it is a valid
    /// state key.
    pub fn target_key(&self) -> Option<StateKey> {
        match self {
            FaultMode::Reliable => None,
            FaultMode::DropFirstNWrites { key, .. } => StateKey::parse(key),
        }
    }
}

/// Internal, experiment-only audit record of one write attempt
/// (spec §18).
///
/// Persisted in artifacts as `environment_write_attempts` so the fault
/// schedule can be proven to have executed exactly as registered. This
/// record is NEVER inserted into the LLM transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteAttempt {
    /// 0-based index of this write attempt within the episode.
    pub sequence: u32,
    pub key: String,
    pub requested_value: String,
    /// Whether the external state was actually updated.
    pub applied: bool,
    /// `Some("drop_first_n_writes")` when the fault caused the drop,
    /// else `None`.
    pub fault_reason: Option<String>,
    /// 1-based ordinal of this attempt among writes to the *registered*
    /// target key (`None` for other keys). Proves which writes fall
    /// inside the registered drop window (ordinals ≤ count).
    pub registered_drop_index: Option<u32>,
}

/// A tool execution failure that the kernel can record.
///
/// Only the two state tools exist, and they cannot fail once their
/// arguments pass schema validation — so exactly two failure shapes
/// remain, both of which are model-behavior (agent) failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// The model requested a tool that does not exist.
    UnknownTool(String),
    /// The model supplied arguments that violate the tool schema.
    InvalidArguments { tool: String, detail: String },
}

/// Result of executing one tool call: a JSON payload to return to the
/// model as the tool result message.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub payload: Value,
}

/// Executes tool calls against one episode's external state under one
/// registered fault mode. A fresh executor is constructed per episode.
pub struct ToolExecutor {
    state: ExternalState,
    fault: FaultMode,
    /// Number of registered silent drops still pending for the target key.
    drops_remaining: u32,
    /// 1-based ordinal counter of writes to the registered target key.
    target_write_ordinal: u32,
    write_attempts: Vec<WriteAttempt>,
}

impl ToolExecutor {
    /// Construct an executor with the task's registered initial state.
    pub fn fresh(initial: &Value, fault: FaultMode) -> Self {
        let drops_remaining = match &fault {
            FaultMode::Reliable => 0,
            FaultMode::DropFirstNWrites { count, .. } => *count,
        };
        Self {
            state: ExternalState::from_value(initial),
            drops_remaining,
            target_write_ordinal: 0,
            fault,
            write_attempts: Vec::new(),
        }
    }

    /// Current state as a JSON object (`{"x": ..., "y": ...}`).
    pub fn final_state_value(&self) -> Value {
        self.state.as_value()
    }

    /// The write-attempt audit log, in execution order.
    pub fn write_attempts(&self) -> &[WriteAttempt] {
        &self.write_attempts
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

    /// Execute a tool call. Fails on unknown tools and schema violations.
    pub fn execute(&mut self, name: &str, arguments: &Value) -> Result<ToolResult, ToolError> {
        match name {
            "state_read" => self.read(arguments),
            "state_write" => self.write(arguments),
            other => Err(ToolError::UnknownTool(other.to_string())),
        }
    }

    fn read(&self, args: &Value) -> Result<ToolResult, ToolError> {
        if !args.is_object() {
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
        if !args.is_object() {
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

        // Fault semantics: the registered first-N drops are silent.
        let is_target = self.fault.target_key().map(|t| t == key).unwrap_or(false);
        if is_target {
            self.target_write_ordinal += 1;
        }
        let (applied, fault_reason) = if is_target && self.drops_remaining > 0 {
            self.drops_remaining -= 1;
            (false, Some("drop_first_n_writes".to_string()))
        } else {
            (true, None)
        };

        if applied {
            self.state.set(key, value);
        }

        // Experiment-only audit record (never enters the transcript).
        self.write_attempts.push(WriteAttempt {
            sequence: self.write_attempts.len() as u32,
            key: key.as_str().to_string(),
            requested_value: value.to_string(),
            applied,
            fault_reason,
            registered_drop_index: is_target.then_some(self.target_write_ordinal),
        });

        // Model-visible result: identical success in both the applied
        // and the silently-dropped case. The model can only discover the
        // fault by reading state.
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
    args.get("key")
        .and_then(Value::as_str)
        .and_then(StateKey::parse)
        .ok_or_else(|| ToolError::InvalidArguments {
            tool: "state".into(),
            detail: "missing or invalid \"key\" (expected \"x\" or \"y\")".into(),
        })
}

fn reject_extra_keys(args: &Value, expected: &[&str]) -> Result<(), String> {
    if let Some(obj) = args.as_object() {
        let mut extra: Vec<&str> = obj
            .keys()
            .filter(|k| !expected.contains(&k.as_str()))
            .map(String::as_str)
            .collect();
        extra.sort();
        if !extra.is_empty() {
            return Err(format!("unexpected argument(s): {}", extra.join(", ")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reliable_exec() -> ToolExecutor {
        ToolExecutor::fresh(&json!({"x": "EMPTY", "y": "B"}), FaultMode::Reliable)
    }

    #[test]
    fn stress_ladder_is_registered() {
        assert_eq!(FaultMode::for_stress("x", 0), FaultMode::Reliable);
        assert_eq!(
            FaultMode::for_stress("x", 3),
            FaultMode::DropFirstNWrites {
                key: "x".into(),
                count: 3
            }
        );
    }

    #[test]
    fn reliable_write_applies_and_reports_success() {
        let mut exec = reliable_exec();
        let r = exec
            .execute("state_write", &json!({"key": "x", "value": "A"}))
            .unwrap();
        assert_eq!(r.payload, json!({"ok": true, "key": "x", "value": "A"}));
        assert_eq!(exec.final_state_value()["x"], "A");
        let a = &exec.write_attempts()[0];
        assert!(a.applied);
        assert_eq!(a.fault_reason, None);
        assert_eq!(a.registered_drop_index, None);
    }

    #[test]
    fn drop_n_drops_exactly_the_first_n_target_writes() {
        let mut exec = ToolExecutor::fresh(
            &json!({"x": "EMPTY", "y": "B"}),
            FaultMode::DropFirstNWrites {
                key: "x".into(),
                count: 2,
            },
        );

        let r1 = exec
            .execute("state_write", &json!({"key": "x", "value": "A"}))
            .unwrap();
        assert_eq!(exec.final_state_value()["x"], "EMPTY"); // dropped
        assert_eq!(
            exec.write_attempts()[0],
            WriteAttempt {
                sequence: 0,
                key: "x".into(),
                requested_value: "A".into(),
                applied: false,
                fault_reason: Some("drop_first_n_writes".into()),
                registered_drop_index: Some(1),
            }
        );

        let r2 = exec
            .execute("state_write", &json!({"key": "x", "value": "A"}))
            .unwrap();
        assert_eq!(exec.final_state_value()["x"], "EMPTY"); // dropped
        assert_eq!(exec.write_attempts()[1].registered_drop_index, Some(2));
        assert!(!exec.write_attempts()[1].applied);

        // Third write is beyond the registered window: applies.
        let r3 = exec
            .execute("state_write", &json!({"key": "x", "value": "A"}))
            .unwrap();
        assert_eq!(exec.final_state_value()["x"], "A");
        assert!(exec.write_attempts()[2].applied);
        assert_eq!(exec.write_attempts()[2].fault_reason, None);
        assert_eq!(exec.write_attempts()[2].registered_drop_index, Some(3));

        // Model-visible payloads are byte-identical: the fault is
        // invisible to the model.
        assert_eq!(r1.payload, r2.payload);
        assert_eq!(r2.payload, r3.payload);
    }

    #[test]
    fn drop_targets_only_the_registered_key() {
        let mut exec = ToolExecutor::fresh(
            &json!({"x": "A", "y": "B"}),
            FaultMode::DropFirstNWrites {
                key: "y".into(),
                count: 1,
            },
        );
        exec.execute("state_write", &json!({"key": "x", "value": "C"}))
            .unwrap();
        exec.execute("state_write", &json!({"key": "y", "value": "C"}))
            .unwrap();
        exec.execute("state_write", &json!({"key": "y", "value": "C"}))
            .unwrap();
        assert_eq!(exec.final_state_value(), json!({"x": "C", "y": "C"}));
        // x write: applied, not a target-key write.
        assert!(exec.write_attempts()[0].applied);
        assert_eq!(exec.write_attempts()[0].registered_drop_index, None);
        // First y write: dropped; second: applied.
        assert!(!exec.write_attempts()[1].applied);
        assert_eq!(exec.write_attempts()[1].registered_drop_index, Some(1));
        assert!(exec.write_attempts()[2].applied);
        assert_eq!(exec.write_attempts()[2].registered_drop_index, Some(2));
    }

    #[test]
    fn read_returns_current_state() {
        let mut exec = reliable_exec();
        exec.execute("state_write", &json!({"key": "y", "value": "C"}))
            .unwrap();
        let r = exec.execute("state_read", &json!({"key": "y"})).unwrap();
        assert_eq!(r.payload, json!({"key": "y", "value": "C"}));
    }

    #[test]
    fn schema_violations_are_invalid_arguments() {
        let mut exec = reliable_exec();
        assert!(matches!(
            exec.execute("state_read", &json!({"key": "z"})),
            Err(ToolError::InvalidArguments { .. })
        ));
        assert!(matches!(
            exec.execute("state_write", &json!({"key": "x"})),
            Err(ToolError::InvalidArguments { .. })
        ));
        assert!(matches!(
            exec.execute(
                "state_write",
                &json!({"key": "x", "value": "A", "extra": 1})
            ),
            Err(ToolError::InvalidArguments { .. })
        ));
        // A schema violation must not mutate state or log a write.
        assert_eq!(exec.final_state_value(), json!({"x": "EMPTY", "y": "B"}));
        assert!(exec.write_attempts().is_empty());
    }

    #[test]
    fn unknown_tool_is_rejected() {
        let mut exec = reliable_exec();
        assert!(matches!(
            exec.execute("shell", &json!({})),
            Err(ToolError::UnknownTool(_))
        ));
    }

    #[test]
    fn fresh_executors_do_not_share_state_or_fault_budget() {
        let fault = FaultMode::DropFirstNWrites {
            key: "x".into(),
            count: 2,
        };
        let initial = json!({"x": "EMPTY", "y": "B"});
        let mut e1 = ToolExecutor::fresh(&initial, fault.clone());
        e1.execute("state_write", &json!({"key": "x", "value": "A"}))
            .unwrap();
        e1.execute("state_write", &json!({"key": "x", "value": "A"}))
            .unwrap();
        e1.execute("state_write", &json!({"key": "x", "value": "A"}))
            .unwrap();
        assert_eq!(e1.final_state_value()["x"], "A"); // 2 dropped, 3rd applies

        let mut e2 = ToolExecutor::fresh(&initial, fault);
        e2.execute("state_write", &json!({"key": "x", "value": "A"}))
            .unwrap();
        // Fresh episode: its own two-drop budget is intact.
        assert_eq!(e2.final_state_value()["x"], "EMPTY");
        assert!(!e2.write_attempts()[0].applied);
    }
}
