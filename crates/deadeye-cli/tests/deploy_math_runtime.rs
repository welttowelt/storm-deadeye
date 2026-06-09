#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used,
    reason = "integration tests panic on setup failure and print debug to stderr"
)]

//! Integration test for `deadeye admin deploy-math-runtime`.
//!
//! Gated behind two env flags so it never runs accidentally:
//!   `DEADEYE_RUN_INTEGRATION=1` (general devnet gate, shared with other
//!     write-path tests) **and**
//!   `DEADEYE_RUN_DEPLOY_MATH_RUNTIME=1` (this test specifically — it
//!     would otherwise compete with `bootstrap_devnet` for the same
//!     devnet account-0 nonce and slow down the suite).
//!
//! Setup:
//!   `starknet-devnet --seed 0 --accounts 10 --port 5050`.

use std::process::Command;

use deadeye_deployer::runtime::{ChainKey, Family as DeployerFamily, RuntimeCache};
use deadeye_testkit::fixture::{
    artifacts::AllArtifacts,
    declare::declare_idempotent,
    env::{BootstrapConfig, bootstrap_devnet},
};

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
        && std::env::var("DEADEYE_RUN_DEPLOY_MATH_RUNTIME").is_ok()
}

fn cli_binary() -> std::path::PathBuf {
    assert_cmd::cargo::cargo_bin("deadeye")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deploy_math_runtime_devnet_idempotent() {
    if !integration_enabled() {
        eprintln!(
            "skip: set DEADEYE_RUN_INTEGRATION=1 and DEADEYE_RUN_DEPLOY_MATH_RUNTIME=1 \
             with starknet-devnet on :5050"
        );
        return;
    }

    // Spin up a fresh devnet (resets to genesis).
    let env = bootstrap_devnet(BootstrapConfig::default())
        .await
        .expect("bootstrap succeeds");

    // The bootstrap declared every class for us; pull the normal math
    // runtime's class hash so we can pass it to the CLI as `--class-hash`.
    // (Devnet's chain id is non-mainnet → ChainKey::Other, slug "devnet",
    // per our slug logic. There's no pinned manifest class hash for devnet,
    // so we forward the freshly-declared hash.)
    let artifacts = AllArtifacts::load().expect("load artifacts");
    let admin_handle = env.account_handle(&env.admin);
    let class_hash = declare_idempotent(&admin_handle, &artifacts.normal_math_runtime)
        .await
        .expect("declare normal math runtime");

    // Create an isolated cache file for this test run.
    let tmp = tempfile::tempdir().expect("tempdir");
    let runtimes_path = tmp.path().join("runtimes.toml");

    let rpc_url = env.url.as_str();
    let admin_addr = format!("{:#x}", env.admin.address);
    let admin_key = format!("{:#x}", env.admin.private_key);
    let class_hex = format!("{class_hash:#x}");
    let salt = "0x0c1a55"; // deterministic so we can assert idempotency

    // First call: real deploy.
    let out1 = Command::new(cli_binary())
        .env("DEADEYE_RUNTIMES_PATH", &runtimes_path)
        .env("DEADEYE_PRIVATE_KEY", &admin_key)
        .env("DEADEYE_ADDRESS", &admin_addr)
        .env("DEADEYE_RPC_URL", rpc_url)
        .env("DEADEYE_CHAIN_ID", "0x534e5f5345504f4c4941")
        .args([
            "admin",
            "deploy-math-runtime",
            "--family",
            "normal",
            "--salt",
            salt,
            "--class-hash",
            &class_hex,
            "--confirm",
            "--output",
            "json",
        ])
        .output()
        .expect("first invocation runs");
    eprintln!(
        "STDOUT (deploy):\n{}",
        String::from_utf8_lossy(&out1.stdout)
    );
    eprintln!(
        "STDERR (deploy):\n{}",
        String::from_utf8_lossy(&out1.stderr)
    );
    assert!(out1.status.success(), "first deploy must succeed");

    let json1: serde_json::Value =
        serde_json::from_slice(&out1.stdout).expect("first invocation emits JSON");
    assert_eq!(json1["mode"], "deploy");
    assert_eq!(json1["family"], "normal");
    assert_eq!(json1["on_chain_verified"], true);
    let deployed_addr = json1["address"]
        .as_str()
        .expect("address present")
        .to_owned();
    let tx_hash = json1["tx_hash"]
        .as_str()
        .expect("tx_hash present")
        .to_owned();
    assert!(!deployed_addr.is_empty());
    assert!(!tx_hash.is_empty());

    // Cache must be populated.
    let cache = RuntimeCache::load(&runtimes_path).expect("cache loads");
    let entry = cache
        .get(ChainKey::Other, DeployerFamily::Normal)
        .expect("entry present");
    assert_eq!(entry.address, deployed_addr);
    assert_eq!(entry.class_hash, class_hex);

    // Second call: idempotency — same salt → cached fast-path.
    let out2 = Command::new(cli_binary())
        .env("DEADEYE_RUNTIMES_PATH", &runtimes_path)
        .env("DEADEYE_PRIVATE_KEY", &admin_key)
        .env("DEADEYE_ADDRESS", &admin_addr)
        .env("DEADEYE_RPC_URL", rpc_url)
        .env("DEADEYE_CHAIN_ID", "0x534e5f5345504f4c4941")
        .args([
            "admin",
            "deploy-math-runtime",
            "--family",
            "normal",
            "--salt",
            salt,
            "--class-hash",
            &class_hex,
            "--confirm",
            "--output",
            "json",
        ])
        .output()
        .expect("second invocation runs");
    eprintln!(
        "STDOUT (idempotent):\n{}",
        String::from_utf8_lossy(&out2.stdout)
    );
    eprintln!(
        "STDERR (idempotent):\n{}",
        String::from_utf8_lossy(&out2.stderr)
    );
    assert!(
        out2.status.success(),
        "second invocation must succeed (idempotent)"
    );

    let json2: serde_json::Value =
        serde_json::from_slice(&out2.stdout).expect("second invocation emits JSON");
    assert_eq!(json2["cached"], true, "must hit cached fast-path");
    assert_eq!(json2["address"], deployed_addr, "address must match");
    assert_eq!(json2["on_chain_verified"], true);

    // Third call: --status should agree the entry is alive.
    let out3 = Command::new(cli_binary())
        .env("DEADEYE_RUNTIMES_PATH", &runtimes_path)
        .env("DEADEYE_ADDRESS", &admin_addr)
        .env("DEADEYE_RPC_URL", rpc_url)
        .env("DEADEYE_CHAIN_ID", "0x534e5f5345504f4c4941")
        .args([
            "admin",
            "deploy-math-runtime",
            "--status",
            "--output",
            "json",
        ])
        .output()
        .expect("status invocation runs");
    eprintln!(
        "STDOUT (status):\n{}",
        String::from_utf8_lossy(&out3.stdout)
    );
    eprintln!(
        "STDERR (status):\n{}",
        String::from_utf8_lossy(&out3.stderr)
    );
    assert!(out3.status.success(), "status check must succeed");
    let json3: serde_json::Value =
        serde_json::from_slice(&out3.stdout).expect("status emits JSON array");
    let arr = json3.as_array().expect("status returns an array");
    assert!(!arr.is_empty(), "status array non-empty");
    assert!(
        arr.iter()
            .any(|row| row["address"] == deployed_addr && row["on_chain_verified"] == true),
        "status must list our entry as verified"
    );
}
