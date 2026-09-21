# Experiment 0004 — Stateful Causality Under Replay

## Experiment ID

0004

## Research Question

Is historical interaction coverage sufficient for faithful candidate replay
when recorded interactions mutate state that influences later
observations?

Secondary formulation: can a candidate request only historically known
interactions and still receive causally invalid historical outcomes after
omitting or reordering state-changing interactions?

## Prior Evidence

Experiment 0001
([`0001-replay-boundary.md`](0001-replay-boundary.md))
demonstrated **only** locally, in one deterministic scenario, that recorded
external observations can form a viable replay seam.

Experiment 0002
([`0002-replay-record-ablation.md`](0002-replay-record-ablation.md))
demonstrated that overly lossy interaction records can destroy replay
fidelity: argument identity, occurrence distinction, and error outcomes can
matter. It considered only trajectories that differ in the *information* of
individual calls — never a trajectory that omits or reorders
interactions.

Experiment 0003
([`0003-trajectory-divergence.md`](0003-trajectory-divergence.md))
demonstrated — specifically in a **deterministic, read-only, and
interaction-independent fixture** — that *trajectory equality ≠ historical
evidence coverage*: a subset candidate and a reordered candidate could
execute from historical evidence even though their trajectories differed.
In that fixture, coverage was sufficient **only because** every
interaction was an independent read-only lookup whose outcome depended on
the call alone.

Experiment 0003 explicitly did **not** establish that this remains true for
stateful interactions, side effects, or causally dependent observations.
This experiment attacks exactly that boundary.

## Hypothesis

Stated before running:

In a deterministic stateful environment, historical interaction coverage
alone is insufficient for faithful counterfactual replay. If an earlier
interaction mutates state that determines a later observation, a candidate
that omits or reorders that mutation can have every requested interaction
represented in the historical record while historical-outcome replay still
disagrees with execution from the same historical pre-state.

Expected observations:

1. the exact historical trajectory is reproducible from the historical
   pre-state;
2. simple historical-outcome replay can reproduce the historical
   baseline's observable result;
3. a subset candidate can have complete historical call coverage but
   receive a stale historical outcome;
4. executing that subset candidate from the same pre-state produces a
   different result;
5. a reordered candidate can also have complete historical call coverage;
6. historical-outcome replay of that reorder can disagree with execution
   from the same pre-state;
7. therefore call-level historical evidence coverage is not sufficient, in
   this stateful fixture, to establish counterfactual validity.

Scope of the hypothesis: scoped to **one deterministic stateful fixture**.
It does not claim a universal solution.

## Historical Pre-State

```
slot = EMPTY
```

A fully known, fully specified initial state of the deterministic
state machine. The exact pre-state is one of the controlled variables of
the experiment: every comparison (historical execution, both candidates,
both mechanisms) starts from exactly this state.

## Historical Execution

One small, fully scripted, in-memory, deterministic stateful execution
(standard library only; no network, filesystem, LLM, randomness, wall
clock, async, or external process):

```
write("slot", "A") -> Ack        (mutates: slot = "A")
read("slot")       -> Value("A") (depends on the prior write)
final state:       slot = "A"
```

The stateful environment itself is the source of outcomes: the trajectory
is *executed* from a fresh copy of the pre-state and the resulting
interactions and outcomes are recorded. No historical outcome is manually
constructed.

The local experimental representation (private to the example file, not a
production type) is:

```rust
enum Call {
    Write { key: &'static str, value: &'static str },
    Read { key: &'static str },
}
enum Outcome {
    Ack,
    Value(&'static str),
}
struct RecordedInteraction { call: Call, outcome: Outcome }
```

## Experimental Mechanisms

Two mechanisms are defined locally in the example file. **Both are
experiment-only mechanisms** — private throwaway scaffolding, not
production design, not a shared abstraction, and not reusable outside this
file.

### Historical-outcome replay (the experimental subject / negative mechanism)

The candidate requests a call; the mechanism finds the first not-yet-
consumed recorded interaction with the same call identity (write/read plus
all arguments); it returns the recorded historical outcome; it does **not**
re-execute the environment transition. It intentionally ignores causal
state evolution: the outcome it returns is a function of the *current call
identity alone*. This is the over-generalization under test. It must not
be presented as a production design.

### Reference stateful execution (the counterfactual reference)

The candidate's actual trajectory is executed against a **fresh copy of the
exact same historical pre-state** using the deterministic state-machine
semantics. This is the counterfactual reference for **this experiment
only**. It is not a universal replay solution and it is not promoted to
production architecture.

It is usable *here* because, and only because:

