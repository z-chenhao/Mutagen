# Experiment 0015: Final Clean Audited Self-Evolution Confirmation

## Status

**Pre-registered; NOT YET EXECUTED.**

This preregistration is byte-immutable after A0. It must never be
edited or appended after live execution begins. All results are written
exclusively to
[`0015-final-clean-confirmation-results.md`](0015-final-clean-confirmation-results.md),
which is not part of the source-freeze set.

This document is the immutable pre-registration for Experiment 0015. It
is committed at Stage A0 (the frozen code commit) and must not change
after that point. Every executable value it states is also encoded in
the machine-readable single source of truth
[`experiments/0015-final-clean-confirmation/design.json`](../experiments/0015-final-clean-confirmation/design.json);
where the two differ, the run is invalid.

## 1. Why this experiment exists

The current toy self-evolution line has produced one SUPPORTED result
(0009, the measurement substrate) and a sequence of INCONCLUSIVE
confirmatory runs, each killing the *control* layer of its predecessor:

| Run | Outcome | Driver |
|---|---|---|
| 0010 | INCONCLUSIVE | post-start source modification |
| 0011 | INCONCLUSIVE | mutation output schema failure |
| 0012 | INCONCLUSIVE | deadline runtime/verifier disagreement |
| 0013 | INCONCLUSIVE | two unregistered post-A1 `GET /v1/models` probes |
| 0014 | INCONCLUSIVE | the frozen final reconciliation omitted the Stage-D record set |

Run 0014-r1 made the channel mechanism itself work: the sealed ledger
contained exactly the experiment's registered traffic (2 095 authorized
/ forwarded rows; 0 unauthorized; 0 missing; 0 extra; 0 duplicate ids;
0 hash-chain violations; 0 gateway restarts), the deadline contract was
clean, and the numerical evidence was strong (selection C1 27W/0L/3T;
promotion 39W/0L/9T, p = 3.637978807091713e-12). The run nonetheless
stands as INCONCLUSIVE because the *frozen* final audit of 0014-r1
reconciled the ledger against B + C + E records only — the Stage-D
(selection) record set was accidentally omitted, so the sealed final
document reported a reconciliation mismatch (recorded 906 ≠ ledger 2 095).

The defect was fixed on `main` in commit `9d3692e` (**the D1 fix**):
the final channel reconciliation now reconciles **B + C + D + E**
together, and a **static D1 regression guard** (in the preflight /
self-test channel section) mechanically re-asserts that the final audit
wires all four record sets. The current merged source
(`56fb381`) is therefore the fixed 0014 implementation, with the D1 fix
and its guard.

**Experiment 0015 is the final confirmation experiment of this line. It
is NOT a new mechanism experiment, NOT an architecture improvement, NOT
a harness refactor, and NOT a task-distribution tuning.** Its single
question:

> With the already-fixed 0014 implementation (D1 fix + guard) **frozen
> before live execution**, can the complete
> Evidence → Generate → Select → Promote loop finish in **one fresh
> run** with source integrity, stage provenance, deadline integrity, and
> audited execution-channel integrity **all clean**, and produce a
> formal SUPPORTED / REFUTED / INCONCLUSIVE result?

If 0015 is cleanly SUPPORTED, the experimental line is complete.

**Hypotheses** (all carried over from 0013/0014):

- **H1** — On a live endpoint, the incumbent G0 is displaced at
  selection by a candidate generated from fresh discovery evidence.
- **H2** — The frozen control surface (source freeze, stage-freeze
  chain, no-restart rule, verifiers, and the audited channel)
  mechanically produces a defensible three-way verdict.
- **H3 (carried regression check)** — the 0013/0014 deadline contract
  remains deadline-safe on a fresh run.
- **H4 (0014, re-confirmed)** — the audited channel is audit-safe: on a
  clean run the sealed ledger reconciles one-to-one with the
  experiment's records (now including Stage D, per the D1 fix), and any
  unregistered request attempt, any reconciliation mismatch, any chain
  break, or any gateway restart makes the run formally INCONCLUSIVE.

## 2. The only differences vs the 0014 run (identity-only)

Everything scientific and mechanical is the merged, D1-fixed 0014
implementation. The complete allowed difference list:

