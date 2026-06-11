//! Distribution value objects.
//!
//! Each market family is backed by a small immutable value type that:
//!
//! 1. Validates its invariants at construction time.
//! 2. Carries the canonical [`Sq128Raw`] representation used in on-chain
//!    calldata.
//! 3. Implements [`Distribution`] so collateral / SDK code can be generic over
//!    distribution shape.
//!
//! ## Numerical posture
//!
//! PDFs and their derivatives are computed in `f64` rather than the full
//! Q128.128 magnitude. This matches the TypeScript SDK's approach for the
//! lognormal/multinoulli families, where the f64 accuracy is more than
//! sufficient for off-chain quoting and the on-chain settlement is the
//! authoritative numeric source. The Q128.128 type is reserved for values
//! that must round-trip bit-identical with Cairo.

use crate::{
    error::CoreError,
    sq128::{Sq128, Sq128Raw},
};

/// Generic distribution interface.
///
/// `Distribution<R>` is parameterised by its raw wire shape `R` so the
/// downstream collateral and contract crates can be generic without knowing
/// the concrete shape.
pub trait Distribution {
    /// On-wire serialised representation.
    type Raw: Copy;

    /// Mean of the distribution.
    fn mean(&self) -> Sq128;

    /// Variance of the distribution.
    fn variance(&self) -> Sq128;

    /// Standard deviation. Always non-negative.
    fn sigma(&self) -> Sq128;

    /// Returns `true` when the distribution is a point mass (variance = 0).
    fn is_degenerate(&self) -> bool {
        self.variance().is_zero()
    }

    /// Probability density at `x`.
    fn pdf(&self, x: Sq128) -> Result<Sq128, CoreError>;

    /// First derivative of the PDF at `x`.
    fn pdf_derivative(&self, x: Sq128) -> Result<Sq128, CoreError>;

    /// Second derivative of the PDF at `x`.
    fn pdf_second_derivative(&self, x: Sq128) -> Result<Sq128, CoreError>;

    /// Returns the canonical on-wire DTO.
    fn to_raw(&self) -> Self::Raw;
}

// ─── Normal ──────────────────────────────────────────────────────────────────

/// On-wire shape of a [`NormalDistribution`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NormalDistributionRaw {
    /// μ
    pub mean: Sq128Raw,
    /// σ²
    pub variance: Sq128Raw,
    /// σ (precomputed)
    pub sigma: Sq128Raw,
}

/// Auxiliary hints precomputed off-chain and passed to the on-chain math
/// runtime to avoid re-deriving an expensive square root in calldata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NormalSqrtHintsRaw {
    /// `1 / ||f||_2^2` for collateral verification.
    pub l2_norm_denom: Sq128Raw,
    /// `1 / backing` for collateral verification.
    pub backing_denom: Sq128Raw,
}

/// Normal (Gaussian) distribution `N(μ, σ²)`.
#[derive(Debug, Clone, Copy)]
pub struct NormalDistribution {
    mean: Sq128,
    variance: Sq128,
    sigma: Sq128,
}

impl NormalDistribution {
    /// Constructs a normal distribution, computing σ from the variance using
    /// the bit-exact Q128.128 floor square root.
    ///
    /// This calls [`Sq128::sqrt`], which reproduces the on-chain
    /// `sqrt_verified` invariant. As a result, the produced σ round-trips
    /// against `compute_hints_view` on the deployed math runtime — even for
    /// non-perfect-square variances like `0.04`, `0.13`, or `100.7` that
    /// the previous f64-mediated implementation rejected.
    pub fn from_variance(mean: Sq128, variance: Sq128) -> Result<Self, CoreError> {
        if variance.is_negative() {
            return Err(CoreError::invalid_input("variance", "must be ≥ 0"));
        }
        let sigma = variance.sqrt()?;
        Ok(Self {
            mean,
            variance,
            sigma,
        })
    }

