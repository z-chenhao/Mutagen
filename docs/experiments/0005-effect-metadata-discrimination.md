# Experiment 0005 — Effect Metadata Discrimination

## Experiment ID

0005

## Research Question

Can resource-scoped read/write effect metadata distinguish causally unsafe
historical-outcome reuse from safe trajectory divergence more accurately
than call coverage alone or coarse mutating/non-mutating information in a
deterministic stateful fixture?

Secondary formulation: does knowing which resources an interaction reads
or writes provide useful replay-safety information beyond knowing only
that an interaction exists or that some prior interaction mutates state?

## Prior Evidence

Narrowly summarized:

- **Experiment 0003**
  ([`0003-trajectory-divergence.md`](0003-trajectory-divergence.md)): in a
  **deterministic, independent, read-only fixture**, historical call
  coverage could be sufficient even when the candidate omitted or
  reordered interactions — because every outcome there was a function of
  the current call alone.
- **Experiment 0004**
  ([`0004-stateful-causality.md`](0004-stateful-causality.md)): in a
  **deterministic stateful fixture**, historical call coverage alone was
  insufficient, because a recorded outcome depended on an earlier state
  mutation that the candidate omitted or reordered. It established:

  ```
  call identity coverage  !=  causal validity
  ```

Experiment 0004 did **not** answer: what information, short of fully
re-executing the environment, could help identify when reuse of a
historical outcome is causally risky? This experiment investigates one
candidate signal: **effect metadata**.

## Hypothesis

Stated before running:

In the tested deterministic state-machine fixtures, call coverage alone
is under-conservative because it can allow reuse of causally stale
historical outcomes, while a coarse "some prior mutation exists" rule is
over-conservative because it can reject unrelated mutations.
Resource-scoped read/write metadata can distinguish these cases by
detecting overlap between prior writes and the state read by the
historical observation.

Expected:

1. exact historical execution should be allowed by all tested guards;
2. coverage-only should incorrectly allow reuse when a causal write is
   omitted;
3. coverage-only should incorrectly allow reuse when a causal write is
   reordered after its dependent read;
4. a coarse mutation-aware guard should reject those unsafe cases;
5. the same coarse guard should also reject a safe case where the
   omitted historical write affects a different resource;
6. a resource-scoped guard should reject the two same-resource causal
   violations;
7. the resource-scoped guard should allow the unrelated-resource case;
8. these decisions should be evaluated against an independent
   deterministic reference execution.

**Important:** this experiment may show useful discriminatory power in
these fixtures. It must **not** claim that resource-scoped read/write
metadata is universally sufficient.

## Metadata Assumption

Effect metadata is **manually declared and assumed correct** for this
experiment. This experiment does **not** test:

- how metadata is inferred;
- whether an LLM can infer it;
- whether a tool author lies;
- whether metadata becomes stale;
- whether metadata is complete;
- whether runtime verification is needed.

The question is only: **assuming effect metadata is correct, does its
information content help distinguish the tested replay cases?**

## Environment

A tiny, fully scripted, in-memory, deterministic state machine (standard
library only; no network, filesystem, LLM, randomness, wall clock, async,
or external process) with **two independent resources**:

```
resources:  x, y
pre-state:  x = EMPTY, y = B
```

Supported structural interactions (domain-neutral):

```
set_x_to_a : x = A          (write of x)
read_x     : returns x      (read of x; EMPTY if unset)
read_y     : returns y      (read of y)
```

The important property:

```
write(x)  affects   read(x)
write(x)  does NOT affect  read(y)
```

Experiment-local effect metadata (manually declared, assumed correct,
private to the example file — not a production type):

```
set_x_to_a :  reads = {}   writes = {x}
read_x     :  reads = {x}  writes = {}
read_y     :  reads = {y}  writes = {}
```

The exact Rust shape in the example is a private `struct Effects { reads:
&'static [&'static str], writes: &'static [&'static str] }`.

## Guards

Three experiment-local guard strategies, each returning
`AllowHistoricalReuse` / `RejectHistoricalReuse` (a private experiment-only
enum; no production policy abstraction).

**Anti-circularity requirement (binding):** each safety guard and the
reference execution must be independent. Each guard decides using ONLY:
historical trajectory, candidate trajectory, call identity, effect
metadata, and relative ordering. Each guard MUST NOT inspect: the
reference execution result, candidate runtime outcomes,
historical-vs-reference outcome comparisons, or environment internal
state. The reference execution is evaluated separately, afterward:

