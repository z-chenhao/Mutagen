# Experiment 0011: Clean Confirmatory Self-Evolution Rerun

## Status

**Pre-registered; NOT YET EXECUTED.**

This document is the immutable pre-registration for Experiment 0011. It is
committed at Stage A0 (the frozen code commit) and must not change after
that point except to append the final Results/Conclusion sections after the
run completes. Every executable value it states is also encoded in the
machine-readable single source of truth
[`experiments/0011-confirmatory-self-evolution/design.json`](../experiments/0011-confirmatory-self-evolution/design.json);
where the two differ, the run is invalid.

## 1. Why this experiment exists

Experiment 0010 (`0010-controlled-self-evolution`) was declared
**INCONCLUSIVE**: its formal verdict was voided because the executable
source changed *after* live model execution had begun (a mid-selection
bugfix commit violated the registered Stage-A source-freeze rule). 0010's
results are retained in the scientific record as *post-protocol-violation
exploratory evidence* only.

0010 also left a genuine open question: on its promotion stage the selected
candidate C1 produced **35W / 0L / 13T** (exact sign test
p = 5.82e-11 < 0.05) — strong *but non-formal* evidence that the
self-evolution loop (Evidence → Generate → Select → Promote) can yield a
real quality improvement. The loop was never run once under a fully
intact protocol.

**Experiment 0011 reruns the identical loop, from scratch, under a
mechanically enforced frozen execution protocol, so that the confirmatory
claim is finally testable.**

## 2. Hypotheses

- **H1 (evolvability).** A model prompted as a mutation operator can
  produce a policy suffix that improves the agent's success on tasks it
  was never shown, relative to the incumbent, under a fixed deterministic
  harness.
- **H2 (control).** With disjoint discovery/selection/promotion splits,
  pre-registered rules, and an independent confirmatory sign test, the
  loop runs end-to-end with every claim re-derivable from committed
  artifacts — *provided the code itself cannot change mid-run*.

0011 supports H1×H2 only if (a) every integrity gate passes and (b) the
confirmatory promotion stage classifies `quality_improvement`
(two-sided exact sign test, p < α, α registered in `design.json`).
A `quality_regression` or `quality_inconclusive` classification refutes
H1 for this model/task family. Any integrity failure renders the run
**INCONCLUSIVE** and refutes nothing about H1.

## 3. The decisive change over 0010: the mechanical source freeze

0010's freeze rule was a *promise*; 0011's is a *mechanism*
(`src/freeze.rs`):

1. **Stage A0** — every file in `FROZEN_PATHS` (20 files: all crate
   sources, `Cargo.toml`, `Cargo.lock`, `design.json`, `tasks.json`,
   `stress-profile.json`, both prompts, and this document) is committed
   as the *frozen code commit*.
2. **Stage A1** — `cargo run -p mutagen-exp-0011 -- init-run` hashes the
   exact bytes of every frozen file and writes `run-manifest.json`
   (committed before the first model request).
3. **Before every live stage** (`discover`/`generate`/`select`/`promote`)
   the guard `verify_run_freeze` runs **before the model client is
   constructed**. It requires ALL of:
   - every frozen file's current bytes hash to the manifest value;
   - every frozen file byte-matches the blob at the frozen code commit
     (`git show`);
   - **zero commits** touching any frozen path after the frozen code
     commit (`git log <A0>..HEAD -- <paths>` empty) — a
     modify→commit→revert sequence still invalidates the run, because the
     *history*, not the net diff, is the authority;
   - no staged/unstaged drift on frozen paths (`git status --porcelain`);
   - the frozen code commit is an ancestor of `HEAD`;
   - the manifest's internal hashes reproduce (manifest tamper detected).
   Any violation ⇒ the stage refuses to run and the run identity is
   **permanently invalid**: no patching, no continuation, no second run.

The `self-test` command exercises the guard against a real temporary git
repository (pristine / byte-change / mid-run commit /
modify-commit-revert / artifact-only / manifest-tamper) so the mechanism
is verified, not assumed.

