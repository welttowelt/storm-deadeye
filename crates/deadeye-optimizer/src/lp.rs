//! Closed-form AMM f-position + LP claim-component math.
//!
//! Ports the closed-form helpers from `@the-situation/optimizer/lp.ts`.
//! See that file for the full economic derivation; the formulas here are
//! verbatim ports.

const PI: f64 = core::f64::consts::PI;

/// The AMM's f-position evaluated at outcome `x`.
///
/// `f(x; μ, σ, k) = k / √(σ · √π) · exp(-½ ((x-μ)/σ)²)`.
///
/// Returns `0` for degenerate inputs.
#[must_use]
pub fn f_at(x: f64, mu: f64, sigma: f64, k: f64) -> f64 {
    if sigma <= 0.0 || k <= 0.0 {
        return 0.0;
    }
    let z = (x - mu) / sigma;
    (k / (sigma * PI.sqrt()).sqrt()) * (-0.5 * z * z).exp()
}

/// Component of an LP's settlement claim, evaluated at outcome `x`.
///
/// `value(x) = pool_share · f(x; μ_entry, σ_entry, k_entry)`.
#[must_use]
pub fn compute_lp_claim_component_value(
    pool_share: f64,
    entry_mu: f64,
    entry_sigma: f64,
    entry_k: f64,
    x: f64,
) -> f64 {
    pool_share * f_at(x, entry_mu, entry_sigma, entry_k)
}

/// Sum of LP claim-component values across multiple deposits.
#[must_use]
pub fn compute_total_lp_claim_value(components: &[LpClaimComponent], x: f64) -> f64 {
    components
        .iter()
        .map(|c| {
            compute_lp_claim_component_value(c.pool_share, c.entry_mu, c.entry_sigma, c.entry_k, x)
        })
        .sum()
}

/// A single LP deposit's claim parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LpClaimComponent {
    /// Fraction of the pool this deposit owns.
    pub pool_share: f64,
    /// Market μ at deposit time.
    pub entry_mu: f64,
    /// Market σ at deposit time.
    pub entry_sigma: f64,
    /// AMM `k` at deposit time.
    pub entry_k: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f_at_peaks_at_mean() {
        let at_mean = f_at(100.0, 100.0, 2.0, 50.0);
        let off_mean = f_at(101.0, 100.0, 2.0, 50.0);
        assert!(at_mean > off_mean);
    }

    #[test]
    fn f_at_zero_for_degenerate_inputs() {
        assert!(f_at(100.0, 100.0, 0.0, 50.0).abs() < 1e-12);
        assert!(f_at(100.0, 100.0, 2.0, 0.0).abs() < 1e-12);
    }

    #[test]
    fn lp_claim_scales_with_pool_share() {
        let half = compute_lp_claim_component_value(0.5, 100.0, 2.0, 50.0, 100.0);
        let full = compute_lp_claim_component_value(1.0, 100.0, 2.0, 50.0, 100.0);
        assert!(2.0_f64.mul_add(-half, full).abs() < 1e-12);
    }

    #[test]
    fn total_lp_claim_is_sum() {
        let cs = vec![
            LpClaimComponent {
                pool_share: 0.3,
                entry_mu: 100.0,
                entry_sigma: 2.0,
                entry_k: 50.0,
            },
            LpClaimComponent {
                pool_share: 0.7,
                entry_mu: 99.0,
                entry_sigma: 2.5,
                entry_k: 50.0,
            },
        ];
        let total = compute_total_lp_claim_value(&cs, 100.0);
        let manual = compute_lp_claim_component_value(0.3, 100.0, 2.0, 50.0, 100.0)
            + compute_lp_claim_component_value(0.7, 99.0, 2.5, 50.0, 100.0);
        assert!((total - manual).abs() < 1e-12);
    }
}
