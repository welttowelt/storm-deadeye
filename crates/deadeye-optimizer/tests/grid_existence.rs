#![allow(
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::float_cmp_const,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::print_stderr,
    clippy::similar_names,
    clippy::too_many_arguments,
    clippy::missing_assert_message,
    clippy::doc_markdown,
    reason = "property tests live at the test-binary root — relaxed lints match the chaos suites"
)]

//! Coverage gap closer: "If the grid contains any positive-net trade, the
//! optimizer MUST return one."
//!
//! ## v0.1.3 — chain-truthful ground truth
//!
//! The pre-v0.1.2 ground truth replicated the SUT's unit mismatch (it
//! compared a **λ-scaled** EV against an **unscaled** collateral) and
//! filtered against the user's real-XP budget on the same unscaled cost.
//! Both the SUT and the ground truth therefore "agreed" on the wrong
//! answer — `net = λ_g·GPI_g − λ_f·GPI_f − (−d_min)` — and the
//! 5 000-case proptest passed trivially while two real bugs in
//! `optimize_normal_trade` went unnoticed (Reviewer B's audit,
//! `docs/REVIEW_ITEM3_DRIVER_B.md` §6).
//!
//! This file's ground truth now mirrors the **chain** semantics that
//! `optimize_quote_offline` already enforces (`crates/deadeye-sdk/src/normal.rs`):
//!
//! * Cost is the **λ-scaled** `max(0, λ_f·f(x*) − λ_g·g(x*))` evaluated
//!   at the audited stationary point `x*` from
//!   [`normal_collateral`].
//! * EV is the same λ-scaled Gaussian-product-integral expression the
//!   optimizer uses (units now match cost).
//! * Budget filter compares the user's XP budget against the λ-scaled
//!   cost (the unit the chain charges).
//!
//! Pre-v0.1.1 the optimizer's inline Newton solver for `collateral_number`
//! mis-converged on equal-μ / σ-only moves, over-estimating cost ~100×
//! and silently filtering out every σ-arbitrage opportunity; v0.1.1
//! rerouted through `deadeye_collateral::normal_collateral` and v0.1.3
//! (this file's companion change) extends the same λ-scaling fix inside
//! `optimize_normal_trade` so internal selection and budget-filtering
//! use the chain unit.
//!
//! Strategy: an independent grid scanner walks the SAME 51×51 candidate
//! lattice the optimizer iterates and computes λ-scaled net via the
//! same `deadeye_collateral` primitives. If that ground truth witnesses
//! a positive-net trade at any lattice point, the optimizer's contract
//! is to return SOME trade with `net > 0` in the chain frame.
//!
//! ## Determinism
//!
//! `failure_persistence: None` keeps the runner sandbox-friendly.

use deadeye_collateral::{MinimizationPolicy, lambda, normal_collateral};
use deadeye_core::{Distribution as _DistributionPdf, NormalDistribution, Sq128};
use deadeye_optimizer::{NormalOptimizationInput, OptimizerConstraints, optimize_normal_trade};
use proptest::prelude::*;

// ── Grid constants ──────────────────────────────────────────────────────
//
// MUST stay in sync with `src/normal.rs::N_SIGMA_SAMPLES`,
// `N_MEAN_SAMPLES`, `DEFAULT_MAX_SIGMA_RATIO`,
// `DEFAULT_MAX_MEAN_SEP_SIGMAS`. Replicated rather than re-exported so the
// ground truth is independent of the system under test.
const N_SIGMA_SAMPLES: u32 = 50;
const N_MEAN_SAMPLES: u32 = 50;
const MAX_SIGMA_RATIO: f64 = 4.0_f64;
const MAX_MEAN_SEP_SIGMAS: f64 = 4.0_f64;

/// Closed-form Gaussian product integral. Mirrors
/// `src/normal.rs::gaussian_product_integral` so the ground-truth EV is
/// numerically identical to what the optimizer computes — we only want to
/// catch grid-filter / convergence holes, not f64-level rounding drift.
fn gaussian_product_integral(mu1: f64, sigma1: f64, mu2: f64, sigma2: f64) -> f64 {
    let sum_var = sigma1.mul_add(sigma1, sigma2 * sigma2);
    if sum_var <= 0.0 {
        return 0.0;
    }
    let diff_mu = mu1 - mu2;
    (1.0 / (2.0 * core::f64::consts::PI * sum_var).sqrt())
        * (-(diff_mu * diff_mu) / (2.0 * sum_var)).exp()
}

