# Experiment 0012 Results — Frozen End-to-End Self-Evolution Confirmation

**Status: COMPLETE.** Run 0012-r1 progressed cleanly through Stage D —
168 agent episodes (18 discovery + 150 selection) plus one
mutation-generation request were executed under the frozen protocol. Its
formal conclusion at Stage D is **INCONCLUSIVE**: the Stage-D verifier
reported one registered integrity violation (a 2 ms
runtime-deadline/verifier-completion boundary disagreement), so the
stage-D freeze was never written and Stage E was never permitted to run.
The experiment was *designed* as an end-to-end confirmation; the run
did not complete the full B → C → D → E chain. This document is NOT
source-frozen; it records what happened, exactly, after the frozen code
commit.

## Run identity

| Field | Value |
|---|---|
| `run_id` | `0012-r1` |
| `code_under_test_commit` (A0) | `1d763b8fa034031b02c08fd81ebfb665724dfdce` |
| A1 manifest commit | `734eca0adab5f21da8b1d77047940dcd04669cbc` |
| `run_manifest_sha256` | `ee7b4b9c34172fe020619c9e799173b3de0408e7db11bc12b9c10dc3afbae9f6` |
| `frozen_design_sha256` (21 files) | `447ce20ef797dacc838975ccddd6e52f96d05ee9b01312e81de757cf4079dcda` |
| Model | `incoai/Qwen3.8-27B-Splash` @ `http://127.0.0.1:8000/v1` |
| Source freeze at run start | PASS (all 21 files, pre- and post-write) |
| Source freeze after run | PASS (re-verified after every stage) |

## Stage B — Discovery (18 episodes)

- 18 episodes, 17 valid, 1 infrastructure failure (transport; recorded
  and excluded), 4 agent failures, 5 incumbent oracle successes.
- **Incumbent failures: 12 / 18** (gate ≥ 8 ✓; valid ≥ 16 ✓).
- Verifier: clean. Gates: all passed. **Stage-B freeze** (3 artifacts)
  written and committed; Stage C verified it against Git before its
  first request.
- Wall time ≈ 462 s; 152 model requests; 138 tool calls.

## Stage C — Mutation generation (≤2 attempts)

