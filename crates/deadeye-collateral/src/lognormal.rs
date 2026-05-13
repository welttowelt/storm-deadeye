//! Off-chain collateral for lognormal markets.
//!
//! Ports `packages/collateral/src/lognormal/compute.ts` — damped
//! Newton-Raphson on the **lambda-scaled** PDF difference
//! `d̃(x) = λ_g · g(x) − λ_f · f(x)` constrained to `x > 0`, mirroring
//! the chain's `scaled_verify_minimum_with_lambda` contract from
//! `packages/onchain-lognormal-math/src/helpers.cairo`.
//!
//! Lognormal lambda formula (Cairo:
//! `packages/market-lognormal/src/l2_norm.cairo:13-46`):
//!
//! ```text
//! ‖p‖₂ = exp(σ²/8 − μ/2) / √(2σ√π)
//! λ    = k / ‖p‖₂
//! ```

use deadeye_core::{Distribution, LognormalDistribution};

use crate::{CollateralError, SQRT_PI};

/// `‖p‖₂` for a lognormal distribution. Cairo:
/// `packages/market-lognormal/src/l2_norm.cairo:13-46`.
#[must_use]
pub fn lognormal_l2_norm(mu: f64, variance: f64) -> f64 {
    let sigma = variance.max(0.0).sqrt();
    if sigma <= 0.0 || !sigma.is_finite() {
        return 0.0;
    }
    let denom = (2.0_f64 * sigma * SQRT_PI).sqrt();
    let scale = ((variance / 8.0) - (mu / 2.0)).exp();
    scale / denom
}

/// `λ = k / ‖p‖₂` for a lognormal distribution.
#[must_use]
pub fn lognormal_lambda(mu: f64, variance: f64, k: f64) -> f64 {
    let norm = lognormal_l2_norm(mu, variance);
    if norm <= 0.0 || !norm.is_finite() {
        0.0
    } else {
        k / norm
    }
}

/// Defaults mirroring the TypeScript solver.
const DEFAULT_TOLERANCE: f64 = 1e-10_f64;
const DEFAULT_MAX_ITERATIONS: u32 = 80;
const DEFAULT_MIN_X: f64 = 1e-9_f64;
const DEFAULT_MAX_X: f64 = 1e9_f64;
const MAX_STEP_FRACTION: f64 = 0.5_f64;

/// Tunable knobs for the lognormal minimiser.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LognormalOptions {
    /// Convergence threshold (relative).
    pub tolerance: f64,
    /// Maximum Newton iterations.
    pub max_iterations: u32,
    /// Lower bound on `x`.
    pub min_x: f64,
    /// Upper bound on `x`.
    pub max_x: f64,
}

impl Default for LognormalOptions {
    fn default() -> Self {
        Self {
            tolerance: DEFAULT_TOLERANCE,
            max_iterations: DEFAULT_MAX_ITERATIONS,
            min_x: DEFAULT_MIN_X,
            max_x: DEFAULT_MAX_X,
        }
    }
}

/// Result of the lognormal minimisation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LognormalVerifiedMinimum {
    /// Location of the minimum (`x* > 0`).
    pub x_star: f64,
    /// `d(x*)`.
    pub d_min: f64,
    /// `max(0, -d_min)`.
    pub collateral: f64,
    /// Iterations performed.
    pub iterations: u32,
    /// Whether Newton converged within tolerance.
    pub converged: bool,
}

fn eval_d_scaled(
    f: &LognormalDistribution,
    g: &LognormalDistribution,
    lambda_f: f64,
    lambda_g: f64,
    x: f64,
) -> Result<f64, CollateralError> {
    let xq = deadeye_core::Sq128::from_f64(x).map_err(CollateralError::Core)?;
    let g_pdf = g.pdf(xq)?.to_f64();
    let f_pdf = f.pdf(xq)?.to_f64();
    Ok(lambda_g.mul_add(g_pdf, -(lambda_f * f_pdf)))
}

fn eval_d_prime_scaled(
    f: &LognormalDistribution,
    g: &LognormalDistribution,
    lambda_f: f64,
    lambda_g: f64,
    x: f64,
) -> Result<f64, CollateralError> {
    let xq = deadeye_core::Sq128::from_f64(x).map_err(CollateralError::Core)?;
    let g_p = g.pdf_derivative(xq)?.to_f64();
    let f_p = f.pdf_derivative(xq)?.to_f64();
    Ok(lambda_g.mul_add(g_p, -(lambda_f * f_p)))
}

fn eval_d_double_prime_scaled(
    f: &LognormalDistribution,
    g: &LognormalDistribution,
    lambda_f: f64,
    lambda_g: f64,
    x: f64,
) -> Result<f64, CollateralError> {
    let xq = deadeye_core::Sq128::from_f64(x).map_err(CollateralError::Core)?;
    let g_pp = g.pdf_second_derivative(xq)?.to_f64();
    let f_pp = f.pdf_second_derivative(xq)?.to_f64();
    Ok(lambda_g.mul_add(g_pp, -(lambda_f * f_pp)))
}

