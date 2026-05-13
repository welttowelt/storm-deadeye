//! 2D grid-search trade optimizer for normal-distribution markets.
//!
//! Maximises **net** expected value (`EV − collateral`) under the trader's
//! belief, subject to `collateral ≤ budget`.

use deadeye_collateral::lambda;

const N_SIGMA_SAMPLES: u32 = 50;
const N_MEAN_SAMPLES: u32 = 50;
const DEFAULT_MAX_SIGMA_RATIO: f64 = 4.0_f64;
const DEFAULT_MAX_MEAN_SEP_SIGMAS: f64 = 4.0_f64;
const SQRT_2PI: f64 = 2.506_628_274_631_000_7_f64;

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

fn normal_pdf(x: f64, mu: f64, sigma: f64) -> f64 {
    if sigma <= 0.0 {
        return 0.0;
    }
    let z = (x - mu) / sigma;
    (1.0 / (sigma * SQRT_2PI)) * (-0.5 * z * z).exp()
}

fn normal_pdf_deriv1(x: f64, mu: f64, sigma: f64) -> f64 {
    let xm = x - mu;
    let var = sigma * sigma;
    -(xm / var) * normal_pdf(x, mu, sigma)
}

fn normal_pdf_deriv2(x: f64, mu: f64, sigma: f64) -> f64 {
    let pdf = normal_pdf(x, mu, sigma);
    let xm = x - mu;
    let s2 = sigma * sigma;
    (xm * xm / (s2 * s2) - 1.0 / s2) * pdf
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

/// f64 Newton-Raphson collateral computation. Mirrors the TS reference
/// implementation's initial-guess heuristic so the optimizer's results
/// are byte-comparable.
fn collateral_number(mu_f: f64, sigma_f: f64, mu_g: f64, sigma_g: f64, k: f64) -> f64 {
    let lam_f = lambda(sigma_f, k);
    let lam_g = lambda(sigma_g, k);
    if (mu_f - mu_g).abs() < 1e-12_f64 && (sigma_f - sigma_g).abs() < 1e-12_f64 {
        return 0.0;
    }
    let mean_diff = (mu_f - mu_g).abs();
    let sigma_narrow = sigma_f.min(sigma_g);
    let nearly_equal_means = sigma_narrow > 0.0 && mean_diff < 0.01 * sigma_narrow;

    let mut x = if nearly_equal_means {
        if sigma_f < sigma_g {
            mu_f
        } else {
            mu_f + sigma_f
        }
    } else {
        let wide_to_narrow = sigma_f > sigma_g;
        let offset = if wide_to_narrow {
            sigma_f
        } else {
            (sigma_f * 0.5).min(mean_diff * 0.5)
        };
        if mu_g > mu_f {
            mu_f - offset
        } else {
            mu_f + offset
        }
    };
    let max_step = 0.5_f64 * sigma_f.max(sigma_g);

    for _ in 0..50 {
        let d1 = lam_g.mul_add(
            normal_pdf_deriv1(x, mu_g, sigma_g),
            -(lam_f * normal_pdf_deriv1(x, mu_f, sigma_f)),
        );
        let d2 = lam_g.mul_add(
            normal_pdf_deriv2(x, mu_g, sigma_g),
            -(lam_f * normal_pdf_deriv2(x, mu_f, sigma_f)),
        );
        if d2.abs() < 1e-30_f64 {
            break;
        }
        let step = (-d1 / d2).clamp(-max_step, max_step);
        let x_new = x + step;
        if (x_new - x).abs() < 1e-10_f64 {
            x = x_new;
            break;
        }
        x = x_new;
    }

    let ds = lam_g.mul_add(
        normal_pdf(x, mu_g, sigma_g),
        -(lam_f * normal_pdf(x, mu_f, sigma_f)),
    );
    (-ds).max(0.0)
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
}
