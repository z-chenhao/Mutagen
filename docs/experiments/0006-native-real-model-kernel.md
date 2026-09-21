# Experiment 0006 — Pi-Informed Rust-Native Real-Model Kernel

## Experiment ID

0006

## Transition from Synthetic to Model-in-the-Loop

Experiments 0001–0005 studied the evolution substrate with
**deterministic, hand-written fixtures**: replays, ablations, divergence
tags, stateful causality, and effect-metadata discrimination. Every
"candidate" behavior in those experiments was authored by the
experimenter, and every "environment" was a Rust struct.

Experiment 0006 makes the first methodological transition: a **real
Qwen model** drives an agent loop, and the trajectories under study are
**model-generated**, not fixture-authored. The experimental subject
changes from "can we *define* these concepts?" to "do these concepts
*measure* something real?" One line of prompt policy is mutated; the
model, kernel, tools, tasks, initial state, and limits are all fixed.

This experiment is **not** self-evolution. It does not evaluate
candidate quality, select, or promote anything.

## Research Question

Can a one-line tool-use policy mutation in the system prompt change the
external-interaction trajectory of a real Qwen-driven agent, measured
through a minimal Rust-owned kernel, while model, kernel, tools, tasks,
and initial state remain fixed?

## Hypothesis

Pre-registered, stated exactly (spec §49):

> A one-line tool-use policy mutation can change the
> external-interaction trajectory of a real Qwen-driven agent while
> model, Rust kernel, tools, tasks, and initial state remain fixed. In
> particular, the verification-after-write mutation is expected to
> increase same-key post-write reads on write-containing tasks and
> produce non-exact trajectories corresponding to structural divergence
> patterns studied in prior experiments.

**No quality hypothesis.** The experiment measures divergence, not
improvement.

## Pi Reference Study

Source: the current `earendil-works/pi` repository, inspected directly
(`packages/agent/src/{agent-loop,agent,types}.ts`,
`packages/coding-agent/src/core/{agent-session,sdk}.ts`,
`packages/coding-agent/src/core/tools/index.ts`,
`packages/coding-agent/docs/{rpc,sdk}.md`). Full table in
[`experiments/0006-native-model-kernel/PI_REFERENCE.md`](../../experiments/0006-native-model-kernel/PI_REFERENCE.md).

Summary of principles, not implementation:

- Pi separates a **small, provider-agnostic agent core** (turn loop,
  transcript state, closed typed event union, tool call/result
  lifecycle) from a **large product session layer** (persistence,
  extensions, compaction, skills, auth, UI, RPC).
- Interactive/TUI, print, RPC, and SDK modes all **observe** the same
  loop rather than each owning a copy of it.
- One turn = one assistant response plus its tool results; the loop
  ends when a response has no tool calls.
- Tools are a **model-visible description** (name, JSON schema) plus an
  **execution boundary** (`execute`), explicitly separated.

## Pi-Informed Design Decisions

| Principle | Pi reference observation | Mutagen decision | Reason |
| --- | --- | --- | --- |
| Small shared core | One `runAgentLoop` in `packages/agent` shared by all modes | A single `run_episode` function in `kernel.rs`; no mode-specific copies | The kernel must be reusable by future evaluators without presenting |
| Tool-first execution | `ToolDefinition` (description) / `AgentTool` (execution) split; `tools` array on the request | `ToolSpec` (JSON schema) + `ToolExecutor::execute` in `tools.rs` | The model only ever sees exactly two state tools |
| Presentation outside kernel | `AgentSession` and modes are outside the agent core; the loop knows no TUI/RPC | `kernel.rs` has no CLI, formatting, or filesystem awareness; `main.rs`/`experiment.rs` own them | Keeps the kernel a clean observation/evolution target |
| Structured events | Closed typed `AgentEvent` union with lifecycle start/end pairs | Closed 8-variant `TrajectoryEvent` enum; kernel appends events to its own vector | Trajectories come from the kernel, never reconstructed from logs |
| Explicit session/episode state | `AgentState { messages, model, tools, isStreaming, pendingToolCalls, errorMessage }` | `EpisodeState { messages, events, model_turn_count, tool_call_count }` | Minimal counters are all this experiment's loop needs |
| Extension/plugin features outside minimum kernel | Extensions, skills, commands, custom tools are session-layer add-ons | Absent entirely: no extension surface exists in the crate | 0006 must not accrete Pi's product machinery |

## Pi Features Deliberately Not Implemented