/// Chain-frame collateral for the `f → g` transition at AMM parameter `k`.
///
/// Mirrors what `optimize_quote_offline` re-computes after calling
/// `normal_collateral` (crates/deadeye-sdk/src/normal.rs:442-465):
///
/// 1. Find the audited stationary point `x*` via `normal_collateral`.
/// 2. Evaluate `max(0, λ_f · f(x*) − λ_g · g(x*))` with `λ = k / ‖p‖₂`.
///
/// This is the unit the chain charges (`helpers.cairo:155-176`,
/// `helpers.cairo:198-230`). Returning the **unscaled** `-d_min` here —
/// what `normal_collateral.collateral` reports — was the bug that hid
/// the unit-mismatch in `optimize_normal_trade` from Driver A's
/// proptest.
fn lambda_scaled_collateral_at(mu_f: f64, sigma_f: f64, mu_g: f64, sigma_g: f64, k: f64) -> f64 {
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
    let verified = match normal_collateral(&f, &g, MinimizationPolicy::unrestricted()) {
        Ok(v) if v.collateral.is_finite() => v,
        _ => return f64::INFINITY,
    };
    if verified.collateral <= 0.0 {
        return 0.0;
    }
    let lam_f = lambda(sigma_f, k);
    let lam_g = lambda(sigma_g, k);
    let Ok(x_q) = Sq128::from_f64(verified.x_min) else {
        return f64::INFINITY;
    };
    let f_at = f.pdf(x_q).map(Sq128::to_f64).unwrap_or(0.0);
    let g_at = g.pdf(x_q).map(Sq128::to_f64).unwrap_or(0.0);
    lam_f.mul_add(f_at, -(lam_g * g_at)).max(0.0)
}

fn expected_value_at(
    mu_f: f64,
    sigma_f: f64,
    mu_g: f64,
    sigma_g: f64,
    k: f64,
    belief_mu: f64,
    belief_sigma: f64,
) -> f64 {
    let lam_g = lambda(sigma_g, k);
    let lam_f = lambda(sigma_f, k);
    lam_g.mul_add(
        gaussian_product_integral(mu_g, sigma_g, belief_mu, belief_sigma),
        -(lam_f * gaussian_product_integral(mu_f, sigma_f, belief_mu, belief_sigma)),
    )
}

/// Independent grid scanner — the **chain-truthful ground truth**.
///
/// Returns `Some((best_net, best_mu, best_sigma))` if any grid point has
/// `net = ev − λ-scaled-cost > 0` and the λ-scaled cost ≤ budget;
/// `None` otherwise.
///
/// Iterates the same 51 × 51 lattice the optimizer iterates. Uses the
/// chain-frame cost (`max(0, λ_f f(x*) − λ_g g(x*))`) so the proptest
/// catches the optimizer's unit mismatch instead of co-mismatching with
/// it.
fn grid_scan_ground_truth(
    mu_b: f64,
    sigma_b: f64,
    mu_m: f64,
    sigma_m: f64,
    budget: f64,
    k: f64,
) -> Option<(f64, f64, f64)> {
    if budget <= 0.0 || sigma_m <= 0.0 || k <= 0.0 {
        return None;
    }

    let sigma_min = (sigma_m / MAX_SIGMA_RATIO).max(1e-6_f64);
    let sigma_max = sigma_m * MAX_SIGMA_RATIO;
    let sigma_step = (sigma_max - sigma_min) / f64::from(N_SIGMA_SAMPLES);

    let mean_dir = if mu_b >= mu_m { 1.0_f64 } else { -1.0_f64 };
    let max_shift = MAX_MEAN_SEP_SIGMAS * sigma_m;

    let mut best: Option<(f64, f64, f64)> = None;

    for i in 0..=N_SIGMA_SAMPLES {
        let cand_sigma = f64::from(i).mul_add(sigma_step, sigma_min);
        for j in 0..=N_MEAN_SAMPLES {
            let shift = (f64::from(j) / f64::from(N_MEAN_SAMPLES)) * max_shift;
            let cand_mu = mean_dir.mul_add(shift, mu_m);
            let coll = lambda_scaled_collateral_at(mu_m, sigma_m, cand_mu, cand_sigma, k);
            if !coll.is_finite() || coll < 0.0 || coll > budget {
                continue;
            }
            let ev = expected_value_at(mu_m, sigma_m, cand_mu, cand_sigma, k, mu_b, sigma_b);
            let net = ev - coll;
            // Require strict net > 0 AND strict coll > 0 — the no-trade
            // point (μ_m, σ_m) has net = 0 and is not a "positive-net
            // trade existed" witness.
            if net > 0.0 && coll > 0.0 {
                let better = match best {
                    None => true,
                    Some((b_net, _, _)) => net > b_net,
                };
                if better {
                    best = Some((net, cand_mu, cand_sigma));
                }
            }
        }
    }

    best
}

