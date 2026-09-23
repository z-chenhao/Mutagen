# Experiment 0012 — Frozen End-to-End Self-Evolution Confirmation

## Purpose

A frozen re-confirmation of the end-to-end self-evolution loop
(Evidence → Generate → Select → Promote), after the 0010/0011 runs showed
that the loop can run mechanically but that (a) a mid-run source change
voided the run identity, (b) mutation generation failed on structural
response-shape drift, and (c) the promotion evidence for one candidate
was not committed as an artifact, weakening the Stage-B provenance
claim.

0012 is a confirmatory re-run — NOT a new hypothesis. Its single question:

> With source, mutation-output schema, retry semantics, stage provenance,
> and endpoint discipline all frozen, does the loop produce complete,
> reproducible, mechanically verifiable evidence — and a formal
> SUPPORTED / REFUTED / INCONCLUSIVE verdict — in one run?

The hypothesis remains the same as 0011: on a live endpoint, the
incumbent (G0) is displaced at selection by a candidate generated
from discovery evidence, and the winner survives promotion against G0.

## Design (frozen, identical to 0011 except the four 0012 changes)

- Model: `incoai/Qwen3.8-27B-Splash`, one registered endpoint
  (`http://127.0.0.1:8000/v1`, named constant `REGISTERED_ENDPOINT` in
  `src/model.rs` — the design manifest is self-contained: a single
  `model` field; no per-request model override is possible).
- Agent temperature 0.2; mutation temperature 0.7. Hard limits: 12
  model turns, 16 tool calls per episode; a registered 600 s
  per-episode time budget (`EPISODE_TIME_LIMIT_MS` in `src/model.rs`);
  one tool call per turn.
- Frozen 0009 stress profile (no recalibration): S2 = 2 dropped writes,
  S1 = 1.
- 12 FRESH tasks (V12 literal space, split-disjoint): 3 discovery
  (18 episodes, 2 reps) / 3 selection (5 conditions × 30 cells = 150) /
  6 promotion (2 conditions × 48 pairs = 96).
- Gates (identical to 0011): discovery valid ≥ 16 / G0 failures ≥ 8;
  selection common valid ≥ 27 / G0 failures ≥ 12 / selected vs G0
  net margin > 0 (ties broken by G0); promotion common valid ≥ 44 /
  selected failures ≤ 12 / G0 failures ≥ 12; promotion infra
  failures ≤ 10.
- Wire semantics frozen: `parallel_tool_calls: false`,
  `tool_choice: "auto"`.

## The four 0012 changes (and only these)

1. **Explicit mutation-output JSON schema** — the frozen mutator prompt
   documents the exact response structure (`candidates`: an object with
   exactly four objects `{suffix, rationale}`, no other fields, no
   strings in the list), and the mutator enforces that exact shape.
2. **Shape-informative retry** — on structural failure the registered
   2-attempt protocol retries with the machine validation errors plus
   the exact expected structure (no performance/selection/cost
   feedback; no human rewrite; the previous raw response is retained
   in the conversation).
3. **Commit-level stage-artifact freeze** (closes the 0011 Stage-B
   provenance gap) — after each live stage, the artifacts (raw
   trajectories + summary + runtime evidence) are verified, manifested
   in a stage-freeze file (paths + SHA-256), and COMMITTED together.
   The next live stage refuses to run unless the previous stage's
   freeze verifies against Git: every frozen artifact must exist in
   the freeze commit with the recorded bytes, the freeze commit must
   post-date the code-under-test commit, and no later commit may have
   touched a frozen artifact (the 0010 mid-run change shape is caught
   even if the tree was later restored). `src/stage_freeze.rs`.
4. **No unregistered endpoint calls** — no `run`/`smoke`/`debug`/
   `probe` commands exist anywhere in the command table; the only live
   commands are the four registered stages. A live stage whose output
   already exists (it emitted its first request) refuses to run again:
   a crashed stage is INCONCLUSIVE — no restart, no resume, no rerun.

## Artifact layout

- Raw trajectories + summaries: `docs/experiments/artifacts/0012-*.json[l]`
  (registered; excluded from the source freeze; frozen per stage).
- Runtime evidence: this crate directory — `run-manifest.json` (A1),
  `mutation-input.json`, `candidate-pool.json`,
  `selected-candidate.json`, `stage-{b,c,d}-freeze.json` — all in the
  frozen code commit / source-freeze table.
- Stage freezes are written by the stage itself (B at discovery
  completion, C at generation completion, D at selection completion)
  and committed together with the artifacts they freeze. There is no
  Stage-E freeze: promotion is the final stage.

## The two mechanical guards (every live stage, before any request)

1. **Source freeze** (0011 mechanism, `src/freeze.rs`): all 21 frozen
   files must still carry their recorded bytes; no source-commit
   activity after the code-under-test commit (tree or history); the
   working tree must not differ from that commit.
2. **Previous stage freeze** (`src/stage_freeze.rs`): as in (3) above —
   B needs only the source freeze; C adds the B freeze; D adds the C
   freeze; E adds the D freeze and a non-G0 selected candidate.

## Usage (registered commands only)

```sh
# A0 (human/agent; the harness prints the exact commands):
cargo run -p mutagen-exp-0012 -- print-a0-commands

# A1: run manifest (tree must exactly match the A0 commit):
cargo run -p mutagen-exp-0012 -- init-run
# → git add run-manifest.json && git commit; then the stage commands:
cargo run -p mutagen-exp-0012 -- discover   # 18 episodes
# → git add <artifacts + stage-b-freeze.json> && git commit
cargo run -p mutagen-exp-0012 -- generate   # mutation (2 attempts)
# → git add <artifacts + stage-c-freeze.json> && git commit
cargo run -p mutagen-exp-0012 -- select     # 150 episodes
# → git add <artifacts + stage-d-freeze.json> && git commit   (STOP if G0 retained: REFUTED)
cargo run -p mutagen-exp-0012 -- promote    # 96 episodes → CONCLUSION
```

`preflight` and `self-test` are network-free. The model API key comes
from `MUTAGEN_EXP_API_KEY`.

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

Pending — see `docs/experiments/0012-frozen-self-evolution-confirmation.md`.