/// Unscaled `d(x) = g(x) − f(x)`, used to report the natural-frame
/// collateral magnitude after the lambda-scaled minimum is located.
fn eval_d(
    f: &LognormalDistribution,
    g: &LognormalDistribution,
    x: f64,
) -> Result<f64, CollateralError> {
    let xq = deadeye_core::Sq128::from_f64(x).map_err(CollateralError::Core)?;
    let g_pdf = g.pdf(xq)?.to_f64();
    let f_pdf = f.pdf(xq)?.to_f64();
    Ok(g_pdf - f_pdf)
}

fn suggest_initial_x(f: &LognormalDistribution, g: &LognormalDistribution) -> f64 {
    let median_f = f.mu().to_f64().exp();
    let median_g = g.mu().to_f64().exp();
    if !median_f.is_finite() || !median_g.is_finite() {
        return 1.0_f64;
    }
    ((median_f + median_g) * 0.5).max(DEFAULT_MIN_X)
}

/// Find the minimum of `d̃(x) = λ_g g(x) − λ_f f(x)` on `x > 0`.
///
/// Reports the natural-frame `d(x*) = g(x*) − f(x*)` and implied
/// collateral `max(0, −d(x*))`. The on-chain verifier rescales by its
/// own `k`, so callers may rely on the SIGN and ZERO structure of
/// `d_min` regardless of which `k` we picked here (we use `k = 1` to
/// keep magnitudes near unit scale).
///
/// Seed: a small grid scan in `(0, μ_f∨g + 6σ_f∨g]` to find the
/// lambda-scaled global minimum, then damped Newton with the chain's
/// half-position step clamp.
pub fn lognormal_collateral(
    f: &LognormalDistribution,
    g: &LognormalDistribution,
    opts: LognormalOptions,
) -> Result<LognormalVerifiedMinimum, CollateralError> {
    let lambda_f = lognormal_lambda(f.mu().to_f64(), f.variance().to_f64(), 1.0_f64);
    let lambda_g = lognormal_lambda(g.mu().to_f64(), g.variance().to_f64(), 1.0_f64);

    // Side constraint mirrors the chain (Cairo:
    // `collateral-lognormal/src/pdf_difference.cairo::is_on_correct_side`):
    // pivot = exp(μ_f), and x* must lie on the side away from g.
    let side = lognormal_side(f, g);
    let pivot = f.mu().to_f64().exp();
    let (clamp_lo, clamp_hi) = match side {
        LognormalSide::LeftOfPivot => (opts.min_x, pivot.min(opts.max_x)),
        LognormalSide::RightOfPivot => (pivot.max(opts.min_x), opts.max_x),
        LognormalSide::Either => (opts.min_x, opts.max_x),
    };

    let mut x = grid_seed_lognormal(f, g, lambda_f, lambda_g, opts.min_x, opts.max_x);
    // Force the initial x onto the correct side even if grid degenerated.
    x = x.clamp(clamp_lo, clamp_hi);
    let mut iterations = 0_u32;
    let mut converged = false;

    for i in 0..opts.max_iterations {
        let d_prime = eval_d_prime_scaled(f, g, lambda_f, lambda_g, x)?;
        let d_double = eval_d_double_prime_scaled(f, g, lambda_f, lambda_g, x)?;
        if !d_prime.is_finite() || !d_double.is_finite() || d_double.abs() < 1e-16_f64 {
            break;
        }
        let mut step = d_prime / d_double;
        let max_step = (x * MAX_STEP_FRACTION).max(1e-9_f64);
        if step.abs() > max_step {
            step = step.signum() * max_step;
        }
        // Clamp to the correct side; this prevents Newton from crossing
        // the pivot into the wrong-side basin where d̃ has a deeper but
        // chain-rejected minimum.
        let x_next = (x - step).clamp(clamp_lo, clamp_hi);
        iterations = i + 1;
        if (x_next - x).abs() <= opts.tolerance * (1.0_f64.max(x.abs())) {
            converged = true;
            x = x_next;
            break;
        }
        x = x_next;
    }

    let d_min = eval_d(f, g, x).unwrap_or(0.0);
    let collateral = if d_min < 0.0 { -d_min } else { 0.0 };
    Ok(LognormalVerifiedMinimum {
        x_star: x,
        d_min,
        collateral,
        iterations,
        converged,
    })
}

/// Side constraint for lognormal: the chain insists the minimum lies on
/// the side of `pivot = exp(μ_f)` opposite to g, falling back to the
/// variance ordering when means tie (Cairo:
/// `verifier-law/src/lib.cairo::side_constraint_from_mean_and_variance`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LognormalSide {
    LeftOfPivot,
    RightOfPivot,
    Either,
}

fn lognormal_side(f: &LognormalDistribution, g: &LognormalDistribution) -> LognormalSide {
    let mu_f = f.mu().to_f64();
    let mu_g = g.mu().to_f64();
    if mu_g > mu_f {
        LognormalSide::LeftOfPivot
    } else if mu_g < mu_f {
        LognormalSide::RightOfPivot
    } else {
        let var_f = f.variance().to_f64();
        let var_g = g.variance().to_f64();
        if var_g > var_f {
            LognormalSide::LeftOfPivot
        } else if var_g < var_f {
            LognormalSide::RightOfPivot
        } else {
            LognormalSide::Either
        }
    }
}

