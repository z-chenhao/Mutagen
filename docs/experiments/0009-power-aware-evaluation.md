# Experiment 0009: Power-Aware, Candidate-Blind, Family-Level Transfer-Validated Improvement Detection

**Status:** in progress. The pre-registered design below was committed
before any model request (calibration code-under-test commit, recorded
in §Freeze Protocol). Results sections are filled after execution.

**Crate:** `experiments/0009-power-aware-evaluation/` (standalone
workspace; not part of the production workspace; no production API).

**Model:** local OpenAI-compatible endpoint, `incoai/Qwen3.8-27B-Splash`
(redacted endpoint `http://127.0.0.1:8000/v1`), temperature 0.2,
`tool_choice = auto`, non-streaming, no fallback model, no API key
(same interface as Experiments 0006–0008).

---

## Prior Evidence

- **0006** validated a real-model Rust-native kernel against
  `incoai/Qwen3.8-27B-Splash`.
- **0007** validated the objective final-state evaluator in the
  regression direction (19 informative pairs, p ≈ 3.8e-6,
  `quality_regression`), but its improvement arm produced only
  **1 informative pair out of 24** — the held-out baseline sat on the
  ceiling, so the improvement direction was undetectable.
- **0008** fixed the direction problem structurally: baseline-only
  difficulty calibration → frozen manifest → held-out
  baseline-vs-repair comparison, with explicit headroom and
  informative-pair gates. It ran, but concluded **`inconclusive`**:
  the calibrated difficulty did not *transfer* to the unseen held-out
  variants (held-out baseline ≈ 91.7% success — the ceiling 0007
  could not name), so both gates failed.

## Why 0008 Was Insufficient

0008's failure was a **design-information** failure, not an execution
failure. Three specific causes, all correctable:

1. **One development task per family.** A single-task success estimate
   is 1 of {0..5}/5 — a 20%-granularity random variable. 0008's
   headroom band (2/5–3/5) accepted stresses where the *single* task
   happened to land at 2/5; the same family's unseen held-out variant
   sat at ~92%. One sample cannot estimate a family's difficulty
   distribution.
2. **Symmetric headroom band (2/5–3/5, "minimize |rate−0.5|").**
   This optimizes for *interesting-looking* calibration, not for the
   downstream detection goal. What Phase B needs is **baseline
   failure headroom on held-out tasks**: a pair is only informative
   when the baseline fails. A 2/5 calibration rate gives only 60%
   baseline-failure probability per pair — and *before* the transfer
   penalty — which is the wrong currency.
3. **No backward power design.** 0008 pre-registered "≥ 24 valid and
   ≥ 12 informative pairs" as intuition. It never asked: *given the
   information a calibrated family actually contains, will the
   pre-registered evaluation budget detect a real robustness
   improvement at 85% power?* The budget scaled with the calibrated
   family count (24 or 36 pairs), so the detection guarantee was never
   pinned.

0009 redesigns the calibration contract around the detection goal.

## Research Question

> Can a candidate-blind, family-level difficulty-calibration procedure
> — designed backward from a pre-registered informative-pair and
> statistical-power requirement, using two development tasks per
> family — produce enough held-out information for Mutagen's objective
> evaluator to detect a controlled robustness improvement on a real
> stochastic model?

## Hypothesis (pre-registered)

For each selected family, the lowest stress at which **both**
registered development tasks show baseline success ≤ 40% (≤ 2 of 5)
transfers to that family's unseen held-out tasks such that:

1. the held-out baseline fails often enough that the pre-registered
   fixed evaluation budget (48 paired comparisons) yields
   ≥ 12 informative pairs with high probability; and
2. the repair candidate (which re-verifies and re-writes after
   observed write drops) converts baseline failures into wins, so the
   exact paired sign test classifies `quality_improvement`
   (p < 0.05 two-sided) with conditional detection power ≥ 0.85.

## Design Changes vs 0008

