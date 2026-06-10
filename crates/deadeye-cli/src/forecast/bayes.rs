#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::suboptimal_flops,
    reason = "forecasting math operates on small f64 sample counts and \
              prioritizes readable formulas over micro-optimized FLOPs"
)]
//! Bayesian + distributional forecasting toolkit.
//!
//! A pure, dependency-light port of the superforecaster primitives we lifted
//! from the hermes forecasting engine, retargeted at Deadeye's **continuous
//! distribution markets**. Two spaces:
//!
//! * **Probability space** — likelihood-ratio updates, log-odds pooling,
//!   reference-class (base-rate) blending, evidence weighting, double-counting
//!   collapse, and market de-vig. Use for binary sub-questions and for turning
//!   the market's own state into a prior.
//! * **Normal space** — weighted, correlation-aware aggregation of component
//!   `(μ, σ)` beliefs into a single `(mean, sd)` with quantiles and downside.
//!   This is the bridge to the trade optimizer: the aggregate `mean` and
//!   `sd²` (variance) feed `deadeye trade quote --mean --variance`.
//!
//! Everything here is total and side-effect free so it is trivially testable
//! and safe to call from the CLI or an agent loop.

use std::f64::consts::PI;

/// Probabilities are clamped into this open interval so odds/logit stay finite.
const EPS: f64 = 1e-9;
/// One-sided z for a 90% central interval (q05 / q95 tails).
const Z90: f64 = 1.645;

/// Clamp a probability into `(EPS, 1 - EPS)`.
#[must_use]
pub(crate) fn clamp_prob(p: f64) -> f64 {
    p.clamp(EPS, 1.0 - EPS)
}

/// Convert probability to odds `p / (1 - p)`.
#[must_use]
pub(crate) fn prob_to_odds(p: f64) -> f64 {
    let p = clamp_prob(p);
    p / (1.0 - p)
}

/// Convert odds back to probability `o / (1 + o)`.
#[must_use]
pub(crate) fn odds_to_prob(odds: f64) -> f64 {
    let odds = odds.max(0.0);
    odds / (1.0 + odds)
}

/// Log-odds (logit) of a probability.
#[must_use]
pub(crate) fn logit(p: f64) -> f64 {
    prob_to_odds(p).ln()
}

/// Inverse logit (logistic sigmoid).
#[must_use]
pub(crate) fn inv_logit(x: f64) -> f64 {
    if x >= 0.0 {
        let z = (-x).exp();
        1.0 / (1.0 + z)
    } else {
        let z = x.exp();
        z / (1.0 + z)
    }
}

/// Apply one likelihood ratio to a prior in odds space.
#[must_use]
pub(crate) fn apply_lr(prior: f64, lr: f64) -> f64 {
    odds_to_prob(prob_to_odds(prior) * lr.max(0.0))
}

/// Apply a chain of independent likelihood ratios.
#[must_use]
pub(crate) fn apply_lrs(prior: f64, lrs: &[f64]) -> f64 {
    lrs.iter().fold(clamp_prob(prior), |p, &lr| apply_lr(p, lr))
}

/// Weighted **log-odds pool** of several probabilities — the geometric mean of
/// odds. The superforecaster default: it respects confident minority views and
/// never lands outside the inputs. Returns `0.5` for an empty/zero-weight set.
#[must_use]
pub(crate) fn log_odds_pool(weighted: &[(f64, f64)]) -> f64 {
    let total_w: f64 = weighted.iter().map(|&(_, w)| w.max(0.0)).sum();
    if total_w <= 0.0 {
        return 0.5;
    }
    let acc: f64 = weighted
        .iter()
        .map(|&(p, w)| w.max(0.0) * logit(p))
        .sum::<f64>()
        / total_w;
    inv_logit(acc)
}

/// Weighted arithmetic (linear) pool of probabilities.
#[must_use]
pub(crate) fn linear_pool(weighted: &[(f64, f64)]) -> f64 {
    let total_w: f64 = weighted.iter().map(|&(_, w)| w.max(0.0)).sum();
    if total_w <= 0.0 {
        return 0.5;
    }
    weighted
        .iter()
        .map(|&(p, w)| w.max(0.0) * clamp_prob(p))
        .sum::<f64>()
        / total_w
}

