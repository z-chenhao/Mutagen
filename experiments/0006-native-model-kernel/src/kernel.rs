//! The experimental agent kernel.
//!
//! What this module knows (and nothing else):
//!
//! - the message transcript
//! - the model (as a call closure — one concrete provider behind it)
//! - the available tools (specs + executor)
//! - external state (owned by the `ToolExecutor`)
//! - turn / tool-call counts
//! - its own trajectory events
//! - termination and eligibility
//!
//! What this module deliberately does NOT know: CLI arguments, output
//! formatting, artifact files, filesystem paths, sessions, branching,
//! compaction, skills, extensions, TUI/RPC, and any other product
//! feature. Presentation and artifact handling live in `main.rs` and
//! `experiment.rs`.
//!
//! The kernel is the authoritative execution recorder: every trajectory
//! event is emitted here, in execution order, into an in-memory vector.

use std::time::Instant;

use serde_json::Value;
use serde_json::json;

use crate::model::ModelError;
use crate::protocol::{Message, ModelResponse, ResponseKind, ToolSpec};
use crate::tools::{ToolError, ToolExecutor};
use crate::trace::{
    EpisodeRecord, TerminationReason, Timing, ToolCallRecord, TrajectoryEvent, UsageRecord,
};

/// Hard experiment limits (spec §27).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KernelConfig {
    pub max_model_turns: usize,
    pub max_tool_calls: usize,
}

impl Default for KernelConfig {
    fn default() -> Self {
        Self {
            max_model_turns: 12,
            max_tool_calls: 16,
        }
    }
}

/// Minimal episode state driving one run.
struct EpisodeState {
    messages: Vec<Message>,
    events: Vec<TrajectoryEvent>,
    tool_call_records: Vec<ToolCallRecord>,
    model_turn_count: usize,
    tool_call_count: usize,
}

/// The complete, kernel-produced result of one episode.
pub struct EpisodeOutcome {
    /// The kernel's own authoritative event log. Consumed by the
    /// kernel's unit tests; production runs persist the derived
    /// `tool_calls` records instead of the raw event vector.
    #[allow(dead_code)]
    pub events: Vec<TrajectoryEvent>,
    pub tool_calls: Vec<ToolCallRecord>,
    pub final_answer: Option<String>,
    pub termination_reason: TerminationReason,
    pub eligible: bool,
    pub error: Option<String>,
    pub model_turn_count: usize,
    pub tool_call_count: usize,
    pub usage: UsageRecord,
    pub timing: Timing,
    pub final_state: Value,
}

