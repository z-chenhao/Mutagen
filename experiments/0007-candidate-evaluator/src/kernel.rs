//! The experimental agent kernel (Rust-owned turn loop).
//!
//! Carried over from Experiment 0006, with Experiment 0007 additions:
//! - per-request usage recording (including `null` when the provider
//!   did not report a field), with a computed `uncached_prompt_tokens`;
//! - separate `requested_tool_calls` and `executed_tool_calls`
//!   records, so rejected parallel responses are preserved;
//! - fault-injecting tool environment (see `tools.rs`).
//!
//! What this module knows (and nothing else): the message transcript,
//! the model (as a call closure), the tools (specs + a fresh
//! per-episode executor), turn / tool-call counts, and its own records.
//!
//! What this module deliberately does NOT know: CLI arguments, output
//! formatting, artifact files, prompts-by-condition, the task registry,
//! the oracle, or any experiment presentation. The kernel never grades
//! anything: it records execution; the oracle grades state.
//!
//! Parallel tool policy (spec §15): exactly one tool call per assistant
//! turn. A response containing more than one tool call fails the
//! episode with `parallel_tool_calls_unsupported`; none of the calls
//! are executed, and all of them are recorded as requested.

use std::time::Instant;

use serde_json::Value;

use crate::model::ModelError;
use crate::protocol::{Message, ModelResponse, ResponseKind, ToolSpec};
use crate::tools::{ToolError, ToolExecutor};
use crate::trace::{
    EpisodeRecord, ExecutedToolCall, ModelRequestRecord, RecordMeta, RequestedToolCall,
    TerminationReason, Timing,
};

/// Hard experiment limits (spec §16): identical across all conditions.
/// Limit hits count as agent-caused task failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KernelConfig {
    pub max_model_turns: u32,
    pub max_tool_calls: u32,
}

impl Default for KernelConfig {
    fn default() -> Self {
        Self {
            max_model_turns: 12,
            max_tool_calls: 16,
        }
    }
}

/// The complete, kernel-produced result of one episode.
pub struct EpisodeOutcome {
    pub termination_reason: TerminationReason,
    pub final_answer: Option<String>,
    /// One entry per model request attempt, in order (turn 1..n).
    pub model_requests: Vec<ModelRequestRecord>,
    pub requested_tool_calls: Vec<RequestedToolCall>,
    pub executed_tool_calls: Vec<ExecutedToolCall>,
    pub write_attempts: Vec<crate::tools::WriteAttempt>,
    pub final_state: Value,
    /// Count of model turns that returned a usable response.
    pub model_turn_count: u32,
    /// Count of executed tool calls.
    pub tool_call_count: u32,
    /// Some when a request's reported usage failed arithmetic
    /// validation (e.g. cached > prompt). Fails artifact verification.
    pub usage_accounting_error: Option<String>,
    pub timing: Timing,
}

/// A fresh `ModelRequestRecord` for one attempted request: every usage
/// field is `null` until the provider reports it.
fn empty_request(turn: u32) -> ModelRequestRecord {
    ModelRequestRecord {
        turn,
        finish_reason: None,
        prompt_tokens: None,
        completion_tokens: None,
        total_tokens: None,
        cached_prompt_tokens: None,
        reasoning_tokens: None,
        uncached_prompt_tokens: None,
    }
}

/// Fill the usage fields of a request record from a parsed response.
///
/// `uncached_prompt_tokens = prompt - cached` only when *both* are
/// reported; when the provider omits cached detail it stays `null`
/// (spec: never inferred, never zero by assumption). A reported
/// `cached > prompt` is a usage accounting error, not a saturation.
fn apply_usage(request: &mut ModelRequestRecord, resp: &ModelResponse) -> Option<String> {
    let usage = &resp.usage;
    request.finish_reason = resp.finish_reason.clone();
    request.prompt_tokens = usage.prompt_tokens;
    request.completion_tokens = usage.completion_tokens;
    request.total_tokens = usage.total_tokens;
    request.cached_prompt_tokens = usage.cached_prompt_tokens;
    request.reasoning_tokens = usage.reasoning_tokens;
    request.uncached_prompt_tokens = match (usage.prompt_tokens, usage.cached_prompt_tokens) {
        (Some(p), Some(c)) if c <= p => Some(p - c),
        (Some(p), Some(c)) => {
            return Some(format!(
                "cached_prompt_tokens {c} exceeds prompt_tokens {p} (turn {})",
                request.turn
            ));
        }
        _ => None,
    };
    None
}

