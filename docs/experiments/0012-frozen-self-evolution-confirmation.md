# Experiment 0012: Frozen End-to-End Self-Evolution Confirmation

## Status

**Pre-registered; NOT YET EXECUTED.**

This document is the immutable pre-registration for Experiment 0012. It is
committed at Stage A0 (the frozen code commit) and must not change after
that point except to append the final Results/Conclusion sections after
the run completes. Every executable value it states is also encoded in the
machine-readable single source of truth
[`experiments/0012-frozen-self-evolution-confirmation/design.json`](../experiments/0012-frozen-self-evolution-confirmation/design.json);
where the two differ, the run is invalid.

## 1. Why this experiment exists

Experiments 0010 and 0011 were both declared **INCONCLUSIVE**, each for a
*mechanical* reason rather than a scientific one:

- **0010** — a source change was committed mid-selection, violating the
  (then-promised) Stage-A source-freeze rule; the run identity was
  permanently invalid. Its C1 promotion evidence (35W/0L/13T) is retained
  as post-protocol-violation exploratory evidence only.
- **0011** — the mechanical source freeze held, but three protocol
  gaps remained: (a) mutation generation returned a structurally
  drifted JSON shape (a `suffixes` key / bare strings) that the frozen
  validator rejected on both registered attempts, so no candidate pool
  ever formed and the loop stopped at Stage C; (b) the retry message
  carried only opaque validation errors, not the expected shape; and
  (c) the promotion-stage evidence for the finally accepted candidate
  was not committed as a frozen artifact, so Stage B's provenance claim
  (that every later stage ran against the *committed* earlier evidence)
  could not be mechanically verified.

**Experiment 0012 is a frozen, confirmatory re-run of the exact 0011
loop** (Evidence → Generate → Select → Promote) with exactly four
control/interface changes and no scientific changes. Its single question:

> With the source, the mutation-output schema, the retry semantics, the
> per-stage artifact provenance, and the endpoint discipline all frozen,
> does the loop produce complete, reproducible, mechanically verifiable
> evidence — and a formal SUPPORTED / REFUTED / INCONCLUSIVE verdict —
> in one run?

The hypothesis under confirmation is the 0011 one (H1 evolvability ×
H2 control): on a live endpoint, the incumbent G0 is displaced at
selection by a candidate generated from discovery evidence, and the
winner survives promotion against G0 under an exact two-sided sign test.

## 2. The four 0012 changes (and only these)

Everything else in 0011 is carried over byte-for-byte in mechanism.

