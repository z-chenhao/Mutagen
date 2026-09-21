//! Baseline-only, pre-registered difficulty calibration (spec §4–6,
//! §31–37).
//!
//! This module is the 0008-specific addition and the entire anti-leakage
//! surface of the experiment: the selector's only inputs are the
//! *family* and *baseline-only* calibration outcomes (spec §5). There is
//! no condition, candidate, prompt, or held-out argument anywhere in
//! this module — the candidate is structurally unable to influence
//! difficulty selection.
//!
//! The registered rules (pure, deterministic, unit-tested):
//!
//! 1. S0 sanity gate: reliable baseline success must be ≥ 4/5, else the
//!    family is `calibration_invalid` (a task that is unstable when
//!    reliable cannot support a clean difficulty reading).
//! 2. Headroom band: among S1..S4, only levels with exactly 2/5 or 3/5
//!    baseline success are selectable (intermediate regime near 50%;
//!    0/5, 1/5, 4/5, 5/5 are explicitly excluded).
//! 3. Selection: minimize |success_rate − 0.5|; 2/5 and 3/5 are equally
//!    distant, so ties break to the LOWER stress level.
//!
//! The result is a `DifficultySelection` manifest (frozen and committed
//! before Phase B; spec §36).

use serde::Deserialize;
use serde::Serialize;

/// The outcome of one *baseline-only* calibration episode.
///
/// Deliberately minimal: a stress level, a repetition, and the oracle
/// verdict. No candidate field exists to put one in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationOutcome {
    /// Registered stress level id ("S0".."S4").
    pub stress_id: String,
    /// Repetition 1..5.
    pub repetition: u32,
    pub oracle_success: bool,
}

/// The outcome of applying the pre-registered selection rule to one
/// family's calibration outcomes (spec §31–33).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionStatus {
    /// A stress level in the headroom band was selected.
    Calibrated,
    /// S0 sanity gate passed, but no S1..S4 level had 2/5 or 3/5
    /// baseline success.
    Uncalibrated,
    /// The reliable (S0) baseline itself was unstable (< 4/5).
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

/// One family's selection decision, including the per-stress success
/// counts the decision was made from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FamilySelection {
    pub family: String,
    pub s0_success: u32,
    pub s1_success: u32,
    pub s2_success: u32,
    pub s3_success: u32,
    pub s4_success: u32,
    pub s0_sanity_pass: bool,
    pub status: SelectionStatus,
    /// Selected stress id (None when the family did not calibrate).
    pub selected_stress: Option<String>,
    pub selection_reason: String,
}

