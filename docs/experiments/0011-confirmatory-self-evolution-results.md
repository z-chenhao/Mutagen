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

1. **The mechanical source-freeze mechanism operated correctly through
   all live stages reached by 0011-r1 (A1/B/C), and its registered tamper
   cases pass in network-free tests.** The run completed its live stages
   with zero freeze violations; the guard ran before every live stage
   executed; the pre-registered failure modes (mid-run commit,
   modify-commit-revert, manifest tamper) are all caught in `self-test`
   against real temporary git repositories. Selection and promotion stage
   guards are covered by implementation and tests, but were **not
   exercised by live execution** in 0011-r1 (D/E never ran). Scoped
   statement: within the registered 0011 live-stage command path,
   frozen-design source drift is mechanically detected *before* the next
   stage is permitted to issue a model request; this mechanism prevents
   *continuation of the registered run* after frozen-path drift — it does
   not imply that arbitrary external processes or ad-hoc requests to the
   same model endpoint are impossible (see
   *Unregistered endpoint smoke request* below).
2. **The gate machinery behaved as registered through the stages
   reached.** Discovery gates, verifier, and whitelist checks all
   behaved as registered, including correctly *blocking* the next stage
   on a failed generation — a **positive control result: the protocol
   failed closed instead of accepting malformed candidates**.
3. **A concrete, reproducible defect in the 0010/0011 mutator output
   contract** (schema adherence; §6 below) is identified with full raw
   evidence.

## Run identity (all re-verifiable from committed artifacts)

| Field | Value |
|---|---|
| Run | `0011-r1` |
| Code-under-test commit (A0) | `1b2cc38edb3a85ebfdb9ae66c138ba0b078e34e9` |
| Frozen design SHA-256 (combined) | `db494301b0d902b500b9b89e3d38779ca8f4c69a22a771e5e7d45601764a1db0` |
| Run manifest SHA-256 | `3cff11c2f28d01d80fade5f8b9e175f64621cfe260e344a2308e681545fc769c` |
| Frozen files | 20 (all crate sources, Cargo.toml/lock, design.json, tasks.json, stress-profile.json, both prompts, pre-registration doc) |
| Model | `incoai/Qwen3.8-27B-Splash` @ `http://127.0.0.1:8000/v1` (no auth), agent temp 0.2, mutator temp 0.7 |
| Integrity at run end | `verify-run-manifest` **PASS** (20/20 frozen files intact, clean history; still passes on the final head) |

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

Formal Stage-C stop semantics, preserved from the artifact
(`mutation-generation.json`):

- `attempt_count = 2` (the registered maximum)
- `mutation_generation_complete = false`
- accepted candidate pool: **none** (`candidate-pool.json` holds an empty pool)
- Stage D (selection): **not run** — blocked by the gate
- Stage E (promotion): **not run** — blocked by the gate

The verifier then reported its 3 registered violations (incomplete
generation carrying a null accepted pool; empty pool ≠ 4 candidates) and
the gate **blocked selection** — the machinery behaved exactly as
pre-registered. This is a **positive control result: the protocol failed
closed instead of accepting malformed candidates**. The
generated-but-rejected content is preserved verbatim in
`mutation-generation.json` (attempts 1–2 raw responses) and in
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

**Defect class:** the mutator output contract was **underspecified** —
the prompt promises a schema it does not define. **This is a design
defect to fix in a *future* experiment** (explicit schema in the frozen
mutator prompt and shape-informative machine-generated retry errors). It
must not be "fixed" inside 0011: the prompt and validator are frozen
design files, and any change would be the exact failure mode this
experiment was built to eliminate. This root cause is **not** evidence
that the model cannot evolve, that the mutation idea is bad, or that H1
is refuted: attempt 2 shows the model producing sound policy content that
failed only on container shape.

### Independent-rediscovery note (anti-seeding contract)

Attempt 2's rejected content — *"After any write, read back the same key
before treating the change as successful…"* — is **semantically similar
to 0010's selected C1** (read-after-write verification). This is recorded
as an **independent-rediscovery diagnostic** on the strength of: fresh
discovery tasks (V11_*), no C1 seeding, no known-repair seeding, no
selection/promotion leakage into the mutation input, and a neutral
mutator prompt — nothing from 0010 is copied into this crate.

**Limitation:** the candidate was **rejected by schema validation**. It is
therefore **not an accepted candidate**, was **not selected**, and was
**not promoted**. It is a diagnostic of the model's convergence on this
policy class under the frozen stress — *not* formal evolutionary
evidence, and it is not upgraded to any formal claim.

## Stage D/E — Not run

Selection and promotion did not execute (gate-blocked). No selection,
promotion, or confirmatory statistics exist for 0011. Accordingly, the
selection/promotion stage *guards* of the control apparatus are covered
by implementation and tests only — they were not exercised by live
execution in 0011-r1.

## Unregistered endpoint smoke request (disclosed; execution-channel, not source-freeze)

**Timeline:** after Stage A1 (run-manifest freeze) and **before**
registered Stage B, one ad-hoc live request was sent to the model
endpoint (`run --task E11A`, the `mutagen-exp-0011` ad-hoc smoke
command) **outside the 0011-r1 stage-command path**, as an endpoint
liveness check before the run.

It did **not**:

- use the registered E11/S11/P11 stage execution (it is not part of the
  registered episode schedule),