- the initial state is fully known;
- the environment semantics are deterministic;
- all effects are fully modeled locally.

## Candidate A — Omitted Causal Predecessor

Trajectory: `read("slot")` only. The call exists in the historical record
(record index 1), so its historical call coverage is complete. But the
historical read's outcome depended on the earlier `write("slot", "A")`,
which the candidate omits.

- S1 (condition A1, historical-outcome replay): the candidate's read has a
  historical identity match (record index 1) and replay returns the
  recorded `Value("A")`. This demonstrates only that call-level evidence
  coverage exists.
- S2 (condition A2, reference stateful execution): executed from a fresh
  copy of the pre-state (`slot = EMPTY`), the read returns
  `Value("EMPTY")` because the historical write never occurred.
- S3: the two mechanisms' results disagree directly
  (`Value("A")` vs `Value("EMPTY")`).

Interpretation: in this fixture, the historical read outcome depended on a
state mutation omitted by the candidate. This experiment does **not**
claim omitted interactions are always causally required.

## Candidate B — Reordered Causal Predecessor

Trajectory: `read("slot")`, then `write("slot", "A")`. Both calls
independently exist in the historical record (record indices 1 and 0), so
Candidate B has complete call-identity coverage.

- R1 (condition B1, historical-outcome replay): replay returns the
  recorded outcomes `Value("A")`, `Ack` — both calls covered.
- R2 (condition B2, reference stateful execution): executed from
  `slot = EMPTY`, the read **before** the write returns
  `Value("EMPTY")`, then the write returns `Ack`.
- R3: the two mechanisms disagree on the read result (`Value("A")` vs
  `Value("EMPTY")`).

Interpretation: in this fixture, reordering a state-mutating predecessor
after a dependent read invalidates reuse of that historical read outcome
even though both call identities are historically covered. This experiment
does **not** conclude all reordering is unsafe.

## Conceptual Distinction: Coverage vs. Causal Validity

The experiment makes visible the difference between:

- **interaction identity coverage** (every call the candidate requests has
  a recorded historical outcome), and
- **causal validity of the historical outcome** (the recorded outcome is
  still the outcome that call would produce under the candidate's
  trajectory).

A historical outcome may exist for the same call but still be invalid
under a changed trajectory, because in a stateful environment:

```
outcome = f(previous state, previous effects, current call)
```

not merely:

```
outcome = f(current call)
```

This is the conceptual result under test. No causal graph representation
is built; the phenomenon is demonstrated directly by comparing mechanism
outputs.

## Candidate Comparison Summary

```
Candidate A — subset
requested historical calls covered?      YES (record index 1)
historical-outcome replay:               A
stateful reference from pre-state:       EMPTY
=> coverage was insufficient

Candidate B — reorder
requested historical calls covered?      YES (record indices 1, 0)
historical-outcome replay:               [A, Ack]
stateful reference:                      [EMPTY, Ack]
=> coverage was insufficient
```

## Independent Variables

The candidate trajectory form, one candidate per form, against the same
history:

- historical (control: `write("slot", "A")`, `read("slot")`)
- subset (Candidate A)
- reorder (Candidate B)

and, within each candidate comparison, the evaluation mechanism:

- reuse historical outcomes (historical-outcome replay)
- execute the candidate against the same historical pre-state (reference
  stateful execution)

## Controlled Variables

Held fixed across every condition:

- Exact historical pre-state (`slot = EMPTY`); every execution and replay
  comparison starts from it.
- Deterministic state-machine semantics (the single private
  `Environment`; two operations, fully modeled locally).
- Interaction identities (one `write` and one `read` on one key).
- Recorded historical outcomes (produced by executing the environment, not
  asserted by hand).
- Candidate trajectory within each comparison (each comparison holds the
  candidate fixed; only the mechanism differs).
- No randomness, no network, no filesystem, no wall clock, no LLM, no
  external process, no concurrency, no asynchronous behavior.

The only variable that differs within each candidate comparison:

- reuse historical outcomes
- vs. execute the candidate against the same pre-state

## Input / Replay Dataset

One fully scripted in-memory history, defined in the example; nothing is
serialized or persisted — the record is constructed in memory each run.

Reproducible by running:

```sh
cargo run -p mutagen-runtime --example exp_0004_stateful_causality
```

## Metrics

Behavioral pass/fail observations (no weighting, no scoring, no
aggregation):