| 0014 | 0015 | Kind |
|---|---|---|
| experiment identity `0014` | `0015` | identity |
| run identity `0014-r1` | `0015-r1` | identity |
| task IDs `E14*` / `S14*` / `P14*` | `E15*` / `S15*` / `P15*` | identity (fresh registry) |
| state literals `V14_*` | fresh `V15_*` (same structural mapping) | identity (fresh registry) |
| gateway port 18140 | 18150 | identity |
| protocol version `0014-audited-execution-channel-v1` | `0015-audited-execution-channel-v1` | identity |
| paths / file names / docs / crate name | `0015`-named | identity |

There is **no** other semantic difference. In particular, the D1 fix
(final reconciliation over B + C + D + E, with its static regression
guard) is **not** a 0015 change: it already exists on the merged 0014
source and is carried as-is. 0015 **tests** it live; it adds **no** new
reconciliation mechanism, no new ledger, and no new seal format.

## 3. What 0015 must NOT do (registered)

- **No opportunistic improvements** before A0: only mechanical
  renaming, compilation fixes caused by renaming, test-fixture identity
  fixes, and fresh-V15 registry consistency are permitted. Any
  substantive mechanism bug discovered before A0 must be STOP +
  reported, not redesigned.
- **No changes after A0**: zero frozen-source edits, zero
  modify-commit-revert, zero uncommitted drift. If a frozen-code bug
  appears after A0, the formal result is INCONCLUSIVE; the run is
  documented and stopped. There is **no** post-run source fix in this
  experiment branch (unlike 0014).
- **No readiness probes — at all**: from the start of the 0015
  execution workflow, no `curl` against 127.0.0.1:8000 or 127.0.0.1:18150,
  no `GET /v1/models`, no smoke `POST`, no `models.list()`, no
  `requests.get()`, no `wget`, no health probe — not before A0, not
  between A0 and A1, not after A1, not after channel-arm. Preflight and
  self-test use fake in-process transports only. If the real upstream is
  unavailable when Stage B begins, the failures are recorded as
  infrastructure failures, the verdict follows the frozen gates, and no
  probe / fix / retry is attempted.
- **No modification** of Experiments 0001–0014 (code or docs) and of
  `crates/mutagen-core`, `crates/mutagen-runtime`, `crates/mutagen-cli`.
- **No promotion** of anything into production/core. If 0015 is
  SUPPORTED, that only states the experimental line is complete.
- **No Experiment 0016** in this task.

## 4. The audited execution channel (carried from the merged 0014, incl. D1)

```json
"channel": {
  "protocol_version": "0015-audited-execution-channel-v1",
  "gateway_endpoint": "http://127.0.0.1:18150/v1",
  "upstream_endpoint": "http://127.0.0.1:8000/v1",
  "allowed_path": "/v1/chat/completions"
}
```

- `gateway_endpoint` is the **registered experiment endpoint** (the
  model client constant `REGISTERED_ENDPOINT`); the client can address
  nothing else.
- `upstream_endpoint` is channel-internal in code: it appears in
  `src/channel.rs` only (static bypass guard); the frozen
  `design.json` carries the URL.
- `allowed_path` is exactly `/v1/chat/completions`. There is **no**
  `/models` endpoint, **no** `/health` endpoint, **no** smoke
  endpoint; any other path is rejected 404 `disallowed_path` and
  ledgered as an unauthorized request.
- One channel session per run: the `channel-init`-generated
  `channel-manifest.json` (run identity, session id, protocol version,
  gateway endpoint, upstream endpoint + its SHA-256, allowed path,
  ledger genesis hash) is self-sealed (`ledger_genesis_hash =
  SHA-256(canonical manifest minus that field)`) and byte-immutable
  after A1.
- Per-request authorization: `HMAC-SHA256(capability_key, run_id ||
  stage || request_id || SHA256(request_body))` in `X-Mutagen-Auth`,
  with `X-Mutagen-Run` / `X-Mutagen-Stage` / `X-Mutagen-Request-Id`
  headers; request ids are stage-bound (`0015-r1-<STAGE>-NNNNNN`).
  Wrong body / stage / replayed id / missing capability → 403 +
  ledgered unauthorized.
- The hash-chained, append-only, fsynced `channel-ledger.jsonl`
  records every attempt (authorized or rejected), chained from the
  manifest genesis hash; any edit, deletion, or reorder breaks the
  chain.
- One long-lived gateway: spawned **UNARMED** by `channel-init`
  (zero HTTP after start); armed once over the Unix control socket
  after A1; finalized after Stage E. A PID change after start is an
  integrity failure. `channel-status-local` inspects PID / socket /
  state only (zero HTTP).