/// Run one full episode: system + user → turns → final answer or
/// failure.
///
/// `call_model` is the model boundary: `ModelClient::complete` in the
/// real run, a scripted closure in network-free tests. `tools` must be a
/// *fresh* `ToolExecutor` per episode so no external state or fault
/// budget is reused between episodes.
pub fn run_episode(
    mut call_model: impl FnMut(&[Message], &[ToolSpec]) -> Result<ModelResponse, ModelError>,
    tools: &mut ToolExecutor,
    specs: &[ToolSpec],
    system_prompt: &str,
    task: &str,
    config: &KernelConfig,
) -> EpisodeOutcome {
    let started = Instant::now();

    let mut messages = vec![Message::system(system_prompt), Message::user(task)];
    let mut model_requests: Vec<ModelRequestRecord> = Vec::new();
    let mut requested: Vec<RequestedToolCall> = Vec::new();
    let mut executed: Vec<ExecutedToolCall> = Vec::new();
    let mut model_turn_count = 0u32;
    let mut tool_call_count = 0u32;
    let mut usage_accounting_error: Option<String> = None;
    let mut model_wait_total = std::time::Duration::ZERO;
    let mut tool_exec_total = std::time::Duration::ZERO;

    // All loop exits reassign; the placeholder exists only so the
    // borrow checker sees an initialized binding.
    #[allow(unused_assignments)]
    let mut termination = TerminationReason::Completed;
    let mut final_answer: Option<String> = None;

    loop {
        if model_turn_count >= config.max_model_turns {
            termination = TerminationReason::TurnLimit;
            break;
        }
        let turn = model_turn_count + 1;
        let t = Instant::now();
        let response = call_model(&messages, specs);
        model_wait_total += t.elapsed();

        let mut request = empty_request(turn);
        let resp = match response {
            Ok(r) => {
                model_turn_count += 1;
                if let Some(err) = apply_usage(&mut request, &r) {
                    usage_accounting_error = Some(err);
                }
                model_requests.push(request);
                r
            }
            Err(e) => {
                // The request was made but produced no usable response;
                // no usage was reported.
                model_requests.push(request);
                // Transport failures, non-2xx statuses, and malformed
                // provider responses are all infrastructure failures.
                termination = match &e {
                    ModelError::Http(_) | ModelError::Status(_) => TerminationReason::HttpError,
                    ModelError::Parse(_) => TerminationReason::MalformedResponse,
                };
                break;
            }
        };

        let kind = resp.kind();
        match kind {
            ResponseKind::FinalAnswer => {
                let text = resp.content.unwrap_or_default();
                if text.trim().is_empty() {
                    // Provider returned a response, but the model said
                    // nothing: a "no final answer" agent failure.
                    termination = TerminationReason::NoFinalAnswer;
                    break;
                }
                messages.push(Message::assistant_text(text.clone()));
                final_answer = Some(text);
                termination = TerminationReason::Completed;
                break;
            }
            ResponseKind::SingleToolCall => {
                let tool_call = &resp.tool_calls[0];
                let name = tool_call.function.name.clone();
                messages.push(Message::assistant_tool_calls(
                    vec![tool_call.clone()],
                    resp.content.clone(),
                ));

                // Record the requested call (parsed args, or the verbatim
                // raw string when the model emitted unparseable JSON).
                let arguments: Value = match serde_json::from_str(&tool_call.function.arguments) {
                    Ok(v) => v,
                    Err(_) => Value::String(tool_call.function.arguments.clone()),
                };
                requested.push(RequestedToolCall {
                    sequence: requested.len() as u32,
                    turn,
                    tool_call_id: tool_call.id.clone(),
                    tool_name: name.clone(),
                    arguments: arguments.clone(),
                });

                if arguments.is_string() {
                    termination = TerminationReason::InvalidArguments;
                    break;
                }
                if tool_call_count >= config.max_tool_calls {
                    termination = TerminationReason::ToolCallLimit;
                    break;
                }

                let te = Instant::now();
                let exec = tools.execute(&name, &arguments);
                tool_exec_total += te.elapsed();
                let result = match exec {
                    Ok(r) => r.payload,
                    Err(ToolError::UnknownTool(_)) => {
                        termination = TerminationReason::UnknownTool;
                        break;
                    }
                    Err(ToolError::InvalidArguments { .. }) => {
                        termination = TerminationReason::InvalidArguments;
                        break;
                    }
                };

                let duration_us = te.elapsed().as_micros() as u64;
                messages.push(Message::tool_result(
                    tool_call.id.clone(),
                    name.clone(),
                    result.to_string(),
                ));

                executed.push(ExecutedToolCall {
                    sequence: executed.len() as u32,
                    turn,
                    tool_call_id: tool_call.id.clone(),
                    tool_name: name,
                    arguments,
                    result,
                    duration_us,
                });
                tool_call_count += 1;
            }
            ResponseKind::ParallelToolCalls => {
                // The kernel executes at most one tool call per turn.
                // Record the (rejected) assistant message verbatim for
                // transcript fidelity, record every requested call,
                // execute none, and fail the episode.
                messages.push(Message::assistant_tool_calls(
                    resp.tool_calls.clone(),
                    resp.content.clone(),
                ));
                for tc in &resp.tool_calls {
                    let arguments: Value = match serde_json::from_str(&tc.function.arguments) {
                        Ok(v) => v,
                        Err(_) => Value::String(tc.function.arguments.clone()),
                    };
                    requested.push(RequestedToolCall {
                        sequence: requested.len() as u32,
                        turn,
                        tool_call_id: tc.id.clone(),
                        tool_name: tc.function.name.clone(),
                        arguments,
                    });
                }
                termination = TerminationReason::ParallelToolCallsUnsupported;
                break;
            }
            ResponseKind::Empty => {
                termination = TerminationReason::NoFinalAnswer;
                break;
            }
        }
    }

    let total = started.elapsed();
    let overhead = total.saturating_sub(model_wait_total + tool_exec_total);

    let final_state = tools.final_state_value();
    let write_attempts = tools.write_attempts().to_vec();

    EpisodeOutcome {
        termination_reason: termination,
        final_answer,
        model_requests,
        requested_tool_calls: requested,
        executed_tool_calls: executed,
        write_attempts,
        final_state,
        model_turn_count,
        tool_call_count,
        usage_accounting_error,
        timing: Timing {
            wall_time_ms: total.as_millis() as u64,
            model_wait_time_ms: model_wait_total.as_millis() as u64,
            tool_execution_time_us: tool_exec_total.as_micros() as u64,
            kernel_overhead_estimate_ms: overhead.as_millis() as u64,
            approximate: true,
        },
    }
}

