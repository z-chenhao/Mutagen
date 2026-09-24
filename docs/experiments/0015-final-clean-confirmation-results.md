# Experiment 0015 Results: Final Clean Audited Self-Evolution Confirmation

## Status

**Executed (Stages A0, A1, B, C, D; E not run). Formal verdict:
INCONCLUSIVE** — Stage D's sealed-channel one-to-one reconciliation
failed: **9 authorized/forwarded Stage-D ledger rows have no matching
experiment record**, and the selection common-valid gate failed (21 < 27
cells, a direct consequence of the same 9 infrastructure failures).
Root cause: a **latent defect carried byte-identical from the 0013/0014
channel integration, first triggered by real transport failures** (the
frozen kernel stamps the audited-channel binding into a model-request
record only on the `Ok` path; a request that fails after the gateway
forwards it is recorded with an *empty* binding, orphaning its ledger
row). The channel mechanism itself behaved correctly: 0 unauthorized
rows, hash chain intact, zero gateway restarts — and it *detected* the
inconsistency and mechanically invalidated the run, exactly as
registered. Per the frozen 0015 protocol, a frozen-code defect
discovered after A0 makes the run INCONCLUSIVE; **no patch, no restart,
no re-run** was applied. The run's numerical evidence is reported in
full below as *post-integrity exploratory evidence only*.

This document is NOT source-frozen; it records what happened, exactly,
after the frozen code commit.

## 1. Run identity

| Field | Value |
|---|---|
| run_id | `0015-r1` |
| experiment_id | `0015` |
| code_under_test_commit (A0) | `5598d31d33329aed663ab6dc3bd41982920445c2` |
| A1 commit | `38a53f15a46d3fc15575ebaf1f21ae110b70786d` |
| B freeze commit | `864f4a15c3157f2eb7652c467b84c231ba3f33d9` |
| C freeze commit | `ec2ab99989f30e965844cbbd61f038c6444ce75c` |
| D freeze commit | **absent** (D gates failed; no D freeze written) |
| frozen_design_sha256 | `faa5b5674ee6b7252b8109e1a41a5a73f1f2f47b5633bdb9954f47dc547057b4` |
| channel session | `353606e0493ceae8a7217cbeaf568ca0` |
| channel protocol | `0015-audited-execution-channel-v1` |
| gateway | `http://127.0.0.1:18150/v1` (pid 82153, single process, no restart) |
| upstream (channel-internal) | `http://127.0.0.1:8000/v1` (sha256 `87459d11…0ed0fd`) |
| model | `incoai/Qwen3.8-27B-Splash` |
| channel-manifest sha256 | `6e3bcbd8be1520f8af8aae5b6ed57757e5fe48fdf4171890526da135ef889c14` |
| channel final seal head | `dbed2085251e0c8f4b7cfd4ad9853cc63e8cbfee2c57467335fa64ac5e5b639e` |

Commit chain: A0 (preregistration, 23 frozen files + README + gitignore)
→ A1 (run manifest + channel manifest) → B `864f4a1` → C `ec2ab99` →
D evidence + channel final seal (this commit). `channel-state.json` and
the control socket are operational and gitignored.

## 2. Source integrity (final audit)

- post-A0 commits touching the 23 frozen files: **0**
- working-tree drift on frozen paths: **0**
- current frozen bytes == A0 bytes (all 23 files re-hashed): **true**
- post-A0 source fixes: **none** (the 0014-style D1 post-run fix
  pattern was explicitly prohibited in 0015 and was not needed — D1
  itself was already on the frozen source; the defect found, D2 below,
  is documented only)

## 3. Stage-freeze provenance

- Stage B freeze: committed, verified at C (`stage-b-freeze.json`).
- Stage C freeze: committed, verified at D (`stage-c-freeze.json`).
- Stage D freeze: **not written** (D gates failed — see §6); the C
  freeze's ledger-prefix checkpoint therefore differs from the current
  ledger tail by the (correctly unfrozen) Stage D append — `verify-
  stage-freeze` reports exactly this and the missing D freeze, as
  designed for a run that stopped at D.
- Verifiers: `verify-discovery` OK, `verify-mutation` OK,
  `verify-selection` OK (record-level), `verify-promotion` reports the
  (expected) absence of promotion artifacts.

## 4. The audited channel, observed

Sealed ledger: **1 350 rows** (gateway `channel-final.json`):

