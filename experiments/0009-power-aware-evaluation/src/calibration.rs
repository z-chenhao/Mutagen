//! Baseline-only, pre-registered, *family-level, power-aware*
//! difficulty calibration (Experiment 0009, spec §4–6, §21–27).
//!
//! This module is the 0009-specific addition and the entire anti-leakage
//! surface of the experiment: the selector's only inputs are the
//! *family*, its registered development tasks, and *baseline-only*
//! development outcomes. There is no condition, candidate, prompt, or
//! held-out argument anywhere in this module — the candidate is
//! structurally unable to influence difficulty selection (spec §5).
//!
//! The registered rules (pure, deterministic, unit-tested):
//!
//! 1. S0 sanity gate: BOTH development tasks must satisfy S0 baseline
//!    success ≥ 4/5, else the family is `calibration_invalid` (a task
//!    that is unstable when reliable cannot support a clean difficulty
//!    reading, and one bad development variant must disqualify the
//!    whole family).
//! 2. Failure-headroom eligibility: a stress S1..S4 is eligible iff
//!    BOTH development variants independently show baseline success ≤ 2/5
//!    (≤ 40%) at that stress. There is NO lower success bound: 0/5 is
//!    eligible. The rule targets *sufficient baseline-failure capacity*,
//!    not "roughly 50% difficulty" (spec §22, §24).
//! 3. Selection: among eligible stresses, the LOWEST stress level
//!    (smallest drop_count) wins. Minimum intervention across BOTH
//!    development variants; no human judgment (spec §23).
//!
//! The result feeds the frozen `EvaluationPlan` manifest
//! (`evaluation-plan.json`), committed before Phase B.
//!
//! The power design constants (`power.rs`) size the Phase B budget
//! *backward* from the downstream information requirement, and the plan
//! carries them so the budget and the calibration rule are frozen
//! together (spec §29, §45).

use serde::Deserialize;
use serde::Serialize;

#[cfg(test)]
use crate::power::{
    DESIGN_DISCORDANT_WIN_PROBABILITY, MIN_CONDITIONAL_DETECTION_POWER, MIN_INFORMATIVE_PAIRS,
    MIN_VALID_PAIRS, PLANNED_PAIRS,
};

/// The outcome of one *baseline-only* development episode.
///
/// Deliberately minimal: a task id, a stress level, a repetition, and
/// the oracle verdict. No candidate field exists to put one in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineDevelopmentOutcome {
    /// Registered development task id ("D1".."D6").
    pub task_id: String,
    /// Registered stress level id ("S0".."S4").
    pub stress_id: String,
    /// Repetition 1..5.
    pub repetition: u32,
    pub oracle_success: bool,
}

/// The outcome of applying the pre-registered selection rule to one
/// family's development outcomes (spec §21–23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionStatus {
    /// A stress level was selected.
    Calibrated,
    /// S0 sanity passed, but no S1..S4 stress satisfied both development
    /// variants' ≤ 40% headroom requirement.
    Uncalibrated,
    /// The reliable (S0) baseline was unstable on at least one
    /// development variant (< 4/5).
    CalibrationInvalid,
}

impl SelectionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SelectionStatus::Calibrated => "calibrated",
            SelectionStatus::Uncalibrated => "uncalibrated",
            SelectionStatus::CalibrationInvalid => "calibration_invalid",
        }
    }
}

/// Baseline success counts of one development task across the registered
/// S0..S4 ladder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStressCounts {
    pub task_id: String,
    pub s0_success: u32,
    pub s1_success: u32,
    pub s2_success: u32,
    pub s3_success: u32,
    pub s4_success: u32,
}

impl TaskStressCounts {
    pub fn count_at(&self, drop_count: u32) -> u32 {
        match drop_count {
            0 => self.s0_success,
            1 => self.s1_success,
            2 => self.s2_success,
            3 => self.s3_success,
            _ => self.s4_success,
        }
    }
}

/// One family's selection decision (spec §45 `per_family` content).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FamilySelection {
    pub family: String,
    /// The family's registered development task ids, in registry order.
    pub development_task_ids: Vec<String>,
    /// Per development task: S0..S4 baseline success counts.
    pub per_task_per_stress_success_counts: Vec<TaskStressCounts>,
    /// Both development tasks passed the S0 sanity gate (≥ 4/5 each).
    pub s0_sanity_pass: bool,
    pub status: SelectionStatus,
    /// Selected stress id (None when the family did not calibrate).
    pub selected_stress: Option<String>,
    pub selection_reason: String,
}

