//! Experiment 0004 — Stateful Causality Under Replay.
//!
//! Research question:
//!   Is historical interaction coverage sufficient for faithful candidate
//!   replay when recorded interactions mutate state that influences later
//!   observations?
//!
//! Secondary formulation:
//!   Can a candidate request only historically known interactions and still
//!   receive causally invalid historical outcomes after omitting or
//!   reordering state-changing interactions?
//!
//! Historical baseline (deterministic, stateful, in-memory):
//!   pre-state:  slot = EMPTY
//!   write("slot", "A") -> Ack        (mutates: slot = "A")
//!   read("slot")       -> Value("A") (depends on the prior write)
//!   final state: slot = "A"
//!
//! Candidate trajectories:
//!   A — subset:  read("slot")               (omits the causal write)
//!   B — reorder: read("slot"), write("slot", "A")  (write after the read)
//!
//! Two experiment-only mechanisms, both defined locally in this file:
//!   historical-outcome replay:  resolves each candidate call by its
//!                     recorded interaction identity and returns the
//!                     recorded historical outcome; it deliberately does
//!                     NOT re-execute the environment, so it ignores
//!                     causal state evolution. This is the experimental
//!                     subject / negative mechanism, not a production
//!                     design.
//!   reference stateful execution: executes the candidate's trajectory
//!                     against a fresh copy of the exact historical
//!                     pre-state using the deterministic state-machine
//!                     semantics. Usable as a counterfactual reference
//!                     HERE only because the initial state is fully
//!                     known, the semantics are deterministic, and all
//!                     effects are fully modeled locally. It is not a
//!                     universal replay solution.
//!
//! The point under test: a recorded outcome is a function of
//! (previous state, previous effects, current call), not of the current
//! call alone. Therefore call-level historical coverage and causal
//! validity of the historical outcome are distinct, and in this stateful
//! fixture coverage alone is insufficient for faithful counterfactual
//! replay.
//!
//! The slot/state/write/read vocabulary is deliberately domain-neutral.
//! These are still synthetic deterministic fixtures; this is not evidence
//! of cross-domain generalization, and no production type, API, or schema
//! is introduced. Every type and function in this file is private to the
//! experiment; no generic environment trait, no shared replay
//! abstraction, and no local duplication with Experiments 0001-0003 is
//! required because nothing from them is reused.

/// One interaction with the stateful slot: write or read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call {
    Write {
        key: &'static str,
        value: &'static str,
    },
    Read {
        key: &'static str,
    },
}

impl Call {
    fn describe(&self) -> String {
        match self {
            Call::Write { key, value } => format!("write(\"{key}\", \"{value}\")"),
            Call::Read { key } => format!("read(\"{key}\")"),
        }
    }
}

/// One observed outcome of a stateful interaction.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    Ack,
    Value(&'static str),
}

impl Outcome {
    /// Plain-text form for diagnostics, e.g. `Ack` or `A`.
    fn text(&self) -> &'static str {
        match self {
            Outcome::Ack => "Ack",
            Outcome::Value(value) => value,
        }
    }

    fn describe(&self) -> String {
        match self {
            Outcome::Ack => "Ack".to_string(),
            Outcome::Value(value) => format!("Value(\"{value}\")"),
        }
    }
}

/// One recorded historical interaction and its observed outcome. The
/// record is an ordered `Vec` of these, so each interaction's trajectory
/// position is preserved as well as its identity and outcome.
///
/// Throwaway experimental scaffolding: a local record, not a production
/// replay schema.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RecordedInteraction {
    call: Call,
    outcome: Outcome,
}

/// The tiny deterministic stateful environment, private to this
/// experiment. One slot, two operations, fully modeled locally:
///
///   write("slot", v): slot = v; returns Ack
///   read("slot"):     returns Value(slot) if the slot holds a value,
///                     otherwise Value("EMPTY")
///
/// This is not a generic environment trait or a production abstraction;
/// it exists because the reference execution needs deterministic, locally
/// modeled semantics over the historical pre-state.
struct Environment {
    slot: Option<&'static str>,
}

impl Environment {
    /// The historical pre-state: `slot = EMPTY`. Every reference execution
    /// in this experiment starts from a fresh copy of exactly this state.
    fn historical_pre_state() -> Self {
        Self { slot: None }
    }

    fn state_text(&self) -> &'static str {
        self.slot.unwrap_or("EMPTY")
    }

    /// Execute one interaction under the deterministic state-machine
    /// semantics. The slot is the only key and the only slot.
    fn execute(&mut self, call: &Call) -> Outcome {
        match *call {
            Call::Write { key, value } => {
                assert_eq!(key, "slot", "the fixture defines only key \"slot\"");
                self.slot = Some(value);
                Outcome::Ack
            }
            Call::Read { key } => {
                assert_eq!(key, "slot", "the fixture defines only key \"slot\"");
                Outcome::Value(self.slot.unwrap_or("EMPTY"))
            }
        }
    }
}

