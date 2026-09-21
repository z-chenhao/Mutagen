//! Experiment 0002 — Replay Record Ablation.
//!
//! Research question:
//!   Which information inside a recorded external interaction becomes
//!   behaviorally necessary when replay must handle argument-sensitive
//!   calls, repeated calls, and error outcomes?
//!
//! Principle: for each fixture the full experimental record (an ordered
//! historical trace of {call, outcome}) must replay the historical
//! behavior correctly; then exactly one category of information is
//! ablated and replay is attempted again. The experiment distinguishes
//! "full record succeeds" from "ablated record loses behavioral
//! fidelity".
//!
//! Three deterministic, domain-neutral fixtures:
//!
//!   A — argument-sensitive: read("alpha") and read("beta") have
//!       different historical outcomes; changed behavior reorders the calls.
//!   B — repeated identical: poll("task") twice, with different historical
//!       outcomes for the two occurrences.
//!   C — error-dependent: fetch("missing") -> Error(NotFound); the
//!       baseline branches on the error.
//!
//! The operation/argument/value/outcome vocabulary is generic interaction
//! language, deliberately domain-neutral. These are still synthetic
//! deterministic fixtures; they are not evidence of cross-domain
//! generalization, and no production type, API, or schema is introduced.
//! Every type and function in this file is private to the experiment.

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

/// The external outcome of one interaction: a value or an error. Errors
/// are outcomes, not failures of the boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    Value(&'static str),
    Error(&'static str),
}

/// The full experimental record entry: the interaction *and* its
/// historical outcome. The full record is the ordered `Vec` of these,
/// which preserves both the occurrence position of each interaction and
/// every outcome including errors.
///
/// This is throwaway experimental scaffolding: a local ordered trace,
/// not a production replay schema.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RecordedInteraction {
    call: Call,
    outcome: Outcome,
}

/// The ablated record for Fixture A: the historical trace with argument
/// identity removed. Only the operation survives, so two distinct
/// historical interactions that share an operation are no longer
/// distinguishable.
#[derive(Clone, Debug, PartialEq, Eq)]
struct OpOnlyRecord {
    operation: &'static str,
    outcome: Outcome,
}

/// The ablated record for Fixture B: a single mapping
/// (operation, argument) -> outcome in which repeated occurrences of the
/// same call collapse into one entry.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CollapsedMapEntry {
    call: Call,
    outcome: Outcome,
}

/// Failure of a replay lookup. Every variant is explicit: replay never
/// consults live state and never fabricates a value.
#[derive(Debug, PartialEq, Eq)]
enum LookupError {
    /// The current call matches more than one recorded interaction and the
    /// record cannot tell which historical outcome it wants.
    AmbiguousMatches { call: String, matches: usize },
    /// No recorded interaction matches the current call.
    MissingRecordedInteraction(String),
    /// The external call produced an error outcome; the fixture's behavior
    /// cannot continue past it.
    ExternalError { call: String, error: String },
}

/// The external-interaction boundary: how a behavior sees the world.
type Lookup<'a> = dyn for<'b> FnMut(&'b Call) -> Result<Outcome, LookupError> + 'a;

/// Full-replay lookup over an ordered historical trace.
///
/// A current call matches the recorded interactions with the *same
/// operation and argument*; among those, its outcome is taken by
/// occurrence: the n-th time a given (operation, argument) call is made
/// receives the n-th recorded outcome for that call. This is the
/// experimental mechanism that preserves both argument identity and
/// occurrence distinction; the experiment does not claim any final
/// matching algorithm.
fn full_replay<'a>(record: &'a [RecordedInteraction]) -> Box<Lookup<'a>> {
    let mut made = Vec::new();
    Box::new(move |call: &Call| {
        let matching: Vec<usize> = record
            .iter()
            .enumerate()
            .filter(|(_, recorded)| recorded.call == *call)
            .map(|(index, _)| index)
            .collect();
        let occurrence = made.iter().filter(|seen| **seen == *call).count();
        match matching.get(occurrence) {
            Some(index) => {
                made.push(*call);
                Ok(record[*index].outcome.clone())
            }
            None => Err(LookupError::MissingRecordedInteraction(call.describe())),
        }
    }) as Box<Lookup<'a>>
}

