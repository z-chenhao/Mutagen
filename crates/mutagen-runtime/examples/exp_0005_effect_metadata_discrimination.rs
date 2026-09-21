//! Experiment 0005 — Effect Metadata Discrimination.
//!
//! Research question:
//!   Can resource-scoped read/write effect metadata distinguish causally
//!   unsafe historical-outcome reuse from safe trajectory divergence more
//!   accurately than call coverage alone or coarse mutating/non-mutating
//!   information in a deterministic stateful fixture?
//!
//! Secondary formulation:
//!   Does knowing which resources an interaction reads or writes provide
//!   useful replay-safety information beyond knowing only that an
//!   interaction exists or that some prior interaction mutates state?
//!
//! Environment (deterministic, in-memory, domain-neutral, standard
//! library only; no network, filesystem, randomness, wall clock, async,
//! or LLM):
//!   resources:  x, y
//!   pre-state:  x = EMPTY, y = B
//!   set_x_to_a : x = A          (a write of x)
//!   read_x     : returns x      (a read of x; EMPTY if x unset)
//!   read_y     : returns y      (a read of y)
//!
//! The important property:
//!   write(x) affects read(x); write(x) does NOT affect read(y).
//!
//! Three experiment-local guard strategies, each returning
//! AllowHistoricalReuse / RejectHistoricalReuse. Each guard decides
//! using ONLY: historical trajectory, candidate trajectory, call
//! identity, effect metadata, and relative ordering. The guards MUST NOT
//! (and structurally cannot) inspect: reference execution results,
//! candidate runtime outcomes, historical-vs-reference comparisons, or
//! environment internal state. The reference execution is evaluated
//! separately, afterward:
//!
//!                  ┌── guard decision
//! metadata/history ┤
//!                  └── candidate reference execution
//!                           ↓
//!                    compare decision with truth
//!
//!   G0 — coverage only: allow if the evaluated candidate call has a
//!        matching historical call identity; ignores all effects.
//!   G1 — coarse mutation-aware: knows only read-only / writes-state.
//!        Rejects if any state-mutating historical interaction before
//!        the matched historical read is missing from the candidate
//!        before the candidate read. Deliberately does not know which
//!        resource is mutated.
//!   G2 — resource-scoped: like G1, but only for prior historical
//!        writes whose writeset overlaps the read's readset
//!        (write.writes ∩ read.reads != empty).
//!
//! Independent reference truth: for each fixture the candidate
//! trajectory is executed from a fresh copy of the exact historical
//! pre-state; the candidate read's actual outcome is compared with the
//! recorded historical outcome of the identity-matched read.
//!
//!   historical outcome reusable in this fixture
//!     iff
//!   reference candidate read outcome == historical recorded read outcome
//!
//! This definition is valid only for this fully modeled deterministic
//! fixture. It is not a universal evaluator rule.
//!
//! Metadata assumption: effect metadata is manually declared and assumed
//! correct for this experiment. This experiment does NOT test how
//! metadata is inferred, whether tool authors can be trusted, whether
//! metadata can be stale or incomplete, or whether runtime verification
//! is needed.
//!
//! Fixtures:
//!   C0 — exact historical trajectory (baseline control)
//!   D1 — dependent write omitted (unsafe reuse)
//!   D2 — dependent write reordered (unsafe reuse)
//!   U1 — unrelated write omitted (safe divergence; essential control
//!        distinguishing "some mutation" from "the mutation touched
//!        what this read depends on")
//!
//! Every type and function in this file is private to the experiment:
//! no production type, API, or schema is introduced; no dependency is
//! added.

/// One domain-neutral structural interaction with the two-resource state
/// machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call {
    SetXToA,
    ReadX,
    ReadY,
}

impl Call {
    fn describe(&self) -> &'static str {
        match self {
            Call::SetXToA => "set_x_to_a",
            Call::ReadX => "read_x",
            Call::ReadY => "read_y",
        }
    }
}

/// One observed outcome of an interaction.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    Ack,
    Value(&'static str),
}

impl Outcome {
    fn describe(&self) -> String {
        match self {
            Outcome::Ack => "Ack".to_string(),
            Outcome::Value(value) => format!("Value(\"{value}\")"),
        }
    }
}

/// Experiment-local effect metadata, manually declared and assumed
/// correct for this experiment. Private throwaway representation; it must
/// not be promoted to a production type, shared abstraction, or generic
/// effect trait.
#[derive(Clone, Copy, Debug)]
struct Effects {
    reads: &'static [&'static str],
    writes: &'static [&'static str],
}

