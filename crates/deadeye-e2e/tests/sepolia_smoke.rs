#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration smoke test — printing aids debugging, unwrap + panic mark hard \
              invariants on a read-only health check"
)]

//! # Sepolia read-only smoke
//!
//! Wave-3 closes the "100% of integration testing runs against devnet"
//! gap. Real Sepolia behaves differently from devnet: real gas costs,
//! real latency, real failure modes, and no admin-via-`only_owner`
//! shortcuts. This test exercises every Wave-1/2 reader path against
//! a live Sepolia full-node so any wire-format / RPC-compatibility
//! regression surfaces in CI before a paying customer trips on it.
//!
//! ## What it does
//!
//! 1. Connects a [`JsonRpcClient`] to the configured Sepolia RPC.
//! 2. Reads + asserts the chain id matches Sepolia.
//! 3. For a known-live Sepolia normal market:
//!    - reads the distribution
//!    - reads the AMM params
//!    - reads the LP info
//!    - reads a known trader's position
//!    - submits a benign `quote_trade` and asserts the verifier either
//!      accepts the candidate OR returns a *typed*
//!      [`TradeRejectionReason`] (it must not panic or return
//!      `Submission`).
//! 4. Confirms [`BulkReader`] works against Sepolia: parallel
//!    distribution reads for 5 markets, parallel position reads for 5
//!    traders.
//!
//! ## What it does NOT do
//!
//! This test is **strictly read-only**. No transactions are submitted
//! — Sepolia STRK is a real resource and the smoke fires on every CI
//! run. If you need a real submission, do it manually with the chaos
//! suite + a funded signer.
//!
//! ## How to run locally
//!
//! ```bash
//! export DEADEYE_RUN_SEPOLIA=1
//! export DEADEYE_SEPOLIA_RPC="https://api.zan.top/public/starknet-sepolia/rpc/v0_10"
//! # A known-live Sepolia normal-family market. The deadeye indexer at
//! # https://situation-indexer.fly.dev/api/markets is the canonical
//! # source — pick any market with `marketType: "normal"`.
//! export DEADEYE_SEPOLIA_MARKET_ADDR=0x…
//! # Optional: a trader address known to hold a position on the above
//! # market. Defaults to `0x0` (= "no live position", which is fine —
//! # the reader path is the contract; reading a position for an
//! # account that hasn't traded returns the zero position).
//! export DEADEYE_SEPOLIA_TRADER_ADDR=0x…
//! cargo test -p deadeye-e2e --test sepolia_smoke -- --ignored --nocapture
//! ```
//!
//! ## Gating
//!
//! Without `DEADEYE_RUN_SEPOLIA=1` the test logs a `skip:` line and
//! exits cleanly. In CI this is a *nightly* job: shared public RPC
//! endpoints rate-limit aggressively, so we don't fire it on every
//! commit.

use std::env;

use deadeye_core::Distribution;
use deadeye_sdk::{
    DeadeyeClient,
    bulk::{BulkReader, Family},
    starknet::JsonRpcProvider,
};
use deadeye_starknet::{Felt, NormalMarketReader, types::common::AmmParamsRaw};
use starknet_core::types::BlockId;
use starknet_providers::{JsonRpcClient, Provider, jsonrpc::HttpTransport};
use url::Url;

/// Canonical Starknet Sepolia chain id (`0x534e5f5345504f4c4941`).
///
/// Computed from the cairo short string `"SN_SEPOLIA"`. Verified
/// against the live `starknet_chainId` RPC response.
const SEPOLIA_CHAIN_ID_HEX: &str = "0x534e5f5345504f4c4941";

/// Default public Sepolia RPC endpoint. Used when
/// `DEADEYE_SEPOLIA_RPC` is unset.
const DEFAULT_SEPOLIA_RPC: &str = "https://api.zan.top/public/starknet-sepolia/rpc/v0_10";

fn run_enabled() -> bool {
    env::var("DEADEYE_RUN_SEPOLIA").is_ok()
}

fn sepolia_rpc() -> String {
    env::var("DEADEYE_SEPOLIA_RPC").unwrap_or_else(|_| DEFAULT_SEPOLIA_RPC.to_owned())
}