- alter any frozen source file,
- enter the discovery artifacts, the mutation input, or any formal
  episode count (the Stage B records were produced fresh afterwards),

It is recorded here with the precise characterization:

- **Not a source-freeze violation.** The frozen design remained intact
  before, during, and after the request (`verify-run-manifest` PASS at
  run end; 20/20 files intact). The source-freeze guard constrains
  *frozen-design immutability*; an external request to the serving
  endpoint does not touch that property.
- **An execution-channel-discipline gap.** The guard controls the
  *registered* 0011 stage-command path; it does not (and cannot)
  constrain arbitrary external processes or ad-hoc clients talking to
  the same serving endpoint. This smoke request demonstrates exactly
  that boundary.
- **No effect on the formal verdict.** The run is already INCONCLUSIVE
  for the Stage C generation failure; this request changes nothing
  about that verdict, the discovery evidence, or the artifact binding.

**Future requirement:** confirmatory runs should use a dedicated
execution channel and prohibit unregistered live requests to that
endpoint after run initialization (see *Minimal next-run changes*, item
D).

## Hypotheses (formal results)

- **H1 (evolvability): NEITHER SUPPORTED NOR REFUTED.** Inconclusive at
  mutation generation; no selection or promotion data exists, so no
  confirmatory claim is made in either direction.
- **H2 (control apparatus): NOT FULLY TESTED / NOT FORMALLY SUPPORTED.**
  The pre-registered H2 was an **end-to-end** control hypothesis —
  discovery → mutation generation → selection → promotion, with all
  claims re-derivable under frozen control. 0011-r1 exercised only
  A0/A1/B/C and stopped before D/E, so the full H2 was **not confirmed**.

The strongest defensible claim, using the observed-vs-registered
distinction:

- **Control mechanisms exercised through Stage C** (frozen-design
  integrity, run identity, discovery gates, mutation validation failing
  closed, downstream stages blocked, verifiers, whitelist leak checks):
  **behaved as registered.**
- **Full end-to-end H2** (including selection- and promotion-stage
  control under live execution): **not confirmed** in 0011-r1.

## Honest 0010 comparison

| | 0010 (voided) | 0011-r1 |
|---|---|---|
| Protocol integrity | **voided** (mid-run commit) | intact through all live stages reached (A1/B/C; verified at each) |
| Discovery | 18 ep, C1-generation evidence | 18 ep, 14 incumbent failures, verified |
| Mutation generation | succeeded on attempt 1 (lucky schema adherence) | **failed schema on 2/2** → run terminated |
| Selection / promotion | ran (post-violation; 35W/0L/13T, p≈5.8e-11 — *exploratory only*) | not run |
| Verdict | INCONCLUSIVE (integrity) | **INCONCLUSIVE (generation)** |

The 0010 C1 result remains *exploratory* evidence only. 0011 does not
confirm it; it confirms that a clean confirmation is a *protocol*
problem first, and pinpoints the one contract (output schema) that must
be made explicit before a confirmatory rerun can be expected to clear
Stage C.

## Stage-B provenance limitation (disclosed)

**Timeline fact:** Stage B (live discovery) and Stage C (live mutation
generation) ran consecutively; their artifacts — Stage B's
`discovery-raw.jsonl` / `discovery-summary.json` / `mutation-input.json`
**and** Stage C's generation failure — were committed **together**
afterwards (`8401ba9`), not separately. 0011-r1 bound the Stage-C
generation artifact to `mutation_input_sha256`, and the final committed
evidence verifies that binding (verifier-checked). However, the Stage-B
mutation input was **not independently Git-committed before Stage C
began**.

Characterization:

- **Not a registered 0011 protocol violation.** The frozen A0
  pre-registration did not make "commit B before C" a formal gate; the
  mutation input was SHA-bound and the binding verifies.
- **But weaker provenance than desired.** A stronger stage-artifact
  freeze would require: B live → verify B → **commit B** → freeze the
  mutation-input hash → only *then* C live (and likewise C commit before
  D, D commit before E). 0011-r1 had SHA binding without the
  between-stage commit step.

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

## Minimal next-run changes (four; no more)

A. **Explicit mutation output schema.** The frozen mutator prompt must
   include the exact JSON shape, with no ambiguity:

   ```json
   {
     "candidates": [
       { "suffix": "string", "rationale": "string" },
       { "suffix": "string", "rationale": "string" },
       { "suffix": "string", "rationale": "string" },
       { "suffix": "string", "rationale": "string" }
     ]
   }
   ```

B. **Shape-informative retry errors.** If the response is invalid, the
   retry message states the expected object shape explicitly (still no
   performance or selection feedback).

C. **Stage-artifact freeze.** Require, for every live stage boundary:

   ```text
   B live → verify B → commit B → mutation-input hash frozen → only then C live
   C commit before D
   D commit before E
   ```

D. **Endpoint discipline.** After A1, no unregistered live request to
   the dedicated model endpoint until the run terminates (a dedicated
   execution channel for the run).

Everything else — source freeze, run identity, gates, verifiers, splits,
0009 stress, temperatures, limits — stays exactly as 0011 registered it.

**Evolvable surface: unchanged.** The next clean confirmation still
uses exactly the one registered surface — the **append-only prompt
suffix**. No tool mutation, routing mutation, source mutation, workflow
mutation, or memory mutation is recommended yet: first obtain one clean
end-to-end confirmation.

**Explicitly out of scope per the 0011 spec: the next experiment is not
designed or started here.**
