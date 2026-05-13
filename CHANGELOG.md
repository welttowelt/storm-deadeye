# Changelog

All notable changes to deadeye-rs are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
