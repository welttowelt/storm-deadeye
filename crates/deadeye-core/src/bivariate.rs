//! Bivariate normal distribution `N₂(μ₁, μ₂, σ₁², σ₂², ρ)`.
//!
//! The on-wire shape carries four precomputed quantities (`σ₁`, `σ₂`,
//! `1/(1-ρ²)`, and the joint normalization) so the on-chain math runtime
//! can validate the candidate without re-deriving any square roots.

use crate::{error::CoreError, sq128::Sq128Raw};

/// Point in `ℝ²`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BivariatePointRaw {
    /// First component.
    pub x1: Sq128Raw,
    /// Second component.
    pub x2: Sq128Raw,
}

/// Compact (5-field) shape of a bivariate normal distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BivariateNormalDistributionCoreRaw {
    /// Mean along axis 1.
    pub mu1: Sq128Raw,
    /// Mean along axis 2.
    pub mu2: Sq128Raw,
    /// Variance along axis 1.
    pub variance1: Sq128Raw,
    /// Variance along axis 2.
    pub variance2: Sq128Raw,
    /// Correlation coefficient `ρ ∈ (-1, 1)`.
    pub rho: Sq128Raw,
}

/// Full (9-field) shape carried on-chain, with precomputed σ₁, σ₂, and
/// the joint normalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BivariateNormalDistributionRaw {
    /// Mean along axis 1.
    pub mu1: Sq128Raw,
    /// Mean along axis 2.
    pub mu2: Sq128Raw,
    /// Variance along axis 1.
    pub variance1: Sq128Raw,
    /// Variance along axis 2.
    pub variance2: Sq128Raw,
    /// Correlation coefficient `ρ`.
    pub rho: Sq128Raw,
    /// σ₁ = √variance1.
    pub sigma1: Sq128Raw,
    /// σ₂ = √variance2.
    pub sigma2: Sq128Raw,
    /// `1 / (1 - ρ²)`.
    pub inv_one_minus_rho_sq: Sq128Raw,
    /// `1 / (2π σ₁ σ₂ √(1-ρ²))`.
    pub normalization: Sq128Raw,
}

/// Sqrt hints for bivariate markets (mirrors normal hints).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BivariateNormalSqrtHintsRaw {
    /// `1 / ||f||₂²`.
    pub l2_norm_denom: Sq128Raw,
    /// `1 / backing`.
    pub backing_denom: Sq128Raw,
}

/// Bivariate normal distribution.
///
/// Does **not** implement [`Distribution`](crate::Distribution) because
/// the trait assumes a univariate distribution.
#[derive(Debug, Clone, Copy)]
pub struct BivariateNormalDistribution {
    mu1: f64,
    mu2: f64,
    variance1: f64,
    variance2: f64,
    sigma1: f64,
    sigma2: f64,
    rho: f64,
    inv_one_minus_rho_sq: f64,
    normalization: f64,
}

const TWO_PI: f64 = core::f64::consts::TAU;

impl BivariateNormalDistribution {
    /// Build a distribution from the 5 core parameters; auto-computes σ₁, σ₂,
    /// `1/(1-ρ²)`, and the joint normalization.
    pub fn from_core(
        mu1: f64,
        mu2: f64,
        variance1: f64,
        variance2: f64,
        rho: f64,
    ) -> Result<Self, CoreError> {
        if variance1 < 0.0 || variance2 < 0.0 {
            return Err(CoreError::invalid_input("variance", "must be ≥ 0"));
        }
        if !rho.is_finite() || rho.abs() >= 1.0 {
            return Err(CoreError::invalid_input("rho", "must be in (-1, 1)"));
        }
        let sigma1 = variance1.sqrt();
        let sigma2 = variance2.sqrt();
        if sigma1 <= 0.0 || sigma2 <= 0.0 {
            return Err(CoreError::invalid_input("sigma", "must be > 0"));
        }
        let one_minus_rho_sq = rho.mul_add(-rho, 1.0);
        if one_minus_rho_sq <= 0.0 {
            return Err(CoreError::invalid_input(
                "rho",
                "|rho| must be strictly < 1",
            ));
        }
        let inv_one_minus_rho_sq = 1.0 / one_minus_rho_sq;
        let normalization = 1.0 / (TWO_PI * sigma1 * sigma2 * one_minus_rho_sq.sqrt());
        Ok(Self {
            mu1,
            mu2,
            variance1,
            variance2,
            sigma1,
            sigma2,
            rho,
            inv_one_minus_rho_sq,
            normalization,
        })
    }

