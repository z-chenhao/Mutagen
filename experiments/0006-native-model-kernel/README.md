# Experiment 0006 — Pi-Informed Rust-Native Real-Model Kernel

Standalone experiment crate. **Not part of the production workspace** —
the empty `[workspace]` table detaches it from the repository root.
Nothing here is a production API and nothing is promoted to
`mutagen-core` / `mutagen-runtime` / `mutagen-cli`.

## What it is

A deliberately small, synchronous, Rust-owned agent kernel that drives a
**real Qwen model** through a fixed six-task suite under two prompt
conditions (baseline vs. a one-line mutation), recording authoritative
tool trajectories and computing pre-registered divergence metrics.

```
main.rs            CLI: run / verify-artifacts (presentation & control)
  └─ experiment.rs task suite, prompts, ordering, artifacts, summary
      └─ kernel.rs    the episode loop (messages, turns, limits, events)
          ├─ model.rs    one concrete OpenAI-compatible client (sync ureq)
          ├─ tools.rs    state_read / state_write over fresh ExternalState
          ├─ protocol.rs chat-completions wire types, parsing, canonical IDs
          └─ trace.rs    events, tags, verification, summary, verifier
```

The kernel is the authoritative execution recorder: it emits a small
closed event set into its own vector. Pi (the reference project studied
in `PI_REFERENCE.md`) is a *design reference only* — it is not a
dependency, not an SDK, not a subprocess, and owns nothing in the
execution path.

## Layout

```
Cargo.toml  (standalone workspace; deps: serde, serde_json, ureq, sha2)
PI_REFERENCE.md   Phase A: the Pi design study (adopt / reject / defer)
prompts/baseline.md, prompts/candidate.md
tasks.json        the six fixed tasks
src/              main, kernel, model, protocol, tools, trace, experiment
```

## Running

Requires a local or reachable OpenAI-compatible endpoint serving Qwen:

```sh
export MUTAGEN_EXP_BASE_URL="http://127.0.0.1:8000/v1"
export MUTAGEN_EXP_MODEL="<qwen-model-id>"
export MUTAGEN_EXP_API_KEY="<optional>"

cargo run --release \
  --manifest-path experiments/0006-native-model-kernel/Cargo.toml \
  -- run --repetitions 3 \
     --trajectories docs/experiments/artifacts/0006-trajectories.jsonl \
     --summary docs/experiments/artifacts/0006-summary.json
```

If the environment variables are missing, the run is *not executed* and
no fallback model is substituted.

Verify artifacts:

```sh
cargo run --manifest-path experiments/0006-native-model-kernel/Cargo.toml \
  -- verify-artifacts \
     --trajectories docs/experiments/artifacts/0006-trajectories.jsonl \
     --summary docs/experiments/artifacts/0006-summary.json
```

The verifier recomputes the summary from the raw JSONL and checks episode
count, run-ID uniqueness, hash/model consistency, tool-call validity,
absence of reasoning/secret fields, and summary consistency.

## Design notes

- **Synchronous on purpose.** 0006 tests semantics (ownership, turn
  semantics, protocol, dispatch, capture, termination), not
  throughput. No async runtime.
- **No named model trait.** One provider exists. The kernel takes a
  call *closure*; `ModelClient::complete` implements it in production
  runs and scripted closures in tests. No `ModelProvider` abstraction.
- **No general event bus.** One consumer: the kernel's own
  `Vec<TrajectoryEvent>`.
- **Capability by construction.** The model is given exactly
  `state_read` and `state_write`. No shell, filesystem, browser, or
  network tools; the only network I/O is the model HTTP request.
- **Fresh state per episode.** Each episode constructs a new
  `ToolExecutor` (`x=EMPTY`, `y=B`); no global mutable state.
- **`#![forbid(unsafe_code)]`** in the crate root.

## Scope guards

This experiment does **not**: evaluate candidate quality, select or
promote a mutation, run Pi, benchmark against Pi, persist sessions,
branch, compact, or switch models. It records divergence and computes
pre-registered, unopinionated metrics. See
`../../docs/experiments/0006-native-real-model-kernel.md` for the
research record.
