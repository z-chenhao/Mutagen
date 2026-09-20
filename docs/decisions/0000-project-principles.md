# 0000 — Project principles

- Status: accepted
- Phase: 0 (repository bootstrap)

## Context

Mutagen will be developed by humans and AI coding agents alike, and its
end goal (self-evolving agent infrastructure) is architecturally
unresolved. We need shared principles that keep every future contribution —
human or agent — aligned before any architecture exists.

## Principles

1. **Performance is a first-class concern.** Throughput, latency, and
   memory are design inputs from day one, but optimization is always
   profile-driven, never speculative.
2. **Evidence before abstraction.** No trait, interface, or module exists
   until a real consumer or completed experiment demands it.
3. **Experiment before architecture.** The evolution architecture will be
   derived from reproducible experiments, not designed top-down on a whiteboard.
4. **Explicit state over hidden state.** No global mutable state, no hidden
   configuration; state that matters is named, owned, and inspectable.
5. **Replayability.** Runs must be recordable and re-executable; this is a
   prerequisite for all validation and a design constraint on everything.
6. **Observability.** Every component exposes enough signal (timings,
   decisions, versions) to diagnose it after the fact.
7. **Versionability.** Evolvable artifacts carry stable, versioned
   identities; lineage is trackable.
8. **Reversibility.** Every promotion of a change has a corresponding
   rollback path.
9. **Minimal dependencies.** Every dependency has a concrete current use.
10. **Stable interfaces emerge from evidence.** We do not "design" stable
    APIs; we promote APIs that experiments have converged on.
11. **AI agents are expected contributors.** Repository documentation
    (`AGENTS.md`, this file, ADRs) must be sufficient for an agent with
    limited context to work safely.

## Deliberately undecided

The following are **intentionally not decided** at this time. Proposals
touching them must be justified by experiments and recorded as new ADRs:

- Hot reload / hot swap mechanisms
- Plugin architecture (and isolation: processes, WASI, dynamic libraries)
- Self-modification boundaries (what the system may and may not change about itself)
- Memory architecture
- Evolution algorithms and promotion criteria

## Consequences

- The Phase 0 codebase contains no evolution abstractions by design.
- `unsafe` is workspace-forbidden until profiling evidence says otherwise.
- CI is the enforcement mechanism for formatting, linting, tests, and the
  dependency invariants above.