```
                 ┌── guard decision
metadata/history ┤
                 └── candidate reference execution
                          ↓
                compare decision with truth
```

Not:

```
run reference → see whether outcome differs → make guard decision
```

The example enforces this structurally: the guard functions take only
trajectory and metadata inputs; only the metric step combines guard
decisions with the reference result.

### G0 — Coverage Only

Allow reuse if the evaluated candidate call has a matching historical
call identity. Ignores all effects and outcomes. This approximates the
intuition falsified by Experiment 0004.

### G1 — Coarse Mutation-Aware

Knows only `read-only` / `writes-state`; it does **not** know which
resource is read or written. For a historical read outcome:

1. identify state-mutating historical interactions before that read;
2. if any such historical mutation is missing from the candidate before
   its corresponding read, reject historical reuse;
3. otherwise allow.

This is a deliberately conservative policy; it should detect
omitted/reordered writes. But because it lacks resource identity, it may
reject unrelated writes. **It is not optimized: its over-conservatism is
part of the experiment.**

### G2 — Resource-Scoped Effects

Uses `reads = {...}` / `writes = {...}`. For the evaluated candidate
read:

1. identify the matching historical read (exact call identity);
2. inspect historical interactions before that read;
3. consider only prior historical writes whose writeset overlaps the
   read's readset;
4. for each overlapping historical write, verify the corresponding write
   (exact call identity) still appears before the candidate read;
5. if a required overlapping write is absent or appears after the
   candidate read, reject;
6. otherwise allow.

The overlap predicate is local and transparent:

```
depends_on(write, read)  iff  write.writes ∩ read.reads != ∅
```

No transitive dependencies, no graph, no production dependency
abstraction, no outcome inspection.

## Reference Validity

For every fixture, whether historical-outcome reuse is *actually* valid
is determined independently:

1. start from a fresh exact historical pre-state (`x = EMPTY, y = B`);
2. execute the candidate trajectory using the deterministic environment;
3. locate the candidate read outcome (the evaluated read);
4. compare that actual outcome with the recorded historical outcome for
   the identity-matched read.

```
historical outcome reusable in this fixture
  iff
reference candidate read outcome == historical recorded read outcome
```

This definition is valid **only for this fully modeled deterministic
fixture**. It is not a universal evaluator rule.

## Fixture C0 — Exact Historical Trajectory

History: `set x = A`, `read x -> A`. Candidate identical: `set x = A`,
`read x`. Reference: `read x -> A`, matching history ⇒ **valid**.

Expected: G0 ALLOW, G1 ALLOW, G2 ALLOW. Baseline control: if this fails,
the run is broken.

## Fixture D1 — Dependent Write Omitted

History: `set x = A`, `read x -> A`. Candidate: `read x` only. Call
coverage exists. Reference from `x = EMPTY`: `read x -> EMPTY` ≠ A ⇒
**invalid**.

Expected: G0 ALLOW (unsafe false acceptance), G1 REJECT, G2 REJECT.

## Fixture D2 — Dependent Write Reordered

History: `set x = A`, `read x -> A`. Candidate: `read x`, `set x = A`.
Both calls historically covered. Reference: `read x -> EMPTY` ⇒
**invalid**.

Expected: G0 ALLOW, G1 REJECT, G2 REJECT.

## Fixture U1 — Unrelated Historical Write Omitted

**Essential control.** Distinguishes "some mutation happened" from "the
mutation affected what this read depends on".

History: `set x = A`, `read y -> B`. Candidate: `read y` only. The
historical write to x is omitted, but `write(x)` does **not** affect
`read(y)`. Reference: `read y -> B` matches history ⇒ **valid**.

Expected: G0 ALLOW, G1 REJECT (false rejection / over-conservative), G2
ALLOW. Without U1, the experiment would only show that "being
conservative" works.

(No additional independent-read-only fixture is added; Experiment 0003
already demonstrated independent read-only reorder behavior, and U1 is
the safe-divergence control needed here. Scope is kept minimal.)

## Metrics

Behavioral pass/fail (no scoring, no percentages, no ranking). Each
metric is derived from the actual guard decisions combined with the
actual reference execution result; no guard inspects the reference
result.

