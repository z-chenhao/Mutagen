# Experiment 0014 Results: Mechanically Audited Execution Channel Confirmation

## Status

**Executed. Formal verdict: INCONCLUSIVE (final channel reconciliation
seal violation, root-caused to a defect in the frozen final-audit code;
see §4). The 0013 failure class (unregistered endpoint requests) never
occurred: the sealed ledger contains exactly the experiment's registered
traffic — 2 095 authorized / forwarded rows, 0 unauthorized, hash chain
intact, zero gateway restarts — and the 2 095 rows match the 2 095
experiment-recorded live requests one-for-one once the defect is
accounted for. Post-integrity numerical evidence is reported in full
below and remains strong and valid as *post-integrity exploratory
evidence* (promotion p ≈ 3.6 × 10⁻¹²); it is not a formally supported
generation transition.**

**Code authority.** The authoritative code-under-test for run 0014-r1 is
the frozen A0 commit `d97ba1d86ba7658932991b622c8e18093e9574c5`, as
bound by the run manifest. The post-run D1 implementation fix was
committed separately at `9d3692eca991889a335b3f6062876c155d2855f9`, after
the run had finalized and the INCONCLUSIVE verdict had already been
recorded; subsequent branch commits only clarify the historical record.
The current branch state is therefore intentionally **not** the frozen
0014-r1 source snapshot, and scientific claims about 0014-r1 must always
be evaluated against A0 `d97ba1d…`, not against the branch tip. See §5.

This document is NOT source-frozen; it records what happened, exactly,
after the frozen code commit, including the integrity event.

## 1. Run identity

| Field | Value |
|---|---|
| run_id | `0014-r1` |
| experiment_id | `0014` |
| code_under_test_commit (A0) | `d97ba1d86ba7658932991b622c8e18093e9574c5` |
| A1 commit | `9aeb097bebc20477191b025e1b9e4a2f09191038` |
| run-manifest sha256 | `0b4229419b77aecdbf5690b2b383c35812599ef67e94938708f85e9da83d9c52` |
| frozen_design_sha256 | `e86fb91e94ac5f66c7af7e5f6df4a2bbaebdaa915a20737b5d57536c75478728` |
| channel session | `e6e2c71641d45ebf182eaa9fe1c275b6` |
| channel protocol | `0014-audited-execution-channel-v1` |
| gateway | `http://127.0.0.1:18140/v1` (pid 92614, single process, no restart) |
| upstream (channel-internal) | `http://127.0.0.1:8000/v1` (sha256 `87459d11…0ed0fd`) |
| model | `incoai/Qwen3.8-27B-Splash` |
| channel-manifest sha256 | `17a1c57ddad6037f4a0e8be8f935411c8fa98a21fc1d0c3b40cbc3594862d0ae` |

Commit chain: A0 (preregistration, 23 frozen files) → A1 (run manifest +
channel manifest) → B `24533b2` → C `b679894` → D `345b864` → E artifacts
+ channel-final. `channel-state.json` and the control socket are
operational and gitignored; every other channel artifact is committed.

## 2. The audited channel, observed

The registered stage commands were the only process that ever addressed
the gateway. The sealed ledger (`channel-final.json`, 2 095 rows) breaks
down exactly as the registered schedule predicts:

| Stage | Episodes | Ledger rows (authorized/forwarded) | Request records (artifacts) | Match |
|---|---|---|---|---|
| B Discovery | 18 | 155 | 155 | exact |
| C Mutation | — (1 attempt) | 1 | 1 | exact |
| D Selection | 150 | 1 189 | 1 189 | exact |
| E Promotion | 96 | 750 | 750 | exact |
| **Total** | **264** | **2 095** | **2 095** | **2 095 / 2 095** |

- **Unauthorized rows: 0.** No probe, no `/v1/models`, no health check,
  no smoke request, no curl, no ad-hoc request of any kind ever reached
  the gateway after arming. The 0013 failure class — two unregistered
  `GET /v1/models` after A1 — did not occur; had it occurred, the ledger
  would now show it as an unauthorized row and the run would be
  mechanically INCONCLUSIVE (H4 mechanism: confirmed working).
- **Hash chain: intact** (0 chain violations in the final seal; genesis
  = manifest self-seal).
- **Gateway restarts: 0** (single pid 92614 from init through
  finalize; origin_pid continuity held at every stage gate).
- **Per-stage channel verifiers** (run at the end of each stage, before
  the stage freeze): B, C, D all reported `chain intact, zero
  unauthorized, fully reconciled` — and each stage's frozen checkpoint
  (B/C/D) matched the ledger prefix re-verified at the next stage.
- Duplicate request ids: 0. Ledger final head `18d2fb3b…862d661039`.

## 3. Stage results (post-integrity numerical evidence)