| Aspect | 0008 | 0009 |
|---|---|---|
| Development tasks per family | 1 | **2** |
| Phase A episodes | 75 | **150** (3 fam × 2 dev × 5 stress × 5 rep) |
| Sanity gate | single task S0 ≥ 4/5 | **both** dev tasks S0 ≥ 4/5, else family `calibration_invalid` |
| Eligibility | 2/5–3/5 band, minimize \|rate−0.5\| | **both** dev tasks ≤ 2/5 (≤ 40%) at the same stress |
| Selection | middle of band | **lowest** eligible stress (minimum intervention) |
| Lower bound | 2/5 (no ceiling tolerance) | **none** — 0/5 eligible (a family whose dev tasks always fail under stress is maximally informative) |
| Phase B budget | scaled (24 or 36 pairs) | **fixed 48 pairs / 96 episodes** (12 or 8 reps per held-out task) |
| Power | not designed | **Power(n, θ) ≥ 0.85 at n = 12 informative pairs, θ = 0.90** (computed value ≈ 0.889) |
| Information gate | headroom band only | **potential information capacity**: baseline failures among valid pairs ≥ 12 (candidate-blind by construction) |
| Conclusion | supported/refuted/inconclusive | same three-way, with the information gate replaced by capacity + informative gates |
| Calibration API | candidate-blind | candidate-blind (unchanged invariant) |

Everything else is carried over unchanged: two-tool kernel
(`state_read`/`state_write`), `max_model_turns = 12`,
`max_tool_calls = 16`, repeated silent-drop ladder S0–S4, objective
oracle, cyclic stress order, alternating condition order, exact
two-sided sign test at α = 0.05 (no external statistics crate).

## Task Registry (frozen in `tasks.json`)

12 tasks, 3 families, 6 development + 6 held-out:

| Family | Development | Held-out | Pattern |
|---|---|---|---|
| `direct_set` | D1: set `x` to `K`; D2: set `y` to `L` | H1: set `x` to `R`; H2: set `y` to `N` | unconditional write |
| `conditional_set` | D3: if `x` EMPTY → `K`; D4: if `y` LOW → `HIGH` | H3: if `x` COLD → `WARM`; H4: if `y` OFF → `ON` | conditional write |
| `replacement` | D5: `x` A→B; D6: `y` C→D | H5: `x` M→N; H6: `y` P→Q | value replacement |

Each task's registered `initial_state` (with the fault key's
`VOID`/sentinel value) and `target_state` are exact, small, and
differ per task (different keys, values, and trigger conditions).
All 12 are committed before Phase A; none is authored afterwards.

## Stress Ladder

Identical to 0008: S0 = `Reliable`; S1–S4 = `DropFirstNWrites { key,
count = 1..4 }` on the task's registered fault key. First `count`
target-key writes return the identical model-visible success payload
without mutating; later writes apply. Invisible to the model; fully
audited experiment-side.

## Repair Candidate

`repair` = baseline prompt + exactly the registered 5-attempt
re-read/re-write line (identical to 0008's mutation, verified
`repair == baseline + registered mutation` before any model call):

> After every successful state_write, read the same key. If the
> observed value differs from the value you intended to write, write it
> again and re-read it. Repeat until the value matches, but make at most
> five state_write attempts for that requested key.

Worst-case tool-call budget at S4: 4 (dropped writes) + 1 (applying
write) + 5 (re-reads) = 10 ≤ 16. Kernel limits unchanged.

## Power Design (backward from the goal)

- `PLANNED_PAIRS = 48` (fixed; 2 families → 4 tasks × 12 reps,
  3 families → 6 tasks × 8 reps)
- `MIN_VALID_PAIRS = 44` (≥ 91.7% of pairs must be infra-clean)
- `MIN_INFORMATIVE_PAIRS = 12`
- `DESIGN_DISCORDANT_WIN_PROBABILITY θ = 0.90` (assumption: on an
  informative pair — baseline failed — a genuinely repairing candidate
  wins it 90% of the time)
- `MIN_CONDITIONAL_DETECTION_POWER = 0.85`

For n informative pairs, the exact two-sided sign test (α = 0.05)
rejects iff wins w satisfies w > n − w and p(w, n − w) ≤ 0.05.
Conditional power:

```
Power(n, θ) = Σ_{w: w>n−w, p(w,n−w)≤0.05} BinomPMF(w; n, θ)
```