| Stage | Ledger rows (authorized/forwarded) | Recorded (artifacts) | Match |
|---|---|---|---|
| B Discovery | 154 / 154 | 154 | **exact** |
| C Mutation | 1 / 1 | 1 | **exact** |
| D Selection | 1 195 / 1 195 | 1 186 | **9 UNMATCHED** |
| E Promotion | 0 / 0 | 0 (stage not run) | — |
| **Total** | **1 350 / 1 350** | **1 341** | **1 341 / 1 350** |

- **Unauthorized rows: 0.** No readiness probe, no `/v1/models`, no
  smoke request, no ad-hoc request of any kind ever reached the gateway
  (the 0013 failure class did not occur; §25 no-probe discipline held).
- Hash chain: intact — 0 chain violations at the final seal (genesis =
  manifest self-seal `b142bf84…f05f9c`).
- Gateway restarts: **0** (single pid 82153 from init through
  finalize; origin-pid continuity held at every stage gate).
- Duplicate request ids: 0.
- Final seal counts at gateway seal time: forwarded 1 350,
  unauthorized 0, chain violations 0, restarts 0, finalized `true`.
  The sealed document's reconciliation fields are left unpopulated
  because the frozen final reconciliation (the `cmd_promote` tail,
  which includes the D1 fix) runs only after Stage E — which was never
  reached.

**Independent recomputation** (re-derived from the committed artifacts,
`(stage, request_id, request_body_sha256)` one-to-one against the
sealed ledger): B 154/154, C 1/1, D **1 186/1 195 (9 unmatched
forwarded rows)**, E 0/0 → total 1 341 recorded vs 1 350 forwarded.
The sealed final document and the independent recomputation **disagree
on reconciliation** — in a formally clean run they must agree; they do
not. This is the formal INCONCLUSIVE driver.

**The 9 unmatched rows** (all Stage D, all `POST /v1/chat/completions`,
all `authorized: true, forwarded: true`, all `status_class: "0xx"` —
the upstream local proxy dropped the connection before any HTTP status
was received, 1.7–2.2 s after request start; none is a structured
timeout): ledger rows 197, 227, 301, 513, 691, 1097, 1175, 1202, 1277
(request ids `0015-r1-D-000042/72/146/358/536/942/1020/1047/1122`).
Each corresponds to exactly one experiment record with an **empty**
channel binding (`channel_request_id: ""`, `request_body_sha256: ""`,
`response_accepted: false`, termination `http_error`): episodes
`sel-005` (S15A C4), `sel-009` (S15A C4), `sel-016` (S15A C3),
`sel-037` (S15A C3), `sel-059` (S15B C4), `sel-121` (S15C C4),
`sel-130` (S15C C4), `sel-134` (S15C C4), `sel-142` (S15C C4) — 7
failed on the *first* request of the episode, 2 (sel-016, sel-037)
failed on a *later* request after 10–11 successful bound requests.
(7+2 = 9; 1 186 bound + 9 unbound = 1 195 D ledger rows.) B and C had
zero `0xx` rows.

## 5. Root cause — D2: the failure path drops the channel binding

In `src/kernel.rs` `run_episode` (byte-identical 0013 → 0014 → 0015,
frozen at A0), the audited-channel binding is returned by the model
boundary as `Result<(ModelResponse, ChannelBinding), ModelError>` and is
stamped into the `ModelRequestRecord` **only in the `Ok` arm**. In the
`Err` arm (transport failure / non-2xx / malformed), the record is
pushed with the binding left empty — but on the real channel path the
gateway has *already* allocated the request id, authorized, and
**ledgered** the forwarded attempt (rows with `status_class "0xx"`
when the upstream transport fails). Consequently, **any model request
that fails after being forwarded orphans exactly one ledger row that no
experiment record can match**, breaking the registered one-to-one
reconciliation in both directions.

This defect is latent in 0013, 0014, and 0015 alike. 0014-r1 never
triggered it (0 of its 2 095 forwarded requests failed at the transport
level). 0015-r1 is the first run in which the local 8 000 proxy
actually dropped connections mid-stage (9 of 1 195 Stage-D requests),
so the defect's first observable consequence is on this run. It was
discovered **after A0**; per the frozen 0015 protocol (§3/§13/§17) the
run is INCONCLUSIVE and no fix, patch, modify-commit-revert, or re-run
is permitted or was applied. The fix itself (stamp the channel binding
on the `Err` path too — the binding is allocated client-side before
send) is a candidate for a *separate* task; it is deliberately **not**
implemented in this branch.

