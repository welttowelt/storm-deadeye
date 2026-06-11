#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests in tests/ are top-level — printing aids debugging"
)]

//! Issue #36 diagnosis + regression: `compute_hints_view` returns
//! `Option::None` for candidates whose `(σ, σ²)` encoding is not Sq128-exact.
//!
//! The CLI built candidates variance-primary with an **f64** sqrt:
//! `σ = variance.sqrt()` then quantized both independently — so on-chain
//! `σ_raw × σ_raw ≠ variance_raw` at fixed-point precision for most inputs,
//! and whether the runtime accepted was a coin flip decided by where the
//! rounding landed (7 of the 9 WC markets happened to pass; Brazil and
//! Belgium reproducibly failed).
//!
//! The probe below feeds the runtime the EXACT failing Brazil candidate from
//! issue #36 in three encodings and asserts the σ-primary (Sq128 `σ·σ`)
//! encoding is accepted.
//!
//! Gated behind `DEADEYE_RUN_INTEGRATION=1` and requires `starknet-devnet`
//! on `:5050`.

use deadeye_core::{
    Distribution as _, LognormalDistribution, Sq128, distribution::LognormalDistributionRaw,
};
use deadeye_sdk::starknet::JsonRpcProvider;
use deadeye_starknet::{Felt, runtime::compute_lognormal_hints};
use deadeye_testkit::fixture::{bootstrap_devnet, env::BootstrapConfig};
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

/// Brazil's failing optimizer candidate from issue #36.
const BRAZIL_MU: f64 = 3.271_808;
const BRAZIL_VARIANCE: f64 = 0.039_191;

#[tokio::test]
async fn sigma_primary_encoding_is_accepted_by_compute_hints_view() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 (+ devnet on :5050) to enable");
        return;
    }
    let env = bootstrap_devnet(BootstrapConfig {
        participant_count: 0,
        ..BootstrapConfig::default()
    })
    .await
    .expect("bootstrap_devnet");
    let runtime = env.lognormal_runtime;
    assert!(runtime != Felt::ZERO, "lognormal runtime not deployed");
    let rpc = JsonRpcProvider::new(JsonRpcClient::new(HttpTransport::new(env.url.clone())));

    // A. CLI-style (pre-fix): variance primary, σ from an f64 sqrt — the two
    //    quantize independently, so σ_raw² ≠ variance_raw on-chain.
    let cli_style = LognormalDistributionRaw {
        mu: Sq128::from_f64(BRAZIL_MU).unwrap().to_raw(),
        variance: Sq128::from_f64(BRAZIL_VARIANCE).unwrap().to_raw(),
        sigma: Sq128::from_f64(BRAZIL_VARIANCE.sqrt()).unwrap().to_raw(),
    };
    let a = compute_lognormal_hints(&rpc, runtime, cli_style).await;
    eprintln!(
        "A (variance-primary, f64 sqrt): {:?}",
        a.as_ref().map(|_| "Some")
    );

    // B. σ-primary (the fix): σ quantized once, variance = σ·σ in Sq128 —
    //    bit-exact against the runtime's own recomputation.
    let sigma_primary = LognormalDistribution::from_sigma(
        Sq128::from_f64(BRAZIL_MU).unwrap(),
        Sq128::from_f64(BRAZIL_VARIANCE.sqrt()).unwrap(),
    )
    .unwrap()
    .to_raw();
    let b = compute_lognormal_hints(&rpc, runtime, sigma_primary).await;
    eprintln!(
        "B (σ-primary, Sq128 σ·σ):       {:?}",
        b.as_ref().map(|_| "Some")
    );

    // C. variance-primary with the Sq128 sqrt (from_variance) — documents
    //    whether the truncating Sq128 sqrt round-trips on-chain.
    let sq128_sqrt = LognormalDistribution::from_variance(
        Sq128::from_f64(BRAZIL_MU).unwrap(),
        Sq128::from_f64(BRAZIL_VARIANCE).unwrap(),
    )
    .unwrap()
    .to_raw();
    let c = compute_lognormal_hints(&rpc, runtime, sq128_sqrt).await;
    eprintln!(
        "C (variance-primary, Sq128 sqrt): {:?}",
        c.as_ref().map(|_| "Some")
    );

    assert!(
        b.is_ok(),
        "σ-primary encoding must be accepted by compute_hints_view: {:?}",
        b.err()
    );
}