/// Re-evaluate a returned `NormalOptimizationResult` in the chain
/// frame.
///
/// Post-v0.1.3 the optimizer's struct reports λ-scaled values directly,
/// but we re-derive the chain-frame cost independently anyway so the
/// proptest assertion does not trust the SUT to report its own units
/// correctly.
fn lambda_scaled_net_from_result(
    result: deadeye_optimizer::NormalOptimizationResult,
    mu_m: f64,
    sigma_m: f64,
    k: f64,
) -> f64 {
    let coll = lambda_scaled_collateral_at(
        mu_m,
        sigma_m,
        result.optimized_mean,
        result.optimized_sigma,
        k,
    );
    if !coll.is_finite() {
        return f64::NEG_INFINITY;
    }
    result.expected_value - coll
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 5_000,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// The property assertion (chain-frame):
    ///
    /// **"Given any (belief, market, budget), if a positive-net trade
    /// in CHAIN UNITS (λ-scaled EV − λ-scaled cost, λ-scaled cost ≤
    /// budget) exists at any grid point, the optimizer must return one."**
    ///
    /// Two-sided:
    /// * ground truth witnesses λ-scaled `net > 0` ⇒ optimizer's
    ///   returned trade, re-evaluated in chain units, must also have
    ///   `net > 0` AND respect the budget on λ-scaled cost.
    /// * ground truth witnesses nothing ⇒ optimizer returns no-trade.
    #[test]
    fn optimizer_returns_a_trade_when_ground_truth_says_one_exists(
        mu_b in -100.0_f64..100.0,
        var_b in 0.01_f64..1000.0,
        mu_m in -100.0_f64..100.0,
        var_m in 0.01_f64..1000.0,
        budget in 0.1_f64..10_000.0,
        k in 1.0_f64..1000.0,
    ) {
        let sigma_b = var_b.sqrt();
        let sigma_m = var_m.sqrt();
        let ground = grid_scan_ground_truth(mu_b, sigma_b, mu_m, sigma_m, budget, k);
        let input = NormalOptimizationInput {
            budget,
            belief_mean: mu_b,
            belief_sigma: sigma_b,
            market_mean: mu_m,
            market_sigma: sigma_m,
            effective_k: k,
            constraints: OptimizerConstraints::default(),
        };
        let result = optimize_normal_trade(input);

        if let Some((gt_best_net, _, _)) = ground {
            if gt_best_net > 0.0 {
                // Optimizer's returned trade, re-evaluated in chain frame.
                let chain_coll = lambda_scaled_collateral_at(
                    mu_m, sigma_m, result.optimized_mean, result.optimized_sigma, k,
                );
                let chain_net = lambda_scaled_net_from_result(result, mu_m, sigma_m, k);
                prop_assert!(
                    chain_net > 0.0,
                    "ground truth says λ-scaled positive-net trade exists at net={gt_best_net:.6}, \
                     but optimizer returned chain_net={chain_net:.6} (λ-scaled coll={chain_coll:.6}) \
                     (μ_b={mu_b}, σ_b={sigma_b}, μ_m={mu_m}, σ_m={sigma_m}, k={k}, budget={budget})"
                );
                prop_assert!(
                    chain_coll <= budget + 1e-9_f64,
                    "optimizer returned a trade whose λ-scaled cost {chain_coll:.6} exceeds \
                     budget={budget} \
                     (μ_b={mu_b}, σ_b={sigma_b}, μ_m={mu_m}, σ_m={sigma_m}, k={k})"
                );
            }
        } else {
            // Ground truth (chain frame) says no positive trade exists;
            // optimizer must return the no-trade sentinel. Post-v0.1.3
            // the optimizer reports λ-scaled `collateral_required` so
            // `coll == 0` is the unambiguous no-trade signal.
            prop_assert!(
                result.collateral_required.abs() < 1e-12_f64,
                "ground truth says no λ-scaled positive-net trade, \
                 but optimizer returned coll={} \
                 (μ_b={mu_b}, σ_b={sigma_b}, μ_m={mu_m}, σ_m={sigma_m}, k={k}, budget={budget})",
                result.collateral_required
            );
        }
    }
}

