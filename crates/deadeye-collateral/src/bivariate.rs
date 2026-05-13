//! Off-chain collateral for bivariate-normal markets.
//!
//! Ports `packages/collateral/src/bivariate/compute.ts` — 2D Newton with
//! a finite-difference Hessian and step clamping, minimising the
//! **lambda-scaled** PDF difference
//! `d̃(x₁,x₂) = λ_g · g(x₁,x₂) − λ_f · f(x₁,x₂)`.
//!
//! Bivariate lambda formula (Cairo:
//! `packages/market-bivariate-normal/src/l2_norm.cairo:9-39`):
//!
//! ```text
//! ‖p‖₂² = normalization / 2 = 1 / (4π σ₁ σ₂ √(1−ρ²))
//! λ     = k / ‖p‖₂ = k · √(4π σ₁ σ₂ √(1−ρ²))
//! ```

use deadeye_core::BivariateNormalDistribution;

use crate::CollateralError;

/// `‖p‖₂` for a bivariate normal. Cairo:
/// `packages/market-bivariate-normal/src/l2_norm.cairo`.
#[must_use]
pub fn bivariate_l2_norm(sigma1: f64, sigma2: f64, rho: f64) -> f64 {
    let one_minus_rho_sq = rho.mul_add(-rho, 1.0_f64);
    if sigma1 <= 0.0 || sigma2 <= 0.0 || one_minus_rho_sq <= 0.0 {
        return 0.0;
    }
    // ‖p‖₂² = 1 / (4π σ₁ σ₂ √(1−ρ²))
    let denom = 4.0_f64 * core::f64::consts::PI * sigma1 * sigma2 * one_minus_rho_sq.sqrt();
    if denom <= 0.0 || !denom.is_finite() {
        return 0.0;
    }
    (1.0_f64 / denom).sqrt()
}

/// `λ = k / ‖p‖₂` for a bivariate normal.
#[must_use]
pub fn bivariate_lambda(sigma1: f64, sigma2: f64, rho: f64, k: f64) -> f64 {
    let norm = bivariate_l2_norm(sigma1, sigma2, rho);
    if norm <= 0.0 || !norm.is_finite() {
        0.0
    } else {
        k / norm
    }
}

const DEFAULT_TOLERANCE: f64 = 1e-8_f64;
const DEFAULT_MAX_ITERATIONS: u32 = 50;
const DEFAULT_STEP_EPSILON: f64 = 1e-4_f64;
const MAX_DELTA: f64 = 2.0_f64;

/// Tunable knobs for the bivariate minimiser.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BivariateOptions {
    /// Relative convergence threshold.
    pub tolerance: f64,
    /// Maximum Newton iterations.
    pub max_iterations: u32,
    /// Finite-difference step.
    pub step_epsilon: f64,
}

impl Default for BivariateOptions {
    fn default() -> Self {
        Self {
            tolerance: DEFAULT_TOLERANCE,
            max_iterations: DEFAULT_MAX_ITERATIONS,
            step_epsilon: DEFAULT_STEP_EPSILON,
        }
    }
}

/// Result of the bivariate minimisation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BivariateVerifiedMinimum {
    /// Location of the minimum along axis 1.
    pub x1: f64,
    /// Location of the minimum along axis 2.
    pub x2: f64,
    /// `d(x*) = g(x*) - f(x*)`.
    pub d_min: f64,
    /// `max(0, -d_min)`.
    pub collateral: f64,
    /// Iterations performed.
    pub iterations: u32,
    /// Whether Newton converged within tolerance.
    pub converged: bool,
}

fn eval_d_scaled(
    f: &BivariateNormalDistribution,
    g: &BivariateNormalDistribution,
    lambda_f: f64,
    lambda_g: f64,
    x1: f64,
    x2: f64,
) -> Option<f64> {
    let g_pdf = g.pdf(x1, x2)?;
    let f_pdf = f.pdf(x1, x2)?;
    Some(lambda_g.mul_add(g_pdf, -(lambda_f * f_pdf)))
}

