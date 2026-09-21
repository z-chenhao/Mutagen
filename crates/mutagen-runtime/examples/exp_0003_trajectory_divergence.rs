//! Experiment 0003 — Replay Under Trajectory Divergence.
//!
//! Research question:
//!   When a candidate's external-interaction trajectory differs from the
//!   historical trajectory, which forms of divergence can still be
//!   replayed using only recorded historical evidence, and which expose an
//!   explicit evidence gap?
//!
//! Historical baseline (deterministic, read-only, in-memory):
//!   read("left")  -> Value("L")
//!   read("right") -> Value("R")
//!   baseline output: "L|R"
//!
//! Candidate trajectories:
//!   A — subset:  read("left")                    (omits read("right"))
//!   B — reorder: read("right"), read("left")
//!   C — novel:   read("left"), read("novel")
//!
//! Two local experimental replay mechanisms:
//!   identity-aware:  resolve each candidate call by its exact recorded
//!                     interaction identity (operation + argument).
//!   strict positional: historical position i must equal candidate call i.
//!
//! The distinction under study is between *historical trajectory equality*
//! and *historical evidence coverage for the candidate's requested
//! interactions*. It is deliberately left as experimental evidence, not a
//! production policy or abstraction.
//!
//! The operation/argument/value/outcome vocabulary is generic interaction
//! language, deliberately domain-neutral. These are still synthetic
//! deterministic read-only fixtures; they are not evidence of
//! cross-domain generalization, and no production type, API, or schema is
//! introduced. Every type and function in this file is private to the
//! experiment; local duplication from Experiment 0002 is deliberate.

/// One external interaction: an operation invoked with one argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Call {
    operation: &'static str,
    argument: &'static str,
}

impl Call {
    fn describe(&self) -> String {
        format!("{}(\"{}\")", self.operation, self.argument)
    }
}

/// A read-only observation. The fixture is total and read-only: every
/// historical interaction produces a value, and no error outcomes occur.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    Value(&'static str),
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

/// Explicit failures of the experimental replay. Every variant makes a gap
/// or a mismatch visible; replay never consults live state, never applies a
/// default, and never fabricates a value.
#[derive(Debug, PartialEq, Eq)]
enum ExperimentalReplayError {
    /// The requested interaction has no recorded historical outcome.
    MissingHistoricalEvidence(String),
    /// Strict positional matching: the candidate call at a trajectory
    /// position differs from the historical call at that position.
    PositionalMismatch {
        position: usize,
        expected: String,
        actual: String,
    },
}

/// The external-interaction boundary: how a behavior sees the world.
type Lookup<'a> = dyn for<'b> FnMut(&'b Call) -> Result<Outcome, ExperimentalReplayError> + 'a;

/// Identity-aware experimental replay.
///
/// Each candidate call is resolved by its exact recorded interaction
/// identity (operation + argument): the first not-yet-consumed record
/// with that identity supplies the historical outcome.
///
/// `consumed_log` and `live_log` are owned by the experiment's driver,
/// outside the replay action itself: a replay that consumes a record must
/// log the record's index in `consumed_log`, and a replay that consulted
/// live state would have to log the call in `live_log`. The driver asserts
/// on these logs afterwards, so the claims "this record was not consumed"
/// and "live state was not consulted" are externally observable rather
/// than self-reported. This experiment does not claim this is any final
/// matching algorithm.
fn identity_replay<'a>(
    record: &'a [RecordedInteraction],
    consumed_log: &'a mut Vec<usize>,
    live_log: &'a mut Vec<Call>,
) -> Box<Lookup<'a>> {
    Box::new(move |call: &Call| {
        let index = record
            .iter()
            .enumerate()
            .find(|(index, recorded)| !consumed_log.contains(index) && recorded.call == *call)
            .map(|(index, _)| index);
        match index {
            Some(index) => {
                consumed_log.push(index);
                Ok(record[index].outcome.clone())
            }
            None => {
                // The gap is reported explicitly: no live lookup, no
                // default value, no substitution of a different
                // historical interaction.
                let _ = &live_log; // the boundary exists; it is never written
                Err(ExperimentalReplayError::MissingHistoricalEvidence(
                    call.describe(),
                ))
            }
        }
    }) as Box<Lookup<'a>>
}

