# Experiment 0007 — Candidate Evaluator Discrimination on Real Model Candidates

Standalone experiment crate. **Not part of the production workspace** —
the empty `[workspace]` table detaches it from the repository root.
Nothing here is a production API and nothing is promoted to
`mutagen-core` / `mutagen-runtime` / `mutagen-cli`.

**Duplication is deliberate.** Experiment 0007 copies only the minimal
kernel / protocol ideas validated in Experiment 0006 rather than
depending on the 0006 crate: two experiments are not yet enough evidence
to promote a shared production abstraction, and 0007 must stay
independent so either experiment can be discarded without affecting the
other.

## Research question

Can a deterministic task oracle, applied to repeated real-model
executions through the Rust-native Mutagen kernel, reliably distinguish
a controlled prompt mutation expected to *improve* task success from a
controlled prompt mutation expected to *regress* task success on
held-out tasks?

This validates the **evaluation substrate**. It is not self-evolution:
no model generates mutations, no candidate is promoted, no winner is
deployed.

## What it is

- A Rust-owned kernel (same validated principles as 0006: typed
  messages, two explicit tools, hard limits, one tool call per
  assistant turn, presentation outside the kernel) driving a real Qwen
  model through **8 tasks × 3 prompt conditions × 6 repetitions = 144
  episodes**, with all six condition-order permutations to
  counterbalance cache/position bias.
- A **deterministic external-state oracle** (`oracle.rs`) that grades
  each episode from *task spec + termination category + initial/final
  state only*. It has no field for condition, prompt, or candidate name
  — anti-circularity is structural.
- **Deterministic fault injection** (`tools.rs`): `Reliable` and
  `DropFirstWrite { key }` — a one-shot *silent* write fault that the
  model cannot see (the tool still reports success) and can only
  discover by reading state; audited in an experiment-only
  `environment_write_attempts` log that never enters the LLM
  transcript.
- Per-request **cache-aware usage accounting** (`protocol.rs`,
  `trace.rs`): `cached_prompt_tokens` and `reasoning_tokens` are
  `null` when the provider does not report them — reproducible from
  committed artifacts, never inferred from server logs or latency.
- An **exact two-sided sign test** (`stats.rs`, standard library only)
  over held-out (task, repetition) pairs, with pre-registered
  quality classification and a pre-registered experiment-level
  conclusion rule.

## Layout

```
Cargo.toml            standalone workspace; deps: serde, serde_json, ureq, sha2
prompts/              baseline.md, repair.md, regression.md  (frozen before any run)
tasks.json            8 tasks: C1–C4 calibration, H1–H4 held-out
src/
  main.rs             CLI: run / self-test / verify-artifacts
  experiment.rs       prompts, invariants, registry, run orchestration, self-test
  kernel.rs           the episode loop (turns, limits, usage, requests)
  model.rs            one concrete synchronous OpenAI-compatible client
  protocol.rs         chat-completions wire types incl. optional usage detail
  tools.rs            state_read / state_write + fault-injection environment
  oracle.rs           the deterministic final-state task oracle
  stats.rs            exact sign test + quality classification
  trace.rs            artifact records, summary, verifier, synthetic dataset
```

## Running

```sh
export MUTAGEN_EXP_BASE_URL="http://127.0.0.1:8000/v1"
export MUTAGEN_EXP_MODEL="<qwen-model-id>"
export MUTAGEN_EXP_API_KEY="<optional>"

# Network-free self-test (no model calls):
cargo run --manifest-path experiments/0007-candidate-evaluator/Cargo.toml -- self-test

# The real run (144 episodes, must be release mode per spec):
cargo run --release \
  --manifest-path experiments/0007-candidate-evaluator/Cargo.toml \
  -- run --repetitions 6 \
     --trajectories docs/experiments/artifacts/0007-trajectories.jsonl \
     --summary docs/experiments/artifacts/0007-summary.json

# Verify artifacts:
cargo run --manifest-path experiments/0007-candidate-evaluator/Cargo.toml \
  -- verify-artifacts \
     --trajectories docs/experiments/artifacts/0007-trajectories.jsonl \
     --summary docs/experiments/artifacts/0007-summary.json
```

If the environment variables are missing, the run is *not executed* and
no fallback model is substituted. The run makes no resume mechanism: an
interrupted 144-run fails verification and the experiment is
re-run from a clean output path.

## Provenance

The raw artifact (`0007-trajectories.jsonl`) is immutable after the run
(see SHA-256 in the experiment record). The summary is a derived,
regenerable file; the verifier recomputes it from the raw records and
requires deep equality.
