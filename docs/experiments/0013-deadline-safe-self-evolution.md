# Experiment 0013: Deadline-Safe End-to-End Self-Evolution Confirmation

## Status

**Pre-registered; NOT YET EXECUTED.**

This document is the immutable pre-registration for Experiment 0013. It is
committed at Stage A0 (the frozen code commit) and must not change after
that point except to append the final Results/Conclusion sections after the
run completes. Every executable value it states is also encoded in the
machine-readable single source of truth
[`experiments/0013-deadline-safe-self-evolution/design.json`](../experiments/0013-deadline-safe-self-evolution/design.json);
where the two differ, the run is invalid.

## 1. Why this experiment exists

Experiment 0012 was a frozen, confirmatory re-run of the 0011 loop. Its
control surface worked exactly as designed — every source-freeze and
stage-freeze guard fired on schedule — and the run died at **Stage D for a
2 ms boundary disagreement**:

- Episode `0012-r1-sel-023` (10 requests; termination `http_error`)
  recorded `wall_time_ms = 600002` against a frozen verifier rule
  `wall_time_ms <= 600000` — a violation.
- Root cause: **two registered contracts disagreed at the deadline
  boundary.** The runtime contract said "no request may *start* at or
  after the deadline, and the per-request transport timeout is
  `min(remaining budget, 600 s)`". The verifier contract said "aggregate
  wall time must not exceed 600 000 ms". But a *blocking* transport
  timeout can return slightly after its nominal expiry: timeout
  detection, OS scheduling, and connection teardown are not
  instantaneous. So a fully runtime-conformant episode (request started
  at 599 998 ms, timeout 2 ms, returned at 600 002 ms) is a **normal,
  valid** deadline-straddling timeout — and the aggregate rule
  misclassified it as record corruption. The frozen verifier then
  invalidated the stage; Stage E never ran; the verdict was
  INCONCLUSIVE.

Nothing else in 0012 failed. The frozen loop, the explicit mutation
schema, the retry semantics, the stage-freeze chain, the no-restart rule,
and the fresh-task anti-seeding all behaved as registered.

**Experiment 0013 is a narrow, confirmatory re-run of the exact 0012
pipeline with exactly ONE semantic change: explicit, internally
consistent episode-deadline semantics, backed by request-level timing
evidence.** Every other mechanism is carried over from 0012 (which itself
carried the 0010/0011 loop).

> **Question under confirmation:** with the deadline contract made
> explicit in the design authority, enforced by the kernel, and verifiable
> from per-request evidence, does the frozen loop produce complete,
> reproducible, mechanically verifiable evidence — and a formal
> SUPPORTED / REFUTED / INCONCLUSIVE verdict — in one run?

**Hypotheses** (carried over; H1 evolvability × H2 control):

- **H1** — On a live endpoint, the incumbent G0 is displaced at
  selection by a candidate generated from discovery evidence.
- **H2** — The frozen control surface (source freeze, stage-freeze
  chain, no-restart rule, verifier) mechanically produces a defensible
  three-way verdict.
- **H3 (new, 0013-specific)** — the explicit deadline contract is
  *deadline-safe*: a request that straddles the deadline is recorded and
  classified as a valid infrastructure outcome, and no deadline
  boundary disagreement of the 0012 kind (a conformant runtime behavior
  flagged as corruption) can invalidate a stage again. H3 is confirmed
  by the stage completing with a clean verifier *and* by the absence of
  the 0012 violation class; if *any* deadline-integrity violation fires,
  the 0013-specific question is REFUTED and the run is INCONCLUSIVE per
  the frozen protocol.

## 2. The one 0013 semantic change (and only this one)

**Explicit, internally consistent episode-deadline semantics with
request-level timing evidence.** Four interlocking parts:

### 2.1 The deadline contract lives in `design.json`

```json
"deadline": {
  "episode_deadline_ms": 600000,
  "request_timeout_ceiling_ms": 600000,
  "deadline_return_tolerance_ms": 1000
}
```

- `episode_deadline_ms` — the per-episode **productive** deadline: 10
  minutes from episode start (the carried-over 0011/0012 value, now a
  design field).
- `request_timeout_ceiling_ms` — the transport-timeout ceiling for any
  single request: 10 minutes (a request may never be given a timeout
  longer than the whole episode).
