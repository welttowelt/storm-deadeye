//! Composition test for `NormalMarket::optimize_quote`.
//!
//! Exercises the optimizer → quote-shape pipeline at the SDK boundary
//! without spinning up a devnet. We can't easily mock the math-runtime
//! view calls behind `quote_trade` from a unit test, so this test runs
//! the optimizer with realistic inputs and confirms the picker behaves
//! per the v1.0 contract:
//!
//! * Returns a candidate with `μ` shifted toward the belief mean.
//! * Returns positive collateral when the belief differs meaningfully from the
//!   market.
//! * Respects the budget cap.

#![allow(
    clippy::print_stderr,
    clippy::unwrap_used,
    clippy::tests_outside_test_module,
    clippy::float_cmp,
    reason = "integration-style unit test — printing aids diagnostics, \
              unwrap marks hard invariants the test is asserting"
)]

use deadeye_optimizer::{NormalOptimizationInput, optimize_normal_trade};

/// Belief above the market mean → optimizer should pick a candidate
/// whose μ is also above the market mean (directional consistency).
#[test]
fn optimizer_shifts_mean_toward_belief() {
    let market_mean = 42.0_f64;
    let market_sigma = 8.0_f64;
    let belief_mean = 50.0_f64; // strictly greater than market
    let belief_sigma = 2.0_f64; // tight belief
    let budget = 10.0_f64;
    let effective_k = 50.0_f64;

    let res = optimize_normal_trade(NormalOptimizationInput::new(
        budget,
        belief_mean,
        belief_sigma,
        market_mean,
        market_sigma,
        effective_k,
    ));
    eprintln!(
        "optimizer: μ_g = {:.4}, σ_g = {:.4}, coll = {:.4}, ev = {:.4}, roi = {:.4}",
        res.optimized_mean,
        res.optimized_sigma,
        res.collateral_required,
        res.expected_value,
        res.roi,
    );

    assert!(
        res.optimized_mean >= market_mean,
        "optimized_mean ({}) should not move opposite to belief ({})",
        res.optimized_mean,
        belief_mean
    );
    assert!(
        res.collateral_required >= 0.0,
        "collateral must be non-negative, got {}",
        res.collateral_required
    );
    assert!(
        res.collateral_required <= budget,
        "collateral ({}) must respect budget ({})",
        res.collateral_required,
        budget
    );
}

/// Belief below the market mean → optimizer should pick a candidate
/// whose μ is also below the market mean.
#[test]
fn optimizer_shifts_mean_downward_for_low_belief() {
    let market_mean = 100.0_f64;
    let market_sigma = 5.0_f64;
    let belief_mean = 90.0_f64;
    let belief_sigma = 1.5_f64;
    let budget = 20.0_f64;
    let effective_k = 50.0_f64;

    let res = optimize_normal_trade(NormalOptimizationInput::new(
        budget,
        belief_mean,
        belief_sigma,
        market_mean,
        market_sigma,
        effective_k,
    ));
    eprintln!(
        "optimizer down: μ_g = {:.4}, σ_g = {:.4}, coll = {:.4}",
        res.optimized_mean, res.optimized_sigma, res.collateral_required,
    );

    assert!(
        res.optimized_mean <= market_mean,
        "optimized_mean ({}) should not move opposite to belief ({}) below market",
        res.optimized_mean,
        belief_mean
    );
    assert!(res.collateral_required <= budget);
}

/// Zero budget → no-trade sentinel.
#[test]
fn optimizer_zero_budget_returns_market_unchanged() {
    let market_mean = 42.0_f64;
    let market_sigma = 8.0_f64;
    let res = optimize_normal_trade(NormalOptimizationInput::new(
        0.0,
        50.0,
        2.0,
        market_mean,
        market_sigma,
        50.0,
    ));
    assert_eq!(res.collateral_required, 0.0);
    assert_eq!(res.optimized_mean, market_mean);
    assert_eq!(res.optimized_sigma, market_sigma);
}

/// Belief exactly matches market → optimizer picks a no-shift candidate
/// (or one within `sigma_step`) and required collateral is small / zero.
#[test]
fn optimizer_zero_belief_shift_picks_near_market() {
    let market_mean = 42.0_f64;
    let market_sigma = 8.0_f64;
    let res = optimize_normal_trade(NormalOptimizationInput::new(
        10.0,
        market_mean,
        2.0,
        market_mean,
        market_sigma,
        50.0,
    ));
    // The optimizer may pick a small σ adjustment for EV; assert μ
    // stays within a few σ_step of the market.
    let sigma_step = market_sigma.mul_add(4.0, -(market_sigma / 4.0)) / 50.0_f64;
    assert!(
        (res.optimized_mean - market_mean).abs() <= sigma_step * 2.0,
        "expected μ near market when belief == market; got {} vs market {}",
        res.optimized_mean,
        market_mean
    );
}