| Metric | Meaning |
|---|---|
| H1 | Historical stateful execution produces the expected record and final state |
| H2 | Historical-outcome replay reproduces the exact historical baseline outcomes |
| S1 | Subset candidate has full call-level historical coverage and receives historical `Value("A")` |
| S2 | Subset candidate reference execution from historical pre-state produces `Value("EMPTY")` |
| S3 | Subset historical-outcome replay disagrees with reference stateful execution |
| R1 | Reordered candidate has full call-level historical coverage and receives historical outcomes |
| R2 | Reordered candidate reference execution from historical pre-state observes `EMPTY` before the write |
| R3 | Reordered historical-outcome replay disagrees with reference stateful execution |

H1 and H2 are controls: the candidate conditions are invalid if the
historical execution or the exact-trajectory replay is wrong. For S3 and
R3, the disagreement is derived from a direct comparison of the two
mechanisms' actual output values — never a hardcoded `true`. For coverage
claims (S1, R1), the driver-owned consumption log records the exact
historical record indices matched, not a bare "covered" statement.

## Execution Environment

```
rustc 1.98.0 (88d9e12ae 2026-08-18) (Homebrew)
cargo 1.98.0 (797e8a9bc 2026-08-05) (Homebrew)
Darwin zhuchenhaos-Mac-Studio.local 25.6.0 Darwin Kernel Version 25.6.0: Fri Jul 31 19:17:26 PDT 2026; root:xnu-12377.161.14~5/RELEASE_ARM64_T6041 arm64
code-under-test commit: 31bebe91d767f9b287dbdc0c6ace50def47c31ae (branch exp/0004-stateful-causality; base main 793806e)
```

Hardware class: Apple M-series Mac (arm64). Dependencies: none added; the
example uses only the Rust standard library. Cargo.toml and Cargo.lock are
unmodified. The recorded commit is the experiment commit whose code the
three runs executed; it is recorded as the **code-under-test commit**.
(If this document is later amended, the SHA refers to the experiment code
commit, not to the amending document commit — the SHA is not
self-referential and is not made to be.)

## Results

Raw output of a run (identical across three runs):

```
Experiment 0004 — Stateful Causality Under Replay
history: write("slot", "A") -> Ack, read("slot") -> Value("A"); final state "A"
H1 PASS
  baseline (historical-outcome replay): write -> Ack, read -> Value("A"); matches history
H2 PASS
  candidate A (subset): read("slot") covered by historical record index 1
 subset:
 historical-outcome replay = A
 stateful reference        = EMPTY
S1 PASS
S2 PASS
S3 PASS
  candidate B (reorder): read covered by historical record index 1, write covered by historical record index 0
 reorder:
 historical-outcome replay read = A
 stateful reference read        = EMPTY
 historical-outcome replay = [A, Ack], stateful reference = [EMPTY, Ack]
R1 PASS
R2 PASS
R3 PASS
Conclusion: supported
```

- H1 PASS — the historical trajectory executed from `slot = EMPTY`
  produced `write -> Ack`, `read -> Value("A")`, final state `"A"`, and
  the record matches the interactions actually executed.
- H2 PASS — historical-outcome replay of the exact historical trajectory
  reproduced the recorded outcomes (`Ack`, `Value("A")`), consuming both
  records: the negative mechanism is not trivially broken for the exact
  trajectory.
- S1 PASS — the subset candidate's `read("slot")` matched historical
  record index 1 and replay returned `Value("A")`.
- S2 PASS — the same candidate executed from a fresh pre-state produced
  `Value("EMPTY")`.
- S3 PASS — the two mechanisms' read results directly compared and
  disagreed: historical-outcome replay = `A`, stateful reference =
  `EMPTY`.
- R1 PASS — the reordered candidate's read and write matched historical
  record indices 1 and 0 and replay returned the recorded `Value("A")`,
  `Ack`.
- R2 PASS — the same candidate executed from the pre-state produced
  `Value("EMPTY")` (the read precedes the write) then `Ack`.
- R3 PASS — the two mechanisms' read results directly compared and
  disagreed: historical-outcome replay = `A`, stateful reference =
  `EMPTY`.

## Three-Run Reproducibility

The example was executed three separate times via
`cargo run -p mutagen-runtime --example exp_0004_stateful_causality`
at the code-under-test commit. All three runs produced byte-identical
output:

| Run | Output |
|---|---|
| Run 1 | `H1 PASS`, `H2 PASS`, `S1 PASS`, `S2 PASS`, `S3 PASS`, `R1 PASS`, `R2 PASS`, `R3 PASS`, `Conclusion: supported` |
| Run 2 | `H1 PASS`, `H2 PASS`, `S1 PASS`, `S2 PASS`, `S3 PASS`, `R1 PASS`, `R2 PASS`, `R3 PASS`, `Conclusion: supported` |
| Run 3 | `H1 PASS`, `H2 PASS`, `S1 PASS`, `S2 PASS`, `S3 PASS`, `R1 PASS`, `R2 PASS`, `R3 PASS`, `Conclusion: supported` |

