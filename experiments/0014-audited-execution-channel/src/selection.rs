//! Frozen deterministic candidate selection (carried over unchanged
//! from Experiment 0010 through 0014; the rule is frozen, not
//! experimental).
//!
//! The selection authority is deterministic Rust code that consumes only
//! the frozen artifact data (oracle outcomes on the common-valid
//! selection cells). The mutator / candidate model never participates:
//! candidate text cannot influence the selector except through observed
//! oracle outcomes (spec §61), and cost is never part of the score
//! (spec §62).
//!
//! Pre-registered lexicographic score per candidate (spec §38):
//!
//! ```text
//! (net_margin DESC, wins DESC, losses ASC, candidate ordinal ASC)
//! ```
//!
//! i.e. the conceptual tuple `(wins − losses, wins, −losses,
//! −candidate_ordinal)`, compared with all of the first three larger
//! and the ordinal smaller. No cost, tokens, rationale, prompt length,
//! or human judgment appears in this type at all.
//!
//! Incumbent retention (spec §39): a mutation displaces G0 ONLY if its
//! `net_margin > 0`. Otherwise the selection is G0 and the experiment
//! concludes `refuted` (a valid "do not evolve" outcome).
//!
//! The selection tournament is EXPLORATORY (spec §4): its metrics are
//! used only to choose one candidate; no confirmatory significance
//! claim is made from it. The `diagnostic_p` field is labeled
//! exploratory and is never used for the final claim.

use serde::Deserialize;
use serde::Serialize;

use crate::mutation::INCUMBENT_ID;

/// One candidate's exploratory tally on the common-valid selection
/// cells, from the perspective of the candidate against G0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateTally {
    pub candidate_id: String,
    /// 1-based harness-assigned ordinal (C1=1 .. C4=4).
    pub ordinal: u32,
    /// Candidate success + G0 failure.
    pub wins: u64,
    /// Candidate failure + G0 success.
    pub losses: u64,
    /// Same outcome.
    pub ties: u64,
    /// wins − losses.
    pub net_margin: i64,
    /// Exact sign-test p of (wins, losses). EXPLORATORY diagnostic only
    /// (spec §37): never used for the final claim.
    pub diagnostic_p_exploratory: f64,
}

/// The lexicographic selection key (spec §38): larger is better in
/// every component.
fn selection_key(t: &CandidateTally) -> (i64, i64, i64, i64) {
    (
        t.net_margin,
        t.wins as i64,
        -(t.losses as i64),
        -(t.ordinal as i64),
    )
}

/// The deterministic total order of the candidates (best first).
pub fn rank_candidates(tallies: &[CandidateTally]) -> Vec<&CandidateTally> {
    let mut ranked: Vec<&CandidateTally> = tallies.iter().collect();
    ranked.sort_by_key(|a| std::cmp::Reverse(selection_key(a)));
    ranked
}

/// The outcome of the frozen selection rule (spec §39).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectionOutcome {
    /// "G0" when the incumbent is retained, else the winning
    /// candidate ID (C1..C4).
    pub selected_candidate_id: String,
    pub incumbent_retained: bool,
    /// Deterministic audit string of the lexicographic path.
    pub tie_break_path: String,
}

impl SelectionOutcome {}

/// Apply the frozen selection rule to one candidate each, in
/// ordinal order (spec §38/§39).
pub fn select(tallies: &[CandidateTally]) -> SelectionOutcome {
    let ranked = rank_candidates(tallies);
    let path = ranked
        .iter()
        .map(|t| {
            format!(
                "{}(net={:+}d,wins={},losses={})",
                t.candidate_id, t.net_margin, t.wins, t.losses
            )
        })
        .collect::<Vec<_>>()
        .join(" > ");
    let best = ranked
        .first()
        .expect("at least one candidate is always tallied");
    let selected = if best.net_margin > 0 {
        let id = best.candidate_id.clone();
        (id, false)
    } else {
        (INCUMBENT_ID.to_string(), true)
    };
    SelectionOutcome {
        selected_candidate_id: selected.0,
        incumbent_retained: selected.1,
        tie_break_path: format!(
            "ranking {path}; best net margin {:+} → {}",
            best.net_margin,
            if best.net_margin > 0 {
                "mutation displaces G0"
            } else {
                "incumbent G0 retained (net margin not > 0)"
            }
        ),
    }
}

/// Diagnostic: the exact sign-test p for a tally (exploratory).
#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, ordinal: u32, wins: u64, losses: u64, ties: u64) -> CandidateTally {
        let net = wins as i64 - losses as i64;
        CandidateTally {
            candidate_id: id.into(),
            ordinal,
            wins,
            losses,
            ties,
            net_margin: net,
            diagnostic_p_exploratory: crate::stats::exact_sign_test_p(wins, losses),
        }
    }

    #[test]
    fn net_margin_dominates() {
        let a = t("C1", 1, 5, 4, 1); // net +1
        let b = t("C2", 2, 9, 1, 1); // net +8
        let arr = [a, b];
        let ranked = rank_candidates(&arr);
        assert_eq!(ranked[0].candidate_id, "C2");
    }

    #[test]
    fn wins_then_losses_then_ordinal() {
        let a = t("C1", 1, 3, 2, 0); // net +1, wins 3, losses 2
        let b = t("C2", 2, 3, 1, 0); // net +2 → wins on net
        let c = t("C3", 3, 3, 2, 0); // exact tie with a on (net,wins,losses)
        let d = t("C4", 4, 2, 2, 0); // net 0
        let arr = [c.clone(), d.clone(), a.clone(), b.clone()];
        let ranked = rank_candidates(&arr);
        assert_eq!(
            ranked
                .iter()
                .map(|x| x.candidate_id.clone())
                .collect::<Vec<_>>(),
            vec!["C2", "C1", "C3", "C4"]
        );
        // Exact (net,wins,losses) tie: lower ordinal wins.
        let sel = select(&[a, c]);
        assert_eq!(sel.selected_candidate_id, "C1");
        assert!(!sel.incumbent_retained);
    }

    #[test]
    fn g0_retained_when_no_positive_net() {
        let sel = select(&[
            t("C1", 1, 1, 1, 8),
            t("C2", 2, 0, 3, 5),
            t("C3", 3, 2, 2, 4),
            t("C4", 4, 0, 0, 9),
        ]);
        assert_eq!(sel.selected_candidate_id, INCUMBENT_ID);
        assert!(sel.incumbent_retained);
    }

    #[test]
    fn positive_net_displaces_g0() {
        let sel = select(&[
            t("C1", 1, 1, 1, 8),
            t("C2", 2, 0, 3, 5),
            t("C3", 3, 2, 2, 4),
            t("C4", 4, 5, 2, 8),
        ]);
        assert_eq!(sel.selected_candidate_id, "C4");
        assert!(!sel.incumbent_retained);
    }

    #[test]
    fn cost_is_not_a_field() {
        // Structural: the tally type has no cost/tokens/rationale
        // fields; the selection key cannot read them.
        let t = t("C1", 1, 2, 1, 5);
        let v = serde_json::to_value(&t).unwrap();
        for k in ["cost", "tokens", "wall_time", "rationale", "prompt_length"] {
            assert!(!v.get(k).is_some(), "{k} must not exist on the tally");
        }
    }
}