fn effects_of(call: Call) -> Effects {
    match call {
        // set x = A
        Call::SetXToA => Effects {
            reads: &[],
            writes: &["x"],
        },
        Call::ReadX => Effects {
            reads: &["x"],
            writes: &[],
        },
        Call::ReadY => Effects {
            reads: &["y"],
            writes: &[],
        },
    }
}

/// Coarse projection consumed by G1: does this interaction mutate any
/// state? G1 deliberately does NOT see which resource is read or
/// written — that missing resource identity is part of the experiment.
fn mutates_state(call: Call) -> bool {
    !effects_of(call).writes.is_empty()
}

/// G2 dependency predicate, local and transparent:
///
///   depends_on(write, read)  iff  write.writes ∩ read.reads != empty
///
/// No transitive dependencies, no graph, no production abstraction.
fn overlaps(producer: Call, consumer: Call) -> bool {
    effects_of(producer)
        .writes
        .iter()
        .any(|resource| effects_of(consumer).reads.contains(resource))
}

/// Guard verdict. Experiment-only; not a production policy type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GuardDecision {
    AllowHistoricalReuse,
    RejectHistoricalReuse,
}

impl GuardDecision {
    fn text(&self) -> &'static str {
        match self {
            GuardDecision::AllowHistoricalReuse => "ALLOW",
            GuardDecision::RejectHistoricalReuse => "REJECT",
        }
    }
}

/// G0 — Coverage only.
///
/// Allow reuse if the evaluated candidate call has a matching
/// historical call identity. Ignores all effects, ordering, and
/// outcomes. This is the intuition falsified by Experiment 0004.
///
/// Anti-circularity: the only inputs are the historical and candidate
/// trajectories and call identity. It never receives the reference
/// execution result.
fn guard_g0_coverage_only(history: &[Call], candidate_read: Call) -> GuardDecision {
    if history.contains(&candidate_read) {
        GuardDecision::AllowHistoricalReuse
    } else {
        GuardDecision::RejectHistoricalReuse
    }
}

/// G1 — Coarse mutation-aware.
///
/// Knows only read-only / writes-state (via `mutates_state`), never
/// resource identity. For the evaluated historical read: identify every
/// state-mutating historical interaction before that read; if any of
/// them is absent from the candidate before the candidate read, reject;
/// otherwise allow.
///
/// This is deliberately conservative and deliberately not optimized: its
/// false rejections on unrelated mutations are part of the experiment.
///
/// Anti-circularity: inputs are the historical and candidate trajectories
/// and the coarse projection of effect metadata only. It never receives
/// the reference execution result or any outcome value.
fn guard_g1_coarse(
    history: &[Call],
    candidate: &[Call],
    candidate_read_index: usize,
) -> GuardDecision {
    let candidate_read = candidate[candidate_read_index];
    let historical_read_index = history
        .iter()
        .position(|recorded| *recorded == candidate_read)
        .expect("fixture guarantee: the evaluated read exists in the history");
    for prior in &history[..historical_read_index] {
        if mutates_state(*prior) && !candidate[..candidate_read_index].contains(prior) {
            return GuardDecision::RejectHistoricalReuse;
        }
    }
    GuardDecision::AllowHistoricalReuse
}

/// G2 — Resource-scoped effects.
///
/// Uses reads/writes resource sets. For the evaluated candidate read:
/// match the historical read by exact call identity; inspect historical
/// interactions before it; consider only prior historical writes whose
/// writeset overlaps the read's readset (`overlaps`); each such
/// overlapping write must still appear, by exact call identity, before
/// the candidate read; if a required overlapping write is absent or
/// appears after the candidate read, reject; otherwise allow.
///
/// No transitive dependencies, no graph, no outcome values.
///
/// Anti-circularity: inputs are the historical and candidate trajectories
/// and the resource-scoped effect metadata only. It never receives the
/// reference execution result or any outcome value.
fn guard_g2_resource_scoped(
    history: &[Call],
    candidate: &[Call],
    candidate_read_index: usize,
) -> GuardDecision {
    let candidate_read = candidate[candidate_read_index];
    let historical_read_index = history
        .iter()
        .position(|recorded| *recorded == candidate_read)
        .expect("fixture guarantee: the evaluated read exists in the history");
    for prior in &history[..historical_read_index] {
        if overlaps(*prior, candidate_read) && !candidate[..candidate_read_index].contains(prior) {
            return GuardDecision::RejectHistoricalReuse;
        }
    }
    GuardDecision::AllowHistoricalReuse
}

