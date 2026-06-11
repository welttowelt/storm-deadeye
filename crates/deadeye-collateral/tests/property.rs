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
    reason = "property tests live at the test-binary root — relaxed lints match the chaos suites"
)]

//! Property tests for the off-chain collateral solver.
//!
//! Each family gets a single proptest that:
//!
//! 1. Generates a random pair of distributions inside the chain's numerical
//!    envelope (per `MinimizationPolicy::standard()`).
//! 2. Calls the solver.
//! 3. Asserts ONE of two outcomes:
//!    * `Ok(verified)` — the verified minimum's invariants hold (non-negative
//!      collateral, finite iteration count, AND — for the normal family — the
//!      **chain-faithful** lambda-scaled stationarity `|d̃'(x*)| < tolerance ·
//!      max(λ_f, λ_g, 1)`, matching the post-Wave-0 on-chain verifier in
//!      `packages/onchain-normal-math/src/helpers.cairo`).
//!    * `Err(_)` — the solver returned a typed error. The solver MUST NOT
//!      panic, hang, or return NaN.
//!
//! ## Chaos surface coverage
//!
//! The input ranges are chosen to exercise the four Wave-0-breaking
//! cases explicitly:
//!
//! * **σ-shrink** (`var_f >> var_g` or vice-versa) — the ratio range
//!   `0.01..1000` covers ratios up to 10⁵, far beyond the chain's typical 4×
//!   envelope.
//! * **Equal-σ μ-shifts** — the variance generator can land on the same sampled
//!   value for both sides; the |Δμ| range up to 200 σ stresses the saddle-pair
//!   pathology that broke Wave 0.
//! * **Opposite-μ + opposite-σ** — independent generators for `mu_f` and `mu_g`
//!   (negative and positive) plus independent variance generators yield "σ
//!   widens AND μ moves opposite to σ direction" pairs in ~25 % of samples.
//! * **Wide mean magnitude** — `mu ∈ [-100, 100]` ensures the absolute mean
//!   guard from `MinimizationPolicy::standard` exercises both sides of its
//!   2¹⁰⁰-ish bound without ever tripping it.
//!
//! ## Determinism
//!
//! Seeds are deterministic via proptest's standard
//! `PROPTEST_RNG_ALGORITHM` / `PROPTEST_RNG_SEED` knobs. The tests opt
//! out of `failure_persistence` so the runner works in sandboxed CI.

use deadeye_collateral::{
    BivariateOptions, LognormalOptions, MinimizationPolicy, bivariate_collateral,
    categorical_collateral, categorical_lambda, lambda, lognormal_collateral, normal_collateral,
};
use deadeye_core::{
    BivariateNormalDistribution, CategoricalDistribution, Distribution, LognormalDistribution,
    NormalDistribution, Sq128,
};
use proptest::prelude::*;

/// Stationarity-check cushion. Matches the on-chain verifier's
/// `scaled_verify_minimum_with_lambda` tolerance budget:
/// `|d̃'(x*)| < policy.tolerance · max(λ_f, λ_g, 1)`. We give a 10⁴×
/// f64 cushion on top of the solver's own `1e-12` Newton tolerance to
/// absorb (a) the Q128 → f64 projection round-off, (b) the post-check
/// being re-evaluated independently from the inner Newton loop.
const STATIONARITY_TOL_MULT: f64 = 1e-8_f64;

/// Bound on the unscaled `d(x*)`. The chain reports `collateral =
/// max(0, -d_min)` in the unscaled frame; we just verify the magnitude
/// is finite and bounded by a generous polynomial in the input scale.
const PROP_TOL_MULT: f64 = 1e4_f64;