- `deadline_return_tolerance_ms` — the maximum allowed overshoot for a
  *returning* request: 1 000 ms. Blocking timeout detection, OS
  scheduling, and teardown are not instantaneous; a request whose
  timeout was correctly bounded by the remaining budget may return up to
  the tolerance past the deadline. Beyond that is a hard
  timing-integrity violation.

The 0012 model-client constants `REQUEST_TIMEOUT` and
`EPISODE_TIME_LIMIT_MS` are **deleted**. The only remaining named
constant in the model client is `REGISTERED_ENDPOINT` — a URL, not a
count, gate, or timing value. Preflight reports the three registered
values; the kernel, the runner, and the verifier all read them from the
same manifest. There is exactly one source of truth.

### 2.2 Kernel enforcement

- **No request may start at or after the deadline.** Before every
  request the kernel checks the elapsed time; if
  `start >= episode_deadline_ms`, the episode terminates immediately
  with `episode_deadline_reached` (registered infrastructure
  termination) and **no request record is emitted** (no request was
  made).
- **Exact registered timeout.** Each request is issued with the exact
  integer `requested_timeout_ms = min(remaining episode budget,
  request_timeout_ceiling_ms)` and passed to the model client as-is. The
  client holds no deadline constant of its own and cannot recompute it.
- **Response acceptance is a kernel decision.** After the call returns,
  the kernel compares `request_finished_elapsed_ms` against the deadline:
  - `finish <= deadline` → the response is **accepted** and may enter
    the trajectory as usual;
  - `finish > deadline` → the response is **discarded**: it is never
    pushed into the conversation, never counted as a model turn, never
    executed as a tool call, and never recorded as a final answer. The
    episode terminates with `episode_deadline_exceeded` (registered
    infrastructure termination) and **no subsequent request may be
    made** — a discarded response cannot influence *any* later behavior.
  - A transport-level timeout (`ModelError::Timeout`, structured
    classification in the model client) terminates the episode with
    `http_error`, exactly as any other transport failure in 0012.

### 2.3 Request-level timing evidence

Every `ModelRequestRecord` carries five additional fields:

| Field | Meaning |
|---|---|
| `request_started_elapsed_ms` | episode-start-relative instant the request was started |
| `requested_timeout_ms` | the exact registered per-request timeout used |
| `request_finished_elapsed_ms` | episode-start-relative instant the request returned (or timed out) |
| `timed_out` | whether the client classified the result as a transport-level request timeout |
| `response_accepted` | kernel acceptance decision (false for any discarded / failed response) |

All five are mandatory in every record. Synthetic (network-free) test
records carry deterministic values; real records carry measured values
(monotonic `Instant` against the episode start).

### 2.4 Verifier contract

The record verifier re-derives deadline validity per request
(`deadline_evidence_violations`):

1. `start >= episode_deadline_ms` ⇒ violation — a request may never
   *start* after the deadline.
2. `requested_timeout_ms == 0` ⇒ violation.
3. `requested_timeout_ms > request_timeout_ceiling_ms` ⇒ violation.
4. `requested_timeout_ms > episode_deadline_ms − start` ⇒ violation —
   the timeout must be bounded by the remaining budget.
5. `finish < start` ⇒ violation (implausible corruption).
6. `finish > episode_deadline_ms + deadline_return_tolerance_ms` ⇒
   violation (hard timing-integrity failure — e.g. a return more than 1 s
   past the deadline).
7. `finish > episode_deadline_ms` additionally requires, all of:
   `response_accepted == false`; no subsequent request record exists;
   termination is an infrastructure reason; no final answer; no tool
   call on a later turn. Any of these absent ⇒ violation.

**The 0012 aggregate rule is removed.** A *contract-conformant crossing*
— e.g. 0012's exact shape: a request started just inside the deadline
whose timeout returned 2 ms (or any value ≤ the registered 1 s
tolerance) past it — is a **valid infrastructure outcome**, not
corruption. Aggregate `wall_time_ms` is retained as diagnostic/cost
evidence only, with a loose corruption bound
(`wall_time_ms ≤ 2 × episode_deadline_ms + deadline_return_tolerance_ms`):
a conformant episode cannot exceed it, so a value beyond the bound
indicates a corrupt record. The registered per-request evidence is the
authoritative deadline check.

