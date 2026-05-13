#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration tests in tests/ are top-level — print/panic are fine"
)]

//! Sanity test: bootstrap a devnet from scratch and verify the full
//! declare → deploy → configure pipeline succeeds.
//!
//! Requires a running `starknet-devnet --seed 0 --accounts 10 --port 5050`.
//! Skipped unless `DEADEYE_RUN_INTEGRATION=1` is set.

use deadeye_testkit::fixture::env::{BootstrapConfig, bootstrap_devnet};

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bootstrap_pipeline_succeeds() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 and start starknet-devnet on :5050");
        return;
    }
    let cfg = BootstrapConfig::default();
    let env = match bootstrap_devnet(cfg).await {
        Ok(env) => env,
        Err(e) => {
            eprintln!("BOOTSTRAP FAILED: {e:#?}");
            panic!("bootstrap failed: {e}");
        },
    };

    eprintln!(
        "✅ devnet bootstrap:\n  factory:           {:#x}\n  normal plugin:     {:#x}\n  lognormal plugin:  {:#x}\n  multinoulli plugin:{:#x}\n  bivariate plugin:  {:#x}\n  collateral token:  {:#x}\n  admin:             {:#x}\n  participants ({}): {:?}",
        env.factory,
        env.normal_plugin,
        env.lognormal_plugin,
        env.multinoulli_plugin,
        env.bivariate_plugin,
        env.collateral,
        env.admin.address,
        env.participants.len(),
        env.participants
            .iter()
            .map(|p| format!("{:#x}", p.address))
            .collect::<Vec<_>>(),
    );
}