No timestamps, random identifiers, or environment-dependent values appear
in any output.

## Artifacts

- Example (the entire experiment):
  `crates/mutagen-runtime/examples/exp_0004_stateful_causality.rs`
- This record: `docs/experiments/0004-stateful-causality.md`

No traces, episodes, or datasets are persisted anywhere else; the record
is constructed in memory each run.

## Conclusion

**supported** — scoped strictly to the tested fixture.

> In this deterministic stateful fixture, complete call-level historical
> evidence coverage is not sufficient for faithful counterfactual replay:
> historical outcomes can depend on earlier state mutations that a changed
> candidate omits or reorders.

Also allowed, and observed: the validity of a recorded outcome can depend
on **causal execution context**, not only call identity.

The strongest allowable form. The experiment establishes a causal
counterexample in one fully modeled deterministic state machine; it does
**not** select a replay policy, design a snapshot system, require a
causal graph, or state any universal solution.

### Evidence We Can Claim

- In this fixture, a candidate with complete call-level historical
  coverage (subset) can receive a stale historical outcome —
  `Value("A")` for a read whose causal predecessor write was omitted —
  while execution from the same pre-state yields `Value("EMPTY")`.
- In this fixture, a candidate with complete call-level historical
  coverage (reorder) can receive a causally invalid historical read
  outcome because the state-mutating predecessor was moved after the read.
- The historical-outcome mechanism reproduced the exact baseline
  faithfully (H2), so the discrepancy is attributable to the changed
  trajectory, not to a broken mechanism.
- Call-level coverage and causal validity are distinct notions, in this
  fixture.

### Evidence We Cannot Claim

This experiment must **not** be read to conclude:

- all stateful replay requires snapshots;
- full environment re-execution is the correct production solution;
- a causal graph is required;
- every side effect makes replay impossible;
- ordering must always be preserved globally;
- historical outcomes can never be reused for stateful tools;
- call coverage is useless — Experiment 0003 showed that call coverage
  can be sufficient in some independent read-only cases;
- a universal replay architecture, state snapshot design, causal graph,
  effect model, or general-agent invariant.

The combined evidence of Experiments 0003 and 0004 instead supports:

```
independent read-only interactions:  coverage may be sufficient
state-dependent interactions:        coverage alone may be insufficient
```

## Relationship to Experiment 0003

- **Experiment 0003:** trajectory divergence can be replayable when
  interactions are independent and read-only. In that fixture, coverage
  was sufficient only because each outcome was `f(current call)`.
- **Experiment 0004:** the same call-coverage intuition can fail when
  historical outcomes depend on prior mutations, because each outcome is
  `f(previous state, previous effects, current call)`.

These are **not contradictory results**. They define different
applicability conditions for the same observation: call-level historical
coverage is a property of the *candidate vs. record* relation, while
causal validity is a property of the *candidate trajectory vs. state*
relation. Experiment 0003 shows coverage can be sufficient under one
condition; Experiment 0004 shows it can be insufficient under another.
This **narrows the applicability** of the Experiment 0003 result rather
than contradicting it.

## Generalization Limitation

This experiment uses one completely modeled deterministic state machine.
It demonstrates a **causal counterexample to call-level evidence
coverage, not a universal replay strategy** for real effectful agent
environments.

Not tested:

- filesystem semantics;
- browser mutations;
- databases;
- distributed systems;
- irreversible side effects;
- concurrency;
- async;
- stochastic tools;
- LLM calls;
- cross-domain generalization.

The slot/state/write/read vocabulary is deliberately domain-neutral. The
stateful interaction pattern is structural and may occur in many agent
environments (filesystem operations, browser actions, code editing,
database tools, interactive environments, workflow systems), but domain-
neutral naming is not itself evidence of cross-domain generalization, and
nothing here claims any. None of those real systems is part of this
experiment.

## Follow-up Questions

Recorded, not implemented:

1. What information is required to establish whether two interactions are
   causally dependent?
2. Can effect classes such as read-only / state-mutating / irreversible
   be inferred or declared safely?
3. What historical state is necessary to evaluate a changed trajectory
   without reusing causally stale outcomes?
4. How should Mutagen handle external effects that cannot be
   deterministically replayed or rolled back?
5. Which of these causal requirements survive across coding, research,
   browser, filesystem, and workflow agents?
