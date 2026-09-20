# AGENTS.md — Operating manual for coding agents

This file is the primary context document for AI coding agents working in
this repository. Read it fully before making changes.

## Repository purpose

**Mutagen** is evolution infrastructure for high-performance AI agents.

What Mutagen **is** (at this time):

- A Rust workspace laying the engineering foundation for an agent system
  that can *eventually* evolve parts of itself from real user feedback and
  execution experience.
- Currently: a Phase 0 repository — clean tooling, CI, documentation, and a
  deliberately tiny, well-documented core.

What Mutagen **is not** (yet, or at all):

- Not an agent framework, chatbot, or LLM client.
- Not currently self-evolving. Nothing in this repository changes itself.
- Not a generic plugin system, workflow engine, or benchmark platform.

The long-term target loop (research direction, **not** current behavior):

```
feedback / experience → capture → replay → analyze → propose changes
→ evaluate → validate → version → deploy/hot-swap → observe → rollback
```

Candidate evolvable surfaces (hypotheses only): prompts, context
construction, memory, skills, tools, routing policies, workflows, runtime
components, evaluation policies, evolution strategies. See
[`docs/evolution.md`](docs/evolution.md).

## Current project phase

**Phase 0 — Repository Bootstrap.**

The repository compiles, tests, and passes CI. The only functional code is
`mutagen --version` and `mutagen doctor`. All self-evolution architecture
is intentionally undecided (see
[`docs/decisions/0000-project-principles.md`](docs/decisions/0000-project-principles.md)).

## Architectural invariants

1. **Dependency direction** (must never be violated):

   ```
   mutagen-cli → mutagen-runtime → mutagen-core
   ```

   Lower crates must not depend on higher crates. `mutagen-core` is the
   foundation: dependency-free, deterministic, no I/O, no global state.

2. **No speculative evolution abstractions.** Do not introduce traits or
   types such as `EvolutionEngine`, `Mutator`, `HotReloadable`,
   `GeneticOptimizer`, or similar, unless a completed experiment
   demonstrates the need and a decision is recorded in `docs/decisions/`.

3. **Core stays minimal.** `mutagen-core` contains foundational domain
   types only. Model providers, HTTP, databases, plugin loading, hot
   reload, and evolution algorithms never belong there without an ADR.

4. **No `unsafe`.** `unsafe_code = "forbid"` is enforced at workspace
   level. Revisiting this requires profiling evidence and an ADR.

5. **No speculative dependencies.** Every dependency must have a concrete
   current use (see the dependency list in `README.md`).

## Development commands

Use exactly these; they mirror CI:

```sh
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo run -p mutagen-cli -- doctor
```

Run all five before considering any task complete. Do not suppress warnings
to pass CI; fix the underlying issue or record a targeted, explained
exception in an ADR.

## Change discipline

1. Do not introduce abstractions without a demonstrated requirement.
2. Do not add dependencies speculatively.
3. Keep components small.
4. Preserve deterministic behavior where possible.
5. Add tests for behavioral changes.
6. Run the complete validation suite (all five commands above) before
   considering a task complete.
7. Do not silently change architectural invariants — if you must, write an
   ADR in `docs/decisions/` in the same change.
8. Record important architectural decisions in `docs/decisions/`
   (numbered `NNNN-title.md`).
9. Evolution-related changes should be backed by experiments (see
   `docs/experiments/`), not intuition.
10. Do not modify unrelated files.

## Performance philosophy

Performance is a first-class, *future* concern. Phase 0 code must not be
prematurely optimized: no custom allocators, no SIMD, no unsafe, no manual
thread pools, no benchmarks of placeholder code. Keep dependency count low,
avoid obvious unnecessary allocations and clones, and keep interfaces
profiling-friendly. Future optimization must be profile-driven and recorded
in an ADR.