- **1 attempt; structurally valid on the first response** — the
  explicit JSON schema (0012 change #1) produced the expected
  structure with zero validation errors. No retry was needed.
- 4 candidates (harness IDs in returned order):

  - **C1** — read-back after write: *"After any state_write that
    affects a task-required key, read that key back; finish only when
    the read shows the requested value, not when the write returns
    ok."* Rationale: writes returned `ok` while the next read still
    showed the old value. **This is an independent rediscovery of the
    0010/0011 read-after-write policy** (accepted per the anti-seeding
    contract; it is reported as rediscovery, not seeded).
  - **C2** — retry-then-read-after-write: *"If a read after a required
    write does not show the requested value, repeat the same write to
    the same key and read again; do not finalize until the read
    matches."*
  - **C3** — key discipline: touch only keys named in the task.
  - **C4** — conditional discipline: read the condition key first;
    write only when the condition holds.

- Pool-vs-raw-response check clean; no forbidden terms, no `V12_*`
  literals, no task IDs; suffixes pairwise distinct. **Stage-C freeze**
  (2 artifacts) written and committed.
- Wall time ≈ 439 s (one long single request).

## Stage D — Selection (150 episodes)

- 150 episodes (3 S12 × 10 reps × 5 conditions, exact cyclic order),
  all cells executed; cyclic order balanced; pool frozen at start.
- Infrastructure failures: 1 (episode 023, time-limit `http_error` —
  the registered budget mechanism terminated the in-flight request;
  recorded and excluded). That episode sits in cell S12A×rep5, which
  consequently lost common-validity.
- Common-valid cells: 29 of 30 (gate ≥ 27 ✓); G0 failures in
  common-valid: 27 (gate ≥ 12 ✓). All gates passed.
- Wall time ≈ 55.5 min of model wait over the 150 episodes
  (G0 12.2 min, C1 26.3 min, C2 5.5 min, C3 8.6 min, C4 3.0 min;
  per episode: C4 5.9 s < C2 10.9 s ≪ G0 24.4 s — the two read-
  confirmation policies are the fastest non-incumbent conditions); 1 017
  model requests.
- Deterministic ranking (non-tied, common-valid cells vs G0):

  | Candidate | W | L | T | net margin | exploratory p |
  |---|---|---|---|---|---|
  | C2 | 27 | 0 | 2 | **+27** | 1.49e-08 |
  | C1 | 17 | 0 | 12 | +17 | 1.53e-05 |
  | C3 | 14 | 0 | 15 | +14 | 1.22e-04 |
  | C4 | 3 | 2 | 24 | +1 | 1.00 |

  Selected: **C2** (net margin +27 > 0 under the registered
  deterministic rule). Because the Stage-D freeze was never written,
C2 is the **exploratory / provisional selection winner** — it is NOT a
  formally selected generation, NOT a promoted generation, and NOT G1.
- **Integrity event:** verifier violation — episode `0012-r1-sel-023`
  recorded `wall_time_ms = 600002` while the frozen verifier requires
  `wall_time_ms <= 600000` (a 2 ms boundary disagreement; root cause
  below).
- Consequence, per the frozen protocol: **no stage-D freeze is
  written; Stage E (promotion) must not run; the run is INCONCLUSIVE
  and is permanently finished.** The no-restart rule now mechanically
  blocks any re-run of Stage D (its trajectory file is non-empty).

## Formal conclusion

**INCONCLUSIVE.**

The 0012 confirmation question was: *with the source, the
mutation-output schema, the retry semantics, the per-stage artifact
provenance, and the endpoint discipline all frozen, does the loop
produce complete, reproducible, mechanically verifiable evidence — and
a formal SUPPORTED / REFUTED / INCONCLUSIVE verdict — in one run?*

Answer: **the frozen control surface worked exactly as designed — the
freeze is what produced the verdict.** Every guard fired on schedule
(source freeze before all four stages; B-freeze before C; C-freeze
before D; stage-D freeze correctly REFUSED by the verifier), the
explicit schema fixed the 0011 Stage-C structural failure (first-attempt
validity), and the run terminated at the first registered integrity
violation with no restart, no patching, and no continuation. The
experiment was designed as an end-to-end confirmation; the run did not
complete it, because Stage E was never permitted to run. The verdict it
produced is INCONCLUSIVE, not SUPPORTED.

**Root cause: two registered contracts disagree at the deadline
boundary (not a protocol bypass, and not a runtime overrun).** The
frozen runtime deadline semantics are: no model request may *start* at
or after the episode deadline, and each request's transport timeout is
set to `min(remaining budget, REQUEST_TIMEOUT)`. Episode 023 (10
requests; termination `http_error`; `wall_time_ms = 600002`;
`model_wait_time_ms = 600002`) obeyed both: its tenth request started
while `remaining > 0`, and its timeout was bounded by that remaining
time — the runtime did not initiate a request after the deadline and
never let a request run for an unbounded extra duration. However, a
*blocking* transport timeout can return slightly after the nominal
deadline because timeout detection, OS scheduling, and request teardown
are not instantaneous: the episode therefore recorded 600 002 ms even
though request initiation and timeout configuration respected the
runtime deadline. The verifier's *independent* contract is on the
recorded completion: `wall_time_ms <= 600 000` (byte-identical to
0011's `max_time_ms` check, which 0011 never exercised because it
stopped at Stage C). The failure is a boundary disagreement between two
registered contracts — **runtime deadline semantics** (no late request
start + timeout ≤ remaining budget) versus **verifier semantics**
(recorded final wall time ≤ 600 000 ms) — and the frozen verifier rule
`wall_time_ms > 600 000 ⇒ violation` is applied strictly: no
retroactive tolerance, no manual waiver, no verifier edit.

**Hypothesis interpretation.**
- **H1 (evolvability): NOT DETERMINED.** No promotion stage ran, so
  there is no confirmatory held-out comparison of any generated
  candidate against G0.
- **H2 (control): NOT FULLY TESTED.** The full registered protocol
  includes Stage E, which never ran. The narrower claim that stands:
  source freeze, endpoint discipline, Stage-B and Stage-C artifact
  freezes, mutation generation, selection execution, and the
  fail-closed Stage-D integrity handling all behaved as registered
  through Stage D.

**Non-formal scientific content (post-protocol-violation, exploratory
only — the 0010 precedent):** on the 29 common-valid cells, three of
four independently generated candidates beat G0 (C2 +27 / 27W-0L-2T,
C1 +17 / 17W-0L-12T, C3 +14 / 14W-0L-15T, all 0 losses; C4 +1), with
C2 the provisional exploratory winner — and C1 is a genuine
independent rediscovery of the verify-after-write policy class (fresh
V12 discovery evidence; no C1 seed; no known-repair seed; the explicit
schema affected output shape only). This is **not** formal
self-evolution evidence: under the frozen protocol these numbers carry
no formal weight, and Stage E (promotion, 96 episodes) **never ran**:
no promotion artifacts exist for 0012.

## 0010 / 0011 / 0012 comparison

| | 0010 | 0011 | 0012 |
|---|---|---|---|
| Source freeze | promise — violated mid-run | mechanical — held | mechanical — held |
| Stage C mutation | generated (weak evidence) | **failed** (shape drift, 2/2 attempts) | **succeeded, attempt 1** (explicit schema) |
| Stage D selection | ran (invalidated) | never ran | ran 150/150; measurement gates passed; **1 integrity violation; NOT frozen** |
| Stage E promotion | ran (C1 35W/0L/13T, non-formal) | never ran | **not permitted** (stage-D freeze refused) |
| Stage-artifact provenance | partial (gap) | partial (gap) | commit-level freeze; B+C frozen, D correctly refused |
| Verdict | INCONCLUSIVE | INCONCLUSIVE | **INCONCLUSIVE** |

## What this run demonstrates

1. **All four 0012 control improvements validated successfully**
   (meaningful engineering evidence): the explicit mutation-output
   schema removed the 0011 Stage-C failure mode (candidate pool valid
   on attempt 1; shape contract held); the Stage-B Git stage-freeze
   held; the Stage-C Git stage-freeze held; the source freeze held;
   endpoint discipline held (zero unregistered endpoint calls);
   no-restart held; and the Stage-D integrity verifier **failed
   closed** exactly as registered (refusing to freeze a violated
   stage). This is a valid control-system result even though the run
   verdict is INCONCLUSIVE.
2. **The run progressed cleanly through Stage D** — 168 agent
   episodes plus one mutation-generation request were executed under
   the frozen protocol — and stopped at the first registered
   integrity violation. It did **not** complete the full loop: Stage E
   (promotion, 96 episodes) was never permitted to run, and no
   0012 promotion artifacts exist.
3. **The boundary disagreement is a registered-contract gap, not a
   runtime defect and not a verifier bypass.** The runtime honored its
   deadline semantics (no late request start; timeout bounded by the
   remaining budget); the verifier honored its completion-time
   contract (recorded wall ≤ 600 000 ms); the two disagree by 2 ms at
   the timeout boundary. A 2 ms overshoot is still a violation under
   the frozen rule — the fail-closed outcome is preserved exactly.
4. No formal SUPPORTED or REFUTED claim about H1 is produced (H1
   **NOT DETERMINED**). The 0010 C1 result remains non-formal; 0012
   neither confirms nor refutes it.

## Policy-attractor observation (post-hoc diagnostic only — NOT part
of the formal verdict)