**Infrastructure-failure semantics (unchanged in force, now
deadline-complete):** infrastructure failures (transport / non-2xx /
malformed / `http_error` incl. timeouts, `episode_deadline_exceeded`,
`episode_deadline_reached`) are classified, recorded, and **excluded from
statistics**; their count against the registered floors is the only
statistical effect. Agent failures (turn/call limit, bad arguments, no
final answer) count as failures in the statistics. Cost summaries report
three diagnostic deadline metrics: `deadline_infrastructure_failures`,
`deadline_crossing_discarded_responses`, `max_return_overshoot_ms`
(re-derived from the request-level evidence; never gates).

### 2.5 What is deliberately NOT changed

Everything else is 0012's: the frozen source (21-file manifest), the
commit-level stage-freeze chain (B→C→D→E), the no-restart rule, the
explicit mutation-output schema and shape-informative retry, the
anti-seeding mutator whitelist, the deterministic selection rule, the
two-sided sign-test promotion, the 18/150/96 episode chain (264 total),
the three-way verdict, the artifact layout, and the 12-task registry.
The fresh 0013 task set uses new IDs (`E13*` / `S13*` / `P13*`) and new
`V13_*` state literals, disjoint from every 0008–0012 literal; the
machine literal-check is renamed to match its registered literal family.

## 3. The mechanical guards (every live stage, before any request)

Identical to 0012 §3: the 21-file **source freeze** (`src/freeze.rs`;
bytes vs manifest, vs A0 blobs, no post-A0 frozen-path commit, no
working-tree drift, A0 ancestor of HEAD) and the **previous-stage
commit-level freeze** (`src/stage_freeze.rs`; B needs only the source
freeze; C adds the B freeze; D adds the C freeze; E adds the D freeze
and a non-G0 selected candidate). Both are exercised by `self-test`
against real temporary git repositories.

## 4. Run identity

One run: `run_id = "0013-r1"`. The committed `run-manifest.json` binds
`run_id`, `code_under_test_commit` (the A0 SHA), the combined
`frozen_design_sha256`, and the per-file hashes of all 21 frozen files.
Every record and summary carries the full provenance block; verifiers
cross-check records ↔ summary ↔ current frozen files. Partial stage
aborts are not resumable (no-restart rule).

## 5. Tasks

Twelve *new* registered tasks (`tasks.json`), three families
(`direct_set`, `conditional_set`, `replacement`), two-key external states
with fresh `V13_*` literals — disjoint from every 0008–0012 literal:

| Split | Count | Tasks | Stress (frozen 0009 profile) |
|---|---|---|---|
| Discovery (E13) | 3 | E13A (direct, x), E13B (conditional, y), E13C (replacement, x) | S2 / S1 / S2 |
| Selection (S13) | 3 | S13A (direct, y), S13B (conditional, x), S13C (replacement, y) | S2 / S1 / S2 |
| Promotion (P13) | 6 | P13A–P13F, both keys per family | S2 / S1 / S2 |

The registry is structurally validated (12 tasks, 3/3/6 splits,
family-map ↔ task consistency, two-key states, registered fault keys) and
its canonical hash is part of every provenance block; the verifier
re-checks literal split-disjointness. The literal machine-check
(`contains_v13_literal`) is keyed to this stage's registered literal
family, and self-test verifies it detects `V13_*` literals and rejects
carryover literals from earlier stages (e.g. `V_D1_0`) as non-0013.

## 6. Frozen stress

The **0009 supported family-level profile, carried over unchanged**
(`stress-profile.json`; no calibration is permitted): `direct_set → S2
(drop first 2 writes)`, `conditional_set → S1 (1)`, `replacement → S2
(2)`. The repeated-silent-drop fault is the only environmental fault; it
is invisible to the model and auditable in the hidden write-attempt
ledger.

## 7. The loop (stage chain)

| Stage | Episodes | Content |
|---|---|---|
| A0 | — | frozen code commit |
| A1 | — | `run-manifest.json` (network-free) |
| B Discovery | 18 | 3 E13 × 6 reps, G0 only, frozen stress |
| C Mutation generation | ≤2 mutator requests | 4 suffix candidates (no tools) |
| D Selection | 150 | 3 S13 × 10 reps × {G0,C1..C4}, exact cyclic order |
| E Promotion | 96 | 6 P13 × 8 reps × {G0, selected}, alternating order |
| **Total** | **264** | |

