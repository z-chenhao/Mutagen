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

Implementation semantics (as executed by the code-under-test commit):
`argument_change` fires when the multiset of canonical arguments for
some tool name differs between the two runs, and
`state_effect_order_change` fires when the ordered write/read pattern
on some key differs. Both are therefore *difference* tags that also
fire when the mutation adds a call of a tool (or a read of a key) the
other run did not perform; they intentionally overlap with
`added_call` / `novel_call` and are reported as independent
observations.

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

Executed with 3 repetitions (36 episodes) against a local Qwen model
(`incoai/Qwen3.8-27B-Splash` served at an OpenAI-compatible endpoint,
`temperature = 0.2`). Code-under-test commit
`fdca01e1a21a958ceb318f44231ad60c91f141e4`. Raw evidence:
[`artifacts/0006-trajectories.jsonl`](artifacts/0006-trajectories.jsonl) (36
records), [`artifacts/0006-summary.json`](artifacts/0006-summary.json).

### Headline numbers

| Measure | Baseline | Candidate |
| --- | --- | --- |
| Eligible episodes | 14 / 18 | 13 / 18 |
| Total tool calls (eligible) | 22 | 31 |
| Successful writes | 13 | 11 |
| Verified writes (same-key read-back after write) | 0 | 11 |
| **Verification rate** | **0.0** | **1.0** |

### Trajectory divergence (12 eligible pairs)

| Tag | Count | Task IDs |
| --- | --- | --- |
| exact | 3 | T1 |
| added_call | 9 | T3, T4, T5 |
| argument_change | 9 | T3, T4, T5 |
| novel_call | 6 | T3, T5 |
| repeated_count_change | 3 | T4 |
| state_effect_order_change | 9 | T3, T4, T5 |
| reordered | 0 | — |
| omitted_call | 0 | — |

Non-exact pairs: **9 of 12** — all three write tasks that produced
eligible pairs (T3, T4, T5) were non-exact in all 3 of their
repetitions each (9/9). The single read-only task with eligible pairs
(T1) was exact in all 3.

Canonical per-repetition trajectories (deterministic across all 3
repetitions on every eligible episode):

| Task | Baseline | Candidate |
| --- | --- | --- |
| T1 | `read(x)` | `read(x)` |
| T3 | `write(x=A)` | `write(x=A)`, `read(x)` |
| T4 | `read(x)`, `write(x=A)` | `read(x)`, `write(x=A)`, `read(x)` |
| T5 | `write(x=A)`, `read(y)` | `write(x=A)`, `read(x)`, `read(y)` |

The one-line mutation produced exactly its predicted behavioral
signature on every eligible write episode: a same-key `state_read`
inserted after each successful `state_write` (e.g. candidate T3 final
answer: *“Done. `x` is now set to `A` (write succeeded and verified by
read-back)”*). Baseline final answers never mention verification and
perform no read-back.

Note on tag semantics: `argument_change` and
`state_effect_order_change` are implemented as *multiset/pattern
difference* tags; they therefore also fire when the mutation adds a
call of a tool (or a read of a key) that the baseline did not perform.
They overlap with `added_call`/`novel_call` by construction; all tags
are independent observations, and the non-overlapping tags
(`added_call`, `novel_call`, `repeated_count_change`) already carry the
same signal.

### Failed episodes

9 of 36 episodes ineligible — **all** with termination
`parallel_tool_calls_unsupported`:

| Task | Condition | Reps failed / 3 | Detail |
| --- | --- | --- | --- |
| T2 (read_both) | baseline | 3 | model issued `read(x)` + `read(y)` in one response, turn 1 |
| T2 (read_both) | candidate | 3 | same |
| T6 (two_writes) | baseline | 1 / 3 | parallel `write(x=A)` + `write(y=B)` in turn 1 |
| T6 (two_writes) | candidate | 2 / 3 | parallel writes in turn 3 (after two sequential single-call turns) |

No HTTP failures, parse failures, unknown tools, invalid arguments, or
limit hits occurred. The read-only control recorded **0** unexpected
`state_write` calls.

### Kernel reliability

| Failure type | Count |
| --- | --- |
| HTTP failures | 0 |
| Response parse failures | 0 |
| Unknown tools | 0 |
| Invalid arguments | 0 |
| Parallel tool calls | 9 |
| Turn limit | 0 |
| Tool-call limit | 0 |
| Eligible episodes | 27 / 36 |

### Timing (all 36 episodes, approximate)

| Measure | Value |
| --- | --- |
| Total wall time | 162.2 s |
| Model wait time | 162.2 s |
| Tool execution | 113 µs |
| Kernel overhead estimate | ≈ 0 ms (labeled approximate) |
| Tokens (36 episodes) | 52,731 prompt / 6,121 completion |

