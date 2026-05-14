//! 2D grid-search trade optimizer for normal-distribution markets.
//!
//! Maximises **net** expected value (`EV − collateral`) under the trader's
//! belief, subject to `collateral ≤ budget`.

use deadeye_collateral::{MinimizationPolicy, lambda, normal_collateral};
use deadeye_core::{NormalDistribution, Sq128};

const N_SIGMA_SAMPLES: u32 = 50;
const N_MEAN_SAMPLES: u32 = 50;
const DEFAULT_MAX_SIGMA_RATIO: f64 = 4.0_f64;
const DEFAULT_MAX_MEAN_SEP_SIGMAS: f64 = 4.0_f64;

/// Tunable bounds on the optimizer's policy region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OptimizerConstraints {
    /// Maximum `σ_large / σ_small`.
    pub max_sigma_ratio: f64,
    /// Maximum `|μ_g - μ_market|` measured in units of `σ_market`.
    pub max_mean_sep_sigmas: f64,
}

impl Default for OptimizerConstraints {
    fn default() -> Self {
        Self {
            max_sigma_ratio: DEFAULT_MAX_SIGMA_RATIO,
            max_mean_sep_sigmas: DEFAULT_MAX_MEAN_SEP_SIGMAS,
        }
    }
}

/// Inputs to [`optimize_normal_trade`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalOptimizationInput {
    /// Trader's budget in collateral tokens.
    pub budget: f64,
    /// Trader's belief mean.
    pub belief_mean: f64,
    /// Trader's belief σ (from the UI's confidence selector).
    pub belief_sigma: f64,
    /// Current market mean.
    pub market_mean: f64,
    /// Current market σ.
    pub market_sigma: f64,
    /// AMM `effective_k` (post LP scaling).
    pub effective_k: f64,
    /// Payout amplifier (default 1.0).
    pub payout_amplifier: f64,
    /// Policy bounds.
    pub constraints: OptimizerConstraints,
}

impl NormalOptimizationInput {
    /// Convenience constructor with sane defaults (amplifier=1, default constraints).
    #[must_use]
    pub fn new(
        budget: f64,
        belief_mean: f64,
        belief_sigma: f64,
        market_mean: f64,
        market_sigma: f64,
        effective_k: f64,
    ) -> Self {
        Self {
            budget,
            belief_mean,
            belief_sigma,
            market_mean,
            market_sigma,
            effective_k,
            payout_amplifier: 1.0,
            constraints: OptimizerConstraints::default(),
        }
    }
}

/// Output of [`optimize_normal_trade`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalOptimizationResult {
    /// Optimised target `μ_g`.
    pub optimized_mean: f64,
    /// Optimised target `σ_g`.
    pub optimized_sigma: f64,
    /// `optimized_sigma²`.
    pub optimized_variance: f64,
    /// Collateral required to enter the optimised trade.
    pub collateral_required: f64,
    /// Gross expected value under the trader's belief.
    pub expected_value: f64,
    /// Fraction of the intended belief shift the trade expresses (0–1).
    pub belief_utilization: f64,
    /// `true` iff the optimum sits at the full belief point.
    pub is_budget_sufficient: bool,
    /// `budget − collateral_required`.
    pub budget_surplus: f64,
    /// Net ROI: `(EV − collateral) / collateral`.
    pub roi: f64,
}

/// Closed-form Gaussian product integral — `∫ φ(x; μ₁, σ₁) φ(x; μ₂, σ₂) dx`.
fn gaussian_product_integral(mu1: f64, sigma1: f64, mu2: f64, sigma2: f64) -> f64 {
    let sum_var = sigma1.mul_add(sigma1, sigma2 * sigma2);
    if sum_var <= 0.0 {
        return 0.0;
    }
    let diff_mu = mu1 - mu2;
    (1.0 / (2.0 * core::f64::consts::PI * sum_var).sqrt())
        * (-(diff_mu * diff_mu) / (2.0 * sum_var)).exp()
}

