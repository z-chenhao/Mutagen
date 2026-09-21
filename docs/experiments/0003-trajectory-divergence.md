# Experiment 0003 — Replay Under Trajectory Divergence

## Experiment ID

0003

## Research Question

When a candidate's external-interaction trajectory differs from the
historical trajectory, which forms of divergence can still be replayed
using only recorded historical evidence, and which expose an explicit
evidence gap?

This experiment studies **trajectory divergence**. It does not judge
candidate quality, and it does not design a production replay policy, an
evaluator framework, or any production replay abstraction. It only
investigates whether the historical evidence is sufficient to execute a
controlled replay.

## Prior Evidence

Experiment 0001
([`0001-replay-boundary.md`](0001-replay-boundary.md))
established **only** this local result: in one deterministic scenario,
recorded external observations formed a viable replay seam. It did not
establish a universal replay architecture, a replay data structure, or a
matching policy, and it never considered a candidate whose trajectory
differs from the history's.

Experiment 0002
([`0002-replay-record-ablation.md`](0002-replay-record-ablation.md))
demonstrated **counterexamples to overly lossy records**: argument
identity can matter, repeated interactions can require occurrence
distinction, and error outcomes can matter. It did not establish a final
event schema or matching policy, and it considered only trajectories that
differ in the *information* of individual calls — never a trajectory that
omits, reorders, or adds interactions.

A replay mechanism that requires *candidate trajectory == historical
trajectory* may be too strict, because behavioral change is exactly what
an evolving candidate is supposed to produce; but a mechanism that accepts
any divergence may silently evaluate a candidate using evidence that does
not exist. This experiment probes that boundary.

## Hypothesis

Stated before running:

Exact trajectory equality is not necessary for replay in all
deterministic read-only interaction scenarios. A candidate can sometimes
remain replayable when it consumes only historically recorded
interactions, even if it omits or reorders them. In contrast, a candidate
interaction for which no historical outcome exists creates an evidence
gap under a replay regime restricted to historical evidence.

Expected observations:

1. a candidate that uses only a subset of historical read-only
   interactions can still execute from recorded evidence;
2. unused historical observations do not necessarily need to be
   force-consumed in this fixture;
3. a candidate that reorders distinct, independently recorded read-only
   interactions can still execute under identity-aware replay in this
   fixture;
4. strict positional trajectory matching may reject that reorder despite
   historical evidence existing for every requested interaction;
5. a candidate that introduces a previously unseen interaction cannot be
   replayed entirely from the historical record;
6. replay must expose that missing evidence explicitly rather than using
   live state or fabricating an outcome.

Scope of the hypothesis: these claims are scoped to **deterministic,
read-only synthetic fixtures**. They do not claim that omission or
reordering is generally safe for side-effecting tools, stateful
environments, concurrent or asynchronous execution, LLM calls, or
stochastic systems.

## Historical Fixture

One small, fully scripted, in-memory, read-only historical execution
(deterministic; standard library only; no network, filesystem, LLM,
randomness, wall clock, async, or external process):

```
read("left")  -> Value("L")
read("right") -> Value("R")
```

These interactions are defined as **independent, read-only lookups** in
this fixture: neither mutates anything, and each argument has a fixed
deterministic outcome. The baseline executes both, in that order, and
its output depends on both observations: `"L|R"`.

The local experimental representation (private to the example file, not
a production type) is an ordered trace of
`{call {operation, argument}, outcome}`, recording each interaction's
identity, its outcome, and its trajectory position.

## Instrumentation: the live fallback boundary

Every replay condition receives an explicit experiment-local live
fallback callback (a small local closure — no trait, no reusable
framework). The callback owns the live-call log and returns a
distinguishable live value (`Value("LIVE")`) if invoked, so it is a
*real* alternative path that could let a candidate continue past an
evidence gap. The replay mechanism receives only the callback — never the
log directly — and the live callback is its only path to live state; the
replay policy under test deliberately refuses to use it. After each
replay, the driver asserts that the callback invocation log is empty,
demonstrating that replay did not fall back to live state — externally
observable rather than self-reported by the replay implementation. A
driver-side probe of the callback (outside any replay) confirms it is a
working path: the probe invocation is logged and returns
`Value("LIVE")`. This is experiment scaffolding, not a production
architecture.

The consumption log (which historical records a replay consumed) is
likewise owned by the driver, outside the replay action; it is the
evidence for "record i was not consumed" claims.

## Candidate A — Subset

Trajectory: `read("left")` only; output `"L"`. It **omits** the
historical `read("right")`, so the candidate trajectory differs from the
history while every requested interaction has recorded evidence.

- S1 — the candidate executes entirely from recorded evidence: identity-
  aware replay resolves `read("left")` by exact interaction identity,
  yields `"L"`, and the driver-owned live-fallback invocation log stays
  empty.
