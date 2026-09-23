# Experiment 0010: First Controlled Self-Evolution Loop

## Status

Complete (this section was the only part updated after the
pre-registration freeze; all other sections are frozen at the code
commit referenced below).

## Hypothesis

H1 (evolvability). A model prompted as a mutation operator can produce
a policy suffix that, when appended to a fixed incumbent agent prompt,
improves the agent's success rate on tasks it was **not** shown,
relative to the incumbent, under a fixed deterministic evaluation
harness.

H2 (control). With three disjoint task splits (discovery / selection /
promotion), a pre-registered selection rule, and an independent
confirmatory sign test on the untouched promotion split, the loop can
be run end-to-end with every claim re-derivable from committed
artifacts — i.e., self-evolution is possible *as a measured,
reproducible procedure* without the evaluator touching the evolvable
surface.

The experiment supports H1×H2 only if the confirmatory promotion test
classifies `quality_improvement` (exact sign test p < 0.05, two-sided,
on pairwise informative comparisons) **and** every integrity gate
passes. A `quality_regression` or `no_clear_difference` classification
refutes H1 for this model/task family. Integrity failures render the
run inconclusive and refute nothing about H1; they do, however, count
against the *procedure* (the control apparatus worked only if the
gates catch what they are designed to catch — exercised in
`self-test`).

## Design

### Evolvable surface

Exactly one: an **append-only policy suffix** to the incumbent system
prompt,

- 5..=100 words, ≤800 UTF-8 bytes,
- no forbidden terms (experiment, benchmark, oracle, promotion,
  selection set, held-out, stress labels, fault internals, `V_*`
  state literals, registered task IDs, …),
- proposed by the model from a whitelisted mutation input only.

Frozen and identical across all conditions: harness, kernel, model
client, tools (`state_read`/`state_write`), fault injection, oracle,
incumbent prompt, task registry, stress profile, temperatures.

### Candidate pool

`CANDIDATE_COUNT = 4`, `CANDIDATE_GENERATION = 1`, `parent_id = "G0"`.
One mutator model call produces the JSON pool; if validation fails,
exactly one retry with the machine-generated errors. The pool must be
byte-derivable from the raw mutator response (suffixes verbatim);
harness-injected text (e.g., a known prior repair) is a
verification failure. The pool is frozen after the generation stage.

### Disjoint task splits

| split | tasks | episodes | purpose |
|---|---|---|---|
| discovery | 3 (one per family) | 18 (6 reps) | behavior only; seeds the mutation input |
| selection | 3 (one per family) | 150 (5 reps × 5 conditions) | candidate comparison |
| promotion | 6 (two per family) | 96 (4 reps × 2 conditions) | confirmatory, untouched |

Family = {direct_set, conditional_set, replacement} (carried over from
Experiment 0009, same state schema, same task shapes, **new task
content**). Stresses are the frozen Experiment 0009 profile:
direct_set S2 (2), conditional_set S1 (1), replacement S2 (2).
No task appears in more than one split; no task or state literal of a
later split may appear in the mutation input (verified).

### Fault

`DropFirstNWrites` (silent drop of the first N target-key writes,
audited, model never informed), N from the frozen stress profile. The
hidden audit (`applied` / `registered_drop_index` / `fault_reason`) is
record-side only: it never appears in the conversation, the mutation
input, or any model request.

### Oracle

Independent final-state judgment vs the registered target;
`oracle_success` recomputed from `final_state` alone by the verifier;
it never consumes the model's self-report.

### Selection (frozen rule, spec-frozen before selection runs)

Per candidate over the 30 cells of its condition: `wins` = cells the
candidate succeeded where G0 failed; `losses` the reverse. Select the
lexicographic maximum of
`(net_margin = wins − losses, wins, −losses, −ordinal)`,
**subject to the promotion gate `net_margin > 0`** — otherwise retain
the incumbent `G0`. Ties on all three score components break to the
lower ordinal. Model-reported rationales, costs, token counts, and
wall time are **not inputs to this rule** (structurally absent from
the tally type).

### Confirmatory promotion test

For each (promotion task, repetition) pair, compare the selected
non-incumbent candidate against G0: win = candidate succeeds & G0
fails; loss = reverse; tie = same outcome. Exact two-sided sign test
on (wins, losses) over informative pairs, plus gates:

- ≥ 24 valid pairs, 0 infrastructure failures,
- potential information (G0 failures) ≥ 8, informative pairs ≥ 8,
- artifact complete, only registered conditions, selected candidate
  byte-frozen.

`quality_improvement` ⇔ p < 0.05 and wins > losses;
`quality_regression` ⇔ p < 0.05 and losses > wins; else
`no_clear_difference`. (Classification thresholds frozen; the
classification operates on the informative-pair sign test only.)

### Cost model

Per episode and per condition: nominal / uncached prompt tokens,
completion tokens, cached prompt tokens, and the provider cache-hit
metric; cost is reported, never used for selection.

## Tamper / verification protocol

Each stage has an offline verifier that re-derives every integrity
property from the frozen configuration and fails on any mismatch:
schema, provenance hashes (prompt/registry/profile/pool/input/selected
files), conversation invariants, fault-audit-vs-final-state replay,
oracle recomputation, usage accounting, pair census, ordering, summary
deep-compare, leakage scans. `self-test` encodes the full synthetic
artifact set plus **27 tamper cases** (leakage into the mutation
input; harness-injected repair; pool mutation; rule tampering;
post-freeze candidate mutation; cost tampering; promotion integrity;
statistic/classification/conclusion tampering) — every case must be
detected.

