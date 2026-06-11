#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration tests in tests/ are top-level — printing aids debugging, unwrap/panic are fine"
)]

//! Chain-parity test for [`Sq128::sqrt`] vs the on-chain `sqrt_verified`.
//!
//! Picks 20 arbitrary variances — a mix of perfect Sq128 squares (4, 16, …)
//! and non-perfect-squares (0.04, 0.13, 100.7, …) — computes σ off-chain
//! via [`Sq128::sqrt`], builds a [`NormalDistributionRaw`], and calls
//! `compute_hints_view` on the deployed `normal_math_runtime`. Asserts:
//!
//! * **Every** distribution survives `to_normal`'s `sqrt_verified` check — not
//!   a single `None` return.
//! * The runtime accepts our σ bit-for-bit (i.e. the deployed contract has no
//!   need to recompute σ — its own `sqrt_verified` already accepted ours).
//!
//! Gated behind `DEADEYE_RUN_INTEGRATION=1`; requires `starknet-devnet` on
//! the default port. The bootstrap re-uses the standalone `normal_runtime`
//! the testkit already deploys for chaos suites.

use deadeye_core::{
    Distribution, Sq128,
    distribution::{NormalDistribution, NormalDistributionRaw},
};
use deadeye_testkit::fixture::{
    env::{BootstrapConfig, bootstrap_devnet},
    lifecycle::fetch_normal_hints,
};
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

/// Twenty variances spanning four orders of magnitude. Mix of perfect
/// `Sq128` squares (4, 9, 16, 25, 64, 100, `10_000`) and non-perfect squares
/// (0.04, 0.09, 0.13, 0.5, 100.7, 0.0001, …) — the latter is what
/// f64-mediated σ used to fail on.
fn sample_variances() -> [f64; 20] {
    [
        // Perfect-square integers.
        4.0,
        9.0,
        16.0,
        25.0,
        64.0,
        100.0,
        10_000.0,
        // Non-perfect squares — these are what previously failed.
        0.04,
        0.09,
        0.13,
        0.5,
        1.0,
        100.7,
        0.0001,
        1_234_567.89,
        // Edge cases — small and large.
        0.000_001,
        1e8,
        7.5,
        0.25,
        0.5625,
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sq128_sqrt_parity_against_normal_runtime() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 and start starknet-devnet on :5050");
        return;
    }

    // ─── Bootstrap devnet ────────────────────────────────────────────────
    let cfg = BootstrapConfig::default();
    let env = match bootstrap_devnet(cfg).await {
        Ok(env) => env,
        Err(e) => {
            eprintln!("BOOTSTRAP FAILED: {e:#?}");
            panic!("bootstrap failed: {e}");
        },
    };
    eprintln!(
        "devnet up: factory={:#x}, normal_runtime={:#x}",
        env.factory, env.normal_runtime
    );

    let provider = JsonRpcClient::new(HttpTransport::new(env.url.clone()));

    // ─── Iterate over the 20 variances ──────────────────────────────────
    let variances = sample_variances();
    let total = variances.len();
    let mut passed: usize = 0;
    let mean = Sq128::from_f64(0.0).expect("zero is finite");

    for v in variances {
        let variance = Sq128::from_f64(v).expect("finite f64");
        let dist = NormalDistribution::from_variance(mean, variance)
            .unwrap_or_else(|e| panic!("from_variance failed for v={v}: {e}"));

        // Build the raw DTO the chain expects.
        let raw = NormalDistributionRaw {
            mean: dist.mean().to_raw(),
            variance: dist.variance().to_raw(),
            sigma: dist.sigma().to_raw(),
        };

        match fetch_normal_hints(&provider, env.normal_runtime, raw).await {
            Ok(hints) => {
                eprintln!(
                    "✅ v={v:<14} σ={:<22.18} accepted; \
                     hints={{l2={:e}, back={:e}}}",
                    dist.sigma().to_f64(),
                    Sq128::from_raw(hints.l2_norm_denom).to_f64(),
                    Sq128::from_raw(hints.backing_denom).to_f64(),
                );
                passed += 1;
            },
            Err(e) => {
                eprintln!(
                    "❌ v={v:<14} σ={:.18} REJECTED by compute_hints_view: {e}",
                    dist.sigma().to_f64(),
                );
            },
        }
    }

    eprintln!("sqrt_parity result: {passed}/{total} variances accepted");
    assert_eq!(
        passed, total,
        "Sq128::sqrt must produce a chain-valid σ for every variance — \
         got {passed}/{total} accepted"
    );
}

/// Stress sweep: 100 variances spanning `2^-50` … `2^50`, mixing perfect
/// squares and irrationals. Any chain rejection here is a bug.
fn stress_variances() -> Vec<f64> {
    let mut out: Vec<f64> = Vec::with_capacity(100);
    // 50 powers-of-two and mid-decade values across the range.
    for exp in (-50_i32..=49_i32).step_by(2) {
        let base = 2.0_f64.powi(exp);
        out.push(base);
        // Irrational neighbour: base * π/3 keeps the value in the same decade
        // but guarantees a non-perfect-square mantissa.
        out.push(base * core::f64::consts::FRAC_PI_3);
    }
    debug_assert!(
        out.len() >= 100,
        "expected ≥ 100 variance samples, generated {len}",
        len = out.len(),
    );
    out.truncate(100);
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sq128_sqrt_parity_stress_sweep() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 and start starknet-devnet on :5050");
        return;
    }

    let cfg = BootstrapConfig::default();
    let env = bootstrap_devnet(cfg).await.expect("bootstrap devnet");
    let provider = JsonRpcClient::new(HttpTransport::new(env.url.clone()));
    let mean = Sq128::from_f64(0.0).expect("zero is finite");

    let variances = stress_variances();
    let total = variances.len();
    let mut passed: usize = 0;
    let mut rejections: Vec<(f64, String)> = Vec::new();

    for v in &variances {
        let variance = match Sq128::from_f64(*v) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("skip v={v} (Sq128::from_f64 rejected: {e})");
                continue;
            },
        };
        let dist = match NormalDistribution::from_variance(mean, variance) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("skip v={v} (from_variance rejected: {e})");
                continue;
            },
        };
        let raw = NormalDistributionRaw {
            mean: dist.mean().to_raw(),
            variance: dist.variance().to_raw(),
            sigma: dist.sigma().to_raw(),
        };
        match fetch_normal_hints(&provider, env.normal_runtime, raw).await {
            Ok(_) => passed += 1,
            Err(e) => rejections.push((*v, format!("{e}"))),
        }
    }

    eprintln!("stress sweep: {passed}/{total} variances accepted");
    for (v, why) in &rejections {
        eprintln!("  REJECTED v={v}: {why}");
    }
    assert!(
        rejections.is_empty(),
        "Sq128::sqrt produced a chain-invalid σ for {} variance(s) out of {total}",
        rejections.len()
    );
}
