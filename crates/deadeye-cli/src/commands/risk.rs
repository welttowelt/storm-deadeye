//! Risk & sizing math for `trade quote` (issues #15 + #24).
//!
//! Everything here is **pure f64 display math** — it informs the trader; the
//! chain-bit-exact collateral/verification path is untouched. Three jobs:
//!
//! 1. **Downside**: P&L of the candidate move at specific settlement values
//!    ("if it resolves at the market mean, you lose Y XP") and CVaR@5% under
//!    the belief.
//! 2. **Sizing**: a principled stake from `(edge, bankroll, kelly fraction)`
//!    instead of hand-tuned belief-σ — keep the forecast and the bet decoupled
//!    (issue #15's core complaint).
//! 3. **Lint**: warnings (never blocks) for the classic self-deceptions — σ
//!    tighter than the market's, candidates moving against the market without
//!    belief support, stakes disproportionate to the edge.

/// Scoring-rule λ scale: `λ(σ, k) = k·√(2σ√π)` (mirrors
/// `deadeye_collateral::lambda`).
fn lambda(sigma: f64, k: f64) -> f64 {
    if sigma <= 0.0 || k <= 0.0 {
        return 0.0;
    }
    k * (2.0 * sigma * core::f64::consts::PI.sqrt()).sqrt()
}

fn normal_pdf(x: f64, mean: f64, sigma: f64) -> f64 {
    if sigma <= 0.0 {
        return 0.0;
    }
    let z = (x - mean) / sigma;
    (-0.5 * z * z).exp() / (sigma * (2.0 * core::f64::consts::PI).sqrt())
}

/// `∫ N(x; μ1, σ1)·N(x; μ2, σ2) dx = N(μ1 − μ2; 0, σ1² + σ2²)` — the closed
/// form behind the optimizer's EV.
fn gaussian_product_integral(mu1: f64, sigma1: f64, mu2: f64, sigma2: f64) -> f64 {
    let var = sigma1.mul_add(sigma1, sigma2 * sigma2);
    if var <= 0.0 {
        return 0.0;
    }
    let diff = mu1 - mu2;
    (-0.5 * diff * diff / var).exp() / (2.0 * core::f64::consts::PI * var).sqrt()
}

/// Settlement P&L (XP) of moving the market `(μ_f, σ_f) → (μ_g, σ_g)` if the
/// market settles at `x`: `λ_g·pdf_g(x) − λ_f·pdf_f(x)`. Bounded below by
/// `−collateral` on-chain.
#[must_use]
pub(crate) fn pnl_at(
    market_mean: f64,
    market_sigma: f64,
    cand_mean: f64,
    cand_sigma: f64,
    k: f64,
    x: f64,
) -> f64 {
    lambda(cand_sigma, k).mul_add(
        normal_pdf(x, cand_mean, cand_sigma),
        -(lambda(market_sigma, k) * normal_pdf(x, market_mean, market_sigma)),
    )
}

/// Expected settlement P&L (XP) under the belief `N(μ_b, σ_b)` — same closed
/// form the optimizer maximizes.
#[must_use]
pub(crate) fn expected_pnl(
    market_mean: f64,
    market_sigma: f64,
    cand_mean: f64,
    cand_sigma: f64,
    k: f64,
    belief_mean: f64,
    belief_sigma: f64,
) -> f64 {
    lambda(cand_sigma, k).mul_add(
        gaussian_product_integral(cand_mean, cand_sigma, belief_mean, belief_sigma),
        -(lambda(market_sigma, k)
            * gaussian_product_integral(market_mean, market_sigma, belief_mean, belief_sigma)),
    )
}