/// One recorded historical interaction and its observed outcome, in
/// trajectory order.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RecordedInteraction {
    call: Call,
    outcome: Outcome,
}

/// The tiny deterministic two-resource state machine, private to this
/// experiment. Fully modeled locally:
///
///   set_x_to_a : x = A; returns Ack
///   read_x     : returns Value(x), or Value("EMPTY") if x is unset
///   read_y     : returns Value(y), or Value("EMPTY") if y is unset
///
/// Not a generic environment trait and not a production abstraction.
struct Environment {
    x: Option<&'static str>,
    y: Option<&'static str>,
}

impl Environment {
    /// The historical pre-state: `x = EMPTY, y = B`. Every execution in
    /// this experiment (historical and reference) starts from a fresh
    /// copy of exactly this state.
    fn historical_pre_state() -> Self {
        Self {
            x: None,
            y: Some("B"),
        }
    }

    fn execute(&mut self, call: Call) -> Outcome {
        match call {
            Call::SetXToA => {
                self.x = Some("A");
                Outcome::Ack
            }
            Call::ReadX => Outcome::Value(self.x.unwrap_or("EMPTY")),
            Call::ReadY => Outcome::Value(self.y.unwrap_or("EMPTY")),
        }
    }
}

/// Independent reference execution — evaluated separately from the
/// guards, afterward.
///
/// Executes the candidate trajectory against a fresh copy of the exact
/// historical pre-state using the deterministic state-machine semantics.
/// Usable as truth HERE only because the initial state is fully known,
/// the semantics are deterministic, and all effects are fully modeled
/// locally. It is not a universal evaluator rule.
fn reference_execution(candidate: &[Call]) -> Vec<Outcome> {
    let mut env = Environment::historical_pre_state();
    candidate.iter().map(|call| env.execute(*call)).collect()
}

/// Record a metric line, e.g. `C0 PASS`.
fn report(metric: &str, passed: bool) {
    println!("{metric} {}", if passed { "PASS" } else { "FAIL" });
}

fn print_decision_row(
    id: &str,
    valid: bool,
    g0: GuardDecision,
    g1: GuardDecision,
    g2: GuardDecision,
) {
    println!(
        "{id:<8} | {:<18} | {:<6} | {:<6} | {}",
        if valid { "valid" } else { "invalid" },
        g0.text(),
        g1.text(),
        g2.text(),
    );
}

/// One fixture: a scripted history, a candidate, and the index of the
/// evaluated candidate read.
struct Fixture {
    id: &'static str,
    history: Vec<Call>,
    candidate: Vec<Call>,
    candidate_read_index: usize,
}

/// Run one fixture end-to-end: record the history, decide with the three
/// guards (independently of any reference execution), then evaluate the
/// independent reference execution and derive the metric.
fn run_fixture(fixture: &Fixture) -> (bool, GuardDecision, GuardDecision, GuardDecision) {
    // 1. History is recorded by executing the environment; no outcome is
    //    manually constructed.
    let mut env = Environment::historical_pre_state();
    let record: Vec<RecordedInteraction> = fixture
        .history
        .iter()
        .map(|call| {
            let outcome = env.execute(*call);
            RecordedInteraction {
                call: *call,
                outcome,
            }
        })
        .collect();

    let candidate_read = fixture.candidate[fixture.candidate_read_index];
    let historical_read_index = record
        .iter()
        .position(|recorded| recorded.call == candidate_read)
        .expect("fixture guarantee: the evaluated read exists in the history");
    let historical_read_outcome = &record[historical_read_index].outcome;

    // 2. Guard decisions. Structurally independent of the reference:
    //    their only inputs are the trajectories, call identity, effect
    //    metadata, and relative ordering.
    let g0 = guard_g0_coverage_only(&fixture.history, candidate_read);
    let g1 = guard_g1_coarse(
        &fixture.history,
        &fixture.candidate,
        fixture.candidate_read_index,
    );
    let g2 = guard_g2_resource_scoped(
        &fixture.history,
        &fixture.candidate,
        fixture.candidate_read_index,
    );

    // 3. Independent reference truth, evaluated afterward.
    let reference = reference_execution(&fixture.candidate);
    let reference_read_outcome = &reference[fixture.candidate_read_index];

    let valid = reference_read_outcome == historical_read_outcome;
    (valid, g0, g1, g2)
}

