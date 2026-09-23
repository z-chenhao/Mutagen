# Experiment 0010: First Controlled Self-Evolution Loop

## Status

Complete — **formal verdict: INCONCLUSIVE** due to a protocol integrity
violation (executable source was changed after real model execution had
already begun, violating the registered Stage-A source-freeze rule).

Genuine **exploratory** mutation / selection / promotion evidence is retained
and clearly labeled as *post-protocol-violation*; it is **not** a formal
`supported` verdict. (This Status section, the pre-registration-inconsistency
note, Results, Conclusion, the new *Protocol Failure Analysis* and *Formal
Verdict vs Exploratory Evidence* sections, and Follow-up were updated in the
scientific-record correction. No source code, prompt, task, or raw/summary
artifact was modified; all remain byte-identical to their stage commits.)

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

> **Pre-registration / implementation inconsistency (Stage A).** The
> registered selection design was 3 tasks × **10** repetitions × 5 conditions
> = 150 episodes, and the Stage-A *code* correctly set
> `SELECTION_REPETITIONS = 10`. But the *prose* table above (and the
> promotion row, which says “4 reps” against a code constant of 8) is
> arithmetically inconsistent with the registered totals. The Stage-A
> registration was therefore **not** internally perfect. This mismatch was
> discovered during execution and is the proximate trigger of the protocol
> violation documented in *Protocol Failure Analysis* (it is what the
> mistaken `551a339` commit “corrected” the code toward before `37219e0`
> restored it). It is recorded here, not concealed.

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

This rule is **authoritative** and was **not** satisfied: three post-A
commits (`df2a046`, `551a339`, `37219e0`) changed executable source after the
discovery and mutation-generation stages had already made real model requests.
Per the rule, that renders the run **inconclusive** (not `refuted`) — see
*Protocol Failure Analysis*. The rule is quoted verbatim, not reinterpreted.
A bug found mid-run makes the run inconclusive, not a re-target.

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

