# Experiment 0009 — Power-Aware Transfer-Validated Improvement Detection

Standalone experiment crate (its own workspace; **not** part of the
production workspace, **not** a production API). See
[`docs/experiments/0009-power-aware-evaluation.md`](../../docs/experiments/0009-power-aware-evaluation.md)
for the full pre-registered design.

## Research question

Can a candidate-blind, family-level difficulty calibration procedure
designed backward from a pre-registered informative-pair and
statistical-power requirement produce enough held-out information for
Mutagen's objective evaluator to detect a controlled robustness
improvement on a real stochastic model?

## Design in one picture

```
Phase A (baseline ONLY — candidate-blind)
  3 families × 2 development tasks × 5 stress levels × 5 repetitions
  = 150 episodes, registered cyclic stress order
        ↓ family-level rule:
        ↓   S0 sanity: BOTH dev tasks ≥ 4/5
        ↓   eligible:  BOTH dev tasks ≤ 2/5 (≤ 40%) at the same stress
        ↓   selected:  lowest eligible stress
  evaluation-plan.json  (frozen 48-pair budget, power constants, gates)
  → freeze + commit
        ↓
Phase B (held-out, frozen plan)
  baseline vs repair, selected families' held-out tasks,
  48 paired comparisons (96 episodes), alternating condition order
```

Phase B always executes exactly **48 pairs / 96 episodes** regardless of
whether two or three families calibrate (12 or 8 repetitions per
held-out task respectively).

## Layout

- `prompts/baseline.md`, `prompts/repair.md` — frozen before Phase A
- `tasks.json` — 12 tasks: 6 development + 6 held-out (spec §16–18)
- `stress-levels.json` — S0..S4 repeated silent-drop ladder
- `src/power.rs` — power design constants + conditional power (new)
- `src/calibration.rs` — two-variant family-level selector + plan type
- `src/trace.rs` — records, gates, summaries, verifiers, tamper tests
- `src/experiment.rs` — orchestration + network-free self-test
- `src/{kernel,model,protocol,tools,oracle,stats}.rs` — carried over
  from Experiments 0006/0008 (identical semantics)

## Commands

```sh
cargo run --manifest-path .../Cargo.toml -- self-test
cargo run --release --manifest-path .../Cargo.toml -- calibrate \
  --trajectories docs/experiments/artifacts/0009-calibration-trajectories.jsonl \
  --summary docs/experiments/artifacts/0009-calibration-summary.json \
  --plan experiments/0009-power-aware-evaluation/evaluation-plan.json
cargo run --manifest-path .../Cargo.toml -- verify-calibration ...
cargo run --release --manifest-path .../Cargo.toml -- evaluate ...
cargo run --manifest-path .../Cargo.toml -- verify-evaluation ...
```

`calibrate`/`evaluate` require `MUTAGEN_EXP_BASE_URL` and
`MUTAGEN_EXP_MODEL` (`MUTAGEN_EXP_API_KEY` optional); no fallback model.
`--code-commit` defaults to `git rev-parse HEAD`.