/// CVaR@α of the settlement P&L under the belief: average P&L over the worst
/// α-tail of belief-weighted outcomes. Grid evaluation over ±5σ.
#[must_use]
#[expect(clippy::too_many_arguments, reason = "plain display-math inputs")]
pub(crate) fn cvar_under_belief(
    market_mean: f64,
    market_sigma: f64,
    cand_mean: f64,
    cand_sigma: f64,
    k: f64,
    belief_mean: f64,
    belief_sigma: f64,
    alpha: f64,
) -> f64 {
    const N: usize = 801;
    if belief_sigma <= 0.0 || !(0.0..1.0).contains(&alpha) || alpha <= 0.0 {
        return f64::NAN;
    }
    let lo = belief_sigma.mul_add(-5.0, belief_mean);
    let hi = belief_sigma.mul_add(5.0, belief_mean);
    let step = (hi - lo) / (N as f64 - 1.0);
    let mut samples: Vec<(f64, f64)> = (0..N)
        .map(|i| {
            let x = step.mul_add(i as f64, lo);
            let w = normal_pdf(x, belief_mean, belief_sigma);
            let pnl = pnl_at(market_mean, market_sigma, cand_mean, cand_sigma, k, x);
            (pnl, w)
        })
        .collect();
    samples.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
    let total_w: f64 = samples.iter().map(|(_, w)| w).sum();
    if total_w <= 0.0 {
        return f64::NAN;
    }
    let tail_w = total_w * alpha;
    let mut acc_w = 0.0;
    let mut acc = 0.0;
    for (pnl, w) in samples {
        let take = (tail_w - acc_w).min(w);
        if take <= 0.0 {
            break;
        }
        acc += pnl * take;
        acc_w += take;
    }
    if acc_w > 0.0 { acc / acc_w } else { f64::NAN }
}

/// Risk preset → Kelly fraction (issue #15).
#[must_use]
pub(crate) fn preset_fraction(preset: &str) -> Option<f64> {
    match preset {
        "conservative" => Some(0.25),
        "balanced" => Some(0.5),
        "aggressive" => Some(1.0),
        _ => None,
    }
}

/// Sizing advice derived from edge and bankroll (issues #15 + #24).
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct SizingAdvice {
    /// Edge per XP at risk: `EV / collateral` (max loss = locked collateral).
    pub(crate) edge_per_xp: f64,
    /// Full-Kelly bankroll fraction (capped at 1).
    pub(crate) full_kelly_fraction: f64,
    /// The applied Kelly multiplier (preset or `--kelly`).
    pub(crate) kelly_multiplier: f64,
    /// Recommended stake in XP (`min(bankroll·kelly·f*, bankroll)`).
    pub(crate) recommended_stake_xp: f64,
}

/// Kelly-style stake: treat the locked collateral as the max loss, so the
/// edge-over-odds ratio is `f* = EV / collateral`. Recommended stake is
/// `kelly · f* · bankroll`, capped at the bankroll.
#[must_use]
pub(crate) fn sizing_advice(
    expected_value: f64,
    collateral: f64,
    bankroll: f64,
    kelly_multiplier: f64,
) -> Option<SizingAdvice> {
    if !(expected_value.is_finite() && collateral > 0.0 && bankroll > 0.0) {
        return None;
    }
    let edge_per_xp = expected_value / collateral;
    let full_kelly_fraction = edge_per_xp.clamp(0.0, 1.0);
    let recommended = (kelly_multiplier * full_kelly_fraction * bankroll).clamp(0.0, bankroll);
    Some(SizingAdvice {
        edge_per_xp,
        full_kelly_fraction,
        kelly_multiplier,
        recommended_stake_xp: recommended,
    })
}

/// Resolve the fractional-Kelly stake cap (issue #33). `probe_ev` /
/// `probe_collateral` come from an unconstrained quote at the full budget —
/// the edge estimate the Kelly fraction is computed from. Returns `None`
/// when no Kelly policy is active; errors when --kelly/--risk lack
/// --bankroll.
pub(crate) fn kelly_stake_cap(
    bankroll: Option<f64>,
    kelly_multiplier: Option<f64>,
    probe_ev: f64,
    probe_collateral: f64,
) -> anyhow::Result<Option<(f64, String)>> {
    let Some(mult) = kelly_multiplier else {
        return Ok(None);
    };
    let bankroll = bankroll.ok_or_else(|| {
        anyhow::anyhow!(
            "--kelly/--risk size the stake from a bankroll — pass --bankroll <XP> as well"
        )
    })?;
    let Some(advice) = sizing_advice(probe_ev, probe_collateral, bankroll, mult) else {
        return Ok(None);
    };
    Ok(Some((
        advice.recommended_stake_xp,
        format!("kelly-{mult:.2}"),
    )))
}

