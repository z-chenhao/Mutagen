# Architecture

This document describes the architecture that **actually exists** in the
repository. It is deliberately short: we do not document imagined
subsystems as though they were real.

## Implemented today (Phase 0)

A three-crate Cargo workspace with a strict dependency direction:

```
mutagen-cli  →  mutagen-runtime  →  mutagen-core
   (binary)        (stub)            (domain types)
```

### mutagen-core

Pure, dependency-free foundational domain types:

- `EpisodeId` — opaque identifier for a recorded run.

Component identity, revision identity, versioning, and lineage are
intentionally undefined in the codebase until experiments establish their
requirements (see [`evolution.md`](evolution.md)).

No I/O, no global state, no `unsafe`. The public API is intentionally
minimal until experiments justify more (see the crate's `lib.rs`).

### mutagen-runtime

An empty stub. Its only current job is to fix the workspace dependency
direction and prove the seam compiles through CI. It exports a `VERSION`
constant consumed by the CLI's doctor check.

### mutagen-cli

The `mutagen` binary with:

- `mutagen --version`
- `mutagen doctor` — deterministic local sanity checks (toolchain version,
  episode id round-trip, workspace wiring). No network, no API keys.

Third-party dependency of the workspace: `clap` (derive) in
`mutagen-cli`, for argument parsing. Nothing else.

## Not implemented (do not document as real)

Agent loops, model providers, memory, tools, skills, workflows, replay,
evaluation, versioning registries, hot deployment, plugins, rollback, and
all evolution mechanisms are **not implemented**. They are research
directions tracked in [`evolution.md`](evolution.md) and will be decided by
experiments in [`experiments/`](experiments/).

## Expected future shape (hypothesis, unverified)

When experiments mature, the workspace is expected to grow *around* the
existing direction (core stays at the bottom), but **no specific future
crate, trait, or subsystem is committed** until an ADR records it.
