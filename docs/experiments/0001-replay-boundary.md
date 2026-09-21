# Experiment 0001 — Minimal Replay Boundary

## Experiment ID

0001

## Research Question

What is the minimum replay boundary required to evaluate a changed agent
component against past execution experience?

Specifically: when an agent execution depends on external observations, how
can we replay the historical scenario while still allowing a changed
component to execute and produce different behavior?

## Hypothesis

Stated before running:

Recording only the original user input and final output is insufficient for
evaluating a changed component. A useful replay mechanism must freeze
behaviorally relevant external observations while re-executing the component
under evaluation.

Expected properties:

1. Live re-execution can change because the external environment changed.
2. Final-output playback is deterministic but bypasses the candidate.
3. Replaying recorded external observations allows both baseline and
   candidate to execute against the same effective environment.
4. Missing historical observations must fail explicitly instead of silently
   falling back to live state.

## Baseline

Normal live re-execution: the exact same component code and logical request
are run again against the current (live) external environment. This is Mode
A. The baseline decision rule is: send the outreach message only if the
account status is `Active`.

## Independent Variable

Replay strategy, one at a time:

- live execution (Mode A)
- final-output playback (Mode B)
- recorded-observation replay (Mode C)

Everything else is held fixed (see Controlled Variables).

## Controlled Variables

