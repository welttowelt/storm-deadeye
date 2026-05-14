# deadeye-rs

A Rust SDK for the Deadeye prediction-market protocol on Starknet, built for
**market makers**: low latency, small dependency surface, no hidden async.

## Crates

| Crate                | Purpose                                                                         |
| -------------------- | ------------------------------------------------------------------------------- |
| `deadeye-core`       | Signed Q128.128 fixed-point, distribution types, error hierarchy. `no_std`-friendly. |
| `deadeye-artifacts`  | Compile-time-embedded contract ABIs and release manifest.                       |
| `deadeye-collateral` | Off-chain collateral solver (L2 norm, lambda, Newton-Raphson minimiser).        |
| `deadeye-starknet`   | Calldata encoders, entry-point selectors, view-call wrappers over `starknet-rs`.|
| `deadeye-sdk`        | High-level façade: client, quote, per-market handles.                           |
| `deadeye-indexer`    | Typed HTTP client for the production indexer (`situation-indexer.fly.dev`).     |
| `deadeye-testkit`    | Integration-test helpers (devnet, Cartridge RPC, harness). Unpublished.         |
| `deadeye-e2e`        | End-to-end tests against a live RPC. Unpublished.                               |
| `xtask`              | Workspace task runner (`cargo xtask ci`, `cargo xtask devnet-up`). Unpublished. |

Each layer is independently usable. A latency-critical MM can drive
`deadeye-starknet` directly; the SDK is convenience, not a wall.

## Design goals

1. **Pedantic from day one.** The workspace runs `clippy::all + pedantic + nursery`
   plus a curated set of `clippy::restriction` lints. `unsafe_code` is
   `forbid`. `#[expect]` everywhere — never bare `#[allow]`.
2. **Small dependency surface.** No `reqwest` in the hot path. No `serde` in
   `deadeye-core`. The `starknet-providers` crate is feature-gated so
   custom transports (multi-RPC racers, mocks) cost nothing.
3. **Numerics that match the chain bit-for-bit.** `Sq128Raw` round-trips
   bit-identical with the Cairo `SQ128x128Raw` struct, verified by proptest.
4. **`no_std`-capable core.** `deadeye-core` and `deadeye-artifacts` can
   compile without `std` so the same primitives feed bots, indexers, and
   on-chain verifier tooling.

## Quick start

```rust
use deadeye_sdk::{
    DeadeyeClient,
    collateral::MinimizationPolicy,
    core::{Distribution, NormalDistribution, Sq128},
    starknet::JsonRpcProvider,
};
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
use url::Url;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rpc = JsonRpcClient::new(HttpTransport::new(
        Url::parse("https://api.cartridge.gg/x/starknet/sepolia")?,
    ));
    let client = DeadeyeClient::new(JsonRpcProvider::new(rpc));

    let market = client.normal_market("0xMARKET_ADDRESS".parse()?);

    let current = market.distribution().await?;
    println!("current mean = {}", current.mean().to_f64());

    let candidate = NormalDistribution::from_variance(
        Sq128::from_f64(105.0)?,
        Sq128::from_f64(4.0)?,
    )?;

    let quote = market
        .prepare_quote(candidate, MinimizationPolicy::standard())
        .await?;
    println!("collateral = {}", quote.collateral);

    Ok(())
}
```

## Normal-market: chain vs. offline preflight

The `NormalMarket` handle ships **two** preflight entry points, chosen by
whether a math-runtime contract instance is deployed on your target
network:

| Method | When to use | Chain round-trips | Guarantees |
| --- | --- | --- | --- |
| `optimize_quote(runtime, μ_b, σ_b, budget)` | A math-runtime instance is deployed (devnet, Sepolia, or self-hosted). | 4 view calls (`distribution`, `params`, `compute_hints_view × 2`, `check_trade_view`) | Chain-validated: `on_chain_will_accept` reflects `check_trade_view`'s verdict. |
| `optimize_quote_offline(μ_b, σ_b, budget)` | **Mainnet today** — the normal AMM uses library dispatch (class hash) with no separate runtime contract. | 3 view calls (`distribution`, `params`, `lp_info`) — no math-runtime hops. | σ + hints are **bit-exact** with what the chain would derive (`Sq128::sqrt` matches `sqrt_verified` 20/20 on devnet; see [`docs/SQ128_SQRT.md`](docs/SQ128_SQRT.md)). |

The offline path eliminates `INVALID_DISTRIBUTION` and `INVALID_HINTS`
rejections by construction. The chain still re-verifies the trade on
submit (balance, nonce, policy envelope) — but the σ/hint precision
footgun is gone.

```rust
let market = client.normal_market(market_addr);
let quote = market
    .optimize_quote_offline(belief_mean, belief_sigma, budget_xp)
    .await?;
if quote.on_chain_will_accept {
    // hand to a signed handle for execute_quote()
}
```

Parity test: `deadeye-e2e/tests/offline_optimize_quote_parity.rs`
(gated on `DEADEYE_RUN_INTEGRATION=1`) — runs both paths against a
deployed runtime and asserts limb-for-limb agreement on
`(μ_g, σ_g, σ_g²)` and `(l2_norm_denom, backing_denom)`.

## Development

```bash
# Run the full local CI pipeline (fmt + clippy + tests)
cargo xtask ci

# Check workspace MSRV
cargo check --workspace --all-features

# Run unit tests
cargo test --workspace --all-features --lib --bins

# Run integration tests against a local devnet
DEADEYE_RUN_INTEGRATION=1 cargo test -p deadeye-e2e -- --nocapture

# Run integration tests against Cartridge Sepolia
DEADEYE_RUN_INTEGRATION=1 DEADEYE_TEST_TARGET=cartridge \
  cargo test -p deadeye-e2e -- --nocapture

# Smoke-test the live Sepolia indexer (situation-indexer.fly.dev)
DEADEYE_RUN_INTEGRATION=1 cargo test -p deadeye-e2e --test indexer_smoke -- --nocapture
```

## MSRV

Rust 1.92 (the workspace uses `resolver = "3"` and `edition = "2024"`).

## License

Dual-licensed under MIT or Apache-2.0.