**B — Discovery (18 episodes, 155 requests, wall 480 s).** 18/18 valid;
oracle successes 4; incumbent failures 14 (gate ≥ 8 ✓); agent failures
3; infrastructure failures 0. All gates passed. The mutator received the
whitelist mutation input (`f7b2b8e9…2fe2f`); network-free tamper checks
clean.

**C — Mutation generation (1 attempt, 1 request, wall 291 s).** All four
candidates structurally valid on the first attempt (no retry needed).

| ID | Suffix (verbatim) |
|---|---|
| C1 | "After every state_write, immediately state_read the same key; if the read value differs from the intended value, repeat the same write and read until it matches." |
| C2 | "Only modify state keys explicitly required by the user's task; do not alter other keys even if they appear to influence persistence…" |
| C3 | "For conditional requests, read the condition first; if it is not met, finish without writes; if it is met, perform the requested write…" |
| C4 | "Treat a single write/read mismatch as possibly transient; retry the same intended write before changing strategy or probing with a different key…" |

C1 is a read-after-write verification policy with a retry loop. The 0013
run's C1 independently stated the same core policy in different words
("After any state_write, state_read the same key and rely on the
read-back, not the write acknowledgment."; 0.576 character-level
similarity). Per the frozen anti-seeding contract this is recorded as
**independent rediscovery of the same policy family** (0012, 0013, and
now 0014 all arrive at read-back verification under the S1/S2 silent-drop
stress) — accepted, not rejected. C4 independently rediscovered the
retry-on-mismatch policy.

**D — Selection (150 episodes, 1 189 requests, wall 5 478 s).**
Infrastructure failures 0. Common-valid cells 30 (gate ≥ 27 ✓);
G0-failures-in-common-valid 27 (gate ≥ 12 ✓). Per-candidate tallies vs
G0 on common-valid cells:

| Candidate | Wins | Losses | Ties | Net |
|---|---|---|---|---|
| **C1 (selected)** | 27 | 0 | 3 | **+27** |
| C4 | 23 | 2 | 5 | +21 |
| C2 | 20 | 0 | 10 | +20 |
| C3 | 15 | 1 | 14 | +14 |

C1 displaced G0 by the frozen lexicographic rule (net margin, no ties at
the top). Selected-candidate and stage-D freeze committed; channel
checkpoint D frozen.

**E — Promotion (96 episodes, 750 requests, wall 1 619 s).**
Infrastructure failures 0 (gate ≤ 10 % ✓). 48/48 pairs valid; G0
successes 8, selected (C1) successes 47; informative pairs 39:
**39 wins / 0 losses / 9 ties; two-sided sign-test p = 3.638 × 10⁻¹²**
(α = 0.05) ⇒ `quality_improvement`. Per-task: C1 dominates on every
promotion task (e.g. P14A 7/8 vs G0 0/8; P14B 8/8 vs 0/8). All
promotion gates passed.

**Deadline regression check (H3, carried from 0013):** across all 264
episodes / 2 095 requests: `deadline_infrastructure_failures = 0`,
`deadline_crossing_discarded_responses = 0`, `max_return_overshoot_ms =
0` in every stage summary. No 0012-style boundary violation class
recurred; H3 regression check holds.

**Cost diagnostics** (per condition; usage fields null — the 8000 local
proxy does not expose per-request token usage; `usage_reporting_requests`
counts the requests that requested usage): B 155 requests / 480 s;
C 1 / 291 s; D 1 189 / 5 478 s; E 750 / 1 619 s. Total ≈ 7 868 s
(≈ 2.2 h) of wall time, 2 095 model requests.

## 4. The integrity event (formal verdict driver)

`promote` completed all 96 episodes, wrote the promotion artifacts,
reported the conclusion, finalized the channel, and then ran the **final
channel reconciliation** — which reported a seal violation:

```
channel SEAL violations — the run is INCONCLUSIVE:
  - gateway forwarded count 2095 != experiment-recorded live request count 906
```

plus 1 189 "unmatched forwarded ledger entry" violations, all stage-D
rows 1 167+.

**Root cause (post-run analysis).** The frozen final-audit call in
`cmd_promote` built the recorded-request set as
`all_recorded(discovery, generation, [], promotion)` — the **selection
(D) record set was passed as empty**. 906 = 155 (B) + 1 (C) + 750 (E):
the D requests were omitted from the reconciliation, so the 1 189 D
ledger rows appeared unmatched and the formal count rule
(forwarded == recorded) failed. This is a **defect in the frozen
control code**, not traffic: an independent recomputation from the
committed artifacts and the sealed ledger shows the full record set is
155 + 1 + 1 189 + 750 = **2 095**, matching the 2 095 authorized/
forwarded ledger rows one-for-one (stage, request id, body hash), with
0 unauthorized rows, 0 chain violations, 0 duplicate ids.

