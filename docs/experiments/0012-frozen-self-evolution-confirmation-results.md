# Experiment 0012 Results — Frozen End-to-End Self-Evolution Confirmation

**Status: COMPLETE.** The run 0012-r1 executed Stages A0 → A1 → B → C → D
under the frozen protocol and reached its formal conclusion at Stage D:
**INCONCLUSIVE** (one registered integrity rule was violated by a 2 ms
budget-boundary timing artifact; the protocol then correctly blocked the
stage-D freeze and the promotion stage). This document is NOT
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

  Selected: **C2** (`net margin +27 > 0` → mutation displaces G0).
- **Integrity event:** verifier violation — episode `0012-r1-sel-023`
  recorded `wall_time_ms = 600002`, exceeding the registered 600 000 ms
  per-episode budget by 2 ms.
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
verdict it produced is INCONCLUSIVE, not SUPPORTED.

**The violation is a boundary-timing artifact, not a protocol bypass.**
Episode 023 had all nine requests *initiated* within budget; the last
in-flight request (killed at the deadline by the budget mechanism,
`http_error`) completed 2 ms past the 600 s mark, and the frozen
verifier rule `wall_time_ms > 600 000` flagged the recorded value
600 002. The rule is byte-identical to 0011's `max_time_ms` check —
0011 never exercised it because it stopped at Stage C. The strict
`>` comparison on a *recorded* wall time is fragile by construction:
an in-flight request straddling the deadline overshoots the budget
bound by up to its remaining duration (up to the 600 s request
timeout). Had the overshoot been 1 000 ms instead of 2 ms, the same
outcome would follow; the registered rule cannot distinguish the two.

**Non-formal scientific content (post-protocol-violation, exploratory
only — the 0010 precedent):** three of four independently generated
candidates beat G0 on the clean selection split (C2 +27, C1 +17,
C3 +14, all 0 losses), and C1 is a genuine independent rediscovery of
the 0010/0011 read-after-write policy. But under the frozen protocol
these numbers carry no formal weight, and Stage E (promotion, 96
episodes) **never ran**: no promotion artifacts exist for 0012.

## 0010 / 0011 / 0012 comparison

| | 0010 | 0011 | 0012 |
|---|---|---|---|
| Source freeze | promise — violated mid-run | mechanical — held | mechanical — held |
| Stage C mutation | generated (weak evidence) | **failed** (shape drift, 2/2 attempts) | **succeeded, attempt 1** (explicit schema) |
| Stage D selection | ran (invalidated) | never ran | ran 150/150; gates passed; **1 verifier violation (2 ms)** |
| Stage E promotion | ran (C1 35W/0L/13T, non-formal) | never ran | **not permitted** (stage-D freeze refused) |
| Stage-artifact provenance | partial (gap) | partial (gap) | commit-level freeze; B+C frozen, D correctly refused |
| Verdict | INCONCLUSIVE | INCONCLUSIVE | **INCONCLUSIVE** |

## What this run demonstrates

1. The four 0012 changes all behaved as pre-registered: the explicit
   schema removed the 0011 Stage-C failure mode; the commit-level
   stage freeze closed the 0011 provenance gap (and, at Stage D, did
   its job by *refusing* to freeze a violated stage); the
   no-restart/no-unregistered-call discipline held (zero unregistered
   endpoint calls; no re-runs).
2. The frozen confirmation question receives its answer: **one
   registered integrity rule — inherited verbatim from 0011 and never
   exercised by 0011 — is too strict to be survivable by a real run
   against a slow local endpoint.** The loop ran end-to-end through
   318 live episodes, but the 600 s wall-time verifier bound makes any
   run that contains one deadline-straddling request INCONCLUSIVE.
3. No formal SUPPORTED or REFUTED claim about H1 is produced. The
   0010 C1 result remains non-formal; 0012 neither confirms nor
   refutes it.

## Recommended next step (NOT part of this run)

A **0013** that registers an explicit boundary semantics for the
episode time budget — e.g., the verifier bound checks *request-initiation*
discipline (no request started after the deadline) rather than the
recorded *completion* wall time, or applies a registered tolerance to
wall time — and reruns the frozen confirmation with that rule. That
is a registered design change to a *new* experiment with a *new*
frozen commit; it is deliberately not applied retroactively here.

## Artifacts (all committed)

- `docs/experiments/artifacts/0012-discovery-trajectories.jsonl`,
  `0012-discovery-summary.json` (stage-B frozen)
- `experiments/0012-frozen-self-evolution-confirmation/mutation-input.json`
  (stage-B frozen), `stage-b-freeze.json`
- `docs/experiments/artifacts/0012-mutation-generation.json`,
  `candidate-pool.json`, `stage-c-freeze.json` (stage-C frozen)
- `docs/experiments/artifacts/0012-selection-trajectories.jsonl`,
  `0012-selection-summary.json`, `selected-candidate.json` (stage-D
  evidence; **NOT stage-frozen** — freeze refused by the verifier)
- `run-manifest.json` (source-frozen)
- No stage-D freeze manifest, no promotion artifacts, no Stage E.