/// Run one full episode: task prompt → turns → final answer or failure.
///
/// `call_model` is the model boundary. In the real run it is
/// `ModelClient::complete`; in network-free tests it is a scripted
/// closure. `tools` must be a *fresh* `ToolExecutor` per episode so no
/// external state is reused between episodes.
pub fn run_episode(
    mut call_model: impl FnMut(&[Message], &[ToolSpec]) -> Result<ModelResponse, ModelError>,
    tools: &mut ToolExecutor,
    specs: &[ToolSpec],
    system_prompt: &str,
    task: &str,
    config: &KernelConfig,
) -> EpisodeOutcome {
    let started = Instant::now();

    let mut state = EpisodeState {
        messages: vec![Message::system(system_prompt), Message::user(task)],
        events: Vec::new(),
        tool_call_records: Vec::new(),
        model_turn_count: 0,
        tool_call_count: 0,
    };
    state.events.push(TrajectoryEvent::EpisodeStarted);

    let mut usage = UsageRecord::default();
    let mut model_wait_total = std::time::Duration::ZERO;
    let mut tool_exec_total = std::time::Duration::ZERO;
    // All loop exits reassign; the placeholder exists only for the
    // borrow checker.
    #[allow(unused_assignments)]
    let mut termination = TerminationReason::Completed;
    let mut error: Option<String> = None;
    let mut final_answer: Option<String> = None;

    loop {
        // Turn limit: no further model call.
        if state.model_turn_count >= config.max_model_turns {
            termination = TerminationReason::TurnLimit;
            error = Some(format!(
                "model turn limit reached ({} turns, no final answer)",
                config.max_model_turns
            ));
            break;
        }

        let turn = state.model_turn_count + 1;
        state
            .events
            .push(TrajectoryEvent::ModelTurnStarted { turn });
        let t = Instant::now();
        let response = call_model(&state.messages, specs);
        model_wait_total += t.elapsed();

        let resp = match response {
            Ok(r) => r,
            Err(e) => {
                // Transport and non-2xx statuses are "model HTTP
                // failures"; 2xx-but-unparseable bodies are parse
                // failures.
                let reason = match &e {
                    ModelError::Http(_) | ModelError::Status(_) => TerminationReason::HttpError,
                    ModelError::Parse(_) => TerminationReason::ParseError,
                };
                state.events.push(TrajectoryEvent::EpisodeFailed {
                    reason: e.message(),
                });
                termination = reason;
                error = Some(e.message());
                break;
            }
        };
        state.model_turn_count += 1;
        usage.add(&resp.usage);

        let kind = resp.kind();
        state.events.push(TrajectoryEvent::ModelResponseReceived {
            turn,
            response_kind: kind_name(kind).to_string(),
        });

        match kind {
            ResponseKind::FinalAnswer => {
                let text = resp.content.unwrap_or_default();
                state.messages.push(Message::assistant_text(text.clone()));
                final_answer = Some(text.clone());
                state.events.push(TrajectoryEvent::FinalAnswer { text });
                termination = TerminationReason::Completed;
                break;
            }
            ResponseKind::SingleToolCall => {
                let tool_call = &resp.tool_calls[0];
                let name = tool_call.function.name.clone();
                state.messages.push(Message::assistant_tool_call(
                    tool_call.id.clone(),
                    tool_call.function.clone(),
                    resp.content.clone(),
                ));

                let arguments: Value = match serde_json::from_str(&tool_call.function.arguments) {
                    Ok(v) => v,
                    Err(e) => {
                        let reason = format!("tool call arguments are not valid JSON: {e}");
                        state.events.push(TrajectoryEvent::EpisodeFailed {
                            reason: reason.clone(),
                        });
                        termination = TerminationReason::ParseError;
                        error = Some(reason);
                        break;
                    }
                };

                state.events.push(TrajectoryEvent::ToolCallRequested {
                    turn,
                    tool_call_id: tool_call.id.clone(),
                    tool_name: name.clone(),
                    arguments: arguments.clone(),
                });

                // Tool-call limit: the request is recorded, the call is
                // not executed, and the episode is ineligible.
                if state.tool_call_count >= config.max_tool_calls {
                    termination = TerminationReason::ToolCallLimit;
                    let reason = format!("tool call limit reached ({})", config.max_tool_calls);
                    state.events.push(TrajectoryEvent::EpisodeFailed {
                        reason: reason.clone(),
                    });
                    error = Some(reason);
                    break;
                }

                state.events.push(TrajectoryEvent::ToolExecutionStarted {
                    tool_call_id: tool_call.id.clone(),
                    tool_name: name.clone(),
                });
                let te = Instant::now();
                let exec = tools.execute(&name, &arguments);
                tool_exec_total += te.elapsed();

                let result = match exec {
                    Ok(r) => r.payload,
                    Err(ToolError::UnknownTool(n)) => {
                        let reason = format!("model requested unknown tool {n:?}");
                        state.events.push(TrajectoryEvent::EpisodeFailed {
                            reason: reason.clone(),
                        });
                        termination = TerminationReason::UnknownTool;
                        error = Some(reason);
                        break;
                    }
                    Err(ToolError::InvalidArguments { tool, detail }) => {
                        let reason = format!("invalid arguments for {tool}: {detail}");
                        state.events.push(TrajectoryEvent::EpisodeFailed {
                            reason: reason.clone(),
                        });
                        termination = TerminationReason::InvalidArguments;
                        error = Some(reason);
                        break;
                    }
                    Err(ToolError::Execution(detail)) => {
                        state.events.push(TrajectoryEvent::EpisodeFailed {
                            reason: format!("tool execution failed: {detail}"),
                        });
                        termination = TerminationReason::EpisodeFailed;
                        error = Some(detail);
                        break;
                    }
                };

                let duration_us = te.elapsed().as_micros() as u64;
                state.events.push(TrajectoryEvent::ToolExecutionFinished {
                    tool_call_id: tool_call.id.clone(),
                    tool_name: name.clone(),
                    result: result.clone(),
                    duration_us,
                });

                state.messages.push(Message::tool_result(
                    tool_call.id.clone(),
                    name.clone(),
                    result.to_string(),
                ));

                state.tool_call_records.push(ToolCallRecord {
                    sequence: state.tool_call_records.len(),
                    turn,
                    tool_call_id: tool_call.id.clone(),
                    tool_name: name,
                    arguments,
                    result,
                });
                state.tool_call_count += 1;
            }
            ResponseKind::ParallelToolCalls => {
                // Record the (rejected) assistant message for transcript
                // fidelity, then fail the episode: the kernel does not
                // reorder or silently serialize parallel calls.
                state.messages.push(Message {
                    role: crate::protocol::Role::Assistant,
                    content: resp.content.clone(),
                    tool_calls: resp.tool_calls.clone(),
                    tool_call_id: None,
                    name: None,
                });
                let reason = format!(
                    "assistant response contains {} parallel tool calls (unsupported)",
                    resp.tool_calls.len()
                );
                state.events.push(TrajectoryEvent::EpisodeFailed {
                    reason: reason.clone(),
                });
                termination = TerminationReason::ParallelToolCallsUnsupported;
                error = Some(reason);
                break;
            }
            ResponseKind::Empty => {
                let reason = "assistant response carried neither text nor tool calls".to_string();
                state.events.push(TrajectoryEvent::EpisodeFailed {
                    reason: reason.clone(),
                });
                termination = TerminationReason::ParseError;
                error = Some(reason);
                break;
            }
        }
    }

    let eligible = termination == TerminationReason::Completed;
    state.events.push(TrajectoryEvent::EpisodeFinished {
        eligible,
        termination_reason: termination.as_str().to_string(),
    });

    let total = started.elapsed();
    // Labeled approximate: residual of total minus model wait minus tool
    // execution (spec §52).
    let overhead = total.saturating_sub(model_wait_total + tool_exec_total);

    EpisodeOutcome {
        events: state.events,
        tool_calls: state.tool_call_records,
        final_answer,
        termination_reason: termination,
        eligible,
        error,
        model_turn_count: state.model_turn_count,
        tool_call_count: state.tool_call_count,
        usage,
        timing: Timing {
            total_wall_time_ms: total.as_millis() as u64,
            model_wait_time_ms: model_wait_total.as_millis() as u64,
            tool_execution_time_us: tool_exec_total.as_micros() as u64,
            kernel_overhead_estimate_ms: overhead.as_millis() as u64,
            approximate: true,
        },
        final_state: tools.final_state_value(),
    }
}

