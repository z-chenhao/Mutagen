# Experiment 0011 Results — Run `0011-r1`

## Formal verdict

**INCONCLUSIVE.** Run `0011-r1` completed Stages A0 → A1 → B (discovery)
and **terminated at Stage C (mutation generation)**: the mutation
generator failed to produce a fully schema-valid candidate pool within the
registered maximum of two attempts, so — exactly as pre-registered —
selection (D) and promotion (E) must not run, the run identity is
permanently invalid for further live stages, and **no confirmatory claim
about H1 (evolvability) is made**. Nothing was patched, hand-accepted,
or re-prompted: the frozen protocol produced this verdict mechanically.

What the run DOES establish (and is its primary scientific value):

1. **The mechanical source-freeze protocol works end-to-end.** The run
   completed its live stages with zero freeze violations; the guard ran
   before every live stage; the pre-registered failure modes (mid-run
   commit, modify-commit-revert, manifest tamper) are all caught —
   including in `self-test` against real temporary git repositories.
   0010's voiding condition cannot recur under this protocol.
2. **The gate machinery works.** Discovery gates, verifier, and
   whitelist checks all behaved as registered, including correctly
   *blocking* the next stage on a failed generation.
3. **A concrete, reproducible defect in the 0010/0011 mutator contract**
   (schema adherence; §6 below) is identified with full raw evidence.

## Run identity (all re-verifiable from committed artifacts)

| Field | Value |
|---|---|
| Run | `0011-r1` |
| Code-under-test commit (A0) | `1b2cc38edb3a85ebfdb9ae66c138ba0b078e34e9` |
| Frozen design SHA-256 (combined) | `db494301b0d902b500b9b89e3d38779ca8f4c69a22a771e5e7d45601764a1db0` |
| Run manifest SHA-256 | `3cff11c2f28d01d80fade5f8b9e175f64621cfe260e344a2308e681545fc769c` |
| Frozen files | 20 (all crate sources, Cargo.toml/lock, design.json, tasks.json, stress-profile.json, both prompts, pre-registration doc) |
| Model | `incoai/Qwen3.8-27B-Splash` @ `http://127.0.0.1:8000/v1` (no auth), agent temp 0.2, mutator temp 0.7 |
| Integrity at run end | `verify-run-manifest` OK (20/20 frozen files intact, clean history) |

## Stage B — Discovery (completed, verified)

- 18 episodes (3 E11 × 6 reps), G0 only, frozen 0009 family stress.
- **18 valid / 0 infrastructure / 6 agent-caused failures / 4 oracle
  successes → 14 incumbent failures** (gates: ≥16 valid, ≥8 failures —
  **both passed**; `proceed_to_mutation_generation: true`).
- Per task: E11A 0/6 · E11B 1/6 · E11C 3/6.
- Diagnostics: 156 model requests, 144 executed tool calls, 932 s total
  wall time (~52 s/episode).
- `verify-discovery`: **OK, no violations** (records, summary, mutation
  input, provenance, oracle recomputation, fault-audit replay all
  consistent). Raw: `discovery-raw.jsonl` (18 records).

## Stage C — Mutation generation (incomplete — run terminated here)

Two registered attempts (≤2 by the frozen protocol; retry message =
machine-generated validation errors only):

| Attempt | Wall | Outcome |
|---|---|---|
| 1 | 401 s | Valid JSON, **wrong key** (`"suffixes"` instead of `"candidates"`); string entries instead of `{suffix, rationale}` objects. Rejected: `wrong schema … missing field candidates` |
| 2 | 164 s | **Correct key** (`"candidates"`) with 4 well-formed suffix strings, still **string entries, not objects**. Rejected: `invalid type: string … expected object` |

The verifier then reported its 3 registered violations (incomplete
generation carrying a null accepted pool; empty pool ≠ 4 candidates) and
the gate **blocked selection** — the machinery behaved exactly as
pre-registered. The generated-but-rejected content is preserved verbatim
in `mutation-generation.json` (attempts 1–2 raw responses) and in
`candidate-pool.json` (empty pool).

### Root-cause analysis (honest)

The pre-registered mutator prompt (`prompts/mutator.md`, frozen) ends
with *"Return JSON only in the required schema"* — but **never defines
the schema** (it does not name `candidates`, `suffix`, or `rationale`).
0010 used the *byte-identical* prompt and simply got lucky: its attempt 1
happened to emit the exact `{"candidates":[{"suffix",…,"rationale",…}]}`
shape (see `0010-mutation-generation.json`). 0011 demonstrates that this
implicit contract is **not reliable** for this model: on 2 of 2 attempts
it produced semantically correct content in a non-conforming container
(key name drifted on attempt 1, object shape on attempt 2; it *did*
correct the key name in response to the attempt-1 error, so it is
following machine feedback — just one level short of the shape).

