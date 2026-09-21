# Experiment 0001 — Observation Replay Boundary

## Experiment ID

0001

## Research Question

Can recorded external observations provide a deterministic replay boundary
for evaluating changed agent behavior under environment drift?

A useful secondary formulation: can baseline and changed behavior be
compared under the same historical external observations without consulting
the current live environment?

This experiment tests the *sufficiency* of one candidate replay boundary in
one controlled scenario. It does not test whether that boundary is
*minimal* — no ablations (removing the request, observation identity, call
arguments, ordering, repetitions, or failures) are performed here.

## Hypothesis

Stated before running:

Recording and replaying behaviorally relevant external observations at an
agent component's external-interaction boundary is sufficient, in this
deterministic scenario, to reproduce baseline behavior under historical
conditions while still allowing changed behavior to execute and produce a
different result.

Expected properties:

1. Live re-execution may differ under environment drift.
2. Final-output playback bypasses the changed behavior (it never executes).
3. Replaying the recorded external observations reproduces baseline
   behavior.
4. Changed behavior remains observable under identical replayed
   observations.
5. An unrecorded external interaction fails explicitly instead of silently
   consulting live state.

This hypothesis is scoped to this one deterministic scenario. It does not
claim sufficiency for all agent architectures.

## Baseline

Normal live re-execution: the exact same component code and logical request
are run again against the current (live) external environment. This is Mode
A. The baseline decision rule is: send only if the `account_status`
observation is `Active`.

## Independent Variable

Replay strategy, one at a time:

- live execution (Mode A)
- final-output playback (Mode B)
- recorded-observation replay (Mode C)

Everything else is held fixed (see Controlled Variables).

## Controlled Variables

