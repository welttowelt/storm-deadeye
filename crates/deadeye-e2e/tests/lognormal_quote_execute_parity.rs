#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_precision_loss,
    reason = "integration tests in tests/ are top-level — printing aids debugging, unwrap/panic/expect are fine"
)]

//! Quote/execute acceptance parity for lognormal markets (issue #30).
//!
//! The contract under test:
//!
//! > Given a belief + budget for which the **off-chain** lognormal optimizer
//! > (`optimize_quote_offline_ev`, the engine behind
//! > `trade quote --belief/--budget`) returns a positive-EV candidate, the
//! > **execute** pipeline — f64 `lognormal_collateral` draft, then
//! > `refine_lognormal_quote` (the chain probe) — must certify an `x*` the
//! > on-chain verifier accepts, and `execute_trade` with the certified
//! > `(candidate, x*, hints, collateral)` must land.
//!
//! Regression context: v0.1.15's `execute_lognormal` fed `check_trade_view`
//! the candidate's μ as `x*`, which fails the side/stationarity verification
//! for **every** candidate — all lognormal trades were rejected as
//! `SideInvalid`/`StationaryInvalid` (issues #30/#31) even though `quote`
//! certified them. This test covers both directions (μ down-move and
//! μ up-move), mirroring the Gherkin in #30.
//!
//! Gated behind `DEADEYE_RUN_INTEGRATION=1` and requires `starknet-devnet`
//! on `:5050`.

use deadeye_collateral::{LognormalOptions, lognormal_collateral};
use deadeye_core::{
    Distribution, LognormalDistribution, Sq128, distribution::LognormalDistributionRaw,
};
use deadeye_sdk::{DeadeyeClient, starknet::JsonRpcProvider};
use deadeye_starknet::{
    Felt, LognormalMarketReader, LognormalMarketWriter, types::lognormal::LognormalTradeInput,
};
use deadeye_testkit::fixture::{
    bootstrap_devnet,
    env::BootstrapConfig,
    erc20::approve,
    lifecycle::{
        deploy_lognormal_market_with_event, fetch_lognormal_hints, initialize_market,
        upsert_lognormal_profile_for_test,
    },
};
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};

const PROFILE_ID: u32 = 7;
const HUGE_APPROVE: u128 = 10_000_000_000_000_000_000_000_u128;
const INITIAL_BACKING_BASE: u128 = 10_000_000_000_000_000_000_000_u128;

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

