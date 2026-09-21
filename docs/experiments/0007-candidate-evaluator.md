# Experiment 0007 — Candidate Evaluator Discrimination on Real Model Candidates

**Experiment ID**: 0007
**Status**: complete — executed on a real model; pre-registered verdict: **refuted** (see [Result](#result)).
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
in DropFirstWrite, **if** the model writes the registered key, the first
such write is dropped and later writes to that key apply, with at most
one drop per episode; writes to other keys always apply; and the
model-visible write result exposes nothing about the drop.

**Registered fault mode ≠ triggered fault.** An episode may register
`DropFirstWrite(key)` while the model never calls
`state_write(key, …)`; then the registered fault simply never triggers
(zero dropped writes) and the episode still conforms to the schedule.
The derived summary therefore reports *registered* and *triggered* as
separate machine-derived counts, and no quantity in this experiment may
collapse the two.

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

- Episodes executed: **144** (8 tasks × 3 conditions × 6 repetitions; 100% of design)
- Model: incoai/Qwen3.8-27B-Splash; redacted endpoint: `http://127.0.0.1:8000/v1`
- Temperature: 0.2; `tool_choice`: auto; non-streaming
- Code-under-test commit: `0ce6de36319e1f083ae598d7046e2d7c5f317c4f` (the pre-registered code; the model run used exactly this build)
- Artifacts: complete (144/144 records, all fields present)

**Artifact provenance** (three distinct provenance points):

- **Raw trajectories** (`0007-trajectories.jsonl`) SHA-256:
  `a368db71ead6f7dbf7859c3c3df981b9c7452cd0dda82d49540c6daa042318d3` —
  the immutable primary evidence; byte-identical throughout all post-run
  audits.
- **Original run-time derived summary** (`0007-summary.json` as written at
  run completion) SHA-256:
  `06d03de88fefa7589205c1f5be2ff315098760fe82bd1ec652855e7f6c0f6005` —
  historical record only.
- **Current committed derived summary** (offline-regenerated after the
  fault-accounting audit added its machine-derived fields) SHA-256:
  `c143d202a2ca47bb1bb1ad29770ab40a27eb05548171171348be03909b35d2f1`.

`0007-summary.json` is a deterministic *derived* artifact: it was
regenerated offline strictly from the immutable raw trajectories and the
committed task registry after the fault-accounting fields were added. No
model execution was repeated, and the raw trajectory file was never
modified.

## Infrastructure Reliability

| Failure type | Count |
|---|---|
| http_error | 0 |
| malformed_response | 0 |

Infrastructure failure rate: **0.0%** (threshold: > 10% → inconclusive). All 144 episodes terminated normally (`completed`); agent-caused failures: 0 (no parallel tool calls, no unknown tools, no limit hits occurred).

## Fault Injection Audit

Registered fault modes (machine-derived from the 144 records; 4 tasks
× 18 episodes per mode): **72 Reliable, 72 DropFirstWrite**. All 144
episodes passed the per-record schedule audit (every write returned the
model-visible success response; zero leaks). Verdict: **passed**.

Machine-derived trigger accounting (from the immutable audit logs —
registered mode and triggered fault are distinct quantities):

| Quantity | Episodes | Notes |
|---|---|---|
| Registered Reliable | 72 | C1, C3, H1, H3 × 3 conditions × 6 reps |
| Registered DropFirstWrite | 72 | C2, C4, H2, H4 × 3 conditions × 6 reps |
| DropFirstWrite episodes with a target-key write | 58 | fault could trigger |
| DropFirstWrite episodes without a target-key write | 14 | fault never triggered; schedule still conforms |
| Triggered silent-drop episodes | 58 | one-shot drop fired on the first target-key write |
| Total silently-dropped write attempts | 58 | one per triggered episode (one-shot) |

The 14 untriggered episodes are all `candidate_regression` episodes (the
regression prompt forbade `state_write`; the model complied 24 − 10 = 14
times across the drop-mode tasks, and even where it wrote another key,
the registered fault key was never touched). Every triggered drop was to
the registered key only, returned the model-visible success response, and
occurred exactly once.

## Calibration Results (C1–C4, evidence only — not determinative)

| Condition | Success | Failure | Rate |
|---|---|---|---|
| baseline | 15 | 9 | 62.5% |
| candidate_repair | 24 | 0 | 100% |
| candidate_regression | 4 | 20 | 16.7% |

Per task (6 reps each). Calibration: baseline `C1 6 · C2 3 · C3 6 · C4 0`;
repair `6 · 6 · 6 · 6`; regression `C1 1 · C2 0 · C3 3 · C4 0`.

Fault-specific split (keep the two quantities separate):

- baseline on calibration **Reliable** tasks (C1, C3): **12/12**
- baseline on calibration **DropFirstWrite** tasks (C2, C4): **3/12**

The baseline's spontaneous read-back recovered 3 of 6 one-shot drops on
the *direct* write task (C2) but none of 6 on the *conditional* drop task
(C4); the repair mutation converted both to 6/6.

## Held-Out Oracle Success (H1–H4)

| Condition | Success | Failure | Rate |
|---|---|---|---|
| baseline | 23 | 1 | 95.8% |
| candidate_repair | 24 | 0 | 100% |
| candidate_regression | 4 | 20 | 16.7% |

Per task (6 reps each): baseline `H1 6 · H2 6 · H3 6 · H4 5` (single failure:
H4 rep 6 — the model's one write was silently dropped and it finalized with
`x` still `A`); repair `6 · 6 · 6 · 6`; regression `H1 3 · H2 1 · H3 0 ·
H4 0`.

Fault-specific split — **keep the two quantities separate**:

- baseline held-out success **overall** (H1–H4): **23/24**
- baseline held-out success on **DropFirstWrite tasks only** (H2, H4):
  **11/12** (H2 6/6, H4 5/6)
- baseline held-out success on **Reliable tasks only** (H1, H3): **12/12**

The model spontaneously verified/recovered on 11 of the 12 held-out
silent-fault baseline episodes — the hypothesis assumed a much lower rate.
(Only 12 held-out baseline episodes experienced a silent-fault task; the
23/24 figure additionally includes the 12 reliable-task episodes, which
are not evidence of drop recovery.)

## Held-Out Pairwise Results

**candidate_repair vs baseline**

| Wins | Losses | Ties | Non-ties | Exact p | Classification |
|---|---|---|---|---|---|
| 1 | 0 | 23 | 1 | 1.0 | **quality_inconclusive** |

The single win: H4 rep 6 (baseline failed on the silent drop; repair
recovered via read-back/retry). The baseline failed only 1 of 24 held-out
episodes overall — and only 1 of the 12 held-out DropFirstWrite episodes
(H4 rep 6) — so the improvement mutation had exactly one discordant pair
available out of 24 valid pairs.

**candidate_regression vs baseline**

| Wins | Losses | Ties | Non-ties | Exact p | Classification |
|---|---|---|---|---|---|
| 0 | 19 | 5 | 19 | 3.814697265625e-06 | **quality_regression** |

Of the 5 ties, 4 are episodes where the model *disobeyed* the regression
instruction and wrote the state anyway (H1: 3, H2: 1 — mirroring baseline
successes); the 5th is H4 rep 6, where *both* baseline and candidate
failed (baseline's dropped write unrecovered, candidate never wrote). The
19 losses all ended with the requested state unmodified.

## Cost / Usage

| Condition | Requests | Nominal Prompt | Cached Prompt | Uncached Prompt | Completion | Reasoning | Wall (s) |
|---|---|---|---|---|---|---|---|
| baseline (held-out) | 103 | 62,329 | 59,296 | 3,033 | 6,866 | 3,620 | 169.6 |
| candidate_repair (held-out) | 108 | 71,356 | 68,576 | 2,780 | 7,019 | 3,533 | 157.2 |
| candidate_regression (held-out) | 59 | 34,138 | 32,768 | 1,370 | 40,860 | 39,052 | 869.7 |

All 270 held-out model requests reported `usage`; 100% of them reported
`cached_prompt_tokens`; 100% reported `reasoning_tokens`. The regression
candidate burned ~5.9× the completion tokens of baseline (mostly reasoning
tokens) while succeeding less often — a "cheap-looking, expensive-in-
attention" profile the token-only accounting of a nominal prompt would miss.

## Cache Accounting (fairness)

Provider-reported cache detail observable: **yes** (100% of requests).
Overall held-out hit ratio: 95.7% (160,640 cached of 167,823 nominal).

| Condition | Position | Episodes | Nominal | Cached | Uncached | Hit ratio |
|---|---|---|---|---|---|---|
| baseline | 1 | 16 | 34,210 | 31,776 | 2,434 | 92.9% |
| baseline | 2 | 16 | 33,866 | 32,640 | 1,226 | 96.4% |
| baseline | 3 | 16 | 36,135 | 34,624 | 1,511 | 95.8% |
| candidate_repair | 1 | 16 | 47,644 | 45,856 | 1,788 | 96.2% |
| candidate_repair | 2 | 16 | 47,674 | 44,480 | 3,194 | 93.3% |
| candidate_repair | 3 | 16 | 47,636 | 45,504 | 2,132 | 95.5% |
| candidate_regression | 1 | 16 | 22,367 | 21,440 | 927 | 95.9% |
| candidate_regression | 2 | 16 | 24,326 | 23,520 | 806 | 96.7% |
| candidate_regression | 3 | 16 | 22,682 | 21,600 | 1,082 | 95.2% |

The six-permutation design **balanced condition position exactly** (each
condition ran 2× first, 2× second, 2× third per task; every condition ran
2 episodes in each position overall per split cell). Provider-reported
cache-hit ratios ranged from 92.9% to 96.7% across the 18
condition-position cells, with no obvious monotonic position pattern.
This does **not** prove cache effects were absent: position balance
removes *systematic* ordering bias only, and server-side cache state
(warm/evicted between runs) is not directly observable beyond the
reported ratios.

## Failure Resource Consumption

Tokens / wall time / model requests spent on oracle-failed episodes, per
condition:

| Condition | Failed episodes | Requests | Nominal Prompt | Completion | Reasoning | Wall (s) |
|---|---|---|---|---|---|---|
| baseline | 10 | 27 | 14,832 | 1,463 | 694 | 30.7 |
| candidate_repair | 0 | 0 | — | — | — | 0 |
| candidate_regression | 40 | 90 | 51,024 | 66,475 | 63,628 | 1,448.8 |

Failed regression episodes are *expensive*: the model deliberated for
long (reasoning-dominated) before declaring completion over an unmodified
state. A selection policy that ignored failed-run consumption would
misprice the regression candidate.

## Conclusion

**refuted** (per the pre-registered rule, applied to held-out tasks):

- held-out `candidate_repair` = **quality_inconclusive** (expected
  `quality_improvement`) — 1 win / 0 losses / 23 ties, p = 1.0
- held-out `candidate_regression` = **quality_regression** (expected
  `quality_regression`) — 0 wins / 19 losses / 5 ties, p = 3.8e-06

Why refuted, faithfully: baseline succeeded on **23/24** held-out
episodes overall and on **11/12** held-out DropFirstWrite episodes
specifically (H2 6/6, H4 5/6). This produced a severe ceiling effect for
the repair candidate: only one held-out baseline failure (H4 rep 6)
remained available to become a repair win, so the comparison held 24
valid pairs but a single non-tied pair — one win cannot reach α = 0.05
under the exact sign test (p = 1.0). The evaluator discriminated the
**regression** direction with high confidence, but the **improvement**
arm lacked statistical headroom.

On calibration the picture was different (baseline failed 9/24;
baseline on calibration fault tasks 3/12 vs 12/12 on reliable tasks;
repair: 9 wins, p = 0.0039 → `quality_improvement`; regression: 11
losses, p = 0.00098 → `quality_regression`). The held-out task family
therefore behaved very differently from calibration **under the same
registered one-shot fault class** (calibration fault-task baseline 3/12
vs held-out fault-task baseline 11/12) — the baseline's spontaneous
verification policy did not transfer uniformly across the two task
families.

The observed refutation is **consistent with insufficient held-out
headroom under the registered one-shot fault/task combination**. The
oracle, kernel, and registered statistical calculation all behaved
consistently with their specifications (144/144 audited, 0 infrastructure
failures, exact sign test applied as registered), but the experiment does
**not** uniquely identify fault strength as the sole cause: task-family
difficulty, the baseline's spontaneous policy, and fault × task
interaction may all contribute. Pre-registration's purpose is to report
this result without reinterpretation, and this record does exactly that
— with the diagnosis stated at the strength the evidence supports, no
more.

## Evidence We Can Claim

(result is **refuted**, so the claim is negative + substrate-level):

1. The Rust-native evaluation substrate executed the full pre-registered
design on a real model: 144/144 episodes, 0 infrastructure failures,
0 agent protocol failures, complete and verifiable artifacts.
2. A deterministic final-state oracle graded all 144 episodes;
100% of model requests reported provider usage including cache and
reasoning token details; the fault-injection audit passed for all 144
episodes.
3. The evaluator **discriminated the intentionally regressive mutation**
with high confidence (19/24 held-out losses, exact two-sided sign-test
p = 3.8e-06 ≪ 0.05).
4. The evaluator **could not** discriminate the controlled robustness-
improving mutation on held-out tasks: baseline succeeded on 11/12
held-out DropFirstWrite episodes (23/24 overall), leaving 1 win / 0
losses / 23 ties (p = 1.0) — hence the pre-registered **refuted**
verdict.
5. Cache-aware, per-position, per-condition cost accounting was
reproducible from committed artifacts; hit ratios were flat across
schedule positions (92.9%–96.7% across condition-position cells, no
obvious monotonic position pattern; this bounds but does not prove the
absence of cache effects).
6. Failed-episode resource consumption was measured per condition and
diverged strongly (regression failures are reasoning-heavy and slow),
demonstrating that a cost dimension distinct from quality is observable
and material.

We can **not** claim (because the verdict is refuted): that this
evaluator can discriminate this particular improvement mutation on
this model for this task family. The result is consistent with
insufficient held-out headroom under the registered fault/task
combination, but the experiment does not uniquely prove that fault
strength alone is the cause; the components (oracle, kernel, statistics)
behaved as specified, and no stronger causal localization is made.

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

## 0007 Methodology Limitation

The pre-registered sufficiency gate required **≥ 12 valid pairs** per
held-out comparison but did **not** require a minimum number of
*non-tied* (informative) pairs. The repair comparison satisfied the
gate with 24 valid pairs while containing only **1 non-tied pair**:
the experiment was *formally sufficient* under the registered rule yet
had almost no statistical headroom for the improvement comparison.
**Valid pair count ≠ informative pair count.** This is recorded as a
limitation of the 0007 methodology. It is *not* a reason to change the
0007 verdict, and the 0007 decision rule is left exactly as registered.

**Registered follow-up (0008 requirement).** 0008 must **pre-register a
headroom/information criterion before final evaluation** so that a
directionally-correct-but-informationally-empty comparison cannot be
disguised by a pair-count gate. Criteria to investigate later include:

- a minimum baseline failure rate on the stress subset,
- a minimum expected discordant-pair count,
- a minimum observed non-tied pair count required before a directional
  (improvement/regression) classification may be made,
- a separate pre-finalization difficulty-calibration stage.

No threshold is chosen here: choosing one is itself an experimental
design question for 0008. The 0007 record stands as: substrate
verified; regression-direction discrimination demonstrated; the specific
improvement mutation left without demonstrable headroom under the
registered design.

## Artifact Verification

`verify-artifacts` result: **PASSED** (144 records; design completeness,
six-permutation balance, fault-injection audit, oracle recomputation,
usage sanity + reasoning-content redaction, and summary deep-equality all
hold against the on-disk artifacts).

**Post-hoc correction (analysis-only, recorded per the 0006 precedent).**
After the run, the summary deep-equality check initially failed on four
`cache_hit_ratio` fields by exactly **1 ulp**. Root cause: serde_json
1.0.151's decimal→f64 parser is off by 1 ulp on certain inputs (its fast
path); the on-disk summary text and the in-memory recomputation both
derive from identical integer sums and identical serialized text, so no
measurement changed. The verifier now canonicalizes the recomputed
summary through the same serialize-then-parse pipeline as the on-disk
file before deep comparison. Caveat: because of the same 1-ulp parse
inaccuracy, a tamper that nudges a decimal's final digit by one in an
f64 *ratio* field is *not* guaranteed to be detected by the deep
equality check; all decision-relevant counts (integers, strings,
booleans) parse exactly and remain fully tamper-evident, and the
tamper regression tests exercise the decision fields. Equivalently: all
decision-relevant statistical counts and classifications (wins/losses/
ties, sign-test inputs, pair counts, fault counts) are recomputed from
raw integer/bool/string evidence; the affected `f64` fields (e.g.
`cache_hit_ratio`) are *descriptive, not verdict inputs*. The raw
trajectory artifact remained byte-identical to run completion throughout
(SHA-256 in [Execution Integrity](#execution-integrity)); this correction
landed in a post-run commit that changed only the verifier, and the later
offline summary regeneration is a separate documented derivation step
(see the artifact-provenance note above).

## Production Changes

None: `crates/*`, root `Cargo.toml`, and root `Cargo.lock` are untouched;
the experiment lives in a standalone workspace with its own `Cargo.lock`.