Across three consecutive controlled runs the mutation operator
converged on the same policy class:

- 0010: a verify/retry-after-write policy emerged;
- 0011: the same verify/retry policy re-emerged inside the
  schema-invalid (rejected) generation output;
- 0012: a verify/retry (C1) and retry+read-back (C2) policy class
  emerged again, with C2 the exploratory selection winner.

This repeated convergence toward the same policy class — a recurring
**mutation attractor** for silent-write faults — is a post-hoc
diagnostic observation only. It is not part of the formal verdict,
carries no formal weight, and is reported solely because three
independent runs now agree on it.

## Minimal change for the next clean confirmation (NOT part of this run)

The next experiment should change **only** the deadline semantics and
add request-level timing evidence. Everything else from 0012 carries
forward unchanged: the explicit mutation schema, the shape-aware
retry, the source freeze, the commit-level stage-artifact freeze,
endpoint discipline, the fresh V12 task splits, the selection rule,
the promotion rule, the frozen 0009 fault profile, the model,
endpoint, and temperatures. No evolvable-surface expansion.

1. **Register an explicit multi-part deadline contract**:
   (1) no model request may START at or after the episode deadline;
   (2) each model request timeout must be ≤ the remaining episode
   budget; (3) timeout-induced request termination is a valid
   infrastructure failure; (4) recorded episode completion may exceed
   the nominal deadline only by a small, pre-registered
   timeout-return / scheduling tolerance (`deadline_return_tolerance_ms`
   pre-registered in the future experiment — no value is selected
   here in 0012); (5) the verifier checks these properties directly.
2. **Record per-request timing evidence** for every model request:
   `request_started_elapsed_ms`, `requested_timeout_ms`,
   `request_finished_elapsed_ms`, `timed_out`. The verifier then
   checks, directly: `request_started_elapsed_ms < episode_deadline`;
   `requested_timeout_ms <= episode_deadline -
   request_started_elapsed_ms`; and `request_finished_elapsed_ms <=
   episode_deadline + registered_return_tolerance` — instead of
   inferring protocol compliance from one aggregate final wall-time
   field.

This is a registered design change to a *new* experiment with a *new*
frozen commit; it is deliberately not applied retroactively here.

## Artifacts (all committed)

- `docs/experiments/artifacts/0012-discovery-trajectories.jsonl`,
  `0012-discovery-summary.json` (stage-B frozen)
- `experiments/0012-frozen-self-evolution-confirmation/mutation-input.json`
  (stage-B frozen), `stage-b-freeze.json`
- `docs/experiments/artifacts/0012-mutation-generation.json`,
  `candidate-pool.json`, `stage-c-freeze.json` (stage-C frozen)
- `docs/experiments/artifacts/0012-selection-trajectories.jsonl`,
  `0012-selection-summary.json`, `selected-candidate.json` — Stage-D
  **evidence only**. The stage-D freeze manifest is **ABSENT**
  (the verifier refused to freeze the stage); these three files are
  NOT frozen selection artifacts in the formal sense.
- `run-manifest.json` (source-frozen)
- No stage-D freeze manifest, no promotion artifacts, no Stage E.
