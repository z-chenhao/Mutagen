# Contributing

Mutagen is an early-stage, experimental research project. Thank you for
helping shape it.

## Ground rules

- **Read [`AGENTS.md`](AGENTS.md) first.** It is the operating manual for
  everyone — humans and AI coding agents — and encodes the project's change
  discipline: evidence before abstraction, no speculative dependencies,
  no speculative evolution traits.
- This is a research project. Small, well-scoped contributions and
  experiment records are more valuable than large architectural guesses.
  The architecture is *deliberately undecided*; proposals should cite the
  experiment or requirement that motivates them.
- Architectural changes require an ADR in [`docs/decisions/`](docs/decisions/).

## Development setup

Stable Rust only (see `rust-toolchain.toml`; the workspace requires
rustc ≥ 1.85 for edition 2024). No nightly features, no external services.

```sh
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo run -p mutagen-cli -- doctor
```

All five must pass before opening a pull request.

## Pull requests

- One concern per PR; keep diffs small and reviewable.
- Add tests for behavioral changes.
- Do not touch unrelated files.
- For anything evolution-related, reference the experiment record in
  [`docs/experiments/`](docs/experiments/).

## Branching and merging

The full policy lives in [`AGENTS.md`](AGENTS.md) (it binds humans and AI
agents alike); the short version:

- `main` is the stable integration branch: no direct development, no
  force-push, no unvalidated merges.
- One task = one branch from the latest `main`, named
  `<type>/<kebab-case-name>` (`feat/`, `fix/`, `refactor/`, `exp/`,
  `docs/`, `test/`, `perf/`, `ci/`, `chore/`); experiments use
  `exp/<id>-<short-name>`.
- Small commits with `<type>: <imperative summary>` messages.
- The full validation suite must pass and your own diff inspected before
  pushing. Push your branch only — never `main`.
- Everything lands via PR, **squash-merged** after review; merged branches
  are deleted, never reused.

## License

By contributing, you agree that your contributions are licensed under the
[Apache License 2.0](LICENSE).
