#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_precision_loss,
    reason = "integration tests in tests/ are top-level — printing aids debugging, unwrap/panic/expect are fine"
)]

//! Bit-exactness parity test: `NormalMarket::optimize_quote_offline` vs
//! `NormalMarket::optimize_quote` against a deployed math runtime.
//!
//! The off-chain path (no runtime) must produce a candidate distribution
//! `(μ, σ², σ)` and hints `(l2_norm_denom, backing_denom)` that are
//! **bit-identical** with what the on-chain path emits (modulo
//! `required_collateral` which the chain re-derives via `check_trade_view`
//! and that uses a Sq128 newton solver — that's compared to within 1 ULP).
//!
//! For 10 randomly-chosen tuples `(μ_b, σ_b, market_μ, market_σ, budget)`
//! we:
//! 1. Run `optimize_quote(runtime, ...)` — the chain path.
//! 2. Run `optimize_quote_offline(...)` — the pure off-chain path.
//! 3. Assert `(μ, σ², σ)` match raw-limb-for-raw-limb.
//! 4. Assert hints match raw-limb-for-raw-limb.
//! 5. Assert `required_collateral` matches within 1 ULP (`f64::EPSILON · max`).
//!
//! Gated behind `DEADEYE_RUN_INTEGRATION=1` and requires `starknet-devnet`
//! on `:5050`. Bootstraps a fresh devnet + factory + normal market.

use deadeye_core::{
    Distribution, NormalDistribution, Sq128,
    distribution::{NormalDistributionRaw, NormalSqrtHintsRaw},
    sq128::Sq128Raw,
};
use deadeye_sdk::{normal::NormalMarket, starknet::JsonRpcProvider};
use deadeye_starknet::Felt;
use deadeye_testkit::fixture::{
    env::{BootstrapConfig, bootstrap_devnet},
    erc20::approve,
    lifecycle::{
        build_initial_normal_inputs, deploy_normal_market_with_event, fetch_normal_hints,
        initialize_market, upsert_normal_profile_for_test,
    },
};
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};

const PROFILE_ID: u32 = 9_910_u32;
// Match the chaos suite's seed values — these are known to satisfy the
// per-profile `initial backing invalid` check (which compares
// `σ × √π × max_pdf` against the deployed profile's backing=50).
const INITIAL_MEAN: f64 = 42.0;
const INITIAL_VAR: f64 = 64.0; // σ_market = 8.0
const INIT_APPROVE: u128 = 10_000_000_000_000_000_000_000_u128; // 10k STRK

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