/// Sharpen (or soften) a probability away from `0.5` in log-odds space.
/// `factor > 1` extremizes; only justified for genuinely independent agreeing
/// sources. `factor == 1` is a no-op.
#[must_use]
pub(crate) fn extremize(p: f64, factor: f64) -> f64 {
    inv_logit(logit(p) * factor.max(0.0))
}

/// Collapse `n` correlated evidence items (shared signal `rho`) into an
/// effective independent count: `n / (1 + (n - 1) * rho)`. Guards against
/// double-counting a cluster of co-moving sources.
#[must_use]
pub(crate) fn effective_independent_count(n: usize, rho: f64) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let n = n as f64;
    let rho = rho.clamp(0.0, 0.999);
    n / (1.0 + (n - 1.0) * rho)
}

// ─── Reference-class (base-rate) blending ────────────────────────────────

/// One reference class contributing to a base rate.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BaseRateClass {
    /// Class base rate in `[0, 1]`.
    pub(crate) base_rate: f64,
    /// How applicable this class is to the question, in `[0, 1]`.
    pub(crate) applicability: f64,
    /// Within-class uncertainty (sd of the base rate), `>= 0`.
    pub(crate) uncertainty: f64,
}

/// Result of blending reference classes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BaseRateBlend {
    /// Applicability-weighted blended base rate in `[0, 1]`.
    pub(crate) blended: f64,
    /// Total uncertainty: between-class disagreement + within-class spread.
    pub(crate) uncertainty_sd: f64,
}

/// Blend reference classes in log-odds space, propagating both between-class
/// disagreement and within-class uncertainty.
#[must_use]
pub(crate) fn blend_base_rates(classes: &[BaseRateClass]) -> BaseRateBlend {
    let total_a: f64 = classes.iter().map(|c| c.applicability.max(0.0)).sum();
    if total_a <= 0.0 {
        return BaseRateBlend {
            blended: 0.5,
            uncertainty_sd: 0.0,
        };
    }
    let pooled_logit: f64 = classes
        .iter()
        .map(|c| c.applicability.max(0.0) * logit(c.base_rate))
        .sum::<f64>()
        / total_a;
    let blended = inv_logit(pooled_logit);
    let between: f64 = classes
        .iter()
        .map(|c| c.applicability.max(0.0) * (c.base_rate - blended).powi(2))
        .sum::<f64>()
        / total_a;
    let within: f64 = classes
        .iter()
        .map(|c| c.applicability.max(0.0) * c.uncertainty.max(0.0).powi(2))
        .sum::<f64>()
        / total_a;
    BaseRateBlend {
        blended,
        uncertainty_sd: (between + within).sqrt(),
    }
}

/// Applicability-weighted blend of **numeric** reference anchors (value space).
/// Use this for continuous markets where `base_rate` is a level (e.g. CPI %),
/// not a probability. Propagates between-class disagreement + within-class sd.
#[must_use]
pub(crate) fn blend_numeric(classes: &[BaseRateClass]) -> NumericBlend {
    let total_a: f64 = classes.iter().map(|c| c.applicability.max(0.0)).sum();
    if total_a <= 0.0 {
        return NumericBlend { mean: 0.0, sd: 0.0 };
    }
    let mean: f64 = classes
        .iter()
        .map(|c| c.applicability.max(0.0) * c.base_rate)
        .sum::<f64>()
        / total_a;
    let between: f64 = classes
        .iter()
        .map(|c| c.applicability.max(0.0) * (c.base_rate - mean).powi(2))
        .sum::<f64>()
        / total_a;
    let within: f64 = classes
        .iter()
        .map(|c| c.applicability.max(0.0) * c.uncertainty.max(0.0).powi(2))
        .sum::<f64>()
        / total_a;
    NumericBlend {
        mean,
        sd: (between + within).sqrt(),
    }
}

/// Result of blending numeric reference anchors.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NumericBlend {
    /// Applicability-weighted mean anchor.
    pub(crate) mean: f64,
    /// Total sd: between-class disagreement + within-class spread.
    pub(crate) sd: f64,
}

// ─── Evidence weighting (qualitative → likelihood ratio) ──────────────────

/// Direction an evidence item pushes the forecast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    /// Supports the hypothesis (LR > 1).
    For,
    /// Cuts against it (LR < 1).
    Against,
    /// No directional signal (LR == 1).
    Neutral,
}