## 4. Run identity

One run is registered in `design.json`: `run_id = "0011-r1"`. The
committed `run-manifest.json` binds: `run_id`,
`code_under_test_commit` (the A0 SHA), the combined
`frozen_design_sha256`, and the per-file hashes. Every record and every
summary carries the run-identity provenance block
(`run_id`, `run_manifest_sha256`, `frozen_design_sha256`) plus model,
temperature, redacted endpoint, and frozen-file hashes; verifiers
cross-check records against the summary and against the current frozen
files. Partial stage aborts are **not** resumable: the run is
inconclusive and the artifacts say so.

## 5. Tasks

Twelve *new* registered tasks (`tasks.json`), three families
(`direct_set`, `conditional_set`, `replacement`), two-key external states
with fresh `V11_*` literals — disjoint from every 0008–0010 literal:

| Split | Count | Tasks | Stress (frozen 0009 profile) |
|---|---|---|---|
| Discovery (E11) | 3 | E11A (direct, x), E11B (conditional, y), E11C (replacement, x) | S2 / S1 / S2 |
| Selection (S11) | 3 | S11A (direct, y), S11B (conditional, x), S11C (replacement, y) | S2 / S1 / S2 |
| Promotion (P11) | 6 | P11A–P11F, both keys per family | S2 / S1 / S2 |

The registry is structurally validated (12 tasks, 3/3/6 splits,
family-map ↔ task consistency, two-key states, registered fault keys) and
its canonical hash is part of every provenance block; the verifier also
re-checks literal split-disjointness.

## 6. Frozen stress

The **0009 supported family-level profile, carried over unchanged**
(`stress-profile.json`; no calibration is permitted in 0011):
`direct_set → S2 (drop first 2 writes)`, `conditional_set → S1 (1)`,
`replacement → S2 (2)`. The repeated-silent-drop fault is the only
environmental fault; it is invisible to the model (the tool reports
success) and auditable in the hidden write-attempt ledger.

## 7. The loop (stage chain)

| Stage | Episodes | Content |
|---|---|---|
| A0 | — | frozen code commit |
| A1 | — | `run-manifest.json` (network-free) |
| B Discovery | 18 | 3 E11 × 6 reps, G0 only, frozen stress |
| C Mutation generation | ≤2 mutator requests | 4 suffix candidates (no tools) |
| D Selection | 150 | 3 S11 × 10 reps × {G0,C1..C4}, exact cyclic order |
| E Promotion | 96 | 6 P11 × 8 reps × {G0, selected}, alternating order |
| **Total** | **264** | |

**Discovered failure mode (0010) #1 — weak-evidence generation.** 0010
let the mutator run when G0 had produced almost no failures. 0011
registers, in `design.json`: discovery proceeds to generation **only if**
`valid_episodes ≥ 16` **and** `incumbent_failures ≥ 8` (of 18).

**Discovered failure mode (0010) #2 — source change mid-run.** Handled by
the mechanical freeze (§3): a mid-run change does not merely "dirty the
data" — it makes the run *unrunnable*, and the failure is provable from
the committed manifest + git history.

## 8. Design numbers (single source of truth: `design.json`)

- `candidate_count = 4`; harness-assigned IDs `C1..C4` (returned order).
- Discovery: 6 reps/task; gates: ≥16 valid, ≥8 incumbent failures.
- Selection: 10 reps/task; conditions `[G0, C1, C2, C3, C4]` in exact
  cyclic order (rep r rotates; positions balanced); gates: ≥27
  common-valid cells, ≥12 G0-failures-in-common-valid (potential
  information capacity).
- Promotion: 8 reps/task; gates: ≥44 valid pairs, ≥12 potential
  information, ≥12 actually informative, infra-failure rate ≤ 0.10
  (floor 9 of 96).
