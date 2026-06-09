#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    reason = "integration tests in tests/ are top-level — printing aids debugging, unwrap is OK"
)]

//! Phase 3 e2e: lognormal read paths + solver against live mainnet.

use deadeye_core::Distribution;
use deadeye_indexer::IndexerClient;
use deadeye_sdk::{DeadeyeClient, starknet::JsonRpcProvider};
use deadeye_testkit::default_mainnet_rpc;
use starknet_core::types::Felt;
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

async fn pick_lognormal_market() -> Option<Felt> {
    let client = IndexerClient::mainnet().ok()?;
    let markets = client.markets().await.ok()?;
    for m in markets {
        if m.market_type != "lognormal" {
            continue;
        }
        let Some(state) = m.state else { continue };
        if state.is_initialised && !state.is_settled {
            return Felt::from_hex(&m.address).ok();
        }
    }
    None
}

#[tokio::test]
async fn mainnet_lognormal_read_passthrough() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 to enable");
        return;
    }
    let Some(address) = pick_lognormal_market().await else {
        eprintln!("skip: no live lognormal market on mainnet");
        return;
    };
    eprintln!("picked lognormal market: {address:#x}");

    let url = default_mainnet_rpc();
    let rpc = JsonRpcClient::new(HttpTransport::new(url));
    let provider = JsonRpcProvider::new(rpc);
    let client = DeadeyeClient::new(provider);
    let market = client.lognormal_market(address);

    let dist = market.distribution().await.expect("distribution reads");
    eprintln!(
        "lognormal: mu={}, variance={}",
        dist.mu().to_f64(),
        dist.variance().to_f64()
    );
    assert!(dist.variance().to_f64() > 0.0);
}
