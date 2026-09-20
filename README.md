# Mutagen

**Evolution infrastructure for high-performance AI agents.**

> ⚠️ **Early-stage / experimental.** Mutagen is in *Phase 0: repository
> bootstrap*. It does not currently self-evolve, run an agent, or call any
> model. This repository is the foundation on which that will be built —
> carefully, and from evidence.

## Motivation

Today's AI agents are static: their prompts, context, memory, tools, and
workflows are hand-crafted and frozen. Mutagen's goal is to make an agent a
system that **improves itself from its own experience** — a role reversal
where the agent doesn't only use AI, but AI is also used *on* the agent:

```
user feedback / execution experience
        ↓
capture → replay → analyze → propose component changes
        ↓
evaluate → validate → version → deploy / hot-swap
        ↓
observe → (rollback if necessary)
```

The architecture for this loop is **deliberately undecided**. It will be
derived from reproducible experiments, not assumed. That discipline is the
product: a high-performance Rust foundation whose every abstraction is
justified by evidence.

## Current scope (Phase 0)

What this repository contains *today*:

- A Cargo workspace with a strict dependency direction:
  `mutagen-cli → mutagen-runtime → mutagen-core`
- `mutagen-core` — a tiny, dependency-free set of foundational domain types
  (`ComponentId`, `EpisodeId`)
- `mutagen-runtime` — an intentionally empty stub fixing the dependency seam
- `mutagen-cli` — the `mutagen` binary: `--version` and `doctor`
- Engineering infrastructure: CI, ADRs, an experiment contract, and an
  operating manual for AI coding agents ([`AGENTS.md`](AGENTS.md))

## Non-goals (for now)

Mutagen does **not** include, and will not add without a recorded decision:

- LLM providers, model routing, or any network I/O
- agent loops, memory, tools, skills, workflows
- self-modification, prompt optimization, or any evolution algorithm
- plugin/hot-reload mechanisms, dynamic loading, or sandboxing
- databases, HTTP servers, or UIs

See [`docs/evolution.md`](docs/evolution.md) for the research agenda and
[`docs/decisions/0000-project-principles.md`](docs/decisions/0000-project-principles.md)
for the principles that gate all of the above.

## Workspace overview

| Crate | Responsibility | Dependencies |
|---|---|---|
| `mutagen-core` | Foundational domain types; no I/O, no global state | *(none)* |
| `mutagen-runtime` | Future orchestration layer (stub in Phase 0) | `mutagen-core` |
| `mutagen-cli` | The `mutagen` binary | `mutagen-runtime`, `clap` (derive) |

Third-party dependencies: `clap` (argument parsing, in the CLI only).
Every dependency must have a concrete current use — this is enforced by
review, per [`AGENTS.md`](AGENTS.md).

## Quick start

Requires stable Rust ≥ 1.85 (Rust 2024 edition; `rust-toolchain.toml`
pins the channel).

```sh
cargo run -p mutagen-cli -- --version
cargo run -p mutagen-cli -- doctor
```

`mutagen doctor` runs deterministic local sanity checks (toolchain version,
core type round-trips, workspace wiring). It never touches the network and
needs no API keys.

## Development

```sh
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo run -p mutagen-cli -- doctor
```

All five must pass before a change is considered complete. The same suite
runs in CI on every push and pull request.

## Research direction

The self-evolution loop above is a hypothesis, not a plan. Open questions
— what should be evolvable, how replay should work, what a candidate is,
how promotion and rollback should behave — are tracked in
[`docs/evolution.md`](docs/evolution.md). Experiment records will live in
[`docs/experiments/`](docs/experiments/README.md); architecture decisions
in [`docs/decisions/`](docs/decisions/).

On performance: extreme performance is a design input from day one (stable
Rust, no `unsafe`, minimal dependencies, replay-friendly data shapes), but
no optimization exists yet and none will be added without profiling
evidence. There are intentionally no performance claims to make.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). In one sentence: small,
evidence-backed changes; the architecture is undecided on purpose.

## License

[Apache License 2.0](LICENSE)