/// Apply the pre-registered, family-level, power-aware selection rule
/// to one family.
///
/// The stress ladder is the registered S0..S4; `development_task_ids`
/// is the family's registered development task set; `outcomes` must
/// contain exactly that family's baseline-only development episodes
/// (labeled by task, stress, repetition). The function is pure and
/// total, and it is baseline-only by construction: the candidate was
/// never executed in Phase A, so no candidate data can be an input.
///
/// Registered rule:
/// - `s0_sanity_pass` = every development task has S0 success ≥ 4/5;
///   else `calibration_invalid` (nothing selected).
/// - eligible = S1..S4 stresses where EVERY development task has success
///   ≤ 2/5 (≤ 40%);
/// - selected = the lowest eligible stress (smallest drop_count);
///   none → `uncalibrated`.
pub fn select_family_stress(
    family: &str,
    development_task_ids: &[String],
    outcomes: &[BaselineDevelopmentOutcome],
) -> FamilySelection {
    let mut counts: Vec<TaskStressCounts> = development_task_ids
        .iter()
        .map(|id| TaskStressCounts {
            task_id: id.clone(),
            s0_success: 0,
            s1_success: 0,
            s2_success: 0,
            s3_success: 0,
            s4_success: 0,
        })
        .collect();
    for o in outcomes {
        let Some(task_index) = counts.iter().position(|c| c.task_id == o.task_id) else {
            continue; // outcomes of other tasks are not this family's
        };
        let Some(level) = crate::tools::StressLevel::by_id(&o.stress_id) else {
            continue;
        };
        if o.oracle_success {
            match level.drop_count {
                0 => counts[task_index].s0_success += 1,
                1 => counts[task_index].s1_success += 1,
                2 => counts[task_index].s2_success += 1,
                3 => counts[task_index].s3_success += 1,
                _ => counts[task_index].s4_success += 1,
            }
        }
    }

    let reps = crate::trace::CALIBRATION_REPETITIONS;
    let s0_pass = counts
        .iter()
        .all(|c| c.s0_success >= crate::trace::S0_MIN_SUCCESS_PER_TASK);

    // 1. S0 sanity gate: BOTH development variants must be reliable.
    if !s0_pass {
        let reason = format!(
            "s0_sanity_gate_failed: per-task S0 success {} (each requires {}/{}); a family calibrates only when every development variant is reliable",
            counts
                .iter()
                .map(|c| format!("{}/{}", c.s0_success, reps))
                .collect::<Vec<_>>()
                .join(", "),
            crate::trace::S0_MIN_SUCCESS_PER_TASK,
            reps
        );
        return FamilySelection {
            family: family.to_string(),
            development_task_ids: development_task_ids.to_vec(),
            per_task_per_stress_success_counts: counts,
            s0_sanity_pass: false,
            status: SelectionStatus::CalibrationInvalid,
            selected_stress: None,
            selection_reason: reason,
        };
    }

    // 2. Failure-headroom eligibility: EVERY development variant must
    //    show baseline success ≤ 2/5 (≤ 40%) at the same stress.
    //    (computed before `counts` moves into the result)
    let mut eligible: Vec<(u32 /* drop_count */, &crate::tools::StressLevel)> = Vec::new();
    for (i, level) in crate::tools::STRESS_LEVELS.iter().enumerate() {
        if i == 0 {
            continue; // S0 is the sanity control, never a selected stress
        }
        let dc = level.drop_count;
        if counts
            .iter()
            .all(|c| c.count_at(dc) <= crate::trace::MAX_DEV_SUCCESS_FOR_ELIGIBILITY)
        {
            eligible.push((dc, level));
        }
    }

    if eligible.is_empty() {
        let reason = format!(
            "no_stress_level_within_headroom: per-task S1..S4 success = {} (every task must be ≤ {}/{} at the same stress)",
            (1..=4)
                .map(|dc| {
                    counts
                        .iter()
                        .map(|c| c.count_at(dc).to_string())
                        .collect::<Vec<_>>()
                        .join("/")
                })
                .collect::<Vec<_>>()
                .join(", "),
            crate::trace::MAX_DEV_SUCCESS_FOR_ELIGIBILITY,
            reps
        );
        return FamilySelection {
            family: family.to_string(),
            development_task_ids: development_task_ids.to_vec(),
            per_task_per_stress_success_counts: counts,
            s0_sanity_pass: true,
            status: SelectionStatus::Uncalibrated,
            selected_stress: None,
            selection_reason: reason,
        };
    }

    // 3. Lowest eligible stress (smallest drop_count): minimum
    //    intervention that satisfies the pre-registered information
    //    target across BOTH development variants.
    let (dc, level) = eligible
        .iter()
        .min_by_key(|(dc, _)| *dc)
        .copied()
        .unwrap_or_else(|| panic!("eligible is non-empty"));
    let reason = format!(
        "selected {}: per-task baseline success at {} = {} (lowest stress at which every development variant is ≤ {}/{}; minimum intervention satisfies the pre-registered information target)",
        level.id,
        level.id,
        per_task_success_reason(&counts, dc),
        crate::trace::MAX_DEV_SUCCESS_FOR_ELIGIBILITY,
        reps
    );
    FamilySelection {
        family: family.to_string(),
        development_task_ids: development_task_ids.to_vec(),
        per_task_per_stress_success_counts: counts,
        s0_sanity_pass: true,
        status: SelectionStatus::Calibrated,
        selected_stress: Some(level.id.to_string()),
        selection_reason: reason,
    }
}