    /// Constructs a normal distribution directly from σ. The variance is
    /// derived as `σ²` in Sq128 (truncating multiplication), making this the
    /// canonical constructor for market makers who quote σ rather than σ².
    ///
    /// Bit-parity contract: the resulting `(variance, sigma)` pair always
    /// satisfies the on-chain `sqrt_verified` check by construction —
    /// `variance == floor(σ × σ)` is by definition the largest value whose
    /// floor sqrt is `σ`.
    pub fn from_sigma(mean: Sq128, sigma: Sq128) -> Result<Self, CoreError> {
        if sigma.is_negative() {
            return Err(CoreError::invalid_input("sigma", "must be ≥ 0"));
        }
        let variance = sigma.checked_mul(sigma)?;
        Ok(Self {
            mean,
            variance,
            sigma,
        })
    }

    /// Constructs a normal distribution with a precomputed σ. Use when σ
    /// arrives from chain state already.
    pub fn with_sigma(mean: Sq128, variance: Sq128, sigma: Sq128) -> Result<Self, CoreError> {
        if variance.is_negative() {
            return Err(CoreError::invalid_input("variance", "must be ≥ 0"));
        }
        if sigma.is_negative() {
            return Err(CoreError::invalid_input("sigma", "must be ≥ 0"));
        }
        if variance.is_zero() && !sigma.is_zero() {
            return Err(CoreError::invalid_input(
                "sigma",
                "must be 0 when variance is 0",
            ));
        }
        Ok(Self {
            mean,
            variance,
            sigma,
        })
    }

    /// Decodes from the on-wire DTO.
    pub fn from_raw(raw: NormalDistributionRaw) -> Result<Self, CoreError> {
        Self::with_sigma(
            Sq128::from_raw(raw.mean),
            Sq128::from_raw(raw.variance),
            Sq128::from_raw(raw.sigma),
        )
    }
}

impl Distribution for NormalDistribution {
    type Raw = NormalDistributionRaw;

    fn mean(&self) -> Sq128 {
        self.mean
    }
    fn variance(&self) -> Sq128 {
        self.variance
    }
    fn sigma(&self) -> Sq128 {
        self.sigma
    }

    fn pdf(&self, x: Sq128) -> Result<Sq128, CoreError> {
        if self.sigma.is_zero() {
            return Ok(Sq128::ZERO);
        }
        let mu = self.mean.to_f64();
        let sigma = self.sigma.to_f64();
        let z = (x.to_f64() - mu) / sigma;
        let value = (-0.5 * z * z).exp() / (sigma * libm_compat::sqrt_2pi());
        Sq128::from_f64(value)
    }

    fn pdf_derivative(&self, x: Sq128) -> Result<Sq128, CoreError> {
        if self.sigma.is_zero() {
            return Ok(Sq128::ZERO);
        }
        let mu = self.mean.to_f64();
        let var = self.variance.to_f64();
        let pdf = self.pdf(x)?.to_f64();
        let value = -((x.to_f64() - mu) / var) * pdf;
        Sq128::from_f64(value)
    }

    fn pdf_second_derivative(&self, x: Sq128) -> Result<Sq128, CoreError> {
        if self.sigma.is_zero() {
            return Ok(Sq128::ZERO);
        }
        let mu = self.mean.to_f64();
        let var = self.variance.to_f64();
        let pdf = self.pdf(x)?.to_f64();
        let xm = x.to_f64() - mu;
        let value = ((xm * xm) / (var * var) - 1.0 / var) * pdf;
        Sq128::from_f64(value)
    }

    fn to_raw(&self) -> Self::Raw {
        NormalDistributionRaw {
            mean: self.mean.to_raw(),
            variance: self.variance.to_raw(),
            sigma: self.sigma.to_raw(),
        }
    }
}

// ─── Lognormal ───────────────────────────────────────────────────────────────

/// On-wire shape of a [`LognormalDistribution`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LognormalDistributionRaw {
    /// Log-space mean `μ`.
    pub mu: Sq128Raw,
    /// Log-space variance `σ²`.
    pub variance: Sq128Raw,
    /// Log-space σ (precomputed).
    pub sigma: Sq128Raw,
}