// ── Targeted regression anchors ─────────────────────────────────────────
//
// Explicit cases for the historical failures we know about. Cheap unit
// tests that pin the bug in plain sight rather than relying on the
// random proptest to rediscover them.

/// σ-only arb regression anchors — chain-frame.
///
/// At chain pricing (`λ ≈ k · √(2σ√π)`, ~80–115× at typical k/σ),
/// the CPI-style cases that the **pre-v0.1.3** optimizer reported as
/// profitable are in fact **negative-net** under the chain's
/// λ-scaling. The Newton solver still converges (the v0.1.1 fix), but
/// the chain charge dominates the EV.
///
/// The chain-frame contract:
/// * `r.collateral_required` is the λ-scaled charge the chain levies.
/// * `r.expected_value` is the λ-scaled belief integral.
/// * The optimizer returns the no-trade sentinel iff no grid point has
///   `λ-scaled EV > λ-scaled cost` within budget.
///
/// We pin each historical scenario at its **chain-correct** outcome.
#[test]
fn sigma_only_arb_chain_frame_outcomes() {
    let cases = [
        // (label, mu_b, sigma_b, mu_m, sigma_m, budget, k, expect_trade)
        // "Modest" tight beliefs at k≈50–75: chain charge dominates EV.
        (
            "live-CPI-2026-05-14",
            4.3274,
            0.2143,
            4.2900,
            0.3500,
            50.0,
            75.07,
            false,
        ),
        (
            "pure σ-arb equal-μ",
            4.2900,
            0.2143,
            4.2900,
            0.3500,
            50.0,
            75.07,
            false,
        ),
        (
            "σ-widening",
            4.2900,
            0.7000,
            4.2900,
            0.3500,
            50.0,
            50.00,
            false,
        ),
        // Aggressively tight belief (σ_b=0.05 ≪ σ_m=0.35) at k=50: λ_g
        // shoots up enough that the GPI peak in the EV dominates the
        // chain charge — this IS chain-profitable.
        (
            "σ-tightening 7x",
            4.2900,
            0.0500,
            4.2900,
            0.3500,
            50.0,
            50.00,
            true,
        ),
        // Low-k cases where σ-arb IS chain-profitable.
        ("low-k σ-shrink", 0.0, 0.5, 0.0, 4.0, 100.0, 1.0, true),
    ];
    for (label, mu_b, sigma_b, mu_m, sigma_m, budget, k, expect_trade) in cases {
        let r = optimize_normal_trade(NormalOptimizationInput::new(
            budget, mu_b, sigma_b, mu_m, sigma_m, k,
        ));
        if expect_trade {
            let net = r.expected_value - r.collateral_required;
            assert!(
                net > 0.0,
                "{label}: expected chain-profitable trade, got net={net:.6}"
            );
            assert!(
                r.collateral_required <= budget + 1e-9,
                "{label}: budget filter leak"
            );
        } else {
            assert!(
                r.collateral_required.abs() < 1e-12_f64,
                "{label}: expected chain no-trade (cost > EV under λ-scaling), got coll={}",
                r.collateral_required,
            );
        }
    }
}