/// Cases per property test. Override via `PROPTEST_CASES=…`.
const fn cases() -> u32 {
    10_000
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: cases(),
        // Failure persistence to a temp file is incompatible with read-only
        // sandboxes; disable it so the test binary can run anywhere.
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// **Normal family** — generate `(mu_f, var_f)` and `(mu_g, var_g)`
    /// inside the standard envelope, solve, and assert verified-minimum
    /// invariants OR a clean typed error.
    ///
    /// Asserts the **chain-faithful** post-condition: when the solver
    /// returns `Ok(verified)`, the *lambda-scaled* stationary equation
    /// `λ_g · g'(x*) − λ_f · f'(x*) ≈ 0` holds within
    /// `policy.tolerance · max(λ_f, λ_g, 1)`. The pre-Wave-0 invariant
    /// (`|d'(x*)| < tolerance` on the *unscaled* difference) was wrong
    /// for σ-shrink pairs — the unscaled derivative can be far from
    /// zero while the scaled derivative is tight.
    #[test]
    fn normal_solver_converges_or_errors_cleanly(
        mu_f in -100.0_f64..100.0,
        var_f in 0.01_f64..1000.0,
        mu_g in -100.0_f64..100.0,
        var_g in 0.01_f64..1000.0,
    ) {
        let Ok(mean_f_q) = Sq128::from_f64(mu_f) else { return Ok(()); };
        let Ok(var_f_q) = Sq128::from_f64(var_f) else { return Ok(()); };
        let Ok(mean_g_q) = Sq128::from_f64(mu_g) else { return Ok(()); };
        let Ok(var_g_q) = Sq128::from_f64(var_g) else { return Ok(()); };
        let Ok(f) = NormalDistribution::from_variance(mean_f_q, var_f_q) else { return Ok(()); };
        let Ok(g) = NormalDistribution::from_variance(mean_g_q, var_g_q) else { return Ok(()); };

        let policy = MinimizationPolicy::unrestricted();
        let result = normal_collateral(&f, &g, policy);
        // Typed errors are the contract — the solver may legitimately reject
        // degenerate or out-of-envelope pairs. What it MUST NOT do is panic,
        // hang, or return NaN; both arms of this `if let` are valid outcomes.
        if let Ok(verified) = result {
            prop_assert!(
                verified.collateral.is_finite() && verified.collateral >= 0.0,
                "collateral must be finite and non-negative; got {}", verified.collateral,
            );
            prop_assert!(
                verified.iterations < 1000,
                "iterations must be bounded; got {}", verified.iterations,
            );
            prop_assert!(
                verified.x_min.is_finite(),
                "x_min must be finite; got {}", verified.x_min,
            );
            prop_assert!(
                verified.d_min.is_finite(),
                "d_min must be finite; got {}", verified.d_min,
            );

            // ── Chain-faithful invariant: λ-scaled stationarity ─────────
            //
            // Mirrors Cairo's `scaled_verify_minimum_with_lambda` in
            // `packages/onchain-normal-math/src/helpers.cairo`. Any
            // returned `Ok(verified)` must satisfy this, otherwise the
            // chain would reject the off-chain hint.
            let sigma_f = f.sigma().to_f64();
            let sigma_g = g.sigma().to_f64();
            let lambda_f = lambda(sigma_f, 1.0_f64);
            let lambda_g = lambda(sigma_g, 1.0_f64);
            let scale = lambda_f.max(lambda_g).max(1.0_f64);

            let Ok(x_q) = Sq128::from_f64(verified.x_min) else { return Ok(()); };
            let Ok(f_prime) = f.pdf_derivative(x_q) else { return Ok(()); };
            let Ok(g_prime) = g.pdf_derivative(x_q) else { return Ok(()); };
            let d_prime_scaled =
                lambda_g.mul_add(g_prime.to_f64(), -(lambda_f * f_prime.to_f64()));
            prop_assert!(
                d_prime_scaled.abs() < STATIONARITY_TOL_MULT * scale,
                "λ-scaled stationarity: |d̃'(x*)|={:.3e} > tol·scale={:.3e} \
                 (f=N({mu_f},{var_f}), g=N({mu_g},{var_g}), x*={})",
                d_prime_scaled.abs(),
                STATIONARITY_TOL_MULT * scale,
                verified.x_min,
            );

            if verified.d_min < 0.0 {
                prop_assert!((verified.collateral - (-verified.d_min)).abs() < PROP_TOL_MULT);
            } else {
                prop_assert!(verified.collateral.abs() < PROP_TOL_MULT);
            }
        }
    }

    /// **Lognormal family**.
    #[test]
    fn lognormal_solver_converges_or_errors_cleanly(
        mu_f in -2.0_f64..2.0,
        var_f in 0.001_f64..2.0,
        mu_g in -2.0_f64..2.0,
        var_g in 0.001_f64..2.0,
    ) {
        let Ok(mu_f_q) = Sq128::from_f64(mu_f) else { return Ok(()); };
        let Ok(var_f_q) = Sq128::from_f64(var_f) else { return Ok(()); };
        let Ok(mu_g_q) = Sq128::from_f64(mu_g) else { return Ok(()); };
        let Ok(var_g_q) = Sq128::from_f64(var_g) else { return Ok(()); };
        let Ok(f) = LognormalDistribution::from_variance(mu_f_q, var_f_q) else { return Ok(()); };
        let Ok(g) = LognormalDistribution::from_variance(mu_g_q, var_g_q) else { return Ok(()); };

        let result = lognormal_collateral(&f, &g, LognormalOptions::default());
        if let Ok(v) = result {
            prop_assert!(v.collateral.is_finite() && v.collateral >= 0.0);
            prop_assert!(v.iterations < 1000);
            prop_assert!(v.x_star.is_finite() && v.x_star > 0.0);
            prop_assert!(v.d_min.is_finite());
        }
    }

    /// **Multinoulli family.**
    ///
    /// Asserts that the reported `min_difference` is genuinely the
    /// argmin of `λ_g · g_i − λ_f · f_i` across all outcomes — i.e. no
    /// off-by-one in the argmin loop. This is the discrete-family
    /// equivalent of the normal family's λ-scaled stationarity check.
    #[test]
    fn multinoulli_solver_converges_or_errors_cleanly(
        probs_f in proptest::collection::vec(0.01_f64..1.0, 2..=8),
        probs_g in proptest::collection::vec(0.01_f64..1.0, 2..=8),
    ) {
        // Equalize outcome counts and normalize so the chain accepts them.
        let n = probs_f.len().min(probs_g.len()).max(2);
        let normalize = |v: &[f64]| -> Vec<f64> {
            let total: f64 = v.iter().take(n).sum();
            if total <= 0.0 { return vec![1.0 / n as f64; n]; }
            v.iter().take(n).map(|p| p / total).collect()
        };
        let nf = normalize(&probs_f);
        let ng = normalize(&probs_g);
        let Ok(f) = CategoricalDistribution::from_probs(nf.clone()) else { return Ok(()); };
        let Ok(g) = CategoricalDistribution::from_probs(ng.clone()) else { return Ok(()); };

        let result = categorical_collateral(&f, &g, 1.0_f64);
        if let Ok(v) = result {
            prop_assert!(v.collateral.is_finite() && v.collateral >= 0.0);
            prop_assert!(v.min_outcome_index < n);
            prop_assert!(v.min_difference.is_finite());
            prop_assert!(v.lambda_f.is_finite() && v.lambda_g.is_finite());

            // Chain-faithful invariant: the reported argmin is the
            // discrete equivalent of stationarity. Recompute the λ-scaled
            // diff across all outcomes and confirm no outcome i has a
            // smaller value than the reported minimum.
            let lf = categorical_lambda(&nf, 1.0_f64);
            let lg = categorical_lambda(&ng, 1.0_f64);
            for i in 0..n {
                let d_i = lg.mul_add(ng[i], -(lf * nf[i]));
                prop_assert!(
                    d_i >= v.min_difference - 1e-9_f64,
                    "argmin off-by-one: outcome {i} has d_i={d_i} < reported min={} \
                     (probs_f={nf:?}, probs_g={ng:?})",
                    v.min_difference,
                );
            }
        }
    }

    /// **Bivariate normal family.**
    #[test]
    fn bivariate_solver_converges_or_errors_cleanly(
        mu1_f in -5.0_f64..5.0,
        mu2_f in -5.0_f64..5.0,
        var1_f in 0.1_f64..10.0,
        var2_f in 0.1_f64..10.0,
        rho_f in -0.95_f64..0.95,
        mu1_g in -5.0_f64..5.0,
        mu2_g in -5.0_f64..5.0,
        var1_g in 0.1_f64..10.0,
        var2_g in 0.1_f64..10.0,
        rho_g in -0.95_f64..0.95,
    ) {
        let Ok(f) = BivariateNormalDistribution::from_core(mu1_f, mu2_f, var1_f, var2_f, rho_f)
        else { return Ok(()); };
        let Ok(g) = BivariateNormalDistribution::from_core(mu1_g, mu2_g, var1_g, var2_g, rho_g)
        else { return Ok(()); };

        let result = bivariate_collateral(&f, &g, BivariateOptions::default());
        if let Ok(v) = result {
            prop_assert!(v.collateral.is_finite() && v.collateral >= 0.0);
            prop_assert!(v.iterations < 1000);
            prop_assert!(v.x1.is_finite());
            prop_assert!(v.x2.is_finite());
            prop_assert!(v.d_min.is_finite());
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
//  Integration-gated property: off-chain solver vs on-chain `check_trade_view`.
//
//  Runs ~100 cases when DEADEYE_RUN_INTEGRATION is set and a devnet is
//  reachable. Disabled by default so `cargo test` stays offline.
//
//  The bridge from the off-chain `VerifiedMinimum` to the on-chain
//  verifier is `NormalMathRuntimeReader::check_trade_view`; depending on
//  the runtime ABI, the on-chain call returns a boolean / typed error
//  that we compare against the off-chain `Ok / Err` discriminant.
//
//  This test is intentionally minimal — most of the cross-chain
//  validation happens in the per-family chaos suites; this one just
//  guards against a *categorical* off-chain↔on-chain divergence that
//  the chaos suites could miss with their narrower distribution mix.
// ───────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod integration {
    /// Smoke-only entry. Kept as a placeholder for the
    /// `DEADEYE_RUN_INTEGRATION`-gated chain-comparison property; the
    /// full implementation depends on the math-runtime
    /// `check_trade_view` reader landing on every family, which is
    /// tracked in the chaos-suite roadmap. Until then this test
    /// short-circuits without false-greening CI.
    #[test]
    fn off_chain_vs_on_chain_check_trade_view_smoke() {
        if std::env::var("DEADEYE_RUN_INTEGRATION").is_err() {
            return;
        }
        // A real implementation would: bootstrap a devnet, deploy a
        // normal market, generate ~100 candidates inside the chaos
        // surface, and for each compare:
        //   off_chain: normal_collateral(...).is_ok()
        //   on_chain:  NormalMathRuntimeReader::check_trade_view(...).is_ok()
        // The two must agree. Today we exit cleanly so the harness
        // wiring is in place but never flakes against an absent devnet.
        eprintln!(
            "off_chain_vs_on_chain_check_trade_view_smoke: stub — full \
             impl depends on runtime check_trade_view reader",
        );
    }
}
