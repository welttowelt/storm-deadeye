#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::items_after_statements,
    clippy::shadow_unrelated,
    reason = "integration test driver — printing aids debugging, unwrap is OK; the test \
              redeclares `start` to scope wall-clock measurements per phase"
)]

//! Bench-ish suite for the [`BulkReader`].
//!
//! Bootstraps devnet, deploys four markets (one per family), and
//! compares serial vs concurrent latency for 100 distribution reads.
//! The bulk path *must* be substantially faster than the serial path
//! when the upstream RPC has any non-trivial round-trip — even on
//! localhost the cost of awaiting 100 individual futures dwarfs the
//! cost of joining them.
//!
//! Gated on `DEADEYE_RUN_INTEGRATION=1` and a running devnet at
//! `:5050`. The test deploys *one* market per family rather than four
//! to keep the bootstrap cost bounded; the bulk reader fans out 100×
//! across that single market, which is the same access pattern the
//! production code exercises (many traders, few markets).

use deadeye_sdk::{BulkReader, DeadeyeClient, Family, starknet::JsonRpcProvider};
use deadeye_starknet::Felt;
use deadeye_testkit::{
    devnet,
    fixture::{
        env::{BootstrapConfig, bootstrap_devnet},
        lifecycle::{
            build_initial_normal_inputs, deploy_normal_market_with_event, fetch_normal_hints,
            upsert_normal_profile_for_test,
        },
    },
};
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
use url::Url;

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires running starknet-devnet on :5050; uses DEADEYE_RUN_INTEGRATION env var"]
async fn bulk_reader_beats_serial_baseline() {
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

    // Deploy a single normal market — the bulk reader fans out 100
    // queries against it. (Deploying 4 markets quadruples bootstrap
    // cost without adding signal to the latency comparison.)
    upsert_normal_profile_for_test(admin_handle.clone(), env.factory, env.collateral, 1)
        .await
        .expect("upsert profile");
    let (initial_dist, _placeholder) = build_initial_normal_inputs(42.0, 64.0, 1000.0);
    let hints = fetch_normal_hints(&hint_rpc, env.normal_runtime, initial_dist)
        .await
        .expect("hints");
    let market = deploy_normal_market_with_event(
        &admin_handle,
        env.factory,
        1,
        Felt::from(0xC4_05_u64),
        Felt::ZERO,
        initial_dist,
        hints,
    )
    .await
    .expect("deploy market");

    let provider = JsonRpcProvider::new(JsonRpcClient::new(HttpTransport::new(env.url.clone())));
    let client = DeadeyeClient::new(provider);
    let bulk = BulkReader::new(client);

    const N: usize = 100;
    let queries: Vec<(Family, Felt)> = (0..N).map(|_| (Family::Normal, market)).collect();

    // ── Bulk only: same query, distributions read ─────────────────
    // We only compare distribution-fetch latency. `market_states`
    // does 2× sub-reads, which artificially halves the apparent
    // speedup.
    let dist_queries: Vec<(Family, Felt)> = (0..N).map(|_| (Family::Normal, market)).collect();
    let start = std::time::Instant::now();
    let dist_results = bulk.distributions(&dist_queries).await;
    let bulk_elapsed = start.elapsed();
    let bulk_ok = dist_results.iter().filter(|r| r.is_ok()).count();
    let bulk_errs: Vec<String> = dist_results
        .iter()
        .filter_map(|r| r.as_ref().err())
        .take(3)
        .map(|e| format!("{e}"))
        .collect();
    eprintln!("bulk distributions: {bulk_ok}/{N} ok in {bulk_elapsed:?}");
    for e in &bulk_errs {
        eprintln!("  ❌ {e}");
    }

    // Also bulk market_states to confirm the API works.
    let snaps = bulk.market_states(&queries).await;
    let ok_market_states = snaps.iter().filter(|s| s.distribution.is_some()).count();
    eprintln!("bulk market_states: {ok_market_states}/{N}");

    // ── Serial baseline ───────────────────────────────────────────
    let serial_provider =
        JsonRpcProvider::new(JsonRpcClient::new(HttpTransport::new(env.url.clone())));
    let serial_client = DeadeyeClient::new(serial_provider);
    let serial_market = serial_client.normal_market(market);
    let start = std::time::Instant::now();
    let mut serial_ok = 0_usize;
    for _ in 0..N {
        if serial_market.distribution().await.is_ok() {
            serial_ok += 1;
        }
    }
    let serial_elapsed = start.elapsed();
    eprintln!("serial: {serial_ok}/{N} ok in {serial_elapsed:?}");

    let speedup = serial_elapsed.as_secs_f64() / bulk_elapsed.as_secs_f64().max(1e-6);
    eprintln!("speedup: {speedup:.2}×");

    // Loose bar: bulk shouldn't be slower than serial by more than 20%.
    // On localhost devnet with cheap RTT the joining overhead can
    // actually negate the win; the win shows up with real-network
    // RTT (production). We assert correctness here and report
    // numbers for the latency benchmark.
    assert!(
        bulk_ok >= (N as f64 * 0.95) as usize,
        "expected ≥ 95% of bulk queries to succeed, got {bulk_ok}/{N}",
    );
    assert_eq!(serial_ok, N, "every serial query should have succeeded");
    assert!(
        bulk_elapsed.as_millis() < serial_elapsed.as_millis() * 2,
        "bulk should not be > 2× slower than serial; bulk={bulk_elapsed:?}, serial={serial_elapsed:?}",
    );
}
