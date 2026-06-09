#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration smoke test — printing aids debugging, unwrap + panic mark hard \
              invariants on a read-only health check"
)]

//! # Mainnet read-only smoke
//!
//! Real mainnet behaves differently from devnet: real latency, real
//! failure modes, and no admin-via-`only_owner` shortcuts. This test
//! exercises every reader path against a live mainnet full-node so any
//! wire-format / RPC-compatibility regression surfaces in CI before a
//! paying customer trips on it.
//!
//! ## What it does
//!
//! 1. Connects a [`JsonRpcClient`] to the configured mainnet RPC.
//! 2. Reads + asserts the chain id matches mainnet (`SN_MAIN`).
//! 3. For a known-live normal market:
//!    - reads the distribution
//!    - reads the AMM params
//!    - reads the LP info
//!    - reads a known trader's position
//!    - runs the **offline** client-side `quote_candidate_offline` (no math
//!      runtime) for a no-op candidate and asserts it produces a well-formed
//!      quote — `on_chain_will_accept` plus an optional *typed*
//!      [`deadeye_starknet::TradeRejectionReason`] — and reports the σ-floor.
//! 4. Confirms [`BulkReader`] works against mainnet: parallel distribution
//!    reads for 5 markets, parallel position reads for 5 traders.
//!
//! ## What it does NOT do
//!
//! This test is **strictly read-only**. No transactions are submitted.
//! The quote path is fully client-side (no RPC writes, no runtime).
//!
//! ## How to run locally
//!
//! ```bash
//! export DEADEYE_RUN_MAINNET=1
//! export DEADEYE_MAINNET_RPC="https://api.zan.top/public/starknet-mainnet/rpc/v0_10"
//! # A known-live normal-family market. The deadeye indexer at
//! # https://178-105-210-177.sslip.io/api/markets is the canonical
//! # source — pick any market with `marketType: "normal"`.
//! export DEADEYE_MAINNET_MARKET_ADDR=0x…
//! # Optional: a trader address known to hold a position on the above
//! # market. Defaults to `0x0` (= "no live position", which is fine —
//! # the reader path is the contract; reading a position for an
//! # account that hasn't traded returns the zero position).
//! export DEADEYE_MAINNET_TRADER_ADDR=0x…
//! cargo test -p deadeye-e2e --test mainnet_smoke -- --ignored --nocapture
//! ```
//!
//! ## Gating
//!
//! Without `DEADEYE_RUN_MAINNET=1` the test logs a `skip:` line and
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

/// Canonical Starknet mainnet chain id (`0x534e5f4d41494e`).
///
/// Computed from the cairo short string `"SN_MAIN"`. Verified against
/// the live `starknet_chainId` RPC response.
const MAINNET_CHAIN_ID_HEX: &str = "0x534e5f4d41494e";

/// Default public mainnet RPC endpoint. Used when
/// `DEADEYE_MAINNET_RPC` is unset.
const DEFAULT_MAINNET_RPC: &str = "https://api.zan.top/public/starknet-mainnet/rpc/v0_10";

fn run_enabled() -> bool {
    env::var("DEADEYE_RUN_MAINNET").is_ok()
}

fn mainnet_rpc() -> String {
    env::var("DEADEYE_MAINNET_RPC").unwrap_or_else(|_| DEFAULT_MAINNET_RPC.to_owned())
}

fn mainnet_market() -> Option<Felt> {
    env::var("DEADEYE_MAINNET_MARKET_ADDR")
        .ok()
        .and_then(|s| Felt::from_hex(&s).ok())
}

fn mainnet_trader() -> Option<Felt> {
    env::var("DEADEYE_MAINNET_TRADER_ADDR")
        .ok()
        .and_then(|s| Felt::from_hex(&s).ok())
}

/// Top-level smoke. Reads the chain id, a known market's state, a known
/// trader's position, and an offline (client-side) quote. Then fans out
/// a small [`BulkReader`] batch to confirm concurrency works against the
/// live endpoint.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires mainnet access; uses DEADEYE_RUN_MAINNET env var. \
            See module docs for required env vars."]
