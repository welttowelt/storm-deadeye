//! Categorical (multinoulli) distribution.
//!
//! Discrete probability distribution over `N ∈ [1, 81]` outcomes. Unlike
//! [`NormalDistribution`](crate::NormalDistribution), categorical is a
//! pure value-object — its statistics (mean, variance, PDF) only make
//! sense relative to an embedding of outcomes onto the real line, so
//! the type intentionally does **not** implement the
//! [`Distribution`](crate::Distribution) trait.

use alloc::vec::Vec;

use crate::{
    error::CoreError,
    sq128::{Sq128, Sq128Raw},
};

/// Maximum supported outcome count, mirroring the on-chain bound.
pub const MAX_OUTCOMES: usize = 81;

/// On-wire shape of a [`CategoricalDistribution`].
///
/// Each element of `probs` is a Q128.128 probability; the implicit
/// length is encoded in the surrounding Cairo `Array<T>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CategoricalDistributionRaw {
    /// Probability values.
    pub probs: Vec<Sq128Raw>,
}

/// L2 norm hint for a categorical distribution, computed off-chain and
/// supplied as a calldata hint so the on-chain verifier can skip the
/// `sqrt(sum p_i²)` step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CategoricalL2HintRaw {
    /// Precomputed `||p||₂`.
    pub l2_norm_hint: Sq128Raw,
}

/// Discrete probability distribution over `n` outcomes.
///
/// Probabilities are held as `f64` because every off-chain consumer
/// (collateral solver, optimizer, UI) operates in floating-point; the
/// canonical bit-exact representation is [`CategoricalDistributionRaw`].
#[derive(Debug, Clone, PartialEq)]
pub struct CategoricalDistribution {
    probs: Vec<f64>,
}

impl CategoricalDistribution {
    /// Default numerical tolerance used by equality / uniformity checks.
    pub const EPSILON: f64 = 1e-12_f64;

    /// Builds a categorical distribution from a probability vector.
    ///
    /// Validates that:
    /// * `probs.len() ∈ [1, 81]`,
    /// * every `pᵢ ∈ [0, ∞)` is finite,
    /// * `Σpᵢ ≈ 1` within `1e-9`.
    pub fn from_probs(probs: Vec<f64>) -> Result<Self, CoreError> {
        if probs.is_empty() || probs.len() > MAX_OUTCOMES {
            return Err(CoreError::invalid_input(
                "probs",
                alloc::format!("length {} out of [1, {MAX_OUTCOMES}]", probs.len()),
            ));
        }
        let mut sum = 0.0_f64;
        for (i, &p) in probs.iter().enumerate() {
            if !p.is_finite() || p < 0.0 {
                return Err(CoreError::invalid_input(
                    "probs",
                    alloc::format!("p[{i}] = {p} is not a valid probability"),
                ));
            }
            sum += p;
        }
        if (sum - 1.0).abs() > 1e-9 {
            return Err(CoreError::invalid_input(
                "probs",
                alloc::format!("Σp = {sum} (expected ≈ 1.0)"),
            ));
        }
        Ok(Self { probs })
    }

    /// Builds a uniform distribution over `n` outcomes.
    pub fn uniform(n: usize) -> Result<Self, CoreError> {
        if n == 0 || n > MAX_OUTCOMES {
            return Err(CoreError::invalid_input(
                "n",
                alloc::format!("uniform over {n} outcomes is out of range"),
            ));
        }
        let p = 1.0_f64 / (n as f64);
        Ok(Self { probs: vec![p; n] })
    }

    /// Number of outcomes.
    #[must_use]
    pub fn outcome_count(&self) -> usize {
        self.probs.len()
    }

    /// Reads the probability of outcome `i` (0 for out-of-range).
    #[must_use]
    pub fn prob(&self, i: usize) -> f64 {
        self.probs.get(i).copied().unwrap_or(0.0)
    }

    /// Borrow the underlying probability vector.
    #[must_use]
    pub fn probs(&self) -> &[f64] {
        &self.probs
    }

    /// `||p||₂ = √(Σ pᵢ²)`.
    #[must_use]
    pub fn l2_norm(&self) -> f64 {
        let mut sum_sq = 0.0_f64;
        for &p in &self.probs {
            sum_sq += p * p;
        }
        sum_sq.sqrt()
    }

