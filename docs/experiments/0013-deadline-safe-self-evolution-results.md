# Experiment 0013 Results — Deadline-Safe End-to-End Self-Evolution Confirmation

**Status: COMPLETE.** Run 0013-r1 progressed cleanly through the full
A0 → A1 → B → C → D → E chain — 264 agent episodes (18 discovery +
150 selection + 96 promotion) plus one mutation-generation request,
**2 173 live model requests in total** — under the frozen protocol.
Formal conclusion: **SUPPORTED**, with every verifier and guard clean.
This document is NOT source-frozen; it records what happened, exactly,
after the frozen code commit.

## Run identity

| Field | Value |
|---|---|
| `run_id` | `0013-r1` |
| `code_under_test_commit` (A0) | `5926c6926664ba274f358039a2fbca6e74601896` |
| A1 manifest commit | `a3fc84c` |
| `run_manifest_sha256` | `727e8507ea522ed802ce1d441f4b790948901618d79b2856fdcccd112f770130` |
| `frozen_design_sha256` (21 files) | `45c8048ba253ba3cc163e06d69c384c020fafa60d569e1d5e7323106eddaee07` |
| Model | `incoai/Qwen3.8-27B-Splash` @ `http://127.0.0.1:8000/v1` |
| Deadline contract (design.json) | 600 000 / 600 000 / 1 000 ms |
| Source freeze | PASS at run start and re-verified before every live stage (B, C, D, E) |
| Stage-freeze chain | B → C → D freezes committed and verified by the next stage |

## Stage B — Discovery (18 episodes)

- 18 episodes, 18 valid, **0 infrastructure failures**, 3 agent failures,
  3 incumbent oracle successes.
- **Incumbent failures: 15 / 18** (gate ≥ 8 ✓; valid ≥ 16 ✓).
- Verifier: clean. **Stage-B freeze** (3 artifacts) written and
  committed; Stage C verified it against Git before its first request.
- Wall time ≈ 10.1 min; 154 model requests.
- Deadline evidence: 0 crossings, 0 discarded responses, max overshoot 0.

## Stage C — Mutation generation (≤2 attempts)

- **1 attempt; structurally valid on the first response** (the explicit
  JSON schema carried over from 0012 held again); no retry needed.
  Wall time ≈ 6.8 min (single long request, prompt 44 943 tokens).
- 4 candidates (harness IDs in returned order):
  - **C1** — read-back: *"After any state_write, state_read the same key
    and rely on the read-back, not the write acknowledgment."* **An
    independent rediscovery of the 0010/0011/0012 read-after-write
    policy — the third time this policy has emerged unseeded** (accepted
    per the anti-seeding contract; reported as rediscovery, not seeded).
  - **C2** — mismatch retry: *"If a read-back mismatches, retry the same
    state_write to that same key before modifying any other key."*
  - **C3** — same-key probe: test-write a simple value to the same key,
    verify, then retry the intended value before touching other keys.
  - **C4** — completion discipline: finish only after every requested key
    reads back as intended and every unrequested key matches its first
    observed value.
- Pool-vs-raw-response check clean; no forbidden terms, no `V13_*`
  literals, no task IDs; suffixes pairwise distinct. **Stage-C freeze**
  written and committed.

## Stage D — Selection (150 episodes)

- 150 episodes (3 S13 × 10 reps × 5 conditions, exact cyclic order); all
  30 cells common-valid (gate ≥ 27 ✓); G0 failures in common-valid: 27
  (gate ≥ 12 ✓). **0 infrastructure failures.**
- Wall time ≈ 61.4 min aggregate; 1 267 model requests.
- **The 0012 integrity-violation class did NOT recur**: no request
  started after the deadline, every recorded request's finish was within
  the registered contract, and the verifier reported zero violations.
  (In this run no request even crossed the deadline — max observed
  request finish was 290.5 s against the 600 s deadline — but the
  mechanism that makes 0012's 2 ms episode a *valid* outcome instead of
  a corruption was in place, registered, and verified on every one of
  the 1 267 selection-stage request records.)
- Deterministic ranking (non-tied, common-valid cells vs G0):

  | Candidate | W | L | T | net margin |
  |---|---|---|---|---|
  | C2 | 27 | 0 | 3 | **+27** |
  | C3 | 26 | 0 | 4 | +26 |
  | C4 | 20 | 2 | 8 | +18 |
  | C1 | 14 | 2 | 14 | +12 |

  Selected: **C2** (net margin +27 > 0 under the registered
  lexicographic rule). **Stage-D freeze** (3 artifacts) written and
  committed; C is now the formally selected candidate.