/// Qualitative strength of an evidence item; maps to a base log-odds shift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Strength {
    /// ~0.0 log-odds.
    Negligible,
    /// ~0.4 log-odds.
    Weak,
    /// ~0.7 log-odds.
    Modest,
    /// ~1.1 log-odds.
    Medium,
    /// ~1.8 log-odds.
    Strong,
    /// ~2.5 log-odds.
    VeryStrong,
    /// ~3.2 log-odds.
    Decisive,
}

impl Strength {
    /// Base log-odds magnitude before quality discounting.
    #[must_use]
    pub(crate) const fn base_log_odds(self) -> f64 {
        match self {
            Self::Negligible => 0.0,
            Self::Weak => 0.4,
            Self::Modest => 0.7,
            Self::Medium => 1.1,
            Self::Strong => 1.8,
            Self::VeryStrong => 2.5,
            Self::Decisive => 3.2,
        }
    }
}

/// Qualitative inputs describing one piece of evidence.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EvidenceInput {
    /// Source credibility / track record, `[0, 1]`.
    pub(crate) reliability: f64,
    /// How directly it bears on the question, `[0, 1]`.
    pub(crate) relevance: f64,
    /// Independence from other evidence already counted, `[0, 1]`.
    pub(crate) independence: f64,
    /// Recency / freshness, `[0, 1]`.
    pub(crate) recency: f64,
    /// Risk of partisan or systematic distortion, `[0, 1]` (higher = worse).
    pub(crate) bias_risk: f64,
    /// Which way it points.
    pub(crate) direction: Direction,
    /// How strong the raw signal is.
    pub(crate) strength: Strength,
}

/// Quantified evidence: a likelihood ratio plus the quality that produced it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EvidenceWeight {
    /// Suggested likelihood ratio `exp(log_odds)`.
    pub(crate) likelihood_ratio: f64,
    /// Signed log-odds contribution (additive in logit space).
    pub(crate) log_odds: f64,
    /// Composite quality `[0, 1]` = reliability·relevance·independence·recency·(1−bias).
    pub(crate) quality: f64,
}

/// Convert a qualitative evidence description into a likelihood ratio. Quality
/// discounts the raw strength so weak/biased/correlated sources move little.
#[must_use]
pub(crate) fn evidence_weight(ev: &EvidenceInput) -> EvidenceWeight {
    let q = ev.reliability.clamp(0.0, 1.0)
        * ev.relevance.clamp(0.0, 1.0)
        * ev.independence.clamp(0.0, 1.0)
        * ev.recency.clamp(0.0, 1.0)
        * (1.0 - ev.bias_risk.clamp(0.0, 1.0));
    let sign = match ev.direction {
        Direction::For => 1.0,
        Direction::Against => -1.0,
        Direction::Neutral => 0.0,
    };
    let log_odds = sign * ev.strength.base_log_odds() * q;
    EvidenceWeight {
        likelihood_ratio: log_odds.exp(),
        log_odds,
        quality: q,
    }
}

// ─── Market de-vig ────────────────────────────────────────────────────────

/// Strip the vig from a two-sided binary market: `yes / (yes + no)`.
#[must_use]
pub(crate) fn devig_binary(yes_mid: f64, no_mid: f64) -> f64 {
    let total = yes_mid.max(0.0) + no_mid.max(0.0);
    if total <= 0.0 {
        return 0.5;
    }
    clamp_prob(yes_mid.max(0.0) / total)
}

// ─── Normal / factor aggregation (the bridge to the trade optimizer) ──────

/// Direction of a component belief in normal space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    /// Mean enters with its stated sign.
    Long,
    /// Mean enters negated.
    Short,
}

/// One component `(μ, σ)` belief contributing to an aggregate distribution.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NormalComponent {
    /// Component mean.
    pub(crate) mu: f64,
    /// Component sd `>= 0`. Zero means "no dispersion" and contributes nothing
    /// to variance — we never fabricate spread.
    pub(crate) sigma: f64,
    /// Raw importance weight `>= 0`.
    pub(crate) weight: f64,
    /// Direction the component points.
    pub(crate) side: Side,
}

