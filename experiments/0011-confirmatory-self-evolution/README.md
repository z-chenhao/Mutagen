# mutagen-exp-0011 — Clean Confirmatory Self-Evolution Rerun

Standalone experiment crate (its own cargo workspace) implementing the
self-evolution loop — **Evidence → Generate → Select → Promote** — under
a **mechanically enforced frozen execution protocol** (the decisive fix
over Experiment 0010, which was voided as inconclusive when its source
changed mid-run).

See [`docs/experiments/0011-confirmatory-self-evolution.md`](../../docs/experiments/0011-confirmatory-self-evolution.md)
for the immutable pre-registration.

## Source freeze (the key mechanism)

`src/freeze.rs` defines `FROZEN_PATHS` (20 files) and the run manifest.
`init-run` (Stage A1) hashes every frozen file into `run-manifest.json`;
**every live stage runs `verify_run_freeze` before its first model
request** — current bytes, git-blob-at-A0, empty post-A0 commit history
on frozen paths, clean working tree, commit ancestry, manifest
self-consistency. Any violation permanently invalidates the run (no
patching, no continuation). `self-test` exercises the guard against real
temporary git repositories.

## The loop

| Stage | Episodes | Notes |
|---|---|---|
| A0 / A1 | — | frozen code commit / run manifest |
| B discovery | 18 | 3 E11 tasks × 6 reps, G0, frozen 0009 stress |
| C mutation generation | ≤2 reqs | 4 suffix candidates; same model; no tools; nothing from 0010 seeded |
| D selection | 150 | 3 S11 × 10 reps × {G0,C1..C4}, cyclic order |
| E promotion | 96 | 6 P11 × 8 reps × {G0,selected} → three-way conclusion |

Every executable value (counts, gates, temperatures, limits, α,
endpoints) lives in `design.json` — the single source of truth.

## Commands

```sh
cargo run -p mutagen-exp-0011 -- self-test               # network-free
cargo run -p mutagen-exp-0011 -- preflight               # network-free
cargo run -p mutagen-exp-0011 -- init-run --commit <A0>  # Stage A1
cargo run -p mutagen-exp-0011 -- discover                # Stage B (live)
cargo run -p mutagen-exp-0011 -- generate                # Stage C (live)
cargo run -p mutagen-exp-0011 -- select                  # Stage D (live)
cargo run -p mutagen-exp-0011 -- promote                 # Stage E (live)
cargo run -p mutagen-exp-0011 -- verify-discovery        # independent re-checks
cargo run -p mutagen-exp-0011 -- verify-mutation
cargo run -p mutagen-exp-0011 -- verify-selection
cargo run -p mutagen-exp-0011 -- verify-promotion
cargo run -p mutagen-exp-0011 -- run --task E11A        # ad-hoc smoke (not part of the run)
```

Artifacts written into the crate directory: `run-manifest.json`,
`discovery-raw.jsonl`, `discovery-summary.json`, `mutation-input.json`,
`mutation-generation.json`, `candidate-pool.json`,
`selection-raw.jsonl`, `selection-summary.json`,
`selected-candidate.json`, `promotion-raw.jsonl`,
`promotion-summary.json`. All are committed with the final PR.

## Module map

`protocol.rs` wire protocol · `model.rs` OpenAI-compatible client (one
endpoint; per-request timeout parameter) · `kernel.rs` turn loop (limits
from design) · `tools.rs` state + silent-drop fault · `oracle.rs` state
judge + registry · `mutation.rs` suffix validation/generation ·
`selection.rs` lexicographic rule · `stats.rs` exact sign test ·
`freeze.rs` source-freeze system · `trace.rs` design manifest, records,
verifiers, synthetic tamper suite · `experiment.rs` commands/stages.