/// Lognormal distribution: `ln(X) ~ N(μ, σ²)`.
#[derive(Debug, Clone, Copy)]
pub struct LognormalDistribution {
    mu: Sq128,
    variance: Sq128,
    sigma: Sq128,
}

impl LognormalDistribution {
    /// Constructs from μ and variance, computing σ via the bit-exact Q128.128
    /// floor square root (`Sq128::sqrt`).
    ///
    /// As with [`NormalDistribution::from_variance`], the produced σ obeys
    /// the on-chain `sqrt_verified` invariant for every non-negative
    /// variance — see `docs/SQ128_SQRT.md`.
    pub fn from_variance(mu: Sq128, variance: Sq128) -> Result<Self, CoreError> {
        if variance.is_negative() {
            return Err(CoreError::invalid_input("variance", "must be ≥ 0"));
        }
        let sigma = variance.sqrt()?;
        Ok(Self {
            mu,
            variance,
            sigma,
        })
    }

    /// Constructs a lognormal directly from σ. The variance is derived as
    /// `σ²` in Sq128 — the canonical market-maker constructor.
    pub fn from_sigma(mu: Sq128, sigma: Sq128) -> Result<Self, CoreError> {
        if sigma.is_negative() {
            return Err(CoreError::invalid_input("sigma", "must be ≥ 0"));
        }
        let variance = sigma.checked_mul(sigma)?;
        Ok(Self {
            mu,
            variance,
            sigma,
        })
    }

    /// Constructs with a precomputed σ.
    pub fn with_sigma(mu: Sq128, variance: Sq128, sigma: Sq128) -> Result<Self, CoreError> {
        if variance.is_negative() {
            return Err(CoreError::invalid_input("variance", "must be ≥ 0"));
        }
        if sigma.is_negative() {
            return Err(CoreError::invalid_input("sigma", "must be ≥ 0"));
        }
        if variance.is_zero() && !sigma.is_zero() {
            return Err(CoreError::invalid_input(
                "sigma",
                "must be 0 when variance is 0",
            ));
        }
        Ok(Self {
            mu,
            variance,
            sigma,
        })
    }

    /// Decodes from the on-wire DTO.
    pub fn from_raw(raw: LognormalDistributionRaw) -> Result<Self, CoreError> {
        Self::with_sigma(
            Sq128::from_raw(raw.mu),
            Sq128::from_raw(raw.variance),
            Sq128::from_raw(raw.sigma),
        )
    }

    /// Returns the log-space mean.
    #[must_use]
    pub const fn mu(&self) -> Sq128 {
        self.mu
    }

    /// Returns `true` when `x` lies in positive support.
    #[must_use]
    pub fn is_in_support(&self, x: Sq128) -> bool {
        let value = x.to_f64();
        value.is_finite() && value > 1e-15_f64
    }
}

impl Distribution for LognormalDistribution {
    type Raw = LognormalDistributionRaw;

    fn mean(&self) -> Sq128 {
        self.mu
    }
    fn variance(&self) -> Sq128 {
        self.variance
    }
    fn sigma(&self) -> Sq128 {
        self.sigma
    }

    fn pdf(&self, x: Sq128) -> Result<Sq128, CoreError> {
        let x_num = x.to_f64();
        let sigma = self.sigma.to_f64();
        if x_num <= 0.0 || sigma <= 0.0 || !x_num.is_finite() {
            return Ok(Sq128::ZERO);
        }
        let mu = self.mu.to_f64();
        let var = self.variance.to_f64();
        let log_term = libm_compat::ln_f64(x_num) - mu;
        let exponent = -(log_term * log_term) / (2.0 * var);
        let denom = x_num * sigma * libm_compat::sqrt_2pi();
        if denom <= 0.0 || !denom.is_finite() {
            return Err(CoreError::OutOfSupport {
                distribution: "lognormal",
                value: alloc::format!("{x_num}"),
            });
        }
        Sq128::from_f64(exponent.exp() / denom)
    }

