# Experiment 0014 — Mechanically Audited Execution Channel Confirmation

## Purpose

Experiment 0013 completed all 264 episodes cleanly (the 0012 deadline
boundary failure did not recur — the deadline contract worked), but died
on the *inherited* channel discipline: after the A1 commit, an ad-hoc
endpoint availability probe issued **two unregistered HTTP GET requests
to `/v1/models`** on the registered endpoint, outside the registered
four-command stage path. Under the inherited 0012 execution-channel
discipline ("after this commit: no unregistered live model requests …"),
the 0013 run was not a formally clean run and its verdict was
**INCONCLUSIVE** (see
`docs/experiments/0013-deadline-safe-self-evolution-results.md` §4).

The structural weakness: **the 0012/0013 channel discipline is a promise
printed on stdout.** Nothing in the repository can observe a
`curl http://127.0.0.1:8000/v1/models` after A1.

**0014 is a narrow, confirmatory re-run of the exact 0013 pipeline with
exactly ONE semantic change: model traffic goes through a registered,
auditable local execution channel — a long-lived gateway process with
per-request HMAC authorization, an append-only hash-chained ledger, and
one-to-one reconciliation between experiment records and gateway
traffic — so the 0013 failure class (unregistered requests after A1)
becomes mechanically observable and mechanically INCONCLUSIVE.**
Everything else (deadline contract, source freeze, stage-freeze chain,
no-restart rule, mutation generation, selection, promotion, 18/150/96
episode chain, fresh `E14*`/`S14*`/`P14*` tasks) is carried over
unchanged from 0013.

> **Question:** with the execution channel made mechanical, does the
> frozen loop produce complete, reproducible, mechanically verifiable
> evidence — including *evidence about the channel itself* — and a
> formal SUPPORTED / REFUTED / INCONCLUSIVE verdict, in one run?

## The one 0014 semantic change

1. **The registered endpoint is the channel gateway**
   (`http://127.0.0.1:18140/v1` — `REGISTERED_ENDPOINT` in
   `src/model.rs`). The model client can address nothing else. The
   physical upstream (`http://127.0.0.1:8000/v1`, the 0013 local proxy)
   is channel-internal: readable only by `src/channel.rs` (a static
   preflight guard asserts the upstream literal appears nowhere else).
2. **One channel session per run** (`channel-manifest.json`, generated
   by `channel-init` before A1, HMAC-sealed, byte-immutable after A1):
   run identity, session id, protocol version, gateway endpoint,
   upstream-endpoint SHA-256, allowed path, ledger genesis hash.
3. **Per-request authorization** — every request carries
   `X-Mutagen-Run` / `X-Mutagen-Stage` / `X-Mutagen-Request-Id` /
   `X-Mutagen-Auth` headers; the auth value is
   `HMAC-SHA256(capability_key, run_id || stage || request_id ||
   SHA256(body))` over the vendored `sha2` crate (RFC 2104 / 4231
   vectors). Wrong body, wrong stage, replayed id, or missing
   capability → HTTP 403 + ledgered unauthorized.
4. **The hash-chained ledger** (`channel-ledger.jsonl`, append-only,
   fsynced per line) records **every** attempt — authorized or
   rejected — chained from the manifest genesis hash. Any edit,
   deletion, or reorder breaks the chain.
5. **One-to-one reconciliation** — every authorized/forwarded ledger
   row must match exactly one experiment record, and every recorded
   live request exactly one ledger row (artifacts are the authority).
   **Any unauthorized ledger row (even a correctly rejected probe)
   fails the run** — the 0013 failure is now mechanically observable.
6. **Single long-lived gateway, no restart** — spawned unarmmed by
   `channel-init`; armed over a local Unix control socket by
   `channel-arm` (after A1); finalized after Stage E. A PID change
   after start is an integrity failure. `channel-status-local`
   inspects PID / socket / state only (zero HTTP).

## Design (frozen; identical to 0013 except the channel)

- Model: `incoai/Qwen3.8-27B-Splash`, one registered endpoint — the
  gateway (`http://127.0.0.1:18140/v1`); the physical upstream
  `http://127.0.0.1:8000/v1` is channel-internal.
- Agent temperature 0.2; mutation temperature 0.7. Hard limits: 12
  model turns, 16 tool calls per episode; **deadline contract
  600 000 / 600 000 / 1 000 ms (0013 design fields, unchanged)**; one
  tool call per turn (`parallel_tool_calls: false`,
  `tool_choice: "auto"`).
- Frozen 0009 stress profile (no recalibration): S2 = 2 dropped writes,
  S1 = 1.
- 12 FRESH tasks (`E14*`/`S14*`/`P14*`, `V14_*` literal space,
  split-disjoint from every 0008–0013 literal): 3 discovery
  (18 episodes) / 3 selection (5 conditions × 30 cells = 150) /
  6 promotion (2 conditions × 48 pairs = 96).
- Gates (identical to 0013): discovery valid ≥ 16 / G0 failures ≥ 8;
  selection common valid ≥ 27 / G0 failures ≥ 12 / selected vs G0
  net margin > 0 (ties broken by G0); promotion valid pairs ≥ 44 /
  potential info ≥ 12 / actual informative ≥ 12; promotion infra
  failure rate ≤ 0.10.

## Artifact layout

- Raw trajectories + summaries: `docs/experiments/artifacts/0014-*.json[l]`
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
   frozen-path commit after A0 (history is the authority); no
   working-tree drift; A0 is an ancestor of HEAD.
2. **Previous-stage freeze** (`src/stage_freeze.rs`): B needs only the
   source freeze; C adds the B freeze; D adds the C freeze; E adds the
   D freeze and a non-G0 selected candidate.
3. **Channel gate** (`src/channel.rs`): the channel manifest seals and
   matches the registered design block; the gateway is alive with its
   origin PID (no restart); the ledger prefix re-verifies from genesis
   and matches the frozen previous-stage checkpoint; post-A1 stages
   additionally require an armed channel.

All three are exercised by `self-test` against real temporary git
repositories and in-process gateway fixtures.

## Usage (registered commands only)

```sh
# A0 (human/agent; the harness prints the exact commands):
cargo run -p mutagen-exp-0014 -- print-a0
# → git add <23 frozen files> && git commit   (then `preflight`)

# Channel session (local only; zero model contact):
cargo run -p mutagen-exp-0014 -- channel-init      # UNARMED gateway starts

# A1: run manifest + channel manifest (tree must exactly match A0):
cargo run -p mutagen-exp-0014 -- init-run
# → git add run-manifest.json channel-manifest.json && git commit

# Arm the channel (control socket only; zero HTTP):
cargo run -p mutagen-exp-0014 -- channel-arm

# Then the stage commands (each commits its artifacts + freeze):
cargo run -p mutagen-exp-0014 -- discover   # 18 episodes through the channel
cargo run -p mutagen-exp-0014 -- generate   # mutation (2 attempts)
cargo run -p mutagen-exp-0014 -- select     # 150 episodes
cargo run -p mutagen-exp-0014 -- promote    # 96 episodes → channel finalize → CONCLUSION
```

`channel-status-local` inspects the gateway (PID / socket / state) with
zero HTTP. `preflight` and `self-test` are network-free (both include
the channel section: static bypass guard + in-process behavioral suite
including the mechanized 0013 regression). The model API key comes from
`MUTAGEN_EXP_API_KEY` (held by the 8000 local proxy; the gateway
forwards it as-is).

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
`docs/experiments/0014-audited-execution-channel.md` (preregistration,
frozen at A0) and `docs/experiments/0014-audited-execution-channel-results.md`.
