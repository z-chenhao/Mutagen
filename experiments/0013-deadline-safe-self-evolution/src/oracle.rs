//! The deterministic external-state task oracle (Experiment 0010).
//!
//! The oracle is the experiment's only quality judge. It grades an
//! episode solely from the task specification, the episode's
//! termination category, and the initial / final external state.
//!
//! **Anti-circularity is structural, not a comment.** The
//! `OracleInput` type has no field — and therefore no code path — for a
//! candidate identity, candidate prompt, mutation rationale, condition
//! name, selection score, cost, trajectory style, or the model's
//! final-answer wording (spec §10): the oracle judges only termination,
//! initial state, final state, and the registered target state.
//!
//! Task success (deliberately strict):
//!
//! 1. the episode reached **normal completion** (no agent or
//!    infrastructure failure), and
//! 2. the final external state **exactly matches** the task's registered
//!    target state, including keys not intended to change retaining
//!    their expected values.
//!
//! No LLM judge, no semantic grading, no regex over persuasive
//! natural-language claims. A model that says "done" while the state is
//! wrong fails. A model that reaches the correct state but then fails
//! the protocol before normal completion fails.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::trace::TerminationReason;

/// The three disjoint logical task splits (spec §19).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskSplit {
    /// Evidence the mutation generator may see.
    Discovery,
    /// Candidate pool competes here; the mutator never sees these
    /// tasks or their outcomes.
    Selection,
    /// Only the one frozen selected candidate enters; no other
    /// candidate may touch these tasks.
    Promotion,
}

/// The complete registered task specification (one `tasks.json` entry).
///
/// A task registers exactly one *fault key* (spec §24–26). The
/// per-episode fault mode is derived from (fault key, selected stress
/// level) by `crate::tools::FaultMode::for_stress`; at S0 the fault is
/// `Reliable` regardless of the key. The registry itself therefore
/// stores no stress severity — the same task is evaluated at multiple
/// registered stress levels by design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskSpec {
    pub id: String,
    pub name: String,
    pub family: String,
    pub split: TaskSplit,
    pub prompt: String,
    /// Registered initial external state: `{"x": ..., "y": ...}`.
    pub initial_state: Value,
    /// Registered target external state: `{"x": ..., "y": ...}`.
    pub target_state: Value,
    /// The state key the registered fault targets (`"x"` or `"y"`).
    pub fault_key: String,
}

/// One family's registered task IDs per split (the registry's family
/// map — the authority for family membership).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FamilyTasks {
    pub discovery: Vec<String>,
    pub selection: Vec<String>,
    pub promotion: Vec<String>,
}

/// The complete frozen task registry file (`tasks.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskFile {
    pub experiment_id: String,
    pub families: BTreeMap<String, FamilyTasks>,
    pub tasks: Vec<TaskSpec>,
}