fn kind_name(kind: ResponseKind) -> &'static str {
    match kind {
        ResponseKind::FinalAnswer => "final_answer",
        ResponseKind::SingleToolCall => "single_tool_call",
        ResponseKind::ParallelToolCalls => "parallel_tool_calls",
        ResponseKind::Empty => "empty",
    }
}

/// Build a persisted `EpisodeRecord` from a kernel outcome. Done by the
/// runner, never by the kernel (spec §15).
pub fn build_record(outcome: EpisodeOutcome, meta: &RecordMeta) -> EpisodeRecord {
    EpisodeRecord {
        experiment_id: "0006".into(),
        run_id: meta.run_id.clone(),
        task_id: meta.task_id.clone(),
        task_name: meta.task_name.clone(),
        condition: meta.condition.clone(),
        repetition: meta.repetition,
        model: meta.model.clone(),
        temperature: crate::protocol::TEMPERATURE,
        redacted_endpoint: meta.redacted_endpoint.clone(),
        code_under_test_commit: meta.code_under_test_commit.clone(),
        baseline_prompt_sha256: meta.baseline_prompt_sha256.clone(),
        candidate_prompt_sha256: meta.candidate_prompt_sha256.clone(),
        task_suite_sha256: meta.task_suite_sha256.clone(),
        initial_state: initial_state_value(),
        final_state: outcome.final_state,
        tool_calls: outcome.tool_calls,
        final_answer: outcome.final_answer,
        model_turn_count: outcome.model_turn_count,
        tool_call_count: outcome.tool_call_count,
        usage: outcome.usage,
        timing: outcome.timing,
        eligible: outcome.eligible,
        termination_reason: outcome.termination_reason.as_str().to_string(),
        error: outcome.error,
    }
}

/// Runner-side provenance fields attached to each record.
#[derive(Debug, Clone)]
pub struct RecordMeta {
    pub run_id: String,
    pub task_id: String,
    pub task_name: String,
    pub condition: String,
    pub repetition: u32,
    pub model: String,
    pub redacted_endpoint: String,
    pub code_under_test_commit: String,
    pub baseline_prompt_sha256: String,
    pub candidate_prompt_sha256: String,
    pub task_suite_sha256: String,
}

