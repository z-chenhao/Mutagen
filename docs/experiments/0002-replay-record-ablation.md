# Experiment 0002 — Replay Record Ablation

## Experiment ID

0002

## Research Question

Which information inside a recorded external interaction becomes
behaviorally necessary when replay must handle argument-sensitive calls,
repeated calls, and error outcomes?

This experiment asks about **information loss**. It does not ask for the
final replay data structure, and it does not ask for the final matching
algorithm.

## Prior Evidence

Experiment 0001
([`0001-replay-boundary.md`](0001-replay-boundary.md))
established **only** this local result: in one deterministic scenario,
replaying recorded external observations at an external-interaction
boundary was sufficient to reproduce baseline behavior while keeping
changed behavior observable under historical conditions.

Experiment 0001 did **not** establish: the minimal replay state; the
observation representation; argument matching; repeated-interaction
semantics; ordering; error replay; general-agent invariants; or a
production API. No ablations of any kind were run in 0001, so no part of
the replay record had been shown to be necessary.

## Hypothesis

Stated before running:

A replay record that preserves only an external observation value or a
coarse interaction name is insufficient for faithful candidate evaluation
once execution contains argument-sensitive calls, repeated identical
calls, or error-dependent control flow.

More specifically, we expected to demonstrate:

1. interaction **arguments** can be necessary to bind a historical
   outcome to the correct call;
2. **repeated identical** calls can require occurrence-sensitive
   information because one (operation, argument) pair may have multiple
   historical outcomes;
3. external **errors** are behavioral outcomes and cannot always be
   discarded while preserving execution behavior.

Important scope of the hypothesis: it may show that a piece of
information is necessary *for at least one tested interaction pattern*.
It must not be read as claiming that every agent or every interaction
universally requires all fields.

## Fixtures

All fixtures are deterministic, in-memory, standard-library-only, and use
only domain-neutral interaction vocabulary (operation, argument, value,
outcome, error, call, interaction, record, replay). No business-domain
concepts appear. The vocabulary is generic, but these remain synthetic
deterministic fixtures; domain-neutral naming itself does not prove
generalization (see Generalization Limitation).

The full experimental record for all fixtures is a local ordered trace
of `{call {operation, argument}, outcome}`, which preserves argument
identity, the occurrence position of each interaction, and error
outcomes. It is throwaway scaffolding, not a production schema. The
full-replay lookup matches by (operation, argument) and, among matching
records, selects by occurrence: the n-th call of a given
(operation, argument) receives the n-th recorded outcome for that call.

### Fixture A — Argument-sensitive interaction

- History: `read("alpha") -> Value("A")`, `read("beta") -> Value("B")`.
- Changed behavior: the same two reads, in the opposite order
  (`read("beta")`, then `read("alpha")`).
- Full condition: replay with (operation, argument) matching; the
  correct historical outcome must be selected for **both** calls.
- Ablated condition: argument identity removed; the record holds only
  (operation, outcome) pairs, and replay matches on operation only.
  Two policies are exercised: a strict policy that fails when a call
  matches more than one record, and a deterministic first-match policy
  that consumes the earliest not-yet-consumed matching record.
- The full record is an ordered trace; a fresh replay instance is used
  per replay attempt.

### Fixture B — Repeated identical interaction

- History: `poll("task") -> Value("pending")`, then
  `poll("task") -> Value("ready")`. Same operation, same argument, two
  different historical outcomes.
- Full condition: the ordered trace replays the first occurrence with
  `pending` and the second with `ready` (occurrence-sensitive matching).
- Ablated condition: the observations collapse into a single
  (operation, argument) -> outcome mapping in which repeated
  occurrences cannot be distinguished. The collapse is necessarily
  lossy — the second occurrence overwrites the first — and replay then
  must receive one outcome for both calls.

### Fixture C — Error-dependent interaction

- History: `fetch("missing") -> Error("NotFound")`.
- Baseline behavior branches on the outcome: error -> fallback path,
  value -> normal path. The historical branch taken is the fallback
  path.
- Full condition: the error is recorded and replayed **as** an external
  outcome; the historical branch must be reproduced.
- Ablated condition: the record preserves only successful values and
  drops external errors. Since the only historical outcome was an error,
  the ablated record is empty.

## Independent Variable

For each fixture, exactly one:

- Fixture A: full record vs. record **without argument identity**
  (operation-only matching).
- Fixture B: full ordered trace vs. **collapsed
  (operation, argument) -> outcome mapping** (occurrences indistinguishable).