#[tokio::test]
async fn lognormal_quote_execute_parity_both_directions() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 (+ devnet on :5050) to enable");
        return;
    }

    // ── Bootstrap: devnet, profile, market, initial LP ──────────────────
    let env = bootstrap_devnet(BootstrapConfig {
        participant_count: 1,
        ..BootstrapConfig::default()
    })
    .await
    .expect("bootstrap_devnet");
    let admin_handle = env.account_handle(&env.admin);
    upsert_lognormal_profile_for_test(
        admin_handle.clone(),
        env.factory,
        env.collateral,
        PROFILE_ID,
    )
    .await
    .expect("upsert lognormal profile");

    // Perfect-square variance so σ×σ == variance at Sq128 precision.
    let initial_mu = 80_000_f64.ln();
    let initial_variance = 0.25_f64;
    let initial_raw = LognormalDistributionRaw {
        mu: Sq128::from_f64(initial_mu).unwrap().to_raw(),
        variance: Sq128::from_f64(initial_variance).unwrap().to_raw(),
        sigma: Sq128::from_f64(initial_variance.sqrt()).unwrap().to_raw(),
    };
    let hints_rpc = JsonRpcClient::new(HttpTransport::new(env.url.clone()));
    let runtime = env.lognormal_runtime;
    assert!(
        runtime != Felt::ZERO,
        "lognormal_runtime not deployed by bootstrap"
    );
    let initial_hints = fetch_lognormal_hints(&hints_rpc, runtime, initial_raw)
        .await
        .expect("fetch initial hints");
    let market = deploy_lognormal_market_with_event(
        &admin_handle,
        env.factory,
        PROFILE_ID,
        Felt::from(1_u64),
        Felt::ZERO,
        initial_raw,
        initial_hints,
    )
    .await
    .expect("deploy lognormal market");
    assert!(market != Felt::ZERO, "deploy returned zero address");
    initialize_market(&admin_handle, market, env.collateral, INITIAL_BACKING_BASE)
        .await
        .expect("initialize_market");
    eprintln!("✅ lognormal market {market:#x} live");

    let trader = &env.participants[0];
    let trader_handle = env.account_handle(trader);
    approve(trader_handle.clone(), env.collateral, market, HUGE_APPROVE)
        .await
        .expect("approve collateral");

    // ── Both directions: belief below, then above, the live market μ ────
    for (round, delta_mu) in [(1_u32, -0.30_f64), (2, 0.30)] {
        let sdk_rpc = JsonRpcClient::new(HttpTransport::new(env.url.clone()));
        let sdk_provider = JsonRpcProvider::new(sdk_rpc);
        let client = DeadeyeClient::new(sdk_provider);
        let handle = client.lognormal_market(market);

        // Live state (the market moves after round 1's trade).
        let current = handle.distribution().await.expect("read distribution");
        let market_mu = current.mu().to_f64();
        let market_sigma = Distribution::sigma(&current).to_f64();
        let belief_mu = market_mu + delta_mu;
        let budget = 500.0_f64;

        // 1. QUOTE: the same optimizer `trade quote --belief/--budget` runs.
        let result = handle
            .optimize_quote_offline_ev(belief_mu, market_sigma, budget)
            .await
            .expect("optimize_quote_offline_ev");
        assert!(
            result.collateral_required > 0.0,
            "[round {round}] optimizer returned a no-trade for Δμ={delta_mu} — fixture too tight"
        );
        eprintln!(
            "[round {round}] optimizer: μ_log {market_mu:.5} → {:.5}, collateral {:.4}, EV {:+.4}",
            result.optimized_mu, result.collateral_required, result.expected_value
        );

        // 2. EXECUTE pipeline: f64 draft → chain probe (exactly what the CLI `trade
        //    execute` runs). Parity = the probe certifies the optimizer's candidate.
        let candidate_dist = LognormalDistribution::from_variance(
            Sq128::from_f64(result.optimized_mu).unwrap(),
            Sq128::from_f64(result.optimized_variance).unwrap(),
        )
        .expect("candidate distribution");
        let candidate_raw = LognormalDistributionRaw {
            mu: Sq128::from_f64(result.optimized_mu).unwrap().to_raw(),
            variance: Sq128::from_f64(result.optimized_variance).unwrap().to_raw(),
            sigma: Sq128::from_f64(result.optimized_variance.sqrt())
                .unwrap()
                .to_raw(),
        };
        let solved = lognormal_collateral(&current, &candidate_dist, LognormalOptions::default())
            .expect("f64 lognormal solver");
        let supplied = Sq128::from_f64((solved.collateral * 3.0).max(50.0))
            .unwrap()
            .to_raw();
        let draft = deadeye_starknet::LognormalTradeQuote {
            candidate: candidate_raw,
            candidate_hints: deadeye_starknet::types::lognormal::LognormalSqrtHintsRaw {
                l2_norm_denom: Sq128::ZERO.to_raw(),
                backing_denom: Sq128::ZERO.to_raw(),
            },
            x_star: Sq128::from_f64(solved.x_star).unwrap().to_raw(),
            required_collateral: Sq128::from_f64(solved.collateral).unwrap().to_raw(),
            padded_collateral: supplied,
            on_chain_will_accept: true,
            rejection: None,
        };

        let writer_rpc = JsonRpcClient::new(HttpTransport::new(env.url.clone()));
        let writer_provider = JsonRpcProvider::new(writer_rpc);
        let writer = LognormalMarketWriter::new(
            LognormalMarketReader::new(&writer_provider, market),
            env.owned_account(trader),
        );
        let outcome = deadeye_starknet::chain_probe::refine_lognormal_quote(
            writer.account(),
            writer.reader(),
            &draft,
        )
        .await
        .expect("chain probe ran")
        .unwrap_or_else(|| {
            panic!(
                "[round {round}] PARITY VIOLATION: quote certified the candidate but the \
                 chain probe could not certify an x* (issue #30 regression)"
            )
        });
        let chain_required = Sq128::from_raw(outcome.computed_collateral).to_f64();
        eprintln!(
            "[round {round}] probe: certified x* (offset {:+.3e}, {} round(s)), collateral {chain_required:.4}",
            outcome.offset, outcome.rounds
        );

        // 3. Real execute with the certified bundle — must land. Mirror the CLI's
        //    sizing: gross up the NET requirement (floored at the AMM's minimum-trade
        //    collateral) by the measured fee rate.
        let params = LognormalMarketReader::new(&writer_provider, market)
            .params()
            .await
            .expect("read params");
        let min_trade = Sq128::from_raw(params.min_trade_collateral).to_f64();
        let gross = (chain_required.max(min_trade) / outcome.net_rate) * 1.002;
        let input = LognormalTradeInput {
            candidate: candidate_raw,
            x_star: outcome.x_star,
            supplied_collateral: Sq128::from_f64(gross).unwrap().to_raw(),
            candidate_hints: outcome.candidate_hints,
        };
        let receipt = writer.execute_trade(input).await.unwrap_or_else(|e| {
            panic!(
                "[round {round}] PARITY VIOLATION: probe-certified trade reverted on \
                 execute_trade: {e}"
            )
        });
        eprintln!(
            "[round {round}] ✅ executed, tx={:#x}",
            receipt.transaction_hash
        );

        // The market must actually have moved toward the candidate.
        let after = handle.distribution().await.expect("read distribution");
        let after_mu = after.mu().to_f64();
        assert!(
            (after_mu - result.optimized_mu).abs() < 1e-6,
            "[round {round}] market μ_log {after_mu} != executed candidate μ_log {}",
            result.optimized_mu
        );
    }
    eprintln!("✅ quote/execute parity holds in both directions");
}
