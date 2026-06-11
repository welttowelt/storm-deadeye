#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    trivial_casts,
    reason = "integration test driver — printing + unwrap aid debugging"
)]

//! Concurrent nonce-stress test.
//!
//! Fires N concurrent `transfer` invocations through a single account
//! routed via a [`NonceManager`]. We verify:
//!
//! * Every `reserve()` call produced a distinct nonce.
//! * The account survived heavy concurrency without the dreaded "Account
//!   transaction nonce is invalid" cascade.
//! * Final chain nonce == initial + successful submissions.
//!
//! **Devnet caveat:** `starknet-devnet-rs` does not buffer future
//! nonces — submitting tx N+1 before tx N lands rejects with
//! `InvalidTransactionNonce`. Production Starknet sequencers
//! (Madara, Juno, Pathfinder) accept future nonces and order them
//! in their mempool. To exercise the *manager* against devnet we use
//! a small concurrency window (4) which fits inside devnet's accepted
//! window. Production deployments uncap this — the nonce manager
//! itself has no such bound.
//!
//! Gated on `DEADEYE_RUN_INTEGRATION=1` and a running devnet at
//! `:5050`. Without it the test prints a skip line and returns.

use std::sync::Arc;

use deadeye_starknet::{Felt, GasParams, NonceManager, account::AccountWithNonceManager};
use deadeye_testkit::{
    devnet,
    fixture::env::{BootstrapConfig, bootstrap_devnet},
};
use starknet_core::{types::Call, utils::get_selector_from_name};
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
use tokio::sync::Semaphore;
use url::Url;

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

/// Number of concurrent submissions we fire at devnet. Each tx is a 1-unit
/// STRK transfer.
const N: usize = 50;

/// In-flight cap. Devnet rejects future nonces past a small window, so
/// we bound concurrency to a value the devnet sequencer can absorb. The
/// SDK itself has no such cap — set `DEADEYE_NONCE_CONCURRENCY` to
/// override.
const DEFAULT_CONCURRENCY: usize = 4;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires running starknet-devnet on :5050; uses DEADEYE_RUN_INTEGRATION env var"]
async fn nonce_manager_fires_50_concurrent_trades() {
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
    eprintln!("✅ devnet bootstrapped — chain {:#x}", env.chain_id);

    let collateral = env.collateral;

    let actor = env.participants[0];
    let owned = env.owned_account(&actor);
    let nonce_fetcher: Arc<dyn deadeye_starknet::NonceFetcher> =
        Arc::new(JsonRpcClient::new(HttpTransport::new(env.url.clone())));
    let initial_nonce = owned.nonce().await.expect("initial nonce");
    eprintln!("initial chain nonce: {initial_nonce:#x}");

    let manager = NonceManager::new(nonce_fetcher, actor.address)
        .await
        .expect("nonce manager constructs");
    let signer = Arc::new(owned.with_nonce_manager(manager));

    let recipient = env.admin.address;
    let transfer_selector = get_selector_from_name("transfer").expect("transfer selector");

    let concurrency = std::env::var("DEADEYE_NONCE_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_CONCURRENCY);
    eprintln!("running {N} submissions with concurrency window {concurrency}");
    let sem = Arc::new(Semaphore::new(concurrency));

    let mut handles = Vec::with_capacity(N);
    let start = std::time::Instant::now();
    for _ in 0..N {
        let signer: Arc<AccountWithNonceManager> = Arc::clone(&signer);
        let sem = Arc::clone(&sem);
        let call = Call {
            to: collateral,
            selector: transfer_selector,
            // amount: u256(low=1, high=0)
            calldata: vec![recipient, Felt::ONE, Felt::ZERO],
        };
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await.unwrap();
            let guard = signer.manager().reserve().await;
            signer
                .execute_managed_with_gas(vec![call], guard, GasParams::generous_defaults())
                .await
        }));
    }

    let mut ok = 0_usize;
    let mut errors: Vec<String> = Vec::new();
    for h in handles {
        match h.await.expect("join task") {
            Ok(_rcpt) => ok += 1,
            Err(e) => errors.push(format!("{e}")),
        }
    }
    let elapsed = start.elapsed();
    eprintln!(
        "concurrent: {ok}/{N} succeeded in {elapsed:?} (throughput ≈ {tps:.1} tx/s)",
        tps = (ok as f64) / elapsed.as_secs_f64(),
    );
    for (i, e) in errors.iter().enumerate().take(5) {
        eprintln!("  ❌ #{i}: {e}");
    }
    let snap = signer.manager().snapshot().await;
    eprintln!(
        "nonce manager snapshot: next={} outstanding={} released={} anchor={}",
        snap.next, snap.outstanding, snap.released, snap.chain_anchor,
    );

    // Read the final nonce. It should equal initial + ok (modulo nonce
    // gaps from released-but-never-resubmitted reservations — we
    // resync to verify there's no gap).
    signer.manager().resync().await.expect("resync");
    let final_nonce = signer.inner_account().nonce().await.expect("final nonce");
    eprintln!("final chain nonce: {final_nonce:#x}");
    let initial_u64 = felt_to_u64(initial_nonce);
    let final_u64 = felt_to_u64(final_nonce);

    assert_eq!(
        final_u64 - initial_u64,
        ok as u64,
        "chain nonce ({final_u64}) − initial ({initial_u64}) = {}, expected {ok} successful submissions",
        final_u64 - initial_u64,
    );
    assert!(
        ok >= (N as f64 * 0.9) as usize,
        "expected ≥ 90% of {N} concurrent trades to land, got {ok}",
    );
}

fn felt_to_u64(felt: Felt) -> u64 {
    let bytes = felt.to_bytes_be();
    let (high, low) = bytes.split_at(24);
    assert!(high.iter().all(|b| *b == 0), "nonce overflows u64");
    let mut buf = [0_u8; 8];
    buf.copy_from_slice(low);
    // BE is deliberate — Starknet felt bytes are big-endian.
    #[allow(
        clippy::big_endian_bytes,
        reason = "Starknet felt encoding is big-endian — see Felt::to_bytes_be"
    )]
    u64::from_be_bytes(buf)
}
