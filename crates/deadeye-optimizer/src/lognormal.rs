//! Lognormal trade optimizer — the log-space twin of [`crate::normal`].
//!
//! A lognormal market stores its distribution as `(μ, σ)` in **log space**
//! (the underlying `ln x` is `N(μ, σ²)`), and every constraint the normal
//! optimizer applies in outcome space applies here in log space: candidate σ
//! within `max_sigma_ratio` of the market σ, candidate μ within
//! `max_mean_sep_sigmas · σ_market` of the market μ, both *per trade* (the
//! chain rejects bigger single moves — reach further targets with a ladder
//! of sequential trades).
//!
//! Objective and budget semantics mirror the normal optimizer (issue #12
//! lineage): maximize **expected P&L** under the trader's belief over the
//! budget-feasible policy region; collateral is a margin lock, not a cost.
//!
//! Collateral per candidate comes from the audited Newton minimiser
//! [`deadeye_collateral::lognormal::lognormal_collateral`] (chain side-law
//! clamps included), scaled by `effective_k`. The expected value uses the
//! closed form
//!
//! ```text
//! E_{x~LN(μb,σb)}[LN_i(x)] = N(μi − μb; 0, σi² + σb²) · exp(σc²/2 − μc)
//!   where 1/σc² = 1/σi² + 1/σb²,  μc = σc²·(μi/σi² + μb/σb²)
//! ```
//!
//! (substitute `y = ln x`; the extra `e^{-y}` from the two `1/x` Jacobians
//! against `dx = e^y dy` integrates to the `exp(σc²/2 − μc)` factor).

use deadeye_collateral::lognormal::{
    LognormalOptions, lognormal_collateral, lognormal_lambda,
};
use deadeye_core::{LognormalDistribution, Sq128};

use crate::normal::OptimizerConstraints;

const N_SIGMA_SAMPLES: u32 = 40;
const N_MEAN_SAMPLES: u32 = 40;

/// Inputs to [`optimize_lognormal_trade`]. All `μ`/`σ` are **log-space**.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LognormalOptimizationInput {
    /// Trader's budget in collateral tokens.
    pub budget: f64,
    /// Trader's belief μ (log space).
    pub belief_mu: f64,
    /// Trader's belief σ (log space).
    pub belief_sigma: f64,
    /// Current market μ (log space).
    pub market_mu: f64,
    /// Current market σ (log space).
    pub market_sigma: f64,
    /// AMM `effective_k` (post LP scaling).
    pub effective_k: f64,
    /// Policy bounds (same semantics as the normal optimizer, in log space).
    pub constraints: OptimizerConstraints,
}

impl LognormalOptimizationInput {
    /// Convenience constructor with default constraints.
    #[must_use]
    pub fn new(
        budget: f64,
        belief_mu: f64,
        belief_sigma: f64,
        market_mu: f64,
        market_sigma: f64,
        effective_k: f64,
    ) -> Self {
        Self {
            budget,
            belief_mu,
            belief_sigma,
            market_mu,
            market_sigma,
            effective_k,
            constraints: OptimizerConstraints::default(),
        }
    }
}

/// Output of [`optimize_lognormal_trade`]. `μ`/`σ` are log-space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LognormalOptimizationResult {
    /// Optimised target μ (log space).
    pub optimized_mu: f64,
    /// Optimised target σ (log space).
    pub optimized_sigma: f64,
    /// `optimized_sigma²`.
    pub optimized_variance: f64,
    /// Collateral required to enter the optimised trade (tokens).
    pub collateral_required: f64,
    /// Audited stationary point `x*` of the chosen candidate (outcome space).
    pub x_star: f64,
    /// Gross expected value under the trader's belief (tokens).
    pub expected_value: f64,
    /// Fraction of the intended μ shift the trade expresses (0–1).
    pub belief_utilization: f64,
    /// `true` iff the optimum sits at (≈) the full belief point.
    pub is_budget_sufficient: bool,
    /// `budget − collateral_required`.
    pub budget_surplus: f64,
    /// Expected return on the locked collateral: `EV / collateral`.
    pub roi: f64,
}

/// `E_{x ~ LN(belief)}[LN(x; μ, σ)]` — closed form (see module docs).
#[must_use]
pub fn lognormal_cross_expectation(mu: f64, sigma: f64, belief_mu: f64, belief_sigma: f64) -> f64 {
    if sigma <= 0.0 || belief_sigma <= 0.0 {
        return 0.0;
    }
    let var_sum = sigma.mul_add(sigma, belief_sigma * belief_sigma);
    let diff = mu - belief_mu;
    let gaussian = (-0.5 * diff * diff / var_sum).exp()
        / (2.0 * core::f64::consts::PI * var_sum).sqrt();
    let combined_var = 1.0 / (1.0 / (sigma * sigma) + 1.0 / (belief_sigma * belief_sigma));
    let combined_mu =
        combined_var * (mu / (sigma * sigma) + belief_mu / (belief_sigma * belief_sigma));
    gaussian * (combined_var / 2.0 - combined_mu).exp()
}