- Logical request: `Request { account: "acme" }`, identical in every run.
  (In this fixture the request is accepted by the components but their
  decision rules depend only on the external observation; the experiment
  therefore makes no claim about the request's necessity in replay state.)
- Baseline implementation: send only if the observation is `Active`.
- Candidate implementation: send if the observation is `Active` or `Warm`
  (a deliberately different deterministic rule).
- Recorded E1 observation: `account_status = Warm` (the only external
  interaction during the original E1 execution).
- Toy scenario: in-memory environment, single request, no network, no LLM,
  no randomness, no wall-clock time. Rust standard library only; no
  dependencies added.
- **The concrete account/outreach scenario is an arbitrary, deterministic
  fixture.** It exists only to exercise a generic external-observation
  boundary (request → decision logic → external lookup → environment).
  The business-shaped vocabulary (`account_status`, `account_score`,
  "outreach") is disposable fixture language and carries zero architectural
  meaning; no Account/Customer/Campaign/Outreach/CRM concept is introduced
  by or for this experiment. Future experiments should prefer
  domain-neutral fixtures unless a specific domain is itself the
  independent variable.

## Input / Replay Dataset

One deterministic scenario, fully defined in the example
(`crates/mutagen-runtime/examples/exp_0001_replay_boundary.rs`):

- Two states of the same in-memory environment:
  - `E1`: `account_status = Warm`, `account_score = false` (state at
    recording time)
  - `E2`: `account_status = Active`, `account_score = false` (drifted state)
- Original execution: baseline vs E1 → decision `DoNotSend`; the boundary
  record holds the one recorded external observation
  `{ account_status = Warm }`.
- The replay dataset is exactly those recorded external observations plus
  the logical request; no other data. Nothing is serialized or persisted;
  the record is constructed in memory each run.

Reproducible by running:

```sh
cargo run -p mutagen-runtime --example exp_0001_replay_boundary
```

## Metrics

Behavioral pass/fail observations (no weighting, no scoring):

| Metric | Meaning |
|---|---|
| M1 | Live rerun may differ under environment drift (same request, same code, E1 → E2) |
| M2 | Final-output playback cannot evaluate changed behavior (deterministic, but no component executes under it) |
| M3 | Recorded-observation replay reproduces baseline behavior (replayed baseline == original E1 baseline) |
| M4 | Repeated recorded-observation replay is deterministic (two replays identical) |
| M5 | The changed behavior receives the exact recorded external observation |
| M6 | The changed behavior can still differ under identical replayed observations |
| M7 | An unrecorded external interaction fails explicitly (no live fallback, no fabricated default) |

## Execution Environment

```
rustc 1.98.0 (88d9e12ae 2026-08-18) (Homebrew)
cargo 1.98.0 (797e8a9bc 2026-08-05) (Homebrew)
Darwin zhuchenhaos-Mac-Studio.local 25.6.0 Darwin Kernel Version 25.6.0: Fri Jul 31 19:17:26 PDT 2026; root:xnu-12377.161.14~5/RELEASE_ARM64_T6041 arm64
git: branch exp/0001-replay-boundary; base commit 48e021f9140f338cec1265aa513101e258f30fb1 (main)
```

Hardware class: Apple M-series Mac (arm64). Dependencies: workspace
dependencies only, unchanged (`mutagen-core` path dependency; the example
uses only `std`). Cargo.toml and Cargo.lock are unmodified.

## Results

Raw results of a run:

```
Experiment 0001 — Observation Replay Boundary
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
  Same request, same code, different external state, different result:
  live re-execution is contaminated by environment drift.
- M2 PASS — output playback returned the recorded `DoNotSend` identically on
  every call and the execution flag shows no component executed under it;
  baseline and candidate are indistinguishable under playback, even though
  the candidate genuinely differs from the baseline under
  recorded-observation replay (`Send`).
- M3 PASS — baseline against the replay lookup (serving `Warm`) reproduced
  `DoNotSend`, matching the original E1 result.
- M4 PASS — a second replay of the baseline produced the identical result.
- M5 PASS — the candidate's actual lookups (captured at the replay boundary)
  were exactly the recorded `{ account_status = Warm }`.
- M6 PASS — under that identical observation the candidate produced `Send`
  while the baseline produced `DoNotSend`.
- M7 PASS — a candidate variant that additionally looks up `account_score`
  failed with an explicit missing-recorded-observation error naming
  `account_score`; it did not consult the live environment (which had the
  value) and did not fabricate a default.

## Three-Run Reproducibility

The example was executed three separate times via
`cargo run -p mutagen-runtime --example exp_0001_replay_boundary`.
All three runs produced identical output:

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

**supported** — scoped strictly to this experiment's own scope: *in this
deterministic scenario*, recording and replaying the external observations
at the component's external-interaction boundary is sufficient to reproduce
baseline behavior while keeping changed behavior observable.

### Demonstrated

In this deterministic toy scenario:

- live re-execution can be contaminated by environment drift (M1);
- final-output playback cannot evaluate changed behavior because it bypasses
  execution entirely (M2);
- replaying the recorded historical external observations reproduces
  baseline behavior (M3) deterministically (M4);
- changed behavior remains observable under the same historical
  observations (M5, M6);
- an unrecorded external interaction fails explicitly instead of silently
  mixing historical and live state (M7).

### Not demonstrated

This experiment does **not** establish:

- the minimal complete replay state — no ablations were run, so no part of
  the replay record (request, observation identity, observation value, call
  arguments, ordering, repetition, errors) has been shown to be *necessary*;
- logical-request necessity — the fixture's decision rules do not actually
  consume the request field;
- any observation *representation* — the implementation is a throwaway
  local lookup over one recorded observation; it does not choose between
  set, map, sequence, ordered event stream, multimap, call-indexed trace,
  or argument-sensitive interaction record;
- call ordering, repeated same-key lookups, or argument matching;
- side-effect replay or error-replay semantics;
- stochastic model replay or LLM replay (the scenario is fully
  deterministic);
- cross-domain generalization or general-agent sufficiency;
- any production replay API, serialization, persistence, component or
  revision identity, lineage, storage, tool schema, plugin architecture,
  promotion, or rollback;
- any production architecture selection.

Two explicit non-claims:

> This experiment does not establish the minimal complete replay state.

> This experiment does not establish generality across heterogeneous agent
> domains.

### Generalization limitation

This experiment uses one deterministic fixture and establishes a local
mechanism result only. It does not demonstrate that the same replay
boundary is sufficient across heterogeneous agent environments such as
coding, research, multi-tool workflows, stateful interactive environments,
or stochastic model execution. A result from one toy scenario is local
evidence, not proof of general-agent applicability; establishing that a
replay boundary is a general agent invariant requires evidence across
heterogeneous agent task types, which is explicitly out of scope here.

## Architectural Evidence

### Supported by this experiment

Local evidence only, from this one deterministic scenario:

- An external-observation seam is a viable replay boundary in this
  deterministic scenario.
- Replaying historical observations can isolate changed behavior from live
  environment drift in this scenario, while still exposing the behavioral
  difference between baseline and changed behavior.
- Direct final-output playback is unsuitable when the purpose is to
  evaluate changed behavior.

Nothing here is a selected architecture.

### Not established by this experiment

Nothing in this experiment establishes or selects any of the following:

- the minimal complete replay state (no necessity ablations were run);
- logical-request necessity in replay state;
- any observation representation (set, map, sequence, ordered stream,
  multimap, call-indexed trace, argument-sensitive record);
- call ordering or repeated-lookup semantics;
- argument matching between a candidate's interactions and the record;
- side-effect replay or error-replay semantics;
- production replay API (the example's closures are private and throwaway);
- serialization format (nothing is serialized; all state is in-memory);
- persistence format (no storage of any kind);
- component identity (no `Component`/ID type exists);
- revision identity (no revision semantics anywhere);
- candidate identity (the words *baseline* and *candidate* are local
  function names, not identities);
- lineage (no parent/child or generation concepts);
- storage (no database, file, or any persistence layer);
- LLM replay (no model calls of any kind);
- stochastic model handling (the scenario is fully deterministic);
- tool schema (the "lookup" boundary is a one-call toy, not a tool schema);
- plugin architecture (no loading, registration, or extension mechanism);
- promotion (no decision about what becomes production);
- rollback (no deployment or versioned-artifact machinery);
- cross-domain generalization or any general-agent invariant.

## Follow-up Question

Primary (necessity, not representation design):

Which pieces of historical execution state are actually necessary for
faithful candidate evaluation — input, observation identity, observation
value, call arguments, ordering, repetition, errors, and other external
effects — and which can be removed without changing the evaluation result?

Second-order (generality, recorded only):

Which replay requirements remain invariant across heterogeneous agent task
types (coding, research, multi-tool workflows, stateful interactive
environments, stochastic model execution), and which are local to one
task type?

Neither question is answered or implemented here; this experiment is
deliberately a local mechanism proof.
