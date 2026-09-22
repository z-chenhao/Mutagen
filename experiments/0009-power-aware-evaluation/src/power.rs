//! Power-aware evaluation design constants and the conditional
//! detection-power computation (Experiment 0009, spec §29–31).
//!
//! Experiment 0008 calibrated difficulty *after* the pair budget was
//! fixed, and only discovered post-hoc that the two requirements were
//! mismatched (24 pairs needed 12 informative pairs, i.e. a 50% baseline
//! failure rate, at a 60%-baseline-success calibration target).
//!
//! Experiment 0009 sizes the design **backward from the downstream
//! information requirement**: the pair budget, the minimum informative
//! count, and the calibrated-failure capacity of the calibration rule
//! are all derived from the same information target and pre-registered
//! together.
//!
//! The *conditional* detection power below is a DESIGN ASSUMPTION used
//! only to size the evaluation. It is not evidence: it answers "if a
//! controlled repair wins each informative pair with probability θ, how
//! reliably does the registered test detect it at this size?" — it says
//! nothing about what the real repair will do.

/// Fixed Phase B paired-comparison budget (spec §26): 48 pairs no
/// matter whether two or three families calibrate.
pub const PLANNED_PAIRS: u32 = 48;

/// Minimum number of actually informative (discordant) pairs required
/// for a directional conclusion (spec §34).
pub const MIN_INFORMATIVE_PAIRS: u32 = 12;

/// Design assumption: among informative/discordant pairs, the
/// controlled repair is designed to win strongly rather than create
/// symmetric wins/losses (spec §29). Used ONLY to size the evaluation.
pub const DESIGN_DISCORDANT_WIN_PROBABILITY: f64 = 0.90;

/// Minimum conditional detection power required at the minimum
/// informative count (spec §30).
pub const MIN_CONDITIONAL_DETECTION_POWER: f64 = 0.85;

/// Minimum valid pairs after infrastructure exclusions (spec §35):
/// at least 44 of the planned 48, retaining more than 90% of the
/// planned paired design.
pub const MIN_VALID_PAIRS: u32 = 44;

/// Conditional detection power (spec §30).
///
/// Given exactly `n` informative pairs and assumed
/// `P(win | informative) = theta`, the probability that the registered
/// exact two-sided sign test (α = 0.05) both declares significance in
/// the winning direction and is significant:
///
/// ```text
/// Power(n, θ) = Σ_{w=0..n} I[ w > n−w  AND  p(w, n−w) ≤ α ]
///               × BinomialPMF(w; n, θ)
/// ```
///
/// where `p(w, l)` is the exact sign-test p-value from `stats`.
/// Computed, never hardcoded.
pub fn conditional_detection_power(n: u32, theta: f64) -> f64 {
    assert!((0.0..=1.0).contains(&theta), "theta must be a probability");
    let mut total = 0.0_f64;
    if theta >= 1.0 {
        return 1.0;
    }
    // BinomialPMF(0; n, θ) = (1 − θ)^n.
    let mut pmf = (1.0 - theta).powi(n as i32);
    for w in 0..=n {
        if w > 0 {
            // Update via the PMF ratio: P(w) = P(w−1)·(n−w+1)/w · θ/(1−θ).
            let ratio = (n - w + 1) as f64 / w as f64 * theta / (1.0 - theta);
            pmf *= ratio;
        }
        let wins = w as u64;
        let losses = (n - w) as u64;
        if wins > losses && crate::stats::exact_sign_test_p(wins, losses) <= crate::stats::ALPHA {
            total += pmf;
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn design_constants_are_registered() {
        assert_eq!(PLANNED_PAIRS, 48);
        assert_eq!(MIN_INFORMATIVE_PAIRS, 12);
        assert!((DESIGN_DISCORDANT_WIN_PROBABILITY - 0.90).abs() < 1e-12);
        assert!((MIN_CONDITIONAL_DETECTION_POWER - 0.85).abs() < 1e-12);
        assert_eq!(MIN_VALID_PAIRS, 44);
    }

    #[test]
    fn power_12_at_090_meets_the_minimum() {
        // Spec §30: compute Power(12, 0.90) and assert only the bound.
        // (The computed value is ≈ 0.8886; the spec's "~0.889" is an
        // approximation, not a target — assert the registered bound,
        // plus a loose sanity band so a broken computation fails.)
        let p =
            conditional_detection_power(MIN_INFORMATIVE_PAIRS, DESIGN_DISCORDANT_WIN_PROBABILITY);
        assert!(
            p >= MIN_CONDITIONAL_DETECTION_POWER,
            "Power(12, 0.90) = {p} is below the registered minimum {MIN_CONDITIONAL_DETECTION_POWER}"
        );
        assert!(
            (0.8..=0.95).contains(&p),
            "Power(12, 0.90) = {p} implausible"
        );
    }

    #[test]
    fn power_zero_theta_is_zero() {
        assert_eq!(conditional_detection_power(12, 0.0), 0.0);
    }

    #[test]
    fn power_one_theta_is_one() {
        // θ = 1: always all wins; the sign test at any n ≥ 1 is
        // significant in the winning direction.
        assert_eq!(conditional_detection_power(12, 1.0), 1.0);
        assert_eq!(conditional_detection_power(1, 1.0), 1.0);
    }

    #[test]
    fn power_half_theta_is_low_at_small_n() {
        // θ = 0.5: wins/losses symmetric; with 12 pairs the sign test
        // needs a ≥ 10–2 split to be significant, which under symmetry
        // has low probability.
        let p = conditional_detection_power(12, 0.5);
        assert!(p < MIN_CONDITIONAL_DETECTION_POWER);
        // Explicit check: at symmetry the only significant win region
        // for n=12 is w ≥ 10 (p(10,2) ≈ 0.0386 ≤ 0.05; p(9,3) ≈ 0.146
        // is not), so Power(12, 0.5) = (C(12,10)+C(12,11)+C(12,12))/2^12
        // = (66+12+1)/4096.
        let manual =
            (crate::stats::binom(12, 2) + crate::stats::binom(12, 1) + crate::stats::binom(12, 0))
                / 2f64.powi(12);
        assert!((p - manual).abs() < 1e-9, "p={p} manual={manual}");
    }

    #[test]
    fn power_is_monotone_in_n_at_design_theta() {
        let p12 = conditional_detection_power(12, 0.90);
        let p24 = conditional_detection_power(24, 0.90);
        let p48 = conditional_detection_power(48, 0.90);
        assert!(p12 <= p24 && p24 <= p48, "{p12} {p24} {p48}");
    }
}
