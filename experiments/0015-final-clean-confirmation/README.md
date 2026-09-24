# Experiment 0015 — Final Clean Audited Self-Evolution Confirmation

## Purpose

Experiments 0009–0014 established and then repeatedly re-confirmed the
frozen self-evolution loop (Evidence → Generate → Select → Promote) under
an ever-hardening integrity surface:

- **0009** — SUPPORTED: the measurement substrate.
- **0010** — INCONCLUSIVE: post-start source modification.
- **0011** — INCONCLUSIVE: mutation output schema failure.
- **0012** — INCONCLUSIVE: deadline runtime/verifier disagreement.
- **0013** — INCONCLUSIVE: two unregistered post-A1 endpoint `GET` requests
  (the channel discipline was a printed promise, not a mechanism).
- **0014** — INCONCLUSIVE: the *new* mechanically audited execution channel
  was itself empirically clean (2095/2095 one-to-one request
  reconciliation, 0 unauthorized, 0 hash-chain violations, 0 gateway
  restarts), but the frozen final reconciliation code of run 0014-r1
  accidentally omitted the Stage-D (selection) record set, so the sealed
  final audit reported a reconciliation mismatch.

**0015 is the final confirmation run of this experimental line. It is NOT
a new mechanism experiment.** Its sole question:

> With the already-fixed 0014 implementation (the D1 fix: the final
> channel reconciliation includes the B + C + D + E record sets and
> carries the static D1 regression guard) **frozen before live
> execution**, can the complete Evidence → Generate → Select → Promote
> loop finish in **one fresh run** with source integrity, stage
> provenance, deadline integrity, and audited execution-channel
> integrity all clean, and produce a formal SUPPORTED / REFUTED /
> INCONCLUSIVE result?

Everything else — the 0014 pipeline, the audited channel, the deadline
contract, the frozen source and stage-freeze chain, the no-restart rule,
the mutation generation and selection and promotion semantics, the
18/150/96 episode chain (264 total), the three-way verdict, the frozen
0009 stress profile — is carried over unchanged. Only *identity* differs
(fresh `0015` / `0015-r1` identities, fresh `E15*`/`S15*`/`P15*` tasks in
a fresh `V15_*` literal space, gateway port 18150, protocol version
`0015-audited-execution-channel-v1`). **No readiness probes at any point**
(preflight/self-test are network-free; if the upstream is unavailable
when Stage B begins, that is a recorded infrastructure failure, not a
reason to probe). No post-run source fix is possible: any frozen-code
bug found after A0 makes the run INCONCLUSIVE and the branch is a clean
historical snapshot. If 0015 is cleanly SUPPORTED, the experimental line
is complete.

## The 0014 implementation as carried (including the D1 fix)

1. **The registered endpoint is the channel gateway**
   (`http://127.0.0.1:18150/v1` — `REGISTERED_ENDPOINT` in
   `src/model.rs`). The model client can address nothing else. The
   physical upstream (`http://127.0.0.1:8000/v1`, the local proxy) is
   channel-internal: readable only by `src/channel.rs` (a static
   preflight guard asserts the upstream literal appears nowhere else).
2. **One channel session per run** (`channel-manifest.json`, generated
   by `channel-init` before A1, HMAC-sealed, byte-immutable after A1).
3. **Per-request authorization** — `HMAC-SHA256(capability_key,
   run_id || stage || request_id || SHA256(body))` in
   `X-Mutagen-Auth`, alongside `X-Mutagen-Run` / `-Stage` /
   `-Request-Id`; any wrong body / stage / replayed id / missing
   capability → HTTP 403 + ledgered unauthorized.
4. **The hash-chained ledger** (`channel-ledger.jsonl`): every attempt
   (authorized or rejected) is chained from the manifest genesis hash;
   any edit, deletion, or reorder breaks the chain.
5. **One-to-one reconciliation, including the D1 fix** — the *final*
   reconciliation records the UNION of all four stage record sets
   (B discovery + C generation + D selection + E promotion) and matches
   it against the sealed ledger in both directions. The static D1
   regression guard (present in merged 0014) re-asserts that the final
   audit wires all four sets — the 0014-r1 defect (omitted D) is
   mechanically blocked. This is the critical live confirmation 0015
   must provide.
6. **Single long-lived gateway, no restart** — spawned unarmmed by
   `channel-init`; armed over the local Unix control socket by
   `channel-arm` (after A1); finalized after Stage E. A PID change after
   start is an integrity failure.

## Design (frozen; identity-only changes vs 0014)

- Model: `incoai/Qwen3.8-27B-Splash`, one registered endpoint — the
  gateway (`http://127.0.0.1:18150/v1`); the physical upstream
  `http://127.0.0.1:8000/v1` is channel-internal.