At the gate minimum n = 12, Power(12, 0.90) ≈ **0.8891** ≥ 0.85
(checked at build time; the summary re-asserts it). If the
realized informative count falls short of 12, power falls with it —
hence the informative gate is a *gate*, not a target.

## Information Gates (pre-registered)

| Gate | Threshold | Meaning |
|---|---|---|
| Phase A artifact complete | 150/150 records, full design | calibration ran as registered |
| Calibrated families | ≥ 2 of 3 | else `inconclusive`, stop (no Phase B) |
| Phase B artifact complete | 96/96 records | evaluation ran as registered |
| Infrastructure | ≤ 10% of 96 episodes | else `inconclusive` |
| Valid pairs | ≥ 44 of 48 | else `inconclusive` |
| **Potential information capacity** | baseline failures among valid pairs ≥ 12 | else `inconclusive` (candidate-blind: known before any repair outcome) |
| **Actual informative pairs** | wins + losses ≥ 12 | else `inconclusive` |

**Conclusion rule (frozen):**

- `inconclusive` — any gate fails;
- `supported` — all gates pass **and** classification =
  `quality_improvement` (p < 0.05, wins > losses);
- `refuted` — all gates pass but classification ≠
  `quality_improvement`.

The *capacity* gate is the new instrument: it measures, on the
held-out baseline alone, whether the experiment will contain enough
decision-relevant pairs — the quantity 0008 only guessed at.

## Anti-Leakage (candidate-blindness)

`select_family_stress` accepts only family id + baseline-only
development outcomes (task, stress, repetition, oracle success). No
argument exists for: condition, candidate identity, repair outcomes,
repair prompt, held-out outcomes, cost, or trajectory data. The
`evaluation-plan.json` manifest contains per-family per-task per-stress
baseline counts, the selection decision, power constants, gate
thresholds, the 48-pair budget, and hashes (baseline **and** repair
prompt, task registry, calibration raw, code commit) — and is
verified to contain no repair or held-out outcome data. Phase B
records additionally carry `calibration_raw_sha256` and
`evaluation_plan_sha256` so the freeze is re-auditable.

## Freeze Protocol

1. Commit the full experiment crate + this document (all design
   inputs) → **calibration code-under-test commit** `C`.
2. Phase A runs with `--code-commit C`; all 150 records carry `C`.
3. Verify Phase A artifacts (design, cyclic order, audit, oracle,
   usage, summary, plan, power).
4. **Freeze:** commit Phase A artifacts + `evaluation-plan.json` as
   one commit; verify the diff contains no source changes (artifacts
   only). `C` is final: Phase B re-verifies the prompt/task hashes
   against the files at `C` and refuses to start on any mismatch.
5. Phase B runs (96 episodes); verify Phase B artifacts; compute
   gates; record conclusion.

Raw artifacts are append-only and SHA-256 recorded; they are never
rewritten (a failed gate is recorded, not retried).

## Execution Protocol

```sh
cargo run --release --manifest-path experiments/0009-power-aware-evaluation/Cargo.toml -- calibrate \
  --trajectories docs/experiments/artifacts/0009-calibration-trajectories.jsonl \
  --summary docs/experiments/artifacts/0009-calibration-summary.json \
  --plan experiments/0009-power-aware-evaluation/evaluation-plan.json \
  --code-commit <C>
MUTAGEN_EXP_BASE_URL=... MUTAGEN_EXP_MODEL=... <same>   # (env vars)

cargo run --manifest-path ... -- verify-calibration ...
# freeze commit, then:
cargo run --release --manifest-path ... -- evaluate \
  --plan ... --trajectories ...0009-evaluation-trajectories.jsonl \
  --summary ...0009-evaluation-summary.json --code-commit <C>
cargo run --manifest-path ... -- verify-evaluation ...
```

`MUTAGEN_EXP_BASE_URL = http://127.0.0.1:8000/v1`,
`MUTAGEN_EXP_MODEL = incoai/Qwen3.8-27B-Splash` (local, no API key).

## Results

*(pending execution — see git history of this document)*
