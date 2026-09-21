# Experiment 0008: Difficulty-Calibrated Improvement Discrimination

**Status:** pre-registered design (sections below) + results (filled by
the execution commits).

**Crate:** `experiments/0008-calibrated-improvement/` (standalone
workspace; not part of the production workspace; no production API).

**Model:** local OpenAI-compatible endpoint, `incoai/Qwen3.8-27B-Splash`
(redacted endpoint `http://127.0.0.1:8000/v1`), temperature 0.2,
`tool_choice = auto`, non-streaming, no fallback model, no API key
required (same interface as Experiments 0006–0007).

---

## Prior Evidence

- **0001–0005** established the synthetic replay / causality / evidence
  substrate.
- **0006** validated a real-model Rust-native kernel against
  `incoai/Qwen3.8-27B-Splash` and real trajectory divergence.
- **0007** validated an objective final-state evaluator that
  discriminates a controlled regression candidate
  (`quality_regression`, p ≈ 3.9e-5, 18/18 held-out pairs informative)
  and cost/cache accounting. BUT its improvement arm produced
  **24 valid held-out pairs and only 1 informative (non-tied) pair**:
  the one-shot silent-fault tasks mostly fell on the *ceiling* (baseline
  succeeded anyway), so the evaluator could not demonstrate the
  improvement direction.

## Research Question

> Can a baseline-only, pre-registered difficulty-calibration procedure
> create sufficient held-out headroom and informative paired outcomes
> for Mutagen's objective evaluator to detect a controlled robustness
> improvement on a real stochastic model?

Secondary questions:

1. Does calibration transfer from development (calibration) tasks to
   unseen task variants from the same registered structural family?
2. Does an explicit informative-pair gate prevent the ceiling-effect
   failure mode observed in Experiment 0007?
3. What additional execution cost does the improving candidate pay for
   the measured quality gain?

## Hypothesis

Pre-registered: for at least two of the three registered task families,
baseline-only calibration will identify a fault severity that produces
intermediate baseline success on calibration tasks. When those
severities are frozen and transferred to unseen held-out variants, the
held-out baseline will retain sufficient headroom and produce enough
informative baseline/candidate_repair pairs for the repair candidate to
achieve significantly more wins than losses under the exact paired
sign test.

No regression-control hypothesis is needed in 0008: Experiment 0007
already demonstrated regression-direction discrimination, and 0008
deliberately isolates the **improvement direction** (exactly two
final-evaluation conditions: `baseline` and `candidate_repair`).

## Why 0007 Was Insufficient

0007's held-out repair comparison: 24 valid pairs, **1 informative
pair** (1 win, 0 losses → p = 1.0, `quality_inconclusive`). The
one-shot `DropFirstWrite` severity was a single fixed fault, and most
held-out episodes landed on the ceiling (the model happened to succeed
despite the drop) or was already resolved by baseline behavior. A valid
pair is not an informative pair: **design completeness ≠ statistical
information**. 0007's conclusion rules had no headroom or
informative-pair gate, so it could only report "refuted" on a
measurement that was mostly silent.

## Valid vs Informative Pairs

- **Valid pair**: a (task, repetition) pair in which neither episode
  suffered an infrastructure failure. Agent failures (turn limit, etc.)
  are ordinary task failures and keep the pair valid.
- **Informative pair**: `baseline oracle_success != repair
  oracle_success`, i.e. wins + losses. Nothing else — not trajectory
  difference, not extra calls, not token difference, not different
  final wording.
- 0008 requires **both** ≥ 24 valid pairs and ≥ 12 informative pairs
  before any directional classification.

## Two-Stage Design

```
Phase A  baseline-only difficulty calibration (75 episodes)
        |  freeze
        v
   Difficulty Selection Manifest (selected-difficulty.json, committed)
        |
        v
Phase B  held-out baseline vs candidate_repair (48 or 72 episodes)
```