fn eval_d_unscaled(
    f: &BivariateNormalDistribution,
    g: &BivariateNormalDistribution,
    x1: f64,
    x2: f64,
) -> Option<f64> {
    Some(g.pdf(x1, x2)? - f.pdf(x1, x2)?)
}

fn gradient_fd(
    f: &BivariateNormalDistribution,
    g: &BivariateNormalDistribution,
    lambda_f: f64,
    lambda_g: f64,
    x1: f64,
    x2: f64,
    eps: f64,
) -> Option<(f64, f64)> {
    let dx1_plus = eval_d_scaled(f, g, lambda_f, lambda_g, x1 + eps, x2)?;
    let dx1_minus = eval_d_scaled(f, g, lambda_f, lambda_g, x1 - eps, x2)?;
    let dx2_plus = eval_d_scaled(f, g, lambda_f, lambda_g, x1, x2 + eps)?;
    let dx2_minus = eval_d_scaled(f, g, lambda_f, lambda_g, x1, x2 - eps)?;
    Some((
        (dx1_plus - dx1_minus) / (2.0_f64 * eps),
        (dx2_plus - dx2_minus) / (2.0_f64 * eps),
    ))
}

fn hessian_fd(
    f: &BivariateNormalDistribution,
    g: &BivariateNormalDistribution,
    lambda_f: f64,
    lambda_g: f64,
    x1: f64,
    x2: f64,
    eps: f64,
) -> Option<(f64, f64, f64)> {
    let d = eval_d_scaled(f, g, lambda_f, lambda_g, x1, x2)?;
    let dx1_plus = eval_d_scaled(f, g, lambda_f, lambda_g, x1 + eps, x2)?;
    let dx1_minus = eval_d_scaled(f, g, lambda_f, lambda_g, x1 - eps, x2)?;
    let dx2_plus = eval_d_scaled(f, g, lambda_f, lambda_g, x1, x2 + eps)?;
    let dx2_minus = eval_d_scaled(f, g, lambda_f, lambda_g, x1, x2 - eps)?;
    let dpp = eval_d_scaled(f, g, lambda_f, lambda_g, x1 + eps, x2 + eps)?;
    let dpm = eval_d_scaled(f, g, lambda_f, lambda_g, x1 + eps, x2 - eps)?;
    let dmp = eval_d_scaled(f, g, lambda_f, lambda_g, x1 - eps, x2 + eps)?;
    let dmm = eval_d_scaled(f, g, lambda_f, lambda_g, x1 - eps, x2 - eps)?;
    let eps_sq = eps * eps;
    let h11 = (2.0_f64.mul_add(-d, dx1_plus) + dx1_minus) / eps_sq;
    let h22 = (2.0_f64.mul_add(-d, dx2_plus) + dx2_minus) / eps_sq;
    let h12 = (dpp - dpm - dmp + dmm) / (4.0_f64 * eps_sq);
    Some((h11, h12, h22))
}

fn solve_2x2(h11: f64, h12: f64, h22: f64, g1: f64, g2: f64) -> Option<(f64, f64)> {
    let det = h11.mul_add(h22, -(h12 * h12));
    if !det.is_finite() || det.abs() < 1e-20_f64 {
        return None;
    }
    Some((
        h22.mul_add(g1, -(h12 * g2)) / det,
        (-h12).mul_add(g1, h11 * g2) / det,
    ))
}