/// Aggregated normal belief with quantiles and downside.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NormalAggregate {
    /// Weighted aggregate mean.
    pub(crate) mean: f64,
    /// Aggregate sd (correlation-aware).
    pub(crate) sd: f64,
    /// Aggregate variance (`sd²`) — feed this to `trade quote --variance`.
    pub(crate) variance: f64,
    /// 5th percentile (`mean − 1.645·sd`).
    pub(crate) q05: f64,
    /// Median (`== mean`).
    pub(crate) q50: f64,
    /// 95th percentile (`mean + 1.645·sd`).
    pub(crate) q95: f64,
    /// 5% expected shortfall (CVaR) of the lower tail.
    pub(crate) cvar05: f64,
    /// Kish effective sample size of the contributing weights.
    pub(crate) n_eff: f64,
}

/// Aggregate component `(μ, σ)` beliefs into one normal distribution with a
/// shared pairwise correlation `rho` (0 = independent, →1 = fully co-moving).
/// Variance uses the correlated quadratic form `wᵀ(R∘σσᵀ)w`; zero-σ components
/// add no variance. Returns a degenerate aggregate at `0` for empty input.
#[must_use]
pub(crate) fn aggregate_normal(components: &[NormalComponent], rho: f64) -> NormalAggregate {
    let total_w: f64 = components.iter().map(|c| c.weight.max(0.0)).sum();
    if total_w <= 0.0 {
        return NormalAggregate {
            mean: 0.0,
            sd: 0.0,
            variance: 0.0,
            q05: 0.0,
            q50: 0.0,
            q95: 0.0,
            cvar05: 0.0,
            n_eff: 0.0,
        };
    }
    let rho = rho.clamp(0.0, 0.95);
    // Normalized weights and direction-signed means.
    let parts: Vec<(f64, f64, f64)> = components
        .iter()
        .map(|c| {
            let w = c.weight.max(0.0) / total_w;
            let mu = match c.side {
                Side::Long => c.mu,
                Side::Short => -c.mu,
            };
            (w, mu, c.sigma.max(0.0))
        })
        .collect();

    let mean: f64 = parts.iter().map(|&(w, mu, _)| w * mu).sum();
    // Correlated variance: wᵀ (R ∘ σσᵀ) w.
    let mut var = 0.0;
    for (i, &(wi, _, si)) in parts.iter().enumerate() {
        for (j, &(wj, _, sj)) in parts.iter().enumerate() {
            let r = if i == j { 1.0 } else { rho };
            var += wi * wj * r * si * sj;
        }
    }
    let sd = var.max(0.0).sqrt();
    // Kish ESS over the raw weights.
    let raw: Vec<f64> = components.iter().map(|c| c.weight.max(0.0)).collect();
    let sum: f64 = raw.iter().sum();
    let sum_sq: f64 = raw.iter().map(|w| w * w).sum();
    let n_eff = if sum_sq > 0.0 {
        (sum * sum) / sum_sq
    } else {
        0.0
    };
    // 5% expected shortfall of a normal lower tail: mean − sd·φ(z)/α.
    let cvar05 = mean - sd * (normal_pdf(Z90) / 0.05);

    NormalAggregate {
        mean,
        sd,
        variance: var.max(0.0),
        q05: mean - Z90 * sd,
        q50: mean,
        q95: mean + Z90 * sd,
        cvar05,
        n_eff,
    }
}

/// Standard normal probability density.
#[must_use]
pub(crate) fn normal_pdf(z: f64) -> f64 {
    (-(z * z) / 2.0).exp() / (2.0 * PI).sqrt()
}

/// Shrink a forecast toward the market (issue #23): a mixture-blend with
/// weight `edge_strength` on the forecaster. The blended σ uses the mixture
/// variance — it *widens* when the two means disagree, which is exactly the
/// honest treatment of model disagreement.
#[must_use]
pub(crate) fn shrink_to_market(
    my_mu: f64,
    my_sigma: f64,
    market_mu: f64,
    market_sigma: f64,
    edge_strength: f64,
) -> (f64, f64) {
    let e = edge_strength.clamp(0.0, 1.0);
    let mu = e.mul_add(my_mu, (1.0 - e) * market_mu);
    let var = e.mul_add(
        my_sigma * my_sigma,
        (1.0 - e) * market_sigma * market_sigma,
    ) + e * (1.0 - e) * (my_mu - market_mu).powi(2);
    (mu, var.max(0.0).sqrt())
}

