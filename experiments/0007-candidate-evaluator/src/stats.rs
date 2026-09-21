//! Pre-registered statistics for Experiment 0007.
//!
//! The only test used is the exact two-sided sign test, implemented
//! with the standard library (spec: no statistics crate). Quality and
//! cost are reported independently: no composite `quality - λ·cost`
//! score exists anywhere in this experiment.

/// Sign-test significance level (pre-registered).
pub const ALPHA: f64 = 0.05;

/// Binomial coefficient C(n, k), computed in `f64`.
///
/// Experiment 0007 sizes (n ≤ 24 per comparison) are far below any
/// floating-point precision concern: every C(n, k) for n ≤ 64 is exact
/// in an `f64` mantissa.
pub fn binom(n: u64, k: u64) -> f64 {
    if k > n {
        return 0.0;
    }
    let k = k.min(n - k);
    let mut result = 1.0_f64;
    for i in 1..=k {
        result *= (n - i + 1) as f64 / i as f64;
    }
    result
}

/// Two-sided exact sign-test p-value for `wins` versus `losses`
/// non-tied pairs.
///
/// ```text
/// n = wins + losses
/// k = min(wins, losses)
/// p = min(1, 2 * Σ_{i=0..k} C(n, i) * 0.5^n)
/// ```
///
/// With zero non-tied pairs the p-value is 1 (no evidence either way).
pub fn exact_sign_test_p(wins: u64, losses: u64) -> f64 {
    let n = wins + losses;
    if n == 0 {
        return 1.0;
    }
    let k = wins.min(losses);
    let cdf: f64 = (0..=k).map(|i| binom(n, i)).sum::<f64>() * 2f64.powi(-(n as i32));
    (2.0 * cdf).min(1.0)
}

/// Pre-registered candidate quality classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityClass {
    Improvement,
    Regression,
    Inconclusive,
}

impl QualityClass {
    pub fn as_str(self) -> &'static str {
        match self {
            QualityClass::Improvement => "quality_improvement",
            QualityClass::Regression => "quality_regression",
            QualityClass::Inconclusive => "quality_inconclusive",
        }
    }
}

/// Classify one candidate against baseline from its win/loss counts.
///
/// Pre-registered:
/// - `quality_improvement`: wins > losses AND p ≤ 0.05
/// - `quality_regression`:  losses > wins AND p ≤ 0.05
/// - otherwise: `quality_inconclusive` (never "neutral")
pub fn classify_quality(wins: u64, losses: u64) -> QualityClass {
    let p = exact_sign_test_p(wins, losses);
    if wins > losses && p <= ALPHA {
        QualityClass::Improvement
    } else if losses > wins && p <= ALPHA {
        QualityClass::Regression
    } else {
        QualityClass::Inconclusive
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binom_known_values() {
        assert_eq!(binom(5, 2), 10.0);
        assert_eq!(binom(10, 3), 120.0);
        assert_eq!(binom(12, 4), 495.0);
        assert_eq!(binom(24, 12), 2_704_156.0);
        assert_eq!(binom(3, 5), 0.0);
        assert_eq!(binom(10, 10), 1.0);
    }

    #[test]
    fn sign_test_known_values() {
        // n=0: no evidence.
        assert_eq!(exact_sign_test_p(0, 0), 1.0);
        // n=1, one win: p = 2 * C(1,0) / 2 = 1.
        assert!((exact_sign_test_p(1, 0) - 1.0).abs() < 1e-12);
        // n=10, 7 wins 3 losses: 2 * (1 + 10 + 45 + 120) / 2^10
        // = 352 / 1024 = 0.34375.
        assert!((exact_sign_test_p(7, 3) - 0.34375).abs() < 1e-12);
        // n=10, 10 wins 0 losses: 2 / 2^10.
        assert!((exact_sign_test_p(10, 0) - 2.0 / 1024.0).abs() < 1e-12);
        // Symmetry.
        assert_eq!(exact_sign_test_p(7, 3), exact_sign_test_p(3, 7));
        // n=24 all one side: 2 / 2^24.
        let p = exact_sign_test_p(24, 0);
        assert!((p - 2.0 / 2.0f64.powi(24)).abs() < 1e-15);
        // Two-sided cap: heavily mixed results cannot exceed 1.
        assert!(exact_sign_test_p(12, 12) <= 1.0);
    }

    #[test]
    fn classification_improvement() {
        // 9 wins 1 loss, n=10: p = 2 * (1 + 10) / 2^10 ≈ 0.0215 ≤ 0.05.
        assert_eq!(classify_quality(9, 1), QualityClass::Improvement);
        // 6 wins 0 losses, n=6: p = 2/64 = 0.03125 ≤ 0.05.
        assert_eq!(classify_quality(6, 0), QualityClass::Improvement);
    }

    #[test]
    fn classification_regression() {
        assert_eq!(classify_quality(1, 9), QualityClass::Regression);
        assert_eq!(classify_quality(0, 6), QualityClass::Regression);
    }

    #[test]
    fn classification_inconclusive() {
        // Balanced: never "neutral", only "inconclusive".
        assert_eq!(classify_quality(5, 5), QualityClass::Inconclusive);
        // Directional but not significant: 4 wins 3 losses, n=7:
        // p = 2 * (1 + 7 + 21) / 2^7 = 58/128 = 0.4531 > 0.05.
        assert_eq!(classify_quality(4, 3), QualityClass::Inconclusive);
        // No non-tied pairs.
        assert_eq!(classify_quality(0, 0), QualityClass::Inconclusive);
    }

    #[test]
    fn class_strings_are_registered() {
        assert_eq!(QualityClass::Improvement.as_str(), "quality_improvement");
        assert_eq!(QualityClass::Regression.as_str(), "quality_regression");
        assert_eq!(QualityClass::Inconclusive.as_str(), "quality_inconclusive");
    }
}