/// Render the per-task success counts at one stress level as
/// `"a/b (task), c/d (task)"`.
fn per_task_success_reason(counts: &[TaskStressCounts], dc: u32) -> String {
    counts
        .iter()
        .map(|c| {
            format!(
                "{}/{} ({})",
                c.count_at(dc),
                crate::trace::CALIBRATION_REPETITIONS,
                c.task_id
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// The frozen evaluation plan (spec §45–46)
// ===========================================================================

/// One family's entry in the frozen evaluation plan.
///
/// Anti-leakage (spec §46): the entry contains ONLY calibration-derived
/// data (development task ids, per-stress baseline success counts, the
/// selection decision). No repair outcome, no held-out outcome, and no
/// held-out result of any kind may appear here — the candidate was never
/// executed and the held-out tasks have no outcomes yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanFamilyEntry {
    pub family: String,
    pub development_task_ids: Vec<String>,
    pub per_task_per_stress_success_counts: Vec<TaskStressCounts>,
    pub s0_sanity_pass: bool,
    /// "calibrated" | "uncalibrated" | "calibration_invalid"
    pub selection_status: String,
    pub selected_stress: Option<String>,
    pub selection_reason: String,
}

/// The frozen power-aware evaluation plan (`evaluation-plan.json`),
/// written at the end of Phase A and committed before Phase B.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationPlan {
    pub experiment_id: String,
    /// Calibration code-under-test commit (40-hex).
    pub calibration_code_commit: String,
    /// SHA-256 of the immutable Phase A raw artifact bytes.
    pub calibration_raw_sha256: String,
    /// Frozen configuration provenance (the repair prompt hash is
    /// committed BEFORE any candidate execution; it influences nothing).
    pub baseline_prompt_sha256: String,
    pub repair_prompt_sha256: String,
    pub task_registry_sha256: String,
    pub stress_registry_sha256: String,
    pub per_family: Vec<PlanFamilyEntry>,
    /// Selected families, in registry order.
    pub selected_families: Vec<String>,
    pub calibrated_family_count: u32,
    /// Fixed Phase B budget (spec §26): 48 pairs regardless of whether
    /// two or three families calibrate.
    pub planned_pairs: u32,
    /// The selected families' registered held-out tasks, in registry
    /// order. Known specifications only — no held-out outcomes exist.
    pub heldout_tasks: Vec<String>,
    /// Machine-derived repetitions per held-out task: 12 if exactly two
    /// families calibrated, 8 if three; the product with the held-out
    /// task count is exactly `planned_pairs` (spec §27).
    pub repetitions_per_task: u32,
    // Pre-registered information gates (frozen with the design).
    pub min_valid_pairs: u32,
    pub min_potential_information: u32,
    pub min_actual_informative: u32,
    /// The design assumption and its computed consequence (spec §29–30).
    pub design_discordant_win_probability: f64,
    pub conditional_power_at_min_informative: f64,
    /// true iff ≥ 2 families calibrated; Phase B may run only then.
    pub proceed_to_evaluation: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{CALIBRATION_REPETITIONS, FAMILIES};

    fn dev(task_a: &str, task_b: &str) -> Vec<String> {
        vec![task_a.to_string(), task_b.to_string()]
    }

    /// Build baseline-only outcomes for one (task, drop_count) cell
    /// with `success` successful repetitions (first `success` of 5).
    fn outcomes(
        pattern: &[(
            &str, /*task*/
            u32,  /*drop_count*/
            u32,  /*success count*/
        )],
    ) -> Vec<BaselineDevelopmentOutcome> {
        let mut out = Vec::new();
        for (task, drop, succ) in pattern {
            let level = crate::tools::StressLevel::by_index(*drop as usize).unwrap();
            for rep in 1..=CALIBRATION_REPETITIONS {
                out.push(BaselineDevelopmentOutcome {
                    task_id: task.to_string(),
                    stress_id: level.id.to_string(),
                    repetition: rep,
                    oracle_success: rep <= *succ,
                });
            }
        }
        out
    }

    #[test]
    fn both_variants_must_show_headroom_single_task_is_enough_for_neither() {
        // S1: task A 2/5 (ok), task B 3/5 (NOT ok) → S1 ineligible.
        // S2: A 2/5, B 2/5 → eligible and lowest → select S2.
        let sel = select_family_stress(
            "direct_set",
            &dev("D1", "D2"),
            &outcomes(&[
                ("D1", 0, 5),
                ("D1", 1, 2),
                ("D1", 2, 2),
                ("D1", 3, 1),
                ("D1", 4, 0),
                ("D2", 0, 5),
                ("D2", 1, 3),
                ("D2", 2, 2),
                ("D2", 3, 0),
                ("D2", 4, 0),
            ]),
        );
        assert_eq!(sel.status, SelectionStatus::Calibrated);
        assert_eq!(sel.selected_stress.as_deref(), Some("S2"));
        assert!(sel.s0_sanity_pass);
    }

    #[test]
    fn zero_success_is_eligible_no_lower_bound() {
        // 0/5 is explicitly allowed (spec §22): a hard task is not
        // information-poor for improvement detection.
        let sel = select_family_stress(
            "conditional_set",
            &dev("D3", "D4"),
            &outcomes(&[
                ("D3", 0, 5),
                ("D3", 1, 0),
                ("D3", 2, 0),
                ("D3", 3, 0),
                ("D3", 4, 0),
                ("D4", 0, 5),
                ("D4", 1, 1),
                ("D4", 2, 0),
                ("D4", 3, 0),
                ("D4", 4, 0),
            ]),
        );
        assert_eq!(sel.status, SelectionStatus::Calibrated);
        assert_eq!(sel.selected_stress.as_deref(), Some("S1"));
    }

    #[test]
    fn selects_the_lowest_eligible_stress() {
        let sel = select_family_stress(
            "direct_set",
            &dev("D1", "D2"),
            &outcomes(&[
                ("D1", 0, 5),
                ("D1", 1, 2),
                ("D1", 2, 2),
                ("D1", 3, 2),
                ("D1", 4, 2),
                ("D2", 0, 5),
                ("D2", 1, 0),
                ("D2", 2, 1),
                ("D2", 3, 1),
                ("D2", 4, 1),
            ]),
        );
        assert_eq!(sel.status, SelectionStatus::Calibrated);
        assert_eq!(sel.selected_stress.as_deref(), Some("S1"));
    }

    #[test]
    fn s0_below_gate_on_either_variant_is_calibration_invalid() {
        // Task A S0 5/5, task B S0 3/5 → family invalid even if A is
        // perfect at every other level.
        let sel = select_family_stress(
            "replacement",
            &dev("D5", "D6"),
            &outcomes(&[
                ("D5", 0, 5),
                ("D5", 1, 0),
                ("D5", 2, 0),
                ("D5", 3, 0),
                ("D5", 4, 0),
                ("D6", 0, 3),
                ("D6", 1, 0),
                ("D6", 2, 0),
                ("D6", 3, 0),
                ("D6", 4, 0),
            ]),
        );
        assert_eq!(sel.status, SelectionStatus::CalibrationInvalid);
        assert_eq!(sel.selected_stress, None);
        assert!(!sel.s0_sanity_pass);
        assert!(sel.selection_reason.contains("s0_sanity_gate_failed"));
    }

    #[test]
    fn s0_exactly_4_on_both_variants_passes() {
        let sel = select_family_stress(
            "direct_set",
            &dev("D1", "D2"),
            &outcomes(&[
                ("D1", 0, 4),
                ("D1", 1, 0),
                ("D1", 2, 0),
                ("D1", 3, 0),
                ("D1", 4, 0),
                ("D2", 0, 4),
                ("D2", 1, 1),
                ("D2", 2, 0),
                ("D2", 3, 0),
                ("D2", 4, 0),
            ]),
        );
        assert!(sel.s0_sanity_pass);
        assert_eq!(sel.status, SelectionStatus::Calibrated);
        assert_eq!(sel.selected_stress.as_deref(), Some("S1"));
    }

    #[test]
    fn no_headroom_on_both_variants_is_uncalibrated() {
        // Every S1..S4 has at least one variant above 40% baseline
        // success → nothing selectable.
        let sel = select_family_stress(
            FAMILIES[1],
            &dev("D3", "D4"),
            &outcomes(&[
                ("D3", 0, 5),
                ("D3", 1, 5),
                ("D3", 2, 4),
                ("D3", 3, 4),
                ("D3", 4, 3),
                ("D4", 0, 5),
                ("D4", 1, 5),
                ("D4", 2, 5),
                ("D4", 3, 4),
                ("D4", 4, 4),
            ]),
        );
        assert_eq!(sel.status, SelectionStatus::Uncalibrated);
        assert_eq!(sel.selected_stress, None);
        assert!(
            sel.selection_reason
                .contains("no_stress_level_within_headroom")
        );
    }

    #[test]
    fn a_stress_passing_only_one_variant_is_never_selected() {
        // S1: A 0/5, B 5/5. S2: A 2/5, B 2/5. S3: A 5/5, B 0/5.
        // The family must pick S2, never S1 or S3, because selection
        // uses BOTH development variants (spec §23) — the 0008 failure
        // mode was calibrating on a single task whose difficulty did not
        // transfer.
        let sel = select_family_stress(
            "direct_set",
            &dev("D1", "D2"),
            &outcomes(&[
                ("D1", 0, 5),
                ("D1", 1, 0),
                ("D1", 2, 2),
                ("D1", 3, 5),
                ("D1", 4, 2),
                ("D2", 0, 5),
                ("D2", 1, 5),
                ("D2", 2, 2),
                ("D2", 3, 0),
                ("D2", 4, 2),
            ]),
        );
        assert_eq!(sel.status, SelectionStatus::Calibrated);
        assert_eq!(sel.selected_stress.as_deref(), Some("S2"));
    }

    #[test]
    fn per_task_counts_are_recorded() {
        let sel = select_family_stress(
            "direct_set",
            &dev("D1", "D2"),
            &outcomes(&[
                ("D1", 0, 5),
                ("D1", 1, 4),
                ("D1", 2, 2),
                ("D1", 3, 1),
                ("D1", 4, 0),
                ("D2", 0, 4),
                ("D2", 1, 2),
                ("D2", 2, 1),
                ("D2", 3, 0),
                ("D2", 4, 0),
            ]),
        );
        let counts = &sel.per_task_per_stress_success_counts;
        assert_eq!(counts.len(), 2);
        let (a, b) = (&counts[0], &counts[1]);
        assert_eq!(
            (
                a.s0_success,
                a.s1_success,
                a.s2_success,
                a.s3_success,
                a.s4_success
            ),
            (5, 4, 2, 1, 0)
        );
        assert_eq!(
            (
                b.s0_success,
                b.s1_success,
                b.s2_success,
                b.s3_success,
                b.s4_success
            ),
            (4, 2, 1, 0, 0)
        );
    }

    #[test]
    fn evaluation_plan_shape_is_fixed() {
        let plan = EvaluationPlan {
            experiment_id: "0009".into(),
            calibration_code_commit: "0".repeat(40),
            calibration_raw_sha256: "a".repeat(64),
            baseline_prompt_sha256: "1".repeat(64),
            repair_prompt_sha256: "2".repeat(64),
            task_registry_sha256: "3".repeat(64),
            stress_registry_sha256: "4".repeat(64),
            per_family: Vec::new(),
            selected_families: Vec::new(),
            calibrated_family_count: 0,
            planned_pairs: PLANNED_PAIRS,
            heldout_tasks: Vec::new(),
            repetitions_per_task: 0,
            min_valid_pairs: MIN_VALID_PAIRS,
            min_potential_information: MIN_INFORMATIVE_PAIRS,
            min_actual_informative: MIN_INFORMATIVE_PAIRS,
            design_discordant_win_probability: DESIGN_DISCORDANT_WIN_PROBABILITY,
            conditional_power_at_min_informative: MIN_CONDITIONAL_DETECTION_POWER,
            proceed_to_evaluation: false,
        };
        assert_eq!(plan.planned_pairs, 48);
        assert_eq!(plan.min_valid_pairs, 44);
        assert_eq!(plan.min_potential_information, 12);
        assert_eq!(plan.min_actual_informative, 12);
    }
}