| Metric | Meaning |
|---|---|
| C0 | Exact historical trajectory is valid and all guards allow reuse |
| D1 | Dependent-write omission is invalid; G0 allows while G1/G2 reject |
| D2 | Dependent-write reorder is invalid; G0 allows while G1/G2 reject |
| U1 | Unrelated-write omission is valid; G0/G2 allow while G1 rejects |

Values: PASS / FAIL only.

## Execution Environment

```
rustc 1.98.0 (88d9e12ae 2026-08-18) (Homebrew)
cargo 1.98.0 (797e8a9bc 2026-08-05) (Homebrew)
Darwin zhuchenhaos-Mac-Studio.local 25.6.0 Darwin Kernel Version 25.6.0: Fri Jul 31 19:17:26 PDT 2026; root:xnu-12377.161.14~5/RELEASE_ARM64_T6041 arm64
code-under-test commit: __CODE_UNDER_TEST_SHA__ (branch exp/0005-effect-metadata-discrimination; base main 39d2cac)
```

Hardware class: Apple M-series Mac (arm64). Dependencies: none added; the
example uses only the Rust standard library. Cargo.toml and Cargo.lock
are unmodified. The recorded commit is the experiment commit whose code
the three runs executed; it is recorded as the **code-under-test commit**.
(If this document is later amended, the SHA refers to the experiment code
commit, not to the amending document commit — the SHA is not
self-referential and is not made to be.)

## Results

Raw output of a run (identical across three runs):

```
Experiment 0005 — Effect Metadata Discrimination
pre-state: x = EMPTY, y = B
  C0: history set_x_to_a -> Ack, read_x -> Value("A")
  D1: history set_x_to_a -> Ack, read_x -> Value("A")
  D2: history set_x_to_a -> Ack, read_x -> Value("A")
  U1: history set_x_to_a -> Ack, read_y -> Value("B")

fixture  | reference validity | G0     | G1     | G2
C0       | valid              | ALLOW  | ALLOW  | ALLOW
D1       | invalid            | ALLOW  | REJECT | REJECT
D2       | invalid            | ALLOW  | REJECT | REJECT
U1       | valid              | ALLOW  | REJECT | ALLOW

C0 PASS
D1 PASS
D2 PASS
U1 PASS
Conclusion: supported
```

Core decision table:

```
fixture | reference validity | G0   | G1   | G2
C0      | valid              | ALLOW| ALLOW| ALLOW
D1      | invalid            | ALLOW| REJECT| REJECT
D2      | invalid            | ALLOW| REJECT| REJECT
U1      | valid              | ALLOW| REJECT| ALLOW
```

- **C0** — the exact trajectory is valid by reference; all three guards
  allow. The baseline control holds.
- **D1** — reference shows the historical `A` is stale (candidate read
  actually returns `EMPTY`). G0 **falsely accepts** (coverage exists);
  G1 rejects (a prior mutation disappeared); G2 rejects (the
  write(x)/read(x) overlap lost its write).
- **D2** — same as D1 with the write present but reordered after the
  read; reference invalid; G0 falsely accepts; G1 and G2 reject.
- **U1** — reference shows reuse is valid (`read y -> B`). G0 allows.
  G1 **falsely rejects** (it sees the omitted mutation but cannot see
  that it concerns a different resource). G2 allows (no
  write(x) ∩ read(y) overlap, so nothing required was lost).

| Metric | PASS / FAIL | Evidence |
|---|---|---|
| C0 | PASS | reference valid; G0/G1/G2 all ALLOW |
| D1 | PASS | reference invalid; G0 ALLOW, G1 REJECT, G2 REJECT |
| D2 | PASS | reference invalid; G0 ALLOW, G1 REJECT, G2 REJECT |
| U1 | PASS | reference valid; G0 ALLOW, G1 REJECT, G2 ALLOW |

## Three-Run Reproducibility

The example was executed three separate times via
`cargo run -p mutagen-runtime --example exp_0005_effect_metadata_discrimination`
at the code-under-test commit. All three runs produced byte-identical
output:

| Run | Output |
|---|---|
| Run 1 | `C0 PASS`, `D1 PASS`, `D2 PASS`, `U1 PASS`, `Conclusion: supported` |
| Run 2 | `C0 PASS`, `D1 PASS`, `D2 PASS`, `U1 PASS`, `Conclusion: supported` |
| Run 3 | `C0 PASS`, `D1 PASS`, `D2 PASS`, `U1 PASS`, `Conclusion: supported` |

