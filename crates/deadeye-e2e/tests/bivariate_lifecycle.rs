#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    reason = "integration tests in tests/ are top-level — printing aids debugging, unwrap is OK"
)]

//! Phase 3 e2e: bivariate read paths against live Sepolia.

use deadeye_indexer::IndexerClient;
use deadeye_sdk::{DeadeyeClient, starknet::JsonRpcProvider};
use deadeye_testkit::cartridge::CartridgeNetwork;
use starknet_core::types::Felt;
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

async fn pick_bivariate_market() -> Option<Felt> {
    let client = IndexerClient::sepolia().ok()?;
    let markets = client.markets().await.ok()?;
    for m in markets {
        if m.market_type != "bivariate" {
            continue;
        }
        return Felt::from_hex(&m.address).ok();
    }
    None
}

#[tokio::test]
async fn sepolia_bivariate_read_passthrough() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 to enable");
        return;
    }
    let Some(address) = pick_bivariate_market().await else {
        eprintln!("skip: no live bivariate market on Sepolia");
        return;
    };
    eprintln!("picked bivariate market: {address:#x}");

    let url = CartridgeNetwork::Sepolia.url();
    let rpc = JsonRpcClient::new(HttpTransport::new(url));
    let provider = JsonRpcProvider::new(rpc);
    let client = DeadeyeClient::new(provider);
    let market = client.bivariate_market(address);

    let dist = market.distribution().await.expect("distribution reads");
    eprintln!(
        "bivariate: mu=({}, {}) sigma=({}, {}) rho={}",
        dist.mu1(),
        dist.mu2(),
        dist.sigma1(),
        dist.sigma2(),
        dist.rho()
    );
    assert!(dist.sigma1() > 0.0 && dist.sigma2() > 0.0);
    assert!(dist.rho().abs() < 1.0);
}