- Fixture C: full record (errors preserved) vs. record **without error
  outcomes** (values only).

## Controlled Variables

For every full/ablated pair, held fixed:

- Fixture logic (the behavior code is identical between the history run
  and the replay; the only change in Fixture A is call order, which is
  the fixture's changed behavior, not part of the ablation).
- Historical external outcomes (the history is constructed the same way
  in every run).
- The execution code (same functions, same lookup boundary type).
- The environment (a fully scripted, in-memory history; no live state,
  no network, no LLM, no randomness, no wall clock, no filesystem).
- All non-ablated interaction information (in A, occurrence order is
  preserved; in B, argument and value are preserved; in C, the call
  identity is preserved).

## Input / Replay Dataset

Three fully scripted in-memory histories, defined in the example;
nothing is serialized or persisted — the records are constructed in
memory each run.

Reproducible by running:

```sh
cargo run -p mutagen-runtime --example exp_0002_replay_record_ablation
```

## Metrics

Behavioral pass/fail observations (no weighting, no scoring, no
aggregation):

| Metric | Meaning |
|---|---|
| F1 | Full experimental record faithfully replays the argument-sensitive fixture (baseline and changed behavior, correct outcome bound to each call) |
| A1 | Removing argument identity loses faithful binding in Fixture A |
| F2 | Full experimental record faithfully replays the repeated-call fixture |
| B1 | Removing occurrence distinction loses faithful replay in Fixture B |
| F3 | Full experimental record faithfully replays the error-dependent fixture |
| C1 | Removing error outcomes loses faithful replay in Fixture C |

F1–F3 are control checks: an ablation result is invalid if its
corresponding full-record control does not work.

## Execution Environment

```
rustc 1.98.0 (88d9e12ae 2026-08-18) (Homebrew)
cargo 1.98.0 (797e8a9bc 2026-08-05) (Homebrew)
Darwin zhuchenhaos-Mac-Studio.local 25.6.0 Darwin Kernel Version 25.6.0: Fri Jul 31 19:17:26 PDT 2026; root:xnu-12377.161.14~5/RELEASE_ARM64_T6041 arm64
code-under-test commit: __EXPERIMENT_COMMIT_SHA__ (branch exp/0002-replay-record-ablation; base main ae3d4b9)
```

Hardware class: Apple M-series Mac (arm64). Dependencies: none added;
the example uses only the Rust standard library. Cargo.toml and
Cargo.lock are unmodified. The recorded commit is the experiment
commit whose code the runs executed; it is recorded as the
**code-under-test commit**. (If this document is later amended, the SHA
refers to the experiment code commit, not to the amending document
commit — the SHA is not self-referential and is not made to be.)

## Results

Raw output of a run (identical across three runs):

```
Experiment 0002 — Replay Record Ablation
Fixture A (argument-sensitive): history read("alpha") -> Value("A"), read("beta") -> Value("B"); changed behavior reorders the two reads
  ablated replay of changed behavior: strict = Err(AmbiguousMatches { call: "read(\"beta\")", matches: 2 }), first-match = Ok(["A", "B"]) (correct: ["B", "A"])
F1 PASS
A1 PASS
Fixture B (repeated identical): history poll("task") -> Value("pending"), poll("task") -> Value("ready")
  collapsed mapping replays: Ok(["ready", "ready"]) (history: ["pending", "ready"])
F2 PASS
B1 PASS
Fixture C (error-dependent): history fetch("missing") -> Error("NotFound"); baseline branches on the error into a fallback path
  error-dropping replay: Err(MissingRecordedInteraction("fetch(\"missing\")"))
F3 PASS
C1 PASS
Conclusion: supported
```

- F1 PASS — baseline replayed `["A", "B"]` (matching the history) and
  the changed behavior replayed `["B", "A"]`: each call received the
  historical outcome of **that specific (operation, argument)**.
- A1 PASS — with argument identity removed, both historical records are
  `("read", outcome)`. Under the strict policy the changed behavior's
  first call `read("beta")` is explicitly ambiguous (2 matching
  records); under the deterministic first-match policy it receives
  `Value("A")` — the historical outcome of `read("alpha")` — and the
  run yields `["A", "B"]` instead of the faithful `["B", "A"]`.
  (The baseline happens to replay correctly under operation-only
  matching only because its call order coincidentally matches the
  history; the reordered calls expose the loss.)
- F2 PASS — the ordered trace replayed `pending` then `ready`,
  reproducing the history.
- B1 PASS — the collapsed mapping holds a single entry
  `poll("task") -> Value("ready")`; the first outcome `pending` is not
  present anywhere in the mapping, and replay deterministically yields
  `["ready", "ready"]` ≠ the historical `["pending", "ready"]`.
- F3 PASS — the error replayed as an outcome took the fallback branch,
  matching the history.
- C1 PASS — with errors dropped, the ablated record is empty; replay
  failed explicitly with `MissingRecordedInteraction(fetch("missing"))`
  instead of fabricating a value. The historical branch cannot be
  reproduced.

## Three-Run Reproducibility

The example was executed three separate times via
`cargo run -p mutagen-runtime --example exp_0002_replay_record_ablation`.
All three runs produced byte-identical output:

| Run | Output |
|---|---|
| Run 1 | `F1 PASS`, `A1 PASS`, `F2 PASS`, `B1 PASS`, `F3 PASS`, `C1 PASS`, `Conclusion: supported` |
| Run 2 | `F1 PASS`, `A1 PASS`, `F2 PASS`, `B1 PASS`, `F3 PASS`, `C1 PASS`, `Conclusion: supported` |
| Run 3 | `F1 PASS`, `A1 PASS`, `F2 PASS`, `B1 PASS`, `F3 PASS`, `C1 PASS`, `Conclusion: supported` |

No timestamps, random identifiers, or environment-dependent values
appear in any output.

## Artifacts

- Example (the entire experiment):
  `crates/mutagen-runtime/examples/exp_0002_replay_record_ablation.rs`
- This record: `docs/experiments/0002-replay-record-ablation.md`

No traces, episodes, or datasets are persisted anywhere else; all
records are constructed in memory each run.

## Conclusion

**supported** — scoped strictly to the tested interaction patterns.

> Across the tested deterministic interaction patterns, dropping
> argument identity, occurrence distinction, or error outcomes can
> destroy faithful replay.

This is the strongest allowable form. The experiment was deliberately
constructed to expose information-loss cases; it establishes
**counterexamples to overly lossy recording**, not a universal final
schema. The valid conclusion is *not* "every replay system must always
store operation, argument, occurrence, and error in exactly this
form."

### Demonstrated (in the tested fixtures only)

- Argument identity was behaviorally necessary in Fixture A: after call
  reordering, operation-only matching cannot bind the correct
  historical outcome to each call.
- Occurrence distinction was behaviorally necessary in Fixture B: one
  (operation, argument) pair with multiple historical outcomes cannot
  be faithfully replayed by a single mapping.
- Error outcomes were behaviorally necessary in Fixture C: dropping
  errors makes the historical branch unreplayable without fabrication.
- The full experimental record (ordered trace preserving argument,
  occurrence position, and errors) faithfully replayed all three
  fixtures (F1, F2, F3).

### Not established

This experiment does **not** establish or select:

- a production replay event schema — the local types are private,
  throwaway scaffolding in the example;
- universal field requirements for any general agent or any
  interaction;
- a **global ordering requirement** — Fixture B necessarily contains
  historical order because its calls repeat, and the ordered trace
  uses it. The evidence supports only: *repeated indistinguishable
  calls require enough information to distinguish their historical
  occurrences.* Whether that requires a global sequence, a per-call
  occurrence counter, a causal order, or a partial order remains
  unresolved and is deliberately not concluded here;
- request/input necessity — left unresolved, as in Experiment 0001
  (no claim about persisting or reconstructing inputs, requests,
  prompts, context, or memory state);
- side-effect semantics;
- asynchronous interactions;
- concurrency;
- stochastic models;
- LLM replay;
- cross-domain generalization;
- persistence or serialization;
- component or revision identity;
- promotion or rollback.

### Generalization limitation

These are **three synthetic deterministic interaction patterns**, not
evidence of cross-domain generalization. The fixtures were selected
because argument-sensitive interaction, repeated identical interaction,
and error-dependent interaction are **structural** interaction patterns
rather than business-domain concepts: they arise from the shape of any
external interface (differing arguments, repeated calls, failing
calls), independent of the domain being served. This gives broader
mechanism coverage than Experiment 0001 (which used one simple
deterministic scenario with a single call and one value) without
proving a general-agent invariant. Nothing here claims more.

## Follow-up Questions

Recorded, not implemented:

1. Which replay-state requirements survive across heterogeneous agent
   task types?
2. How should replay handle changed trajectories that introduce,
   remove, or reorder external interactions?
3. Is logical input/context itself part of the replay state, and under
   what conditions?