- S2 — the unused historical interaction does not invalidate the replay:
  the externally owned consumption log shows record 0 (`read("left")`)
  consumed and record 1 (`read("right")`) **not** consumed, and the
  live-fallback invocation log stays empty. The replay does not force the
  candidate to execute `read("right")`.

Allowed interpretation if S1/S2 pass: *in this deterministic read-only
fixture, exact historical interaction consumption is not necessary for
this subset candidate.* Not allowed: "historical interactions can always
be safely omitted" — for effectful or stateful systems this may be
false.

## Candidate B — Reorder

Trajectory: `read("right")`, then `read("left")` — the historical two
read-only interactions in the opposite order. Output preserves the
candidate's own order: `"R|L"`.

Two local experimental replay mechanisms are tested:

- R1 (condition B1, identity-aware experimental replay): each candidate
  call is resolved by its exact recorded interaction identity
  (operation + argument). Expected: `read("right") -> R`,
  `read("left") -> L`, output `"R|L"`, both records consumed, and the
  live-fallback callback never invoked. PASS if the reordered candidate
  executes correctly from historical evidence.
- R2 (condition B2, strict positional negative control): a deliberately
  strict mechanism requiring historical position *i* == candidate call
  *i*. The candidate's first call `read("right")` does not match
  historical position 0 `read("left")`; the mechanism must fail
  explicitly. PASS if exact positional matching rejects the reorder
  **while** R1 demonstrates historical evidence existed for both
  requested interactions.

Allowed interpretation: *exact global trajectory equality can be
unnecessarily restrictive in a deterministic read-only fixture.*
Not allowed: "interaction ordering never matters," or "identity-based
matching is the final correct replay policy." Side effects and state
dependencies are not tested here.

## Candidate C — Novel Interaction

Trajectory: `read("left")`, then `read("novel")`. The history contains
`read("left")` and `read("right")` but **not** `read("novel")`. The
candidate trajectory differs, and requested evidence is **not** fully
covered.

- N1 — the unseen call is detected explicitly: replay succeeds for
  `read("left") -> L`, then produces an explicit
  `MissingHistoricalEvidence("read(\"novel\")")` result (a private,
  local error type in the example; not a production error).
- N2 — the evidence gap has no fallback, derived from three external
  observations: the consumption log shows only record 0 consumed (the
  historical `read("right")` was not substituted in); the live-fallback
  callback invocation log stays empty — the callback would have returned
  a distinguishable live value if it had been invoked, but it was not;
  and the run is an error, not a fabricated value. The candidate
  therefore **cannot** be claimed as fully replayed from historical
  evidence.

Allowed interpretation: *under a replay regime restricted to the recorded
historical evidence, this candidate cannot be fully replayed because one
requested external interaction has no recorded outcome.* Not allowed:
"every unseen interaction makes candidate evaluation impossible" — other
mechanisms (simulation, environment snapshots, models, or other sources
of additional controlled evidence) might help; they are outside this
experiment.

## Conceptual Distinction: Evidence Coverage vs. Trajectory Equality

The experiment makes visible the difference between:

- **historical trajectory equality** (the candidate's sequence of
  interactions equals the historical sequence), and
- **historical evidence coverage for candidate interactions** (every
  interaction the candidate requests has a recorded historical outcome).

In this fixture:

| Candidate | Trajectory differs? | Requested evidence covered? |
|---|---|---|
| A (subset) | yes | yes |
| B (reorder) | yes (order) | yes |
| C (novel) | yes | no |

The distinction is deliberately left as **experimental evidence**. No
production concept such as `CoveragePolicy`, `ReplayEligibility`,
`ComparabilityEngine`, or `TrajectoryMatcher` is introduced.

## Independent Variables

The trajectory divergence form, one candidate per form, against the same
history:

- historical (control: `read("left")`, `read("right")`)
- subset (Candidate A)
- reorder (Candidate B)
- novel interaction (Candidate C)

and, for the reorder negative control:

- identity-aware experimental replay
- strict positional replay

## Controlled Variables

Held fixed across every condition:

- Same historical record (constructed identically each run).
- Same deterministic read-only outcomes (`L`, `R`; no errors, no
  mutation).
- Same read-only interaction semantics for every replay.
- No live fallback during replay: each replay receives the
  experiment-local live fallback callback (the only path to live state),
  and each condition's callback invocation log is asserted empty
  afterwards. The callback is a real path — it would return a
  distinguishable live value if invoked — but the replay policy under
  test deliberately refuses it.
- No randomness, no wall clock, no I/O, no async.
- Same candidate behavior code within each comparison; only the
  argument trajectory exercised by that code differs.