/// Build a persisted `EpisodeRecord` from a kernel outcome plus the
/// runner-side provenance. Oracle fields are left at their defaults and
/// are filled by the runner via `EpisodeRecord::apply_oracle` — the
/// kernel does not know the oracle exists.
pub fn build_record(outcome: EpisodeOutcome, meta: &RecordMeta) -> EpisodeRecord {
    let agent_failure = outcome
        .termination_reason
        .is_agent_failure()
        .then(|| outcome.termination_reason.as_str().to_string());
    let infrastructure_failure = outcome
        .termination_reason
        .is_infrastructure()
        .then(|| outcome.termination_reason.as_str().to_string());

    EpisodeRecord {
        experiment_id: crate::trace::EXPERIMENT_ID.to_string(),
        run_id: meta.run_id.clone(),
        sequence: meta.sequence,
        task_id: meta.task.id.clone(),
        task_name: meta.task.name.clone(),
        split: meta.task.split.as_str().to_string(),
        condition: meta.condition.clone(),
        repetition: meta.repetition,
        condition_position: meta.condition_position,
        model: meta.model.clone(),
        temperature: crate::protocol::TEMPERATURE,
        redacted_endpoint: meta.redacted_endpoint.clone(),
        code_under_test_commit: meta.code_under_test_commit.clone(),
        baseline_prompt_sha256: meta.baseline_prompt_sha256.clone(),
        repair_prompt_sha256: meta.repair_prompt_sha256.clone(),
        regression_prompt_sha256: meta.regression_prompt_sha256.clone(),
        task_suite_sha256: meta.task_suite_sha256.clone(),
        initial_state: meta.task.initial_state.clone(),
        target_state: meta.task.target_state.clone(),
        fault_mode: serde_json::to_value(&meta.task.fault).unwrap_or(Value::Null),
        model_requests: outcome.model_requests,
        requested_tool_calls: outcome.requested_tool_calls,
        executed_tool_calls: outcome.executed_tool_calls,
        environment_write_attempts: outcome.write_attempts,
        final_state: outcome.final_state,
        final_answer: outcome.final_answer,
        model_turn_count: outcome.model_turn_count,
        tool_call_count: outcome.tool_call_count,
        agent_failure,
        infrastructure_failure,
        usage_accounting_error: outcome.usage_accounting_error,
        termination_reason: outcome.termination_reason.as_str().to_string(),
        oracle_success: false,
        oracle_failure_reasons: Vec::new(),
        timing: outcome.timing,
    }
}