## Stage E — Promotion (96 episodes)

- 96 episodes (6 P13 × 8 reps × {G0, C2}, exact alternating order);
  48/48 valid pairs (gate ≥ 44 ✓); 0 infrastructure failures
  (rate 0 ≤ 0.10 ✓); 38 informative pairs.
- Wall time ≈ 31.5 min aggregate; 751 model requests.
- Outcomes: **C2 47 successes / G0 9 successes** (48 valid pairs);
  pairs: **38 wins, 0 losses, 10 ties**.
- Exact two-sided sign test: **p = 7.28 × 10⁻¹²** (< 0.05) ⇒
  `quality_improvement`.
- Verifier: clean; all gates passed.

## Deadline-evidence report (0013-specific, spec §14)

- Requests across the whole run: 2 173 (agents) + 1 (mutator).
- Requests started at/after the deadline: **0**.
- Requests crossing the productive deadline: **0**; discarded responses:
  **0**; max return overshoot: **0 ms** (max observed finish 290.5 s).
- Deadline infrastructure terminations (`episode_deadline_exceeded` /
  `episode_deadline_reached` / timeout `http_error`): **0**.
- **Did the 0012 2 ms violation class recur? No** — in 0012, a
  conformant runtime episode with `wall_time_ms = 600002` was a
  registered verifier violation; in 0013 the same shape is contract-
  conformant (finish within deadline + 1 s tolerance, discarded,
  infrastructure termination, no subsequent request) and is
  mechanically verified as such by `deadline_evidence_violations`
  (exercised by the preflight/self-test case suite with exactly those
  values: 599000/1000/600002 passes, 601001 fails, accepted-late fails,
  post-crossing request fails).

## 0012 comparison

| | 0012-r1 | 0013-r1 |
|---|---|---|
| B discovery | 17/18 valid, 1 infra, 12 incumbent failures | 18/18 valid, 0 infra, 15 incumbent failures |
| C mutation | 1 attempt, valid | 1 attempt, valid |
| D selection | verifier violation (2 ms wall-time boundary) → **no freeze, run dead** | verifier clean, freeze committed |
| E promotion | never ran | 38W/0L/10T, p = 7.28e-12 |
| Verdict | INCONCLUSIVE | **SUPPORTED** |

Same loop, same control surface; the single deadline-contract change
is the difference between a run that dies at Stage D on a 2 ms
boundary and a run that completes.

## Formal conclusion

**SUPPORTED.**

1. **The confirmation question (carried over from 0012):** with the
   source, the mutation-output schema, the retry semantics, the
   per-stage artifact provenance, and the endpoint discipline all
   frozen, does the loop produce complete, reproducible, mechanically
   verifiable evidence — and a formal verdict — in one run? **Yes** —
   for the first time the full B → C → D → E chain completed in one
   run with every guard, verifier, and gate clean.
2. **H1 (evolvability):** the incumbent G0 was displaced at selection
   by a candidate generated from discovery evidence (C2, net margin
   +27), and C2 survived promotion against G0 on the untouched
   promotion split with a highly significant quality improvement
   (p ≈ 7.3 × 10⁻¹², two-sided exact sign test).
3. **H2 (control):** every registered guard fired on schedule and all
   registered checks passed; the frozen machinery produced a
   defensible verdict rather than a post-hoc narrative.
4. **H3 (deadline-safety, 0013-specific):** the explicit deadline
   contract held on all 2 173 live requests — zero late starts, zero
   unbounded timeouts, zero un-contracted returns, zero discarded
   responses, and zero verifier violations. The 0012 failure mode
   (conformant deadline-straddling timeout misclassified as record
   corruption) is closed: the same evidence shape is now registered,
   kernel-enforced, and verifier-verified as a *valid infrastructure
   outcome*, and the preflight/self-test case suite pins exactly that
   semantics (the 0012 `600002` shape passes; the four invalid shapes
   fail). H3 is **confirmed for 0013-r1**.

**Scope note.** The verdict is exactly the preregistered scope: one
live endpoint, one model, one 12-task frozen registry, prompt-suffix
mutation only. The selected C2 is a **generation-1 experimental
candidate of this experiment** — not a promoted production artifact;
any promotion into the workspace remains a separate ADR-backed
decision.