Session persistence, branching/forking, compaction, extension system,
skills, slash commands, TUI, RPC server, bash, filesystem editing, model
switching/cycling, and subagents are all **absent by construction**,
not by flag. The absence is deliberate: Experiment 0006 requires a
one-shot, fixed-model, two-tool episode with a hard turn budget. None
of those features is a dependency of that requirement, and including
any of them would (a) enlarge the surface whose behavior could confound
the mutation signal, and (b) violate the minimalism the experiment
tests. They remain *deferred* candidates for future experiments (see
`PI_REFERENCE.md` → Defer/Reject).

## Why Rust Owns the Kernel

- **Ownership.** The Rust process owns agent state, the turn loop,
  model invocation, tool-call parsing, dispatch, execution,
  observation insertion, termination, and trajectory recording. Qwen
  is a model backend; Pi is a design reference; neither is in the
  execution path.
- **Typed state.** Messages, tool calls, events, and termination
  reasons are closed Rust types; a malformed provider field cannot
  silently enter kernel state.
- **Capability boundary.** The model is *given* exactly two tools;
  there is no shell, filesystem, browser, or network tool to misuse.
  The only network I/O the process performs is the model HTTP request.
- **Trajectory authority.** The kernel emits the trajectory itself in
  execution order; the artifact is a serialization of kernel records,
  not a reconstruction from logs. This is what future evolution
  evaluation will need.
- **Future evolvability.** A single-language kernel makes the
  kernel/protocol/tools seams the natural *evolvable surfaces* that
  Mutagen exists to study.

No performance claim against Pi is made or implied; Pi was not run.

## Experimental Kernel

Implemented in `experiments/0006-native-model-kernel/src/kernel.rs`:

- Episode loop: `system + user` → repeat { model complete → final
  answer? stop : single tool call? execute → append tool result }.
- Limits: `max_model_turns = 12`, `max_tool_calls = 16`; exceeding
  either makes the episode ineligible with termination `turn_limit` /
  `tool_call_limit`.
- Parallel tool calls (>1 per response): **not executed, not
  reordered** — the episode is ineligible with
  `parallel_tool_calls_unsupported`.
- Model errors: HTTP transport/status failures → `http_error`;
  unparseable responses or arguments → `parse_error`; unknown tool →
  `unknown_tool`; schema-invalid arguments → `invalid_arguments`
  (state is not mutated).
- Reasoning-style response fields (`reasoning`, `reasoning_content`,
  `thinking`, `analysis`) are ignored and never persisted.
- Events recorded in order: `EpisodeStarted`, `ModelTurnStarted`,
  `ModelResponseReceived`, `ToolCallRequested`,
  `ToolExecutionStarted`, `ToolExecutionFinished`, `FinalAnswer`,
  `EpisodeFinished` / `EpisodeFailed`.

## Model Configuration

- Endpoint: OpenAI-compatible `POST {MUTAGEN_EXP_BASE_URL}/chat/completions`
  (env-provided; no fallback model, no retry).
- Model: `MUTAGEN_EXP_MODEL` (env-provided Qwen model id).
- `temperature = 0.2` fixed for all runs; `tool_choice = "auto"`.
- Non-streaming; single choice consumed; no async runtime (synchronous
  `ureq`).

## Tool Environment

Exactly two tools, fresh per episode, `x = "EMPTY"`, `y = "B"`:

- `state_read { key: "x"|"y" }` → `{ "key", "value" }`
- `state_write { key: "x"|"y", value: string }` →
  `{ "ok": true, "key", "value" }` (no implicit verification via read)

Both schemas enforce `required` keys and `additionalProperties: false`;
violations make the episode ineligible.

## Baseline Prompt

```
You are an agent operating a small deterministic external state.
Use the available state tools whenever you need to observe or change external state.
Never invent an external-state value that you have not observed or intentionally written.
Issue at most one tool call per turn.
Satisfy the user's task, then give a concise final answer.
```

## Candidate Mutation

Candidate = baseline plus exactly one appended line (verified
programmatically at run start; a violation aborts the run as a
configuration error):

```
After every successful state_write, call state_read on the same key before finalizing, even if the write reported success.
```

This mutation is **not assumed better**. It exists to induce a
potentially observable behavior difference (expected:
`write(x) → read(x) → final` where baseline may do `write(x) → final`).
The experiment measures divergence, not quality.

## Task Suite

All runs start from `x = EMPTY`, `y = "B"`. Wording is fixed for the
entire run and not adapted after observing results:

| ID | Name | Prompt |
| --- | --- | --- |
| T1 | read_x | Report the current value of x. |
| T2 | read_both | Report the current values of x and y. |
| T3 | write_x | Set x to A. Report when it is done. |
| T4 | conditional_write | If x is EMPTY, set x to A. Then report the resulting value of x. |
| T5 | unrelated_read_after_write | Set x to A, then report the current value of y. |
| T6 | two_writes | Set x to A and set y to B. Then report completion. |