/// Closed-form CRPS of a normal forecast `N(mean, sd)` against a realized
/// value: `sd · (z(2Φ(z) − 1) + 2φ(z) − 1/√π)`. Lower is better; same units
/// as the outcome.
#[must_use]
pub(crate) fn crps_normal(mean: f64, sd: f64, realized: f64) -> f64 {
    if sd <= 0.0 {
        return (realized - mean).abs();
    }
    let z = (realized - mean) / sd;
    let pdf = (-0.5 * z * z).exp() / (2.0 * core::f64::consts::PI).sqrt();
    let cdf = normal_cdf(z);
    sd * (z.mul_add(2.0f64.mul_add(cdf, -1.0), 2.0 * pdf) - 1.0 / core::f64::consts::PI.sqrt())
}

/// Standard normal CDF via the Abramowitz–Stegun erf approximation.
#[must_use]
pub(crate) fn normal_cdf(z: f64) -> f64 {
    0.5 * (1.0 + erf(z / std::f64::consts::SQRT_2))
}

/// Probability a `Normal(mean, sd)` lands at or below `x`.
#[must_use]
pub(crate) fn normal_cdf_at(x: f64, mean: f64, sd: f64) -> f64 {
    if sd <= 0.0 {
        return if x >= mean { 1.0 } else { 0.0 };
    }
    normal_cdf((x - mean) / sd)
}