fn grid_seed_lognormal(
    f: &LognormalDistribution,
    g: &LognormalDistribution,
    lambda_f: f64,
    lambda_g: f64,
    min_x: f64,
    max_x: f64,
) -> f64 {
    // The chain's `is_on_correct_side` for lognormal pivots on
    // `pivot = exp(μ_f)` and demands x* lies on the side opposite to g
    // (Cairo: `collateral-lognormal/src/pdf_difference.cairo:120-132`).
    // The global minimum of `d̃ = λ_g·g − λ_f·f` is often on the WRONG
    // side (where g peaks and f is small), so we constrain the grid
    // scan to the correct side. Without this constraint phase 04 of
    // the lognormal chaos suite (μ↓, σ widens) lands x* on the g-side
    // tail and the chain rejects with VERIFICATION_FAILED (side_valid
    // = false).
    let mu_f = f.mu().to_f64();
    let mu_g = g.mu().to_f64();
    let pivot = mu_f.exp();
    let log_sigma_f = f.sigma().to_f64().max(0.1_f64);
    let log_sigma_max = (f.sigma().to_f64()).max(g.sigma().to_f64()).max(0.1_f64);
    let side = lognormal_side(f, g);
    let log_centre = 0.5_f64 * (mu_f + mu_g);
    let lo_unconstrained = (-6.0_f64)
        .mul_add(log_sigma_max, log_centre)
        .exp()
        .max(min_x);
    let hi_unconstrained = 6.0_f64.mul_add(log_sigma_max, log_centre).exp().min(max_x);

    // Constrain to the side opposite g (matching Cairo's seed
    // `y0 = μ_f ± 2σ_f`: the seed is on the side of f away from g and
    // local Newton stays there).
    let (mut lo, mut hi) = match side {
        LognormalSide::LeftOfPivot => (lo_unconstrained, pivot.min(hi_unconstrained)),
        LognormalSide::RightOfPivot => (pivot.max(lo_unconstrained), hi_unconstrained),
        LognormalSide::Either => (lo_unconstrained, hi_unconstrained),
    };

    // Tail extension: place the seed window centred on `μ_f ± 2σ_f`
    // (matching Cairo's `suggest_initial_guess`). Empirically the
    // chain-acceptable minimum is at most ~4σ_f from μ_f.
    let two_sigma_f_offset_x = (2.0_f64 * log_sigma_f).exp(); // multiplicative factor
    match side {
        LognormalSide::LeftOfPivot => {
            // x* < pivot; tail decays toward x → 0.
            let cairo_seed = pivot / two_sigma_f_offset_x;
            lo = lo.min(cairo_seed * 0.1_f64).max(min_x);
        },
        LognormalSide::RightOfPivot => {
            // x* > pivot; tail extends to large x.
            let cairo_seed = pivot * two_sigma_f_offset_x;
            hi = hi.max(cairo_seed * 10.0_f64).min(max_x);
        },
        LognormalSide::Either => {},
    }

    if !(lo.is_finite() && hi.is_finite() && lo > 0.0_f64 && hi > lo) {
        return suggest_initial_x(f, g);
    }
    let samples: u32 = 96;
    let log_lo = lo.ln();
    let log_hi = hi.ln();
    let step = (log_hi - log_lo) / f64::from(samples);
    let mut best_x = match side {
        LognormalSide::LeftOfPivot => (pivot * 0.5).max(min_x),
        LognormalSide::RightOfPivot => pivot * 2.0,
        LognormalSide::Either => suggest_initial_x(f, g),
    };
    let mut best_d = f64::INFINITY;
    for i in 0..=samples {
        let x = step.mul_add(f64::from(i), log_lo).exp();
        if let Ok(d) = eval_d_scaled(f, g, lambda_f, lambda_g, x)
            && d.is_finite()
            && d < best_d
        {
            best_d = d;
            best_x = x;
        }
    }
    best_x
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use deadeye_core::{LognormalDistribution, Sq128};

    use super::*;

    fn lognormal(mu: f64, variance: f64) -> LognormalDistribution {
        LognormalDistribution::from_variance(
            Sq128::from_f64(mu).unwrap(),
            Sq128::from_f64(variance).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn identical_pair_has_zero_collateral() {
        let f = lognormal(0.0, 1.0);
        let g = lognormal(0.0, 1.0);
        let result = lognormal_collateral(&f, &g, LognormalOptions::default()).unwrap();
        assert!(result.collateral.abs() < 1e-8);
    }

    #[test]
    fn shifted_pair_yields_positive_collateral() {
        let f = lognormal(0.0, 0.25);
        let g = lognormal(0.05, 0.25);
        let result = lognormal_collateral(&f, &g, LognormalOptions::default()).unwrap();
        assert!(result.collateral >= 0.0);
        assert!(result.x_star > 0.0);
    }
}
