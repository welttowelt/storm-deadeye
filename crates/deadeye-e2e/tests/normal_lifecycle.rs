#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    reason = "integration tests in tests/ are top-level — printing aids debugging, unwrap is OK"
)]

//! Phase 1 e2e: account wiring + Normal market read/write paths.
//!
//! Two test surfaces:
//!
//! 1. **Devnet account smoke** — verifies that `OwnedAccount` constructs
//!    correctly from a starknet-devnet predeployed account and can read
//!    its own nonce. Requires `DEADEYE_RUN_INTEGRATION=1` and a running
//!    devnet at `:5050`.
//!
//! 2. **Sepolia read passthrough** — against the live Sepolia deployment
//!    via Cartridge, opens a known normal market and reads its
//!    distribution + market status. Pure read-only; no funded account
//!    required.
//!
//! The "full deploy → trade → sell → claim" lifecycle test lands in
//! `crates/deadeye-e2e/tests/factory_deploy.rs` once Phase 4 is in,
//! since deploying a market requires the factory wrapper.

use deadeye_core::Distribution;
use deadeye_sdk::{DeadeyeClient, starknet::JsonRpcProvider};
use deadeye_starknet::{Account, OwnedAccount};
use deadeye_testkit::{cartridge::CartridgeNetwork, devnet, predeployed_one};
use starknet_core::types::Felt;
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
use url::Url;

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

#[tokio::test]
async fn devnet_account_smoke() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 to enable");
        return;
    }

    let url = Url::parse(devnet::DEFAULT_URL).expect("devnet URL parses");
    // Probe devnet liveness; bail with a clear message if it's not up.
    if devnet::check_health(&url).await.is_err() {
        eprintln!("skip: devnet at {url} is not reachable");
        return;
    }

    let predeployed = predeployed_one(&url, 0)
        .await
        .expect("first predeployed account");
    eprintln!(
        "devnet account #0: address={:#x} key={:#x}",
        predeployed.address, predeployed.private_key
    );

    // Chain ID for starknet-devnet-rs (SN_SEPOLIA-like by default).
    let chain_id = devnet::chain_id(&url).await.expect("chain_id reads");

    let rpc = JsonRpcClient::new(HttpTransport::new(url.clone()));
    let owned =
        OwnedAccount::from_signing_key(rpc, predeployed.address, predeployed.private_key, chain_id);
    assert_eq!(Account::address(&owned), predeployed.address);
    let nonce = owned.nonce().await.expect("nonce reads");
    eprintln!("devnet account nonce: {nonce:#x}");
}

#[tokio::test]
async fn sepolia_normal_market_read_passthrough() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 to enable");
        return;
    }
    let url = CartridgeNetwork::Sepolia.url();
    let rpc = JsonRpcClient::new(HttpTransport::new(url));
    let provider = JsonRpcProvider::new(rpc);
    let client = DeadeyeClient::new(provider);

    // Known live normal market on Sepolia (resolved via the indexer at
    // dev-time; if this market settles or moves, replace with another
    // `marketType=="normal"` address from /api/markets).
    let market_address =
        Felt::from_hex("0x53e5ee2a3ff003fbcf7f96bba8370b833a06f4a8e23c055b91f1f9076a6fcf4")
            .unwrap();
    let market = client.normal_market(market_address);

    let dist = market.distribution().await.expect("distribution reads");
    eprintln!(
        "sepolia normal market: mean={}, sigma={}",
        dist.mean().to_f64(),
        dist.sigma().to_f64()
    );
    assert!(
        !dist.sigma().is_zero(),
        "live market must have positive sigma"
    );

    let status = market.reader().market_status().await.expect("status reads");
    eprintln!(
        "status: initialised={}, paused={}, settled={}",
        status.is_initialised, status.is_paused, status.is_settled
    );
    assert!(status.is_initialised, "live market must be initialised");
}