/// Expected P&L (tokens) of moving the market `f → g` under the belief.
fn expected_value(
    market_mu: f64,
    market_sigma: f64,
    cand_mu: f64,
    cand_sigma: f64,
    k: f64,
    belief_mu: f64,
    belief_sigma: f64,
) -> f64 {
    let lam_g = lognormal_lambda(cand_mu, cand_sigma * cand_sigma, k);
    let lam_f = lognormal_lambda(market_mu, market_sigma * market_sigma, k);
    lam_g.mul_add(
        lognormal_cross_expectation(cand_mu, cand_sigma, belief_mu, belief_sigma),
        -(lam_f * lognormal_cross_expectation(market_mu, market_sigma, belief_mu, belief_sigma)),
    )
}

/// Collateral (tokens) for the move `f → g` at `effective_k`, plus the
/// audited `x*`. The Newton minimiser runs with unit-`k` λs; collateral
/// scales linearly in `k` since both λs share it.
fn collateral_and_x_star(
    market_mu: f64,
    market_sigma: f64,
    cand_mu: f64,
    cand_sigma: f64,
    effective_k: f64,
) -> Option<(f64, f64)> {
    let f = LognormalDistribution::from_variance(
        Sq128::from_f64(market_mu).ok()?,
        Sq128::from_f64(market_sigma * market_sigma).ok()?,
    )
    .ok()?;
    let g = LognormalDistribution::from_variance(
        Sq128::from_f64(cand_mu).ok()?,
        Sq128::from_f64(cand_sigma * cand_sigma).ok()?,
    )
    .ok()?;
    let verified = lognormal_collateral(&f, &g, LognormalOptions::default()).ok()?;
    if !verified.collateral.is_finite() {
        return None;
    }
    Some((verified.collateral * effective_k, verified.x_star))
}