fn sepolia_market() -> Option<Felt> {
    env::var("DEADEYE_SEPOLIA_MARKET_ADDR")
        .ok()
        .and_then(|s| Felt::from_hex(&s).ok())
}

fn sepolia_trader() -> Option<Felt> {
    env::var("DEADEYE_SEPOLIA_TRADER_ADDR")
        .ok()
        .and_then(|s| Felt::from_hex(&s).ok())
}

/// Top-level smoke. Reads the chain id, a known market's state, a
/// known trader's position, and a benign `quote_trade`. Then fans out
/// a small [`BulkReader`] batch to confirm concurrency works against
/// the live endpoint.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Sepolia access; uses DEADEYE_RUN_SEPOLIA env var. \
            See module docs for required env vars."]
async fn sepolia_read_only_smoke() {
    if !run_enabled() {
        eprintln!("skip: set DEADEYE_RUN_SEPOLIA=1 to enable");
        return;
    }
    let rpc_url = sepolia_rpc();
    eprintln!("▶ Sepolia smoke against {rpc_url}");
    let url = Url::parse(&rpc_url).expect("DEADEYE_SEPOLIA_RPC must be a valid URL");
    let rpc = JsonRpcClient::new(HttpTransport::new(url));

    // 1. Chain id matches Sepolia.
    let chain_id = rpc.chain_id().await.expect("starknet_chainId");
    let expected = Felt::from_hex(SEPOLIA_CHAIN_ID_HEX).expect("constant felt parses");
    assert_eq!(
        chain_id, expected,
        "expected Sepolia chain id {expected:#x}, got {chain_id:#x}",
    );
    eprintln!("✅ chain id = {chain_id:#x} (SN_SEPOLIA)");

    // Block number sanity check — the node is alive and ahead of genesis.
    let block_id = BlockId::Tag(starknet_core::types::BlockTag::Latest);
    let block_number = rpc.block_number().await.expect("block_number");
    assert!(block_number > 0, "Sepolia block_number must be > 0");
    eprintln!("✅ block_number = {block_number} (block_id={block_id:?})");

    // 2. Known-live normal market reads.
    let Some(market_addr) = sepolia_market() else {
        eprintln!(
            "⚠️  DEADEYE_SEPOLIA_MARKET_ADDR unset — skipping market / position / bulk paths."
        );
        return;
    };
    eprintln!("▶ reading market {market_addr:#x}");

    let provider = JsonRpcProvider::new(JsonRpcClient::new(HttpTransport::new(
        Url::parse(&rpc_url).expect("URL parses"),
    )));
    let reader = NormalMarketReader::new(&provider, market_addr);

    // 2a. get_distribution.
    let dist = reader
        .distribution()
        .await
        .expect("Sepolia normal market: distribution() must read");
    eprintln!(
        "  ✅ distribution: μ = {mean:.6}, σ = {sigma:.6}",
        mean = dist.mean().to_f64(),
        sigma = dist.sigma().to_f64(),
    );

    // 2b. get_params.
    let params: AmmParamsRaw = reader
        .params()
        .await
        .expect("Sepolia normal market: params() must read");
    eprintln!(
        "  ✅ params: k={k:.4}, backing={backing:.4}, tol={tol:.4e}",
        k = deadeye_core::Sq128::from_raw(params.k).to_f64(),
        backing = deadeye_core::Sq128::from_raw(params.backing).to_f64(),
        tol = deadeye_core::Sq128::from_raw(params.tolerance).to_f64(),
    );

    // 2c. get_lp_info.
    let lp_info = reader
        .lp_info()
        .await
        .expect("Sepolia normal market: lp_info() must read");
    eprintln!(
        "  ✅ lp_info: total_shares={shares:.4}, backing_deposited={bd:.4}",
        shares = deadeye_core::Sq128::from_raw(lp_info.total_shares).to_f64(),
        bd = deadeye_core::Sq128::from_raw(lp_info.total_backing_deposited).to_f64(),
    );

    // 2d. Read a trader's position. Falls back to address-zero — the
    // reader contract returns a well-formed empty position when the
    // trader has never touched the market. We're testing the wire,
    // not the position.
    let trader = sepolia_trader().unwrap_or(Felt::ZERO);
    eprintln!("▶ reading position for trader {trader:#x}");
    let position = reader
        .position(trader)
        .await
        .expect("Sepolia normal market: position() must read");
    eprintln!(
        "  ✅ position: total_collateral={tc:.6}",
        tc = deadeye_core::Sq128::from_raw(position.total_collateral).to_f64(),
    );

    // 2e. Benign quote_trade — candidate = current distribution (no-op
    // shift). The verifier should accept it OR return a typed
    // rejection reason. Either way the read path is exercised; we
    // refuse a `Submission`-typed error (those mean RPC compatibility
    // broke).
    //
    // We need a runtime address for the math runtime; the indexer
    // exposes that per network, but the read path the smoke covers
    // does NOT include the runtime lookup. Skip the runtime portion
    // when the runtime env var is unset — `quote_trade` requires it.
    let runtime_addr = env::var("DEADEYE_SEPOLIA_NORMAL_RUNTIME_ADDR")
        .ok()
        .and_then(|s| Felt::from_hex(&s).ok());
    if let Some(runtime) = runtime_addr {
        let cur_raw = dist.to_raw();
        let x_star = cur_raw.mean; // pick μ as the stationary point
        let supplied = deadeye_core::Sq128::from_f64(0.0)
            .expect("0.0 -> Sq128")
            .to_raw();
        let quote_result = reader
            .quote_trade(runtime, cur_raw, x_star, supplied, supplied)
            .await;
        match quote_result {
            Ok(quote) => {
                eprintln!(
                    "  ✅ quote_trade: on_chain_will_accept={accept}, rejection={rej:?}",
                    accept = quote.on_chain_will_accept,
                    rej = quote.rejection,
                );
            },
            Err(err) => {
                if let Some(reason) = err.rejection() {
                    eprintln!("  ✅ quote_trade rejected (typed): {reason:?}");
                } else {
                    panic!(
                        "quote_trade returned a non-typed Submission error — \
                         wire-format / RPC compatibility regression: {err}"
                    );
                }
            },
        }
    } else {
        eprintln!("  ⚠️  DEADEYE_SEPOLIA_NORMAL_RUNTIME_ADDR unset — skipping quote_trade smoke.");
    }

    // 3. BulkReader sanity. We need 5 markets + 5 traders for the
    // brief's "5 markets, 5 traders, parallel" check. Without a curated
    // list we read the same market / trader N times — the goal is the
    // *concurrency* path, not 5 distinct payloads.
    eprintln!("▶ BulkReader: 5 markets × distribution + 5 traders × position");
    // BulkReader wraps a DeadeyeClient — wrap our provider so the
    // bulk path runs against the same HTTP transport.
    let bulk_provider = JsonRpcProvider::new(JsonRpcClient::new(HttpTransport::new(
        Url::parse(&rpc_url).expect("URL parses"),
    )));
    let bulk_client = DeadeyeClient::new(bulk_provider);
    let bulk = BulkReader::new(bulk_client);
    let market_queries: Vec<(Family, Felt)> =
        (0..5).map(|_| (Family::Normal, market_addr)).collect();
    let dist_results = bulk.distributions(&market_queries).await;
    let dist_ok = dist_results.iter().filter(|r| r.is_ok()).count();
    eprintln!("  ✅ bulk distributions: {dist_ok}/5 ok");
    assert_eq!(
        dist_ok, 5,
        "expected all 5 bulk-distribution reads to succeed (got {dist_ok}/5)"
    );

    let trader_queries: Vec<(Family, Felt, Felt)> = (0..5)
        .map(|_| (Family::Normal, market_addr, trader))
        .collect();
    let pos_results = bulk.positions(&trader_queries).await;
    let pos_ok = pos_results.iter().filter(|r| r.is_ok()).count();
    eprintln!("  ✅ bulk positions: {pos_ok}/5 ok");
    assert_eq!(
        pos_ok, 5,
        "expected all 5 bulk-position reads to succeed (got {pos_ok}/5)"
    );

    eprintln!("✅ sepolia_read_only_smoke PASSED");
}