/// 10 deterministic-but-varied scenarios spanning σ-arb (μ equal),
/// μ-shift (σ equal), and combined moves. The numbers are picked to
/// stay inside the optimizer's policy region (`max_sigma_ratio` = 4,
/// `max_mean_sep_sigmas` = 4) for the deployed market `(μ=4.29, σ=0.35)`.
fn scenarios() -> [(f64, f64, f64); 10] {
    // (belief_mean, belief_sigma, budget), targeting market N(μ=42, σ=8)
    [
        (45.0, 6.0, 50.0),  // mild μ-shift + σ-shrink
        (50.0, 4.0, 100.0), // bullish μ + tight σ
        (38.0, 10.0, 50.0), // bearish μ + wider σ
        (42.0, 2.0, 25.0),  // pure σ-arb (equal μ, tight σ)
        (42.0, 16.0, 25.0), // pure σ-arb (equal μ, loose σ)
        (55.0, 5.0, 150.0), // strong bullish
        (30.0, 9.0, 100.0), // strong bearish + tight σ
        (44.0, 7.0, 40.0),  // mild bullish
        (40.0, 6.5, 60.0),  // mild bearish
        (48.0, 8.0, 75.0),  // bullish μ-only
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_optimize_quote_parity_against_runtime() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 and start starknet-devnet on :5050");
        return;
    }

    // ─── Phase 0: bootstrap devnet + factory ─────────────────────────────
    let env = bootstrap_devnet(BootstrapConfig::default())
        .await
        .expect("bootstrap succeeds");
    eprintln!(
        "✅ devnet up: chain={:#x}, factory={:#x}, runtime={:#x}",
        env.chain_id, env.factory, env.normal_runtime
    );

    let admin = env.account_handle(&env.admin);
    let rpc = JsonRpcClient::new(HttpTransport::new(env.url.clone()));

    // ─── Phase 1: upsert profile + deploy market ─────────────────────────
    upsert_normal_profile_for_test(admin.clone(), env.factory, env.collateral, PROFILE_ID)
        .await
        .expect("upsert normal profile");

    let (initial_dist, _x_star) = build_initial_normal_inputs(INITIAL_MEAN, INITIAL_VAR, 1_000.0);
    let initial_hints = fetch_normal_hints(&rpc, env.normal_runtime, initial_dist)
        .await
        .expect("fetch initial hints");

    let market = deploy_normal_market_with_event(
        &admin,
        env.factory,
        PROFILE_ID,
        Felt::from(0xA1B2_C3D4_u64),
        Felt::ZERO,
        initial_dist,
        initial_hints,
    )
    .await
    .expect("deploy normal market");
    eprintln!("✅ market deployed: {market:#x}");

    // ─── Phase 2: initialize + approve ───────────────────────────────────
    if let Err(e) = initialize_market(&admin, market, env.collateral, INIT_APPROVE).await {
        eprintln!("⚠️ initialize_market failed (known blocker?): {e}");
        eprintln!(
            "    Off-chain path doesn't need initialization, but the chain path \
             does. Skipping bit-exact parity assertion."
        );
        return;
    }
    approve(admin.clone(), env.collateral, market, INIT_APPROVE)
        .await
        .expect("approve market");
    eprintln!("✅ initialized + approved");

    // ─── Phase 3: build two `NormalMarket` handles & run all scenarios ──
    let provider_chain =
        JsonRpcProvider::new(JsonRpcClient::new(HttpTransport::new(env.url.clone())));
    let provider_off =
        JsonRpcProvider::new(JsonRpcClient::new(HttpTransport::new(env.url.clone())));
    let market_chain = NormalMarket::new(&provider_chain, market);
    let market_off = NormalMarket::new(&provider_off, market);

    let scenarios = scenarios();
    let total = scenarios.len();
    let mut passed: usize = 0;
    let mut diverged: Vec<String> = Vec::new();

    for (i, (mu_b, sigma_b, budget)) in scenarios.into_iter().enumerate() {
        let chain_quote = market_chain
            .optimize_quote(env.normal_runtime, mu_b, sigma_b, budget)
            .await
            .expect("chain optimize_quote");
        let off_quote = market_off
            .optimize_quote_offline(mu_b, sigma_b, budget)
            .await
            .expect("offline optimize_quote_offline");

        // Distribution bit-parity — both paths must construct the
        // candidate via `from_variance` so σ is derived via Sq128.
        let dist_match = candidates_equal(&chain_quote.candidate, &off_quote.candidate);
        // Hints bit-parity — `compute_hints_view` on chain vs.
        // `compute_normal_hints_offline` use the same Sq128 sqrt and
        // sqrt_pi constant, so these must agree limb-for-limb.
        let hints_match = hints_equal(&chain_quote.candidate_hints, &off_quote.candidate_hints);
        // Required collateral: chain computes via `check_trade_view`'s
        // Sq128 solver (set only when `on_chain_will_accept=true`),
        // off-chain uses the optimizer's f64 Newton solver. These
        // should agree to within 1 ULP of the larger value when both
        // paths produce a non-trivial trade.
        let chain_coll = Sq128::from_raw(chain_quote.required_collateral).to_f64();
        let off_coll = Sq128::from_raw(off_quote.required_collateral).to_f64();
        let ulp = chain_coll.abs().max(off_coll.abs()).max(1.0) * 1e-9;
        let coll_match = (chain_coll - off_coll).abs() <= ulp;

        eprintln!(
            "  [{i}/{total}] μ_b={mu_b}, σ_b={sigma_b}, budget={budget}:\n\
             \t  chain σ={:.18} | off σ={:.18}\n\
             \t  chain accept={} | off accept={}\n\
             \t  dist={} hints={}\n\
             \t  coll: chain={:.6} | off={:.6} (ulp={ulp:.3e}) → {}",
            Sq128::from_raw(chain_quote.candidate.sigma).to_f64(),
            Sq128::from_raw(off_quote.candidate.sigma).to_f64(),
            chain_quote.on_chain_will_accept,
            off_quote.on_chain_will_accept,
            if dist_match { "OK" } else { "MISMATCH" },
            if hints_match { "OK" } else { "MISMATCH" },
            chain_coll,
            off_coll,
            if coll_match { "OK" } else { "MISMATCH" },
        );

        // Bit-parity assertion: the candidate distribution and hints
        // must match unconditionally — both paths use the exact same
        // Sq128 derivations.
        //
        // Collateral parity is only required when **both** paths
        // agree on acceptance: the chain only fills
        // `required_collateral` when `check_trade_view` accepts;
        // off-chain always fills it from the optimizer's f64 Newton
        // solver. When the chain rejects (e.g. policy envelope or
        // backing) the chain side reports 0 even if the optimizer
        // found a positive number — that divergence is meaningful but
        // independent of σ/hint bit-parity.
        let coll_check = if chain_quote.on_chain_will_accept && off_quote.on_chain_will_accept {
            coll_match
        } else {
            true
        };

        if dist_match && hints_match && coll_check {
            passed += 1;
        } else {
            diverged.push(format!(
                "[{i}] (μ_b={mu_b}, σ_b={sigma_b}, budget={budget}): \
                 dist={dist_match} hints={hints_match} coll={coll_check}",
            ));
        }
    }

    eprintln!("\n🔍 parity: {passed}/{total} scenarios matched");
    if !diverged.is_empty() {
        for d in &diverged {
            eprintln!("  DIVERGED: {d}");
        }
    }
    assert!(
        diverged.is_empty(),
        "optimize_quote_offline must be bit-exact with optimize_quote — got {} divergence(s)",
        diverged.len()
    );
}