Note that D2 is *not* the D1 defect: the D1 fix (final reconciliation
over B + C + D + E, with its static regression guard) is present on the
frozen source and its synthetic tests pass; D1 was **not** the driver
here. The driver is the earlier, still-latent D2 accounting gap
activated by real infrastructure flakiness.

## 6. Stage results (post-integrity numerical evidence — exploratory only)

**B — Discovery (18 episodes, 154 requests, wall 932 s).** 18/18
valid; 0 infrastructure; 8 agent failures; 1 oracle success; **17
incumbent failures** (gate ≥ 8 ✓; valid gate ≥ 16 ✓). Channel B: 154/
154 exact. B frozen.

**C — Mutation generation (1 attempt, 1 request, wall 346 s).** All
four candidates structurally valid on the first attempt (no retry).
Channel C: 1/1 exact. C frozen.

**D — Selection (150 episodes, 1 195 requests, wall 3 474 s).**
9 infrastructure failures (all `http_error`, all transport drops,
§4); 18 agent failures; 79 oracle successes. **Common-valid cells 21
(gate ≥ 27 ✗)** — the 9 failures hit 9 distinct cells (S15A reps 1, 2,
4, 8; S15B rep 2; S15C reps 5, 6, 7, 9). G0 on common-valid: 6
successes / 15 failures (gate ≥ 12 ✓). Per-candidate tallies vs G0 on
the 21 common-valid cells:

| Candidate | W | L | T | Net |
|---|---|---|---|---|
| **C2 (would have been selected)** | 15 | 0 | 6 | **+15** |
| C1 | 7 | 0 | 14 | +7 |
| C4 | 8 | 4 | 9 | +4 |
| C3 | 7 | 4 | 10 | +3 |

Frozen ranking `C2 > C1 > C4 > C3`; C2 net margin +15 > 0 → a
non-G0 selection was made and frozen into `selected-candidate.json`
before the channel check ran. **But** the Stage-D channel verifier
then reported the 9-row reconciliation mismatch (§4) and the
`common_valid_sufficient` gate failed → **the run is INCONCLUSIVE;
Stage E (promotion) was never run.** The selection tallies are
post-integrity exploratory evidence only.

**E — Promotion: not run** (96 episodes / 48 pairs never executed).

## 7. Deadline integrity (H3 regression)

Across all 1 349 agent model requests (154 B + 1 195 D) plus the 1
mutator request (1 350 total): **late starts 0; oversized timeouts 0;
deadline crossings 0; accepted late responses 0; deadline
infrastructure failures 0** — the 0013/0014 deadline contract and all
of its verifier rules behaved as registered on the fresh run.
**H3: clean.**

## 8. Anti-seeding audit

- Historical candidate text seeded into the mutator: **false** (the
  mutator saw only: incumbent prompt, tool schemas, valid fresh E15
  discovery evidence, oracle success/failure, model-visible
  conversation/tool calls/results, final state — the strict
  `mutation-input.json` whitelist; tamper checks clean).
- Historical performance / promotion results seeded: **false**.
- No "read-after-write" / "verify after write" / "retry mismatch" hints
  present in the mutation input.

## 9. The four candidates (verbatim) and post-hoc attractor analysis

| ID | Suffix (verbatim) |
|---|---|
| C1 | "After every state_write, read back the same key before proceeding; do not treat a write response as confirmation." |
| C2 | "If a read-back after a write does not match the intended value, retry the same write a few times before changing any other key." |
| C3 | "Only write to keys the user explicitly requests; if a diagnostic write to another key is unavoidable, restore its previously observed value before finishing." |
| C4 | "If the available state has only a few keys, read all of them before the first write and before the final answer." |

**Post-hoc (not formal evidence):** C1 is the *read-back
verification* policy and C2 the *retry-on-mismatch* policy — the same
two families 0012, 0013, and 0014 independently arrived at under the
S1/S2 silent-drop stress (0014 C1 = read-after-write verification;
0014 C4 = retry-on-mismatch). Under the frozen anti-seeding contract
this is reported only after the verdict is fixed, as a **recurring
mutation attractor** of the environment (the silent-drop fault makes
write-acknowledgment unreliable, so a policy that distrusts write
acknowledgments and verifies by read-back is the dominant local optimum
for the mutator). C3 (key minimalism / restore-before-finish) and C4
(bulk-read the small state) are new family members. Nothing from
history influenced the selection itself.

