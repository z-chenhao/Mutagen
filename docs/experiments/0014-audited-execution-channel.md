# Experiment 0014: Mechanically Audited Execution Channel Confirmation

## Status

**Pre-registered; NOT YET EXECUTED.**

This document is the immutable pre-registration for Experiment 0014. It is
committed at Stage A0 (the frozen code commit) and must not change after
that point except to append the final Results/Conclusion sections after the
run completes. Every executable value it states is also encoded in the
machine-readable single source of truth
[`experiments/0014-audited-execution-channel/design.json`](../experiments/0014-audited-execution-channel/design.json);
where the two differ, the run is invalid.

## 1. Why this experiment exists

Experiment 0013 was a frozen, confirmatory re-run of the 0012 pipeline with
exactly one semantic change (the explicit, internally consistent episode
deadline contract). The deadline question was answered cleanly: all 264
episodes completed, every deadline-evidence rule behaved as registered,
the 0012 2 ms boundary violation class did not recur, and the loop
reached a formal conclusion.

The 0013 run died on the *other* inherited contract. After the A1 commit,
an ad-hoc **endpoint availability probe issued two unregistered HTTP GET
requests to `/v1/models`** on the registered endpoint, outside the
registered four-command stage path. Under the inherited 0012 execution
channel discipline ("after the A1 commit: no unregistered live model
requests … start Stage B directly"), the run was not a formally clean
run and the formal verdict was **INCONCLUSIVE** (see
`0013-deadline-safe-self-evolution-results.md` §4).

The structural weakness is obvious once named: **the 0012/0013 channel
discipline is a promise printed on stdout.** A printed rule cannot detect
a violation — a human (or agent) who runs `curl http://127.0.0.1:8000/v1/models`
after A1 breaks the run, and *nothing in the repository observes it*. The
endpoint itself has no view of the experiment's run identity, so from the
network's point of view a probe is indistinguishable from … anything.

**Experiment 0014 is a narrow, confirmatory re-run of the exact 0013
pipeline with exactly ONE semantic change: model traffic goes through a
registered, auditable local execution channel — a long-lived gateway
process with per-request authorization, an append-only hash-chained
ledger, and one-to-one reconciliation between experiment records and
gateway traffic — so that the 0013 failure class (unregistered requests
after A1) becomes mechanically observable and mechanically
INCONCLUSIVE.**

Everything else — the 0013 deadline contract, the kernel, the frozen
source and stage-freeze chain, the no-restart rule, the mutation
generation and selection and promotion semantics, the 18/150/96 episode
chain (264 total), the three-way verdict, the task registry (fresh
`E14*` / `S14*` / `P14*` IDs, `V14_*` literals), and the frozen 0009
stress profile — is carried over unchanged from 0013.

> **Question under confirmation:** with the execution channel made
> mechanical, does the frozen loop produce complete, reproducible,
> mechanically verifiable evidence — including *evidence about the
> channel itself* — and a formal SUPPORTED / REFUTED / INCONCLUSIVE
> verdict, in one run?

**Hypotheses** (H1 evolvability, H2 control carried over; H3 deadline
carried over as a regression check; H4 new):

- **H1** — On a live endpoint, the incumbent G0 is displaced at
  selection by a candidate generated from discovery evidence.
- **H2** — The frozen control surface (source freeze, stage-freeze
  chain, no-restart rule, verifier, **and the audited channel**)
  mechanically produces a defensible three-way verdict.
- **H3 (carried regression check)** — the 0013 deadline contract remains
  deadline-safe on a fresh run.
- **H4 (new, 0014-specific)** — the audited channel is *audit-safe*: on a
  clean run, the channel ledger contains **exactly** the experiment's
  registered requests (one-to-one, hash-chained, per-request
  authorization, zero unauthorized rows), and **any** unregistered
  request attempt after arming (e.g. the 0013 `GET /v1/models` probe,
  `curl`, a health check, a smoke test) is (a) rejected or logged,
  (b) recorded in the ledger, and (c) detected by the channel verifier
  such that the run is formally INCONCLUSIVE. H4 is confirmed by a
  clean one-to-one reconciliation on a clean run; it is refuted (run
  INCONCLUSIVE) if any unauthorized ledger row exists, any ledger row
  lacks a matching experiment record, any recorded request lacks a
  matching ledger row, the hash chain fails, or the gateway process
  restarted after arming.

## 2. The one 0014 semantic change (and only this one)

**Audited execution channel: a registered, long-lived local gateway
process is the ONLY route between the experiment and the model.**
Five interlocking parts:

### 2.1 The registered channel (design authority)

```json
"channel": {
  "protocol_version": "0014-audited-execution-channel-v1",
  "gateway_endpoint": "http://127.0.0.1:18140/v1",
  "upstream_endpoint": "http://127.0.0.1:8000/v1",
  "allowed_path": "/v1/chat/completions"
}
```

- `gateway_endpoint` — the **registered experiment endpoint**: a local
  loopback port owned by the gateway process. The model client's
  `REGISTERED_ENDPOINT` constant is *the gateway*; the client cannot
  address anything else.
- `upstream_endpoint` — the physical model transport (the 0013 local
  proxy, unchanged). This value is **channel-internal in the code**: it
  is read from the raw `design.json` JSON **only in `src/channel.rs`**
  (the gateway). No other module — agent kernel, model client, mutation,
  experiment runner — ever names it in code. A static preflight guard
  asserts that the upstream literal appears in no other source file.
  (The frozen `design.json` itself carries the URL, as it did in 0013;
  the guard is a code-level bypass guard.)
- `allowed_path` — exactly `/v1/chat/completions`. There is no
  model-list endpoint, no health endpoint, no other path: any other
  path (including 0013's `GET /v1/models`) is rejected with HTTP 404
  `disallowed_path` and **ledgered as an unauthorized request**.
- `protocol_version` — the channel protocol identifier; part of the
  manifest seal and of every ledger row.

### 2.2 The channel session (capability + identity)

Before A1 (`channel-init`), the harness generates one **channel
session**: a `channel-manifest.json` containing the run identity, a
session id, the protocol version, the gateway endpoint, the upstream
endpoint **plus its SHA-256** (the run manifest binds the hash; the
manifest is self-contained so the gateway never re-reads `design.json`
at serve time — it reads it once at start and cross-checks it against
the manifest), the allowed path, and the **ledger genesis hash**.
The manifest is **self-sealed**: `ledger_genesis_hash =
SHA-256(canonical manifest serialization minus that field)`; any byte
edit of the committed manifest fails the seal check at load time
(and, independently, would orphan every ledger row, since they chain
from the genesis). The per-request capability key is derived
deterministically from (protocol version, session id) — no random
secret is stored. The manifest is written once, then byte-immutable
for the lifetime of the run.

Each authorized request carries a **per-request authorization**:
`HMAC-SHA256(capability_key, run_id || stage || request_id ||
SHA256(request_body))` in the `X-Mutagen-Auth` header, alongside
`X-Mutagen-Run`, `X-Mutagen-Stage`, and `X-Mutagen-Request-Id` headers.
The request id is stage-bound: `{run_id}-{stage}-{6-digit ordinal}`.
HMAC-SHA256 is implemented directly on the vendored `sha2` crate (RFC
2104; the `hmac` crate is unavailable in the offline package cache) and
is verified against RFC 4231 known-answer vectors in the test suite.
The exact cryptographic strength is **not** the scientific variable:
the goal is detection of *accidental* unregistered requests (wrong
body, wrong stage, replayed id, probe to a dead path) plus
tamper-evidence, not adversarial secret protection against the
repository owner.

### 2.3 The gateway process (single, long-lived, no-restart)

`channel-init` spawns **one** gateway process (a re-exec of the same
binary, `gateway --state-dir …`) that:

- listens on `127.0.0.1:18140` plus a **Unix control socket**
  (`channel-control.sock`) — the *only* channel-management surface,
  used for `arm`, `status`, and `finalize`; the control socket carries
  no HTTP and can never reach the upstream;
- starts **UNARMED**: after A1, before `channel-arm`, *all* requests
  are rejected with HTTP 503 `channel_not_armed` and ledgered;
- after `channel-arm` (run exactly once, after the A1 commit), accepts
  only requests that pass the full check: path allow-list → method →
  run/stage/request-id/auth header presence → run identity → stage →
  request-id format → constant-time MAC → duplicate-id rejection.
  Every accepted request is appended to the ledger **before** it is
  forwarded to the upstream; every rejected request is ledgered
  (unauthorized) without being forwarded;
- **finalizes** after Stage E (`channel-finalize`, over the control
  socket): stops accepting, flushes the ledger, seals the final head
  hash, writes `channel-final.json`, and then refuses everything with
  HTTP 503 `channel_finalized`.

**Process continuity is part of the protocol.** The gateway records
its own PID on start (`origin_pid`) in `channel-state.json` (operational,
gitignored) and updates `pid` on each (re)start. If the current PID
differs from `origin_pid` at any verification point, the run is
INCONCLUSIVE (a restarted gateway would have lost its in-memory
duplicate-id set).

### 2.4 The hash-chained ledger

`channel-ledger.jsonl` — one JSON line per gateway request **attempt**
(authorized or not), append-only, fsynced per line. Each row:
`seq`, `timestamp`, `protocol_version`, `run_id`, `session_id`,
`stage`, `request_id`, `method`, `path`, `body_sha256`, `authorized`,
`reason` (for rejections), `forwarded`, `status_class`,
`previous_entry_hash`, `entry_hash`. The hash chain links every row to
its predecessor from the **genesis hash** in the channel manifest; the
chain verifier re-computes every row hash, row linkage, and the strict
sequence — any edit, deletion, insertion, or reorder of any line
violates the chain.

**Reconciliation is one-to-one and artifact-authoritative.** The
channel verifier matches: every *authorized & forwarded* ledger row
must have exactly one experiment record (discovery / generation-attempt
/ selection / promotion record with a non-empty `channel_request_id`
and matching stage + body hash), and every recorded live request must
have exactly one such ledger row. Experiment artifacts are the
authority for the recorded side (no hardcoded request counts).
**Any unauthorized ledger row fails the run** — a correctly *rejected*
probe is still an integrity event: the 0013 failure is exactly "someone
addressed the endpoint outside the stage path", and rejection is the
mechanism that now *proves* it happened.

### 2.5 What is deliberately NOT changed

Everything else is 0013's: the explicit deadline contract (600 000 /
600 000 / 1 000 ms; request-level timing evidence; the 0013 verifier
rules, now a carried regression check H3), the frozen source (now a
**23-file** manifest — the 0013 21 files plus `src/channel.rs` and
`src/channel_ledger.rs`), the commit-level stage-freeze chain
(B→C→D→E, with the ledger and per-stage channel checkpoints frozen
into each stage commit), the no-restart rule, the explicit
mutation-output schema and shape-informative retry, the anti-seeding
mutator whitelist, the deterministic selection rule, the two-sided
sign-test promotion, the 18/150/96 episode chain (264 total), the
three-way verdict, the artifact layout, the 12-task registry (fresh
`E14*` / `S14*` / `P14*` IDs, `V14_*` literals, disjoint from every
0008–0013 literal), and the frozen 0009 stress profile.

## 3. The mechanical guards (every live stage, before any request)

0013 §3 plus the **channel gate**: the 23-file source freeze, the
previous-stage commit-level freeze, and — new — (a) the channel
manifest exists, seals, and matches the registered design channel
block; (b) the gateway process is alive with the session's origin PID
(no restart); (c) the ledger prefix re-verifies from genesis and its
head hash equals the frozen checkpoint of the previous stage; (d) for
the post-A1 stages, the channel is armed (Stage B onward).

## 4. Run identity

One run: `run_id = "0014-r1"`. The committed `run-manifest.json` binds
`run_id`, `code_under_test_commit` (the A0 SHA), the combined
`frozen_design_sha256`, the per-file hashes of all 23 frozen files,
**and the channel block**: session id, protocol version, gateway
endpoint, upstream-endpoint SHA-256, and channel-manifest SHA-256.
Every record and summary carries the full provenance block. Every
`ModelRequestRecord` carries `channel_request_id` and
`request_body_sha256`; every `GenerationAttempt` carries the same.
Partial stage aborts are not resumable (no-restart rule).

## 5. Tasks

Twelve *new* registered tasks (`tasks.json`), three families
(`direct_set`, `conditional_set`, `replacement`), two-key external states
with fresh `V14_*` literals — disjoint from every 0008–0013 literal:

| Split | Count | Tasks | Stress (frozen 0009 profile) |
|---|---|---|---|
| Discovery (E14) | 3 | E14A (direct, x), E14B (conditional, y), E14C (replacement, x) | S2 / S1 / S2 |
| Selection (S14) | 3 | S14A (direct, y), S14B (conditional, x), S14C (replacement, y) | S2 / S1 / S2 |
| Promotion (P14) | 6 | P14A–P14F, both keys per family | S2 / S1 / S2 |

The registry is structurally validated (12 tasks, 3/3/6 splits,
family-map ↔ task consistency, two-key states, registered fault keys) and
its canonical hash is part of every provenance block; the verifier
re-checks literal split-disjointness. The literal machine-check
(`contains_v14_literal`) is keyed to this stage's registered literal
family, and self-test verifies it detects `V14_*` literals and rejects
carryover literals from earlier stages (e.g. `V_D1_0`) as non-0014.

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
| A0 | — | frozen code commit (23 files, this document) |
| — | — | `channel-init` (session + UNARMED gateway; local only, zero HTTP) |
| A1 | — | `init-run` → `run-manifest.json` + `channel-manifest.json` committed |
| — | — | `channel-arm` (control socket only; zero HTTP) |
| B Discovery | 18 | 3 E14 × 6 reps, G0 only, frozen stress — **via the channel** |
| C Mutation generation | ≤2 mutator requests | 4 suffix candidates (no tools) — **via the channel** |
| D Selection | 150 | 3 S14 × 10 reps × {G0,C1..C4}, exact cyclic order — **via the channel** |
| E Promotion | 96 | 6 P14 × 8 reps × {G0, selected}, alternating order — **via the channel** |
| — | — | `channel-finalize` + final channel reconciliation |

| **Total** | **264** | |

Gate failures block the next stage mechanically (registered in
`design.json`); infra failures above the registered floor invalidate the
stage; any guard violation — including any channel violation — makes the
whole run INCONCLUSIVE.

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
- Model: `incoai/Qwen3.8-27B-Splash`. The **single registered endpoint
  is the channel gateway `http://127.0.0.1:18140/v1`** (named constant
  in the model client; no auth header of its own — authorization is the
  channel's per-request MAC; no fallback model). The physical upstream
  `http://127.0.0.1:8000/v1` is channel-internal (§2.1). The mutation
  generator uses the **same** underlying model, no tools, issued with
  the registered `request_timeout_ceiling_ms`, through the channel.

## 9. Mutation generation (anti-seeding contract)

Carried over from 0013: the mutator receives ONLY the strict-whitelist
mutation input (incumbent prompt, tool schemas, discovery episodes —
model-visible conversation, requested calls, model-visible results, final
state, oracle success bit); no fault internals, no selection/promotion
tasks or literals, no hidden reasoning (network-free tamper-checked).
**Nothing from Experiments 0010/0011/0012/0013 is seeded**: no candidate
text, no hash, no hint; the 0012 and 0013 runs' suffixes must not appear
in the mutator prompt, the mutation input, or the crate sources. If the
fresh mutator *independently rediscoveries* a semantically similar
policy, that is ACCEPTED as independent rediscovery and reported as such
— never rejected post hoc for similarity.

Suffix bounds: 5–100 words, ≤800 bytes, no forbidden terms, no `V14_*`
literals, no registered task IDs, pairwise distinct after whitespace
normalization. ≤2 attempts with the shape-informative retry (0012
mechanism). **Injection is mechanically blocked**: every pool suffix must
equal the raw model response verbatim (verifier re-derives the pool from
the stored raw attempt). No file access for the mutator. Each
`GenerationAttempt` additionally records its `channel_request_id` and
`request_body_sha256`.

## 10. Selection rule (frozen, deterministic)

Carried over from 0013: per candidate, on the common-valid cells vs G0 —
wins / losses / ties. Lexicographic: **(net_margin ↓, wins ↓, losses ↑,
ordinal ↑)**. A mutation displaces G0 **only if** `net_margin > 0`;
otherwise G0 is retained (a valid "do not evolve" outcome ⇒ formal result
REFUTED, promotion does not run). No cost, no tokens, rationale, or model
judgment. The selected candidate is frozen into `selected-candidate.json`
(no promotion outcomes) and stage-frozen into the D commit before
promotion begins.

## 11. Promotion and the final three-way conclusion

Carried over from 0013: the frozen selected candidate is compared
pairwise against G0 on the clean promotion split. The exact two-sided
sign test over the non-tied informative pairs (p < α ⇒ significant)
yields the **final three-way conclusion**: `supported` (significant
quality improvement) / `refuted` (significant regression, or G0 retained
at selection) / `inconclusive` (everything else, including any
integrity failure, gate failure, infra overflow, **any channel
violation**, or crash). Cost, cache, and the three deadline diagnostics
are reported as diagnostics only. After Stage E the channel finalizes
and the final reconciliation (chain + one-to-one + seal) is reported.

## 12. Failure taxonomy & inconclusive handling

Carried over from 0013, plus the channel as a first-class integrity
surface:

- **Infra failures** — `http_error` (transport / non-2xx / malformed,
  including structured request timeouts, **including gateway-rejected
  503 `channel_not_armed` / `channel_finalized`**),
  `episode_deadline_exceeded`, `episode_deadline_reached`: classified,
  recorded, excluded from statistics; counts above the registered floor
  invalidate the stage.
- **Agent failures** (turn/call limit, bad arguments, no final answer)
  count as G0/candidate *failures* in the statistics.
- **Gate failures** block the next stage mechanically.
- **Any source-freeze or stage-freeze violation ⇒ the whole run is
  INCONCLUSIVE**, the run identity is permanently invalid, and no later
  stage may run.
- **Any channel violation ⇒ the whole run is INCONCLUSIVE** (§16).

No resume, no patch, no second attempt at the same stage (no-restart
rule) — for both the stages and the channel session.

## 13. Preflight / self-test (0014 additions)

`preflight` and `self-test` retain all 0013 sections (deadline
contract, endpoint discipline now asserting `REGISTERED_ENDPOINT ==
design.channel.gateway_endpoint`, source/stage-freeze git-repo
scenarios, mutation schema, §9.1 byte immutability, §9.2 explicit
endpoints) and add an **audited-execution-channel section** (shared
implementation, reported by both), all network-free — in-process gateway
on an ephemeral loopback port + in-process fake upstream:

1. **Static bypass guard**: the physical upstream literal (from
   `design.json`) appears in `src/channel.rs` and in *no other* source
   file; `REGISTERED_ENDPOINT` equals the registered gateway endpoint.
2. Authorized request (all headers, valid MAC) → forwarded, ledgered
   `authorized=true`.
3. Missing capability → 403 + ledgered unauthorized.
4. Wrong stage → 403 + ledgered.
5. Wrong body hash (tampered body under a valid MAC) → 403
   `auth_mismatch` + ledgered.
6. `GET /v1/models` → 404 `disallowed_path` + ledgered (no model-list
   endpoint exists).
7. Duplicate request id (valid MAC) → 403 `duplicate_request_id` +
   ledgered.
8. Pristine ledger chain verifies; tampered row fails; deleted line
   fails; reordered lines fail.
9. Reconciliation: extra authorized row with no record fails; recorded
   request with no ledger row fails.
10. Gateway restart (state pid ≠ origin pid) fails session continuity.
11. **The 0013 regression, mechanized**: after arm, an ad-hoc
    `GET /v1/models` → rejected + ledgered unauthorized + the channel
    verifier marks the run INCONCLUSIVE.
12. Any unauthorized attempt (even a correctly rejected one) fails the
    channel verifier.
13. `channel-status-local` inspects PID / control socket / state file
    only — zero HTTP.

## 14. Artifact layout & reporting

0013 layout, 0014-named, plus the channel artifacts. Raw trajectories +
summaries under `docs/experiments/artifacts/0014-*.json[l]` (registered;
excluded from the source freeze; frozen per stage by the stage-freeze
mechanism). Runtime evidence in the crate directory, **committed with
the stage that produced them**: `run-manifest.json` (A1),
`channel-manifest.json` (A1; byte-immutable after A1),
`channel-ledger.jsonl` (frozen into every stage freeze from B on, plus
its per-stage `channel-checkpoint-{b,c,d}.json` prefix checkpoint),
`channel-final.json` (finalized after E), `mutation-input.json`,
`candidate-pool.json`, `selected-candidate.json`,
`stage-{b,c,d}-freeze.json` (each now also freezes the ledger file and
its stage checkpoint). `channel-state.json` and the control socket are
operational and gitignored.

The results document
(`0014-audited-execution-channel-results.md`) must report: run identity
+ frozen hashes; **the channel integrity summary** (session id, ledger
rows, authorized/forwarded/unauthorized counts, matched/unmatched/missing
reconciliation counts, duplicate ids, hash-chain violations, gateway
restarts, and the per-stage request counts B/C/D/E — with the statement
that gateway-forwarded == experiment-recorded); per-stage
episode/wall-time/cost diagnostics including the three deadline metrics;
the four candidate suffixes + rationales (and whether any is an
independent rediscovery); the mutation attempts (including any retry and
its shape errors); selection tallies + tie-break path; promotion W/L/T +
exact p + conclusion; verifier and guard outputs for all stages; the
deadline-evidence report (carried 0013 regression check); the 0013
comparison; and any integrity events — **with the explicit statement of
whether any unregistered request attempt occurred (the 0013 failure
class) and, if so, that the run is INCONCLUSIVE**. All artifacts are
committed.

## 15. Execution protocol

1. **A0** — commit the frozen code (this commit). The 23-file freeze
   manifest and this document are in it (`print-a0` prints the exact
   git commands; `preflight` must pass before `channel-init`).
2. **channel-init** — generate the channel session, write
   `channel-manifest.json`, start the **UNARMED** gateway (local only;
   zero HTTP). No model contact.
3. **A1** — `init-run`: build + commit `run-manifest.json` and
   `channel-manifest.json` (network-free). After this commit the run
   identity and the channel are frozen.
4. **channel-arm** — arm the channel (control socket only; zero HTTP).
5. **B** — `discover`: 18 discovery episodes **through the channel**;
   guards (source freeze + channel gate) run first.
6. **C** — `generate`: mutation generation (≤2 attempts, through the
   channel); guards (source + B freeze + channel).
7. **D** — `select`: 150 selection episodes through the channel; guards
   (source + C freeze + channel).
8. **E** — `promote`: 96 promotion episodes through the channel; guards
   (source + D freeze + non-G0 selection + channel).
9. **channel-finalize** + final channel reconciliation (control socket
   only), then the final three-way conclusion.

Each stage: refuse on any guard/verifier/gate/channel violation; write
and commit artifacts + stage freeze on success; no restart on crash
(INCONCLUSIVE). The model client's endpoint is the gateway
`http://127.0.0.1:18140/v1`; the physical upstream
`http://127.0.0.1:8000/v1` is reachable **only** from the gateway
process.

## 16. Verdict rules (registered)

- **SUPPORTED** — the full B→C→D→E chain completes; all verifiers and
  guards clean, **including the channel** (zero unauthorized ledger
  rows; one-to-one reconciliation exact; hash chain intact; no gateway
  restart); all gates pass; selection displaces G0
  (`net_margin > 0`); promotion sign test significant (p < 0.05,
  candidate better than G0).
- **REFUTED** — chain completes cleanly (channel included) but the loop
  does not evolve: G0 retained at selection (net margin ≤ 0), or
  promotion significant in the *bad* direction.
- **INCONCLUSIVE** — everything else, including: any integrity
  violation; **any channel violation** (any unauthorized ledger row —
  the 0013 failure class; reconciliation mismatch in either direction;
  hash-chain failure; ledger prefix divergence from a frozen checkpoint;
  gateway restart; missing/sealed-manifest mismatch) — which
  additionally REFUTES H4; any guard failure; any gate failure; infra
  overflow; crash; or a stage never written.

## 17. Explicit non-goals

No hot reload, no core promotion, no new production abstraction:
everything here is experiment-local (`mutagen-exp-0014`), and any future
promotion of the source-freeze, stage-freeze, deadline-contract, or
**audited-channel** mechanism into the workspace is a *separate*
ADR-backed decision. No tuning of the deadline values or channel
parameters (they are registered, not fitted). No second gateway, no
channel fallback path, no fallback model, no task changes after A0, no
channel re-initialization mid-run (one session per run identity; a new
run identity is a new experiment run).

## 18. Scope boundary (registered)

0014 changes the **execution channel** of the 0013 experiment and
nothing else. It is not a general gateway product, not an API-key
system, not a multi-tenant proxy, and not a security boundary against a
hostile operator: the HMAC capability and the hash-chained ledger target
*accidental* unregistered traffic (probes, tooling, agents acting
outside the stage path) plus post-hoc tamper-evidence. Any stronger
security claim is out of scope for this experiment.

## 19. Byte immutability statement (registered)

After the A0 commit, the frozen bytes of every file in the 23-file
manifest are immutable for the lifetime of the run. After A1,
`channel-manifest.json` is additionally byte-immutable (HMAC-sealed;
re-verified at every gate). After each stage, that stage's
`channel-ledger.jsonl` prefix is immutable (frozen with the stage
checkpoint and re-verified at the next stage). The experiment cannot
"edit" the channel: it can only append authorized rows.