**Why the formal verdict is nevertheless INCONCLUSIVE.** The preregistration
(§16) registers the rule: *any channel violation — including a
reconciliation mismatch in either direction — makes the run
INCONCLUSIVE*. The run's own sealed evidence
(`channel-final.json`: `unmatched_count = 1189`) does in fact contain a
reconciliation mismatch, because the sealed document was produced by the
defective audit. The control surface is applied **as frozen**: a defect
in the control code that produces a spurious violation is an integrity
event for *that run* (the same logic by which 0012's 2 ms verifier
disagreement made 0012 INCONCLUSIVE). The run identity `0014-r1` is
permanently INCONCLUSIVE; no later statement reopens it, and the stage
chain is not re-run (no-restart rule; the channel session is finalized).

**H4 status.** Split precisely:
- *The channel mechanism itself* — single long-lived gateway, per-request
  authorization, hash-chained ledger, no-restart continuity,
  stage-checkpoint prefix freezing, and the rejection-and-ledgering of
  unregistered traffic — **behaved exactly as designed across all 264
  episodes and 2 095 requests; the 0013 failure class was absent and
  would have been mechanically detected if present.**
- *The 0014 confirmation as formally registered* — **not completed**:
  the final one-to-one seal the registration requires was computed by
  defective code and failed on its face.

## 5. Post-run D1 implementation fix and code-authority boundary

The authoritative code-under-test for run 0014-r1 is the frozen A0 commit
`d97ba1d86ba7658932991b622c8e18093e9574c5`, as bound by the run manifest
(`code_under_test_commit`; its per-file hashes remain the run's
verification authority and are not updated).

After the run had already completed Stage E, the channel had been
finalized, the INCONCLUSIVE verdict had been recorded, and the results
(evidence commit `d580e112dae01608f9a731553fc79af6fb25f9fa`) committed,
commit `9d3692eca991889a335b3f6062876c155d2855f9` applied the D1 fix for
future runs. That commit modifies two files that were part of the A0
frozen source set: `src/experiment.rs` (include the Stage-D selection
records in the final `all_recorded(…)`) and `src/channel.rs` (a static
D1 regression guard in the self-test channel section). No other frozen
file changed: the A0 → evidence-commit diff over the entire frozen set is
empty, and the evidence-commit → D1-fix-commit diff is exactly those two
files (any later branch commits are documentation-only).

These post-run changes **do not alter or reopen 0014-r1**. They are
follow-up implementation state only: `0014-r1` was not re-run, the
channel session was not reopened, `channel-final.json` was not
regenerated, and no model or stage was re-executed. Any scientific claim
about 0014-r1 must be evaluated against A0 `d97ba1d…`, not against
current branch HEAD.

Accordingly:

- **Run-time freeze integrity**: during the lifetime of 0014-r1, no
  frozen source changed before the run finalized — the source-freeze
  guards were clean through run finalization.
- **Repository state after the run**: the branch contains the
  intentionally committed post-run D1 fix. These are distinct facts and
  must not be conflated.

The branch/PR therefore intentionally contains both (1) the immutable
historical evidence for 0014-r1 and (2) the post-run D1 implementation
fix (plus later documentation-only clarifications) intended for a fresh
future run. The branch tip must not be described as the frozen source
snapshot of 0014-r1, and no "source freeze passes at the branch tip" claim
is made or implied by this document.

**Verifier expectation at the current branch state.** Run-manifest
verification against the current tree (e.g. `verify-run-manifest` from the
branch tip) now
fails for `src/experiment.rs` / `src/channel.rs` — **expected**, because
HEAD contains the registered post-run fix while the manifest's authority
remains the historical A0 commit. The 0014-r1 verification authority is
A0 `d97ba1d…`; the mismatch is reported here, not hidden by changing
the verifier.

## 6. Comparison with 0013

| Dimension | 0013 (deadline-safe) | 0014 (audited channel) |
|---|---|---|
| Deadline contract | the 0013 change; H3 confirmed | carried; 0 violations (regression check ✓) |
| Endpoint discipline | **violated** (2 unregistered `GET /v1/models` after A1) | **held** (0 unauthorized rows; 2 095/2 095 reconciled) |
| Integrity failure that set the verdict | the inherited unregistered probe | the final channel-reconciliation seal (audit defect, §4) |
| Selection | C2 displaced G0 (net +27) | C1 displaced G0 (net +27) |
| Promotion | 38W/0L/10T, p ≈ 7.3 × 10⁻¹² | 39W/0L/9T, p ≈ 3.6 × 10⁻¹² |
| Formal verdict | INCONCLUSIVE | INCONCLUSIVE |

## 7. Registered defects and follow-up

