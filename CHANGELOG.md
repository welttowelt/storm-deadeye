# Changelog

All notable changes to deadeye-rs are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added — deadeye-sdk v0.1.1

- `NormalMarket::optimize_quote_offline(belief_mean, belief_sigma, budget_xp)`
  — chain-bit-exact off-chain EV optimizer for the **no-math-runtime**
  preflight path. Reads live `(distribution, params, lp_info)`, derives
  σ via [`Sq128::sqrt`] (matches `sqrt_verified` 20/20 on devnet), and
  emits hints via the same `sqrt(mul_down(...))` chain the on-chain
  `compute_hints_view` runs. The output `NormalTradeQuote` survives the
  on-chain `INVALID_DISTRIBUTION` / `INVALID_HINTS` checks by construction.
  See [`docs/OFFLINE_PREFLIGHT.md`](docs/OFFLINE_PREFLIGHT.md).
- Integration test `deadeye-e2e/tests/offline_optimize_quote_parity.rs`
  — runs both `optimize_quote` and `optimize_quote_offline` against a
  freshly-bootstrapped devnet market and asserts limb-for-limb agreement
  on the candidate distribution and hints (10/10).

### Changed — deadeye-sdk v0.1.1

- `NormalMarket::optimize_quote` now constructs the candidate via
  `NormalDistribution::from_variance` (instead of `from_sigma`), so the
  candidate σ is **Sq128-derived** instead of f64-derived. Internal
  behaviour change only — the public API is unchanged. This brings the
  chain-preflight path into bit-parity with the new offline path.

### Added



- Initial workspace scaffold:
  - `deadeye-core` — signed Q128.128 fixed-point, distribution traits
    (`Distribution`, `NormalDistribution`, `LognormalDistribution`),
    typed `CoreError`.
  - `deadeye-artifacts` — compile-time-embedded contract ABIs and
    release manifest, with optional `serde_json`-backed typed view.
  - `deadeye-collateral` — `l2_norm`, `lambda`, and a damped
    Newton-Raphson collateral solver with `MinimizationPolicy`.
  - `deadeye-starknet` — `CairoSerde` trait + concrete impls,
    `Provider` abstraction with a `starknet-providers`-backed adapter,
    pre-computed entry-point selectors, `NormalMarketReader` view client.
  - `deadeye-sdk` — `DeadeyeClient`, per-market handles, `PreparedQuote`.
  - `deadeye-indexer` — typed HTTP client for the production indexer
    (`situation-indexer.fly.dev`), with `health()`, `markets()`, and
    per-market detail accessors.
  - `deadeye-testkit` — devnet lifecycle helpers, Cartridge RPC
    discovery, integration `Harness`.
  - `deadeye-e2e` — read-only end-to-end tests, opt-in via
    `DEADEYE_RUN_INTEGRATION=1`.
  - `xtask` — workspace task runner.
- Pedantic lint posture: `clippy::all + pedantic + nursery` plus curated
  `clippy::restriction` lints; `unsafe_code = forbid`.
- GitHub Actions CI for fmt, clippy, test, docs, MSRV, and
  `cargo-deny` checks; weekly integration workflow against Cartridge
  Sepolia and starknet-devnet-rs.