    fn pdf_derivative(&self, x: Sq128) -> Result<Sq128, CoreError> {
        let x_num = x.to_f64();
        if x_num <= 0.0 {
            return Ok(Sq128::ZERO);
        }
        let pdf = self.pdf(x)?.to_f64();
        let mu = self.mu.to_f64();
        let var = self.variance.to_f64();
        let u = 1.0 + (libm_compat::ln_f64(x_num) - mu) / var;
        Sq128::from_f64(pdf * (-u / x_num))
    }

    fn pdf_second_derivative(&self, x: Sq128) -> Result<Sq128, CoreError> {
        let x_num = x.to_f64();
        if x_num <= 0.0 {
            return Ok(Sq128::ZERO);
        }
        let pdf = self.pdf(x)?.to_f64();
        let mu = self.mu.to_f64();
        let var = self.variance.to_f64();
        let u = 1.0 + (libm_compat::ln_f64(x_num) - mu) / var;
        let second = pdf * (u * u + u - 1.0 / var) / (x_num * x_num);
        Sq128::from_f64(second)
    }

    fn to_raw(&self) -> Self::Raw {
        LognormalDistributionRaw {
            mu: self.mu.to_raw(),
            variance: self.variance.to_raw(),
            sigma: self.sigma.to_raw(),
        }
    }
}

// ─── libm shim ───────────────────────────────────────────────────────────────
//
// The PDFs above use `exp` and `ln`. Under `std` these are intrinsics on
// `f64`; under `no_std` we'd need `libm`. We isolate the call sites here so
// adding a `libm` feature is a one-line change in a future PR. (σ is now
// computed in Sq128 directly — see `Sq128::sqrt` — so no f64 sqrt shim is
// needed any more.)
mod libm_compat {
    /// `ln(value)`.
    #[inline]
    pub(super) fn ln_f64(value: f64) -> f64 {
        #[cfg(feature = "std")]
        {
            value.ln()
        }
        #[cfg(not(feature = "std"))]
        {
            let _ = value;
            f64::NAN
        }
    }