The kernel is an in-memory observer of a local model; the timing
table shows the run was dominated by model inference. No benchmark
claim is made.

## Conclusion

**inconclusive** — exactly as the pre-registered rule dictates.

Eligibility sufficiency failed on the first gate: fewer than 2
read-only tasks have ≥1 eligible pair. T1 qualified (3/3 pairs); T2
(`read_both`) qualified in **zero** repetitions because the model
answered every T2 prompt by issuing both reads as *parallel* tool
calls in a single response, which the one-call-per-turn kernel policy
correctly refuses (6/6 T2 episodes ineligible). Write-task
sufficiency was met (T3, T4, T5 each 3/3 eligible pairs; T6 0/3 for
the same parallel-call reason).

This is **not** a failure of the mutation. Both behavioral conditions
of the rule were in fact strongly met: 3 distinct write-containing
tasks show 9/9 non-exact pairs, and the candidate verification rate
(1.0) exceeds the baseline (0.0). The experiment is inconclusive
solely because a read-only *control* could not produce eligible pairs
against a real model that parallelizes reads.

The methodologically interesting finding is the gate itself: a real
Qwen model, prompted with “at most one tool call per turn,” still
responds to multi-value tasks (read *both*; write *two* keys) with
parallel calls — a measured interaction between the tool-use policy and
the model's native parallel-call behavior that a synthetic fixture can
never surface. Whether such responses should be rejected, serialized,
or treated as a separate protocol feature is a genuine open question
for future experiments; 0006's kernel chose (by pre-registration) to
reject and record.

## Relationship to Experiments 0002–0005

Structural correspondence only — and the correspondence held on real
data:

- **0002 (record ablation)** → the artifact fields persist exactly the
  canonical record 0002 studied; on real trajectories the ordering
  field is load-bearing (the mutation *inserts* a read between write
  and final answer).
- **0003 (trajectory divergence)** → the tag family was designed on
  independent read-only fixtures; on real data `exact` / `added_call`
  / `novel_call` / `state_effect_order_change` all fired as designed,
  and `reordered` / `omitted_call` did not — the mutation adds, it
  does not permute.
- **0004 (stateful causality)** → the W/R same-key pattern is now
  observed to *change* under a policy mutation; the verification
  metric (post-write same-key read) is 0004's causality question
  measured on live behavior.
- **0005 (effect metadata)** → no reuse is attempted here; 0006 only
  establishes that the measurement substrate (kernel + real model +
  canonical records) works, which 0005's reuse guards would need as
  input.

## Evidence We Can Claim

1. A Rust-owned synchronous kernel can drive a real Qwen model through
   a complete tool-using episode, with the kernel (not the model, not
   any external harness) owning state, dispatch, termination, and
   authoritative trajectory recording.
2. A one-line system-prompt mutation changed the external-interaction
   trajectory of a real Qwen-driven agent: on all 9 eligible
   write-task pairs the candidate trajectories are non-exact, and the
   change is uniform (same-key read-back inserted after every
   successful write) across all 3 repetitions of each task.
3. The verification metric moves 0.0 → 1.0 (13/0 baseline writes
   verified vs 11/11 candidate writes verified); the candidate's own
   final answers reference the read-back explicitly.
4. The artifact pipeline (canonical fields, hashes, redaction, verifier
   with 12 structural checks, summary recomputation) is stable on real
   model output, including the model's `reasoning_content` field
   (present in raw responses; absent from every artifact).
5. A real Qwen model issues parallel tool calls on multi-value tasks
   even under an explicit one-call-per-turn instruction (9/36
   episodes; 6/6 on `read_both`). This is a measured property of the
   model/kernel boundary.

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

- Code-under-test commit: `fdca01e1a21a958ceb318f44231ad60c91f141e4`
  (`fix: set Content-Type application/json on model request`; the
  initial kernel commit is `c59e01956113bfa1150ea12772bec22c9ec3adb5`).
- Model: `incoai/Qwen3.8-27B-Splash`; endpoint (redacted):
  `http://127.0.0.1:8000/v1`; temperature 0.2; no API key required
  (local server); no reasoning fields or credentials appear in any
  artifact.
- Prompt hashes (SHA-256): baseline
  `cba4b0f156b4cd82ebb65fb4e01757f3f15daf4a804be4317c9529fcd48b8def`,
  candidate `798059e34d71aff90420ad687c5990e6707fbc48e6fb26c44dcc54bdf2b0a613`;
  task suite `4bc8852e8e2de2a2ddb2eab3db9c6d67f9af783df8ee4c7ff398047618caeb1b`.
- Artifacts: `artifacts/0006-trajectories.jsonl` (36 records),
  `artifacts/0006-summary.json`; verifier (`verify-artifacts`): 12/12
  PASS.