T1/T2 are read-only controls (writes there are unexpected); T3–T6 are
write-containing.

## Repetition Protocol

3 repetitions × 6 tasks × 2 conditions = **36 episodes**.
Deterministic counterbalancing per task: rep1 baseline→candidate,
rep2 candidate→baseline, rep3 baseline→candidate. Everything except
the system prompt is identical between conditions (same model,
endpoint, temperature, kernel, tool schemas, tool implementations,
initial state, limits).

## Eligibility

An episode is eligible iff: every model call succeeded and parsed, a
final answer was produced, every tool call named a known tool with
schema-valid arguments, no response contained parallel calls, and
neither limit was exceeded. Failed episodes are recorded in full and
counted as ineligible.

## Trajectory Tags

Computed independently per eligible (task, repetition) pair; multiple
tags may apply: `exact`, `added_call`, `omitted_call`, `novel_call`,
`reordered`, `repeated_count_change`, `argument_change`,
`state_effect_order_change` (the relative W/R order on a key changes).
Canonical call identity = tool name + key-sorted JSON arguments.

## Verification Metric

For each successful `state_write(key, …)` in an eligible episode, the
write is *verified* if a later `state_read(key)` occurs before the
final answer. Aggregated per condition: `successful_writes`,
`verified_writes`, `verification_rate` (`null` when zero writes).

## Kernel Reliability

Counts of episodes by failure type (HTTP failures, parse failures,
unknown tools, invalid arguments, parallel calls, turn limit, tool-call
limit) plus eligible episodes. No aggregate score.

## Timing

Per episode: `total_wall_time_ms`, `model_wait_time_ms`,
`tool_execution_time_us`, and `kernel_overhead_estimate_ms` (a
**labeled-approximate** residual: total − model wait − tool
execution). No Pi-vs-Rust or any other benchmark claim.

## Pre-Registered Conclusion Rule

Fixed before execution (spec §50):

- **inconclusive** — if fewer than 2 read-only tasks (T1/T2) have ≥1
  eligible pair, **or** fewer than 2 distinct write-containing tasks
  (T3–T6) have ≥1 eligible pair.
- **supported** — eligibility sufficient **and** at least 2 distinct
  write-containing tasks show at least one non-exact
  baseline/candidate pair **and** candidate verification rate >
  baseline verification rate (a null rate compares as 0).
- **refuted** — eligibility sufficient but one or both behavioral
  conditions fail.

The rule is not modified after results.

## Results

_PENDING — real model execution recorded after the code-under-test
commit._

## Failed Episodes

_PENDING._

## Conclusion

_PENDING: supported / refuted / inconclusive._

## Relationship to Experiments 0002–0005

Structural correspondence only:

- **0002 (record ablation)** → which recorded fields matter: 0006's
  artifacts persist the canonical fields (sequence, turn,
  tool_call_id, tool_name, arguments, result) that 0002 studied
  synthetically.
- **0003 (trajectory divergence)** → the 8 tags, especially
  `exact` / `reordered` / `added` / `novel`, are the same tag family
  0003 defined; 0006 tests whether real model trajectories exhibit
  them under a prompt mutation.
- **0004 (stateful causality)** → `state_effect_order_change` and the
  verification metric are 0004's same-key W/R causality question on
  real trajectories.
- **0005 (effect metadata)** → 0006 uses no replay and no effect
  metadata; it only establishes that the *measurement substrate*
  (kernel + real model) works before any reuse question is revisited.

## Evidence We Can Claim

_PENDING after execution._

## Evidence We Cannot Claim

- No candidate-quality claim: divergence is not quality.
- No improvement claim: the mutation may be worse, equal, or better;
  this experiment does not measure that.
- No selection, no promotion, no rollback, no self-evolution claim.
- No production-kernel acceptance: the kernel is experiment-local and
  unvalidated for production.
- No general-agent generalization: one model, one endpoint, six
  trivial tasks, two tools.
- No Rust-vs-Pi performance claim: Pi was not run.

## Architectural Question Opened

Which experiment-local kernel boundaries, if any, have now accumulated
enough evidence to promote into `mutagen-runtime`?

(Left deliberately open; not answered here.)

## Next Research Question

Can this Rust-native kernel support fair repeated evaluation of
baseline and mutated candidates well enough to distinguish
improvement from regression?

(Experiment 0007 is not implemented here.)

## Execution Provenance

- Code-under-test commit: _PENDING_
- Model / endpoint (redacted): _PENDING_
- Artifacts: `artifacts/0006-trajectories.jsonl`,
  `artifacts/0006-summary.json`