    /// Returns `true` if every outcome has probability `1/n`.
    #[must_use]
    pub fn is_uniform(&self) -> bool {
        if self.probs.is_empty() {
            return true;
        }
        let expected = 1.0_f64 / (self.probs.len() as f64);
        self.probs
            .iter()
            .all(|&p| (p - expected).abs() <= Self::EPSILON)
    }

    /// Returns `true` if exactly one outcome has probability ≈ 1 (point mass).
    #[must_use]
    pub fn is_degenerate(&self) -> bool {
        self.probs
            .iter()
            .filter(|&&p| (p - 1.0).abs() <= Self::EPSILON)
            .count()
            == 1
    }

    /// Returns `true` if `other` has the same outcome count and per-outcome
    /// probabilities within `EPSILON`.
    #[must_use]
    pub fn is_identical(&self, other: &Self) -> bool {
        if self.outcome_count() != other.outcome_count() {
            return false;
        }
        self.probs
            .iter()
            .zip(other.probs.iter())
            .all(|(a, b)| (a - b).abs() <= Self::EPSILON)
    }

    /// Index of the outcome with the highest probability. Ties broken by
    /// first occurrence.
    #[must_use]
    pub fn max_prob_index(&self) -> usize {
        let mut max_i = 0_usize;
        let mut max_p = f64::NEG_INFINITY;
        for (i, &p) in self.probs.iter().enumerate() {
            if p > max_p {
                max_p = p;
                max_i = i;
            }
        }
        max_i
    }

    /// Convert to the on-wire DTO. Probabilities are encoded as Q128.128.
    pub fn to_raw(&self) -> Result<CategoricalDistributionRaw, CoreError> {
        let mut probs = Vec::with_capacity(self.probs.len());
        for &p in &self.probs {
            probs.push(Sq128::from_f64(p)?.to_raw());
        }
        Ok(CategoricalDistributionRaw { probs })
    }

    /// Decode from the on-wire DTO.
    pub fn from_raw(raw: &CategoricalDistributionRaw) -> Result<Self, CoreError> {
        let probs: Vec<f64> = raw
            .probs
            .iter()
            .map(|p| Sq128::from_raw(*p).to_f64())
            .collect();
        Self::from_probs(probs)
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use super::*;

    #[test]
    fn uniform_distribution_round_trips() {
        let d = CategoricalDistribution::uniform(4).unwrap();
        assert_eq!(d.outcome_count(), 4);
        assert!(d.is_uniform());
        assert!(!d.is_degenerate());
        let probs = d.probs().to_vec();
        let again = CategoricalDistribution::from_probs(probs).unwrap();
        assert!(d.is_identical(&again));
    }

    #[test]
    fn degenerate_distribution_detected() {
        let d = CategoricalDistribution::from_probs(vec![1.0, 0.0, 0.0]).unwrap();
        assert!(d.is_degenerate());
        assert_eq!(d.max_prob_index(), 0);
    }

    #[test]
    fn rejects_invalid_sum() {
        let result = CategoricalDistribution::from_probs(vec![0.3, 0.3, 0.3]);
        assert!(matches!(result, Err(CoreError::InvalidInput { .. })));
    }

    #[test]
    fn rejects_negative_probability() {
        let result = CategoricalDistribution::from_probs(vec![0.5, 0.6, -0.1]);
        assert!(matches!(result, Err(CoreError::InvalidInput { .. })));
    }

    #[test]
    fn raw_round_trip() {
        let d = CategoricalDistribution::from_probs(vec![0.2, 0.3, 0.5]).unwrap();
        let raw = d.to_raw().unwrap();
        assert_eq!(raw.probs.len(), 3);
        let back = CategoricalDistribution::from_raw(&raw).unwrap();
        assert!(d.is_identical(&back));
    }

    #[test]
    fn l2_norm_uniform() {
        let d = CategoricalDistribution::uniform(4).unwrap();
        // ||p||₂ = sqrt(4 * (1/4)²) = sqrt(1/4) = 0.5
        assert!((d.l2_norm() - 0.5).abs() < 1e-12);
    }
}
