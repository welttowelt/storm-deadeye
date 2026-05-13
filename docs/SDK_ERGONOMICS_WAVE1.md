# SDK ergonomics — wave 1

Five items from the post-chaos backlog. MMs no longer hand-build
calldata, manually fetch hints, parse revert strings, or route admin
calls through factory selectors by name.

## 1. New public API surface

### `deadeye-starknet`

```rust
// error.rs
pub enum TradeError { Rejected { reason, source }, Submission(ContractError) }
pub enum TradeRejectionReason {
    InvalidDistribution, InvalidHints, BackingFail, SigmaTooLow, LowCollateral,
    VerificationFailed { sub_reason: Option<VerificationSubReason> },
    StaleState { field: &'static str },
    MarketSettled, MarketPaused, NoPosition, AlreadyClaimed,
    RequiresAdditionalCollateral, NoCollateralOut, ConversionFailed,
    OnlyOwner, NotAuthorized, MinOutNotMet, InvalidMinOutcome, Reentrant,
    MarketNotInitialized, Other { raw: &'static str },
}
pub enum VerificationSubReason {
    SideInvalid, StationaryInvalid, CurvatureInvalid, CollateralInsufficient, MinimumInvalid,
}
pub fn parse_revert_reason(err: &ContractError) -> Option<TradeRejectionReason>;
pub type TradeResult<T> = Result<T, TradeError>;

// runtime.rs (new module) — per-family math runtime view helpers
pub async fn compute_normal_hints / compute_lognormal_hints
            / compute_multinoulli_hint / compute_bivariate_hints
            / expand_bivariate_distribution
            / check_normal_trade / check_lognormal_trade;

// per-family readers — preflight
impl {Normal,Lognormal,Multinoulli,Bivariate}MarketReader {
    pub async fn quote_trade(&self, runtime, candidate, …) -> TradeResult<…TradeQuote>;
}
impl BivariateMarketReader { pub async fn distribution_raw(&self) -> ContractResult<…>; }

// per-family writers — typed execute + ergonomic sell
impl {Normal,Lognormal,Bivariate}MarketWriter {
    pub async fn execute_quote(&self, q)                       -> TradeResult<ExecutionReceipt>;
    pub async fn sell_position(&self, runtime, min_token_out)  -> TradeResult<ExecutionReceipt>;
}
impl MultinoulliMarketWriter {
    pub async fn execute_quote(&self, q)                       -> TradeResult<ExecutionReceipt>;
    pub async fn execute_sparse_with_runtime(…)                -> TradeResult<ExecutionReceipt>;
    pub async fn execute_transfers_with_runtime(…)             -> TradeResult<ExecutionReceipt>;
    pub async fn sell_position(&self, min_token_out)           -> TradeResult<ExecutionReceipt>;
}

// factory.rs — typed admin
impl FactoryWriter {
    pub async fn settle_{normal,lognormal,multinoulli,bivariate}(&self, markets, payload, strict)
        -> TradeResult<ExecutionReceipt>;                                       // batch
    pub async fn settle_{normal,lognormal,multinoulli,bivariate}_market(&self, market, payload)
        -> TradeResult<ExecutionReceipt>;                                       // single
    pub async fn pause_market_typed(&self, market)        -> TradeResult<ExecutionReceipt>;
    pub async fn unpause_market_typed(&self, market)      -> TradeResult<ExecutionReceipt>;
    pub async fn collect_protocol_fees(&self, market, recipient)
        -> TradeResult<ExecutionReceipt>;
}
```

All new methods are additive; existing `execute_trade`,
`sell_position_guarded`, `build_*_call`, untyped `pause_market`,
`unpause_market`, and `settle_normal_markets_*` are unchanged.

### `deadeye-sdk`

`SdkError::Trade(#[from] TradeError)` forwards typed reverts;
module-level `no_run` worked examples on `lib.rs`, every family handle
(`normal.rs`, `lognormal.rs`, `multinoulli.rs`, `bivariate.rs`), and
`deadeye-starknet::factory.rs`.

## 2. Revert-string → variant mapping

| Cairo `assert(false, '…')` | Variant |
|----------------------------|---------|
| `INVALID_DISTRIBUTION` | `InvalidDistribution` |
| `INVALID_HINTS` | `InvalidHints` |
| `BACKING_FAIL` | `BackingFail` |
| `SIGMA_TOO_LOW` | `SigmaTooLow` |
| `LOW_COLLATERAL` | `LowCollateral` |
| `VERIFICATION_FAILED` | `VerificationFailed { sub_reason: None }` |
| `SIDE_INVALID` | `VerificationFailed { Some(SideInvalid) }` |
| `STATIONARY_INVALID` | `VerificationFailed { Some(StationaryInvalid) }` |
| `CURVATURE_INVALID` | `VerificationFailed { Some(CurvatureInvalid) }` |
| `COLLATERAL_INSUFFICIENT` | `VerificationFailed { Some(CollateralInsufficient) }` |
| `MINIMUM_INVALID` | `VerificationFailed { Some(MinimumInvalid) }` |
| `STALE_STATE` | `StaleState { field: "guard" }` |
| `market settled` / `market is settled` | `MarketSettled` |
| `market paused` / `market is paused` | `MarketPaused` |
| `no position` | `NoPosition` |
| `already claimed` | `AlreadyClaimed` |
| `REQUIRES_ADDITIONAL_COLLATERAL` | `RequiresAdditionalCollateral` |
| `NO_COLLATERAL_OUT` | `NoCollateralOut` |
| `CONVERSION_FAILED` | `ConversionFailed` |
| `only owner` | `OnlyOwner` |
| `not authorized` | `NotAuthorized` |
| `MIN_OUT_NOT_MET` | `MinOutNotMet` |
| `INVALID_MIN_OUTCOME` | `InvalidMinOutcome` |
| `REENTRANT` | `Reentrant` |
| `market not initialized` / `not initialized` | `MarketNotInitialized` |
| anything else | `parse_revert_reason → None` (caller keeps `ContractError`) |

The parser scans the stringified provider error for `0x…` hex felts,
decodes each via `parse_cairo_short_string`, then falls back to direct
substring match for plain-text revert formats.

## 3. Chaos-test diff stats

`dispatch_sell` in `normal_chaos.rs`: **53 → 19 lines (−34, −64%)**.
`run_sell_all` in `lognormal_chaos.rs`: **52 → 16 lines (−36, −69%)**.
`SellExecutionGuardsRaw` / `LognormalSellExecutionGuardsRaw` imports
removed; the manual `params()` + `lp_info()` + hints-fetch + guard
construction is gone from all call sites. `multinoulli_chaos.rs`
settle now calls `factory_writer.settle_multinoulli_market(market, outcome)`
directly (the local `build_settle_call` helper remains for callers that
want raw calldata). Total chaos-test lines deleted: ~70.

## 4. `cargo build --workspace --tests`

Builds in 3.59s. All four chaos tests build clean. The three remaining
`missing-copy-implementations` / `trivial-casts` warnings pre-date this
wave.

## 5. `cargo doc --no-deps --workspace`

Builds clean. Six new `no_run` doctests added; all compile under
`cargo test --doc -p deadeye-sdk -p deadeye-starknet` (5+1 passed). Nine
pre-existing `unresolved link` warnings untouched.

## Unit tests

`error::tests` adds 6 tests (revert classification, refined-guard
precedence, felt-hex round-trip, `TradeError::from_contract` promotion);
all pass. Workspace-wide `cargo test --lib`: zero failures, 36 in
`deadeye-starknet` (up from 30).