Gate failures block the next stage mechanically (registered in
`design.json`); infra failures above the registered floor invalidate the
stage; any guard violation makes the whole run INCONCLUSIVE.

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
  **12 model turns / 16 tool calls** per episode; **deadline contract
  600 000 / 600 000 / 1 000 ms** (design fields); one tool call per turn
  (`parallel_tool_calls: false`, `tool_choice: "auto"`).
- Model: `incoai/Qwen3.8-27B-Splash` over the single registered endpoint
  `http://127.0.0.1:8000/v1` (named constant; no auth header; no fallback
  model; the mutation generator uses the **same** underlying model, no
  tools, issued with the registered `request_timeout_ceiling_ms`).

## 9. Mutation generation (anti-seeding contract)

Carried over from 0012: the mutator receives ONLY the strict-whitelist
mutation input (incumbent prompt, tool schemas, discovery episodes —
model-visible conversation, requested calls, model-visible results, final
state, oracle success bit); no fault internals, no selection/promotion
tasks or literals, no hidden reasoning (network-free tamper-checked).
**Nothing from Experiments 0010/0011/0012 is seeded**: no candidate text,
no hash, no hint; the 0012 run's four suffixes (C1..C4, including the
independently rediscovered read-after-write policies) must not appear in
the mutator prompt, the mutation input, or the crate sources. If the
fresh mutator *independently rediscoveries* a semantically similar
policy, that is ACCEPTED as independent rediscovery and reported as such
— never rejected post hoc for similarity.

Suffix bounds: 5–100 words, ≤800 bytes, no forbidden terms, no `V13_*`
literals, no registered task IDs, pairwise distinct after whitespace
normalization. ≤2 attempts with the shape-informative retry (0012
mechanism). **Injection is mechanically blocked**: every pool suffix must
equal the raw model response verbatim (verifier re-derives the pool from
the stored raw attempt). No file access for the mutator.

## 10. Selection rule (frozen, deterministic)

Carried over from 0012: per candidate, on the common-valid cells vs G0 —
wins / losses / ties. Lexicographic: **(net_margin ↓, wins ↓, losses ↑,
ordinal ↑)**. A mutation displaces G0 **only if** `net_margin > 0`;
otherwise G0 is retained (a valid "do not evolve" outcome ⇒ formal result
REFUTED, promotion does not run). No cost, no tokens, rationale, or model
judgment. The selected candidate is frozen into `selected-candidate.json`
(no promotion outcomes) and stage-frozen into the D commit before
promotion begins.

## 11. Promotion and the final three-way conclusion

Carried over from 0012: the frozen selected candidate is compared
pairwise against G0 on the clean promotion split. The exact two-sided
sign test over the non-tied informative pairs (p < α ⇒ significant)
yields the **final three-way conclusion**: `supported` (significant
quality improvement) / `refuted` (significant regression, or G0 retained
at selection) / `inconclusive` (everything else, including any
integrity failure, gate failure, infra overflow, or crash). Cost, cache,
and the three deadline diagnostics are reported as diagnostics only.

## 12. Failure taxonomy & inconclusive handling

Carried over from 0012, with the deadline terminations now first-class:

- **Infra failures** — `http_error` (transport / non-2xx / malformed,
  including structured request timeouts), `episode_deadline_exceeded`,
  `episode_deadline_reached`: classified, recorded, excluded from
  statistics; counts above the registered floor invalidate the stage.
- **Agent failures** (turn/call limit, bad arguments, no final answer)
  count as G0/candidate *failures* in the statistics.
- **Gate failures** block the next stage mechanically.
- **Any source-freeze or stage-freeze violation ⇒ the whole run is
  INCONCLUSIVE**, the run identity is permanently invalid, and no later
  stage may run. No resume, no patch, no second attempt at the same
  stage (no-restart rule).

## 13. Preflight / self-test (0013 additions)

`preflight` (live, network-free) and `self-test` (live, network-free)
retain all 0012 sections and add a **deadline-contract section** (shared
implementation, reported by both):

1. design.json deadline block equals 600 000 / 600 000 / 1 000.
2. No `REQUEST_TIMEOUT` / `EPISODE_TIME_LIMIT_MS` constants remain in
   `src/model.rs` (the single-source-of-truth rule).
