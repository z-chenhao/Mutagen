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

## Git branch and merge workflow

This workflow binds all contributors, human and AI agent alike. Do not
bypass it unless the repository owner explicitly instructs otherwise.

### main is protected

`main` is the stable integration branch. Never develop on it, never create
implementation commits on it, never force-push or rewrite its history, and
never merge unvalidated work into it. Every implementation task happens on
a dedicated branch and lands through a pull request.

### Before any task

Inspect state first:

```sh
git status --short
git branch --show-current
git log -5 --oneline --decorate
```

The working tree should be clean. Then synchronize:

```sh
git switch main
git pull --ff-only
```

If `--ff-only` fails, stop and inspect the divergence; never resolve
unexpected divergence automatically.

### One task = one branch

Create a dedicated branch from the latest `main` (`git switch -c
<name>`). Each branch represents exactly one coherent concern. Names use
lowercase kebab-case with a prefix:

- `feat/` new user-visible or architectural capability
- `fix/` bug or correctness fix
- `refactor/` behavior-preserving structural change
- `exp/` research experiment — prefer `exp/<experiment-id>-<short-name>`
  (e.g. `exp/0001-replay-boundary`)
- `docs/` documentation-only change
- `test/` test-only work
- `perf/` evidence-backed performance work
- `ci/` CI or repository automation
- `chore/` narrowly scoped maintenance

Never use vague names such as `update`, `changes`, `new`, `test`, `phase`,
`work`, or `tmp`. Never branch from an old feature branch unless the task
explicitly depends on unmerged work — and then report that dependency.

### Commits

Small, coherent, reviewable; one logical change each. Use
`<type>: <concise imperative description>` (e.g. `fix: remove linear
component version semantics`). Never commit `target/`, secrets, local
environment files, editor state, or machine-specific paths. Never use
meaningless messages (`update`, `wip`, `final2`, …).

### Validation before push

Run the full suite (see "Development commands" above) — plus
`mutagen --version` if CLI version behavior changed. All must pass. Do not
suppress warnings to obtain a green build.

### Inspect your own diff

```sh
git status --short
git diff --check
git diff main...HEAD
git diff --stat main...HEAD
```

Only task-relevant files, no accidental formatting, no secrets or debug
output, no unrelated refactors, no speculative abstractions, no unexpected
dependency changes. If the task is narrow, the diff must be narrow.

### Push branches, never main

`git push -u origin <branch-name>`. Never push directly to `origin/main`.
Never use plain `--force`; for rewriting your own unpublished branch,
`--force-with-lease` at most.

### Pull requests

Every branch merges through a PR targeting `main`. The description contains:
Summary, Why, What changed, What deliberately did not change, Validation,
Risks / open questions. Experiments additionally record: Hypothesis,
Baseline, Independent variable, Metrics, Result, Conclusion. One primary
purpose per PR. If a task reveals a second independent problem: record it,
finish the current task, and create a separate branch — never silently
expand scope.

### Syncing before merge

`git fetch origin && git rebase origin/main`, then rerun validation.
(If the branch is shared, a merge of `origin/main` is acceptable; avoid
unnecessary merge commits.) Never resolve a conflict by blindly accepting
one side: inspect both versions, understand the intent, preserve the
task's invariants, rerun validation.

### Merge requirements and strategy

Merge only when: scope complete, diff reviewed, tests pass, CI green, no
unresolved review comments, docs match behavior, no unrelated changes.
Prefer **squash merge** so `main` carries one coherent commit per task.
After merge: switch to `main`, `git pull --ff-only`, delete the branch
locally and remotely. Never reuse a merged branch for new work.

### Experiment branches

Experiment code may be intentionally temporary, but must be clearly
identifiable; experiment-specific assumptions must not silently become core
abstractions. A successful result does not automatically justify
production architecture — promoting an experimental abstraction into
`mutagen-core` or `mutagen-runtime` is a separate architectural decision:

```
hypothesis → exp/NNNN branch → implementation → measurement
→ experiment record → PR review → result accepted
→ separate architecture decision (if warranted)
```

### AI agent permissions

An agent must never autonomously: push directly to `main`; merge a PR;
force-push shared history; delete another contributor's branch; combine
unrelated tasks; reinterpret a failed CI check as acceptable; resolve
ambiguous conflicts without understanding them; expand scope because an
adjacent improvement "looks useful". An agent may: create its task branch,
commit task-scoped work, run validation, push its own branch, and prepare
the PR. Merging into `main` remains a distinct human integration step.