**Defect class:** pre-registered prompt/protocol mismatch — the prompt
promises a schema it does not specify. **This is a design defect to fix
in a *future* experiment** (explicit schema in the frozen mutator prompt
and/or more informative machine-generated errors, e.g. showing the exact
expected object shape). It must not be "fixed" inside 0011: the prompt
and validator are frozen design files, and any change would be the exact
failure mode this experiment was built to eliminate.

### Independent-rediscovery note (anti-seeding contract)

Attempt 2's rejected content — *"After any write, read back the same key
before treating the change as successful…"* — is **semantically similar
to 0010's selected C1** (read-after-write verification). Per the frozen
contract this is recorded as **independent rediscovery**, not injected
seed material: nothing from 0010 is copied into this crate, and the
similarity is reported, not rejected. (The content was not *accepted* —
acceptance requires schema validity — so it enters the record only as
evidence of the model's convergence on this policy class under the
frozen stress.)

## Stage D/E — Not run

Selection and promotion did not execute (gate-blocked). No selection,
promotion, or confirmatory statistics exist for 0011.

## Hypotheses

- **H1 (evolvability): NEITHER SUPPORTED NOR REFUTED** by 0011-r1
  (inconclusive at generation; no confirmatory data).
- **H2 (control apparatus): SUPPORTED at the stages exercised.** The
  loop's control machinery — frozen design single source of truth,
  mechanical source freeze with history semantics, run identity,
  discovery gates, independent verifiers, whitelist leak checks,
  containment of a failed generation — all operated exactly as
  pre-registered. In particular, the 0010 voiding condition (source
  change mid-run) is now mechanically impossible.

## Honest 0010 comparison

| | 0010 (voided) | 0011-r1 |
|---|---|---|
| Protocol integrity | **voided** (mid-run commit) | intact (frozen; verified at every stage) |
| Discovery | 18 ep, C1-generation evidence | 18 ep, 14 incumbent failures, verified |
| Mutation generation | succeeded on attempt 1 (lucky schema adherence) | **failed schema on 2/2** → run terminated |
| Selection / promotion | ran (post-violation; 35W/0L/13T, p≈5.8e-11 — *exploratory only*) | not run |
| Verdict | INCONCLUSIVE (integrity) | **INCONCLUSIVE (generation)** |

The 0010 C1 result remains *exploratory* evidence only. 0011 does not
confirm it; it confirms that a clean confirmation is a *protocol*
problem first, and pinpoints the one contract (output schema) that must
be made explicit before a confirmatory rerun can be expected to clear
Stage C.

## Pre-registration / mechanism inconsistency (disclosed)

The pre-registration doc (frozen at A0) states the document "must not
change after that point **except to append the final Results/Conclusion
sections**" — but the mechanical freeze hashes that same file, so
appending would violate the freeze. The inconsistency is disclosed here
rather than resolved by touching a frozen file; the results are recorded
in *this* (non-frozen) document instead. Future pre-registrations should
state that results live in a separate non-frozen file.

A second, cosmetic verifier quirk is disclosed: for an *incomplete*
generation the verifier reports `accepted_pool` (serialized `null`) as
"carrying an accepted pool". The outcome (generation incomplete → block)
is unchanged; the wording is noted for a future fix.

## Reproduction

```sh
cargo run -p mutagen-exp-0011 -- self-test        # network-free; all pass
cargo run -p mutagen-exp-0011 -- preflight
cargo run -p mutagen-exp-0011 -- verify-run-manifest
cargo run -p mutagen-exp-0011 -- verify-discovery  # OK
cargo run -p mutagen-exp-0011 -- verify-mutation   # 3 violations (incomplete generation)
```

Raw evidence: `experiments/0011-confirmatory-self-evolution/{run-manifest.json,
discovery-raw.jsonl, discovery-summary.json, mutation-input.json,
mutation-generation.json, candidate-pool.json}` (all committed).

## What a clean confirmatory rerun (future experiment) must change

1. State the **exact output schema** in the frozen mutator prompt
   (`{"candidates":[{"suffix","rationale"}]}` with an example).
2. Make machine-generated retry errors **shape-informative** (show the
   expected object schema, not the raw serde error).
3. Keep everything else (freeze, run identity, gates, verifiers, splits,
   0009 stress, temperatures, limits) exactly as 0011 registered it.

**Explicitly out of scope per the 0011 spec: no Experiment 0012 is
designed or started here.**