/// Historical-outcome replay — the experimental subject (negative
/// mechanism).
///
/// For each candidate call, it finds the first not-yet-consumed recorded
/// interaction with the same call identity (write/read plus all
/// arguments) and returns the recorded historical outcome. It does NOT
/// re-execute the environment transition and deliberately ignores causal
/// state evolution: the outcome it returns is a function of the *current
/// call identity alone*, which is exactly the over-generalization under
/// test. `consumed` is owned by the driver, so the exact historical record
/// indices matched by each candidate are externally observable.
///
/// This is not a production replay design.
fn historical_outcome_replay(
    record: &[RecordedInteraction],
    consumed: &mut Vec<usize>,
    trajectory: &[Call],
) -> Vec<Outcome> {
    let mut outcomes = Vec::new();
    for call in trajectory {
        let index = record
            .iter()
            .enumerate()
            .find(|(index, recorded)| !consumed.contains(index) && recorded.call == *call)
            .map(|(index, _)| index);
        match index {
            Some(index) => {
                consumed.push(index);
                outcomes.push(record[index].outcome.clone());
            }
            None => panic!(
                "no historical evidence for {} — the fixture never produces this",
                call.describe()
            ),
        }
    }
    outcomes
}

/// Reference stateful execution — the counterfactual reference for THIS
/// experiment only.
///
/// Executes the candidate's actual trajectory against a fresh copy of the
/// exact historical pre-state using the deterministic state-machine
/// semantics, returning the observable outcomes. This is the ground truth
/// for the comparison because the initial state is fully known, the
/// semantics are deterministic, and all effects are fully modeled locally.
/// It is not a universal replay solution and is not promoted to production
/// architecture.
fn reference_execution(trajectory: &[Call]) -> (Vec<Outcome>, &'static str) {
    let mut env = Environment::historical_pre_state();
    let outcomes = trajectory
        .iter()
        .map(|call| env.execute(call))
        .collect::<Vec<_>>();
    (outcomes, env.state_text())
}

/// Record a metric line, e.g. `S3 PASS`.
fn report(metric: &str, passed: bool) {
    println!("{metric} {}", if passed { "PASS" } else { "FAIL" });
}