/// Apply the oracle verdict to a record. Called by the runner, never by
/// the kernel: the oracle sees only task + termination + states.
impl EpisodeRecord {
    pub fn apply_oracle(&mut self, task: &crate::oracle::TaskSpec) {
        let eval = crate::oracle::evaluate_task(
            task,
            &crate::oracle::OracleInput {
                termination_reason: self.termination_reason.clone(),
                initial_state: self.initial_state.clone(),
                final_state: self.final_state.clone(),
            },
        );
        self.oracle_success = eval.success;
        self.oracle_failure_reasons = eval.reasons;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ToolCall, parse_response};
    use crate::tools::{FaultMode, ToolExecutor};
    use serde_json::Value;
    use serde_json::json;

    type ModelResult = std::result::Result<ModelResponse, ModelError>;

    /// A scripted model: returns the given JSON responses in order and
    /// repeats the last one once exhausted.
    fn scripted(responses: &[&str]) -> impl FnMut(&[Message], &[ToolSpec]) -> ModelResult {
        let mut i = 0usize;
        move |_messages: &[Message], _tools: &[ToolSpec]| {
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
    const USAGE: &str = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"state_read","arguments":"{\"key\":\"x\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":5,"total_tokens":105,"prompt_tokens_details":{"cached_tokens":40},"completion_tokens_details":{"reasoning_tokens":2}}}"#;

    fn run_with<F: FnMut(&[Message], &[ToolSpec]) -> ModelResult>(
        call: F,
        fault: FaultMode,
        config: &KernelConfig,
    ) -> EpisodeOutcome {
        let mut tools = ToolExecutor::fresh(&json!({"x": "EMPTY", "y": "B"}), fault);
        let specs = ToolExecutor::specs();
        run_episode(call, &mut tools, &specs, "sys", "task", config)
    }

