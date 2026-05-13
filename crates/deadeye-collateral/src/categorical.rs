//! Off-chain collateral for multinoulli (categorical) markets.
//!
//! For a discrete distribution the "minimum of `d(x)`" reduces to a
//! single-pass O(N) scan: the worst outcome is the one that minimises
//! `λ_g · g_i − λ_f · f_i`. No Newton-Raphson required.

use deadeye_core::CategoricalDistribution;

use crate::CollateralError;

/// Verified minimum of the discrete `d_i = λ_g · g_i − λ_f · f_i`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CategoricalVerifiedMinimum {
    /// Outcome index at which the minimum occurs.
    pub min_outcome_index: usize,
    /// `λ_f` used during the scan.
    pub lambda_f: f64,
    /// `λ_g` used during the scan.
    pub lambda_g: f64,
    /// Minimum value of the difference.
    pub min_difference: f64,
    /// Collateral required: `max(0, -min_difference)`.
    pub collateral: f64,
}

/// `||p||₂` for a categorical distribution.
#[must_use]
pub fn categorical_l2_norm(probs: &[f64]) -> f64 {
    let mut sum_sq = 0.0_f64;
    for &p in probs {
        sum_sq += p * p;
    }
    sum_sq.sqrt()
}

/// `λ = k / ||p||₂` for a categorical distribution.
#[must_use]
pub fn categorical_lambda(probs: &[f64], k: f64) -> f64 {
    let norm = categorical_l2_norm(probs);
    if norm <= 0.0 { 0.0 } else { k / norm }
}

/// Computes the collateral required to move the market from `f` to `g`.
///
/// `k` is the AMM scaling parameter from `get_params()`.
pub fn categorical_collateral(
    f: &CategoricalDistribution,
    g: &CategoricalDistribution,
    k: f64,
) -> Result<CategoricalVerifiedMinimum, CollateralError> {
    if f.outcome_count() != g.outcome_count() {
        return Err(CollateralError::Core(
            deadeye_core::CoreError::invalid_input("categorical", "outcome count mismatch"),
        ));
    }
    if f.outcome_count() == 0 {
        return Err(CollateralError::Core(
            deadeye_core::CoreError::invalid_input("categorical", "empty distribution"),
        ));
    }
    if f.is_identical(g) {
        return Ok(CategoricalVerifiedMinimum {
            min_outcome_index: 0,
            lambda_f: categorical_lambda(f.probs(), k),
            lambda_g: categorical_lambda(g.probs(), k),
            min_difference: 0.0,
            collateral: 0.0,
        });
    }

    let lambda_f = categorical_lambda(f.probs(), k);
    let lambda_g = categorical_lambda(g.probs(), k);

    let mut min_idx = 0_usize;
    let mut min_val = lambda_g.mul_add(g.prob(0), -(lambda_f * f.prob(0)));
    for i in 1..f.outcome_count() {
        let diff = lambda_g.mul_add(g.prob(i), -(lambda_f * f.prob(i)));
        if diff < min_val {
            min_val = diff;
            min_idx = i;
        }
    }

    let collateral = if min_val < 0.0 { -min_val } else { 0.0 };
    Ok(CategoricalVerifiedMinimum {
        min_outcome_index: min_idx,
        lambda_f,
        lambda_g,
        min_difference: min_val,
        collateral,
    })
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use deadeye_core::CategoricalDistribution;

    use super::*;

    #[test]
    fn uniform_l2_norm() {
        let probs = vec![0.25_f64; 4];
        // ||p||₂ = sqrt(4 * 1/16) = 0.5
        assert!((categorical_l2_norm(&probs) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn identical_distributions_zero_collateral() {
        let f = CategoricalDistribution::uniform(4).unwrap();
        let g = CategoricalDistribution::uniform(4).unwrap();
        let result = categorical_collateral(&f, &g, 1.0).unwrap();
        assert!(result.collateral.abs() < 1e-12);
    }

    #[test]
    fn shifted_distribution_positive_collateral() {
        let f = CategoricalDistribution::uniform(4).unwrap();
        let g = CategoricalDistribution::from_probs(vec![0.1, 0.1, 0.4, 0.4]).unwrap();
        let result = categorical_collateral(&f, &g, 1.0).unwrap();
        // outcomes 0,1 lost mass (0.25 → 0.1), so the min outcome is one of those
        assert!(result.collateral > 0.0);
        assert!(matches!(result.min_outcome_index, 0 | 1));
    }

    #[test]
    fn outcome_count_mismatch_errors() {
        let f = CategoricalDistribution::uniform(3).unwrap();
        let g = CategoricalDistribution::uniform(4).unwrap();
        let _err = categorical_collateral(&f, &g, 1.0).unwrap_err();
    }
}