### The D1 fix (mandatory, already on merged 0014)

The final reconciliation in `cmd_promote` (after `channel-finalize`)
computes the recorded side as the **union of all four stage record
sets**:

```rust
let all_rec = channel::all_recorded(
    &discovery_records,   // B
    &generation_artifact, // C
    &selection_records,   // D   ← the 0014-r1 defect omitted this set
    &records,             // E
);
```

and requires `forwarded == recorded`, `missing = 0`, `unmatched = 0`.
A static D1 regression guard (preflight + self-test) asserts that the
final audit source wires all four sets. 0015 adds no new reconciliation
mechanism: the D1 fix and the existing guard are sufficient, and 0015
**tests them live** — the clean live B + C + D + E reconciliation is
the decisive new evidence of this run.

## 5. Run identity

One run: `run_id = "0015-r1"`, `experiment_id = "0015"`. The committed
`run-manifest.json` binds the run identity, `code_under_test_commit`
(the A0 SHA), the combined `frozen_design_sha256`, the per-file hashes
of all 23 frozen files, and the channel block (session id, protocol
version, gateway endpoint, upstream-endpoint SHA-256, channel-manifest
SHA-256). Every record and summary carries the full provenance block;
every `ModelRequestRecord` and every `GenerationAttempt` carries
`channel_request_id` and `request_body_sha256`. Partial stage aborts
are not resumable (no-restart rule).

## 6. Tasks

Twelve *new* registered tasks (`tasks.json`), the same structural
mapping as 0014 (families `direct_set` / `conditional_set` /
`replacement`, the same key-rotation pattern: discovery x/y/x,
selection y/x/y, promotion both keys per family; same fault-key
placement; same stress per family), with **fresh `V15_*` literals
exclusively** — disjoint from every 0008–0014 literal, and deliberately
not recalibrated (no difficulty change):

| Split | Count | Tasks | Stress (frozen 0009 profile) |
|---|---|---|---|
| Discovery (E15) | 3 | E15A (direct, x), E15B (conditional, y), E15C (replacement, x) | S2 / S1 / S2 |
| Selection (S15) | 3 | S15A (direct, y), S15B (conditional, x), S15C (replacement, y) | S2 / S1 / S2 |
| Promotion (P15) | 6 | P15A–P15F, both keys per family | S2 / S1 / S2 |

The registry is structurally validated (12 tasks, 3/3/6 splits,
family-map ↔ task consistency, two-key states, registered fault keys)
and its canonical hash is part of every provenance block; the verifier
re-checks literal split-disjointness. The literal machine-check
(`contains_v15_literal`) is keyed to the fresh `V15_*` family and
self-test verifies it detects `V15_*` literals and rejects carryover
literals from earlier stages as non-0015.

## 7. Frozen stress

The **0009 supported family-level profile, carried over unchanged**
(`stress-profile.json`; no calibration): `direct_set → S2 (drop first 2
writes)`, `conditional_set → S1 (1)`, `replacement → S2 (2)`. The
repeated-silent-drop fault is the only environmental fault; it is
invisible to the model and auditable in the hidden write-attempt ledger.

## 8. The loop (stage chain)

| Stage | Episodes | Content |
|---|---|---|
| A0 | — | frozen code commit (23 files, this document) |
| — | — | `channel-init` (session + UNARMED gateway; local only, zero HTTP; no readiness check after) |
| A1 | — | `init-run` → `run-manifest.json` + `channel-manifest.json` committed |
| — | — | `channel-arm` (control socket only; zero HTTP) |
| B Discovery | 18 | 3 E15 × 6 reps, G0 only, frozen stress — via the channel |
| C Mutation generation | ≤2 mutator requests | 4 suffix candidates (no tools) — via the channel |
| D Selection | 150 | 3 S15 × 10 reps × {G0,C1..C4}, exact cyclic order — via the channel |
| E Promotion | 96 | 6 P15 × 8 reps × {G0, selected}, alternating order — via the channel |
| — | — | `channel-finalize` + final channel reconciliation (B + C + D + E) |

| **Total** | **264** | |

Gate failures block the next stage mechanically (registered in
`design.json`); infra failures above the registered floor invalidate the
stage; any guard violation — including any channel violation — makes the
whole run INCONCLUSIVE. No additional live action occurs between these
steps.

## 9. Design numbers (single source of truth: `design.json`)