    #[test]
    fn final_answer_completes_the_episode() {
        let outcome = run_with(
            scripted(&[FINAL]),
            FaultMode::Reliable,
            &KernelConfig::default(),
        );
        assert_eq!(outcome.termination_reason, TerminationReason::Completed);
        assert!(outcome.final_answer.is_some());
        assert!(!outcome.termination_reason.is_agent_failure());
        assert!(!outcome.termination_reason.is_infrastructure());
        assert_eq!(outcome.model_turn_count, 1);
        assert_eq!(outcome.model_requests.len(), 1);
    }

    #[test]
    fn single_call_then_final_records_requests_and_calls() {
        let outcome = run_with(
            scripted(&[READ_X, WRITE_X_A, FINAL]),
            FaultMode::Reliable,
            &KernelConfig::default(),
        );
        assert_eq!(outcome.termination_reason, TerminationReason::Completed);
        assert_eq!(outcome.tool_call_count, 2);
        assert_eq!(outcome.model_turn_count, 3);
        assert_eq!(outcome.requested_tool_calls.len(), 2);
        assert_eq!(outcome.executed_tool_calls.len(), 2);
        assert_eq!(outcome.model_requests.len(), 3);
        assert_eq!(outcome.executed_tool_calls[1].tool_name, "state_write");
        assert_eq!(outcome.executed_tool_calls[1].result["value"], "A");
    }

    #[test]
    fn per_request_usage_records_nulls_and_cache_detail() {
        let outcome = run_with(
            scripted(&[USAGE, FINAL]),
            FaultMode::Reliable,
            &KernelConfig::default(),
        );
        let q = &outcome.model_requests[0];
        assert_eq!(q.prompt_tokens, Some(100));
        assert_eq!(q.completion_tokens, Some(5));
        assert_eq!(q.total_tokens, Some(105));
        assert_eq!(q.cached_prompt_tokens, Some(40));
        assert_eq!(q.reasoning_tokens, Some(2));
        assert_eq!(q.uncached_prompt_tokens, Some(60));
        assert_eq!(q.finish_reason.as_deref(), Some("tool_calls"));
        assert!(outcome.usage_accounting_error.is_none());
    }

    #[test]
    fn absent_usage_stays_null_not_zero() {
        let outcome = run_with(
            scripted(&[FINAL]),
            FaultMode::Reliable,
            &KernelConfig::default(),
        );
        let q = &outcome.model_requests[0];
        assert_eq!(q.prompt_tokens, None);
        assert_eq!(q.uncached_prompt_tokens, None);
    }

    #[test]
    fn cached_exceeding_prompt_is_an_accounting_error() {
        let bad = r#"{"choices":[{"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":1,"total_tokens":101,"prompt_tokens_details":{"cached_tokens":101}}}"#;
        let outcome = run_with(
            scripted(&[bad]),
            FaultMode::Reliable,
            &KernelConfig::default(),
        );
        assert!(outcome.usage_accounting_error.is_some());
        assert_eq!(outcome.model_requests[0].uncached_prompt_tokens, None);
    }

    #[test]
    fn turn_limit_counts_as_agent_failure() {
        let outcome = run_with(
            scripted(&[READ_X]),
            FaultMode::Reliable,
            &KernelConfig::default(),
        );
        assert_eq!(outcome.termination_reason, TerminationReason::TurnLimit);
        assert!(TerminationReason::TurnLimit.is_agent_failure());
        assert!(!TerminationReason::TurnLimit.is_infrastructure());
        assert_eq!(outcome.model_turn_count, 12);
        assert_eq!(outcome.model_requests.len(), 12);
        assert!(outcome.final_answer.is_none());
    }

    #[test]
    fn tool_call_limit_counts_as_agent_failure() {
        let config = KernelConfig {
            max_model_turns: 100,
            max_tool_calls: 3,
        };
        let outcome = run_with(scripted(&[READ_X]), FaultMode::Reliable, &config);
        assert_eq!(outcome.termination_reason, TerminationReason::ToolCallLimit);
        assert_eq!(outcome.tool_call_count, 3);
        assert_eq!(outcome.requested_tool_calls.len(), 4);
        assert_eq!(outcome.executed_tool_calls.len(), 3);
    }