Candidate behavior **MUST NOT** influence difficulty selection. Phase A
executes baseline only; the repair candidate is never run during
calibration. The selected difficulties are frozen in a committed
manifest and its SHA is recorded in every Phase B record.

## Anti-Leakage

The difficulty selector (`calibration.rs::select_family_stress`)
accepts only: a family id and baseline-only calibration outcomes
(stress id, repetition, oracle success). It has **no** argument for:
candidate outcomes, the candidate prompt, candidate success rate,
candidate trajectory, candidate cost, or held-out outcomes. This is
enforced structurally (the type system) and artifact-level (calibration
records carry `condition: "baseline"` only, and the verifier rejects
candidate-side fields in Phase A artifacts). The frozen manifest
contains no candidate outcome data; it carries prompt *hashes* (config
provenance) only.

## Task Families

Exactly three structural families, each with 1 calibration + 2
held-out tasks (9 registered tasks, frozen in `tasks.json`):

| Family | Calibration | Held-out | Pattern |
|---|---|---|---|
| `direct_set` | C1: set x to A (fault key x) | H1: set y to C (key y); H2: set x to R (key x) | unconditional write |
| `conditional_set` | C2: if x EMPTY set x to A (key x) | H3: if y B set y to C (key y); H4: if x Q set x to R (key x) | conditional write |
| `replacement` | C3: change x from A to B (key x) | H5: change y from C to D (key y); H6: change x from M to N (key x) | value replacement |

All held-out tasks were registered and committed **before** Phase A.
No held-out task is authored or modified after observing calibration.

## Stress Ladder

