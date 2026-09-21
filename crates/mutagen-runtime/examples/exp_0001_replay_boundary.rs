//! Experiment 0001 — Observation Replay Boundary.
//!
//! Research question:
//!   Can recorded external observations provide a deterministic replay
//!   boundary for evaluating changed agent behavior under environment
//!   drift? Equivalently: can baseline and changed behavior be compared
//!   under the same historical external observations without consulting
//!   the current live environment?
//!
//! Toy scenario (deterministic, in-memory, Rust standard library only):
//!
//!     request ─> decision logic ─> lookup("account_status") ─> environment
//!
//! Logical request: "Should we send an outreach message to this account?"
//!
//!   baseline : send only if the account status is Active.
//!   candidate: send if the account status is Active or Warm.
//!
//! Replay strategies:
//!   Mode A — live re-execution against a drifting environment (M1)
//!   Mode B — final-output playback, a negative control        (M2)
//!   Mode C — recorded-observation (boundary) replay           (M3–M7)
//!
//! The account/outreach vocabulary is arbitrary, disposable fixture
//! language: it exists only to exercise a generic external-observation
//! boundary. It carries no architectural meaning and is not a production
//! concept.
//!
//! No production abstractions are introduced. Every type and function in
//! this file is private to the experiment.

/// Possible values of the `account_status` external observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AccountStatus {
    Active,
    Inactive,
    Warm,
}

/// One external observation, addressed by a string key at the lookup boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Observation {
    AccountStatus(AccountStatus),
    AccountScore(bool),
}

/// Final decision of a component run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Decision {
    Send,
    DoNotSend,
}

/// Failure of an external lookup.
#[derive(Debug, PartialEq, Eq)]
enum LookupError {
    /// The live environment has no such key.
    UnknownKey(String),
    /// Replay asked for an observation that was never recorded. This must
    /// fail explicitly: never fall back to live state, never fabricate a
    /// default, never continue silently.
    MissingRecordedObservation(String),
}

/// The in-memory external environment. The only mutable state in the
/// experiment, and the thing that drifts between E1 and E2.
#[derive(Debug)]
struct Environment {
    account_status: AccountStatus,
    account_score: bool,
}

/// The logical request handed to a component.
#[derive(Debug)]
struct Request {
    account: &'static str,
}

/// One external observation captured at the boundary during a live run.
#[derive(Debug, PartialEq, Eq)]
struct RecordedObservation {
    key: String,
    value: Observation,
}