- Same non-divergence replay information (the full record is used for
  every replay condition; nothing is ablated in this experiment).
- External instrumentation: the live fallback callback owns the
  live-call log and the driver reads it only after the replay is
  dropped; the consumption log is likewise driver-owned and read only
  after the replay is dropped. No metric is self-reported by the code it
  measures.

## Input / Replay Dataset

One fully scripted in-memory history, defined in the example; nothing is
serialized or persisted — the record is constructed in memory each run.

Reproducible by running:

```sh
cargo run -p mutagen-runtime --example exp_0003_trajectory_divergence
```

## Metrics

Behavioral pass/fail observations (no weighting, no scoring, no
aggregation):

| Metric | Meaning |
|---|---|
| F1 | Historical baseline faithfully replays |
| S1 | Subset candidate executes entirely from recorded historical evidence |
| S2 | Unused historical interaction does not invalidate subset replay in this fixture |
| R1 | Reordered candidate executes from recorded evidence under identity-aware experimental replay |
| R2 | Strict positional replay rejects the reorder despite evidence existing for both requested interactions |
| N1 | Novel candidate interaction produces an explicit historical-evidence gap |
| N2 | Evidence gap does not fall back to live state, defaults, or another historical interaction |

F1 is a control condition: the divergent-candidate conditions are
invalid if the historical baseline does not replay.

## Execution Environment

```
rustc 1.98.0 (88d9e12ae 2026-08-18) (Homebrew)
cargo 1.98.0 (797e8a9bc 2026-08-05) (Homebrew)
Darwin zhuchenhaos-Mac-Studio.local 25.6.0 Darwin Kernel Version 25.6.0: Fri Jul 31 19:17:26 PDT 2026; root:xnu-12377.161.14~5/RELEASE_ARM64_T6041 arm64
code-under-test commit: 901181ae830bfaaf934e45f249f06b92d80f4ae0 (branch exp/0003-trajectory-divergence; base main 21b755b; supersedes bb8892a, whose self-referential live-log instrumentation was replaced by the external live-fallback callback)
```

Hardware class: Apple M-series Mac (arm64). Dependencies: none added; the
example uses only the Rust standard library. Cargo.toml and Cargo.lock
are unmodified. The recorded commit is the experiment commit whose code
the three runs executed; it is recorded as the **code-under-test
commit**. (If this document is later amended, the SHA refers to the
experiment code commit, not to the amending document commit — the SHA is
not self-referential and is not made to be.)

## Results

Raw output of a run (identical across three runs):

```
Experiment 0003 — Replay Under Trajectory Divergence
history: read("left") -> Value("L"), read("right") -> Value("R"); baseline output "L|R"
  instrumentation: live fallback callback logs each invocation and returns Value("LIVE"); every replay holds the callback but must never invoke it
  control: read("left") -> L, read("right") -> R, output "L|R"; live fallback never invoked
F1 PASS
  candidate A: read("left") -> L, output "L"; unused historical interaction read("right") remains unconsumed; live fallback never invoked
S1 PASS
S2 PASS
  candidate B (identity-aware): read("right") -> R, read("left") -> L, output "R|L"; live fallback never invoked
  candidate B (strict positional): mismatch at position 0 — expected read("left"), got read("right"); evidence existed for both requested interactions under identity-aware replay
R1 PASS
R2 PASS
  candidate C: read("left") -> L, then read("novel") -> MissingHistoricalEvidence(read("novel")); live fallback never invoked, no substitution of read("right"), no fabricated value
N1 PASS
N2 PASS
Conclusion: supported
```

- F1 PASS — the baseline replayed `"L|R"` from the record, consuming both
  records in order, with the live-fallback callback never invoked.
- S1 PASS — the subset candidate produced `"L"` with the
  live-fallback invocation log empty.
- S2 PASS — the external consumption log shows `read("left")` (record 0)
  consumed and `read("right")` (record 1) unconsumed; the replay neither
  invalidated the run nor force-executed the unused interaction.
- R1 PASS — the reordered candidate produced `"R|L"` (the candidate's own
  order) with both records consumed and the live-fallback callback never
  invoked.
- R2 PASS — strict positional matching rejected the reorder at position 0
  (expected `read("left")`, got `read("right")`) while R1 shows evidence
  existed for both requested interactions.
- N1 PASS — `read("novel")` produced an explicit
  `MissingHistoricalEvidence(read("novel"))` after the covered
  `read("left")` succeeded.
- N2 PASS — after the gap: consumption log = [0] only (no substitution of
  `read("right")`), live-fallback invocation log empty (the callback is a
  real path that would return `Value("LIVE")` if invoked; it was not),
  and the run is an error, not a fabricated value.

## Three-Run Reproducibility