    /// Decode a fully-expanded distribution from its on-wire shape.
    pub fn from_raw(raw: BivariateNormalDistributionRaw) -> Result<Self, CoreError> {
        let mu1 = crate::sq128::Sq128::from_raw(raw.mu1).to_f64();
        let mu2 = crate::sq128::Sq128::from_raw(raw.mu2).to_f64();
        let variance1 = crate::sq128::Sq128::from_raw(raw.variance1).to_f64();
        let variance2 = crate::sq128::Sq128::from_raw(raw.variance2).to_f64();
        let rho = crate::sq128::Sq128::from_raw(raw.rho).to_f64();
        let sigma1 = crate::sq128::Sq128::from_raw(raw.sigma1).to_f64();
        let sigma2 = crate::sq128::Sq128::from_raw(raw.sigma2).to_f64();
        let inv_one_minus_rho_sq = crate::sq128::Sq128::from_raw(raw.inv_one_minus_rho_sq).to_f64();
        let normalization = crate::sq128::Sq128::from_raw(raw.normalization).to_f64();
        if variance1 < 0.0 || variance2 < 0.0 || sigma1 < 0.0 || sigma2 < 0.0 {
            return Err(CoreError::invalid_input(
                "bivariate",
                "negative variance or sigma",
            ));
        }
        Ok(Self {
            mu1,
            mu2,
            variance1,
            variance2,
            sigma1,
            sigma2,
            rho,
            inv_one_minus_rho_sq,
            normalization,
        })
    }

    /// Convert to the on-wire shape.
    pub fn to_raw(&self) -> Result<BivariateNormalDistributionRaw, CoreError> {
        let s = crate::sq128::Sq128::from_f64;
        Ok(BivariateNormalDistributionRaw {
            mu1: s(self.mu1)?.to_raw(),
            mu2: s(self.mu2)?.to_raw(),
            variance1: s(self.variance1)?.to_raw(),
            variance2: s(self.variance2)?.to_raw(),
            rho: s(self.rho)?.to_raw(),
            sigma1: s(self.sigma1)?.to_raw(),
            sigma2: s(self.sigma2)?.to_raw(),
            inv_one_minus_rho_sq: s(self.inv_one_minus_rho_sq)?.to_raw(),
            normalization: s(self.normalization)?.to_raw(),
        })
    }

    /// Convert to the 5-field core on-wire shape.
    pub fn to_core_raw(&self) -> Result<BivariateNormalDistributionCoreRaw, CoreError> {
        let s = crate::sq128::Sq128::from_f64;
        Ok(BivariateNormalDistributionCoreRaw {
            mu1: s(self.mu1)?.to_raw(),
            mu2: s(self.mu2)?.to_raw(),
            variance1: s(self.variance1)?.to_raw(),
            variance2: s(self.variance2)?.to_raw(),
            rho: s(self.rho)?.to_raw(),
        })
    }

    /// Returns `μ₁`.
    pub const fn mu1(&self) -> f64 {
        self.mu1
    }
    /// Returns `μ₂`.
    pub const fn mu2(&self) -> f64 {
        self.mu2
    }
    /// Returns `σ₁`.
    pub const fn sigma1(&self) -> f64 {
        self.sigma1
    }
    /// Returns `σ₂`.
    pub const fn sigma2(&self) -> f64 {
        self.sigma2
    }
    /// Returns `ρ`.
    pub const fn rho(&self) -> f64 {
        self.rho
    }

    /// Returns the joint PDF at `(x1, x2)`.
    pub fn pdf(&self, x1: f64, x2: f64) -> Option<f64> {
        if self.sigma1 <= 0.0 || self.sigma2 <= 0.0 || self.rho.abs() >= 1.0 {
            return None;
        }
        let z1 = (x1 - self.mu1) / self.sigma1;
        let z2 = (x2 - self.mu2) / self.sigma2;
        // quad = (z1² - 2ρ z1 z2 + z2²) / (2(1-ρ²))
        let quad = z1.mul_add(z1, (-2.0 * self.rho).mul_add(z1 * z2, z2 * z2))
            * (0.5 * self.inv_one_minus_rho_sq);
        let value = self.normalization * (-quad).exp();
        if value.is_finite() && value >= 0.0 {
            Some(value)
        } else {
            None
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_rho() {
        let result = BivariateNormalDistribution::from_core(0.0, 0.0, 1.0, 1.0, 1.0);
        assert!(matches!(result, Err(CoreError::InvalidInput { .. })));
    }

    #[test]
    fn pdf_peaks_at_mean() {
        let d = BivariateNormalDistribution::from_core(0.0, 0.0, 1.0, 1.0, 0.0).unwrap();
        let at_mean = d.pdf(0.0, 0.0).unwrap();
        let off_mean = d.pdf(1.0, 1.0).unwrap();
        assert!(at_mean > off_mean);
    }

    #[test]
    fn raw_round_trip() {
        let d = BivariateNormalDistribution::from_core(1.0, 2.0, 4.0, 9.0, 0.5).unwrap();
        let raw = d.to_raw().unwrap();
        let back = BivariateNormalDistribution::from_raw(raw).unwrap();
        assert!((back.mu1() - 1.0).abs() < 1e-12);
        assert!((back.mu2() - 2.0).abs() < 1e-12);
        assert!((back.sigma1() - 2.0).abs() < 1e-9);
        assert!((back.sigma2() - 3.0).abs() < 1e-9);
        assert!((back.rho() - 0.5).abs() < 1e-12);
    }
}