/// Pre-trade lint (issue #24): warn, never block. Each warning names the
/// specific reason.
#[must_use]
#[expect(clippy::too_many_arguments, reason = "plain display-math inputs")]
pub(crate) fn lint_quote(
    belief: Option<(f64, f64)>,
    market_mean: f64,
    market_sigma: f64,
    cand_mean: f64,
    cand_sigma: f64,
    sigma_floor: Option<f64>,
    stake: Option<f64>,
    advice: Option<&SizingAdvice>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if let Some((belief_mean, belief_sigma)) = belief {
        let belief_side = belief_mean - market_mean;
        let cand_side = cand_mean - market_mean;
        if belief_side * cand_side < 0.0 {
            warnings.push(format!(
                "candidate mean {cand_mean:.4} moves AWAY from your belief side of the market \
                 (belief {belief_mean:.4} vs market {market_mean:.4}) — re-check inputs",
            ));
        }
        if belief_sigma < market_sigma {
            warnings.push(format!(
                "belief σ {belief_sigma:.4} is TIGHTER than the market σ {market_sigma:.4} — \
                 claiming to be sharper than the pool needs recorded evidence; never tighten σ \
                 to bet more (use --bankroll/--kelly to size instead)",
            ));
        }
    }
    if let Some(floor) = sigma_floor
        && cand_sigma < floor
    {
        warnings.push(format!(
            "candidate σ {cand_sigma:.4} is below the backing σ-floor {floor:.4} — the chain \
             will reject with SIGMA_TOO_LOW",
        ));
    }
    if let (Some(stake), Some(advice)) = (stake, advice)
        && stake > advice.recommended_stake_xp * 1.5
        && advice.recommended_stake_xp > 0.0
    {
        warnings.push(format!(
            "stake {stake:.2} XP is >1.5× the Kelly recommendation \
             ({:.2} XP at {:.0}% Kelly) — size disproportionate to the recorded edge",
            advice.recommended_stake_xp,
            advice.kelly_multiplier * 100.0,
        ));
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    const K: f64 = 200.0;

    #[test]
    fn pnl_is_zero_for_identity_move() {
        let p = pnl_at(4.2, 0.1, 4.2, 0.1, K, 4.25);
        assert!(p.abs() < 1e-12);
    }

    #[test]
    fn ev_positive_when_belief_matches_candidate_side() {
        // Belief below the market: moving the mean down has positive EV.
        let ev = expected_pnl(4.20, 0.10, 4.17, 0.10, K, 4.16, 0.08);
        assert!(ev > 0.0, "{ev}");
        // And the mirrored (wrong-side) move has negative EV.
        let bad = expected_pnl(4.20, 0.10, 4.23, 0.10, K, 4.16, 0.08);
        assert!(bad < 0.0, "{bad}");
    }

    #[test]
    fn cvar_is_below_expected_value() {
        let ev = expected_pnl(4.20, 0.10, 4.17, 0.10, K, 4.16, 0.08);
        let cvar = cvar_under_belief(4.20, 0.10, 4.17, 0.10, K, 4.16, 0.08, 0.05);
        assert!(cvar.is_finite());
        assert!(cvar < ev, "cvar {cvar} should be worse than ev {ev}");
    }

    #[test]
    fn kelly_scales_with_fraction_and_edge() {
        let half = sizing_advice(5.0, 100.0, 1000.0, 0.5).expect("advice");
        let full = sizing_advice(5.0, 100.0, 1000.0, 1.0).expect("advice");
        assert!((half.recommended_stake_xp - 25.0).abs() < 1e-9); // 0.5·0.05·1000
        assert!((full.recommended_stake_xp - 50.0).abs() < 1e-9);
        assert!(sizing_advice(f64::NAN, 100.0, 1000.0, 0.5).is_none());
    }

    #[test]
    fn presets_map_to_fractions() {
        assert_eq!(preset_fraction("conservative"), Some(0.25));
        assert_eq!(preset_fraction("balanced"), Some(0.5));
        assert_eq!(preset_fraction("aggressive"), Some(1.0));
        assert_eq!(preset_fraction("yolo"), None);
    }

    #[test]
    fn lint_flags_the_three_failure_modes() {
        let advice = sizing_advice(2.0, 100.0, 1000.0, 0.5).expect("advice");
        let warnings = lint_quote(
            Some((4.16, 0.05)), // tighter than market σ 0.10
            4.20,
            0.10,
            4.23, // moves away from belief side
            0.04, // below floor
            Some(0.06),
            Some(500.0), // way above recommendation
            Some(&advice),
        );
        assert_eq!(warnings.len(), 4, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("AWAY")));
        assert!(warnings.iter().any(|w| w.contains("TIGHTER")));
        assert!(warnings.iter().any(|w| w.contains("SIGMA_TOO_LOW")));
        assert!(warnings.iter().any(|w| w.contains("Kelly")));
    }

    #[test]
    fn clean_quote_produces_no_warnings() {
        let warnings = lint_quote(
            Some((4.16, 0.12)),
            4.20,
            0.10,
            4.18,
            0.11,
            Some(0.06),
            None,
            None,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}
