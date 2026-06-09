//! End-to-end smoke tests for the `deadeye` binary.
//!
//! These exercise the CLI surface via `assert_cmd` so the
//! flag-parsing + dispatch layer is covered. They deliberately do not
//! depend on a live RPC for the common-case checks — the only test that
//! talks to mainnet is gated behind `DEADEYE_RUN_MAINNET=1`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    reason = "test harness — panics are how assertions are written"
)]

use std::process::Command;

use assert_cmd::prelude::*;
use predicates::str::contains;

fn deadeye() -> Command {
    Command::cargo_bin("deadeye").expect("binary built")
}

/// `deadeye --help` succeeds and mentions "Deadeye".
#[test]
fn help_mentions_deadeye() {
    deadeye()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Deadeye"));
}

/// `deadeye config show` is offline-friendly: it must work when no
/// config file is present and prints the resolved config from env vars.
#[test]
fn config_show_with_env_overrides() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg_path = tmp.path().join("config.toml");
    deadeye()
        .env("DEADEYE_CONFIG", &cfg_path)
        .env("DEADEYE_PROFILE", "envtest")
        .env("DEADEYE_RPC_URL", "https://example.com/rpc")
        .env("DEADEYE_INDEXER_URL", "https://example.com/idx")
        .env("DEADEYE_ADDRESS", "0xabc123")
        .arg("config")
        .arg("show")
        .arg("--no-color")
        .arg("--output")
        .arg("plain")
        .assert()
        .success()
        .stdout(contains("active_profile: envtest"))
        .stdout(contains("rpc_url: https://example.com/rpc"))
        .stdout(contains("address: 0xabc123"));
}

/// `deadeye config init` writes a valid TOML file. `deadeye config show`
/// then reads back the same profile.
#[test]
fn config_init_then_show() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg_path = tmp.path().join("config.toml");
    deadeye()
        .env("DEADEYE_CONFIG", &cfg_path)
        .arg("config")
        .arg("init")
        .arg("--profile")
        .arg("smoke")
        .arg("--rpc-url")
        .arg("https://rpc.example/")
        .arg("--indexer-url")
        .arg("https://idx.example/")
        .arg("--address")
        .arg("0xdeadbeef")
        .arg("--set-default")
        .assert()
        .success();

    assert!(cfg_path.exists(), "config file must exist after init");
    let contents = std::fs::read_to_string(&cfg_path).expect("read config file");
    assert!(contents.contains("[profiles.smoke]"));
    assert!(contents.contains("0xdeadbeef"));

    deadeye()
        .env("DEADEYE_CONFIG", &cfg_path)
        .arg("config")
        .arg("show")
        .arg("--output")
        .arg("plain")
        .assert()
        .success()
        .stdout(contains("active_profile: smoke"))
        .stdout(contains("address: 0xdeadbeef"));
}

/// `deadeye config profile-list --output json` returns a JSON array.
#[test]
fn profile_list_json_is_valid_array() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg_path = tmp.path().join("config.toml");
    deadeye()
        .env("DEADEYE_CONFIG", &cfg_path)
        .arg("config")
        .arg("init")
        .arg("--profile")
        .arg("a")
        .arg("--address")
        .arg("0x1")
        .assert()
        .success();
    let output = deadeye()
        .env("DEADEYE_CONFIG", &cfg_path)
        .arg("config")
        .arg("profile-list")
        .arg("--output")
        .arg("json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value =
        serde_json::from_slice(&output).expect("profile-list JSON parses");
    assert!(parsed.is_array(), "profile-list JSON must be an array");
}

/// `deadeye markets show <addr> --output json` against the live mainnet
/// indexer-discovered market produces parseable JSON. Gated.
#[test]
#[ignore = "requires mainnet access; set DEADEYE_RUN_MAINNET=1 and \
            DEADEYE_MAINNET_MARKET_ADDR=0x… to enable"]
