#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_precision_loss,
    reason = "integration test driver — printing aids debugging, unwrap is OK"
)]

//! `MultiRpcProvider` mid-flight failover test.
//!
//! Requires **two** running devnets — one on `:5050`, one on `:5051`.
//! Configure via env vars:
//!
//! * `DEADEYE_RUN_INTEGRATION=1` to enable.
//! * `DEADEYE_DEVNET_B_PID=<pid>` to identify the secondary devnet
//!   process; the test will `kill -KILL` it half-way through.
//!
//! Flow:
//! 1. Build a `MultiRpcProvider` over both endpoints.
//! 2. Issue 50 `block_number` calls — every endpoint should serve some.
//! 3. Kill the secondary devnet.
//! 4. Issue 50 more `block_number` calls — all must succeed via the
//!    primary.
//! 5. Inspect endpoint health. The killed endpoint must be marked
//!    `Down` (or `HalfOpen` after cooldown expires).

use std::time::{Duration, Instant};

use deadeye_starknet::{EndpointHealthState, MultiRpcProvider, RpcConfig};
use deadeye_testkit::devnet;
use starknet_providers::Provider as StarknetProvider;
use url::Url;

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires two devnets (:5050, :5051); set DEADEYE_DEVNET_B_PID to enable kill"]
async fn multi_rpc_recovers_when_secondary_dies_midflight() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 to enable");
        return;
    }
    let Some(b_pid) = std::env::var("DEADEYE_DEVNET_B_PID")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
    else {
        eprintln!("skip: set DEADEYE_DEVNET_B_PID=<pid of secondary devnet>");
        return;
    };

    let a = Url::parse("http://127.0.0.1:5050/").unwrap();
    let b = Url::parse("http://127.0.0.1:5051/").unwrap();

    // Quick reachability check.
    for url in [&a, &b] {
        let healthy = devnet::check_health(url).await.is_ok();
        assert!(healthy, "expected {url} to be reachable");
    }

    let cfg = RpcConfig {
        max_retries: 3,
        initial_backoff: Duration::from_millis(20),
        max_backoff: Duration::from_millis(200),
        circuit_breaker_threshold: 2,
        circuit_breaker_cooldown: Duration::from_secs(30),
        timeout_per_call: Duration::from_secs(2),
    };
    let provider = MultiRpcProvider::new(vec![a.clone(), b.clone()], cfg);

    // ── Phase 1: 50 healthy calls ─────────────────────────────────
    let mut ok = 0_usize;
    for _ in 0..50_u32 {
        if provider.block_number().await.is_ok() {
            ok += 1;
        }
    }
    eprintln!("phase 1 (both alive): {ok}/50 succeeded");
    assert_eq!(ok, 50, "phase 1 should be 50/50 with both endpoints alive");

    let h1 = provider.endpoint_health().await;
    eprintln!("health after phase 1: {h1:?}");
    let a_succ_phase1 = h1.iter().find(|(u, _)| u == &a).unwrap().1.successes;
    let b_succ_phase1 = h1.iter().find(|(u, _)| u == &b).unwrap().1.successes;
    assert!(
        a_succ_phase1 > 0 && b_succ_phase1 > 0,
        "both endpoints should have served at least one call: a={a_succ_phase1} b={b_succ_phase1}",
    );

    // ── Phase 2: kill secondary, then keep calling ────────────────
    // The "SAFETY" comment is conventional even though kill(2) is a
    // process-level call — clippy's `unnecessary_safety_comment` is
    // fine with a plain narrative comment.
    eprintln!("killing devnet pid {b_pid}");
    let kill_ok = std::process::Command::new("kill")
        .args(["-KILL", &b_pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(kill_ok, "kill -KILL on {b_pid} should succeed");
    // Give the OS a brief moment to tear down the socket.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let start = Instant::now();
    let mut post_ok = 0_usize;
    let mut post_err = 0_usize;
    for _ in 0..50_u32 {
        if provider.block_number().await.is_ok() {
            post_ok += 1;
        } else {
            post_err += 1;
        }
    }
    let recovery_total = start.elapsed();
    eprintln!(
        "phase 2 (secondary dead): {post_ok}/50 succeeded, {post_err} failed in {recovery_total:?}",
    );
    let h2 = provider.endpoint_health().await;
    eprintln!("health after phase 2: {h2:?}");
    let b_state = h2.iter().find(|(u, _)| u == &b).unwrap().1.state;

    assert_eq!(
        post_ok, 50,
        "all 50 post-kill calls should have succeeded via the primary"
    );
    assert!(
        matches!(
            b_state,
            EndpointHealthState::Down | EndpointHealthState::HalfOpen
        ),
        "killed endpoint should be Down or HalfOpen, got {b_state:?}",
    );
}
