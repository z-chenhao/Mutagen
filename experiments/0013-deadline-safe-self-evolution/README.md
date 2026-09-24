# Experiment 0013 — Deadline-Safe End-to-End Self-Evolution Confirmation

## Purpose

Experiment 0012's frozen loop worked exactly as designed and died at
Stage D for a **2 ms boundary disagreement**: a fully runtime-conformant
deadline-straddling timeout (request started inside the deadline,
transport timeout bounded by the remaining budget, returned 2 ms past
the 600 000 ms deadline) was flagged as record corruption by an aggregate
wall-time verifier rule. The two registered contracts — the runtime's
("no request may *start* after the deadline") and the verifier's
("aggregate wall time must not exceed the deadline") — disagreed about
what a legitimate blocking-timeout return means.

**0013 is a narrow, confirmatory re-run of the exact 0012 pipeline with
exactly ONE semantic change: explicit, internally consistent
episode-deadline semantics, backed by request-level timing evidence.**
Everything else (source freeze, stage-freeze chain, no-restart rule,
explicit mutation schema, retry semantics, anti-seeding, selection rule,
sign-test promotion, 18/150/96 episode chain, artifacts) is carried over
from 0012.

> **Question:** with the deadline contract made explicit in the design
> authority, enforced by the kernel, and verifiable from per-request
> evidence, does the frozen loop produce complete, reproducible,
> mechanically verifiable evidence — and a formal SUPPORTED / REFUTED /
> INCONCLUSIVE verdict — in one run?

## The one 0013 semantic change

The deadline contract lives in `design.json` (the single source of
truth; the 0012 model-client constants are deleted):

```json
"deadline": {
  "episode_deadline_ms": 600000,
  "request_timeout_ceiling_ms": 600000,
  "deadline_return_tolerance_ms": 1000
}
```

1. **No request may start at or after the deadline** — the kernel checks
   before every request; otherwise the episode ends with
   `episode_deadline_reached` (infra) and no request record is emitted.
2. **Exact registered timeout** — each request is issued with
   `requested_timeout_ms = min(remaining episode budget,
   request_timeout_ceiling_ms)`; the model client holds no deadline
   constant of its own.
3. **Response acceptance** — a response returning after the deadline is
   *discarded* (never enters the transcript, never a tool call, never
   the final answer, no subsequent request) and the episode ends with
   `episode_deadline_exceeded` (infra). A return up to
   `deadline_return_tolerance_ms` (1 s) past the deadline is a **valid**
   infrastructure outcome — blocking timeouts are not instantaneous.
   Beyond the tolerance is a hard timing-integrity violation.
4. **Request-level evidence** — every request record carries
   `request_started_elapsed_ms`, `requested_timeout_ms`,
   `request_finished_elapsed_ms`, `timed_out`, `response_accepted`.
   The verifier re-derives the contract per request (see the
   preregistration §2.4); the 0012 aggregate wall-time rule is removed
   (aggregate wall time is diagnostic-only, with a loose corruption
   bound of 2×deadline + tolerance).

## Design (frozen, identical to 0012 except the deadline contract)

- Model: `incoai/Qwen3.8-27B-Splash`, one registered endpoint
  (`http://127.0.0.1:8000/v1`, named constant `REGISTERED_ENDPOINT` in
  `src/model.rs` — a URL, not a count or gate; no per-request model or
  endpoint override).
- Agent temperature 0.2; mutation temperature 0.7. Hard limits: 12
  model turns, 16 tool calls per episode; **deadline contract
  600 000 / 600 000 / 1 000 ms (design fields)**; one tool call per turn
  (`parallel_tool_calls: false`, `tool_choice: "auto"`).
- Frozen 0009 stress profile (no recalibration): S2 = 2 dropped writes,
  S1 = 1.
- 12 FRESH tasks (`E13*`/`S13*`/`P13*`, `V13_*` literal space,
  split-disjoint from every 0008–0012 literal): 3 discovery
  (18 episodes) / 3 selection (5 conditions × 30 cells = 150) /
  6 promotion (2 conditions × 48 pairs = 96).
- Gates (identical to 0012): discovery valid ≥ 16 / G0 failures ≥ 8;
  selection common valid ≥ 27 / G0 failures ≥ 12 / selected vs G0
  net margin > 0 (ties broken by G0); promotion valid pairs ≥ 44 /
  potential info ≥ 12 / actual informative ≥ 12; promotion infra
  failure rate ≤ 0.10.

## Artifact layout

- Raw trajectories + summaries: `docs/experiments/artifacts/0013-*.json[l]`
  (registered; excluded from the source freeze; frozen per stage by the
  stage-freeze mechanism).
- Runtime evidence: this crate directory — `run-manifest.json` (A1),
  `mutation-input.json`, `candidate-pool.json`,
  `selected-candidate.json`, `stage-{b,c,d}-freeze.json`.
- Stage freezes are written by the stage itself (B at discovery
  completion, C at generation completion, D at selection completion)
  and committed together with the artifacts they freeze. There is no
  Stage-E freeze: promotion is the final stage.

## The two mechanical guards (every live stage, before any request)

1. **Source freeze** (`src/freeze.rs`, 21 files): every frozen file's
   bytes must hash to the manifest value and match the A0 commit; no
   frozen-path commit after A0 (history is the authority); no
   working-tree drift; A0 is an ancestor of HEAD.
2. **Previous-stage freeze** (`src/stage_freeze.rs`): B needs only the
   source freeze; C adds the B freeze; D adds the C freeze; E adds the
   D freeze and a non-G0 selected candidate.

Both are exercised by `self-test` against real temporary git
repositories.

## Usage (registered commands only)

```sh
# A0 (human/agent; the harness prints the exact commands):
cargo run -p mutagen-exp-0013 -- print-a0

# A1: run manifest (tree must exactly match the A0 commit):
cargo run -p mutagen-exp-0013 -- init-run
# → git add run-manifest.json && git commit; then the stage commands:
cargo run -p mutagen-exp-0013 -- discover   # 18 episodes
# → git add <artifacts + stage-b-freeze.json> && git commit
cargo run -p mutagen-exp-0013 -- generate   # mutation (2 attempts)
# → git add <artifacts + stage-c-freeze.json> && git commit
cargo run -p mutagen-exp-0013 -- select     # 150 episodes
# → git add <artifacts + stage-d-freeze.json> && git commit   (STOP if G0 retained: REFUTED)
cargo run -p mutagen-exp-0013 -- promote    # 96 episodes → CONCLUSION
```

`preflight` and `self-test` are network-free (preflight includes the
deadline-contract section; self-test includes the same shared checks
plus the full 0012 suite). The model API key comes from
`MUTAGEN_EXP_API_KEY`.

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
`docs/experiments/0013-deadline-safe-self-evolution.md` (preregistration,
frozen at A0) and `docs/experiments/0013-deadline-safe-self-evolution-results.md`.
