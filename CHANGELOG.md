# Changelog

All notable changes to deadeye-rs are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added — deadeye-starknet v0.1.1

- `CollateralTokenReader` / `CollateralTokenWriter` — typed view + write
  client pair for the deployed `restricted_collateral_token` (XP on
  Deadeye mainnet, SPICE on sepolia). Reader exposes `balance_of`,
  `allowance`, `total_supply`, `initial_grant`,
  `has_claimed_initial_grant`, `is_market_registered`,
  `is_market_enabled`; writer pairs it with an `Account` and exposes
  `claim_initial_grant`, `approve`, plus `build_*_call` builders for
  multicall composition. Mirrors the existing `NormalMarketReader` /
  `NormalMarketWriter` shape so callers don't need to learn a second
  pattern.
- `MAINNET_XP_TOKEN_ADDRESS` — `Felt` constant pinned to
  `0x01d77ce77f1d86035c5e27444da7d2fc77de1d384326074f60f973fa0dd80aff`
  (read off `deployment-mainnet.json`).
- `U256Value` — `CairoSerde`-implementing newtype around
  `starknet_core::types::U256` so `core::integer::u256` ABI returns
  decode through the same trait pipeline as every other view call.
- Constructor shorthand: `CollateralTokenReader::mainnet_xp(provider)`
  binds to the mainnet XP token without the operator typing the hex
  felt.
- 5 unit tests pin the mainnet address constant, the `u256` round-trip,
  the `approve(spender, u256)` calldata layout (`spender, low, high`),
  and selector stability / distinctness.

### Added — deadeye-sdk v0.1.5

- Transitively re-exports the new `CollateralTokenReader`,
  `CollateralTokenWriter`, `MAINNET_XP_TOKEN_ADDRESS`, and `U256Value`
  through `deadeye_sdk::starknet::*` (no SDK-side wrapper needed — the
  crate already does `pub use deadeye_starknet as starknet`). Pairs the
  Deadeye AMMs with their underlying collateral surface in one import.
- Bumps the `deadeye-starknet` dep to `0.1.1`.

### Added — deadeye-cli v0.1.4

- `deadeye collateral claim-grant` — calls `claim_initial_grant()` on
  the XP token, minting the fixed grant to the configured wallet.
  Idempotent: reads `has_claimed_initial_grant` up front and skips the
  submit on an already-funded wallet. Dry-run by default; `--execute`
  to submit. Honors `--token` for non-mainnet deploys
  (e.g. sepolia's SPICE) and falls back to `MAINNET_XP_TOKEN_ADDRESS`.
- `deadeye collateral balance` — prints the wallet's XP balance, the
  `initial_grant` amount, and whether the grant has been claimed.
  Read-only.
- Fails loud when the signer's address doesn't match the resolved
  `--address` / `DEADEYE_ADDRESS` (since `claim_initial_grant` mints
  to the caller, a mismatch would burn gas claiming to the wrong
  wallet).

### Fixed — deadeye-sdk v0.1.4

- `NormalMarket::optimize_quote` (chain-runtime variant) now derives
  `x_star` from the audited `normal_collateral` solver and supplies the
  chain-scaled `λ_f · f(x*) − λ_g · g(x*)` collateral — matching the
  FU2 fix to `optimize_quote_offline`. Previously the runtime variant
  passed `x_star = cand_mean`, which silently tripped
  `stationary_valid` on the deployed math runtime (visible only on
  devnet / when `DEADEYE_NORMAL_RUNTIME_ADDR` is set). The two inner
  paths now run byte-identical math up to the chain hand-off: same
  `optimize_normal_trade` call, same `Sq128::from_f64` conversions,
  same `normal_collateral(..., MinimizationPolicy::standard())` solver,
  same λ-scaled collateral formula, same `(cand_mean, 0.0)` no-trade
  fallback. The two **outputs** agree on `x_star` byte-for-byte; the
  returned `required_collateral` differs only by the chain's Sq128
  re-computation (`check.verification.computed_collateral`) versus the
  offline f64 round-trip — bit-equal to within 1 ULP per
  `offline_optimize_quote_parity.rs`. Hints differ by construction
  (chain bytes via `compute_hints_view` vs Sq128 mirror via
  `compute_normal_hints_offline`).

### Added — deadeye-sdk v0.1.3

- `NormalMarket::optimize_quote_with_override(runtime, belief_mean,
  belief_sigma, budget, effective_k_override)` — chain-faithful
  preflight variant that accepts a caller-supplied `effective_k`
  instead of re-reading it from `params.k`. Use this when the caller
  already knows `effective_k` (backtest replay, simulation sweep, bot
  offline mode, unit tests). Non-positive / non-finite values are
  rejected with `CoreError::InvalidInput` before any chain I/O.
- `NormalMarket::optimize_quote_offline_with_override(belief_mean,
  belief_sigma, budget_xp, effective_k_override)` — offline twin of
  the above. Eliminates the `params` + `lp_info` reads (~150ms saved
  per quote on indexer cache miss). All chain-bit-exact behavior
  (Sq128-derived σ, λ-scaled collateral, `sqrt(mul_down(...))` hints)
  is preserved — only the `k`-derivation step is short-circuited.
- Internal `optimize_quote_offline_inner` extracted as a free function
  so the unit-test path can exercise the math without standing up a
  `Provider` mock. 8 new normal-module tests covering override
  validation, determinism, and `k`-responsiveness.
- `BacktestEngine::from_journal(path)` — real implementation
  (previously a stub returning `io::Error::other`). Reads
  newline-delimited `JournalEntry` records from disk, converts each
  to a `MarketEvent`, and seeds `initial_state` from the first
  Normal trade (falling back to N(0, 1)). Per the journal's
  permissive contract, corrupted lines are emitted as
  `tracing::warn` and skipped; pre-submission "skipped (...)" rows
  are filtered out so the replay sees only events that reached the
  chain. The cpi-bot's in-crate `entries_to_events` workaround in
  `analytics::cmd_replay` becomes redundant and can delegate (P2).
  6 new tests cover Trade/Sell/Claim entries, corrupted-line
  recovery, empty file, missing path, and skipped-row filtering.

### Fixed — deadeye-sdk v0.1.3

- `live_effective_k` doc-comment in `normal.rs` had `pool_backing`
  and `initial_backing` swapped (claimed `pool := params.backing`,
  `initial := lp_info.total_backing_deposited`). The function body
  was always correct; only the comment lied. Per `REVIEW_ITEM5`
  (Cairo storage + on-chain math runtime + TS indexer all agree):
  `pool_backing` is the live `lp_info.total_backing_deposited`,
  `initial_backing` is the immutable `params.backing`. Two new
  convention-pin tests assert (`base_k=50, pool=20_000,
  initial=10_000`) → `effective_k = 100`, and the mainnet CPI YoY
  ratio rises above `base_k` — a swapped mapping would silently
  floor at `base_k`.

### Notes — deadeye-sdk v0.1.3 (additive, non-breaking)

- The original `optimize_quote` / `optimize_quote_offline`
  signatures are unchanged. Old callers continue to work without
  modification.
- `BacktestEngine::from_journal` previously returned
  `Err(io::Error::other("not implemented"))`; the new behaviour
  succeeds on well-formed journals and returns `Ok(_)` with an
  empty event list on an empty file. Callers that relied on the
  stub erroring should switch to checking the resulting
  `engine.events.len()` instead.

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
