# Experiment 0007 — Candidate Evaluator Discrimination on Real Model Candidates

**Experiment ID**: 0007
**Status**: see the [Result](#result) section.
**Crate**: [`experiments/0007-candidate-evaluator/`](../../experiments/0007-candidate-evaluator/README.md) (standalone workspace; not part of the production workspace)
**Artifacts**: [`artifacts/0007-trajectories.jsonl`](artifacts/0007-trajectories.jsonl), [`artifacts/0007-summary.json`](artifacts/0007-summary.json)

## Prior Evidence

Experiment 0006 ([record](0006-native-real-model-kernel.md)) established,
narrowly:

1. a Mutagen-owned Rust-native kernel can drive a real Qwen tool-use loop;
2. a controlled prompt mutation produces real trajectory divergence
   (the mutation candidate verified every write; the baseline did not);
3. trajectory divergence alone does **not** establish improvement — 0006
   measured tags (added/omitted/novel calls), not task outcomes;
4. real models emit protocol behaviors the kernel must define semantics
   for (parallel tool calls were rejected as agent failures);
5. cost measurement must distinguish nominal prompt tokens, cached
   tokens, reasoning tokens, failed-run waste, and wall time.

0006 therefore left the key question open: *when the model behavior
changes, did the task actually get better or worse?* Experiment 0007
attacks exactly that question.

## Research Question

Can a deterministic task oracle, applied to repeated real-model
executions through the Rust-native Mutagen kernel, reliably distinguish a
controlled prompt mutation expected to improve task success from a
controlled prompt mutation expected to regress task success on held-out
tasks?

Secondary questions:

- What execution-cost differences accompany those quality changes?
- Can cache-aware token accounting be made reproducible from committed
  artifacts rather than inferred from server logs?

## Hypothesis

Pre-registered: *On held-out tasks, a generic verify-and-repair prompt
mutation will produce significantly more task-success wins than losses
relative to baseline under deterministic silent-write faults, while an
intentionally regressive skip-write mutation will produce significantly
more losses than wins relative to baseline. A deterministic
external-state oracle should classify the former as a quality
improvement and the latter as a quality regression.*

The hypothesis concerns **quality only**. Token cost is reported as an
independent dimension and is never mixed into the quality verdict
(no `quality − λ·cost` scalar exists anywhere in this experiment).

## Why an Objective State Oracle

The task is: manipulate a two-key external state to a registered target
state. Success is therefore *fully decidable* from the final external
state plus normal completion. This makes the experiment the minimal
possible setting in which "did the mutation improve the task?" has a
ground truth — the strongest possible test of an evaluation substrate,
because there is no room for a judge to rationalize.

Task success is deliberately strict:

- success = the episode reached **normal completion** (no agent or
  infrastructure failure) **and** the final external state exactly
  matches the registered target state;
- keys not intended to change must retain their expected values;
- if the model says "done" but the state is wrong → **FAIL**;
- if the model reaches the correct state but then fails the protocol
  (turn limit, …) → **FAIL**.

No LLM judge, no semantic grading model, no regex over persuasive
natural-language claims.

## Oracle Anti-Circularity

The oracle must be independent of condition name, system prompt,
candidate identity, mutation text, trajectory tags, cost, and the model's
final-answer wording. This is enforced **structurally, not by comment**:
the evaluator signature is

```rust
pub fn evaluate_task(task: &TaskSpec, outcome: &OracleInput) -> TaskEvaluation

pub struct OracleInput {
    termination_reason: String,
    initial_state: Value,
    final_state: Value,
}
```

`OracleInput` has no field — and therefore no code path — for a
condition, prompt, or candidate name. Unit tests verify that identical
(task, termination, initial, final) inputs yield identical verdicts and
that the oracle cannot receive experiment-side identity at all. The
kernel does not know the oracle exists: the runner applies it after
execution.

## Candidate Controls

Exactly three conditions; everything is held fixed (model,
temperature 0.2, kernel, tool schemas, tasks, fault schedules, initial
states, limits). Only the system prompt differs, and each candidate is
the baseline plus **exactly one registered line**, verified
programmatically before any run:

- **baseline** (`prompts/baseline.md`): the 0006 semantic baseline
  ("operate a small deterministic external state; at most one tool call
  per turn; …").
- **candidate_repair** (`prompts/repair.md`): baseline + one line
  instructing: after every `state_write`, read the same key; if it
  differs from what was intended, retry exactly once, then read again
  before finalizing. This is the **expected-improvement control** — a
  robustness mutation expected to recover from silent write faults.
- **candidate_regression** (`prompts/regression.md`): baseline + one
  line instructing: do **not** call `state_write`; report the change as
  completed anyway. This is an **intentionally bad positive control for
  regression detection**. It is *not* intended to resemble a naturally
  selected mutation; it tests that the evaluator can also discriminate
  in the other direction.

Neither candidate is a "discovered" mutation: both were authored
deliberately to bracket the evaluation.

## Calibration / Held-Out Split

Eight registered tasks: four **calibration** (C1–C4: reliable/silent
× direct/conditional write of `x`) and four **held-out** (H1–H4:
reliable/silent × direct write of `y` / replacement of `x`). The
candidate prompts are committed **before any real model run** and may not
be changed after calibration results; calibration evidence is reported
separately and never determines the experiment verdict. The final
verdict uses **H1–H4 only**.

## Deterministic Fault Model

Each task registers one of exactly two fault modes for its environment:

- **Reliable**: every valid `state_write` applies immediately.
- **DropFirstWrite(key)**: the *first* write to the registered key
  returns the **same model-visible success response**
  (`{"ok":true,"key":…,"value":…}`) but does **not** change external
  state; subsequent writes behave normally. This is a deterministic
  one-shot silent fault.

The fault is invisible to the model (no `applied:false`, no
`fault_injected` in the tool result); it can only be discovered by
reading state. The kernel maintains an experiment-only
`environment_write_attempts` audit log (`key`, `requested_value`,
`applied`, `fault_reason`) that is persisted in artifacts but **never
inserted into the LLM transcript**. The artifact verifier re-checks the
whole schedule for every episode: in Reliable mode no write is dropped;
in DropFirstWrite exactly the first write to the registered key is
dropped, at most one drop per episode, all other writes applied, and the
model-visible write result exposes nothing about the drop.

## Task Suite

Exactly eight tasks (4 calibration + 4 held-out), each with a registered
initial state (`x`, `y`), target state, and fault schedule:

| ID | Split | Initial | Fault | Target | Prompt |
|----|-------|---------|-------|--------|--------|
| C1 | calibration | x=EMPTY, y=B | Reliable | x=A, y=B | Set x to A… |
| C2 | calibration | x=EMPTY, y=B | DropFirstWrite(x) | x=A, y=B | Set x to A… |
| C3 | calibration | x=EMPTY, y=B | Reliable | x=A, y=B | If x is EMPTY, set x to A… |
| C4 | calibration | x=EMPTY, y=B | DropFirstWrite(x) | x=A, y=B | If x is EMPTY, set x to A… |
| H1 | held-out | x=EMPTY, y=B | Reliable | x=EMPTY, y=C | Set y to C… |
| H2 | held-out | x=EMPTY, y=B | DropFirstWrite(y) | x=EMPTY, y=C | Set y to C… |
| H3 | held-out | x=A, y=B | Reliable | x=B, y=B | Change x from A to B… |
| H4 | held-out | x=A, y=B | DropFirstWrite(x) | x=B, y=B | Change x from A to B… |

(All prompts end "Finish when the requested external state has been
achieved.") Each episode receives a **fresh** environment constructed
from the task's registered initial state; no state is reused between
episodes. Held-out tasks change the resource key, initial value, and
target value while preserving the structural problem, so the verdict
cannot be a memorization of the calibration cases.

## Rust Kernel

Carried over from 0006 (deliberately duplicated; not shared):
Rust-owned turn loop, typed messages, exactly two model-visible tools
(`state_read`, `state_write`), fresh per-episode external state, hard
limits `max_model_turns = 12`, `max_tool_calls = 16` (identical across
all conditions; a limit hit is an agent-caused task failure), no unsafe,
no async runtime, no external agent harness. Pi remains a design
reference only. 0007 additions: per-request usage recording and separate
requested/executed tool-call records.

## Parallel Tool Policy

One tool call per assistant turn. A response containing multiple calls
is an **agent failure**: `termination_reason =
parallel_tool_calls_unsupported`, `task_success = false`, *none* of the
calls executed, and all of them recorded in `requested_tool_calls`.
Parallel tool calls are **not** infrastructure failures, and no parallel
execution is implemented (0007 validates the evaluator, not parallel
semantics).

## Run Ordering / Cache Counterbalancing

Because prompt caching and server warm state can advantage whichever
condition runs later, all six permutations of the three conditions are
used exactly once per task:

| rep | order |
|-----|-------|
| 1 | baseline → repair → regression |
| 2 | repair → regression → baseline |
| 3 | regression → baseline → repair |
| 4 | baseline → regression → repair |
| 5 | regression → repair → baseline |
| 6 | repair → baseline → regression |

Each condition therefore runs 2× first, 2× second, 2× third per task,
and every pairwise ordering is balanced. This does not make cache
effects disappear — it reduces systematic ordering bias; the remaining
effect is measured directly via per-position cache accounting (below).

## Usage / Cache Observability

For **every** model request the raw artifact persists: `turn`,
`prompt_tokens`, `completion_tokens`, `total_tokens`,
`cached_prompt_tokens` (from `usage.prompt_tokens_details.cached_tokens`),
`reasoning_tokens` (from
`usage.completion_tokens_details.reasoning_tokens`), `finish_reason`, and
the computed `uncached_prompt_tokens = prompt − cached`. Fields the
provider does not report are `null` — **never zero by assumption**.
`cached > prompt` is a usage accounting error that fails verification
(not silently saturated). Reasoning *counts* are persisted; reasoning
*content* (`reasoning_content`, `thinking`, `analysis`, …) is ignored by
construction and a verifier field-scan fails any leak.

## Pairwise Quality Evaluation

Candidate vs baseline is compared **independently** (repair vs baseline,
regression vs baseline), on **held-out tasks only**, paired by
`(task_id, repetition)`:

- candidate success, baseline failure → **WIN**
- candidate failure, baseline success → **LOSS**
- same status → **TIE**
- a pair where either episode suffered an **infrastructure** failure is
  excluded from the sign test and recorded separately. **Agent**
  failures (parallel calls, unknown tool, invalid arguments, turn limit,
  tool-call limit, no final answer) are ordinary task failures and stay
  in the comparison.

## Exact Sign Test

On non-tied pairs, `n = wins + losses`, `k = min(wins, losses)`:

```
p = min(1, 2 · Σ_{i=0..k} C(n, i) · 0.5ⁿ)
```

implemented with the standard library only (no statistics crate); known
values are unit-tested.

## Pre-Registered Classification

α = 0.05:

- `quality_improvement`: wins > losses AND p ≤ 0.05
- `quality_regression`: losses > wins AND p ≤ 0.05
- `quality_inconclusive`: otherwise (never "neutral")

## Pre-Registered Experiment Conclusion

- **inconclusive** if: artifact completeness fails (not exactly the
  registered 8 × 3 × 6 = 144 design, or the fault audit fails); OR the
  infrastructure failure rate exceeds **10%** of all episodes; OR either
  held-out comparison has fewer than **12** valid pairs.
- **supported** (if the above hold) iff: held-out repair =
  `quality_improvement` AND held-out regression = `quality_regression`.
- **refuted** otherwise.

No post-hoc alterations are permitted.

## Expected Qualitative Pattern (hypothesis, not a prediction)

- **Reliable tasks**: baseline likely passes; repair likely passes but
  costs extra reads; regression expected to fail.
- **DropFirstWrite tasks**: baseline may fail unless it independently
  verifies/retries; repair expected to recover; regression expected to
  fail.

None of this is hardcoded into the runner or verifier: outcomes come
from the model and the oracle.

---

# Result

## Execution Integrity

- Episodes executed: — (registered design: 8 tasks × 3 conditions × 6 repetitions = 144)
- Model: — ; redacted endpoint: —
- Temperature: 0.2; `tool_choice`: auto; non-streaming
- Code-under-test commit: —
- Artifacts: complete? —
- Raw artifact SHA-256: —

## Infrastructure Reliability

| Failure type | Count |
|---|---|
| http_error | — |
| malformed_response | — |

Infrastructure failure rate: — (threshold: > 10% → inconclusive)

## Fault Injection Audit

Reliable episodes: — / 96 passed. DropFirstWrite episodes: — / 48 passed.
Model-visible leak checks: passed.

## Calibration Results (C1–C4, evidence only — not determinative)

| Condition | Success | Failure | Rate |
|---|---|---|---|
| baseline | — | — | — |
| candidate_repair | — | — | — |
| candidate_regression | — | — | — |

## Held-Out Oracle Success (H1–H4)

| Condition | Success | Failure | Rate |
|---|---|---|---|
| baseline | — | — | — |
| candidate_repair | — | — | — |
| candidate_regression | — | — | — |

## Held-Out Pairwise Results

**candidate_repair vs baseline**

| Wins | Losses | Ties | Non-ties | Exact p | Classification |
|---|---|---|---|---|---|
| — | — | — | — | — | — |

**candidate_regression vs baseline**

| Wins | Losses | Ties | Non-ties | Exact p | Classification |
|---|---|---|---|---|---|
| — | — | — | — | — | — |

## Cost / Usage

| Condition | Requests | Nominal Prompt | Cached Prompt | Uncached Prompt | Completion | Reasoning | Wall (s) |
|---|---|---|---|---|---|---|---|
| baseline (held-out) | — | — | — | — | — | — | — |
| candidate_repair (held-out) | — | — | — | — | — | — | — |
| candidate_regression (held-out) | — | — | — | — | — | — | — |

## Cache Accounting (fairness)

Provider-reported cache detail observable: —

| Condition | Position | Episodes | Nominal | Cached | Uncached | Hit ratio |
|---|---|---|---|---|---|---|
| — | — | — | — | — | — | — |

## Failure Resource Consumption

Tokens / wall time / model requests spent on oracle-failed episodes, per
condition: —

## Conclusion

— (`supported` / `refuted` / `inconclusive`)

## Evidence We Can Claim

(if supported): In the tested deterministic state-manipulation
environment, the Rust-native Mutagen evaluation substrate used an
independent final-state oracle and repeated held-out real-model runs to
distinguish a controlled robustness-improving prompt mutation from an
intentionally regressive prompt mutation.

## Evidence We Cannot Claim

- no self-generated mutations (both candidates were authored controls)
- no production promotion of kernel or evaluator
- no general-agent generalization (two keys, two tools, one state shape)
- no natural-distribution benchmark
- no claim that a final-state oracle covers subjective/qualitative tasks
- no claim that the sign-test rule is a final production selection policy
- no monetary-cost optimization (no pricing exists for the local server)
- no architecture promotion: the 0007 kernel/evaluator stays in the
  experiment crate; promotion requires a separate ADR/PR

## Artifact Verification

`verify-artifacts` result: —

## Production Changes

None: `crates/*`, root `Cargo.toml`, and root `Cargo.lock` are untouched;
the experiment lives in a standalone workspace with its own `Cargo.lock`.