/// Picks the highest-EV lognormal trade in the policy region. See module docs
/// for spaces and semantics.
#[must_use]
pub fn optimize_lognormal_trade(input: LognormalOptimizationInput) -> LognormalOptimizationResult {
    let no_trade = LognormalOptimizationResult {
        optimized_mu: input.market_mu,
        optimized_sigma: input.market_sigma,
        optimized_variance: input.market_sigma * input.market_sigma,
        collateral_required: 0.0,
        x_star: input.market_mu.exp(),
        expected_value: 0.0,
        belief_utilization: 0.0,
        is_budget_sufficient: false,
        budget_surplus: input.budget,
        roi: 0.0,
    };

    if input.budget <= 0.0
        || input.market_sigma <= 0.0
        || input.belief_sigma <= 0.0
        || input.effective_k <= 0.0
    {
        return no_trade;
    }

    let sigma_min = (input.market_sigma / input.constraints.max_sigma_ratio).max(1e-6_f64);
    let sigma_max = input.market_sigma * input.constraints.max_sigma_ratio;
    let sigma_step = (sigma_max - sigma_min) / f64::from(N_SIGMA_SAMPLES);

    let mean_dir = if input.belief_mu >= input.market_mu {
        1.0_f64
    } else {
        -1.0_f64
    };
    let max_shift = input.constraints.max_mean_sep_sigmas * input.market_sigma;

    let mut best_mu = input.market_mu;
    let mut best_sigma = input.market_sigma;
    let mut best_coll = 0.0_f64;
    let mut best_x_star = input.market_mu.exp();
    let mut best_ev = 0.0_f64;

    for i in 0..=N_SIGMA_SAMPLES {
        let cand_sigma = f64::from(i).mul_add(sigma_step, sigma_min);
        for j in 0..=N_MEAN_SAMPLES {
            let shift = (f64::from(j) / f64::from(N_MEAN_SAMPLES)) * max_shift;
            let cand_mu = mean_dir.mul_add(shift, input.market_mu);
            let Some((coll, x_star)) = collateral_and_x_star(
                input.market_mu,
                input.market_sigma,
                cand_mu,
                cand_sigma,
                input.effective_k,
            ) else {
                continue;
            };
            if coll <= 0.0 || coll > input.budget {
                continue;
            }
            let ev = expected_value(
                input.market_mu,
                input.market_sigma,
                cand_mu,
                cand_sigma,
                input.effective_k,
                input.belief_mu,
                input.belief_sigma,
            );
            if ev > best_ev || (ev >= best_ev && best_coll > 0.0 && coll < best_coll) {
                best_ev = ev;
                best_mu = cand_mu;
                best_sigma = cand_sigma;
                best_coll = coll;
                best_x_star = x_star;
            }
        }
    }
    if best_ev <= 0.0 || best_coll <= 0.0 {
        return no_trade;
    }

    let full_shift = (input.belief_mu - input.market_mu).abs();
    let achieved_shift = (best_mu - input.market_mu).abs();
    let utilization = if full_shift > 1e-9_f64 {
        (achieved_shift / full_shift).min(1.0)
    } else {
        1.0
    };

    LognormalOptimizationResult {
        optimized_mu: best_mu,
        optimized_sigma: best_sigma,
        optimized_variance: best_sigma * best_sigma,
        collateral_required: best_coll,
        x_star: best_x_star,
        expected_value: best_ev,
        belief_utilization: utilization,
        is_budget_sufficient: (best_mu - input.belief_mu).abs() < sigma_step * 0.5
            && (best_sigma - input.belief_sigma).abs() < sigma_step * 0.5,
        budget_surplus: input.budget - best_coll,
        roi: if best_coll > 0.0 { best_ev / best_coll } else { 0.0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Numeric cross-check of the closed-form expectation: midpoint grid over
    /// `y = ln x`, `E[LN_i] = ∫ LN_i(e^y)·N(y; μb, σb) dy`.
    fn numeric_cross_expectation(mu: f64, sigma: f64, belief_mu: f64, belief_sigma: f64) -> f64 {
        let n = 20_000;
        let lo = belief_sigma.mul_add(-8.0, belief_mu);
        let hi = belief_sigma.mul_add(8.0, belief_mu);
        let step = (hi - lo) / f64::from(n);
        let mut acc = 0.0;
        for i in 0..n {
            let y = (f64::from(i) + 0.5).mul_add(step, lo);
            let belief_pdf = (-0.5 * ((y - belief_mu) / belief_sigma).powi(2)).exp()
                / (belief_sigma * (2.0 * core::f64::consts::PI).sqrt());
            // LN(e^y; μ, σ) = N(y; μ, σ)/e^y
            let ln_pdf = (-0.5 * ((y - mu) / sigma).powi(2)).exp()
                / (sigma * (2.0 * core::f64::consts::PI).sqrt())
                / y.exp();
            acc += belief_pdf * ln_pdf * step;
        }
        acc
    }

    #[test]
    fn cross_expectation_matches_numeric_integration() {
        for (mu, sigma, bmu, bsigma) in [
            (0.5_f64, 0.3_f64, 0.6_f64, 0.25_f64),
            (2.0, 0.8, 1.5, 0.5),
            (-0.2, 0.15, -0.1, 0.2),
        ] {
            let closed = lognormal_cross_expectation(mu, sigma, bmu, bsigma);
            let numeric = numeric_cross_expectation(mu, sigma, bmu, bsigma);
            assert!(
                ((closed - numeric) / numeric).abs() < 1e-4,
                "closed {closed} vs numeric {numeric} for ({mu},{sigma},{bmu},{bsigma})",
            );
        }
    }

    #[test]
    fn optimizer_moves_toward_belief() {
        let result = optimize_lognormal_trade(LognormalOptimizationInput::new(
            500.0, // budget
            0.40,  // belief μ (log)
            0.20,  // belief σ
            0.50,  // market μ
            0.25,  // market σ
            200.0, // effective k
        ));
        assert!(result.collateral_required > 0.0, "{result:?}");
        assert!(result.expected_value > 0.0, "{result:?}");
        assert!(
            result.optimized_mu < 0.50 && result.optimized_mu >= 0.40 - 1e-9,
            "moves down toward belief: {result:?}",
        );
        assert!(result.collateral_required <= 500.0);
        assert!(result.x_star.is_finite() && result.x_star > 0.0);
    }

    #[test]
    fn tight_budget_buys_a_partial_move() {
        let rich = optimize_lognormal_trade(LognormalOptimizationInput::new(
            1_000.0, 0.30, 0.15, 0.50, 0.25, 200.0,
        ));
        let poor = optimize_lognormal_trade(LognormalOptimizationInput::new(
            30.0, 0.30, 0.15, 0.50, 0.25, 200.0,
        ));
        assert!(rich.belief_utilization >= poor.belief_utilization, "{rich:?} vs {poor:?}");
        assert!(poor.collateral_required <= 30.0 + 1e-9);
    }

    #[test]
    fn candidates_respect_policy_region() {
        let result = optimize_lognormal_trade(LognormalOptimizationInput::new(
            10_000.0, // huge budget
            5.0,      // belief μ far beyond the per-trade cap
            0.05,     // belief σ far below σ/4
            0.50,
            0.25,
            200.0,
        ));
        let constraints = OptimizerConstraints::default();
        assert!(result.optimized_sigma >= 0.25 / constraints.max_sigma_ratio - 1e-9);
        assert!(result.optimized_sigma <= 0.25 * constraints.max_sigma_ratio + 1e-9);
        assert!(
            (result.optimized_mu - 0.50).abs()
                <= constraints.max_mean_sep_sigmas * 0.25 + 1e-9,
            "single-trade μ move is capped: {result:?}",
        );
        // A capped move means the belief was NOT fully expressed — the CLI
        // surfaces this so the skill can suggest a multi-trade ladder.
        assert!(result.belief_utilization < 1.0);
        assert!(!result.is_budget_sufficient);
    }

    #[test]
    fn zero_budget_is_no_trade() {
        let result = optimize_lognormal_trade(LognormalOptimizationInput::new(
            0.0, 0.4, 0.2, 0.5, 0.25, 200.0,
        ));
        assert!(result.collateral_required == 0.0 && result.expected_value == 0.0);
    }
}
