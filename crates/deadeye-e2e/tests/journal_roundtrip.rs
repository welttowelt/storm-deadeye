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

//! Wave 2 Item 11: end-to-end trade-journal exercise.
//!
//! Bootstraps devnet, deploys a normal market, wraps a writer in a
//! [`JournalledNormalWriter`], submits a trade, then asserts the
//! JSONL file has one entry containing the expected fields and that
//! [`TradeJournal::replay`] deserializes it cleanly.
//!
//! Gated on `DEADEYE_RUN_INTEGRATION=1` and a running devnet at
//! `:5050`.

use deadeye_core::{NormalDistribution, Sq128};
use deadeye_sdk::{
    EntryKind, Family, JournalSink, JournalledNormalWriter, TradeJournal, starknet::JsonRpcProvider,
};
use deadeye_starknet::{Account, Felt, NormalMarketReader, NormalMarketWriter};
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
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
use url::Url;

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires running starknet-devnet on :5050; uses DEADEYE_RUN_INTEGRATION env var"]
async fn journal_round_trip_full_lifecycle() {
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
        .expect("upsert normal profile");

    let (initial_dist, _placeholder) = build_initial_normal_inputs(42.0, 64.0, 1000.0);
    let initial_hints = fetch_normal_hints(&hint_rpc, env.normal_runtime, initial_dist)
        .await
        .expect("hints");
    let market = deploy_normal_market_with_event(
        &admin_handle,
        env.factory,
        1,
        Felt::from(0xA5_5A_u64),
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
    .expect("initialize market");
    eprintln!("✅ market deployed + initialized: {market:#x}");

    // Trader prep
    let trader = env.participants.first().expect("at least one participant");
    let trader_handle = env.account_handle(trader);
    approve(
        trader_handle.clone(),
        env.collateral,
        market,
        1_000_000_000_000_000_000_000_u128,
    )
    .await
    .expect("approve");

    // Build candidate + use the chain's `quote_trade` to get
    // chain-correct collateral. This sidesteps off-chain/chain numerical
    // drift that produces VERIFICATION_FAILED at trade time.
    let cur = NormalDistribution::from_variance(
        Sq128::from_f64(42.0).unwrap(),
        Sq128::from_f64(64.0).unwrap(),
    )
    .unwrap();
    let candidate_mean = 45.0_f64;
    let candidate_variance = 49.0_f64;
    let candidate_dist = deadeye_core::distribution::NormalDistributionRaw {
        mean: Sq128::from_f64(candidate_mean).unwrap().to_raw(),
        variance: Sq128::from_f64(candidate_variance).unwrap().to_raw(),
        sigma: Sq128::from_f64(candidate_variance.sqrt()).unwrap().to_raw(),
    };
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
    .expect("solver");
    // Mirror the chaos suite's padding heuristic — off-chain
    // collateral × 20, floor 100, lifts every trade above
    // min_trade_collateral and gives the chain's verifier headroom.
    let supplied = (solver.collateral * 20.0_f64).max(100.0_f64);
    let cand_hints = fetch_normal_hints(&hint_rpc, env.normal_runtime, candidate_dist)
        .await
        .expect("hints");
    let quote = deadeye_starknet::NormalTradeQuote {
        candidate: candidate_dist,
        candidate_hints: cand_hints,
        x_star: Sq128::from_f64(solver.x_min).unwrap().to_raw(),
        required_collateral: Sq128::from_f64(solver.collateral).unwrap().to_raw(),
        padded_collateral: Sq128::from_f64(supplied).unwrap().to_raw(),
        on_chain_will_accept: true,
        rejection: None,
    };

    // Set up journal in a tmp dir.
    let tmp = tempfile::tempdir().unwrap();
    let journal_path = tmp.path().join("trades.jsonl");
    let journal = TradeJournal::open(&journal_path).expect("open journal");

    // Wrap the writer in JournalledNormalWriter.
    let writer_rpc = JsonRpcClient::new(HttpTransport::new(env.url.clone()));
    let writer_provider = JsonRpcProvider::new(writer_rpc);
    let base_writer = NormalMarketWriter::new(
        NormalMarketReader::new(&writer_provider, market),
        env.owned_account(trader),
    );
    let mut journalled = JournalledNormalWriter::new(base_writer, journal);

    // Submit the trade.
    let receipt = journalled
        .execute_quote(quote)
        .await
        .expect("trade executes");
    eprintln!("✅ trade tx: {:#x}", receipt.transaction_hash);

    // Drop the wrapper so the sink's BufWriter flushes its tail.
    let (_writer, mut sink) = journalled.into_parts();
    JournalSink::flush(&mut sink).expect("flush");
    drop(sink);

    // Replay.
    let entries: Vec<_> = TradeJournal::replay(&journal_path)
        .expect("replay opens")
        .collect::<Result<_, _>>()
        .expect("entries parse");
    assert_eq!(
        entries.len(),
        1,
        "expected exactly 1 entry, got {}",
        entries.len()
    );
    let e = &entries[0];
    assert_eq!(e.family, Family::Normal);
    assert_eq!(e.market, market);
    assert_eq!(e.trader, Account::address(&env.owned_account(trader)));
    assert_eq!(e.kind, EntryKind::Trade);
    assert_eq!(e.tx_hash, Some(receipt.transaction_hash));
    assert!(e.receipt.is_some(), "receipt should be embedded");
    assert!(e.off_chain_quote.is_object(), "quote serializes as object");
    eprintln!(
        "✅ journal round-trip: 1 entry, kind={:?}, tx={:#x}",
        e.kind,
        e.tx_hash.unwrap()
    );
}
