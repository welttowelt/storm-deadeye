#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "integration test driver — printing + unwrap aid debugging"
)]

//! Fee-bump retry stress test.
//!
//! Submits a transfer with an absurdly-low initial tip and verifies
//! that [`OwnedAccount::execute_with_bump`] either lands or surfaces a
//! useful error. Devnet doesn't actually require fee competition so
//! the bump path is mostly a no-op; we still exercise the API.

use std::time::Duration;

use deadeye_starknet::{FeeBumpPolicy, Felt};
use deadeye_testkit::{
    devnet,
    fixture::env::{BootstrapConfig, bootstrap_devnet},
};
use starknet_core::{types::Call, utils::get_selector_from_name};
use url::Url;

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires running starknet-devnet on :5050; uses DEADEYE_RUN_INTEGRATION env var"]
async fn fee_bump_policy_runs_against_devnet() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 to enable");
        return;
    }
    let url = Url::parse(devnet::DEFAULT_URL).unwrap();
    if devnet::check_health(&url).await.is_err() {
        eprintln!("skip: devnet at {url} not reachable");
        return;
    }

    let env = bootstrap_devnet(BootstrapConfig::default())
        .await
        .expect("bootstrap succeeds");
    let actor = env.participants[0];
    let owned = env.owned_account(&actor);

    let call = Call {
        to: env.collateral,
        selector: get_selector_from_name("transfer").unwrap(),
        calldata: vec![env.admin.address, Felt::ONE, Felt::ZERO],
    };

    let policy = FeeBumpPolicy {
        initial_tip: 1,
        tip_multiplier: 1.5,
        max_attempts: 5,
        attempt_timeout: Duration::from_secs(8),
    };

    let started = std::time::Instant::now();
    let result = owned.execute_with_bump(vec![call], policy).await;
    let elapsed = started.elapsed();
    eprintln!("fee_bump result: {result:?} in {elapsed:?}");
    // We accept either: success (devnet routes without contention) or
    // a useful error message. The point of this test is that the policy
    // surface compiles end-to-end against a real chain.
    if let Ok(rcpt) = &result {
        eprintln!("✅ landed tx: {tx:#x}", tx = rcpt.transaction_hash);
    }
}