/// Apply the pre-registered selection rule to one family.
///
/// The stress ladder is the registered S0..S4 (spec §19); `outcomes`
/// must contain the family's baseline-only calibration episodes (each
/// labeled with its stress id). The function is pure and total.
pub fn select_family_stress(family: &str, outcomes: &[CalibrationOutcome]) -> FamilySelection {
    let mut counts = [0u32; 5];
    for o in outcomes {
        if let Some(l) = crate::tools::StressLevel::by_id(&o.stress_id) {
            let i = l.drop_count as usize;
            if i < 5 && o.oracle_success {
                counts[i] += 1;
            }
        }
    }

    let base = FamilySelection {
        family: family.to_string(),
        s0_success: counts[0],
        s1_success: counts[1],
        s2_success: counts[2],
        s3_success: counts[3],
        s4_success: counts[4],
        s0_sanity_pass: counts[0] >= crate::trace::S0_MIN_SUCCESS,
        status: SelectionStatus::Calibrated,
        selected_stress: None,
        selection_reason: String::new(),
    };

    // 1. S0 sanity gate.
    if !base.s0_sanity_pass {
        return FamilySelection {
            status: SelectionStatus::CalibrationInvalid,
            selection_reason: format!(
                "s0_sanity_gate_failed: {} / {} baseline successes at S0 (required {})",
                counts[0],
                crate::trace::CALIBRATION_REPETITIONS,
                crate::trace::S0_MIN_SUCCESS
            ),
            ..base
        };
    }

    // 2. Headroom band: S1..S4 with success count in {2, 3}.
    let eligible: Vec<(usize, &crate::tools::StressLevel, u32)> = (1..5)
        .zip(crate::tools::STRESS_LEVELS.iter().skip(1))
        .filter(|(i, _)| {
            let c = counts[*i];
            crate::trace::HEADROOM_SUCCESS.contains(&c)
        })
        .map(|(i, l)| (i, l, counts[i]))
        .collect();

    if eligible.is_empty() {
        return FamilySelection {
            status: SelectionStatus::Uncalibrated,
            selection_reason: format!(
                "no_stress_level_in_headroom_band: S1..S4 baseline success counts = [{}, {}, {}, {}] (selectable band: 2 or 3 of {})",
                counts[1],
                counts[2],
                counts[3],
                counts[4],
                crate::trace::CALIBRATION_REPETITIONS
            ),
            ..base
        };
    }

    // 3. Minimize |rate - 0.5|; tie-break to the LOWER stress level.
    let mut best = eligible[0];
    for cand in eligible.iter().skip(1) {
        let best_dist = (base_rate(best.2) - 0.5).abs();
        let cand_dist = (base_rate(cand.2) - 0.5).abs();
        // Strictly closer, or equally close and lower index, wins.
        if cand_dist < best_dist || (best_dist == cand_dist && cand.0 < best.0) {
            best = *cand;
        }
    }
    let level = crate::tools::STRESS_LEVELS[best.0];
    FamilySelection {
        status: SelectionStatus::Calibrated,
        selected_stress: Some(level.id.to_string()),
        selection_reason: format!(
            "selected {}: {} / {} baseline success (min |rate - 0.5|, lower stress on ties; band = 2 or 3 of {})",
            level.id,
            best.2,
            crate::trace::CALIBRATION_REPETITIONS,
            crate::trace::CALIBRATION_REPETITIONS
        ),
        ..base
    }
}

fn base_rate(count: u32) -> f64 {
    count as f64 / crate::trace::CALIBRATION_REPETITIONS as f64
}

/// One family's entry in the frozen difficulty manifest (spec §36).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFamilyEntry {
    pub family: String,
    pub calibration_task: String,
    pub s0_success: u32,
    pub s1_success: u32,
    pub s2_success: u32,
    pub s3_success: u32,
    pub s4_success: u32,
    pub status: String,
    pub selected_stress: Option<String>,
    pub selection_reason: String,
}