/// Ablated replay lookup for Fixture A: argument identity removed.
///
/// With `fail_on_ambiguity`, a call matching more than one not-yet-
/// consumed record fails with `AmbiguousMatches` (the record cannot tell
/// which historical outcome the call wants). Otherwise it falls back to
/// the deterministic first-match policy: the earliest not-yet-consumed
/// historical record with the same operation.
fn op_only_replay<'a>(record: &'a [OpOnlyRecord], fail_on_ambiguity: bool) -> Box<Lookup<'a>> {
    let mut consumed = vec![false; record.len()];
    Box::new(move |call: &Call| {
        let candidates: Vec<usize> = (0..record.len())
            .filter(|&index| !consumed[index] && record[index].operation == call.operation)
            .collect();
        if fail_on_ambiguity && candidates.len() > 1 {
            return Err(LookupError::AmbiguousMatches {
                call: call.describe(),
                matches: candidates.len(),
            });
        }
        match candidates.into_iter().next() {
            Some(index) => {
                consumed[index] = true;
                Ok(record[index].outcome.clone())
            }
            None => Err(LookupError::MissingRecordedInteraction(call.describe())),
        }
    }) as Box<Lookup<'a>>
}

/// Ablated replay lookup for Fixture B: a single (operation, argument)
/// -> outcome mapping; repeated occurrences of the same call all receive
/// the one stored outcome.
fn map_replay<'a>(map: &'a [CollapsedMapEntry]) -> Box<Lookup<'a>> {
    Box::new(
        move |call: &Call| match map.iter().find(|entry| entry.call == *call) {
            Some(entry) => Ok(entry.outcome.clone()),
            None => Err(LookupError::MissingRecordedInteraction(call.describe())),
        },
    ) as Box<Lookup<'a>>
}

/// Fixture A behavior: read the given arguments, in this order, through
/// the boundary; an error outcome aborts the run.
fn run_fixture_a(
    order: &[&'static str],
    lookup: &mut Lookup<'_>,
) -> Result<Vec<&'static str>, LookupError> {
    let mut values = Vec::new();
    for &argument in order {
        let call = Call {
            operation: "read",
            argument,
        };
        match lookup(&call)? {
            Outcome::Value(value) => values.push(value),
            Outcome::Error(error) => {
                return Err(LookupError::ExternalError {
                    call: call.describe(),
                    error: error.to_string(),
                });
            }
        }
    }
    Ok(values)
}