1. **Explicit mutation-output JSON schema (0011 failure #1/#2).**
   The frozen mutator prompt documents the exact expected response: a
   single JSON object with exactly one key `candidates`, whose value is
   an array of exactly `candidate_count` (4) objects, each object
   having exactly the string fields `suffix` and `rationale` — no other
   fields, no top-level arrays of strings, JSON only. The mutator
   enforces that exact shape (`src/mutation.rs`), so a structurally
   drifted response fails *deterministically* with shape-informative
   errors instead of being silently dropped.
2. **Shape-informative retry.** On structural failure, the registered
   2-attempt protocol retries with: the previous raw response retained
   in the conversation, plus a retry message containing (a) the exact
   machine validation errors and (b) the exact expected JSON structure
   again. Still no performance/selection/cost feedback, no human
   rewrite.
3. **Commit-level stage-artifact freeze (0011 gap #3).** After each
   live stage the artifacts are verified, manifested in a stage-freeze
   file (paths + SHA-256, in the crate directory), and COMMITTED
   together with the artifacts. The next live stage refuses to run
   unless the previous stage's freeze verifies against Git: every
   frozen artifact must exist at the freeze commit with the recorded
   bytes; the freeze commit must post-date the code-under-test commit;
   no later commit may have touched a frozen artifact (a mid-run edit
   is caught even if the tree was later restored — history, not net
   diff, is the authority). `src/stage_freeze.rs`. There is no
   Stage-E freeze: promotion is the final stage.
4. **No unregistered endpoint calls.** No `run` / `smoke` / `debug` /
   `probe` commands exist anywhere in the command table; the only live
   commands are the four registered stages. A live stage whose output
   already exists on disk (it emitted its first request) refuses to run
   again: a crashed stage is INCONCLUSIVE — no restart, no resume, no
   rerun. The endpoint URL and the 600 s per-episode time budget are
   named constants in `src/model.rs` (`REGISTERED_ENDPOINT`,
   `EPISODE_TIME_LIMIT_MS`); the design manifest carries the single
   registered `model` field, and there is no per-request model or
   endpoint override path.

## 3. The mechanical guards (every live stage, before any request)

1. **Source freeze** (0011 mechanism, `src/freeze.rs`, 21 frozen files):
   every frozen file's current bytes must hash to the manifest value,
   must match the blob at the code-under-test commit, no frozen-path
   commit after A0 (history is the authority), no working-tree drift
   on frozen paths, A0 is an ancestor of HEAD, and the manifest's
   internal hashes must reproduce. Any violation ⇒ the stage refuses
   and the run identity is permanently invalid.
2. **Previous-stage freeze** (`src/stage_freeze.rs`, change #3 above):
   B needs only the source freeze; C adds the B freeze; D adds the C
   freeze; E adds the D freeze **and** a non-G0 selected candidate (if
   G0 was retained at selection, the formal result is REFUTED and
   promotion must not run).

Both guards are exercised by `self-test` against real temporary git
repositories (pristine / byte-change / mid-run commit /
modify-commit-revert / uncommitted artifact / tampered manifest /
C-before-D / D-before-E) so the mechanisms are verified, not assumed.

## 4. Run identity

One run: `run_id = "0012-r1"`. The committed `run-manifest.json` binds
`run_id`, `code_under_test_commit` (the A0 SHA), the combined
`frozen_design_sha256`, and the per-file hashes of all 21 frozen files.
Every record and summary carries the full provenance block (run
identity + model + temperature + redacted endpoint + frozen-file
hashes); verifiers cross-check records ↔ summary ↔ current frozen files.
Partial stage aborts are not resumable (no-restart rule, change #4).

## 5. Tasks

Twelve *new* registered tasks (`tasks.json`), three families
(`direct_set`, `conditional_set`, `replacement`), two-key external
states with fresh `V12_*` literals — disjoint from every 0008–0011
literal:

| Split | Count | Tasks | Stress (frozen 0009 profile) |
|---|---|---|---|
| Discovery (E12) | 3 | E12A (direct, x), E12B (conditional, y), E12C (replacement, x) | S2 / S1 / S2 |
| Selection (S12) | 3 | S12A (direct, y), S12B (conditional, x), S12C (replacement, y) | S2 / S1 / S2 |
| Promotion (P12) | 6 | P12A–P12F, both keys per family | S2 / S1 / S2 |

The registry is structurally validated (12 tasks, 3/3/6 splits,
family-map ↔ task consistency, two-key states, registered fault keys)
and its canonical hash is part of every provenance block; the verifier
re-checks literal split-disjointness.

## 6. Frozen stress

The **0009 supported family-level profile, carried over unchanged**
(`stress-profile.json`; no calibration is permitted): `direct_set → S2
(drop first 2 writes)`, `conditional_set → S1 (1)`, `replacement → S2
(2)`. The repeated-silent-drop fault is the only environmental fault;
it is invisible to the model and auditable in the hidden write-attempt
ledger.

## 7. The loop (stage chain)

| Stage | Episodes | Content |
|---|---|---|
| A0 | — | frozen code commit |
| A1 | — | `run-manifest.json` (network-free) |
| B Discovery | 18 | 3 E12 × 6 reps, G0 only, frozen stress |
| C Mutation generation | ≤2 mutator requests | 4 suffix candidates (no tools) |
| D Selection | 150 | 3 S12 × 10 reps × {G0,C1..C4}, exact cyclic order |
| E Promotion | 96 | 6 P12 × 8 reps × {G0, selected}, alternating order |
| **Total** | **264** | |

Gate failures block the next stage mechanically (registered in
`design.json`); infra failures above the registered floor invalidate
the stage; any guard violation makes the whole run INCONCLUSIVE.

## 8. Design numbers (single source of truth: `design.json`)

- `candidate_count = 4`; harness-assigned IDs `C1..C4` (returned order).
- Discovery: 6 reps/task; gates: ≥16 valid, ≥8 incumbent failures (of 18).
- Selection: 10 reps/task; conditions `[G0, C1, C2, C3, C4]` in exact
  cyclic order (rep r rotates; all five positions covered every rep);
  gates: ≥27 common-valid cells, ≥12 G0-failures-in-common-valid.
- Promotion: 8 reps/task; gates: ≥44 valid pairs, ≥12 potential
  information, ≥12 actually informative, infra-failure rate ≤ 0.10.
- `alpha = 0.05` (two-sided exact sign test).
- Agent temperature **0.2**; mutator temperature **0.7**; hard limits
  **12 model turns / 16 tool calls** per episode; a registered 600 s
  per-episode time budget (`EPISODE_TIME_LIMIT_MS` in `src/model.rs`,
  carried over from 0011's `max_time_ms`); 60 s+ per-request timeout
  ceiling; one tool call per turn (`parallel_tool_calls: false`,
  `tool_choice: "auto"`).
- Model: `incoai/Qwen3.8-27B-Splash` over the single registered
  endpoint `http://127.0.0.1:8000/v1` (named constant; no auth header;
  no fallback model; the mutation generator uses the **same**
  underlying model, no tools).

## 9. Mutation generation (anti-seeding contract)

- The mutator receives ONLY the strict-whitelist mutation input: the
  incumbent prompt, tool schemas, and discovery episodes (model-visible
  conversation, requested calls, model-visible results, final state,
  oracle success bit). No fault internals, no selection/promotion tasks
  or literals, no hidden reasoning (network-free tamper-checked).
- **Nothing from Experiments 0010/0011 is seeded**: no candidate text,
  no hash, no hint. The mutator prompt is neutral apart from the
  explicit output schema (§2.1). If the fresh mutator *independently
  rediscoveries* a semantically similar policy, that is ACCEPTED as
  independent rediscovery and reported as such — never rejected post
  hoc for similarity.
- Suffix bounds: 5–100 words, ≤800 bytes, no forbidden terms, no
  `V12_*` literals, no registered task IDs, pairwise distinct after
  whitespace normalization. ≤2 attempts.
- **Injection is mechanically blocked**: every pool suffix must equal
  the raw model response verbatim (verifier re-derives the pool from
  the stored raw attempt). No file access for the mutator.

## 10. Selection rule (frozen, deterministic)

Per candidate, on the common-valid cells vs G0: wins / losses / ties.
Lexicographic: **(net_margin ↓, wins ↓, losses ↑, ordinal ↑)**. A
mutation displaces G0 **only if** `net_margin > 0`; otherwise G0 is
retained (a valid "do not evolve" outcome ⇒ formal result REFUTED).
No cost, no tokens, no rationale, no model judgment. The selected
candidate is frozen into `selected-candidate.json` (contains no
promotion outcomes) and stage-frozen into the D commit before
promotion begins.

## 11. Promotion and the final three-way conclusion

The frozen selected candidate (G0 if retained — but then promotion does
not run) is compared pairwise against G0 on the clean promotion split.
The exact two-sided sign test over the non-tied informative pairs
(p < α ⇒ significant) yields the **final three-way conclusion**:
`supported` (significant quality improvement) / `refuted`
(significant regression, or G0 retained at selection) / `inconclusive`
(everything else, including any integrity failure, gate failure, infra
overflow, or crash). Cost and cache are reported as diagnostics only.

## 12. Failure taxonomy & inconclusive handling

- **Infra failures** (transport / non-2xx / malformed response / time
  limit) are classified, recorded, and excluded from statistics;
  counts above the registered floor invalidate the stage.
- **Agent failures** (turn/call limit, bad arguments, no final answer)
  count as G0/candidate *failures* in the statistics.
- **Gate failures** block the next stage mechanically.
- **Any source-freeze or stage-freeze violation ⇒ the whole run is
  INCONCLUSIVE**, the run identity is permanently invalid, and no
  later stage may run. No resume, no patch, no second attempt at the
  same stage (no-restart rule).

## 13. Artifact layout & reporting

- Raw trajectories + summaries: `docs/experiments/artifacts/0012-*.json[l]`
  (registered; excluded from the source freeze; frozen per stage by the
  stage-freeze mechanism).
- Runtime evidence in the crate directory: `run-manifest.json` (A1),
  `mutation-input.json`, `candidate-pool.json`,
  `selected-candidate.json`, `stage-{b,c,d}-freeze.json` — the freeze
  files and the manifest/registry/prompts are source-frozen design
  inputs or outputs bound by their own mechanism.
- Each live stage commits its artifacts + freeze manifest together; the
  next stage verifies that commit before making its first request.

The results document
(`0012-frozen-self-evolution-confirmation-results.md`) must report: run
identity + frozen hashes; per-stage episode/wall-time/cost diagnostics;
the four candidate suffixes + rationales (and whether any is an
independent rediscovery of the 0010/0011 policy); the mutation attempts
(including any retry and its shape errors); selection tallies +
tie-break path; promotion W/L/T + exact p + conclusion; verifier and
guard outputs for all stages; the 0010/0011 comparison; and any
integrity events. All artifacts are committed.

## 14. Explicit non-goals

No hot reload, no core promotion, no new production abstraction:
everything here is experiment-local (`mutagen-exp-0012`), and any
future promotion of the source-freeze or stage-freeze mechanism into
the workspace is a *separate* ADR-backed decision.