> **Read this before the per-stage numbers.** The formal Experiment-0010
> verdict is **INCONCLUSIVE**: the registered source-freeze rule ("no source
> change after A") was violated after real model execution had begun
> (discovery + mutation generation had already made model requests) by the
> post-A commits `df2a046` / `551a339` / `37219e0`. Therefore the selection
> and promotion stages below do **not** belong to a single fully-frozen
> pre-registered execution. The numbers are retained and are genuine, but the
> **selection** and **promotion** results are *exploratory / post-protocol
> evidence*, not a formal `supported` result. (Final artifact verification
> still passes **under the corrected executable** — that is a weaker claim
> than “the original A→E run satisfies the registered freeze protocol”.)

Model `incoai/Qwen3.8-27B-Splash` via the local endpoint (redacted). One
unseeded run, end to end. Per-stage artifacts are committed at each
stage-freeze commit (B–E); the `verify-*` verifiers all report `PASSED`
**under the final (corrected) executable**, and the offline `self-test`
(35 checks + 27 tamper cases) is green. Verifier passes confirm the final
artifacts are internally consistent with the final code; they do not erase
the historical freeze violation.

### Discovery (stage B) — 18 episodes, incumbent G0 only

- 3 tasks × 6 repetitions, incumbent suffix only.
- Oracle successes **4/18**, agent failures 6, infrastructure failures 0.
- The fault genuinely challenges the incumbent (~22% success); discovery is
  therefore an honest baseline, not a pass/fail gate. Mutation input frozen.
- Cost: 158 model requests, 145 executed tool calls.

### Mutation generation (stage C) — frozen candidate pool

- 4 bounded append-only prompt-suffix candidates C1–C4 (≤800 bytes each),
  same model, no known-repair seeding. Pool sha256 frozen
  `b2840fed…c788c`. Accepted in the **first** valid generation attempt
  (one mutator call; no retry was needed for the accepted pool).

  **Status: valid exploratory evidence.** This stage ran *before* any of the
  later source fixes, and its evidence packet (the mutation input) remained
  leakage-controlled, so the protocol violation does **not** invalidate it.
  - Mutation-input leakage audit: **no** `DropFirstNWrites`, `fault_reason`,
    `registered_drop_index`, the fault-audit `applied` *field*, `SEL1–3`,
    `PRO1–6`, or the known Experiment-0009 repair instruction. (The only
    textual occurrence of the word “applied” is the model’s own discovery
    reasoning — “*a transformation applied*” — which is legitimate
    model-visible evidence, not the hidden audit field.)
  - The mutator system prompt did **not** explicitly instruct read-after-write /
    verify / retry / handle-silent-failure. The accepted C1 nevertheless
    states exactly that policy → the generator **independently inferred** a
    write/read-back/retry policy from model-visible discovery trajectories.
  - This is *exploratory evidence of autonomous mutation discovery*, **not**
    a confirmatory self-evolution result.

### Selection (stage D) — 150 episodes, 3×10×5 balanced conditions

> **Label: exploratory selection result under the corrected post-A
> executable.** This tournament ran after the `37219e0` source fix, so it is
> *not* part of a fully pre-registered frozen A→E run. The numbers are
> genuine and are retained.

- 150 episodes (3 tasks × 10 repetitions × 5 conditions, cyclic rotation);
  2 infrastructure failures → **28/30 common-valid cells**, 23 G0 failures
  (potential information).
- Frozen lexicographic rule (net margin > 0 required; ordinal tie-break;
  cost not an input) selects **C1**:
  - **C1: 19W / 0L / 9T, net +19** (selected)
  - C3: 14W / 2L / 12T, net +12
  - C4: 6W / 2L / 20T, net +4
  - C2: 6W / 3L / 19T, net +3
  - Ranking **C1 > C3 > C4 > C2**; C1 displaces G0 → promotion authorized.
- Structural integrity gates pass *under the corrected executable*;
  cost excluded from the rule. Cost: ~1238 model requests, ~1124 executed
  tool calls.

### Promotion (stage E) — 96 episodes, G0 vs C1 on the held-out split

> **Label: strong independent exploratory validation — not the formal 0010
> verdict.** Promotion used **only** G0 and C1 on a promotion split that never
> appeared in the mutation input, and the other candidates did not touch it —
> so it is an *independent* held-out check of the generated candidate. But
> because the executable changed after Stage A, this result **cannot** upgrade
> Experiment 0010’s formal conclusion from **inconclusive**.

- 6 promotion tasks × 8 repetitions × 2 conditions (G0 + C1), disjoint from
  discovery/selection.
- **48 valid pairs** (0 infrastructure failures); G0 succeeds **11/48**,
  C1 succeeds **46/48**; 37 potential information, 35 informative pairs.
- C1 vs G0: **35 wins / 0 losses / 13 ties**.
- Exact two-sided sign test **p = 2 / 2^35 = 5.820766091346741e-11**
  (< 0.05). Classification **quality_improvement**.
- Cost: ~730 model requests, ~640 executed tool calls.

### Selected candidate (C1) — the one evolvable artifact

> After any write, read the same key before treating the change as complete.
> If the read differs from the intended value, retry the same write and
> verify again. Do not claim success based only on a write acknowledgement.

(sha256 `0101d618…76f2ad`; full prompt sha256 `e4ff2994…4712d3`.)

## Protocol Failure Analysis

This experiment’s scientific rule is stricter than software correctness:
**after the first real model request, executable changes require a new run
identity.** The three post-A source changes below are documented not to defend
them but to establish that the registered freeze was violated:

- **P1 — agent request semantics** (`df2a046`, added `parallel_tool_calls=false`).
  Changes the request the model sees on every agent turn, and therefore which
  tool calls it can generate. The commit records that the previous selection
  attempt produced many `parallel_tool_calls_unsupported` failures. This is an
  executable *semantic* change, not a cosmetic one.
- **P2 — repetition miscorrection** (`551a339`, `SELECTION_REPETITIONS` 10 → 5).
  Moved the code *away* from the intended 150-episode design toward the
  arithmetically-wrong prose table (see the Stage-A inconsistency note). It is
  the proximate trigger of the mid-run correction.
- **P3 — selection-loop / design restoration** (`37219e0`). Fixed the exclusive
  `1..len` range that excluded the 5th condition position (making 150 episodes
  impossible), restored repetitions to 10, and corrected the balance/cache /
  verifier off-by-one and the Stage-A prose/code mismatch.

All three are reasonable *engineering* corrections. But any one of them, made
after discovery and mutation generation had already made real model requests,
invalidates the confirmatory A→E protocol as a *single frozen execution*. They
are retained in history (not squashed, not rewritten) precisely because the
broken timeline is part of the scientific provenance: it is *why* the verdict is
inconclusive.

## Formal Verdict vs Exploratory Evidence

These are two distinct layers that must not be mixed.

**Formal pre-registered verdict: INCONCLUSIVE.**
Reason: post-A executable modifications (P1/P2/P3) violated the registered
source-freeze gate after real model execution had begun. The registered rule is
explicit: *a bug found mid-run makes the run inconclusive, not a re-target.*
An integrity failure renders the run inconclusive and refutes nothing about the
scientific question (it is **not** `refuted`).

**Exploratory evidence retained (post-protocol-violation):**
1. Model-visible discovery evidence was sufficient for the mutator to
   independently generate a read-back / verify / retry policy (no known-repair
   seeding; the 0009 repair was not leaked into the mutation input).
2. Under the corrected harness, that generated candidate C1 ranked **first** in
   the selection tournament (19W / 0L, net +19; C1 > C3 > C4 > C2).
3. The frozen C1 then achieved **35W / 0L / 13T** on a disjoint promotion set
   that was never exposed to the mutation generator.
4. The promotion exact sign test is highly significant
   (p = 2 / 2^35 = 5.820766091346741e-11) → `quality_improvement`.

## Conclusion

**Formal conclusion: INCONCLUSIVE.**

Experiment 0010 did not satisfy its own pre-registered source-freeze
invariant. Real model execution had already occurred in discovery and mutation
generation before executable code was changed three times (`df2a046`,
`551a339`, `37219e0`) to correct provider/request and selection-loop defects.
The original protocol explicitly specified that any post-A source change makes
the run inconclusive. It is therefore **not** reported as `supported`, and the
`H1×H2` hypothesis is **not** formally confirmed:

- **H1 (evolvability):** *suggestive / strong exploratory* support — the mutator
  independently inferred a verification/retry policy from visible evidence, and
  the generated candidate then performed well on a held-out set.
- **H2 (control):** *not formally supported* — the frozen-executable control
  protocol was violated, so the “self-evolution as a measured, reproducible
  procedure” claim is not established by this run.

The run nevertheless produced important exploratory evidence: the mutation
generator independently inferred a write/read-back/retry policy from
model-visible discovery trajectories; the corrected selection tournament chose
that generated candidate; and the frozen candidate later achieved 35 wins, 0
losses, and 13 ties on a disjoint promotion set. These findings **motivate a
clean confirmatory rerun** but do not convert 0010 into a formally supported
experiment.

Framed plainly: **0010 is a successful pilot of the intended generate →
select → promote mechanism, but not a valid confirmatory execution of the
frozen protocol.** It produced strong exploratory evidence that the mechanism
can work, while simultaneously demonstrating that the experiment-control
implementation was not yet reliable enough for a formal `supported` verdict.
“Self-evolution is confirmed” is **not** claimed.

(Verifier note: the *final* artifacts still verify cleanly — `verify-*` all
`PASSED` and `self-test` green — **under the final corrected executable**. That
is a weaker statement than “the original A→E run satisfies the registered
freeze protocol,” and the two are kept distinct.)

## Follow-up

**0011 = a clean confirmatory rerun of the same bounded self-evolution
question** — not a widening of the evolvable surface yet.

- **Same core question, fresh everything:** fresh discovery / selection /
  promotion tasks; fresh mutation generation; the *same* bounded append-only
  suffix surface; the *same* selection authority; the *same* promotion sign
  test.
- **No C1 reuse:** 0011 must **not** seed or directly reuse C1. The new mutator
  must independently generate mutations from fresh discovery evidence; otherwise
  0011 would validate the known solution rather than autonomous evolution.
- **Mechanical source freeze (the key 0011 fix):** enforce source immutability
  in code, not by process convention. Evaluate: record the code commit /
  executable hash at run start; before every stage, verify the git tree /
  executable hash still matches the frozen code-under-test; **refuse stage
  execution after any source drift**; separate `run_id` from `experiment_id`;
  and treat any source modification after the first real model request as a
  **permanent invalidation requiring a new run identity**.
- Only **after** a clean 0011 `supported` result should later experiments widen
  the evolvable surface (routing / tool-usage behavior, multi-seed /
  multi-model estimates, …).

What 0010 does **not** claim: no generalization guarantee; no
deployment/hot-swap; no formal `supported` verdict; `H1×H2` is not confirmed;
the no-source-change invariant did **not** hold; and “self-evolution is
confirmed” is not asserted. The kernel, oracle, tasks, faults, stresses,
selection rule, and sign test are frozen and carry over unchanged.

Do **not** implement 0011 in this change; it is recorded for a separate
clean-run effort.