/// Abramowitz–Stegun 7.1.26 error-function approximation (|error| < 1.5e-7).
fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t
            + 0.254_829_592)
            * t
            * (-x * x).exp();
    sign * y
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn odds_logit_roundtrip() {
        for p in [0.01, 0.2, 0.5, 0.73, 0.99] {
            assert!(approx(odds_to_prob(prob_to_odds(p)), p, 1e-9));
            assert!(approx(inv_logit(logit(p)), p, 1e-9));
        }
    }

    #[test]
    fn lr_update_matches_odds_math() {
        // prior 0.5, LR 3 -> odds 1*3=3 -> p=0.75
        assert!(approx(apply_lr(0.5, 3.0), 0.75, 1e-9));
        // chained LRs == product
        assert!(approx(
            apply_lrs(0.5, &[2.0, 2.0]),
            apply_lr(0.5, 4.0),
            1e-9
        ));
    }

    #[test]
    fn log_odds_pool_stays_within_and_respects_minority() {
        let p = log_odds_pool(&[(0.9, 1.0), (0.6, 1.0)]);
        assert!(p > 0.6 && p < 0.9);
        // empty -> 0.5
        assert!(approx(log_odds_pool(&[]), 0.5, 1e-12));
    }

    #[test]
    fn base_rate_blend_propagates_disagreement() {
        let tight = blend_base_rates(&[
            BaseRateClass {
                base_rate: 0.30,
                applicability: 1.0,
                uncertainty: 0.0,
            },
            BaseRateClass {
                base_rate: 0.30,
                applicability: 1.0,
                uncertainty: 0.0,
            },
        ]);
        let wide = blend_base_rates(&[
            BaseRateClass {
                base_rate: 0.10,
                applicability: 1.0,
                uncertainty: 0.0,
            },
            BaseRateClass {
                base_rate: 0.50,
                applicability: 1.0,
                uncertainty: 0.0,
            },
        ]);
        assert!(wide.uncertainty_sd > tight.uncertainty_sd);
    }

    #[test]
    fn numeric_blend_is_value_space_not_logit() {
        // CPI-style anchors (3.0% and 3.4%, both highly applicable) -> ~3.2.
        let b = blend_numeric(&[
            BaseRateClass {
                base_rate: 3.0,
                applicability: 1.0,
                uncertainty: 0.1,
            },
            BaseRateClass {
                base_rate: 3.4,
                applicability: 1.0,
                uncertainty: 0.1,
            },
        ]);
        assert!(approx(b.mean, 3.2, 1e-9));
        assert!(b.sd > 0.1 && b.sd < 0.5); // disagreement + within-class spread
    }

    #[test]
    fn evidence_quality_discounts_weak_sources() {
        let strong = evidence_weight(&EvidenceInput {
            reliability: 1.0,
            relevance: 1.0,
            independence: 1.0,
            recency: 1.0,
            bias_risk: 0.0,
            direction: Direction::For,
            strength: Strength::Strong,
        });
        let biased = evidence_weight(&EvidenceInput {
            reliability: 0.5,
            relevance: 0.5,
            independence: 0.5,
            recency: 0.5,
            bias_risk: 0.5,
            direction: Direction::For,
            strength: Strength::Strong,
        });
        assert!(strong.likelihood_ratio > biased.likelihood_ratio);
        assert!(biased.likelihood_ratio > 1.0); // still supportive
        // Against -> LR < 1
        let against = evidence_weight(&EvidenceInput {
            reliability: 1.0,
            relevance: 1.0,
            independence: 1.0,
            recency: 1.0,
            bias_risk: 0.0,
            direction: Direction::Against,
            strength: Strength::Medium,
        });
        assert!(against.likelihood_ratio < 1.0);
    }

    #[test]
    fn effective_count_collapses_correlated_cluster() {
        assert!(approx(effective_independent_count(5, 0.0), 5.0, 1e-9));
        assert!(effective_independent_count(5, 0.8) < 2.0);
        assert!(approx(effective_independent_count(1, 0.5), 1.0, 1e-9));
    }

    #[test]
    fn devig_removes_overround() {
        // yes 0.55 / no 0.50 (sum 1.05 overround) -> ~0.5238
        assert!(approx(devig_binary(0.55, 0.50), 0.55 / 1.05, 1e-9));
    }

    #[test]
    fn normal_aggregate_independent_vs_correlated() {
        let comps = [
            NormalComponent {
                mu: 2.0,
                sigma: 1.0,
                weight: 1.0,
                side: Side::Long,
            },
            NormalComponent {
                mu: 4.0,
                sigma: 1.0,
                weight: 1.0,
                side: Side::Long,
            },
        ];
        let indep = aggregate_normal(&comps, 0.0);
        let corr = aggregate_normal(&comps, 0.9);
        assert!(approx(indep.mean, 3.0, 1e-9));
        assert!(approx(corr.mean, 3.0, 1e-9));
        // correlation widens variance
        assert!(corr.variance > indep.variance);
        // quantile ordering
        assert!(indep.q05 < indep.q50 && indep.q50 < indep.q95);
        assert!(approx(indep.variance, indep.sd * indep.sd, 1e-12));
    }

    #[test]
    fn short_side_negates_mean() {
        let agg = aggregate_normal(
            &[NormalComponent {
                mu: 5.0,
                sigma: 0.0,
                weight: 1.0,
                side: Side::Short,
            }],
            0.0,
        );
        assert!(approx(agg.mean, -5.0, 1e-9));
        assert!(approx(agg.variance, 0.0, 1e-12)); // zero sigma -> no fabricated spread
    }

    #[test]
    fn shrink_to_market_endpoints() {
        // edge 0 → market; edge 1 → self; midway widens on disagreement.
        let (mu0, sd0) = shrink_to_market(4.16, 0.08, 4.20, 0.10, 0.0);
        assert!((mu0 - 4.20).abs() < 1e-12 && (sd0 - 0.10).abs() < 1e-12);
        let (mu1, sd1) = shrink_to_market(4.16, 0.08, 4.20, 0.10, 1.0);
        assert!((mu1 - 4.16).abs() < 1e-12 && (sd1 - 0.08).abs() < 1e-12);
        let (muh, sdh) = shrink_to_market(4.16, 0.08, 4.20, 0.10, 0.5);
        assert!((muh - 4.18).abs() < 1e-12);
        assert!(sdh > 0.09, "disagreement widens σ: {sdh}");
    }

    #[test]
    fn crps_zero_distance_beats_miss() {
        let hit = crps_normal(4.2, 0.1, 4.2);
        let miss = crps_normal(4.2, 0.1, 4.6);
        assert!(hit > 0.0 && miss > hit);
        // Known closed-form value at z=0: sd·(2φ(0) − 1/√π) ≈ sd·0.2337.
        assert!((hit - 0.1 * 0.233_69).abs() < 1e-4, "{hit}");
    }

    #[test]
    fn normal_cdf_is_sane() {
        assert!(approx(normal_cdf(0.0), 0.5, 1e-6));
        assert!(normal_cdf(1.96) > 0.974 && normal_cdf(1.96) < 0.976);
        assert!(approx(normal_cdf_at(5.0, 5.0, 0.0), 1.0, 1e-12));
    }
}