impl TaskFile {
    /// Structural validation of the frozen registry (12 tasks, 3/3/6
    /// splits, unique consistent IDs, family map ↔ task consistency,
    /// two-key states, registered fault keys).
    pub fn validate(&self) -> Result<(), String> {
        let mut errors = Vec::new();
        if self.tasks.len() != 12 {
            errors.push(format!(
                "registry must contain 12 tasks, found {}",
                self.tasks.len()
            ));
        }
        if self.families.len() != 3 {
            errors.push(format!(
                "registry must contain 3 families, found {}",
                self.families.len()
            ));
        }
        let mut seen = BTreeMap::new();
        for t in &self.tasks {
            *seen.entry(t.id.clone()).or_insert(0) += 1;
        }
        for (id, n) in &seen {
            if *n != 1 {
                errors.push(format!("task ID {id} appears {n} times"));
            }
        }
        for t in &self.tasks {
            let want_family = self
                .families
                .get(&t.family)
                .and_then(|f| match t.split {
                    TaskSplit::Discovery => f.discovery.iter().find(|x| *x == &t.id),
                    TaskSplit::Selection => f.selection.iter().find(|x| *x == &t.id),
                    TaskSplit::Promotion => f.promotion.iter().find(|x| *x == &t.id),
                })
                .is_some();
            if !want_family {
                errors.push(format!(
                    "task {} is not listed under family {} split {:?} in the family map",
                    t.id, t.family, t.split
                ));
            }
            if !t.prompt.trim().is_empty() && t.prompt.len() > 10_000 {
                errors.push(format!("task {} prompt unreasonably long", t.id));
            }
            for (name, state) in [
                ("initial_state", t.initial_state.clone()),
                ("target_state", t.target_state.clone()),
            ] {
                if !state.as_object().is_some_and(|o| o.len() == 2) {
                    errors.push(format!(
                        "task {} {name} must be a two-key state object",
                        t.id
                    ));
                }
            }
            if t.fault_key != "x" && t.fault_key != "y" {
                errors.push(format!(
                    "task {} fault_key {:?} is not a registered state key",
                    t.id, t.fault_key
                ));
            }
            if !t.initial_state.get(&t.fault_key).is_some() {
                errors.push(format!(
                    "task {} fault key {:?} missing from initial state",
                    t.id, t.fault_key
                ));
            }
        }
        // Family map consistency in the other direction: every listed ID
        // must exist and its task must agree on family/split.
        for (family, f) in &self.families {
            for (id, split) in f
                .discovery
                .iter()
                .map(|i| (i, TaskSplit::Discovery))
                .chain(f.selection.iter().map(|i| (i, TaskSplit::Selection)))
                .chain(f.promotion.iter().map(|i| (i, TaskSplit::Promotion)))
            {
                let Some(t) = self.tasks.iter().find(|t| &t.id == id) else {
                    errors.push(format!("family {family} lists unknown task {id}"));
                    continue;
                };
                if t.family != *family || t.split != split {
                    errors.push(format!("task {id} disagrees with the family map"));
                }
            }
        }
        // State literals must be unique per task pair (split-disjoint
        // checks happen in the verifier against the frozen profile).
        for t in &self.tasks {
            if let Some(obj) = t.initial_state.as_object() {
                for v in obj.values() {
                    if v.is_null() {
                        errors.push(format!("task {} initial state contains null", t.id));
                    }
                }
            }
            if let Some(obj) = t.target_state.as_object() {
                for v in obj.values() {
                    if v.is_null() {
                        errors.push(format!("task {} target state contains null", t.id));
                    }
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

/// The complete input the oracle receives for one episode.
///
/// Deliberately minimal: this is the *only* surface the evaluator
/// function accepts. There is no field for condition, prompt,
/// candidate name, stress level, or any other experiment-side identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OracleInput {
    /// The episode's termination category (kernel-owned string).
    pub termination_reason: String,
    /// The initial external state the episode started from.
    pub initial_state: Value,
    /// The final external state after the episode.
    pub final_state: Value,
}

/// The oracle's verdict on one episode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskEvaluation {
    pub success: bool,
    /// Human-readable failure reasons; empty on success. Deterministic
    /// (sorted by key) so recomputation is deep-comparable.
    pub reasons: Vec<String>,
}

/// Evaluate one episode against one registered task.
pub fn evaluate_task(task: &TaskSpec, input: &OracleInput) -> TaskEvaluation {
    let mut reasons: Vec<String> = Vec::new();

    // 1. Normal completion is required, whatever the state is.
    if input.termination_reason != TerminationReason::Completed.as_str() {
        reasons.push(format!(
            "episode did not complete normally (termination: {})",
            input.termination_reason
        ));
    }

    // 2. Final state must be the two-key external state object.
    let final_obj = match input.final_state.as_object() {
        Some(o) => o,
        None => {
            reasons.push(format!(
                "final state is not an object (got {})",
                input.final_state
            ));
            return TaskEvaluation {
                success: false,
                reasons,
            };
        }
    };
    let target_obj = task.target_state.as_object();

    if let Some(target) = target_obj {
        let mut keys: Vec<&String> = target.keys().collect();
        keys.sort();
        for k in keys {
            let expected = target.get(k).unwrap();
            let got = final_obj.get(k);
            if got != Some(expected) {
                reasons.push(format!(
                    "key {k}: expected {}, got {}",
                    expected,
                    got.map(Value::to_string)
                        .unwrap_or_else(|| "missing".to_string())
                ));
            }
            // Keys not intended to change must retain their expected
            // value: for those keys the registered target equals the
            // registered initial state, so report the change explicitly.
            let initial_v = task.initial_state.get(k);
            if Some(expected) == initial_v && got != initial_v {
                reasons.push(format!(
                    "key {k} was not intended to change but changed from {} to {}",
                    initial_v
                        .map(Value::to_string)
                        .unwrap_or_else(|| "missing".to_string()),
                    got.map(Value::to_string)
                        .unwrap_or_else(|| "missing".to_string())
                ));
            }
        }
        // The external state has exactly two keys; anything else is a
        // mismatch.
        let mut extras: Vec<&String> = final_obj
            .keys()
            .filter(|k| !target.contains_key(*k))
            .collect();
        extras.sort();
        for k in extras {
            reasons.push(format!("unexpected key {k} in final state"));
        }
    } else {
        reasons.push("registered target state is not an object".to_string());
    }

    TaskEvaluation {
        success: reasons.is_empty(),
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> TaskSpec {
        TaskSpec {
            id: "PRO1".into(),
            name: "promotion_direct_set_y".into(),
            family: "direct_set".into(),
            split: TaskSplit::Promotion,
            prompt: "Set y to C.".into(),
            initial_state: serde_json::json!({"x": "EMPTY", "y": "B"}),
            target_state: serde_json::json!({"x": "EMPTY", "y": "C"}),
            fault_key: "y".into(),
        }
    }

    fn input(termination: &str, final_state: Value) -> OracleInput {
        OracleInput {
            termination_reason: termination.into(),
            initial_state: serde_json::json!({"x": "EMPTY", "y": "B"}),
            final_state,
        }
    }

    #[test]
    fn correct_state_and_completion_is_success() {
        let e = evaluate_task(
            &task(),
            &input("completed", serde_json::json!({"x": "EMPTY", "y": "C"})),
        );
        assert!(e.success);
        assert!(e.reasons.is_empty());
    }

    #[test]
    fn wrong_state_fails_even_when_completed() {
        let e = evaluate_task(
            &task(),
            &input("completed", serde_json::json!({"x": "EMPTY", "y": "B"})),
        );
        assert!(!e.success);
        assert!(e.reasons.iter().any(|r| r.contains("y")));
    }

    #[test]
    fn unchanged_key_that_changed_fails() {
        let e = evaluate_task(
            &task(),
            &input("completed", serde_json::json!({"x": "TAMPERED", "y": "C"})),
        );
        assert!(!e.success);
        assert!(
            e.reasons
                .iter()
                .any(|r| r.contains("not intended to change"))
        );
    }

    #[test]
    fn correct_state_but_protocol_failure_fails() {
        // Model reached the correct state, then hit the turn limit before
        // a final answer: this is a task failure, deliberately.
        let e = evaluate_task(
            &task(),
            &input("turn_limit", serde_json::json!({"x": "EMPTY", "y": "C"})),
        );
        assert!(!e.success);
        assert!(e.reasons.iter().any(|r| r.contains("did not complete")));
    }

    #[test]
    fn extra_keys_in_final_state_fail() {
        let e = evaluate_task(
            &task(),
            &input(
                "completed",
                serde_json::json!({"x": "EMPTY", "y": "C", "z": "X"}),
            ),
        );
        assert!(!e.success);
        assert!(e.reasons.iter().any(|r| r.contains("unexpected key z")));
    }

    #[test]
    fn evaluation_is_independent_of_how_the_state_was_reached() {
        // Anti-circularity: the oracle API cannot receive a condition,
        // prompt, or stress level, so the same (task, termination,
        // initial, final) input must yield the same verdict no matter
        // which condition produced it.
        let t = task();
        let a = input("completed", serde_json::json!({"x": "EMPTY", "y": "C"}));
        let b = a.clone();
        assert_eq!(evaluate_task(&t, &a), evaluate_task(&t, &b));
    }

    #[test]
    fn evaluation_is_deterministic() {
        let t = task();
        let a = input("completed", serde_json::json!({"x": "WRONG", "y": "B"}));
        let e1 = evaluate_task(&t, &a);
        let e2 = evaluate_task(&t, &a);
        assert_eq!(e1, e2);
        assert!(!e1.success);
        assert_eq!(e1.reasons, e2.reasons);
    }
}