1. **D1 (the defect that set the verdict): the final reconciliation
   omitted the selection (D) record set.** The frozen 0014-r1 final
   audit passed the D records as `&[]`, so it reconciled B + C + E (906)
   against the ledger instead of B + C + D + E (2 095).
   **Post-run fix status**: the D1 fix (include the selection records in
   the final `all_recorded(…)`) and its static regression guard were
   **implemented after the run** in commit `9d3692e…` (§5); `0014-r1`
   was **not re-run**, the channel was **not reopened**, and
   `channel-final.json` was **not regenerated**. The fix can only take
   effect in a **new run** (new A0, new run identity, new channel
   session; `0014-r1` is closed).
2. The sealed `channel-final.json` of this run therefore carries
   `matched_count = 906` / `unmatched_count = 1189` computed by the
   defective audit; it is preserved verbatim as evidence, with the
   corrected independent recomputation (2 095/2 095, §2) reported here
   beside it. Both facts coexist: the **sealed formal audit result
   failed** (D1), and the **independent artifact recomputation is clean**.

**Follow-up** (registered; a separate, freshly preregistered run — not
implemented here): a confirmatory re-run of the 0014 channel with the D1
fix — expected to yield the first formally clean audited-channel
confirmation (H4) in one run, given that this run already demonstrated
the mechanism end-to-end with zero unauthorized traffic.

## 8. Transparency appendix

- **Pre-A1 direct upstream checks.** Two direct upstream transport checks
  occurred **before `channel-init` / before A1** — one
  `GET http://127.0.0.1:8000/v1/models` and one
  `POST http://127.0.0.1:8000/v1/chat/completions` issued directly
  against the local 8000 proxy to verify the model transport was alive
  before the run. They are disclosed for completeness. They are
  **outside the registered post-A1 audited-channel window** (no channel
  session existed, the run identity was not yet frozen, and the
  registered 0014 endpoint is the gateway, not the proxy) and are **not**
  the formal integrity event that determines the verdict. They are **not
  part of the channel ledger** (the ledger of session `e6e2c716…` begins
  with the first Stage-B request through the gateway).
- No other direct upstream contact occurred at any point after the A1
  commit. After A1, the only process that ever touched the gateway was
  the registered stage commands; the only other gateway-adjacent
  activity was `channel-arm` / `channel-status-local` /
  `channel-finalize` over the local Unix control socket (zero HTTP).
- The gateway process (pid 92614) was left in its registered finalized
  state; it refuses all further traffic with `channel_finalized`.

## 9. Conclusion

The audited channel mechanism works: for the first time in this
project's live runs, the execution channel was a mechanical control
rather than a printed promise — 2 095 requests, all authorized, all
reconciled, zero unauthorized, zero restarts, chain intact — and the
formal verdict is **INCONCLUSIVE** only because the frozen final-audit
code that seals the run had a record-set defect (D1), which the sealed
evidence now permanently records. The numerical evidence (C1: +27 at
selection; 39W/0L, p ≈ 3.6 × 10⁻¹² at promotion; zero deadline and
zero infrastructure failures) is strong post-integrity exploratory
evidence. 0014-r1 was executed under A0 `d97ba1d…`; the current branch
HEAD contains the separately committed post-run D1 fix. The registered
next step is a new confirmatory run with the D1 fix.

## 10. Formal hypothesis interpretation

| Hypothesis | Status for 0014-r1 |
|---|---|
| Formal verdict | **INCONCLUSIVE** (permanent; §4) |
| H1 (evolvability) | **NOT FORMALLY DETERMINED** — the evidence strongly supports C1 empirically (net +27 at selection; 39W/0L at promotion), but no formal clean generation transition exists. |
| H2 (control surface) | **NOT FORMALLY SUPPORTED** — the full registered control surface produced a failing final seal (D1). |
| H3 (deadline regression) | Carried regression check **clean** (0 deadline violations across all 264 episodes); not elevated beyond the whole-run verdict. |
| H4 (audited channel) | Split: the **channel mechanism empirically behaved correctly** (2 095/2 095, 0 unauthorized, chain intact, 0 restarts, 0013 failure class absent); the **formal 0014 confirmation is INCONCLUSIVE** because the frozen final reconciliation failed on its face (D1). |

## 11. Preregistration documentation inconsistency (disclosed)

The frozen preregistration
(`docs/experiments/0014-audited-execution-channel.md`) contains a
documentation inconsistency: its opening section says final
Results/Conclusion sections *may be appended* after A0, while §19 of the
same document and the 23-file source-freeze mechanism make the
preregistration itself byte-immutable (it is one of the 23 frozen
files). **No append occurred during 0014-r1**: the preregistration
remained unchanged, and all results were written to this separate,
non-frozen results document. This wording defect did not affect execution
or the run verdict, but future preregistrations must remove the append
allowance.