- `candidate_count = 4`; harness-assigned IDs `C1..C4` (returned order);
  `max_generation_attempts = 2` (shape-informative retry).
- Discovery: 6 reps/task = 18 episodes; gates: ≥16 valid, ≥8 incumbent
  failures.
- Selection: 10 reps/task × 5 conditions `{G0,C1..C4}` = 150 episodes;
  gates: ≥27 common-valid cells, ≥12 G0-failures-in-common-valid.
- Promotion: 8 reps/task × 2 conditions = 96 episodes (48 pairs);
  gates: ≥44 valid pairs, ≥12 potential information, ≥12 actually
  informative, infra-failure rate ≤ 0.10.
- `alpha = 0.05` (two-sided exact sign test).
- Agent temperature **0.2**; mutator temperature **0.7**; hard limits
  **12 model turns / 16 tool calls** per episode; **deadline contract
  600 000 / 600 000 / 1 000 ms** (design fields, unchanged); one tool
  call per turn (`parallel_tool_calls: false`, `tool_choice: "auto"`).
  Per-request deadline evidence (`request_started_elapsed_ms`,
  `requested_timeout_ms`, `request_finished_elapsed_ms`, `timed_out`,
  `response_accepted`) is carried, and all 0013/0014 deadline tests
  carry over unchanged.
- Model: `incoai/Qwen3.8-27B-Splash`. No fallback model, no alternate
  endpoint.

## 10. Mutation generation (anti-seeding contract)

Carried over from 0014: the mutator receives ONLY the strict-whitelist
mutation input (incumbent prompt, tool schemas, valid fresh E15
discovery evidence — model-visible conversation, requested calls,
model-visible results, final state, oracle success bit); no fault
internals, no hidden drop ledger, no selection/promotion tasks, and —
critically for 0015 — **no historical candidate text or result from any
earlier run** (no 0010/0011/0012 candidates, no 0013/0014 C1–C4, no
promotion results, no "read-after-write" / "verify after write" /
"retry mismatch" hints). Nothing from Experiments 0010–0014 is seeded.
If the fresh mutator *independently rediscoveries* a semantically
similar policy, that is ACCEPTED and reported only **after** the formal
verdict is fixed, as post-hoc policy-attractor evidence — never used to
influence selection.

Suffix bounds: 5–100 words, ≤800 bytes, no forbidden terms, no `V15_*`
literals, no registered task IDs, pairwise distinct after whitespace
normalization. The explicit output schema is exactly:

```json
{
  "candidates": [
    { "suffix": "string", "rationale": "string" },
    { "suffix": "string", "rationale": "string" },
    { "suffix": "string", "rationale": "string" },
    { "suffix": "string", "rationale": "string" }
  ]
}
```

(exactly four objects, exactly the string fields `suffix` and
`rationale`). ≤2 attempts with the shape-informative retry; if the
schema never becomes valid the run is INCONCLUSIVE. **Injection is
mechanically blocked**: every pool suffix must equal the raw model
response verbatim (the verifier re-derives the pool from the stored raw
attempt). Each `GenerationAttempt` records its `channel_request_id` and
`request_body_sha256`.

## 11. Selection rule (frozen, deterministic)