/// The fixed initial external state, as persisted in artifacts.
pub fn initial_state_value() -> Value {
    json!({ "x": "EMPTY", "y": "B" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{FunctionCall, parse_response};
    use crate::tools::ToolExecutor;

    type ModelResult = std::result::Result<ModelResponse, ModelError>;

    /// A scripted model: returns the given JSON responses in order.
    fn scripted(responses: &[&str]) -> impl FnMut(&[Message], &[ToolSpec]) -> ModelResult {
        let mut i = 0usize;
        move |_messages: &[Message], _tools: &[ToolSpec]| {
            // Exhausted scripts repeat the last response, so tests can
            // exercise limit termination with a single canned response.
            let raw = responses[i.min(responses.len() - 1)];
            let raw: Value = serde_json::from_str(raw).unwrap();
            i += 1;
            parse_response(&raw).map_err(ModelError::Parse)
        }
    }

    const READ_X: &str = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"state_read","arguments":"{\"key\":\"x\"}"}}]},"finish_reason":"tool_calls"}]}"#;
    const WRITE_X_A: &str = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"c2","type":"function","function":{"name":"state_write","arguments":"{\"key\":\"x\",\"value\":\"A\"}"}}]},"finish_reason":"tool_calls"}]}"#;
    const PARALLEL: &str = r#"{"choices":[{"message":{"role":"assistant","content":"","tool_calls":[{"id":"c1","type":"function","function":{"name":"state_read","arguments":"{\"key\":\"x\"}"}},{"id":"c2","type":"function","function":{"name":"state_read","arguments":"{\"key\":\"y\"}"}}]},"finish_reason":"tool_calls"}]}"#;
    const FINAL: &str =
        r#"{"choices":[{"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}]}"#;

    fn run_with<F: FnMut(&[Message], &[ToolSpec]) -> ModelResult>(
        call: F,
        config: &KernelConfig,
    ) -> EpisodeOutcome {
        let mut tools = ToolExecutor::fresh();
        let specs = ToolExecutor::specs();
        run_episode(call, &mut tools, &specs, "sys", "task", config)
    }

    // --- final answer flow ---------------------------------------------------

    #[test]
    fn simple_final_answer_is_eligible() {
        let outcome = run_with(scripted(&[FINAL]), &KernelConfig::default());
        assert!(outcome.eligible);
        assert_eq!(outcome.termination_reason, TerminationReason::Completed);
        assert_eq!(outcome.final_answer.as_deref(), Some("done"));
        assert_eq!(outcome.model_turn_count, 1);
        assert!(matches!(
            outcome.events.last().unwrap(),
            TrajectoryEvent::EpisodeFinished { eligible: true, .. }
        ));
    }

    #[test]
    fn tool_call_then_final_answer_records_ordered_calls() {
        let outcome = run_with(
            scripted(&[READ_X, WRITE_X_A, FINAL]),
            &KernelConfig::default(),
        );
        assert!(outcome.eligible);
        assert_eq!(outcome.tool_call_count, 2);
        assert_eq!(outcome.model_turn_count, 3);
        // authoritative in-kernel ordering
        assert_eq!(outcome.tool_calls[0].sequence, 0);
        assert_eq!(outcome.tool_calls[0].tool_name, "state_read");
        assert_eq!(outcome.tool_calls[1].sequence, 1);
        assert_eq!(outcome.tool_calls[1].tool_name, "state_write");
        assert_eq!(outcome.tool_calls[1].result["value"], "A");
    }

    // --- limits ----------------------------------------------------------------

    #[test]
    fn turn_limit_triggers_after_max_turns() {
        // Model keeps requesting the same tool call forever.
        let outcome = run_with(scripted(&[READ_X]), &KernelConfig::default());
        assert!(!outcome.eligible);
        assert_eq!(outcome.termination_reason, TerminationReason::TurnLimit);
        assert_eq!(outcome.model_turn_count, 12);
        assert!(outcome.final_answer.is_none());
    }

    #[test]
    fn tool_call_limit_triggers_after_max_calls() {
        let config = KernelConfig {
            max_model_turns: 100,
            max_tool_calls: 3,
        };
        let outcome = run_with(scripted(&[READ_X]), &config);
        assert!(!outcome.eligible);
        assert_eq!(outcome.termination_reason, TerminationReason::ToolCallLimit);
        assert_eq!(outcome.tool_call_count, 3);
        assert_eq!(outcome.tool_calls.len(), 3);
    }

    // --- parallel rejection -------------------------------------------------------

    #[test]
    fn parallel_tool_calls_fail_the_episode() {
        let outcome = run_with(scripted(&[PARALLEL]), &KernelConfig::default());
        assert!(!outcome.eligible);
        assert_eq!(
            outcome.termination_reason,
            TerminationReason::ParallelToolCallsUnsupported
        );
        // Neither call is executed.
        assert_eq!(outcome.tool_calls.len(), 0);
        assert!(outcome.error.as_deref().unwrap().contains("2 parallel"));
    }

    // --- protocol failure handling ---------------------------------------------------

    #[test]
    fn http_failure_records_failure_without_final_answer() {
        let outcome = run_with(
            |_m: &[Message], _t: &[ToolSpec]| Err(ModelError::Http("connection refused".into())),
            &KernelConfig::default(),
        );
        assert!(!outcome.eligible);
        assert_eq!(outcome.termination_reason, TerminationReason::HttpError);
        assert_eq!(outcome.model_turn_count, 0);
    }

    #[test]
    fn malformed_arguments_are_a_parse_failure() {
        let bad = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"state_read","arguments":"{not json"}}]},"finish_reason":"tool_calls"}]}"#;
        let outcome = run_with(scripted(&[bad]), &KernelConfig::default());
        assert!(!outcome.eligible);
        assert_eq!(outcome.termination_reason, TerminationReason::ParseError);
    }

    #[test]
    fn unknown_tool_fails_the_episode() {
        let bad = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"shell","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;
        let outcome = run_with(scripted(&[bad]), &KernelConfig::default());
        assert!(!outcome.eligible);
        assert_eq!(outcome.termination_reason, TerminationReason::UnknownTool);
        assert!(outcome.tool_calls.is_empty());
    }

    #[test]
    fn invalid_arguments_fail_the_episode_and_do_not_mutate_state() {
        let bad = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"state_write","arguments":"{\"key\":\"z\",\"value\":\"A\"}"}}]},"finish_reason":"tool_calls"}]}"#;
        let outcome = run_with(scripted(&[bad]), &KernelConfig::default());
        assert!(!outcome.eligible);
        assert_eq!(
            outcome.termination_reason,
            TerminationReason::InvalidArguments
        );
        assert!(outcome.tool_calls.is_empty());
    }

    // --- event ordering -----------------------------------------------------------------

    #[test]
    fn events_follow_lifecycle_order() {
        let outcome = run_with(scripted(&[READ_X, FINAL]), &KernelConfig::default());
        let kinds: Vec<&str> = outcome
            .events
            .iter()
            .map(|e| match e {
                TrajectoryEvent::EpisodeStarted => "started",
                TrajectoryEvent::ModelTurnStarted { .. } => "turn",
                TrajectoryEvent::ModelResponseReceived { .. } => "response",
                TrajectoryEvent::ToolCallRequested { .. } => "requested",
                TrajectoryEvent::ToolExecutionStarted { .. } => "exec_start",
                TrajectoryEvent::ToolExecutionFinished { .. } => "exec_end",
                TrajectoryEvent::FinalAnswer { .. } => "final",
                TrajectoryEvent::EpisodeFinished { .. } => "finished",
                TrajectoryEvent::EpisodeFailed { .. } => "failed",
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "started",
                "turn",
                "response",
                "requested",
                "exec_start",
                "exec_end",
                "turn",
                "response",
                "final",
                "finished"
            ]
        );
    }

    // --- fresh state per episode ------------------------------------------------------------

    #[test]
    fn two_episodes_do_not_share_external_state() {
        // Episode 1 writes x=A. Episode 2 must still read x=EMPTY.
        let outcome1 = run_with(scripted(&[WRITE_X_A, FINAL]), &KernelConfig::default());
        assert_eq!(outcome1.tool_calls[0].result["value"], "A");

        let outcome2 = run_with(scripted(&[READ_X, FINAL]), &KernelConfig::default());
        assert_eq!(outcome2.tool_calls[0].result["value"], "EMPTY");
    }

    #[test]
    fn final_state_reflects_episode_writes() {
        let outcome = run_with(scripted(&[WRITE_X_A, FINAL]), &KernelConfig::default());
        assert_eq!(outcome.final_state["x"], "A");
        assert_eq!(outcome.final_state["y"], "B");
    }

    // --- assistant message round-trip ---------------------------------------------------

    #[test]
    fn assistant_tool_call_message_round_trips() {
        let m = Message::assistant_tool_call(
            "c1",
            FunctionCall {
                name: "state_read".into(),
                arguments: r#"{"key":"x"}"#.into(),
            },
            None,
        );
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["tool_calls"][0]["function"]["name"], "state_read");
    }
}