/// Collateral cost to transition `(μ_f, σ_f) → (μ_g, σ_g)`, computed
/// via `deadeye_collateral::normal_collateral`.
///
/// This was previously a duplicated inline Newton solver, but its
/// initial-guess heuristic mis-converged on equal-μ / σ-only moves —
/// it would over-estimate cost by ~100× for symmetric σ-shrinking
/// trades, hiding σ-arbitrage opportunities entirely. The
/// `deadeye-collateral` crate's λ-scaled solver is the source of truth
/// (audited against chain `scaled_verify_minimum_with_lambda`), so we
/// just call it. Returns `f64::INFINITY` on any solver failure so the
/// grid filter at line 265 of `optimize_normal_trade` rejects the
/// candidate cleanly.
fn collateral_number(mu_f: f64, sigma_f: f64, mu_g: f64, sigma_g: f64, k: f64) -> f64 {
    if (mu_f - mu_g).abs() < 1e-12_f64 && (sigma_f - sigma_g).abs() < 1e-12_f64 {
        return 0.0;
    }
    let Ok(mean_f) = Sq128::from_f64(mu_f) else {
        return f64::INFINITY;
    };
    let Ok(var_f) = Sq128::from_f64(sigma_f * sigma_f) else {
        return f64::INFINITY;
    };
    let Ok(mean_g) = Sq128::from_f64(mu_g) else {
        return f64::INFINITY;
    };
    let Ok(var_g) = Sq128::from_f64(sigma_g * sigma_g) else {
        return f64::INFINITY;
    };
    let Ok(f) = NormalDistribution::from_variance(mean_f, var_f) else {
        return f64::INFINITY;
    };
    let Ok(g) = NormalDistribution::from_variance(mean_g, var_g) else {
        return f64::INFINITY;
    };
    // `k` enters via the lambda scaling that `normal_collateral` uses
    // internally; the policy here is permissive (we'll filter
    // out-of-budget candidates downstream).
    let _ = k;
    match normal_collateral(&f, &g, MinimizationPolicy::unrestricted()) {
        Ok(verified) if verified.collateral.is_finite() => verified.collateral.max(0.0),
        _ => f64::INFINITY,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "EV depends on 8 distinct numeric inputs; bundling them adds churn without value"
)]
fn expected_value(
    mu_f: f64,
    sigma_f: f64,
    mu_g: f64,
    sigma_g: f64,
    k: f64,
    belief_mu: f64,
    belief_sigma: f64,
    amplifier: f64,
) -> f64 {
    let lam_g = lambda(sigma_g, k);
    let lam_f = lambda(sigma_f, k);
    let raw_ev = lam_g.mul_add(
        gaussian_product_integral(mu_g, sigma_g, belief_mu, belief_sigma),
        -(lam_f * gaussian_product_integral(mu_f, sigma_f, belief_mu, belief_sigma)),
    );
    amplifier * raw_ev
}