3. All five request-timing fields serialize in `ModelRequestRecord`.
4. The 0012 aggregate wall-time rule is absent from `src/trace.rs`.
5. The §2.4 verifier cases, all network-free: valid normal request
   (start=100, timeout=1000, finish=900, accepted) passes; valid
   straddling timeout (599 000/1 000/600 002, `http_error`) passes
   **with zero violations**; valid late successful return
   (599 000/1 000/600 100, discarded, `episode_deadline_exceeded`) passes;
   late start (600 000) fails; oversized timeout (590 000/20 000) fails;
   beyond-tolerance return (601 001) fails; accepted-late response
   (600 100, accepted=true) fails; subsequent request after a crossing
   fails.
6. Kernel discard behavior (network-free): a scripted model returning
   after a short deadline is discarded — no assistant message, no tool
   call, no final answer, no subsequent request, `response_accepted =
   false` — and a zero-deadline episode makes **no** request at all
   (`episode_deadline_reached`).
7. All carried-over 0012 sections: design numbers, task registry,
   frozen stress, kernel/fault/oracle self-tests, endpoint discipline,
   source-freeze and stage-freeze git-repo scenarios, mutation schema.

## 14. Artifact layout & reporting

Identical to 0012, 0013-named: raw trajectories + summaries under
`docs/experiments/artifacts/0013-*.json[l]` (registered; excluded from
the source freeze; frozen per stage by the stage-freeze mechanism);
runtime evidence in the crate directory: `run-manifest.json` (A1),
`mutation-input.json`, `candidate-pool.json`,
`selected-candidate.json`, `stage-{b,c,d}-freeze.json`. Each live stage
commits its artifacts + freeze manifest together; the next stage verifies
that commit before making its first request.

The results document
(`0013-deadline-safe-self-evolution-results.md`) must report: run
identity + frozen hashes; per-stage episode/wall-time/cost diagnostics
including the three deadline metrics; the four candidate suffixes +
rationales (and whether any is an independent rediscovery); the mutation
attempts (including any retry and its shape errors); selection tallies +
tie-break path; promotion W/L/T + exact p + conclusion; verifier and
guard outputs for all stages; the deadline-evidence report (how many
requests crossed the deadline, max overshoot, how many responses were
discarded, and — explicitly — whether the 0012 2 ms violation class
recurred); the 0012 comparison; and any integrity events. All artifacts
are committed.

## 15. Execution protocol

1. **A0** — commit the frozen code (this commit). The 21-file freeze
   manifest and this document are in it (`print-a0` prints the exact
   git commands; `preflight` must pass before A1).
2. **A1** — `init-run`: build + commit `run-manifest.json`
   (network-free).
3. **B** — `discover`: 18 discovery episodes against the live endpoint;
   guards (source freeze) run first.
4. **C** — `generate`: mutation generation (≤2 attempts); guards (source
   + B freeze).
5. **D** — `select`: 150 selection episodes; guards (source + C freeze).
6. **E** — `promote`: 96 promotion episodes; guards (source + D freeze +
   non-G0 selection).

Each stage: refuse on any guard/verifier/gate violation; write and
commit artifacts + stage freeze on success; no restart on crash
(INCONCLUSIVE). The endpoint must be the registered local
`http://127.0.0.1:8000/v1` model; preflight must pass before A1.

## 16. Verdict rules (registered)

- **SUPPORTED** — the full B→C→D→E chain completes; all verifiers and
  guards clean; all gates pass; selection displaces G0
  (`net_margin > 0`); promotion sign test significant (p < 0.05,
  candidate better than G0).
- **REFUTED** — chain completes cleanly but the loop does not evolve:
  G0 retained at selection (net margin ≤ 0), or promotion significant in
  the *bad* direction.
- **INCONCLUSIVE** — everything else, including: any integrity
  violation (including any §2.4 deadline-evidence violation — which
  additionally REFUTES H3 for 0013), any guard failure, any gate
  failure, infra overflow, crash, or a stage never written.

## 17. Explicit non-goals

No hot reload, no core promotion, no new production abstraction:
everything here is experiment-local (`mutagen-exp-0013`), and any future
promotion of the source-freeze, stage-freeze, or deadline-contract
mechanism into the workspace is a *separate* ADR-backed decision. No
tuning of the deadline values (they are registered, not fitted), no
second endpoint, no fallback model, no task changes after A0.
