#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    reason = "integration tests in tests/ are top-level — printing aids debugging, unwrap is OK"
)]

//! Phase 4 e2e: factory + oracle read paths against live mainnet.
//!
//! The mainnet factory (per deployment-mainnet.json)
//! is the production fixture. We hit a handful of read endpoints to verify
//! the Cairo Serde shapes match the on-chain types end-to-end.

use deadeye_sdk::{DeadeyeClient, starknet::JsonRpcProvider};
use deadeye_testkit::default_mainnet_rpc;
use starknet_core::types::Felt;
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};

const MAINNET_FACTORY: &str = "0x00a7f815C7921687b3cDe2e58e1006621a4E424a78C76df6134698Ba83eB29f6";

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

#[tokio::test]
async fn mainnet_factory_read_passthrough() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 to enable");
        return;
    }
    let url = default_mainnet_rpc();
    let rpc = JsonRpcClient::new(HttpTransport::new(url));
    let provider = JsonRpcProvider::new(rpc);
    let client = DeadeyeClient::new(provider);

    let factory_address = Felt::from_hex(MAINNET_FACTORY).unwrap();
    let factory = client.factory(factory_address);

    let owner = factory.reader().owner().await.expect("get_owner reads");
    let treasury = factory
        .reader()
        .treasury()
        .await
        .expect("get_treasury reads");
    let count = factory.market_count().await.expect("market count reads");
    eprintln!("factory: owner={owner:#x}, treasury={treasury:#x}, markets={count}");
    assert!(owner != Felt::ZERO);
    assert!(count > 0, "factory must have deployed markets");

    let first = factory.market_at(0).await.expect("market_at(0)");
    eprintln!("first deployed market: {first:#x}");

    // Look up the market-type discriminant — should be ≤ a small number of
    // known kinds (4: normal/lognormal/multinoulli/bivariate).
    let market_type = factory
        .reader()
        .market_type_for_market(first)
        .await
        .expect("market_type_for_market reads");
    eprintln!("market type discriminant for first market: {market_type}");
    assert!(
        market_type < 16,
        "market type discriminant out of expected range"
    );
}
