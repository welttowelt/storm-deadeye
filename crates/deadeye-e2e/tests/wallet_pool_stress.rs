#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "integration test driver — printing + unwrap aid debugging"
)]

//! Wallet-pool concurrency stress test.
//!
//! Builds a pool of 5 devnet accounts, fires 100 concurrent transfers
//! using [`WalletPool::lease`], and asserts:
//! 1. All 100 submissions succeeded.
//! 2. Per-wallet load is roughly balanced (each wallet handled
//!    20 ± 5 tx under round-robin).
//!
//! Gated on `DEADEYE_RUN_INTEGRATION=1` + devnet on `:5050`.

use std::sync::Arc;

use deadeye_starknet::{Felt, GasParams, NonceFetcher, NonceManager, WalletPool};
use deadeye_testkit::{
    devnet,
    fixture::env::{BootstrapConfig, bootstrap_devnet},
};
use starknet_accounts::Account as _;
use starknet_core::{types::Call, utils::get_selector_from_name};
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
use tokio::sync::Semaphore;
use url::Url;

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

const N_TX: usize = 100;
const POOL_SIZE: usize = 5;
const DEFAULT_CONCURRENCY: usize = 8;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires running starknet-devnet on :5050"]
async fn wallet_pool_fires_100_concurrent_tx() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 to enable");
        return;
    }
    let url = Url::parse(devnet::DEFAULT_URL).unwrap();
    if devnet::check_health(&url).await.is_err() {
        eprintln!("skip: devnet at {url} unreachable");
        return;
    }

    let env = bootstrap_devnet(BootstrapConfig {
        participant_count: POOL_SIZE,
        ..Default::default()
    })
    .await
    .expect("bootstrap succeeds");
    eprintln!("✅ devnet bootstrapped — chain {:#x}", env.chain_id);

    // Build pool: one OwnedAccount + one NonceManager per participant.
    let mut accounts = Vec::with_capacity(POOL_SIZE);
    let mut managers = Vec::with_capacity(POOL_SIZE);
    for participant in env.participants.iter().take(POOL_SIZE) {
        let owned = env.owned_account(participant);
        accounts.push(Arc::new(owned));
        let fetcher: Arc<dyn NonceFetcher> =
            Arc::new(JsonRpcClient::new(HttpTransport::new(env.url.clone())));
        let nm = NonceManager::new(fetcher, participant.address)
            .await
            .expect("nonce manager builds");
        managers.push(nm);
    }
    let pool = Arc::new(WalletPool::new(accounts, managers).unwrap());

    let recipient = env.admin.address;
    let xfer = get_selector_from_name("transfer").unwrap();

    let concurrency = std::env::var("DEADEYE_POOL_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_CONCURRENCY);
    let sem = Arc::new(Semaphore::new(concurrency));
    eprintln!(
        "running {N_TX} submissions across pool of {POOL_SIZE} wallets, concurrency {concurrency}",
    );

    let started = std::time::Instant::now();
    let mut handles = Vec::with_capacity(N_TX);
    for _ in 0..N_TX {
        let pool: Arc<WalletPool> = Arc::clone(&pool);
        let sem: Arc<Semaphore> = Arc::clone(&sem);
        let call = Call {
            to: env.collateral,
            selector: xfer,
            calldata: vec![recipient, Felt::ONE, Felt::ZERO],
        };
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await.unwrap();
            let lease = pool.lease().await;
            let slot = lease.slot;
            // Submit through the inner SingleOwnerAccount, bypassing
            // fee estimation by setting explicit gas.
            let g = GasParams::generous_defaults();
            let nonce = lease.nonce.value();
            let exec = lease
                .account
                .inner()
                .execute_v3(vec![call])
                .nonce(nonce)
                .l1_gas(g.l1_gas)
                .l1_gas_price(g.l1_gas_price)
                .l2_gas(g.l2_gas)
                .l2_gas_price(g.l2_gas_price)
                .l1_data_gas(g.l1_data_gas)
                .l1_data_gas_price(g.l1_data_gas_price)
                .tip(g.tip);
            let outcome = exec.send().await;
            match outcome {
                Ok(_) => {
                    lease.nonce.commit();
                    Ok::<usize, String>(slot)
                },
                Err(e) => Err(format!("slot={slot}: {e}")),
            }
        }));
    }

    let mut slot_counts: [usize; POOL_SIZE] = [0; POOL_SIZE];
    let mut ok = 0_usize;
    let mut errors: Vec<String> = Vec::new();
    for h in handles {
        match h.await.expect("join") {
            Ok(slot) => {
                ok += 1;
                slot_counts[slot] += 1;
            },
            Err(e) => errors.push(e),
        }
    }
    let elapsed = started.elapsed();
    let tps = (ok as f64) / elapsed.as_secs_f64();
    eprintln!("pool: {ok}/{N_TX} succeeded in {elapsed:?} (throughput ≈ {tps:.1} tx/s)",);
    eprintln!("per-wallet distribution: {slot_counts:?}");
    for (i, e) in errors.iter().enumerate().take(5) {
        eprintln!("  ❌ #{i}: {e}");
    }

    assert!(
        ok >= (N_TX as f64 * 0.9) as usize,
        "expected ≥ 90% of {N_TX} pool submissions to land, got {ok}",
    );
    // Round-robin should give each wallet ~20 tx ± 5.
    let expected = (N_TX / POOL_SIZE) as i64;
    for (i, c) in slot_counts.iter().enumerate() {
        let dev = (*c as i64 - expected).abs();
        assert!(
            dev <= 8,
            "wallet {i} handled {c} tx (expected ≈{expected}, deviation {dev})",
        );
    }
}