fn main() {
    println!("Experiment 0005 — Effect Metadata Discrimination");
    println!("pre-state: x = EMPTY, y = B");

    let fixtures = [
        // C0 — exact historical trajectory (baseline control).
        Fixture {
            id: "C0",
            history: vec![Call::SetXToA, Call::ReadX],
            candidate: vec![Call::SetXToA, Call::ReadX],
            candidate_read_index: 1,
        },
        // D1 — dependent write omitted (causally unsafe reuse).
        Fixture {
            id: "D1",
            history: vec![Call::SetXToA, Call::ReadX],
            candidate: vec![Call::ReadX],
            candidate_read_index: 0,
        },
        // D2 — dependent write reordered after its dependent read
        // (causally unsafe reuse).
        Fixture {
            id: "D2",
            history: vec![Call::SetXToA, Call::ReadX],
            candidate: vec![Call::ReadX, Call::SetXToA],
            candidate_read_index: 0,
        },
        // U1 — unrelated write omitted (safe divergence; the essential
        // control separating "some mutation" from "a mutation that
        // touched what this read depends on").
        Fixture {
            id: "U1",
            history: vec![Call::SetXToA, Call::ReadY],
            candidate: vec![Call::ReadY],
            candidate_read_index: 0,
        },
    ];

    // Diagnostic: each fixture's recorded history.
    for fixture in &fixtures {
        let mut env = Environment::historical_pre_state();
        let parts: Vec<String> = fixture
            .history
            .iter()
            .map(|call| format!("{} -> {}", call.describe(), env.execute(*call).describe()))
            .collect();
        println!("  {}: history {}", fixture.id, parts.join(", "));
    }

    // Core result: fixture | reference validity | G0 | G1 | G2
    println!();
    println!(
        "{:<8} | {:<18} | {:<6} | {:<6} | G2",
        "fixture", "reference validity", "G0", "G1",
    );
    let mut rows = Vec::new();
    for fixture in &fixtures {
        let (valid, g0, g1, g2) = run_fixture(fixture);
        print_decision_row(fixture.id, valid, g0, g1, g2);
        rows.push((fixture.id, valid, g0, g1, g2));
    }

    println!();

    let allow = GuardDecision::AllowHistoricalReuse;
    let reject = GuardDecision::RejectHistoricalReuse;

    let c0 = rows[0];
    let d1 = rows[1];
    let d2 = rows[2];
    let u1 = rows[3];

    // Metrics, each derived from the actual guard decisions combined
    // with the actual reference execution result — never hardcoded.
    // (The guards never saw the reference; only the metric combines
    // them.)
    let c0_pass = c0.1 && c0.2 == allow && c0.3 == allow && c0.4 == allow;
    let d1_pass = !d1.1 && d1.2 == allow && d1.3 == reject && d1.4 == reject;
    let d2_pass = !d2.1 && d2.2 == allow && d2.3 == reject && d2.4 == reject;
    let u1_pass = u1.1 && u1.2 == allow && u1.3 == reject && u1.4 == allow;

    report("C0", c0_pass);
    report("D1", d1_pass);
    report("D2", d2_pass);
    report("U1", u1_pass);

    // Conclusion: if every metric passes, the hypothesis is supported
    // (scoped to these fixtures). If the baseline control C0 fails, the
    // run is inconclusive. If a control holds but a hypothesized
    // discrimination does not appear, the hypothesis is refuted.
    let conclusion = if c0_pass && d1_pass && d2_pass && u1_pass {
        "supported"
    } else if !c0_pass {
        "inconclusive"
    } else {
        "refuted"
    };
    println!("Conclusion: {conclusion}");

    // Hard guarantees so a broken run is never silently green.
    assert!(
        c0_pass,
        "C0: exact trajectory must be valid and allowed by all guards"
    );
    assert!(
        d1_pass,
        "D1: omitted dependent write must be invalid; G0 allows, G1/G2 reject"
    );
    assert!(
        d2_pass,
        "D2: reordered dependent write must be invalid; G0 allows, G1/G2 reject"
    );
    assert!(
        u1_pass,
        "U1: omitted unrelated write must be valid; G0/G2 allow, G1 rejects"
    );
}