/// Fixture B behavior: poll the same (operation, argument) twice, in
/// order; an error outcome aborts the run.
fn run_fixture_b(lookup: &mut Lookup<'_>) -> Result<Vec<&'static str>, LookupError> {
    let mut values = Vec::new();
    for _ in 0..2 {
        let call = Call {
            operation: "poll",
            argument: "task",
        };
        match lookup(&call)? {
            Outcome::Value(value) => values.push(value),
            Outcome::Error(error) => {
                return Err(LookupError::ExternalError {
                    call: call.describe(),
                    error: error.to_string(),
                });
            }
        }
    }
    Ok(values)
}

/// Fixture C behavior: one external call whose *error* the baseline
/// branches on: Error -> fallback path, Value -> normal path.
fn run_fixture_c(lookup: &mut Lookup<'_>) -> Result<&'static str, LookupError> {
    match lookup(&Call {
        operation: "fetch",
        argument: "missing",
    })? {
        Outcome::Error(_) => Ok("fallback"),
        Outcome::Value(_) => Ok("normal"),
    }
}

/// Record a metric line, e.g. `F1 PASS`.
fn report(metric: &str, passed: bool) {
    println!("{metric} {}", if passed { "PASS" } else { "FAIL" });
}

fn main() {
    println!("Experiment 0002 — Replay Record Ablation");

    // ------------------------------------------------------------------
    // Fixture A — argument-sensitive interaction.
    //
    // History: read("alpha") -> Value("A"), read("beta") -> Value("B").
    // Changed behavior: the same two reads, in the opposite order.
    //
    // Ablation: argument identity removed from the record.
    // ------------------------------------------------------------------
    println!(
        "Fixture A (argument-sensitive): history read(\"alpha\") -> Value(\"A\"), \
         read(\"beta\") -> Value(\"B\"); changed behavior reorders the two reads"
    );

    let mut recorded_a = Vec::new();
    let mut live_a = |call: &Call| {
        assert!(
            call.operation == "read",
            "fixture A history defines only read"
        );
        let outcome = match call.argument {
            "alpha" => Outcome::Value("A"),
            "beta" => Outcome::Value("B"),
            other => panic!("fixture A: unexpected argument {other}"),
        };
        recorded_a.push(RecordedInteraction {
            call: *call,
            outcome: outcome.clone(),
        });
        Ok(outcome)
    };

    let baseline_order: &[&str] = &["alpha", "beta"];
    let changed_order: &[&str] = &["beta", "alpha"];
    let history_output_a =
        run_fixture_a(baseline_order, &mut live_a).expect("fixture A history run must succeed");
    let record_a = recorded_a;
    debug_assert_eq!(
        history_output_a,
        vec!["A", "B"],
        "fixture A history is A then B by construction"
    );

    // F1 (control): the full experimental record (arguments and
    // occurrences preserved) replays both behaviors with the correct
    // outcome bound to each call.
    // Each replay attempt gets a fresh replay over the full record: the
    // occurrence counter tracks calls within one replay, and the record
    // itself is never mutated.
    let mut full_a_baseline = full_replay(&record_a);
    let baseline_a = run_fixture_a(baseline_order, &mut full_a_baseline)
        .expect("F1: baseline under full replay must succeed");
    let correct_changed = vec!["B", "A"];
    let mut full_a_changed = full_replay(&record_a);
    let changed_a = run_fixture_a(changed_order, &mut full_a_changed)
        .expect("F1: changed behavior under full replay must succeed");
    let f1 = baseline_a == history_output_a && changed_a == correct_changed;

    // A1: remove argument identity. Both historical records are now just
    // ("read", outcome). Under a strict policy the changed behavior's
    // first call is explicitly ambiguous; under a deterministic
    // first-match policy it is bound to the wrong historical outcome.
    let op_only: Vec<OpOnlyRecord> = record_a
        .iter()
        .map(|record| OpOnlyRecord {
            operation: record.call.operation,
            outcome: record.outcome.clone(),
        })
        .collect();

    let mut strict_a = op_only_replay(&op_only, true);
    let ablated_strict = run_fixture_a(changed_order, &mut strict_a);
    let ambiguity_explicit = matches!(&ablated_strict, Err(LookupError::AmbiguousMatches { matches, .. }) if *matches == 2);

    let mut first_match_a = op_only_replay(&op_only, false);
    let ablated_first_match = run_fixture_a(changed_order, &mut first_match_a);
    // Correct binding under full replay was [B, A]; the ablated first-
    // match replay deterministically yields [A, B] — the first changed
    // call read("beta") receives the historical outcome of
    // read("alpha").
    let wrong_binding = matches!(ablated_first_match.as_ref(), Ok(values)
        if values.first() == Some(&"A") && values == &["A", "B"]
            && values != &correct_changed);
    let a1 = ambiguity_explicit && wrong_binding;

    println!(
        "  ablated replay of changed behavior: strict = {ablated_strict:?}, first-match = {ablated_first_match:?} (correct: {correct_changed:?})"
    );
    report("F1", f1);
    report("A1", ambiguity_explicit && wrong_binding);

    // ------------------------------------------------------------------
    // Fixture B — repeated identical interaction.
    //
    // History: poll("task") -> Value("pending"), poll("task") ->
    // Value("ready"). Same operation, same argument, two different
    // historical outcomes.
    //
    // Ablation: the observations collapse into a single
    // (operation, argument) -> outcome mapping; occurrences cannot be
    // distinguished.
    // ------------------------------------------------------------------
    println!(
        "Fixture B (repeated identical): history poll(\"task\") -> Value(\"pending\"), \
         poll(\"task\") -> Value(\"ready\")"
    );

    let mut polls = 0;
    let mut recorded_b = Vec::new();
    let mut live_b = |call: &Call| {
        assert!(
            call.operation == "poll" && call.argument == "task",
            "fixture B history defines only poll(\"task\")"
        );
        let outcome = match polls {
            0 => Outcome::Value("pending"),
            1 => Outcome::Value("ready"),
            _ => panic!("fixture B history makes exactly two polls"),
        };
        polls += 1;
        recorded_b.push(RecordedInteraction {
            call: *call,
            outcome: outcome.clone(),
        });
        Ok(outcome)
    };

    let history_output_b = run_fixture_b(&mut live_b).expect("fixture B history run must succeed");
    let record_b = recorded_b;
    debug_assert_eq!(
        history_output_b,
        vec!["pending", "ready"],
        "fixture B history is pending then ready by construction"
    );

    // F2 (control): the full ordered trace replays the two occurrences
    // with their distinct historical outcomes.
    let mut full_b = full_replay(&record_b);
    let baseline_b =
        run_fixture_b(&mut full_b).expect("F2: baseline under full replay must succeed");
    let f2 = baseline_b == history_output_b;

    // B1: collapse the history into a single (operation, argument) ->
    // outcome mapping. The collapse is necessarily lossy (the second
    // occurrence overwrites the first); replay then reproduces only one
    // of the two historical outcomes.
    let mut map: Vec<CollapsedMapEntry> = Vec::new();
    for record in &record_b {
        match map.iter_mut().find(|entry| entry.call == record.call) {
            Some(entry) => entry.outcome = record.outcome.clone(),
            None => map.push(CollapsedMapEntry {
                call: record.call,
                outcome: record.outcome.clone(),
            }),
        }
    }
    let first_outcome_lost = !map
        .iter()
        .any(|entry| entry.outcome == Outcome::Value("pending"));
    let mut map_replay = map_replay(&map);
    let ablated_b = run_fixture_b(&mut map_replay);
    let b1 = map.len() == 1
        && first_outcome_lost
        && ablated_b == Ok(vec!["ready", "ready"])
        && ablated_b != Ok(history_output_b.clone());

    println!(
        "  collapsed mapping replays: {:?} (history: {:?})",
        ablated_b, history_output_b
    );
    report("F2", f2);
    report("B1", b1);

    // ------------------------------------------------------------------
    // Fixture C — error-dependent interaction.
    //
    // History: fetch("missing") -> Error("NotFound"); the baseline
    // branches on it (Error -> fallback path).
    //
    // Ablation: the record preserves only successful values; the error
    // outcome is dropped.
    // ------------------------------------------------------------------
    println!(
        "Fixture C (error-dependent): history fetch(\"missing\") -> Error(\"NotFound\"); \
         baseline branches on the error into a fallback path"
    );

    let mut recorded_c = Vec::new();
    let mut live_c = |call: &Call| {
        assert!(
            call.operation == "fetch" && call.argument == "missing",
            "fixture C history defines only fetch(\"missing\")"
        );
        let outcome = Outcome::Error("NotFound");
        recorded_c.push(RecordedInteraction {
            call: *call,
            outcome: outcome.clone(),
        });
        Ok(outcome)
    };

    let history_output_c = run_fixture_c(&mut live_c).expect("fixture C history run must succeed");
    let record_c = recorded_c;
    debug_assert_eq!(
        history_output_c, "fallback",
        "fixture C history takes the error branch by construction"
    );

    // F3 (control): the full record, which includes the error as an
    // outcome, replays the historical branch.
    let mut full_c = full_replay(&record_c);
    let replay_c = run_fixture_c(&mut full_c);
    let f3 = replay_c == Ok(history_output_c);

    // C1: keep only successful values; the single historical interaction
    // had an error outcome, so the ablated record is empty and replay
    // must fail explicitly — it cannot reproduce the historical branch
    // and it must not fabricate a value.
    let value_only: Vec<RecordedInteraction> = record_c
        .iter()
        .filter(|record| matches!(record.outcome, Outcome::Value(_)))
        .cloned()
        .collect();
    let mut value_only_replay = full_replay(&value_only);
    let ablated_c = run_fixture_c(&mut value_only_replay);
    let c1 = ablated_c
        == Err(LookupError::MissingRecordedInteraction(
            "fetch(\"missing\")".to_string(),
        ));

    println!("  error-dropping replay: {ablated_c:?}");
    report("F3", f3);
    report("C1", c1);

    assert!(
        f1,
        "F1: the full experimental record must replay the argument-sensitive fixture"
    );
    assert!(
        a1,
        "A1: removing argument identity must lose faithful binding in Fixture A"
    );
    assert!(
        f2,
        "F2: the full experimental record must replay the repeated-call fixture"
    );
    assert!(
        b1,
        "B1: removing occurrence distinction must lose faithful replay in Fixture B"
    );
    assert!(
        f3,
        "F3: the full experimental record must replay the error-dependent fixture"
    );
    assert!(
        c1,
        "C1: dropping error outcomes must lose faithful replay in Fixture C"
    );

    println!("Conclusion: supported");
}