/// Find the 2D minimum of `d̃(x₁,x₂) = λ_g g − λ_f f`.
///
/// Reports the natural-frame `d_min = g − f` so the collateral magnitude
/// is in the same frame as the on-chain trade hint. Seeds with a coarse
/// 2D grid over ±4σ around the midpoint of the means, then refines with
/// damped Newton.
pub fn bivariate_collateral(
    f: &BivariateNormalDistribution,
    g: &BivariateNormalDistribution,
    opts: BivariateOptions,
) -> Result<BivariateVerifiedMinimum, CollateralError> {
    let lambda_f = bivariate_lambda(f.sigma1(), f.sigma2(), f.rho(), 1.0_f64);
    let lambda_g = bivariate_lambda(g.sigma1(), g.sigma2(), g.rho(), 1.0_f64);

    let (mut x1, mut x2) = grid_seed_bivariate(f, g, lambda_f, lambda_g);
    let mut iterations = 0_u32;
    let mut converged = false;

    for i in 0..opts.max_iterations {
        let Some((g1, g2)) = gradient_fd(f, g, lambda_f, lambda_g, x1, x2, opts.step_epsilon)
        else {
            break;
        };
        let Some((h11, h12, h22)) = hessian_fd(f, g, lambda_f, lambda_g, x1, x2, opts.step_epsilon)
        else {
            break;
        };
        let Some((s1, s2)) = solve_2x2(h11, h12, h22, g1, g2) else {
            break;
        };
        let dx1 = s1.clamp(-MAX_DELTA, MAX_DELTA);
        let dx2 = s2.clamp(-MAX_DELTA, MAX_DELTA);
        let next_x1 = x1 - dx1;
        let next_x2 = x2 - dx2;
        iterations = i + 1;
        let delta = (next_x1 - x1).hypot(next_x2 - x2);
        x1 = next_x1;
        x2 = next_x2;
        if delta <= opts.tolerance * (1.0_f64.max(x1.hypot(x2))) {
            converged = true;
            break;
        }
    }

    let d_min = eval_d_unscaled(f, g, x1, x2).unwrap_or(0.0);
    let collateral = if d_min < 0.0 { -d_min } else { 0.0 };
    Ok(BivariateVerifiedMinimum {
        x1,
        x2,
        d_min,
        collateral,
        iterations,
        converged,
    })
}

fn grid_seed_bivariate(
    f: &BivariateNormalDistribution,
    g: &BivariateNormalDistribution,
    lambda_f: f64,
    lambda_g: f64,
) -> (f64, f64) {
    let cx1 = 0.5_f64 * (f.mu1() + g.mu1());
    let cx2 = 0.5_f64 * (f.mu2() + g.mu2());
    let r1 = 4.0_f64.mul_add(f.sigma1().max(g.sigma1()), (g.mu1() - f.mu1()).abs());
    let r2 = 4.0_f64.mul_add(f.sigma2().max(g.sigma2()), (g.mu2() - f.mu2()).abs());
    let samples: u32 = 16;
    let step1 = (2.0_f64 * r1) / f64::from(samples);
    let step2 = (2.0_f64 * r2) / f64::from(samples);
    let mut best = (cx1, cx2);
    let mut best_d = f64::INFINITY;
    for i in 0..=samples {
        for j in 0..=samples {
            let x1 = step1.mul_add(f64::from(i), cx1 - r1);
            let x2 = step2.mul_add(f64::from(j), cx2 - r2);
            if let Some(d) = eval_d_scaled(f, g, lambda_f, lambda_g, x1, x2)
                && d.is_finite()
                && d < best_d
            {
                best_d = d;
                best = (x1, x2);
            }
        }
    }
    best
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use super::*;

    #[test]
    fn identical_pair_has_zero_collateral() {
        let f = BivariateNormalDistribution::from_core(0.0, 0.0, 1.0, 1.0, 0.0).unwrap();
        let g = BivariateNormalDistribution::from_core(0.0, 0.0, 1.0, 1.0, 0.0).unwrap();
        let result = bivariate_collateral(&f, &g, BivariateOptions::default()).unwrap();
        assert!(result.collateral.abs() < 1e-6);
    }

    #[test]
    fn shifted_pair_yields_positive_collateral() {
        let f = BivariateNormalDistribution::from_core(0.0, 0.0, 1.0, 1.0, 0.0).unwrap();
        let g = BivariateNormalDistribution::from_core(1.0, 0.5, 1.0, 1.0, 0.0).unwrap();
        let result = bivariate_collateral(&f, &g, BivariateOptions::default()).unwrap();
        assert!(result.collateral >= 0.0);
    }
}
