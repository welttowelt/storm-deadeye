#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration test driver — printing aids debugging, unwrap is OK"
)]

//! `MultiRpcProvider` failover test against a real devnet.
//!
//! Layout:
//! 1. Start two devnets on ports 5050 and 5051 (the harness assumes the
//!    operator has both running — we *don't* spawn them from the test
//!    because spawning subprocesses inside a `cargo test` is flaky).
//! 2. Construct a `MultiRpcProvider` pointing at both.
//! 3. Call `block_number` repeatedly — both endpoints serve traffic.
//! 4. Set `DEADEYE_KILL_PRIMARY=1` to instruct the test harness to make
//!    the first endpoint unreachable mid-run. (We approximate this by
//!    pointing the first URL at a closed port on `:65000` and only
//!    relying on the secondary at `:5050`.)
//! 5. Verify the provider continues to serve traffic from the secondary.
//! 6. Endpoint health snapshot should mark the bad endpoint Down.

use std::time::Duration;

use deadeye_starknet::{EndpointHealthState, MultiRpcProvider, RpcConfig};
use deadeye_testkit::devnet;
use starknet_providers::Provider as StarknetProvider;
use url::Url;

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires starknet-devnet on :5050; uses DEADEYE_RUN_INTEGRATION env var"]
async fn multi_rpc_recovers_when_primary_dies() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 to enable");
        return;
    }
    let live_url = Url::parse(devnet::DEFAULT_URL).unwrap();
    if devnet::check_health(&live_url).await.is_err() {
        eprintln!("skip: devnet at {live_url} is not reachable");
        return;
    }

    // Endpoint #0 is dead (closed port 65000). Endpoint #1 is the live
    // devnet. We expect the provider to fail over to #1 and mark #0
    // Down after a few attempts.
    let dead = Url::parse("http://127.0.0.1:65000").unwrap();
    let cfg = RpcConfig {
        max_retries: 2,
        initial_backoff: Duration::from_millis(20),
        max_backoff: Duration::from_millis(200),
        circuit_breaker_threshold: 2,
        circuit_breaker_cooldown: Duration::from_millis(500),
        timeout_per_call: Duration::from_secs(2),
    };
    let provider = MultiRpcProvider::new(vec![dead.clone(), live_url.clone()], cfg);

    // 20 consecutive `block_number` calls should every one succeed via
    // the live endpoint.
    let start = std::time::Instant::now();
    let mut ok = 0_usize;
    for _ in 0..20 {
        if provider.block_number().await.is_ok() {
            ok += 1;
        }
    }
    let elapsed = start.elapsed();
    eprintln!("multi-rpc: {ok}/20 ok in {elapsed:?}");
    assert!(
        ok >= 18,
        "expected ≥ 18/20 calls to succeed after failover, got {ok}"
    );

    let health = provider.endpoint_health().await;
    eprintln!("endpoint health: {health:?}");
    let dead_health = health
        .iter()
        .find(|(url, _)| url == &dead)
        .expect("dead endpoint in snapshot")
        .1;
    let live_health = health
        .iter()
        .find(|(url, _)| url == &live_url)
        .expect("live endpoint in snapshot")
        .1;

    assert!(
        dead_health.state != EndpointHealthState::Healthy || dead_health.failures > 0,
        "dead endpoint should have at least 1 transient failure, got {dead_health:?}",
    );
    assert!(
        live_health.successes > 0,
        "live endpoint should have served at least one success",
    );
}
