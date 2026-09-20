# Experiments

This directory will hold the records of Mutagen's experiments.

**No experiment framework exists yet** — Phase 0 contains only this
contract. Future experiments are expected to be small, throwaway, and
reproducible; a framework will be extracted *from* real experiments, not
built ahead of them.

## The experiment contract

Every experiment record must contain:

| Field | Meaning |
|---|---|
| **Experiment ID** | Stable identifier; never reused. |
| **Hypothesis** | A falsifiable claim, stated before running. |
| **Baseline** | The configuration/behavior the experiment compares against. |
| **Independent variable** | Exactly what is being changed. One at a time. |
| **Controlled variables** | What is held fixed, and how. |
| **Input / replay dataset** | The exact scenarios/inputs used; reproducible or linked. |
| **Metrics** | What is measured, with units and direction of "better". |
| **Execution environment** | OS, toolchain version, hardware class, dependency versions (Cargo.lock). |
| **Result** | Raw outcomes; numbers, not adjectives. |
| **Artifacts** | Where traces, episodes, and outputs are stored. |
| **Conclusion** | Confirmed / refuted / inconclusive — tied to the hypothesis. |
| **Follow-up** | Next experiment, or an ADR if the result settles a design question. |

## Reproducibility rules

- A component change is **not** considered an improvement because one run
  performed better. Single observations are anecdote, not evidence.
- Comparisons must be controlled: same inputs, same environment,
  everything else held fixed.
- Measurements must be **repeated**; future experiments will define their
  own measurement protocols (iteration counts, variance treatment,
  holdout sets). The statistical methodology is a separate design effort
  and is intentionally not fixed here.
- Evaluation must use **held-out** scenarios distinct from those that
  motivated the change, to separate learning from overfitting.
- An experiment that cannot be re-run from its record does not count.

## Naming

Proposed: `NNNN-short-name.md` (e.g. `0001-replay-determinism.md`), in
chronological order, matching the ADR numbering domain in
[`../decisions/`](../decisions/). Final naming is settled when the first
experiment lands.