fn main() {
    println!("Experiment 0004 — Stateful Causality Under Replay");

    let write_a = Call::Write {
        key: "slot",
        value: "A",
    };
    let read = Call::Read { key: "slot" };

    // ------------------------------------------------------------------
    // H1 — Historical stateful execution.
    //
    // The environment is the source of outcomes: the historical trajectory
    // is executed from a fresh historical pre-state (slot = EMPTY) and its
    // interactions and outcomes are recorded. No historical outcome is
    // manually constructed.
    // ------------------------------------------------------------------
    let mut env = Environment::historical_pre_state();
    assert_eq!(
        env.state_text(),
        "EMPTY",
        "historical pre-state is slot = EMPTY"
    );
    let history_trajectory = [write_a, read];
    let mut record = Vec::new();
    for call in &history_trajectory {
        let outcome = env.execute(call);
        record.push(RecordedInteraction {
            call: *call,
            outcome: outcome.clone(),
        });
    }
    let history_outcomes = record
        .iter()
        .map(|recorded| recorded.outcome.clone())
        .collect::<Vec<_>>();
    let final_state = env.state_text();
    println!(
        "history: {} -> {}, {} -> {}; final state \"{}\"",
        write_a.describe(),
        history_outcomes[0].describe(),
        read.describe(),
        history_outcomes[1].describe(),
        final_state
    );

    // H1: expected outcomes, expected final state, and a record that
    // matches the interactions actually executed.
    let h1 = history_outcomes == vec![Outcome::Ack, Outcome::Value("A")]
        && final_state == "A"
        && record
            == vec![
                RecordedInteraction {
                    call: write_a,
                    outcome: Outcome::Ack,
                },
                RecordedInteraction {
                    call: read,
                    outcome: Outcome::Value("A"),
                },
            ];
    report("H1", h1);

    // ------------------------------------------------------------------
    // H2 — Baseline control: historical-outcome replay of the exact
    // historical trajectory.
    //
    // The negative mechanism must not be trivially broken for the exact
    // trajectory: it must reproduce the historical baseline's recorded
    // outcomes.
    // ------------------------------------------------------------------
    let mut consumed = Vec::new();
    let baseline_replayed = historical_outcome_replay(&record, &mut consumed, &history_trajectory);
    let h2 = baseline_replayed == history_outcomes && consumed == vec![0, 1];
    println!(
        "  baseline (historical-outcome replay): write -> {}, read -> {}; \
         matches history",
        baseline_replayed[0].describe(),
        baseline_replayed[1].describe()
    );
    report("H2", h2);

    // ------------------------------------------------------------------
    // Candidate A — omitted causal predecessor: read("slot") only.
    //
    // The sole call exists in the historical record, so call-level
    // historical coverage is complete. But the historical read's outcome
    // depended on the earlier write, which the candidate omits.
    // ------------------------------------------------------------------
    let candidate_a = [read];

    // Condition A1: historical-outcome replay.
    let mut consumed = Vec::new();
    let subset_replayed = historical_outcome_replay(&record, &mut consumed, &candidate_a);
    let s1 = consumed == vec![1] && subset_replayed == vec![Outcome::Value("A")];

    // Condition A2: reference stateful execution from the same pre-state.
    let (subset_reference, _) = reference_execution(&candidate_a);
    let s2 = subset_reference == vec![Outcome::Value("EMPTY")];

    // S3: direct comparison of the two mechanisms' read results.
    let s3 = subset_replayed[0] != subset_reference[0];
    println!(
        "  candidate A (subset): read(\"slot\") covered by historical record index 1\n\
         \x20subset:\n\
         \x20historical-outcome replay = {}\n\
         \x20stateful reference        = {}",
        subset_replayed[0].text(),
        subset_reference[0].text()
    );
    report("S1", s1);
    report("S2", s2);
    report("S3", s3);

    // ------------------------------------------------------------------
    // Candidate B — reordered causal predecessor:
    // read("slot"), then write("slot", "A").
    //
    // Both calls independently exist in the historical record, so
    // call-identity coverage is complete. But reading before the write
    // observes the pre-write state.
    // ------------------------------------------------------------------
    let candidate_b = [read, write_a];

    // Condition B1: historical-outcome replay.
    let mut consumed = Vec::new();
    let reorder_replayed = historical_outcome_replay(&record, &mut consumed, &candidate_b);
    let r1 = consumed == vec![1, 0] && reorder_replayed == vec![Outcome::Value("A"), Outcome::Ack];

    // Condition B2: reference stateful execution from the same pre-state.
    let (reorder_reference, _) = reference_execution(&candidate_b);
    let r2 = reorder_reference == vec![Outcome::Value("EMPTY"), Outcome::Ack];

    // R3: direct comparison of the two mechanisms' read results.
    let r3 = reorder_replayed[0] != reorder_reference[0];
    println!(
        "  candidate B (reorder): read covered by historical record index 1, \
         write covered by historical record index 0\n\
         \x20reorder:\n\
         \x20historical-outcome replay read = {}\n\
         \x20stateful reference read        = {}\n\
         \x20historical-outcome replay = [{}], stateful reference = [{}]",
        reorder_replayed[0].text(),
        reorder_reference[0].text(),
        reorder_replayed
            .iter()
            .map(Outcome::text)
            .collect::<Vec<_>>()
            .join(", "),
        reorder_reference
            .iter()
            .map(Outcome::text)
            .collect::<Vec<_>>()
            .join(", ")
    );
    report("R1", r1);
    report("R2", r2);
    report("R3", r3);

    // ------------------------------------------------------------------
    // Conclusion.
    //
    // supported:   the baseline controls hold and the causal mismatch is
    //              demonstrated for both the subset and the reorder.
    // inconclusive: the controls failed, so the experiment is broken for
    //              this run and the hypothesis is not judged.
    // refuted:     the controls hold but the hypothesized causal mismatch
    //              does not appear.
    // ------------------------------------------------------------------
    let hypothesis_metrics = s1 && s2 && s3 && r1 && r2 && r3;
    let conclusion = if h1 && h2 && hypothesis_metrics {
        "supported"
    } else if !h1 || !h2 {
        "inconclusive"
    } else {
        "refuted"
    };
    println!("Conclusion: {conclusion}");

    // Hard guarantees the experiment must satisfy to be taken seriously:
    // the controls must hold and the mismatch must be real.
    assert!(
        h1,
        "H1: the historical stateful execution must produce the expected record and final state"
    );
    assert!(
        h2,
        "H2: historical-outcome replay must reproduce the exact historical baseline"
    );
    assert!(
        s1,
        "S1: the subset read must be historically covered and replay the recorded Value(\"A\")"
    );
    assert!(s2, "S2: the subset reference execution must observe EMPTY");
    assert!(
        s3,
        "S3: subset historical-outcome replay must disagree with the stateful reference"
    );
    assert!(r1, "R1: the reorder must be fully historically covered");
    assert!(
        r2,
        "R2: the reorder reference execution must observe EMPTY before the write"
    );
    assert!(
        r3,
        "R3: reorder historical-outcome replay must disagree with the stateful reference"
    );
}