- `alpha = 0.05` (two-sided exact sign test).
- Agent temperature **0.2** (the 0010 value; the earlier 0006 value 0.7
  is explicitly *not* carried over); mutator temperature **0.7**;
  hard limits **12 model turns / 16 tool calls / 10 minutes per episode**;
  one tool call per turn; endpoint + model registered in `design.json`
  (`http://127.0.0.1:8000/v1`, `incoai/Qwen3.8-27B-Splash`), no auth
  header. The mutation generator uses the **same underlying model**
  (`mutator.endpoint_index`), **no tools**.

## 9. Mutation generation (anti-seeding contract)

- The mutator receives ONLY the strict-whitelist mutation input: the
  incumbent prompt, tool schemas, and discovery episodes (model-visible
  conversation, requested calls, model-visible results, final state,
  oracle success bit). No fault internals, no selection/promotion tasks
  or literals, no hidden reasoning (network-free tamper-checked).
- **Nothing from Experiment 0010 is seeded**: no C1 text, no C1 hash, no
  hint. The mutator prompt is neutral. If the fresh mutator
  *independently rediscoveries* a semantically similar read-after-write
  policy, that is ACCEPTED as independent rediscovery and reported as
  such — it is never rejected post hoc for similarity.
- Suffix bounds: 5–100 words, ≤800 bytes, no forbidden terms, no
  `V11_*` literals, no registered task IDs, pairwise distinct after
  whitespace normalization. ≤2 attempts; the retry message carries only
  machine-generated validation errors (no performance/selection
  feedback).
- **Injection is mechanically blocked**: every pool suffix must equal
  the raw model response verbatim (verifier re-derives the pool from the
  stored raw attempt). No file access for the mutator.

## 10. Selection rule (frozen, deterministic)

Per candidate, on the common-valid cells vs G0: wins / losses / ties.
Lexicographic: **(net_margin ↓, wins ↓, losses ↑, ordinal ↑)**. A
mutation displaces G0 **only if** `net_margin > 0`; otherwise G0 is
retained (a valid "do not evolve" outcome). No cost, no tokens, no
rationale, no model judgment. The selected candidate is frozen into
`selected-candidate.json` (contains no promotion outcomes) before
promotion begins.

## 11. Promotion and the final three-way conclusion

The frozen selected candidate (or G0 if retained) is compared pairwise
against G0 on the clean promotion split. The exact two-sided sign test
over the non-tied informative pairs (p < α ⇒ significant) yields the
**final three-way conclusion**: `supported` (quality improvement,
significant) / `refuted` (quality regression, significant, or
G0-retained with no improvement) / `inconclusive` (everything else,
including any integrity failure). Cost and cache are reported as
diagnostics only; they never enter the claim.

## 12. Failure taxonomy & inconclusive handling

- **Infra failures** (transport / non-2xx / malformed response / time
  limit) are classified, recorded, and excluded from statistics; counts
  above the registered floor invalidate the stage.
- **Agent failures** (turn/call limit, bad arguments, no final answer)
  count as G0/candidate *failures* in the statistics.
- **Gate failures** block the next stage mechanically.
- **Any source-freeze violation ⇒ the whole run is INCONCLUSIVE** and
  the run identity is permanently invalid. No resume, no patch.

## 13. Reporting

The results document
(`0011-confirmatory-self-evolution-results.md`) must report: run
identity + frozen hashes; per-stage episode/wall-time/cost diagnostics;
the four candidate suffixes + rationales; selection tallies + tie-break
path; promotion W/L/T + exact p + conclusion; verifier outputs for all
four stages; the full 0010-comparison table (including an honest
assessment of whether the 0010 C1 result was real); and any integrity
events. Raw trajectories, summaries, mutation input, generation artifact,
pool, selected candidate, and run manifest are all committed.

## 14. Explicit non-goals

No Experiment 0012 is designed or started by this change. No hot reload,
no core promotion, no new production abstraction: everything here is
experiment-local (`mutagen-exp-0011`), and any future promotion of the
freeze mechanism into the workspace is a *separate* ADR-backed decision.
