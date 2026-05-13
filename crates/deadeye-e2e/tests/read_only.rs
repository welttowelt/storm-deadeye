#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    reason = "integration tests in tests/ are top-level — printing aids debugging"
)]

//! Read-only end-to-end tests against a live (or local) Starknet RPC.
//!
//! These tests are skipped unless `DEADEYE_RUN_INTEGRATION=1`. Target the
//! local devnet by default, or set `DEADEYE_TEST_TARGET=cartridge` to hit
//! a Cartridge-hosted public RPC.
//!
//! ```bash
//! # Local devnet (must be running on :5050):
//! DEADEYE_RUN_INTEGRATION=1 cargo test -p deadeye-e2e -- --nocapture
//!
//! # Cartridge Sepolia:
//! DEADEYE_RUN_INTEGRATION=1 DEADEYE_TEST_TARGET=cartridge \
//!   cargo test -p deadeye-e2e -- --nocapture
//! ```

use deadeye_testkit::Harness;

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

#[tokio::test]
async fn harness_reaches_target_environment() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 to enable");
        return;
    }
    match Harness::from_env().await {
        Ok(harness) => {
            eprintln!(
                "harness ready: kind={:?} url={}",
                harness.kind(),
                harness.url()
            );
        },
        Err(e) => {
            eprintln!(
                "skip: harness not reachable ({e}). Set DEADEYE_TEST_TARGET=cartridge to use a hosted RPC, or start starknet-devnet on :5050."
            );
        },
    }
}