/// The frozen difficulty selection manifest
/// (`selected-difficulty.json`), written at the end of Phase A and
/// committed before Phase B.
///
/// Anti-leakage: the manifest contains *no* candidate outcome, success,
/// cost, or trajectory field — the candidate was never executed in
/// Phase A. The prompt hashes are frozen *configuration* provenance,
/// not outcomes (spec §38).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DifficultySelection {
    pub experiment_id: String,
    pub calibration_code_commit: String,
    pub calibration_raw_sha256: String,
    pub baseline_prompt_sha256: String,
    pub repair_prompt_sha256: String,
    pub task_registry_sha256: String,
    pub stress_registry_sha256: String,
    pub per_family: Vec<ManifestFamilyEntry>,
    pub calibrated_family_count: u32,
    pub proceed_to_evaluation: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{CALIBRATION_REPETITIONS, FAMILIES};

    fn outcomes(
        pattern: &[(u32 /*drop_count*/, u32 /*success count*/)],
    ) -> Vec<CalibrationOutcome> {
        let mut out = Vec::new();
        for (drop, succ) in pattern {
            let level = crate::tools::StressLevel::by_index(*drop as usize).unwrap();
            for rep in 1..=CALIBRATION_REPETITIONS {
                out.push(CalibrationOutcome {
                    stress_id: level.id.to_string(),
                    repetition: rep,
                    oracle_success: rep <= *succ,
                });
            }
        }
        out
    }

    #[test]
    fn selects_the_lower_eligible_stress_on_equal_distance() {
        // S0 5/5; S1 3/5 and S2 2/5 are equally distant from 0.5 → S1
        // (lower stress) wins.
        let sel = select_family_stress(
            "direct_set",
            &outcomes(&[(0, 5), (1, 3), (2, 2), (3, 0), (4, 1)]),
        );
        assert_eq!(sel.status, SelectionStatus::Calibrated);
        assert_eq!(sel.selected_stress.as_deref(), Some("S1"));
        assert!(sel.s0_sanity_pass);
    }

    #[test]
    fn s0_below_gate_is_calibration_invalid_and_unselectable() {
        // S0 3/5 < 4/5: even a perfect S2 3/5 band cannot be selected.
        let sel = select_family_stress(
            "replacement",
            &outcomes(&[(0, 3), (1, 0), (2, 3), (3, 2), (4, 1)]),
        );
        assert_eq!(sel.status, SelectionStatus::CalibrationInvalid);
        assert_eq!(sel.selected_stress, None);
        assert!(sel.selection_reason.contains("s0_sanity_gate_failed"));
    }

    #[test]
    fn no_band_is_uncalibrated() {
        // S0 fine; S1..S4 all outside the {2,3} band.
        let sel = select_family_stress(
            "conditional_set",
            &outcomes(&[(0, 5), (1, 5), (2, 4), (3, 1), (4, 0)]),
        );
        assert_eq!(sel.status, SelectionStatus::Uncalibrated);
        assert_eq!(sel.selected_stress, None);
        assert!(
            sel.selection_reason
                .contains("no_stress_level_in_headroom_band")
        );
    }

    #[test]
    fn boundary_counts_0_1_4_5_are_never_selectable() {
        for excluded in [0u32, 1, 4, 5] {
            let sel = select_family_stress(
                "direct_set",
                &outcomes(&[(0, 5), (1, excluded), (2, 0), (3, 0), (4, 0)]),
            );
            assert_eq!(
                sel.status,
                SelectionStatus::Uncalibrated,
                "count {excluded} must not be selectable"
            );
        }
    }

    #[test]
    fn s0_exactly_4_is_a_passing_sanity_gate() {
        let sel = select_family_stress(
            "direct_set",
            &outcomes(&[(0, 4), (1, 0), (2, 3), (3, 0), (4, 0)]),
        );
        assert!(sel.s0_sanity_pass);
        assert_eq!(sel.status, SelectionStatus::Calibrated);
        assert_eq!(sel.selected_stress.as_deref(), Some("S2"));
    }

    #[test]
    fn counts_are_recorded_for_all_five_levels() {
        let sel = select_family_stress(
            "direct_set",
            &outcomes(&[(0, 5), (1, 4), (2, 3), (3, 3), (4, 2)]),
        );
        assert_eq!(
            (
                sel.s0_success,
                sel.s1_success,
                sel.s2_success,
                sel.s3_success,
                sel.s4_success
            ),
            (5, 4, 3, 3, 2)
        );
        // Two levels in the band (S2, S3, S4 actually three: 3,3,2) —
        // the lowest wins.
        assert_eq!(sel.selected_stress.as_deref(), Some("S2"));
    }

    #[test]
    fn selection_ignores_outcomes_of_other_families_by_construction() {
        // Outcomes carry a stress id, not a family id: the rule is
        // applied per family slice by the caller. Feeding the same
        // outcomes under a different family name must give the same
        // decision.
        let a = select_family_stress(
            FAMILIES[0],
            &outcomes(&[(0, 5), (1, 2), (2, 3), (3, 0), (4, 3)]),
        );
        let b = select_family_stress(
            FAMILIES[1],
            &outcomes(&[(0, 5), (1, 2), (2, 3), (3, 0), (4, 3)]),
        );
        assert_eq!(a.selected_stress, b.selected_stress);
        assert_eq!(a.status, b.status);
    }
}