/// The one tiny external lookup boundary: how a component sees the world.
type Lookup<'a> = dyn for<'b> FnMut(&'b str) -> Result<Observation, LookupError> + 'a;

/// Live lookup against an environment.
fn live_lookup(env: &Environment, key: &str) -> Result<Observation, LookupError> {
    match key {
        "account_status" => Ok(Observation::AccountStatus(env.account_status)),
        "account_score" => Ok(Observation::AccountScore(env.account_score)),
        other => Err(LookupError::UnknownKey(other.to_string())),
    }
}

/// Replay lookup: serves only the observations recorded during the original
/// execution. A key that was never recorded fails explicitly; the live
/// environment is never consulted and no default is fabricated.
fn replay_lookup(recorded: &[RecordedObservation], key: &str) -> Result<Observation, LookupError> {
    recorded
        .iter()
        .find(|observation| observation.key == key)
        .map(|observation| Ok(observation.value))
        .unwrap_or_else(|| Err(LookupError::MissingRecordedObservation(key.to_string())))
}

/// Mode B: final-output playback. Returns the recorded final output
/// verbatim and reports that the "component" never executed: playback
/// simply returns the stored output, so `component_executed` stays `false`.
fn playback(recorded_output: Decision, component_executed: &mut bool) -> Decision {
    *component_executed = false;
    recorded_output
}

/// Read the `account_status` observation through the lookup boundary.
fn read_status(lookup: &mut Lookup<'_>) -> Result<AccountStatus, LookupError> {
    match lookup("account_status")? {
        Observation::AccountStatus(status) => Ok(status),
        other => panic!("account_status returned an unexpected observation: {other:?}"),
    }
}

/// Baseline rule: send only if the account is Active.
fn baseline(_request: &Request, lookup: &mut Lookup<'_>) -> Result<Decision, LookupError> {
    let status = read_status(lookup)?;
    Ok(if matches!(status, AccountStatus::Active) {
        Decision::Send
    } else {
        Decision::DoNotSend
    })
}

/// Candidate rule: send if the account is Active or Warm. Same logical
/// request, same external observation, deliberately different decision.
fn candidate(_request: &Request, lookup: &mut Lookup<'_>) -> Result<Decision, LookupError> {
    let status = read_status(lookup)?;
    Ok(
        if matches!(status, AccountStatus::Active | AccountStatus::Warm) {
            Decision::Send
        } else {
            Decision::DoNotSend
        },
    )
}

/// Candidate variant that additionally consults a second observation
/// (`account_score`) that was not recorded during the original execution.
/// Used for the missing-observation case (M7).
fn candidate_with_score(
    _request: &Request,
    lookup: &mut Lookup<'_>,
) -> Result<Decision, LookupError> {
    let status = read_status(lookup)?;
    let high_score = match lookup("account_score")? {
        Observation::AccountScore(high) => high,
        other => panic!("account_score returned an unexpected observation: {other:?}"),
    };
    let send = matches!(status, AccountStatus::Active | AccountStatus::Warm)
        || (status == AccountStatus::Inactive && high_score);
    Ok(if send {
        Decision::Send
    } else {
        Decision::DoNotSend
    })
}

/// Run the baseline live against `env`, recording every observation made
/// at the boundary. Returns the component result and the boundary record.
fn run_live_recording(
    env: &Environment,
    request: &Request,
) -> (Result<Decision, LookupError>, Vec<RecordedObservation>) {
    let mut recorded = Vec::new();
    let mut lookup = |key: &str| {
        let result = live_lookup(env, key);
        if let Ok(value) = &result {
            recorded.push(RecordedObservation {
                key: key.to_string(),
                value: *value,
            });
        }
        result
    };
    let result = baseline(request, &mut lookup);
    (result, recorded)
}

fn report(metric: &str, passed: bool) {
    println!("{metric} {}", if passed { "PASS" } else { "FAIL" });
}

fn main() {
    let request = Request { account: "acme" };

    // E1: the external state at the time the original execution happened.
    // E2: a later state of the same environment (it has drifted).
    let env1 = Environment {
        account_status: AccountStatus::Warm,
        account_score: false,
    };
    let env2 = Environment {
        account_status: AccountStatus::Active,
        account_score: false,
    };

    println!("Experiment 0001 — Observation Replay Boundary");
    println!("request: send outreach to account '{}'", request.account);

    // Original execution: baseline against E1. The boundary record holds
    // the external observations, and `original` the final output.
    let (original, recorded) = run_live_recording(&env1, &request);
    for observation in &recorded {
        println!(
            "recorded observation: {} = {:?}",
            observation.key, observation.value
        );
    }

    // Mode A — live re-execution of the same baseline and request against
    // the drifted environment E2.
    let (live_again, _) = run_live_recording(&env2, &request);
    let m1 = original != live_again;

    // Mode B — final-output playback (negative control): return the
    // recorded final output directly, for any "component". Attempt both
    // the baseline and the candidate this way; neither actually executes.
    let final_output = match &original {
        Ok(decision) => *decision,
        Err(err) => panic!("original baseline run unexpectedly failed: {err:?}"),
    };
    let mut component_executed = true;
    let baseline_under_playback = playback(final_output, &mut component_executed);
    let candidate_under_playback = playback(final_output, &mut component_executed);

    // Mode C — recorded-observation (boundary) replay: the replay lookup
    // serves the E1 observations instead of querying the live E2 state.
    let mut replay = |key: &str| replay_lookup(&recorded, key);
    let baseline_replay_1 = baseline(&request, &mut replay);
    let baseline_replay_2 = baseline(&request, &mut replay);
    let m3 = baseline_replay_1 == original;
    let m4 = baseline_replay_1 == baseline_replay_2;

    // Candidate under the exact same recorded observations. The recording
    // wrapper verifies the observations the candidate actually received.
    let mut seen = Vec::new();
    let mut recording_replay = |key: &str| {
        let result = replay_lookup(&recorded, key);
        if let Ok(value) = &result {
            seen.push(RecordedObservation {
                key: key.to_string(),
                value: *value,
            });
        }
        result
    };
    let candidate_replay = candidate(&request, &mut recording_replay);
    let m5 = candidate_replay.is_ok() && seen == recorded;
    let m6 = m5 && candidate_replay != baseline_replay_1;

    // Missing observation: a candidate that requests `account_score`, which
    // was never recorded. Replay must fail explicitly.
    let missing = candidate_with_score(&request, &mut replay);
    let expected = Err(LookupError::MissingRecordedObservation(
        "account_score".to_string(),
    ));
    let m7 = missing == expected;

    // M2: playback is deterministic, the "component" demonstrably never
    // executed under it (flag stays false), yet the candidate genuinely
    // differs from the baseline when both run against the same recorded
    // observations — so playback cannot evaluate the candidate.
    let m2 = baseline_under_playback == candidate_under_playback
        && baseline_under_playback == final_output
        && !component_executed
        && candidate_replay != baseline_replay_1;

    report("M1", m1);
    report("M2", m2);
    report("M3", m3);
    report("M4", m4);
    report("M5", m5);
    report("M6", m6);
    report("M7", m7);

    assert!(
        m1,
        "expected live re-execution to change under environment drift"
    );
    assert!(
        m2,
        "expected output playback to be deterministic and leave the candidate unobservable"
    );
    assert!(
        m3,
        "expected boundary replay to reproduce the baseline result"
    );
    assert!(m4, "expected repeated boundary replay to be deterministic");
    assert!(
        m5,
        "expected the candidate to receive the exact recorded observation"
    );
    assert!(
        m6,
        "expected the candidate difference to remain observable under replay"
    );
    assert!(
        m7,
        "expected a missing recorded observation to fail explicitly"
    );

    println!("Conclusion: supported");
}