## Reproducibility protocol

Stage-freeze commit protocol (A → E, as executed):

- **A** — this pre-registration + all code; no results.
- **B** — discovery artifacts (18 episodes + mutation input + summary).
- **C** — generation artifact + candidate pool (frozen).
- **D** — selection artifacts (150 episodes + summary + selected
  candidate, byte-frozen before promotion).
- **E** — promotion artifacts (96 episodes + summary/conclusion) +
  results below.

No source change after A; a bug found mid-run makes the run
inconclusive, not a re-target.

## Commands

```
cargo run -- self-test
MUTAGEN_EXP_BASE_URL=... MUTAGEN_EXP_MODEL=... cargo run -- discover ...
cargo run -- verify-discovery ...
MUTAGEN_EXP_BASE_URL=... MUTAGEN_EXP_MODEL=... cargo run -- generate ...
cargo run -- verify-mutation ...
MUTAGEN_EXP_BASE_URL=... MUTAGEN_EXP_MODEL=... cargo run -- select ...
cargo run -- verify-selection ...
MUTAGEN_EXP_BASE_URL=... MUTAGEN_EXP_MODEL=... cargo run -- promote ...
cargo run -- verify-promotion ...
```

## Cost

See Results.

## Results

Model `incoai/Qwen3.8-27B-Splash` via the local endpoint (redacted). One
unseeded run, end to end. Per-stage artifacts are committed at each
stage-freeze commit (B–E); the `verify-*` verifiers all report `PASSED` and
the offline `self-test` (35 checks + 27 tamper cases) is green.

### Discovery (stage B) — 18 episodes, incumbent G0 only

- 3 tasks × 6 repetitions, incumbent suffix only.
- Oracle successes **4/18**, agent failures 6, infrastructure failures 0.
- The fault genuinely challenges the incumbent (~22% success); discovery is
  therefore an honest baseline, not a pass/fail gate. Mutation input frozen.
- Cost: 158 model requests, 145 executed tool calls.

### Mutation generation (stage C) — frozen candidate pool

- 4 bounded append-only prompt-suffix candidates C1–C4 (≤800 bytes each),
  same model, no known-repair seeding. Pool sha256 frozen
  `b2840fed…c788c`.

### Selection (stage D) — 150 episodes, 3×10×5 balanced conditions

- 150 episodes (3 tasks × 10 repetitions × 5 conditions, cyclic rotation);
  2 infrastructure failures → **28/30 common-valid cells**, 23 G0 failures
  (potential information).
- Frozen lexicographic rule (net margin > 0 required; ordinal tie-break;
  cost not an input) selects **C1**: 19 wins / 0 losses / 9 ties, net **+19**.
  Ranking **C1 > C3 > C4 > C2** (C3 14W/2L/+12, C4 6W/2L/+4, C2 6W/3L/+3).
- All integrity gates pass; C1 displaces G0 → promotion authorized.
- Cost: ~1238 model requests, ~1124 executed tool calls.

### Promotion (stage E) — 96 episodes, G0 vs C1 on the held-out split

- 6 promotion tasks × 8 repetitions × 2 conditions (G0 + C1), disjoint from
  discovery/selection.
- 48 valid pairs (0 infrastructure failures), 37 potential information,
  35 informative pairs.
- C1 vs G0: **35 wins / 0 losses / 13 ties**. Exact two-sided sign test
  **p = 5.82e-11** (< 0.05). Classification **quality_improvement**.
- G0 succeeds 11/48 pairs; C1 succeeds 46/48.
- All gates pass. Cost: ~730 model requests, ~640 executed tool calls.

### Selected candidate (C1) — the one evolvable artifact

> After any write, read the same key before treating the change as complete.
> If the read differs from the intended value, retry the same write and
> verify again. Do not claim success based only on a write acknowledgement.

(sha256 `0101d618…76f2ad`; full prompt sha256 `e4ff2994…4712d3`.)

## Conclusion

**Supported.** The first controlled self-evolution loop closed end to end on
the frozen, disjoint task splits: the model-generated C1 suffix (a
write-then-verify policy) displaced the incumbent in the frozen selection
tournament and then produced a statistically significant quality improvement
on the held-out promotion set (35W/0L, exact sign-test p ≈ 5.8e-11) with zero
regressions. The whole loop is reproducible and independently verifiable
from the committed artifacts: every stage is re-checkable by the `verify-*`
verifiers without any network access, and the frozen selection/promotion
rules plus the exact sign test are the only decision logic.

The result is a single-model, single-run confirmation on one fault, with one
evolved suffix. It is not a claim that the suffix generalizes across models,
faults, or tasks; it is a demonstration that a bounded, verifiable
self-evolution loop can produce a real, confirmable improvement.

## Follow-up

- **What 0011 would change:** widen the evolvable surface beyond an
  append-only policy suffix (e.g. routing/policy or tool-usage behavior),
  and/or move from a single held-out confirmation to a multi-seed / multi-model
  estimate so the quality gain is characterized rather than only confirmed.
  The surface, verifiers, and disjoint splits would all need to grow with it.
- **What this experiment deliberately does not claim:** no generalization
  guarantee; no deployment/hot-swap; no claim that more candidates or more
  repetitions would still select C1; the incumbent and the candidate are the
  same model, so this is self-improvement of prompt policy, not of weights or
  architecture.
- The kernel, oracle, tasks, faults, stresses, selection rule, and sign test
  are frozen and carry over unchanged into any follow-up; only the evolvable
  surface and its verifier are in scope for extension.