## 10. Hypotheses

- **H1** (incumbent displaced by a discovery-generated candidate):
  **NOT FORMALLY DETERMINED** — the full B→C→D→E chain did not
  complete; the (exploratory) D selection would have displaced G0
  (C2, net +15), but E never ran, so no formal promotion evidence
  exists.
- **H2** (the frozen control surface mechanically produces a defensible
  three-way verdict): **SUPPORTED** — source freeze, stage-freeze
  chain, no-restart rule, deadline contract, and the audited channel
  together (a) let B and C complete cleanly with exact channel
  reconciliation, (b) detected the Stage-D reconciliation mismatch and
  the common-valid gate failure, (c) refused the D freeze and the
  promotion, and (d) produced the registered INCONCLUSIVE verdict with
  complete, tamper-evident evidence — with zero unauthorized rows and
  zero gateway restarts.
- **H3** (deadline contract regression): **SUPPORTED (clean)** — 0
  deadline violations across 1 350 requests (§7).
- **H4** (channel audit-safety, 0014 definition): **NOT FORMALLY
  SUPPORTED** — the *detection* mechanisms all worked (any
  unauthorized row, chain break, or restart would have been observed;
  the 0013 failure class never occurred: 0 unauthorized rows), but the
  registered clean-run condition — exact one-to-one reconciliation —
  was not met (9 unmatched rows, §4), which the registered verdict
  rules map to INCONCLUSIVE. The D1 *final* reconciliation (B + C + D +
  E) was additionally never executed live (it runs only after Stage
  E); it is covered by the static guard and the synthetic B+C+D+E /
  D-omitted self-tests, both of which pass.

## 11. Verdict and its consequences

**Formal verdict: INCONCLUSIVE** (channel reconciliation mismatch +
common-valid gate failure, both downstream of 9 real transport
failures hitting the D2 frozen-code accounting gap).

- Formal generation transition: **none**.
- Model/stage re-runs: **none** (no-restart rule; the run identity
  0015-r1 is permanently invalid as a formal run).
- Post-A0 source fixes: **none**.
- Direct readiness probes: **none** (no `curl`, no `/v1/models`, no
  smoke request, no health check — at any point in the run).
- The current toy confirmation line is **not** complete: 0015 is the
  line's third INCONCLUSIVE confirmatory run (after 0013 and 0014) and
  the second consecutive run in which the audited channel's empirical
  behavior was clean (0 unauthorized / 0 restarts / chain intact) while
  a *frozen code* accounting defect (D2) — not the channel mechanism —
  produced the formal INCONCLUSIVE. The next work item (a separate
  task, for human review) is a channel-integration fix on a *new* run
  identity (stamp the client-allocated binding on the request-failure
  path, or classify post-forward failures so reconciliation holds),
  re-audited against the same one-to-one contract; only a fully clean
  run can complete the line. Nothing here is promoted to
  production/core.

## 12. Cost / timing diagnostics

Token usage is not exposed by the 8 000 local proxy (all usage fields
null; `usage_reporting_requests` = 154 B / 1 195 D / 1 C). Wall time:
B 932 s, C 346 s, D 3 474 s (per condition: G0 659 s, C1 958 s, C2
219 s, C3 980 s, C4 657 s); total ≈ 4 752 s. B oracle successes 1;
D oracle successes 79.

## 13. Validation performed after run completion (all network-free)

Workspace: `cargo fmt --check` / `cargo check --workspace` /
`cargo test --workspace` / `cargo clippy --workspace --all-targets
--all-features -- -D warnings` / `mutagen-cli doctor` — all pass.
Experiment: `cargo test` (94/94), `cargo clippy --all-targets -- -D
warnings`, `preflight` (PASS, incl. D1 static guard + synthetic final
reconciliation pair), `self-test` (ALL PASS), `verify-run-manifest`
(OK), `verify-discovery` (OK), `verify-mutation` (OK),
`verify-selection` (OK at record level), `verify-promotion` (reports
the expected absence of E artifacts), `verify-stage-freeze` (reports
the expected D-stop state, §3). No live model requests during
validation.