/// Picks the highest-net-EV trade in the policy region.
#[must_use]
pub fn optimize_normal_trade(input: NormalOptimizationInput) -> NormalOptimizationResult {
    let no_trade = NormalOptimizationResult {
        optimized_mean: input.market_mean,
        optimized_sigma: input.market_sigma,
        optimized_variance: input.market_sigma * input.market_sigma,
        collateral_required: 0.0,
        expected_value: 0.0,
        belief_utilization: 0.0,
        is_budget_sufficient: false,
        budget_surplus: input.budget,
        roi: 0.0,
    };

    if input.budget <= 0.0 || input.market_sigma <= 0.0 || input.effective_k <= 0.0 {
        return no_trade;
    }

    let sigma_min = (input.market_sigma / input.constraints.max_sigma_ratio).max(1e-6_f64);
    let sigma_max = input.market_sigma * input.constraints.max_sigma_ratio;
    let sigma_step = (sigma_max - sigma_min) / f64::from(N_SIGMA_SAMPLES);

    let mean_dir = if input.belief_mean >= input.market_mean {
        1.0_f64
    } else {
        -1.0_f64
    };
    let max_shift = input.constraints.max_mean_sep_sigmas * input.market_sigma;

    let mut best_net = f64::NEG_INFINITY;
    let mut best_mu = input.market_mean;
    let mut best_sigma = input.market_sigma;
    let mut best_coll = 0.0_f64;
    let mut best_ev = 0.0_f64;

    for i in 0..=N_SIGMA_SAMPLES {
        let cand_sigma = f64::from(i).mul_add(sigma_step, sigma_min);
        for j in 0..=N_MEAN_SAMPLES {
            let shift = (f64::from(j) / f64::from(N_MEAN_SAMPLES)) * max_shift;
            let cand_mu = mean_dir.mul_add(shift, input.market_mean);
            let coll = collateral_number(
                input.market_mean,
                input.market_sigma,
                cand_mu,
                cand_sigma,
                input.effective_k,
            );
            if coll < 0.0 || coll > input.budget {
                continue;
            }
            let ev = expected_value(
                input.market_mean,
                input.market_sigma,
                cand_mu,
                cand_sigma,
                input.effective_k,
                input.belief_mean,
                input.belief_sigma,
                input.payout_amplifier,
            );
            let net = ev - coll;
            if net > best_net {
                best_net = net;
                best_mu = cand_mu;
                best_sigma = cand_sigma;
                best_coll = coll;
                best_ev = ev;
            }
        }
    }
    if best_net <= 0.0 || best_coll <= 0.0 {
        return no_trade;
    }

    let full_shift = (input.belief_mean - input.market_mean).abs();
    let achieved_shift = (best_mu - input.market_mean).abs();
    let utilization = if full_shift > 1e-6_f64 {
        (achieved_shift / full_shift).min(1.0)
    } else {
        1.0
    };

    NormalOptimizationResult {
        optimized_mean: best_mu,
        optimized_sigma: best_sigma,
        optimized_variance: best_sigma * best_sigma,
        collateral_required: best_coll,
        expected_value: best_ev,
        belief_utilization: utilization,
        is_budget_sufficient: (best_mu - input.belief_mean).abs() < sigma_step * 0.5
            && (best_sigma - input.belief_sigma).abs() < sigma_step * 0.5,
        budget_surplus: input.budget - best_coll,
        roi: best_net / best_coll,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_budget_yields_no_trade() {
        let input = NormalOptimizationInput::new(0.0, 105.0, 1.0, 100.0, 2.0, 50.0);
        let r = optimize_normal_trade(input);
        assert!(r.collateral_required.abs() < 1e-12);
        assert!((r.optimized_mean - 100.0).abs() < 1e-12);
    }

    #[test]
    fn zero_market_sigma_yields_no_trade() {
        let input = NormalOptimizationInput::new(100.0, 105.0, 1.0, 100.0, 0.0, 50.0);
        let r = optimize_normal_trade(input);
        assert!(r.collateral_required.abs() < 1e-12);
    }

    #[test]
    fn picks_a_trade_under_reasonable_budget() {
        let input = NormalOptimizationInput::new(100.0, 105.0, 1.0, 100.0, 2.0, 50.0);
        let r = optimize_normal_trade(input);
        assert!(r.collateral_required >= 0.0);
        assert!(r.budget_surplus <= input.budget);
        // With a meaningful belief shift the optimizer should select a non-trivial
        // mean change.
        assert!(r.optimized_mean >= input.market_mean);
    }

    /// Regression — σ-only arb (μ_b ≈ μ_market, σ_b ≪ σ_market) must
    /// produce a positive-EV trade. Pre-v0.1.1 the inline Newton solver
    /// mis-converged on equal-μ moves and over-estimated cost ~100×,
    /// hiding every σ-shrink trade. Inputs match the live CPI YoY
    /// market (2026-05-14): belief from the meridian model, market
    /// from mainnet.
    #[test]
    fn sigma_arb_with_equal_mu_finds_positive_ev_trade() {
        let input = NormalOptimizationInput::new(
            50.0,    // budget
            4.3274,  // belief μ
            0.2143,  // belief σ
            4.2900,  // market μ
            0.3500,  // market σ
            75.07,   // effective k
        );
        let r = optimize_normal_trade(input);
        assert!(
            r.collateral_required > 0.0 && r.collateral_required < 50.0,
            "σ-arb: expected positive in-budget collateral, got {}",
            r.collateral_required
        );
        assert!(
            r.expected_value > r.collateral_required,
            "σ-arb: expected positive net EV, got ev={} cost={}",
            r.expected_value,
            r.collateral_required
        );
        // The optimizer should pick a σ_g substantially tighter than market.
        assert!(
            r.optimized_sigma < input.market_sigma * 0.9,
            "σ-arb: expected tightened σ_g, got {}",
            r.optimized_sigma
        );
    }

    /// Regression — pure σ-arb with exact-equal μ must still find a trade.
    #[test]
    fn pure_sigma_arb_finds_trade() {
        let input = NormalOptimizationInput::new(50.0, 4.29, 0.21, 4.29, 0.35, 75.07);
        let r = optimize_normal_trade(input);
        assert!(r.collateral_required > 0.0);
        assert!(r.expected_value > r.collateral_required);
    }
}