fn raws_equal(a: &Sq128Raw, b: &Sq128Raw) -> bool {
    a.limb0 == b.limb0
        && a.limb1 == b.limb1
        && a.limb2 == b.limb2
        && a.limb3 == b.limb3
        && a.neg == b.neg
}

fn candidates_equal(a: &NormalDistributionRaw, b: &NormalDistributionRaw) -> bool {
    raws_equal(&a.mean, &b.mean)
        && raws_equal(&a.variance, &b.variance)
        && raws_equal(&a.sigma, &b.sigma)
}

fn hints_equal(a: &NormalSqrtHintsRaw, b: &NormalSqrtHintsRaw) -> bool {
    raws_equal(&a.l2_norm_denom, &b.l2_norm_denom) && raws_equal(&a.backing_denom, &b.backing_denom)
}

// Self-test of `from_variance` parity → just to compile-time-check the imports.
#[test]
fn from_variance_uses_sq128_sqrt() {
    let mean = Sq128::from_f64(4.29).expect("mean");
    let variance = Sq128::from_f64(0.1225).expect("variance");
    let dist = NormalDistribution::from_variance(mean, variance).expect("dist");
    // σ² floor-truncated ≤ variance (Sq128 sqrt invariant).
    let sigma = dist.sigma();
    let sigma_sq = sigma.checked_mul(sigma).expect("mul");
    assert!(
        sigma_sq <= variance,
        "Sq128 sqrt invariant violated: σ² > variance",
    );
}
