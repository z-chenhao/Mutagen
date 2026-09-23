# Experiment 0010 — First Controlled Self-Evolution Loop

**Status: complete.** See
[`docs/experiments/0010-controlled-self-evolution.md`](../../docs/experiments/0010-controlled-self-evolution.md)
for the pre-registration (frozen at commit A) and the filled-in results.

## Hypothesis

A model can be *prompted* to improve its own agent policy from observed
behavior, and that improvement generalizes to untouched tasks — **when**
the loop is closed with three disjoint task splits and pre-registered
frozen rules. The evolvable surface is exactly one append-only policy
suffix (≤100 words, ≤800 bytes); the harness, kernel, tools, faults,
oracle, and all task state are frozen and identical across conditions.

## Pipeline

```
DISCOVER (18 episodes, 3 tasks × 6 reps, discovery split only)
  └→ mutation input (whitelisted, leakage-verified)
GENERATE (≤2 mutator calls, same model, temp 0.7 → frozen pool C1..C4)
SELECT   (150 episodes, 3 tasks × 5 reps × 5 conditions, cyclic order)
  └→ deterministic selection (net margin > 0 required; ordinal tie-break)
PROMOTE  (96 episodes, 6 tasks × 4 reps × 2 conditions, alternating)
  └→ exact sign test + pre-registered gates → supported / refuted / inconclusive
```

Every stage's artifacts (JSONL episodes + JSON summary/freeze) are
re-verified by offline stage verifiers that re-derive every check from
the frozen configuration: conversation invariants, fault-audit replay
against the final state, oracle recomputation, cost recomputation,
summary deep-compare, and tamper detection (27 tamper cases in the
self-test, all detected).

## Files

| file | role |
|---|---|
| `src/kernel.rs` | turn loop, 12-turn / 16-call limits, deterministic faults |
| `src/model.rs` | model client (endpoint from env; redacted in artifacts) |
| `src/protocol.rs` | message / usage / canonical-JSON / hashing |
| `src/tools.rs` | state_read / state_write executor + `DropFirstNWrites` |
| `src/oracle.rs` | task registry types; independent final-state judge |
| `src/trace.rs` | episode record schemas, summaries, verifiers, synthetic set |
| `src/mutation.rs` | mutation input, suffix validation, pool, leakage checks |
| `src/selection.rs` | frozen selection rule (net margin > 0; ordinal tie-break) |
| `src/stats.rs` | exact sign test + quality classification |
| `src/experiment.rs` | the four stage orchestration functions + self-test |
| `src/main.rs` | CLI: `self-test`, `discover/generate/select/promote`, `verify-*` |
| `prompts/incumbent.md` | frozen incumbent policy prompt |
| `prompts/mutator.md` | mutation operator prompt (model, not code, proposes) |
| `tasks.json` | 12 tasks: 3 discovery / 3 selection / 6 promotion |
| `stress-profile.json` | the frozen Experiment 0009 per-family stresses |
| `candidate-pool.json`, `mutation-input.json`, `selected-candidate.json`, `0010-mutation-generation.json` | frozen intermediate artifacts |

## Commands

```sh
cargo run -- self-test
MUTAGEN_EXP_BASE_URL=... MUTAGEN_EXP_MODEL=... cargo run -- discover \
  --trajectories 0010-discovery-episodes.jsonl \
  --summary 0010-discovery-summary.json \
  --mutation-input mutation-input.json
cargo run -- verify-discovery --trajectories ... --summary ... --mutation-input ...
MUTAGEN_EXP_BASE_URL=... MUTAGEN_EXP_MODEL=... cargo run -- generate \
  --mutation-input mutation-input.json \
  --generation-artifact 0010-mutation-generation.json \
  --candidate-pool candidate-pool.json
cargo run -- verify-mutation --generation-artifact ... --candidate-pool ... --mutation-input ...
MUTAGEN_EXP_BASE_URL=... MUTAGEN_EXP_MODEL=... cargo run -- select \
  --candidate-pool candidate-pool.json \
  --trajectories 0010-selection-episodes.jsonl \
  --summary 0010-selection-summary.json \
  --selected selected-candidate.json
cargo run -- verify-selection --trajectories ... --summary ... --selected ... --candidate-pool ... --mutation-input ...
MUTAGEN_EXP_BASE_URL=... MUTAGEN_EXP_MODEL=... cargo run -- promote \
  --selected selected-candidate.json \
  --trajectories 0010-promotion-episodes.jsonl \
  --summary 0010-promotion-summary.json
cargo run -- verify-promotion --selected ... --candidate-pool ... --trajectories ... --summary ... --mutation-input ...
```

No network is used by `self-test` or any `verify-*`. Model endpoint and
name come from `MUTAGEN_EXP_BASE_URL` / `MUTAGEN_EXP_MODEL`; the
endpoint is redacted in every artifact (model name is public, endpoint
is not).

## What this experiment is deliberately not

- Not a claim that the selected suffix is generally better. It is one
  controlled instance of a closed loop: one model, one task family
  design, one pre-registered confirmatory test.
- Not self-modifying code. The evolvable surface is a policy suffix in
  a prompt; the harness that proposes it never executes its own
  output as code.
- Not a guarantee against future leakage. The leakage verifiers pin
  today's boundaries (discovery-only input, frozen pool, frozen
  selected candidate); a new surface needs a new verifier.
