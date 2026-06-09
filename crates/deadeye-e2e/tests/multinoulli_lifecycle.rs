#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    reason = "integration tests in tests/ are top-level — printing aids debugging, unwrap is OK"
)]

//! Phase 2 e2e: multinoulli read paths + solver against live mainnet.
//!
//! Full deploy/trade lifecycle for multinoulli markets lands in Phase 4
//! once the Factory wrapper is in.

use deadeye_collateral::categorical_collateral;
use deadeye_core::CategoricalDistribution;
use deadeye_indexer::IndexerClient;
use deadeye_sdk::{DeadeyeClient, starknet::JsonRpcProvider};
use deadeye_testkit::default_mainnet_rpc;
use starknet_core::types::Felt;
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

/// Resolves a live multinoulli market via the indexer.
async fn pick_multinoulli_market() -> Option<(Felt, f64)> {
    let client = IndexerClient::mainnet().ok()?;
    let markets = client.markets().await.ok()?;
    for m in markets {
        if m.market_type != "multinoulli" {
            continue;
        }
        let Some(state) = m.multinoulli_state else {
            continue;
        };
        if !state.is_initialised || state.is_settled {
            continue;
        }
        let address = Felt::from_hex(&m.address).ok()?;
        let k = state.k.unwrap_or(1.0);
        return Some((address, k));
    }
    None
}

#[tokio::test]
async fn mainnet_multinoulli_read_passthrough() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 to enable");
        return;
    }

    let Some((address, k)) = pick_multinoulli_market().await else {
        eprintln!("skip: no live multinoulli market found on mainnet");
        return;
    };
    eprintln!("picked multinoulli market: {address:#x} k={k}");

    let url = default_mainnet_rpc();
    let rpc = JsonRpcClient::new(HttpTransport::new(url));
    let provider = JsonRpcProvider::new(rpc);
    let client = DeadeyeClient::new(provider);
    let market = client.multinoulli_market(address);

    let dist = market.distribution().await.expect("distribution reads");
    eprintln!(
        "current dist: n={}, probs={:?}",
        dist.outcome_count(),
        dist.probs()
    );
    assert!(dist.outcome_count() >= 2);
    let dist_sum: f64 = dist.probs().iter().sum();
    assert!((dist_sum - 1.0).abs() < 1e-6, "Σp = {dist_sum}");

    let status = market.reader().market_status().await.expect("status reads");
    eprintln!(
        "status: init={}, paused={}, settled={}",
        status.is_initialised, status.is_paused, status.is_settled
    );
    assert!(status.is_initialised);

    // Off-chain solver round-trip: a small perturbation should yield
    // non-negative collateral.
    let perturbed: Vec<f64> = dist
        .probs()
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            if i == 0 {
                (p + 0.01).max(0.0)
            } else {
                (p - 0.01 / (dist.outcome_count() as f64 - 1.0)).max(0.0)
            }
        })
        .collect();
    // Re-normalise to absorb any clamp drift.
    let perturbed_sum: f64 = perturbed.iter().sum();
    let candidate_probs: Vec<f64> = perturbed.iter().map(|p| p / perturbed_sum).collect();
    let candidate = CategoricalDistribution::from_probs(candidate_probs).unwrap();
    let result = categorical_collateral(&dist, &candidate, k).expect("solver runs");
    eprintln!(
        "quote: outcome={}, lambda_f={}, lambda_g={}, collateral={}",
        result.min_outcome_index, result.lambda_f, result.lambda_g, result.collateral
    );
    assert!(result.collateral >= 0.0);
}
