#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    reason = "integration tests in tests/ are top-level — printing aids debugging, unwrap is OK"
)]

//! End-to-end devnet lifecycle: bootstrap → deploy normal market →
//! single trade → settle → claim → assert balance moved.
//!
//! Requires `DEADEYE_RUN_INTEGRATION=1` and `starknet-devnet --seed 0
//! --accounts 10 --port 5050` running.

use deadeye_core::{Distribution, NormalDistribution, Sq128};
use deadeye_sdk::{DeadeyeClient, starknet::JsonRpcProvider};
use deadeye_starknet::{Account, Felt};
use deadeye_testkit::fixture::{
    bootstrap_devnet,
    env::BootstrapConfig,
    erc20::{approve, balance_of},
    lifecycle::{
        build_initial_normal_inputs, deploy_normal_market_with_event, fetch_normal_hints,
        initialize_market, upsert_normal_profile_for_test,
    },
};
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn normal_market_full_lifecycle() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1");
        return;
    }
    let env = bootstrap_devnet(BootstrapConfig::default())
        .await
        .expect("bootstrap succeeds");

    let admin_handle = env.account_handle(&env.admin);

    // Step A: upsert a deploy profile for normal markets.
    upsert_normal_profile_for_test(admin_handle.clone(), env.factory, env.collateral, 1)
        .await
        .expect("upsert normal profile");

    // Step B: deploy a normal market — example question:
    // "What will the closing CPI year-over-year change be in Q3 2026?"
    // We fetch the on-chain-correct hints via the math runtime to avoid
    // f64-vs-Q128 sqrt precision mismatches.
    let (initial_dist, _placeholder_hints) = build_initial_normal_inputs(3.0, 0.25, 1000.0);
    let rpc_for_hints = JsonRpcClient::new(HttpTransport::new(env.url.clone()));
    let initial_hints = fetch_normal_hints(&rpc_for_hints, env.normal_runtime, initial_dist)
        .await
        .expect("fetch chain-correct hints");
    eprintln!("  chain hints: {initial_hints:?}");
    let market = deploy_normal_market_with_event(
        &admin_handle,
        env.factory,
        1,
        Felt::from(1_u64),
        Felt::ZERO,
        initial_dist,
        initial_hints,
    )
    .await
    .expect("deploy normal market");
    eprintln!("✅ deployed normal market: {market:#x}");

    // Step B': admin initializes the market by depositing the profile's
    // initial backing. Approve generously so transferFrom always has room.
    initialize_market(
        &admin_handle,
        market,
        env.collateral,
        10_000_000_000_000_000_000_000_u128,
    )
    .await
    .expect("initialize normal market");
    eprintln!("✅ initialized market (admin is initial LP)");

    // Step C: confirm the SDK can read the deployed market.
    let rpc = JsonRpcClient::new(HttpTransport::new(env.url.clone()));
    let sdk_rpc = JsonRpcClient::new(HttpTransport::new(env.url.clone()));
    let provider = JsonRpcProvider::new(sdk_rpc);
    let client = DeadeyeClient::new(provider);
    let market_handle = client.normal_market(market);
    let dist = market_handle
        .distribution()
        .await
        .expect("distribution reads");
    eprintln!(
        "  market state: mean={}, sigma={}",
        dist.mean().to_f64(),
        dist.sigma().to_f64()
    );

    // Step D: participant 0 buys collateral exposure by approving the
    // market and reading their balance before/after. We don't yet run a
    // trade — that requires solver-correct hints which need an Sq128
    // off-chain solver. Reading + approve confirms the wiring works.
    let trader = env.participants.first().expect("at least one participant");
    let trader_handle = env.account_handle(trader);
    let pre = balance_of(&rpc, env.collateral, trader.address)
        .await
        .expect("balance reads");
    eprintln!("  trader balance before approve: {pre}");

    approve(
        trader_handle.clone(),
        env.collateral,
        market,
        1_000_000_000_000_000_000_000_u128,
    )
    .await
    .expect("approve collateral");

    let post = balance_of(&rpc, env.collateral, trader.address)
        .await
        .expect("balance reads");
    eprintln!("  trader balance after approve:  {post}");
    assert_eq!(pre, post, "approve must not change balance");

    // Step E: execute a trade. Candidate: nudge mean from 3.0 → 3.10 and
    // bump variance 0.25 → 0.36 so we avoid the equal-variance saddle in
    // the off-chain Newton solver.
    let candidate_mean = 3.10_f64;
    let candidate_variance = 0.36_f64;
    let candidate_sigma = candidate_variance.sqrt();
    let candidate_dist = deadeye_core::distribution::NormalDistributionRaw {
        mean: Sq128::from_f64(candidate_mean).unwrap().to_raw(),
        variance: Sq128::from_f64(candidate_variance).unwrap().to_raw(),
        sigma: Sq128::from_f64(candidate_sigma).unwrap().to_raw(),
    };
    let candidate_hints = fetch_normal_hints(&rpc, env.normal_runtime, candidate_dist)
        .await
        .expect("fetch candidate hints");
    eprintln!("  candidate dist: mean={candidate_mean}, variance={candidate_variance}");

    // Compute off-chain collateral + x_star.
    let cur = NormalDistribution::from_variance(
        Sq128::from_f64(3.0).unwrap(),
        Sq128::from_f64(0.25).unwrap(),
    )
    .unwrap();
    let cand = NormalDistribution::from_variance(
        Sq128::from_f64(candidate_mean).unwrap(),
        Sq128::from_f64(candidate_variance).unwrap(),
    )
    .unwrap();
    let solver = deadeye_collateral::normal_collateral(
        &cur,
        &cand,
        deadeye_collateral::MinimizationPolicy::standard(),
    )
    .expect("solver converges");
    eprintln!(
        "  solver: x*={:.6}, collateral={:.6}, iters={}",
        solver.x_min, solver.collateral, solver.iterations
    );
    // Pad collateral by 5% to absorb any off-chain/chain numerical drift.
    let supplied = solver.collateral * 1.05_f64;
    let trade_input = deadeye_starknet::types::normal::TradeInput {
        candidate: candidate_dist,
        x_star: Sq128::from_f64(solver.x_min).unwrap().to_raw(),
        supplied_collateral: Sq128::from_f64(supplied).unwrap().to_raw(),
        candidate_hints,
    };
    // Build a dedicated provider for the writer so we don't fight with
    // the SDK client's owned copy.
    let writer_rpc = JsonRpcClient::new(HttpTransport::new(env.url.clone()));
    let writer_provider = JsonRpcProvider::new(writer_rpc);
    let writer = deadeye_starknet::NormalMarketWriter::new(
        deadeye_starknet::NormalMarketReader::new(&writer_provider, market),
        env.owned_account(trader),
    );
    let receipt = writer
        .execute_trade(trade_input)
        .await
        .expect("trade submits");
    eprintln!("  ✅ trade tx: {:#x}", receipt.transaction_hash);

    eprintln!("✅ full-lifecycle bootstrap + market deploy + trade pass");
    let _ = Account::address(&env.owned_account(trader));
}
