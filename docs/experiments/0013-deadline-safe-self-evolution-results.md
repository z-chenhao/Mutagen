# Experiment 0013 Results — Deadline-Safe End-to-End Self-Evolution Confirmation

**Status: execution COMPLETE, formal verdict INCONCLUSIVE (execution-channel
integrity failure).**

Run 0013-r1 executed the full A0 → A1 → B → C → D → E chain — 264 agent
episodes (18 discovery + 150 selection + 96 promotion) plus one
mutation-generation request, **2 172 agent model requests + 1 mutator
request = 2 173 total live model requests** — and every stage's verifier,
guard, and gate was clean. However, a post-hoc execution-history audit
(§4 below) established that, after A1 and before Stage B, **one ad-hoc
endpoint availability probe issued two unregistered HTTP GET requests to
`/v1/models`** on the registered endpoint, outside the registered stage
path. Under the inherited 0012 execution-channel
discipline ("after this commit: no unregistered live model requests …
start Stage B directly"), the run is not a formally clean run. The
formal verdict is therefore **INCONCLUSIVE**, not SUPPORTED. The
numerical evidence recorded here remains valid and strong as
**post-integrity exploratory evidence**; it is not a formally supported
generation transition.

This document is NOT source-frozen; it records what happened, exactly,
after the frozen code commit, including the integrity failure.

## 1. Run identity

| Field | Value |
|---|---|
| `run_id` | `0013-r1` |
| `code_under_test_commit` (A0) | `5926c6926664ba274f358039a2fbca6e74601896` |
| A1 manifest commit | `a3fc84c35a7a9e0e522c6ad23399bffe2d5c3afe` |
| `run_manifest_sha256` | `727e8507ea522ed802ce1d441f4b790948901618d79b2856fdcccd112f770130` |
| `frozen_design_sha256` (21 files) | `45c8048ba253ba3cc163e06d69c384c020fafa60d569e1d5e7323106eddaee07` |
| Model | `incoai/Qwen3.8-27B-Splash` @ `http://127.0.0.1:8000/v1` |
| Deadline contract (design.json) | 600 000 / 600 000 / 1 000 ms |
| Source freeze | PASS at run start and re-verified before every live stage (B, C, D, E) |
| Stage-freeze chain | B → C → D freezes committed and verified by the next stage |
| Artifact chain | B commit `96ad095`, C commit `9926bdb`, D commit `2ebd84a`, evidence commit `b67d681` |

## 2. Stage evidence (all internally verified; see §4 for the integrity failure)

### Stage B — Discovery (18 episodes)

- 18 episodes, 18 valid, **0 infrastructure failures**, 3 agent failures,
  3 incumbent oracle successes.
- **Incumbent failures: 15 / 18** (gate ≥ 8 ✓; valid ≥ 16 ✓).
- Verifier: clean. **Stage-B freeze** (3 artifacts) written and
  committed (`96ad095`); Stage C verified it against Git before its first
  request.
- Wall time ≈ 10.1 min; 154 model requests.
- Deadline evidence: 0 crossings, 0 discarded responses, max overshoot 0.

### Stage C — Mutation generation (≤2 attempts)

- **1 attempt; structurally valid on the first response** (the explicit
  JSON schema carried over from 0012 held again); no retry needed.
  Wall time ≈ 6.8 min (single long request, prompt 44 943 tokens).
- 4 candidates (harness IDs in returned order):
  - **C1** — read-back: *"After any state_write, state_read the same key
    and rely on the read-back, not the write acknowledgment."* C1 is the
    **third independent rediscovery** of the verify-after-write policy
    class, making this the **fourth unseeded emergence overall** across
    Experiments 0010–0013 (0010: initial unseeded emergence; 0011: first
    independent rediscovery; 0012: second; 0013: third). Accepted per
    the anti-seeding contract; **post-hoc diagnostic only — it does not
    affect the formal verdict**.
  - **C2** — mismatch retry: *"If a read-back mismatches, retry the same
    state_write to that same key before modifying any other key."*
  - **C3** — same-key probe: test-write a simple value to the same key,
    verify, then retry the intended value before touching other keys.
  - **C4** — completion discipline: finish only after every requested key
    reads back as intended and every unrequested key matches its first
    observed value.
- Pool-vs-raw-response check clean; no forbidden terms, no `V13_*`
  literals, no task IDs; suffixes pairwise distinct. **Stage-C freeze**
  written and committed (`9926bdb`).

### Stage D — Selection (150 episodes)

- 150 episodes (3 S13 × 10 reps × 5 conditions, exact cyclic order); all
  30 cells common-valid (gate ≥ 27 ✓); G0 failures in common-valid: 27
  (gate ≥ 12 ✓); G0 successes in common-valid: 3. **0 infrastructure
  failures.** 1 267 model requests; wall time ≈ 61.4 min aggregate.
- **The 0012 integrity-violation class did NOT recur**: no request
  started after the deadline, every recorded request's finish was within
  the registered contract, and the verifier reported zero violations on
  all 1 267 selection-stage request records. (In this run no request
  even crossed the deadline — the mechanism that makes 0012's 2 ms
  episode a *valid* outcome instead of a corruption was in place,
  registered, and verified, but was not exercised by a live crossing;
  see §6.)
- Deterministic ranking (non-tied, common-valid cells vs G0):

  | Candidate | W | L | T | net margin |
  |---|---|---|---|---|
  | C2 | 27 | 0 | 3 | **+27** |
  | C3 | 26 | 0 | 4 | +26 |
  | C4 | 20 | 2 | 8 | +18 |
  | C1 | 14 | 2 | 14 | +12 |

  Selected: **C2** (net margin +27 > 0 under the registered
  lexicographic rule). **Stage-D freeze** (3 artifacts) written and
  committed (`2ebd84a`); C2 is frozen in `selected-candidate.json`.

### Stage E — Promotion (96 episodes)

- 96 episodes (6 P13 × 8 reps × {G0, C2}, exact alternating order);
  48/48 valid pairs (gate ≥ 44 ✓); 0 infrastructure failures (rate 0 ≤
  0.10 ✓); 38 informative pairs. 751 model requests; wall time ≈ 31.5
  min aggregate.
- Outcomes: **C2 47 successes / G0 9 successes** (of 48 valid pairs);
  pairs: **38 wins, 0 losses, 10 ties**.
- Exact two-sided sign test: **p = 7.275957614183426 × 10⁻¹²**
  (= 2/2³⁸ over the 38 non-tied informative pairs; < 0.05 ⇒
  `quality_improvement`).
- Verifier: clean; all gates passed.

## 3. Request counts (corrected)

| Stage | Model requests |
|---|---|
| B discovery | 154 |
| D selection | 1 267 |
| E promotion | 751 |
| **Agent subtotal** | **2 172** |
| C mutation generation | 1 |
| **Total live model requests** | **2 173** |

## 4. Execution-history audit — the endpoint check (integrity failure)

**Findings.** The execution transcript for run 0013-r1 establishes,
exactly, the following sequence after the Stage-A1 commit
(`a3fc84c`) and before Stage B began:

1. `verify-run-manifest` — network-free local verification (registered
   command).
2. An availability check, executed as:
   ```sh
   curl -s -o /dev/null -w "%{http_code}" --max-time 5 \
     http://127.0.0.1:8000/v1/models
   curl -s --max-time 5 http://127.0.0.1:8000/v1/models | head -c 300
   ```
   Both returned successfully (`200`; the registered model's metadata
   list).
3. `cargo run -- discover` — registered Stage B.

**Classification.** The single availability-probe action **issued two
HTTP requests to the registered endpoint** — two read-only
`GET /v1/models` metadata queries: no inference call, no prompt or task
data sent, no state mutation, and nothing from either entered any
artifact. Both requests were read-only model-list metadata queries and
did not affect experiment artifacts or model inference state. However,
the inherited channel discipline did not permit unregistered endpoint
access after A1 — the 0012 discipline (which 0013 carried forward) is
"no unregistered endpoint calls … the only live commands are the four
registered stages", and the Stage-A1 output itself stated: *"AFTER THIS
COMMIT: no unregistered live model requests (zero smoke/debug requests);
start Stage B directly."* — so both requests constitute an
execution-channel integrity violation. One ad-hoc endpoint-probe action
consisting of two HTTP GET requests; **execution-channel integrity is
therefore failed because two unregistered endpoint requests occurred
outside the registered B/C/D/E stage path**.

**Protocol-documentation weakness (recorded honestly).** Experiment 0013
intended to carry forward the 0012 endpoint-discipline control
unchanged, but the 0013 preregistration did not restate the "no
unregistered endpoint calls after A1" condition with the same
explicitness as 0012 — the frozen 0013 text says "everything else is
0012's" rather than repeating the rule verbatim. The execution-history
audit therefore matters: the formal record must distinguish the intended
inherited control from what was textually and mechanically enforced in
0013 itself. (The 0013 crate's own `self-test` checks the *binary's*
command table for smoke/probe commands — which is clean — but nothing in
0013 mechanically policed operator-side ad-hoc endpoint probes; that
police was always a human/agent discipline, and it was broken here.)
The frozen preregistration is NOT modified; this weakness is recorded
here and should be fixed in future preregistrations.

**Consequence, per the frozen protocol's own spirit and per the
registered verdict rules** (any integrity failure ⇒ INCONCLUSIVE): the
run cannot receive the formal SUPPORTED verdict, and **no re-run under
run identity `0013-r1` is permitted** (the no-restart rule; the stage
trajectories are non-empty). The protocol is NOT patched retroactively:
the registered verdict taxonomy already maps "any integrity failure" to
INCONCLUSIVE, and this audit is the evidence that maps here.

## 5. Formal conclusion

**INCONCLUSIVE** — execution-channel integrity failure (one ad-hoc
endpoint availability probe after A1, before Stage B, that issued two
unregistered HTTP GET requests to `/v1/models`; §4).

- **H1 (evolvability):** **NOT FORMALLY DETERMINED** under a clean run.
  The numerical evidence strongly favors evolution — C2 displaced G0 at
  selection (net +27; 27W/0L/3T) and promoted 47/48 vs G0 9/48,
  38W/0L/10T, p = 7.275957614183426 × 10⁻¹² — but the run cannot serve
  as the first formally clean autonomous generation transition, because
  two unregistered endpoint requests occurred after A1 and outside the
  registered stage path. There is **no formal G0 → C2 generation
  transition**; C2 remains a generation-1 *exploratory* candidate of
  this experiment.
- **H2 (control):** **NOT SUPPORTED** — control integrity violated. The
  *mechanical* controls all worked (source freeze, stage-freeze chain,
  no-restart rule, verifiers, gates all clean and on schedule; the run
  even died-in-spirit correctly: the moment this audit is recorded, the
  no-restart rule permanently blocks any 0013-r1 continuation), but the
  *channel* discipline — the one control that is a human/agent rule
  rather than a mechanical guard — was broken.
- **H3 (deadline-safety):** the implementation evidence remains valid:
  the registered deadline contract was enforced by the kernel and
  verified from the request-level evidence on **all 2 172 agent
  requests** (zero violations, §6). But the whole 0013 run cannot
  receive the formal SUPPORTED verdict, so H3 is reported as
  *implementation evidence valid, overall run integrity failed* — not
  as a confirmed formal result.

**Scope note.** C2 is an experiment-local candidate of this experiment
— not a production or workspace artifact, and any promotion into the
workspace remains a separate ADR-backed decision.

## 6. Deadline-evidence report (0013-specific)

Across the **2 172 agent requests** in Stages B, D, and E, the
request-level deadline verifier found zero late starts, zero oversized
timeouts, zero uncontracted returns, zero accepted late responses, and
zero deadline-integrity violations. Stage C additionally issued **one
mutator request** (a single tools-less generation call issued with the
registered request-timeout ceiling), bringing the total live
model-request count to 2 173; the per-episode deadline contract applies
to agent episodes, and the mutator request is reported separately.

- Requests started at/after the deadline: **0**.
- Requests crossing the productive deadline: **0**; discarded
  responses: **0**; max return overshoot: **0 ms**; **max observed live
  request finish ≈ 290.5 s** against the 600 s deadline.
- Deadline infrastructure terminations
  (`episode_deadline_exceeded` / `episode_deadline_reached` / timeout
  `http_error`): **0**.
- **The deadline-straddling behavior was tested mechanically and
  synthetically, not observed live in 0013-r1.** The registered
  network-free case suite (preflight + self-test, deterministic)
  exercises exactly the 0012 `599000/1000/600002` shape (passes as a
  valid infra outcome) and the four invalid shapes (late start,
  oversized timeout, beyond-tolerance return, accepted-late response,
  post-crossing request — all fail), and the kernel discard behavior is
  exercised against scripted late responses. The live run itself never
  produced a crossing, so the live evidence is "contract held on every
  observed request", not "a crossing was observed and handled".
- **Did the 0012 2 ms violation class recur? No** — in 0012 a
  conformant runtime episode with `wall_time_ms = 600002` was a
  registered verifier violation; in 0013 the same shape verifies clean
  by construction (conformant crossing within the 1 s return tolerance,
  response discarded, infrastructure termination, no subsequent
  request), and no such violation occurred among the 2 172 agent
  requests.

## 7. 0012 comparison

| | 0012-r1 | 0013-r1 |
|---|---|---|
| B discovery | 17/18 valid, 1 infra, 12 incumbent failures | 18/18 valid, 0 infra, 15 incumbent failures |
| C mutation | 1 attempt, valid | 1 attempt, valid |
| D selection | verifier violation (2 ms wall-time boundary) → no freeze, run dead | verifier clean, freeze committed |
| E promotion | never ran | 38W/0L/10T, p = 7.275957614e-12 |
| Channel discipline | held (the run died at the *verifier*, which is a registered control) | **violated** (unregistered models-list probe after A1, §4) |
| Verdict | INCONCLUSIVE | **INCONCLUSIVE** (different root cause) |

The single deadline-contract change is validated as intended: the 0012
failure mode (a conformant deadline-straddling timeout misclassified as
corruption) is closed, and the 0013 run completed end-to-end with every
*registered* control clean. The new failure is a different, previously
unpoliced class — operator-side ad-hoc endpoint probing — which the
frozen controls of 0013 were never designed to detect.

## 8. Frozen-documentation limitations (recorded, not patched)

1. **Preregistration append-vs-byte-freeze contradiction.** The frozen
   preregistration contains a documentation inconsistency: it says
   Results/Conclusion may be appended after A0, while the source-freeze
   mechanism hashes that same document and therefore makes it
   byte-immutable. No append occurred in 0013-r1; all run results were
   correctly written to this separate non-frozen results document. This
   did not affect execution or the frozen source state, but future
   preregistrations should remove the append allowance.
2. **Endpoint-discipline restatement.** See §4: the 0013 preregistration
   inherited the 0012 "no unregistered endpoint calls" rule by reference
   ("everything else is 0012's") instead of restating it verbatim; the
   run's channel failure occurred in exactly that documentation gap.
   Future preregistrations should restate channel discipline explicitly
   and define its audit requirement (execution-history review before a
   SUPPORTED verdict).

Neither issue invalidates the frozen source state; both are recorded
here because the frozen text may not be edited.

## 9. Disposition

- **No re-run under `0013-r1`** (no-restart rule; trajectories
  non-empty).
- **No retroactive protocol patch** (frozen files unchanged).
- The evidence above stands as strong **post-integrity exploratory
  evidence** for the verify-after-write policy class and for the
  deadline-contract mechanism.
- The natural follow-up (a separate, separately pre-registered
  experiment) would add an explicit, mechanically audited
  endpoint-channel control (e.g. an egress log / proxy gate that the
  run-verifier checks) and re-execute the confirmation under a fresh
  run identity. That is out of scope for this record.