The example was executed three separate times via
`cargo run -p mutagen-runtime --example exp_0003_trajectory_divergence`
at the code-under-test commit. All three runs produced byte-identical
output:

| Run | Output |
|---|---|
| Run 1 | `F1 PASS`, `S1 PASS`, `S2 PASS`, `R1 PASS`, `R2 PASS`, `N1 PASS`, `N2 PASS`, `Conclusion: supported` |
| Run 2 | `F1 PASS`, `S1 PASS`, `S2 PASS`, `R1 PASS`, `R2 PASS`, `N1 PASS`, `N2 PASS`, `Conclusion: supported` |
| Run 3 | `F1 PASS`, `S1 PASS`, `S2 PASS`, `R1 PASS`, `R2 PASS`, `N1 PASS`, `N2 PASS`, `Conclusion: supported` |

No timestamps, random identifiers, or environment-dependent values
appear in any output.

## Artifacts

- Example (the entire experiment):
  `crates/mutagen-runtime/examples/exp_0003_trajectory_divergence.rs`
- This record: `docs/experiments/0003-trajectory-divergence.md`

No traces, episodes, or datasets are persisted anywhere else; the record
is constructed in memory each run.

## Conclusion

**supported** — scoped strictly to the tested interaction patterns.

> In a deterministic, read-only fixture, a candidate that omits or
> reorders recorded interactions can remain replayable from historical
> evidence when every interaction it requests is covered, while a
> candidate requesting an unseen interaction exposes an explicit evidence
> gap — and strict trajectory equality can reject reorders that the
> recorded evidence fully covers.

The strongest allowable form. The experiment establishes local
feasibility and explicit-gap counterexamples; it does **not** select a
replay policy, a matching key, or a comparability criterion.

### Demonstrated (in the tested fixture only)

- Omission (S1, S2): a subset candidate executed entirely from recorded
  evidence; the unused historical interaction neither invalidated the
  replay nor was force-consumed or force-executed.
- Reordering (R1, R2): identity-aware replay executed the reordered
  candidate correctly; strict positional matching rejected it; both
  observations hold against the same record, making the trajectory-
  equality / evidence-coverage distinction visible.
- Novel interaction (N1, N2): the unseen call produced an explicit
  `MissingHistoricalEvidence` with no live, default, or substitution
  fallback.
- External instrumentation: "live state not consulted" is asserted from
  the driver-owned invocation log of the live fallback callback — the
  only path a replay could take to live state — and "record not consumed"
  from the driver-owned consumption log; both are external to the
  replay action (per the Experiment 0001/0002 lesson against
  self-reporting).

### Not established

This experiment does **not** establish or select:

- a final replay policy or a production replay schema — the local types
  (`Call`, `Outcome`, `RecordedInteraction`,
  `ExperimentalReplayError`) and both replay mechanisms are private,
  throwaway scaffolding in the example file;
- a universal comparability criterion, or the claim that a candidate is
  comparable iff all of its calls exist in the history (that is a
  production policy claim);
- the claim that unused historical calls are always irrelevant;
- the claim that reordering is always safe;
- the claim that exact-call identity is the final matching key;
- the claim that live fallbacks should never be permitted in any replay
  regime — the experiment only shows that this particular replay regime
  did not use one;
- the claim that historical evidence is the only possible source of
  controlled evidence;
- trajectory coverage as a final evaluator architecture;
- side-effect safety — the fixture is side-effect-free by construction;
- reorder safety in general, or any claim about stateful or effectful
  interactions;
- request/context necessity — left unresolved, as in Experiments 0001
  and 0002 (no claim about persisting or reconstructing inputs, requests,
  prompts, context, or memory state);
- asynchronous or concurrent execution;
- stochastic execution;
- LLM replay;
- cross-domain generalization;
- a production API or public type of any kind;
- serialization or persistence;
- promotion, rollback, or any deployment policy.

### Generalization limitation

This experiment uses deterministic, side-effect-free read interactions.
It does not establish replay validity for effectful, stateful,
asynchronous, concurrent, stochastic, or model-driven interactions.

The experiment investigates structural trajectory-divergence patterns,
but does not prove a general-agent invariant. The interaction patterns
(omission, reordering, novel call) are structural rather than
business-domain concepts and deliberately use domain-neutral vocabulary
(read, left, right, novel); domain-neutral naming is not itself evidence
of cross-domain generalization, and nothing here claims any.

## Follow-up Questions

Recorded, not implemented:

1. How does replay change when external interactions have side effects or
   mutate shared state?
2. Can a general notion of evidence coverage survive across
   heterogeneous agent task types?
3. When a candidate introduces an unseen interaction, what controlled
   sources of additional evidence, if any, can make evaluation valid?
4. How should stochastic model-generated trajectory divergence be
   evaluated?