A registered deterministic ladder of **repeated silent write drops**
(replacing 0007's one-shot-only fault):

| Level | Fault | Selected? |
|---|---|---|
| S0 | `Reliable` (0 dropped) | sanity control only, never a final stress |
| S1 | `DropFirstNWrites { key, count = 1 }` | yes |
| S2 | `DropFirstNWrites { key, count = 2 }` | yes |
| S3 | `DropFirstNWrites { key, count = 3 }` | yes |
| S4 | `DropFirstNWrites { key, count = 4 }` | yes |

Semantics: for the registered target key, the first `count` writes
return the identical model-visible success payload but do not mutate
state; writes from `count+1` onward apply normally, as do all
other-key writes. The fault is invisible to the model (no
`applied`/`dropped`/`fault_*` field ever appears in the transcript;
the model can only discover it via `state_read`). Every write attempt
is persisted in an experiment-only audit log
(`sequence, key, requested_value, applied, fault_reason,
registered_drop_index`) that never enters the transcript.

## Repair Candidate

`candidate_repair` = baseline prompt + exactly one registered line:

> After every successful state_write, read the same key. If the
> observed value differs from the value you intended to write, write it
> again and re-read it. Repeat until the value matches, but make at most
> five state_write attempts for that requested key.

Verified programmatically (`repair == baseline + registered mutation`)
before any model call in either phase. Why five: the ladder maximum S4
drops 4 writes, so a compliant repair has theoretical capacity
`5 writes + 5 reads = 10 tool calls` ≤ `max_tool_calls = 16`; kernel
limits stay `max_model_turns = 12`, `max_tool_calls = 16`
(unchanged from 0007, not raised).

Both prompts (and `tasks.json`, `stress-levels.json`, this document)
are committed in the calibration code-under-test commit, **before any
real model request**. The repair prompt's SHA-256 must still match
after calibration; Phase B refuses to start otherwise.

## Objective Oracle

Carried over from 0007: an episode succeeds iff (1) it reached normal
completion and (2) the final external state exactly matches the
registered target state (unchanged keys must retain their expected
values). Agent failures are ordinary task failures; infrastructure
failures are excluded from pairs. The oracle's input type has no field
for condition, prompt, candidate identity, **stress level**, cost, or
trajectory style — it judges the outcome, not how it was produced.

## Calibration Rule

Phase A = 3 families × 5 stress levels × 5 repetitions = **75
baseline-only episodes**, in the registered cyclic stress order
(rep r, position p → level `(r+p−2) mod 5`), so every stress occurs
once in every execution position per family.

- **Reliable sanity gate:** S0 baseline success ≥ 4/5, else the family
  is `calibration_invalid` and gets no selection.
- **Headroom band:** among S1..S4, a level is eligible iff baseline
  success is exactly 2/5 or 3/5 (40% or 60% — deliberately
  intermediate). 0/5, 1/5, 4/5, 5/5 are never selectable.
- **Selection:** among eligible levels, minimize `|rate − 0.5|`; 2/5
  and 3/5 are equidistant, so ties break to the **lower** stress level.
  No human judgment; the pure function is unit-tested over its edge
  cases.

## Family Selection Rule → Manifest

`selected-difficulty.json` (the Difficulty Selection Manifest)
contains: `experiment_id`, `calibration_code_commit`,
`calibration_raw_sha256`, frozen prompt/registry hashes, per-family
`s0_success..s4_success` + `selected_stress` + `selection_reason`,
`calibrated_family_count`, and `proceed_to_evaluation`. It is
recomputable byte-for-byte from the raw calibration artifact by the
verifier.

## Minimum Calibrated-Family Gate

Proceed to Phase B only if **≥ 2 of 3** families calibrate. Otherwise
the experiment concludes `inconclusive`, the calibration artifacts are
committed, and nothing is tuned.

## Held-Out Design

For each calibrated family, both registered held-out tasks are
evaluated under **that family's frozen selected stress**. Exactly
**6 repetitions** per (held-out task × condition), with the registered
alternating condition order per rep:
`[baseline, repair], [repair, baseline], …` — each condition is first
exactly 3 times and second exactly 3 times. Expected size is derived
from the committed manifest: 2 families → 48 episodes; 3 families → 72.

## Headroom Gate (NEW)

Before interpreting improvement significance: the pooled selected-family
held-out **baseline** success rate must lie in `[0.20, 0.80]`.
Outside the band → `inconclusive` (not refuted). This broad
information-availability gate protects against floor/ceiling regimes
and is the direct correction of 0007. The threshold is not re-tuned
after seeing data.

## Valid-Pair Gate

After infrastructure exclusions, **≥ 24 valid held-out pairs** are
required (24 for two calibrated families; 36 possible for three).
Otherwise `inconclusive`.

## Informative-Pair Gate (NEW)

**≥ 12 informative pairs** (wins + losses) are required before
directional classification, **even if** many pairs are valid. This
explicitly closes 0007's methodology hole. Informative is defined
strictly as discordant oracle success (see "Valid vs Informative
Pairs"); it is never redefined after results.

## Exact Sign Test

Identical to 0007: two-sided exact sign test on `n = wins + losses`
non-tied pairs, `k = min(wins, losses)`,
`p = min(1, 2 · Σ_{i=0..k} C(n,i) · 0.5ⁿ)`, α = 0.05. No statistics
crate. Classification: `quality_improvement` iff wins > losses and
p ≤ 0.05; `quality_regression` iff losses > wins and p ≤ 0.05; else
`quality_inconclusive`. The primary test **pools all selected
held-out families**; per-family win/loss values are diagnostic only and
never the basis of a significance claim.

## Pre-Registered Conclusion Rule

`inconclusive` if ANY: Phase A artifact incomplete; calibrated
families < 2; Phase B artifact incomplete; infrastructure failure rate
> 10%; held-out baseline success outside [0.20, 0.80]; valid pairs <
24; informative pairs < 12.

`supported` iff all gates pass AND repair = `quality_improvement`.
`refuted` iff all gates pass but repair ≠ `quality_improvement`.

This encodes the 0008 core distinction: *insufficient measurement
information* → **inconclusive**; *sufficient information, no expected
improvement* → **refuted**. The rule is not modified after results.

## Usage / Cache Accounting

Per-request provider-reported usage (`prompt_tokens`,
`cached_prompt_tokens`, `uncached_prompt_tokens`, `completion_tokens`,
`reasoning_tokens`, `finish_reason`) is stored as `null` when not
reported; reasoning *content* is not captured. Quality and cost are
never combined into a utility score. Reported separately: quality
direction, model requests, tool calls, nominal/cached/uncached prompt
tokens, completion tokens, reasoning tokens, wall time, and failure
consumption (per-condition `failure` cost block). Phase A cache audit
is by stress level × execution position; Phase B by condition ×
condition position. Cache is taken from provider-reported counts only,
never inferred from latency. Primary quality is **not** cost-adjusted:
if repair costs more and still improves, that is a quality result; the
quality/cost trade-off is a later selection-policy question, not 0008's.

## Evidence We Can Claim (if supported)

- That a baseline-only, pre-registered difficulty-calibration
  procedure — on these three registered structural families, this
  state-manipulation environment, and this model — selected stress
  levels without observing the candidate, and that after freezing and
  transferring them to unseen variants, the Mutagen objective evaluator
  retained sufficient held-out headroom and informative paired outcomes
  to classify the registered controlled robustness improvement under
  the exact paired sign test.

## Evidence We Cannot Claim

- General agent evaluation solved; automatic evolution solved.
- That difficulty calibration generalizes to other environments,
  models, or task families.
- That the selection policy is production-ready or that any of the
  experimental code (kernel, oracle, stats, calibration, manifest)
  should enter production crates.
- Any quality/cost trade-off decision.

---

# Results

*Filled in by the execution commits below; result sections are the only
permitted workspace changes between the calibration and final commits.*

## Phase A — Calibration

### Calibration Integrity

<!-- PLACEHOLDER: 75/75 records, cyclic order, fault audit, verifier result -->

| Family | S0 | S1 | S2 | S3 | S4 | Selected | Reason |
|---|---|---|---|---|---|---|---|
| direct_set | — | — | — | — | — | — | — |
| conditional_set | — | — | — | — | — | — | — |
| replacement | — | — | — | — | — | — | — |

### Reliable Sanity Gate

<!-- PLACEHOLDER -->

### Selected / Uncalibrated Families

<!-- PLACEHOLDER -->

### Calibration Provenance

- Calibration code-under-test commit: <!-- PLACEHOLDER -->
- Calibration raw SHA-256: <!-- PLACEHOLDER -->
- Selection manifest SHA-256: <!-- PLACEHOLDER -->
- Final-evaluation execution commit: <!-- PLACEHOLDER -->

## Phase B — Evaluation

### Evaluation Integrity

<!-- PLACEHOLDER: record count vs manifest-derived expected, verifier result -->

### Held-Out Baseline Headroom

<!-- PLACEHOLDER: successes/failures/rate, gate PASS/FAIL -->

### Valid Pair Count / Informative Pair Count

<!-- PLACEHOLDER -->

### Repair vs Baseline

<!-- PLACEHOLDER: wins / losses / ties / non-ties / exact p / classification -->

### Per-Family Diagnostic

<!-- PLACEHOLDER -->

### Difficulty Transfer

| Family | Calibration Baseline Rate | Held-Out Baseline Rate | Transfer Error |
|---|---|---|---|
| — | — | — | — |

### Cost / Usage & Cache Position Audit

<!-- PLACEHOLDER -->

### Failure Resource Consumption

<!-- PLACEHOLDER -->

## Experiment Conclusion

<!-- PLACEHOLDER: supported | refuted | inconclusive, with the exact gate(s) -->

## Architectural Question (documented, not answered)

Has the evaluator accumulated enough evidence to support the first
mutation → evaluation → selection experiment? If 0008 is supported,
this becomes the natural Experiment 0009 question. **Experiment 0009 is
not implemented by this experiment.**