No timestamps, random identifiers, or environment-dependent values
appear in any output.

## Artifacts

- Example (the entire experiment):
  `crates/mutagen-runtime/examples/exp_0005_effect_metadata_discrimination.rs`
- This record: `docs/experiments/0005-effect-metadata-discrimination.md`

No traces, episodes, or datasets are persisted anywhere else; records
are constructed in memory each run.

## Conclusion

**supported** — scoped strictly to the tested fixtures.

> In these deterministic state-machine fixtures, call coverage alone
> fails to detect same-resource causal invalidation. A coarse
> mutation-aware guard can detect the tested causal violations but can
> reject an unrelated mutation unnecessarily. Resource-scoped read/write
> metadata provides enough additional information to distinguish the
> tested same-resource dependencies from the tested unrelated mutation.

Also allowed and observed: **the granularity of effect information can
matter for replay-safety discrimination.**

### Evidence We Can Claim

- In these fixtures, G0 (coverage only) produces a false acceptance on
  both same-resource causal violations (D1, D2): call identity coverage
  exists for the evaluated read, yet the historical outcome is stale by
  reference.
- G1 (coarse mutation-aware) rejects both same-resource violations
  (D1, D2) — detecting them without resource knowledge — but produces a
  false rejection on U1, where the omitted mutation concerns a different
  resource and reuse is valid by reference.
- G2 (resource-scoped) makes the reference-correct decision on all four
  tested fixtures: allows C0 and U1, rejects D1 and D2.
- Progressively richer effect information (none → mutation flag →
  resource-scoped read/write sets) changes the false-acceptance /
  false-rejection trade-off in the predicted direction, within these
  fixtures.

### Evidence We Cannot Claim

This experiment must **not** be read to conclude:

- resource-scoped read/write sets are sufficient for general agents;
- this is the minimal metadata representation;
- read/write sets are the correct production API;
- all causal dependencies are resource overlap;
- effect metadata can always be trusted;
- effect metadata can be inferred accurately;
- G2 is the final replay safety policy;
- snapshots are unnecessary;
- causal graphs are unnecessary;
- any universal replay architecture or effect model.

### Important Untested Dependency Classes

Explicitly untested; resource overlap is deliberately a simple case:

- transitive dependencies;
- hidden shared state;
- global state;
- time;
- randomness;
- external caches;
- network state;
- filesystem aliases;
- database constraints;
- side effects whose outputs do not expose the mutation;
- irreversible actions;
- async/concurrency;
- LLM/model state.

### Metadata Correctness Limitation (mandatory)

Experiment 0005 assumes effect metadata is complete and correct. It does
**not** answer:

- who produces metadata?
- can the tool author be trusted?
- can static analysis derive it?
- can an LLM infer it?
- can runtime tracing verify it?
- what happens when metadata is wrong?

These become follow-up questions.

## Relationship to Experiments 0003/0004

- **Experiment 0003:** coverage can suffice under independent read-only
  semantics (outcome = f(current call)).
- **Experiment 0004:** coverage can fail under causal state dependencies
  (outcome = f(previous state, previous effects, current call)).
- **Experiment 0005:** tests whether progressively richer effect
  information (coverage → mutation flag → resource-scoped read/write
  sets) can discriminate those kinds of cases in a controlled state
  machine, and whether the added information improves both false
  acceptance and false rejection behavior.

These are complementary, not contradictory: 0003 and 0004 bound where
coverage works and fails; 0005 asks whether modest effect information
moves the boundary.

## Generalization Limitation

The experiment studies **structural concepts**: read set, write set,
resource overlap, effect preservation, and historical outcome reuse.
These may be relevant to coding agents, filesystem agents, browser
agents, workflow agents, database tools, and interactive environments —
but **none of those systems is tested here**. Do not claim cross-domain
generalization. The x/y set/read vocabulary is deliberately
domain-neutral; domain-neutral naming is not itself evidence of
generalization.

## Follow-up Questions

Recorded, not implemented:

1. What happens when effect metadata is incomplete or wrong?
2. Can effect metadata be inferred safely rather than manually declared?
3. Are read/write sets sufficient when dependencies are transitive or
   hidden?
4. How should unknown effects be represented?
5. Should an unknown effect default to conservative rejection?
6. Which effect concepts survive across heterogeneous agent
   environments?