    #[test]
    fn parallel_tool_calls_fail_and_record_requested_but_execute_none() {
        let outcome = run_with(
            scripted(&[PARALLEL]),
            FaultMode::Reliable,
            &KernelConfig::default(),
        );
        assert_eq!(
            outcome.termination_reason,
            TerminationReason::ParallelToolCallsUnsupported
        );
        assert!(outcome.termination_reason.is_agent_failure());
        assert_eq!(outcome.requested_tool_calls.len(), 2);
        assert!(outcome.executed_tool_calls.is_empty());
        assert!(outcome.final_answer.is_none());
    }

    #[test]
    fn unknown_tool_is_agent_failure() {
        let bad = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"shell","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;
        let outcome = run_with(
            scripted(&[bad]),
            FaultMode::Reliable,
            &KernelConfig::default(),
        );
        assert_eq!(outcome.termination_reason, TerminationReason::UnknownTool);
        assert!(outcome.requested_tool_calls.len() == 1);
        assert!(outcome.executed_tool_calls.is_empty());
    }

    #[test]
    fn invalid_arguments_are_agent_failure() {
        let bad = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"state_read","arguments":"{\"key\":\"z\"}"}}]},"finish_reason":"tool_calls"}]}"#;
        let outcome = run_with(
            scripted(&[bad]),
            FaultMode::Reliable,
            &KernelConfig::default(),
        );
        assert_eq!(
            outcome.termination_reason,
            TerminationReason::InvalidArguments
        );
        assert!(outcome.executed_tool_calls.is_empty());
    }

    #[test]
    fn unparseable_arguments_are_agent_failure_with_verbatim_args() {
        let bad = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"state_read","arguments":"{not json"}}]},"finish_reason":"tool_calls"}]}"#;
        let outcome = run_with(
            scripted(&[bad]),
            FaultMode::Reliable,
            &KernelConfig::default(),
        );
        assert_eq!(
            outcome.termination_reason,
            TerminationReason::InvalidArguments
        );
        assert_eq!(
            outcome.requested_tool_calls[0].arguments,
            Value::String("{not json".to_string())
        );
    }

    #[test]
    fn empty_response_is_no_final_answer_agent_failure() {
        let empty = r#"{"choices":[{"message":{"role":"assistant","content":null},"finish_reason":"stop"}]}"#;
        let outcome = run_with(
            scripted(&[empty]),
            FaultMode::Reliable,
            &KernelConfig::default(),
        );
        assert_eq!(outcome.termination_reason, TerminationReason::NoFinalAnswer);
        assert!(outcome.termination_reason.is_agent_failure());
    }

    #[test]
    fn http_failure_is_infrastructure_failure() {
        let outcome = run_with(
            |_m: &[Message], _t: &[ToolSpec]| Err(ModelError::Http("refused".into())),
            FaultMode::Reliable,
            &KernelConfig::default(),
        );
        assert_eq!(outcome.termination_reason, TerminationReason::HttpError);
        assert!(outcome.termination_reason.is_infrastructure());
        assert!(!outcome.termination_reason.is_agent_failure());
        // The failed request is recorded with null usage.
        assert_eq!(outcome.model_requests.len(), 1);
        assert_eq!(outcome.model_requests[0].prompt_tokens, None);
        assert_eq!(outcome.model_turn_count, 0);
    }

    #[test]
    fn malformed_response_is_infrastructure_failure() {
        let outcome = run_with(
            |_m: &[Message], _t: &[ToolSpec]| Err(ModelError::Parse("no choices".into())),
            FaultMode::Reliable,
            &KernelConfig::default(),
        );
        assert_eq!(
            outcome.termination_reason,
            TerminationReason::MalformedResponse
        );
        assert!(outcome.termination_reason.is_infrastructure());
    }

    #[test]
    fn drop_first_write_is_silent_to_the_model_and_audited() {
        let mut tools = ToolExecutor::fresh(
            &json!({"x": "EMPTY", "y": "B"}),
            FaultMode::DropFirstWrite { key: "x".into() },
        );
        let specs = ToolExecutor::specs();
        let outcome = run_episode(
            scripted(&[WRITE_X_A, READ_X, WRITE_X_A, READ_X, FINAL]),
            &mut tools,
            &specs,
            "sys",
            "task",
            &KernelConfig::default(),
        );
        // The first write was silently dropped; the model-visible
        // result still reported success.
        assert!(!outcome.write_attempts[0].applied);
        assert!(outcome.write_attempts[1].applied);
        assert_eq!(outcome.executed_tool_calls[0].result["ok"], true);
        assert_eq!(outcome.executed_tool_calls[0].result["value"], "A");
        assert!(
            !outcome.executed_tool_calls[0]
                .result
                .to_string()
                .contains("drop")
        );
        assert_eq!(outcome.final_state["x"], "A");
        assert_eq!(outcome.termination_reason, TerminationReason::Completed);
    }

    #[test]
    fn fresh_executors_do_not_share_state_or_fault_budget() {
        let o1 = run_with(
            scripted(&[WRITE_X_A, FINAL]),
            FaultMode::DropFirstWrite { key: "x".into() },
            &KernelConfig::default(),
        );
        assert_eq!(o1.final_state["x"], "EMPTY"); // first write dropped
        let o2 = run_with(
            scripted(&[WRITE_X_A, FINAL]),
            FaultMode::DropFirstWrite { key: "x".into() },
            &KernelConfig::default(),
        );
        assert_eq!(o2.final_state["x"], "EMPTY"); // its own one-shot drop
        assert!(!o2.write_attempts[0].applied);
    }

    #[test]
    fn build_record_uses_meta_and_oracle_is_separate() {
        let outcome = run_with(
            scripted(&[FINAL]),
            FaultMode::Reliable,
            &KernelConfig::default(),
        );
        let meta = RecordMeta {
            run_id: "exp0007-t-1-1".into(),
            sequence: 1,
            task: crate::oracle::TaskSpec {
                id: "T".into(),
                name: "t".into(),
                split: crate::oracle::TaskSplit::HeldOut,
                prompt: "task".into(),
                initial_state: json!({"x": "EMPTY", "y": "B"}),
                target_state: json!({"x": "A", "y": "B"}),
                fault: FaultMode::Reliable,
            },
            condition: "baseline".into(),
            repetition: 1,
            condition_position: 1,
            model: "m".into(),
            redacted_endpoint: "http://h/v1".into(),
            code_under_test_commit: "c".repeat(40),
            baseline_prompt_sha256: "b".repeat(64),
            repair_prompt_sha256: "r".repeat(64),
            regression_prompt_sha256: "g".repeat(64),
            task_suite_sha256: "s".repeat(64),
        };
        let mut record = build_record(outcome, &meta);
        assert_eq!(record.experiment_id, "0007");
        assert!(!record.oracle_success);
        assert!(record.oracle_failure_reasons.is_empty());
        record.apply_oracle(&meta.task);
        // x never reached A, so the oracle must fail this episode.
        assert!(!record.oracle_success);
        assert!(
            record
                .oracle_failure_reasons
                .iter()
                .any(|r| r.contains("x"))
        );
    }

    #[test]
    fn assistant_message_round_trips_with_multiple_calls() {
        let m = Message::assistant_tool_calls(
            vec![
                ToolCall::function("c1", "state_read", r#"{"key":"x"}"#),
                ToolCall::function("c2", "state_read", r#"{"key":"y"}"#),
            ],
            Some("note".into()),
        );
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["content"], "note");
        assert_eq!(v["tool_calls"].as_array().unwrap().len(), 2);
    }
}