Carried over from 0014: per candidate against G0 on the common-valid
cells — win = candidate success && G0 failure; loss = candidate failure
&& G0 success; tie = same binary outcome. Lexicographic ranking:
**(net_margin ↓, wins ↓, losses ↑, ordinal ↑)** where
`net_margin = wins − losses`. A mutation displaces G0 **only if**
`best net_margin > 0`; otherwise G0 is retained (a valid "do not
evolve" outcome ⇒ formal result REFUTED; promotion does not run). No
cost, no tokens, rationale, or model judgment. The selected candidate
is frozen into `selected-candidate.json` (no promotion outcomes) and
stage-frozen into the D commit before promotion begins.

## 12. Promotion and the final three-way conclusion

Carried over from 0014: the frozen selected candidate is compared
pairwise against G0 on the promotion split. The exact two-sided sign
test over the non-tied informative pairs:

```
n = wins + losses;  k = min(wins, losses)
p = min(1, 2 * Σ[i=0..k] C(n,i) * 0.5^n)
```

- `wins > losses && p < 0.05` → `quality_improvement`
- `losses > wins && p < 0.05` → `quality_regression`
- otherwise → `quality_inconclusive`

Formal taxonomy: `quality_improvement` → SUPPORTED (if every
integrity/control gate is clean); `quality_regression` → REFUTED;
`quality_inconclusive` → INCONCLUSIVE. Cost, cache, and the deadline
diagnostics are reported as diagnostics only. After Stage E the channel
finalizes and the final reconciliation (chain + one-to-one over B + C +
D + E + seal) is reported; the sealed final document and an independent
recomputation from the committed artifacts must agree.

## 13. Failure taxonomy & inconclusive handling

Carried over from 0014:

- **Infra failures** — `http_error` (transport / non-2xx / malformed,
  including structured request timeouts, including gateway-rejected
  503 `channel_not_armed` / `channel_finalized`),
  `episode_deadline_exceeded`, `episode_deadline_reached`: classified,
  recorded, excluded from statistics; counts above the registered floor
  invalidate the stage.
- **Agent failures** (turn/call limit, bad arguments, no final answer)
  count as G0/candidate *failures* in the statistics.
- **Gate failures** block the next stage mechanically.
- **Any source-freeze or stage-freeze violation ⇒ the whole run is
  INCONCLUSIVE** and no later stage may run.
- **Any channel violation ⇒ the whole run is INCONCLUSIVE** (any
  unauthorized ledger row — including any correctly-rejected probe,
  readiness probe or smoke request; any reconciliation mismatch in
  either direction; hash-chain failure; ledger prefix divergence from a
  frozen checkpoint; gateway restart; missing/sealed-manifest mismatch).
- **Any frozen-code bug discovered after A0 ⇒ the whole run is
  INCONCLUSIVE**; document and stop. No patch, no restart, no
  modify-commit-revert — 0015 must remain a clean historical snapshot.

No resume, no patch, no second attempt at the same stage (no-restart
rule) — for both the stages and the channel session.

## 14. Preflight / self-test (network-free; all 0014 coverage + D1)

`preflight` and `self-test` retain every 0014 section (deadline
contract, endpoint discipline, source/stage-freeze git-repo scenarios,
mutation schema, stress profile, and the in-process channel behavioral
suite) and are **entirely network-free** — the in-process gateway runs
on an ephemeral loopback port against a fake in-process upstream. The
channel section additionally carries:

1. **Static bypass guard**: the physical upstream literal appears in
   `src/channel.rs` and in no other source file; `REGISTERED_ENDPOINT`
   equals the registered gateway endpoint.
2. **D1 static guard**: the final audit source wires all four record
   sets (`all_recorded(&discovery_records, &generation_artifact,
   &selection_records, &records)`) — the 0014-r1 defect class.
3. **Synthetic complete final reconciliation**: a B + C + D + E record
   set against a matching ledger → exact success.
4. **Synthetic D omission** → reconciliation failure.
5. Extra gateway row with no record → failure.
6. Missing record for a forwarded row → failure.
7. Unauthorized `GET /v1/models` → 404 + ledgered + verifier failure
   (the mechanized 0013 regression).
8. Ledger tamper / deletion / reordering → chain failure.
9. Gateway restart (pid ≠ origin pid) → session-continuity failure.
10. Deadline-crossing cases (0013/0014 cases, unchanged).

These are tests of existing mechanisms, not a new protocol.

## 15. Artifact layout & reporting

0014 layout, 0015-named. Raw trajectories + summaries under
`docs/experiments/artifacts/0015-*.json[l]` (registered; excluded from
the source freeze; frozen per stage by the stage-freeze mechanism).
Runtime evidence in the crate directory, **committed with the stage
that produced them**: `run-manifest.json` (A1),
`channel-manifest.json` (A1; byte-immutable after A1),
`channel-ledger.jsonl` (frozen into every stage freeze from B on, plus
its per-stage `channel-checkpoint-{b,c,d}.json`), `channel-final.json`
(after E), `mutation-input.json`, `candidate-pool.json`,
`selected-candidate.json`, `stage-{b,c,d}-freeze.json`.
`channel-state.json` and the control socket are operational and
gitignored.

The results document (`0015-final-clean-confirmation-results.md`) must
report: formal verdict; run identity; A0/A1 SHAs; frozen hashes; source
integrity (post-A0 frozen commits, drift, A0 byte match); D1 regression
(B/C/D/E inclusion in the final reconciliation); channel integrity
(session id, ledger entries, forwarded, recorded, unauthorized,
unmatched, missing, duplicates, hash-chain violations, gateway restarts,
final seal PASS/FAIL); the independent recomputation (per-stage
B/C/D/E request counts re-derived from committed artifacts,
(stage, request_id, body_sha256) one-to-one); deadline integrity;
discovery tallies; mutation attempts + the four candidates; anti-seeding
audit; selection tallies + selected candidate; promotion W/L/T + exact p
+ classification; H1–H4; the formal generation transition (if and only
if SUPPORTED); post-hoc policy-attractor analysis; cost/timing
diagnostics. **No result goes into this (frozen) preregistration.**

## 16. Execution protocol

1. **A0** — commit the frozen code (this commit; the 23-file manifest
   and this document; `print-a0` prints the exact git commands).
   `preflight` + `self-test` pass first; both are network-free.
2. **channel-init** — generate the channel session, write
   `channel-manifest.json`, start the **UNARMED** gateway (local only;
   zero HTTP). **No readiness check afterward.**
3. **A1** — `init-run`: build + commit `run-manifest.json` and
   `channel-manifest.json` (network-free). Verify locally
   (`verify-run-manifest`). Then **channel-arm** (control socket only;
   zero HTTP). Then directly to Stage B.
4. **B** — `discover`: 18 discovery episodes through the channel. If
   any gate fails: INCONCLUSIVE, stop. If pass: commit B artifacts +
   freeze.
5. **C** — `generate`: mutation generation (≤2 attempts, through the
   channel). If the schema never becomes valid: INCONCLUSIVE, stop. If
   success: commit C artifacts + freeze.
6. **D** — `select`: 150 selection episodes through the channel. If
   insufficient or any integrity violation: INCONCLUSIVE, stop. Apply
   the frozen ranking. If G0 is retained: REFUTED, stop. If a candidate
   is selected: commit D artifacts + freeze.
7. **E** — `promote`: 96 promotion episodes through the channel (no
   rerun), then **channel-finalize**, then the final reconciliation
   (B + C + D + E, the live D1 confirmation).
8. Formal verdict; write the results document; final source audit
   (post-A0 frozen commits = 0, drift = 0, A0 byte match = true); final
   commit (result/evidence artifacts only); push; PR.

## 17. Verdict rules (registered)

- **SUPPORTED** — the full B→C→D→E chain completes; source freeze clean;
  stage freezes clean; deadline clean; channel clean (zero
  unauthorized; one-to-one reconciliation exact over B + C + D + E; hash
  chain intact; no gateway restart; sealed final document and
  independent recomputation agree); all gates pass; selection displaces
  G0 (`net_margin > 0`); promotion `quality_improvement`.
- **REFUTED** — the chain completes with integrity clean, but the loop
  does not evolve: G0 retained at selection (net margin ≤ 0), or
  promotion `quality_regression`.
- **INCONCLUSIVE** — everything else, including: `quality_inconclusive`;
  insufficient information; infra overflow; mutation schema failure;
  deadline violation; channel violation (any kind); source drift;
  stage-provenance failure; crash; or any frozen-code bug discovered
  after A0.

## 18. What SUPPORTED means (registered maximum claim)

If and only if 0015 is cleanly SUPPORTED, the formal experimental
generation transition **G0 → <selected candidate>** is recorded — the
first formally clean autonomous generation transition in the current
Mutagen experiment line. Maximum claim: *Under the tested model, frozen
silent-write environment, registered three-family task structure,
prompt-suffix evolvable surface, deadline-safe runtime, and
mechanically audited execution channel, Mutagen used fresh incumbent
evidence to autonomously generate bounded candidate policies, selected
one candidate without promotion leakage, and independently confirmed
that candidate as a statistically significant improvement over G0 in
one fully frozen run.* No generalization beyond this environment. The
current toy confirmation line is complete; the next work item (a
separate task) is production abstraction extraction, not Experiment
0016.

## 19. Byte immutability statement (registered)

After the A0 commit, the frozen bytes of every file in the 23-file
manifest are immutable for the lifetime of the run. This preregistration
is one of the 23 files. **This preregistration is byte-immutable after
A0. It must never be edited or appended after live execution begins.
All results are written exclusively to
`0015-final-clean-confirmation-results.md`, which is not part of the
source-freeze set.** After A1, `channel-manifest.json` is
additionally byte-immutable (HMAC-sealed; re-verified at every gate).
After each stage, that stage's `channel-ledger.jsonl` prefix is
immutable (frozen with the stage checkpoint and re-verified at the next
stage). The experiment cannot "edit" the channel: it can only append
authorized rows.