    /// `sqrt(2π)` — precomputed at full f64 precision.
    #[inline]
    pub(super) const fn sqrt_2pi() -> f64 {
        2.506_628_274_631_000_7_f64
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use super::*;

    #[test]
    fn normal_round_trips_via_raw() {
        let mean = Sq128::from_i128(100);
        let variance = Sq128::from_i128(4);
        let dist = NormalDistribution::from_variance(mean, variance).unwrap();
        let raw = dist.to_raw();
        let back = NormalDistribution::from_raw(raw).unwrap();
        assert_eq!(back.mean(), mean);
        assert_eq!(back.variance(), variance);
    }

    #[test]
    fn normal_pdf_peak_is_at_mean() {
        let dist =
            NormalDistribution::from_variance(Sq128::from_i128(0), Sq128::from_i128(1)).unwrap();
        let at_mean = dist.pdf(Sq128::ZERO).unwrap().to_f64();
        let off_mean = dist.pdf(Sq128::from_i128(1)).unwrap().to_f64();
        assert!(at_mean > off_mean, "{at_mean} should exceed {off_mean}");
    }

    #[test]
    fn lognormal_rejects_negative_variance() {
        let result = LognormalDistribution::from_variance(Sq128::ZERO, Sq128::from_i128(-1));
        assert!(matches!(result, Err(CoreError::InvalidInput { .. })));
    }

    #[test]
    fn lognormal_pdf_zero_outside_support() {
        let dist = LognormalDistribution::from_variance(Sq128::ZERO, Sq128::one()).unwrap();
        let result = dist.pdf(Sq128::from_i128(-5)).unwrap();
        assert!(
            result.is_zero(),
            "PDF must be zero outside positive support"
        );
    }

    // ─── from_variance Sq128 round-trip ────────────────────────────────────

    /// Helper: the σ × σ ≤ variance + (variance − σ²) < 2σ + ε invariant.
    /// Matches Cairo's `sqrt_verified` exactly.
    fn assert_sqrt_verified_invariant(variance: Sq128, sigma: Sq128) {
        let sigma_sq = sigma.checked_mul(sigma).unwrap();
        assert!(
            sigma_sq <= variance,
            "σ² ({}) > variance ({})",
            sigma_sq.to_f64(),
            variance.to_f64()
        );
        let gap = variance.checked_sub(sigma_sq).unwrap();
        let two_sigma = sigma.checked_add(sigma).unwrap();
        let threshold = Sq128::new(
            two_sigma.magnitude() + ruint::aliases::U256::from(1_u64),
            false,
        );
        assert!(
            gap < threshold,
            "variance − σ² ({}) ≥ 2σ + ε ({}) — σ is not the floor sqrt",
            gap.to_f64(),
            threshold.to_f64()
        );
    }

    #[test]
    fn normal_from_variance_round_trip() {
        for v in [0.04_f64, 0.09, 0.13, 0.5, 1.0, 4.0, 100.7, 1e-6, 1e10] {
            let variance = Sq128::from_f64(v).unwrap();
            let dist = NormalDistribution::from_variance(Sq128::ZERO, variance).unwrap();
            assert_eq!(
                dist.variance(),
                variance,
                "variance must be preserved exactly for v={v}"
            );
            assert_sqrt_verified_invariant(variance, dist.sigma());
        }
    }

    #[test]
    fn lognormal_from_variance_round_trip() {
        for v in [0.04_f64, 0.09, 0.13, 0.5, 1.0, 4.0, 100.7, 1e-6, 1e10] {
            let variance = Sq128::from_f64(v).unwrap();
            let dist = LognormalDistribution::from_variance(Sq128::ZERO, variance).unwrap();
            assert_eq!(dist.variance(), variance);
            assert_sqrt_verified_invariant(variance, dist.sigma());
        }
    }

    #[test]
    fn normal_from_sigma_round_trip() {
        // σ supplied directly; variance must equal σ × σ exactly.
        for s in [0.2_f64, 0.3, 0.5, 1.0, 1.5, 10.0, 0.001, 100.0] {
            let sigma = Sq128::from_f64(s).unwrap();
            let dist = NormalDistribution::from_sigma(Sq128::ZERO, sigma).unwrap();
            assert_eq!(dist.sigma(), sigma);
            let expected_var = sigma.checked_mul(sigma).unwrap();
            assert_eq!(
                dist.variance(),
                expected_var,
                "variance must equal σ × σ exactly for σ={s}"
            );
            // And the chain invariant still holds (it always does for
            // sigma² → sqrt round-trips by construction).
            assert_sqrt_verified_invariant(dist.variance(), sigma);
        }
    }

    #[test]
    fn lognormal_from_sigma_round_trip() {
        for s in [0.2_f64, 0.3, 0.5, 1.0, 1.5, 10.0, 0.001, 100.0] {
            let sigma = Sq128::from_f64(s).unwrap();
            let dist = LognormalDistribution::from_sigma(Sq128::ZERO, sigma).unwrap();
            assert_eq!(dist.sigma(), sigma);
            assert_eq!(dist.variance(), sigma.checked_mul(sigma).unwrap());
        }
    }

    #[test]
    fn from_sigma_rejects_negative() {
        let normal_err = NormalDistribution::from_sigma(Sq128::ZERO, Sq128::from_i128(-1))
            .expect_err("must reject negative σ");
        assert!(matches!(normal_err, CoreError::InvalidInput { .. }));
        let lognormal_err = LognormalDistribution::from_sigma(Sq128::ZERO, Sq128::from_i128(-1))
            .expect_err("must reject negative σ");
        assert!(matches!(lognormal_err, CoreError::InvalidInput { .. }));
    }
}