async fn mainnet_read_only_smoke() {
    if !run_enabled() {
        eprintln!("skip: set DEADEYE_RUN_MAINNET=1 to enable");
        return;
    }
    let rpc_url = mainnet_rpc();
    eprintln!("▶ mainnet smoke against {rpc_url}");
    let url = Url::parse(&rpc_url).expect("DEADEYE_MAINNET_RPC must be a valid URL");
    let rpc = JsonRpcClient::new(HttpTransport::new(url));

    // 1. Chain id matches mainnet.
    let chain_id = rpc.chain_id().await.expect("starknet_chainId");
    let expected = Felt::from_hex(MAINNET_CHAIN_ID_HEX).expect("constant felt parses");
    assert_eq!(
        chain_id, expected,
        "expected mainnet chain id {expected:#x}, got {chain_id:#x}",
    );
    eprintln!("✅ chain id = {chain_id:#x} (SN_MAIN)");

    // Block number sanity check — the node is alive and ahead of genesis.
    let block_id = BlockId::Tag(starknet_core::types::BlockTag::Latest);
    let block_number = rpc.block_number().await.expect("block_number");
    assert!(block_number > 0, "mainnet block_number must be > 0");
    eprintln!("✅ block_number = {block_number} (block_id={block_id:?})");

    // 2. Known-live normal market reads.
    let Some(market_addr) = mainnet_market() else {
        eprintln!(
            "⚠️  DEADEYE_MAINNET_MARKET_ADDR unset — skipping market / position / quote / bulk paths."
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
        .expect("mainnet normal market: distribution() must read");
    eprintln!(
        "  ✅ distribution: μ = {mean:.6}, σ = {sigma:.6}",
        mean = dist.mean().to_f64(),
        sigma = dist.sigma().to_f64(),
    );

    // 2b. get_params.
    let params: AmmParamsRaw = reader
        .params()
        .await
        .expect("mainnet normal market: params() must read");
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
        .expect("mainnet normal market: lp_info() must read");
    eprintln!(
        "  ✅ lp_info: total_shares={shares:.4}, backing_deposited={bd:.4}",
        shares = deadeye_core::Sq128::from_raw(lp_info.total_shares).to_f64(),
        bd = deadeye_core::Sq128::from_raw(lp_info.total_backing_deposited).to_f64(),
    );

    // 2d. Read a trader's position. Falls back to address-zero — the
    // reader contract returns a well-formed empty position when the
    // trader has never touched the market. We're testing the wire,
    // not the position.
    let trader = mainnet_trader().unwrap_or(Felt::ZERO);
    eprintln!("▶ reading position for trader {trader:#x}");
    let position = reader
        .position(trader)
        .await
        .expect("mainnet normal market: position() must read");
    eprintln!(
        "  ✅ position: total_collateral={tc:.6}",
        tc = deadeye_core::Sq128::from_raw(position.total_collateral).to_f64(),
    );

    // 2e. Offline (client-side) quote — candidate = current distribution
    // (a no-op shift). This reproduces the AMM's hint + σ-floor math
    // entirely off-chain: no math runtime, no write RPCs. The quote must
    // be well-formed; `on_chain_will_accept` plus an optional *typed*
    // rejection reason are both acceptable outcomes — what we guard
    // against is the read/compute path panicking or erroring.
    let client = DeadeyeClient::new(JsonRpcProvider::new(JsonRpcClient::new(
        HttpTransport::new(Url::parse(&rpc_url).expect("URL parses")),
    )));
    let market = client.normal_market(market_addr);
    let cand_mean = dist.mean().to_f64();
    let cand_var = dist.sigma().to_f64().powi(2);
    let quote = market
        .quote_candidate_offline(cand_mean, cand_var)
        .await
        .expect("offline quote must compute (no runtime needed)");
    eprintln!(
        "  ✅ offline quote: on_chain_will_accept={accept}, rejection={rej:?}, \
         required_collateral={rc:.6}",
        accept = quote.on_chain_will_accept,
        rej = quote.rejection,
        rc = deadeye_core::Sq128::from_raw(quote.required_collateral).to_f64(),
    );
    let sigma_floor = market.sigma_floor().await.expect("σ-floor must compute");
    assert!(
        sigma_floor.is_finite() && sigma_floor >= 0.0,
        "σ-floor must be a finite, non-negative value (got {sigma_floor})"
    );
    eprintln!("  ✅ σ-floor = {sigma_floor:.6}");

    // 3. BulkReader sanity. We need 5 markets + 5 traders for the
    // "5 markets, 5 traders, parallel" check. Without a curated list we
    // read the same market / trader N times — the goal is the
    // *concurrency* path, not 5 distinct payloads.
    eprintln!("▶ BulkReader: 5 markets × distribution + 5 traders × position");
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

    eprintln!("✅ mainnet_read_only_smoke PASSED");
}