/// Strict positional experimental replay (deliberate negative control).
///
/// Candidate call i must equal historical record i exactly; the first
/// divergence fails explicitly. Like identity replay, consumption is
/// logged to `consumed_log` and any live consultation would have to be
/// logged to `live_log`, both owned by the experiment's driver.
fn positional_replay<'a>(
    record: &'a [RecordedInteraction],
    consumed_log: &'a mut Vec<usize>,
    live_log: &'a mut Vec<Call>,
) -> Box<Lookup<'a>> {
    let mut position = 0usize;
    Box::new(move |call: &Call| {
        if position >= record.len() {
            let _ = &live_log; // the boundary exists; it is never written
            return Err(ExperimentalReplayError::MissingHistoricalEvidence(
                call.describe(),
            ));
        }
        let recorded = &record[position];
        if recorded.call != *call {
            return Err(ExperimentalReplayError::PositionalMismatch {
                position,
                expected: recorded.call.describe(),
                actual: call.describe(),
            });
        }
        consumed_log.push(position);
        position += 1;
        Ok(recorded.outcome.clone())
    }) as Box<Lookup<'a>>
}

/// The read-only behavior executed by the baseline and every candidate:
/// issue `read(argument)` for each argument, in the given order, and join
/// the observed values with `"|"`. The baseline trajectory is
/// `["left", "right"]` -> `"L|R"`; candidates A, B, and C differ only in
/// the argument trajectory they execute.
fn run(
    trajectory: &[&'static str],
    lookup: &mut Lookup<'_>,
) -> Result<String, ExperimentalReplayError> {
    let mut parts = Vec::new();
    for &argument in trajectory {
        match lookup(&Call {
            operation: "read",
            argument,
        })? {
            Outcome::Value(value) => parts.push(value),
        }
    }
    Ok(parts.join("|"))
}

/// Record a metric line, e.g. `F1 PASS`.
fn report(metric: &str, passed: bool) {
    println!("{metric} {}", if passed { "PASS" } else { "FAIL" });
}

fn main() {
    println!("Experiment 0003 — Replay Under Trajectory Divergence");
    println!(
        "history: read(\"left\") -> Value(\"L\"), read(\"right\") -> Value(\"R\"); \
         baseline output \"L|R\""
    );

    let left = Call {
        operation: "read",
        argument: "left",
    };
    let right = Call {
        operation: "read",
        argument: "right",
    };

    // ------------------------------------------------------------------
    // Historical fixture.
    //
    // One fully scripted, in-memory, read-only environment. Every call is
    // logged to `history_live_log` (owned by this driver, outside any
    // replay) and appended to the record. This is the only phase in which
    // the live environment is consulted.
    // ------------------------------------------------------------------
    let mut history_live_log = Vec::new();
    let mut record = Vec::new();
    let mut live = |call: &Call| {
        history_live_log.push(*call);
        let outcome = match call.argument {
            "left" => Outcome::Value("L"),
            "right" => Outcome::Value("R"),
            other => panic!(
                "history defines only read(\"left\") and read(\"right\"), got read(\"{other}\")"
            ),
        };
        record.push(RecordedInteraction {
            call: *call,
            outcome: outcome.clone(),
        });
        Ok(outcome)
    };

    let history_output = run(&["left", "right"], &mut live).expect("history run must succeed");
    assert_eq!(history_output, "L|R");
    assert_eq!(
        history_live_log,
        vec![left, right],
        "history consults the live environment exactly twice"
    );
    debug_assert_eq!(
        record,
        vec![
            RecordedInteraction {
                call: left,
                outcome: Outcome::Value("L"),
            },
            RecordedInteraction {
                call: right,
                outcome: Outcome::Value("R"),
            },
        ],
        "record is the two historical read interactions in order"
    );

    // ------------------------------------------------------------------
    // F1 (control): the historical baseline replayed from the record.
    // ------------------------------------------------------------------
    let mut consumed = Vec::new();
    let mut live_probe = Vec::new();
    let mut replay = identity_replay(&record, &mut consumed, &mut live_probe);
    let f1_result = run(&["left", "right"], &mut replay);
    drop(replay); // release the mutable borrows of `consumed`/`live_probe`
    let f1 =
        f1_result == Ok(history_output.clone()) && consumed == vec![0, 1] && live_probe.is_empty();
    println!(
        "  control: read(\"left\") -> L, read(\"right\") -> R, output \"L|R\"; \
         no live consultation"
    );
    report("F1", f1);

    // ------------------------------------------------------------------
    // Candidate A — subset divergence: read("left") only.
    // ------------------------------------------------------------------
    let mut consumed = Vec::new();
    let mut live_probe = Vec::new();
    let mut replay = identity_replay(&record, &mut consumed, &mut live_probe);
    let subset_result = run(&["left"], &mut replay);
    drop(replay); // release the mutable borrows of `consumed`/`live_probe`
    let s1 = subset_result == Ok("L".to_string()) && live_probe.is_empty();
    // The unused historical interaction must not invalidate the replay and
    // must not be force-consumed or force-executed. `consumed` is owned by
    // this driver, so "read(\"right\") was not consumed" is observable.
    let s2 = consumed == vec![0] && !consumed.contains(&1) && live_probe.is_empty();
    println!(
        "  candidate A: read(\"left\") -> L, output \"L\"; unused historical \
         interaction read(\"right\") remains unconsumed"
    );
    report("S1", s1);
    report("S2", s2);

    // ------------------------------------------------------------------
    // Candidate B — reorder divergence: read("right") then read("left").
    // ------------------------------------------------------------------
    // Condition B1: identity-aware experimental replay.
    let mut consumed = Vec::new();
    let mut live_probe = Vec::new();
    let mut replay = identity_replay(&record, &mut consumed, &mut live_probe);
    let reorder_result = run(&["right", "left"], &mut replay);
    drop(replay); // release the mutable borrows of `consumed`/`live_probe`
    let r1 = reorder_result == Ok("R|L".to_string())
        && consumed.len() == 2
        && consumed.contains(&0)
        && consumed.contains(&1)
        && live_probe.is_empty();
    println!(
        "  candidate B (identity-aware): read(\"right\") -> R, read(\"left\") -> L, \
         output \"R|L\""
    );

    // Condition B2: strict positional negative control.
    let mut consumed = Vec::new();
    let mut live_probe = Vec::new();
    let mut replay = positional_replay(&record, &mut consumed, &mut live_probe);
    let positional_result = run(&["right", "left"], &mut replay);
    drop(replay); // release the mutable borrows of `consumed`/`live_probe`
    let r2 = matches!(
        &positional_result,
        Err(ExperimentalReplayError::PositionalMismatch {
            position,
            expected,
            actual,
        }) if *position == 0 && expected == "read(\"left\")" && actual == "read(\"right\")"
    ) && consumed.is_empty()
        && live_probe.is_empty()
        && r1;
    println!(
        "  candidate B (strict positional): mismatch at position 0 — expected \
         read(\"left\"), got read(\"right\"); evidence existed for both requested \
         interactions under identity-aware replay"
    );
    report("R1", r1);
    report("R2", r2);

    // ------------------------------------------------------------------
    // Candidate C — novel-interaction divergence: read("left") then
    // read("novel"), where the history contains no read("novel").
    // ------------------------------------------------------------------
    let mut consumed = Vec::new();
    let mut live_probe = Vec::new();
    let mut replay = identity_replay(&record, &mut consumed, &mut live_probe);
    let novel_result = run(&["left", "novel"], &mut replay);
    drop(replay); // release the mutable borrows of `consumed`/`live_probe`
    let n1 = matches!(
        &novel_result,
        Err(ExperimentalReplayError::MissingHistoricalEvidence(missing))
            if missing == "read(\"novel\")"
    );
    // No fallback: the only consumption is the covered read("left") — the
    // historical read("right") was not substituted in — and no live lookup
    // occurred. The failed run also fabricates nothing: it is an Err, not a
    // value.
    let n2 = consumed == vec![0] && !consumed.contains(&1) && live_probe.is_empty();
    println!(
        "  candidate C: read(\"left\") -> L, then read(\"novel\") -> \
         MissingHistoricalEvidence(read(\"novel\")); no live lookup, no \
         substitution of read(\"right\"), no fabricated value"
    );
    report("N1", n1);
    report("N2", n2);

    assert!(f1, "F1: the historical baseline must replay faithfully");
    assert!(
        s1,
        "S1: the subset candidate must execute entirely from recorded evidence"
    );
    assert!(
        s2,
        "S2: the unused historical interaction must not invalidate this replay"
    );
    assert!(
        r1,
        "R1: the reordered candidate must execute under identity-aware replay"
    );
    assert!(
        r2,
        "R2: strict positional replay must reject the reorder while evidence exists"
    );
    assert!(
        n1,
        "N1: the novel interaction must produce an explicit evidence gap"
    );
    assert!(
        n2,
        "N2: the evidence gap must have no live/default/substitution fallback"
    );

    let conclusion = if f1 && s1 && s2 && r1 && r2 && n1 && n2 {
        "supported"
    } else {
        "refuted"
    };
    println!("Conclusion: {conclusion}");
}