- Logical request: `Request { account: "acme" }` ("should we send an outreach
  message to this account?"), identical in every run.
- Baseline implementation: send only if `account_status == Active`.
- Candidate implementation: send if `account_status` is `Active` or `Warm`
  (deliberately different deterministic rule).
- Recorded E1 observation: `account_status = Warm` (the only observation made
  during the original E1 execution).
- Toy scenario: in-memory `Environment { account_status, account_score }`,
  single account, no network, no LLM, no randomness, no wall-clock time.
  Rust standard library only; no dependencies added.

## Input / Replay Dataset

One deterministic scenario, fully defined in the example
(`crates/mutagen-runtime/examples/exp_0001_replay_boundary.rs`):

- Two states of the same in-memory environment:
  - `E1`: `account_status = Warm`, `account_score = false` (state at
    recording time)
  - `E2`: `account_status = Active`, `account_score = false` (drifted state)
- Original execution: baseline vs E1 → decision `DoNotSend`; boundary record
  = `{ account_status = Warm }`.
- The replay dataset is exactly that one boundary record. There is no other
  data; no external files, no serialization, no persistence.

Reproducible by running:

```sh
cargo run -p mutagen-runtime --example exp_0001_replay_boundary
```

## Metrics

Behavioral pass/fail observations (no weighting, no scoring):

| Metric | Meaning |
|---|---|
| M1 | Live rerun changes under environment drift (same request, same code, E1 → E2) |
| M2 | Final-output playback cannot evaluate candidate behavior (deterministic, but candidate unobservable) |
| M3 | Boundary replay reproduces baseline behavior (replayed baseline == original E1 baseline) |
| M4 | Repeated boundary replay is deterministic (two replays identical) |
| M5 | Candidate receives the exact recorded external observation |
| M6 | Candidate behavior can still differ under identical replayed observations |
| M7 | Missing recorded observation fails explicitly (no live fallback, no fabricated default) |

## Execution Environment

```
rustc 1.98.0 (88d9e12ae 2026-08-18) (Homebrew)
cargo 1.98.0 (797e8a9bc 2026-08-05) (Homebrew)
Darwin zhuchenhaos-Mac-Studio.local 25.6.0 Darwin Kernel Version 25.6.0: Fri Jul 31 19:17:26 PDT 2026; root:xnu-12377.161.14~5/RELEASE_ARM64_T6041 arm64
git rev-parse HEAD: 48e021f9140f338cec1265aa513101e258f30fb1 (main, base of the branch)
```

Hardware class: Apple M-series Mac (arm64). Dependencies: workspace
dependencies only, unchanged (`mutagen-core` path dependency; the example
uses only `std`). Cargo.toml and Cargo.lock are unmodified.

## Results

Raw results of a single run:

```
Experiment 0001 — Minimal Replay Boundary
request: send outreach to account 'acme'
recorded observation: account_status = AccountStatus(Warm)
M1 PASS
M2 PASS
M3 PASS
M4 PASS
M5 PASS
M6 PASS
M7 PASS
Conclusion: supported
```

- M1 PASS — baseline: E1 (`Warm`) → `DoNotSend`; E2 (`Active`) → `Send`.
  Same request, same code, different external state, different result.
- M2 PASS — output playback returned the recorded `DoNotSend` identically on
  every call, and the "candidate" under playback is indistinguishable from
  the baseline, even though the candidate genuinely differs from the
  baseline under recorded-observation replay (`Send`).
- M3 PASS — baseline against the replay lookup (serving `Warm`) reproduced
  `DoNotSend`, matching the original E1 result.
- M4 PASS — a second replay of the baseline produced the identical result.
- M5 PASS — the candidate's actual lookups (captured at the replay boundary)
  were exactly `{ account_status = Warm }`, i.e. the original record.
- M6 PASS — under that identical observation the candidate produced `Send`
  while the baseline produced `DoNotSend`.
- M7 PASS — a candidate variant that additionally looks up `account_score`
  failed with an explicit missing-recorded-observation error naming
  `account_score`; it did not consult the live environment (which had the
  value) and did not fabricate a default.

## Three-Run Reproducibility

The example was executed three separate times via
`cargo run -p mutagen-runtime --example exp_0001_replay_boundary`.
All three runs produced byte-identical output:

| Run | Output |
|---|---|
| Run 1 | `M1 PASS`, `M2 PASS`, `M3 PASS`, `M4 PASS`, `M5 PASS`, `M6 PASS`, `M7 PASS`, `Conclusion: supported` |
| Run 2 | `M1 PASS`, `M2 PASS`, `M3 PASS`, `M4 PASS`, `M5 PASS`, `M6 PASS`, `M7 PASS`, `Conclusion: supported` |
| Run 3 | `M1 PASS`, `M2 PASS`, `M3 PASS`, `M4 PASS`, `M5 PASS`, `M6 PASS`, `M7 PASS`, `Conclusion: supported` |

No timestamps or random identifiers appear in any output.

## Artifacts

- Example (the entire experiment):
  `crates/mutagen-runtime/examples/exp_0001_replay_boundary.rs`
- This record: `docs/experiments/0001-replay-boundary.md`

No traces, episodes, or datasets are persisted anywhere else; the replay
dataset is constructed in memory each run.

## Conclusion

**supported**

All four expected properties of the hypothesis held, and the three replay
strategies behaved exactly as predicted:

1. Live re-execution is not a valid evaluation of a changed component: the
   same code and request produced a different result once the external
   environment drifted (M1).
2. Final-output playback is deterministic but evaluates nothing about the
   component — the candidate never executes, so its behavior is
   unobservable (M2).
3. Freezing the external observations at the lookup boundary lets both
   baseline and candidate re-execute against the same effective historical
   environment: the baseline is exactly reproduced (M3, M4), the candidate
   demonstrably receives the identical observations (M5), and its different
   behavior remains measurable (M6).
4. When the candidate probes a boundary that the original execution never
   exercised, replay fails explicitly rather than blending in live state or
   invented values (M7).

The minimum replay boundary demonstrated here is therefore the *set of
external observations a component actually makes at its lookup boundary*
together with the logical request — not the final output, and not the raw
environment. (This is a claim about this toy lookup boundary, see below.)

## Architectural Evidence

### Supported by this experiment

- Recording external observations at a component's lookup boundary, and
  re-serving them during replay, is sufficient in this scenario to make
  both baseline and candidate execution comparable under one frozen
  effective environment.
- The final output alone is not a sufficient replay unit for evaluating a
  changed component (it makes the candidate unobservable).
- Replay can preserve baseline identity while still surfacing a candidate's
  behavioral difference.
- Explicit failure on unrecorded boundary probes is a viable and
  distinguishable behavior (distinct from live lookup and from defaults).
- The distinction between "component behavior" and "external observation"
  is a meaningful and useful seam for a replay design.

### Not established by this experiment

Nothing in this experiment establishes or selects any of the following:

- production replay API (the example's closures are private and throwaway)
- serialization format (nothing is serialized; all state is in-memory)
- persistence format (no storage of any kind)
- component identity (no `Component`/ID type exists)
- revision identity (no revision semantics anywhere)
- candidate identity (the words *baseline* and *candidate* are local
  function names, not identities)
- lineage (no parent/child or generation concepts)
- storage (no database, file, or any persistence layer)
- LLM replay (no model calls of any kind)
- stochastic model handling (the scenario is fully deterministic)
- tool schema (the "lookup" boundary is a one-call toy, not a tool schema)
- plugin architecture (no loading, registration, or extension mechanism)
- promotion (no decision about what becomes production)
- rollback (no deployment or versioned-artifact machinery)

## Follow-up Question

When a changed component makes *more* or *different* external lookups than
the original execution (as M7's `account_score` probe shows), what is the
minimum *matching* policy between the candidate's lookup sequence and the
recorded observation set — which lookups must match by key, and how should
the boundary record represent the *sequence* of observations (order,
repeats, conditional branches) rather than just the set of distinct keys?

(This question is recorded only; it is not implemented here.)
