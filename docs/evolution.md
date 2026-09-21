# Self-Evolution: Problem Definition

This document captures **the problem**. It deliberately does not contain
**the solution** — the solution must come from experiments
(see [`experiments/`](experiments/)) and decisions
(see [`decisions/`](decisions/)).

## Current definition

**Self-evolution** is a persistent system change caused by accumulated
execution experience or user feedback.

A change only counts as evolution if it is:

- **Caused by evidence** — derived from recorded experience or feedback,
  not from human redesign.
- **Persistent** — survives across runs, i.e. it is a versioned artifact,
  not an in-memory tweak.
- **Validated** — measured against scenarios before it affects real usage.
- **Reversible** — can be rolled back when observation shows harm.

## Conceptual lifecycle

```
Experience → Diagnosis → Candidate change → Replay/Evaluation → Validation
           → Promotion → Observation → Rollback (if necessary)
```

Stage glossary:

| Stage | Meaning |
|---|---|
| Experience | Recorded runs, failures, and user feedback. |
| Diagnosis | Identifying *which* part of the system and *why* it underperformed. |
| Candidate change | A concrete, addressable proposal (a versioned artifact). |
| Replay/Evaluation | Re-executing recorded scenarios against the candidate. |
| Validation | Deciding, with controlled comparison, whether the candidate is better. |
| Promotion | Making the candidate the active version (deploy / hot-swap). |
| Observation | Watching the promoted version in real use. |
| Rollback | Restoring the previous version when observation shows regression. |

## Candidate evolvable surfaces (hypotheses)

These are *questions to investigate*, not a component list to build:

- **Prompt** — instruction text given to models.
- **Context** — how information is selected and arranged for a model call.
- **Memory** — what is stored, retrieved, and forgotten.
- **Skill** — reusable procedures the agent can apply.
- **Tool** — capabilities exposed to the agent.
- **Workflow** — multi-step orchestration structure.
- **Routing** — decisions about which component/model handles what.
- **Runtime** — scheduling, concurrency, and execution policy.
- **Evaluation** — the judges and metrics themselves.
- **Evolution strategy** — the optimizer that proposes changes.

Nothing here is committed. Some may turn out to be the same underlying
mechanism; some may be impossible to make safely evolvable.

## Open research questions

These are the agenda. **None of them is answered in this repository yet.**

1. What exactly should be evolvable?
2. What should remain immutable? (Correctness-critical invariants, security
   boundaries, resource limits?)
3. What unit represents an evolution candidate? (File? Config? Code? A
   graph? A prompt?)
4. How should feedback be converted into a proposed change?
5. How do we distinguish learning from benchmark overfitting?
6. How should replay work? (Determinism, stochastic model responses,
   environment drift.)
7. How do we evaluate generalization beyond the scenarios that motivated a
   change?
8. How do we detect regressions?
9. How should versions and lineage be represented? (The current codebase
   intentionally defines no representation.)
10. How should rollback work?
11. What does safe hot deployment mean in Rust? (Locks, `Arc` swaps,
    capability isolation, in-flight work?)
12. Should plugins use process isolation, WASI, dynamic libraries, or
    another mechanism?
13. Can an evolution strategy itself evolve — and what stops that from
    degenerating?
14. What metrics determine whether a candidate is promoted?

## Explicit non-claims

- Mutagen does **not** currently self-evolve.
- No design in this document implies any of the above surfaces will be
  built, or in what form.