fn markets_show_json_mainnet_gated() {
    if std::env::var_os("DEADEYE_RUN_MAINNET").is_none() {
        eprintln!("skip: DEADEYE_RUN_MAINNET not set");
        return;
    }
    let market = std::env::var("DEADEYE_MAINNET_MARKET_ADDR")
        .expect("DEADEYE_MAINNET_MARKET_ADDR required for this gated test");
    let output = deadeye()
        .arg("markets")
        .arg("show")
        .arg(&market)
        .arg("--output")
        .arg("json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value =
        serde_json::from_slice(&output).expect("markets show JSON parses");
    assert!(parsed.is_object());
    assert!(parsed.get("address").is_some(), "address field present");
}

/// `deadeye markets list` against the live indexer returns a non-empty
/// list. Gated.
#[test]
#[ignore = "requires mainnet access; set DEADEYE_RUN_MAINNET=1 to enable"]
fn markets_list_mainnet_gated() {
    if std::env::var_os("DEADEYE_RUN_MAINNET").is_none() {
        eprintln!("skip: DEADEYE_RUN_MAINNET not set");
        return;
    }
    let output = deadeye()
        .arg("markets")
        .arg("list")
        .arg("--output")
        .arg("json")
        .arg("--limit")
        .arg("3")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value =
        serde_json::from_slice(&output).expect("markets list JSON parses");
    let arr = parsed.as_array().expect("array");
    assert!(!arr.is_empty(), "mainnet indexer should return ≥1 market");
}

// ─── `deadeye admin deploy-math-runtime` confirm-gate smoke tests ─────
//
// These verify the **dry-run / --confirm split** without touching any
// chain. The dry-run path doesn't call `getClassHashAt` when the cache
// is empty, so an isolated `DEADEYE_RUNTIMES_PATH` keeps them hermetic.

/// Without `--confirm` the CLI must emit a `dry_run` result and never
/// reach the deploy submission code path. Output is JSON for parsing.
#[test]
fn deploy_math_runtime_without_confirm_is_dry_run() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let runtimes = tmp.path().join("runtimes.toml");
    let cfg = tmp.path().join("config.toml");
    let output = deadeye()
        .env("DEADEYE_CONFIG", &cfg)
        .env("DEADEYE_RUNTIMES_PATH", &runtimes)
        .env("DEADEYE_ADDRESS", "0xdeadbeef")
        .env("DEADEYE_CHAIN_ID", "0x534e5f4d41494e") // mainnet
        .env_remove("DEADEYE_PRIVATE_KEY")
        .args([
            "admin",
            "deploy-math-runtime",
            "--family",
            "normal",
            "--salt",
            "0x42",
            "--output",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&output).expect("dry-run emits JSON");
    assert_eq!(parsed["mode"], "dry_run", "must be a dry-run");
    assert_eq!(parsed["family"], "normal");
    assert_eq!(parsed["chain"], "mainnet");
    assert_eq!(parsed["on_chain_verified"], false);
    assert_eq!(parsed["cached"], false);
    assert!(parsed["tx_hash"].is_null(), "no tx submitted in dry-run");
    // Cache must NOT be populated by a dry-run.
    assert!(
        !runtimes.exists(),
        "dry-run must not write to the cache file"
    );
}

/// `--status` with an empty cache prints a single "no cached runtimes"
/// row and exits 0 — no RPC contact.
#[test]
fn deploy_math_runtime_status_empty_cache() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let runtimes = tmp.path().join("runtimes.toml");
    let cfg = tmp.path().join("config.toml");
    let output = deadeye()
        .env("DEADEYE_CONFIG", &cfg)
        .env("DEADEYE_RUNTIMES_PATH", &runtimes)
        .env("DEADEYE_ADDRESS", "0xdeadbeef")
        .env("DEADEYE_CHAIN_ID", "0x534e5f4d41494e")
        .args([
            "admin",
            "deploy-math-runtime",
            "--status",
            "--output",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&output).expect("status emits JSON");
    assert_eq!(parsed["mode"], "status");
    assert_eq!(parsed["cached"], false);
    assert_eq!(parsed["chain"], "mainnet");
}

/// `--family` is required unless `--status` is set. Without either,
/// clap parsing succeeds but the handler aborts with a friendly error.
#[test]
fn deploy_math_runtime_requires_family_or_status() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let runtimes = tmp.path().join("runtimes.toml");
    let cfg = tmp.path().join("config.toml");
    deadeye()
        .env("DEADEYE_CONFIG", &cfg)
        .env("DEADEYE_RUNTIMES_PATH", &runtimes)
        .env("DEADEYE_ADDRESS", "0xdeadbeef")
        .env("DEADEYE_CHAIN_ID", "0x534e5f4d41494e")
        .args(["admin", "deploy-math-runtime", "--output", "json"])
        .assert()
        .failure()
        .stderr(contains("--family"));
}

/// `--confirm --output json` with no private key in env must NOT attempt
/// a deploy — the `OwnedAccount` build short-circuits with a friendly
/// "set `DEADEYE_PRIVATE_KEY`" error. This proves the confirm-gate
/// reaches deploy code only when a key is present, and that scripted
/// (`--output json`) usage doesn't spawn an interactive prompt.
#[test]
fn deploy_math_runtime_confirm_without_key_errors_before_deploy() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let runtimes = tmp.path().join("runtimes.toml");
    let cfg = tmp.path().join("config.toml");
    let assert = deadeye()
        .env("DEADEYE_CONFIG", &cfg)
        .env("DEADEYE_RUNTIMES_PATH", &runtimes)
        .env("DEADEYE_ADDRESS", "0xdeadbeef")
        .env("DEADEYE_CHAIN_ID", "0x534e5f4d41494e")
        .env_remove("DEADEYE_PRIVATE_KEY")
        .args([
            "admin",
            "deploy-math-runtime",
            "--family",
            "normal",
            "--salt",
            "0x42",
            "--confirm",
            "--output",
            "json",
        ])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        stderr.contains("DEADEYE_PRIVATE_KEY") || stderr.contains("private key"),
        "expected key-required error, got: {stderr}"
    );
}
