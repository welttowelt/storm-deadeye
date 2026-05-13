#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "integration test driver — printing aids debugging, unwrap is OK"
)]

//! Wave 2 Item 9: end-to-end exercise of the [`MarketStateStream`].
//!
//! Boots devnet, deploys a normal market, subscribes to its state
//! stream with `include_distribution=true`, then submits 3 trades and
//! asserts the stream yields ≥ 3 updates with the new distributions.
//!
//! Gated on `DEADEYE_RUN_INTEGRATION=1` and a running devnet at
//! `:5050`.

use std::time::Duration;

use deadeye_core::{NormalDistribution, Sq128};
use deadeye_sdk::{
    DeadeyeClient, DistributionSnapshot, Family, MarketStateStream, StarknetBlockSource,
    StreamConfig, starknet::JsonRpcProvider,
};
use deadeye_starknet::{Felt, types::normal::TradeInput};
use deadeye_testkit::{
    devnet,
    fixture::{
        bootstrap_devnet,
        env::BootstrapConfig,
        erc20::approve,
        lifecycle::{
            build_initial_normal_inputs, deploy_normal_market_with_event, fetch_normal_hints,
            initialize_market, upsert_normal_profile_for_test,
        },
    },
};
use futures::StreamExt;
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
use url::Url;

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires running starknet-devnet on :5050; uses DEADEYE_RUN_INTEGRATION env var"]
async fn stream_yields_at_least_one_update_per_trade() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 to enable");
        return;
    }
    let url = Url::parse(devnet::DEFAULT_URL).unwrap();
    if devnet::check_health(&url).await.is_err() {
        eprintln!("skip: devnet at {url} is not reachable");
        return;
    }

    let env = bootstrap_devnet(BootstrapConfig::default())
        .await
        .expect("bootstrap succeeds");
    eprintln!("✅ devnet bootstrapped");

    let admin_handle = env.account_handle(&env.admin);
    let hint_rpc = JsonRpcClient::new(HttpTransport::new(env.url.clone()));

    upsert_normal_profile_for_test(admin_handle.clone(), env.factory, env.collateral, 1)
        .await
        .expect("profile");
    let (initial_dist, _placeholder) = build_initial_normal_inputs(42.0, 64.0, 1000.0);
    let initial_hints = fetch_normal_hints(&hint_rpc, env.normal_runtime, initial_dist)
        .await
        .expect("hints");
    let market = deploy_normal_market_with_event(
        &admin_handle,
        env.factory,
        1,
        Felt::from(0x5C4E_u64),
        Felt::ZERO,
        initial_dist,
        initial_hints,
    )
    .await
    .expect("deploy market");
    initialize_market(
        &admin_handle,
        market,
        env.collateral,
        10_000_000_000_000_000_000_000_u128,
    )
    .await
    .expect("initialize");
    eprintln!("✅ market: {market:#x}");

    let trader = env.participants.first().expect("trader");
    let trader_handle = env.account_handle(trader);
    approve(
        trader_handle.clone(),
        env.collateral,
        market,
        1_000_000_000_000_000_000_000_u128,
    )
    .await
    .expect("approve");

    // Set up the stream.
    let stream_rpc = JsonRpcClient::new(HttpTransport::new(env.url.clone()));
    let stream_provider = JsonRpcProvider::new(stream_rpc);
    let stream_client = DeadeyeClient::new(stream_provider);
    // Block source: a separate JsonRpcClient — gives us a real
    // `starknet_providers::Provider` to poll `block_number()`.
    let block_rpc = JsonRpcClient::new(HttpTransport::new(env.url.clone()));
    let block_source = StarknetBlockSource::new(block_rpc);

    let config = StreamConfig {
        poll_interval: Duration::from_millis(100),
        include_distribution: true,
        include_lp_info: false,
        include_quote_for_candidate: None,
    };
    let mut stream =
        MarketStateStream::subscribe(stream_client, block_source, Family::Normal, market, config);
    eprintln!("✅ stream subscribed");
    // Drain the initial-state tick so the rest of the loop only sees
    // post-trade transitions.
    let _initial = tokio::time::timeout(Duration::from_secs(3), stream.next()).await;

    // Submit 3 trades. The chaos suite uses `execute_trade` directly;
    // we replicate the minimal happy-path: nudge mean each time.
    let writer_rpc = JsonRpcClient::new(HttpTransport::new(env.url.clone()));
    let writer_provider = JsonRpcProvider::new(writer_rpc);
    let writer = deadeye_starknet::NormalMarketWriter::new(
        deadeye_starknet::NormalMarketReader::new(&writer_provider, market),
        env.owned_account(trader),
    );

    // Trade plan: each step nudges μ and σ² so the off-chain solver
    // returns positive collateral. Values mirror the chaos suite's
    // safe envelope.
    let trades: [(f64, f64); 3] = [(43.0, 49.0), (44.0, 36.0), (45.0, 25.0)];
    let mut cur_mean = 42.0_f64;
    let mut cur_variance = 64.0_f64;
    let mut per_trade_observations: usize = 0;
    for (target_mean, target_variance) in trades {
        let cand_dist = deadeye_core::distribution::NormalDistributionRaw {
            mean: Sq128::from_f64(target_mean).unwrap().to_raw(),
            variance: Sq128::from_f64(target_variance).unwrap().to_raw(),
            sigma: Sq128::from_f64(target_variance.sqrt()).unwrap().to_raw(),
        };
        let cand_hints = fetch_normal_hints(&hint_rpc, env.normal_runtime, cand_dist)
            .await
            .expect("hints");
        let f = NormalDistribution::from_variance(
            Sq128::from_f64(cur_mean).unwrap(),
            Sq128::from_f64(cur_variance).unwrap(),
        )
        .unwrap();
        let g = NormalDistribution::from_variance(
            Sq128::from_f64(target_mean).unwrap(),
            Sq128::from_f64(target_variance).unwrap(),
        )
        .unwrap();
        let solver = deadeye_collateral::normal_collateral(
            &f,
            &g,
            deadeye_collateral::MinimizationPolicy::standard(),
        )
        .expect("solver");
        // Use the chaos suite's padding heuristic.
        let supplied = (solver.collateral * 20.0_f64).max(100.0_f64);
        let input = TradeInput {
            candidate: cand_dist,
            x_star: Sq128::from_f64(solver.x_min).unwrap().to_raw(),
            supplied_collateral: Sq128::from_f64(supplied).unwrap().to_raw(),
            candidate_hints: cand_hints,
        };
        let receipt = writer.execute_trade(input).await.expect("trade");
        eprintln!(
            "  ✅ trade → N({target_mean},{target_variance}): {:#x}",
            receipt.transaction_hash
        );
        cur_mean = target_mean;
        cur_variance = target_variance;

        // Wait for the stream to catch this trade's block transition
        // before submitting the next one — keeps each trade in its
        // own block.
        let mut saw_this_trade = false;
        for _ in 0..30_u32 {
            let Some(update) = tokio::time::timeout(Duration::from_millis(500), stream.next())
                .await
                .ok()
                .flatten()
            else {
                continue;
            };
            if let Some(DistributionSnapshot::Normal(d)) = update.distribution {
                let mean = Sq128::from_raw(d.mean).to_f64();
                eprintln!(
                    "  📡 update @block={} mean={:.4}",
                    update.block_number, mean,
                );
                if (mean - target_mean).abs() < 0.5_f64 {
                    saw_this_trade = true;
                    break;
                }
            }
        }
        assert!(
            saw_this_trade,
            "stream did not catch the transition to N({target_mean},{target_variance})",
        );
        per_trade_observations += 1;
    }
    drop(stream);
    eprintln!("✅ stream caught all 3 trade transitions ({per_trade_observations} observed)");
    assert!(
        per_trade_observations >= 3,
        "expected ≥ 3 observed transitions, got {per_trade_observations}",
    );
}