- Agent temperature 0.2; mutation temperature 0.7. Hard limits: 12
  model turns, 16 tool calls per episode; **deadline contract
  600 000 / 600 000 / 1 000 ms (unchanged)**; one tool call per turn
  (`parallel_tool_calls: false`, `tool_choice: "auto"`).
- Frozen 0009 stress profile (no recalibration): S2 = 2 dropped writes,
  S1 = 1.
- 12 FRESH tasks (`E15*`/`S15*`/`P15*`, `V15_*` literal space,
  split-disjoint from every 0008–0014 literal): 3 discovery
  (18 episodes) / 3 selection (5 conditions × 30 cells = 150) /
  6 promotion (2 conditions × 48 pairs = 96).
- Gates (identical to 0014): discovery valid ≥ 16 / G0 failures ≥ 8;
  selection common valid ≥ 27 / G0 failures ≥ 12 / selected vs G0
  net margin > 0 (ties broken by G0); promotion valid pairs ≥ 44 /
  potential info ≥ 12 / actual informative ≥ 12; promotion infra
  failure rate ≤ 0.10; two-sided exact sign test, α = 0.05.
- **No readiness probes**: `preflight` and `self-test` use fake
  in-process transports only; no `curl`, no `GET /v1/models`, no smoke
  request, no health check, at any point in the run.

## Artifact layout

- Raw trajectories + summaries: `docs/experiments/artifacts/0015-*.json[l]`
  (registered; excluded from the source freeze; frozen per stage by the
  stage-freeze mechanism).
- Runtime evidence: this crate directory — `run-manifest.json` (A1),
  `channel-manifest.json` (A1; byte-immutable), `channel-ledger.jsonl`
  + `channel-checkpoint-{b,c,d}.json` (frozen with each stage),
  `channel-final.json` (after E), `mutation-input.json`,
  `candidate-pool.json`, `selected-candidate.json`,
  `stage-{b,c,d}-freeze.json` (each also freezes the ledger + its
  checkpoint).
- `channel-state.json` and `channel-control.sock` are operational and
  gitignored.

## The three mechanical guards (every live stage, before any request)

1. **Source freeze** (`src/freeze.rs`, 23 files): every frozen file's
   bytes must hash to the manifest value and match the A0 commit; no
   frozen-path commit after A0; no working-tree drift; A0 is an
   ancestor of HEAD.
2. **Previous-stage freeze** (`src/stage_freeze.rs`): B needs only the
   source freeze; C adds the B freeze; D adds the C freeze; E adds the
   D freeze and a non-G0 selected candidate.
3. **Channel gate** (`src/channel.rs`): the channel manifest seals and
   matches the registered design block; the gateway is alive with its
   origin PID (no restart); the ledger prefix re-verifies from genesis
   and matches the frozen previous-stage checkpoint; post-A1 stages
   additionally require an armed channel.

All three are exercised by `self-test` against real temporary git
repositories and in-process gateway fixtures, including the D1 static
guard, the synthetic B+C+D+E reconciliation, and the D-omission
failure case.

## Usage (registered commands only)

```sh
# A0 (human/agent; the harness prints the exact commands):
cargo run -p mutagen-exp-0015 -- print-a0
# → git add <23 frozen files> && git commit   (then `preflight`)

# Channel session (local only; zero model contact; no readiness check after):
cargo run -p mutagen-exp-0015 -- channel-init      # UNARMED gateway starts

# A1: run manifest + channel manifest (tree must exactly match A0):
cargo run -p mutagen-exp-0015 -- init-run
# → git add run-manifest.json channel-manifest.json && git commit

# Arm the channel (control socket only; zero HTTP):
cargo run -p mutagen-exp-0015 -- channel-arm

# Then the stage commands (each commits its artifacts + freeze):
cargo run -p mutagen-exp-0015 -- discover   # 18 episodes through the channel
cargo run -p mutagen-exp-0015 -- generate   # mutation (2 attempts)
cargo run -p mutagen-exp-0015 -- select     # 150 episodes
cargo run -p mutagen-exp-0015 -- promote    # 96 episodes → channel finalize → CONCLUSION
```

`channel-status-local` inspects the gateway (PID / socket / state) with
zero HTTP. `preflight` and `self-test` are network-free. The model API
key comes from `MUTAGEN_EXP_API_KEY` (the 8000 local proxy holds the
real key; the gateway forwards headers as-is).

## Validation

```sh
cargo fmt --all -- --check
cargo check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo run -- preflight
cargo run -- self-test
```

## Result

Pending — see
`docs/experiments/0015-final-clean-confirmation.md` (preregistration,
frozen at A0) and
`docs/experiments/0015-final-clean-confirmation-results.md`.
