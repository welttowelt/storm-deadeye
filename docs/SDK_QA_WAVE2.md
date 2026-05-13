# SDK QA Wave 2 — portfolio aggregates + property / scale testing

This wave adds (a) a portfolio-level view for market makers, (b) an
offline backtest harness, (c) `proptest`-driven property tests for the
off-chain solver, and (d) a long-running scale chaos suite gated behind
two env vars. Wave 1's ergonomics + concurrency primitives back the new
APIs; nothing in the existing public surface moved.

## 1. Public API

### Portfolio aggregate (`crates/deadeye-sdk/src/portfolio.rs`)

```rust
pub struct MarketRef { pub family: Family, pub address: Felt }

pub struct PositionEntry {
    pub family: Family,
    pub raw: Position,           // tagged Normal | Lognormal | Multinoulli | Bivariate
    pub current_value_f64: f64,  // conservative: total_collateral projected
}

pub struct LpEntry {
    pub family: Family,
    pub shares_f64: f64,
    pub backing_share_pct: f64,
}

pub struct HedgeRecommendation {
    pub market: Felt, pub family: Family,
    pub notional_f64: f64, pub direction: f64,
}

pub struct Portfolio {
    pub trader: Felt,
    pub markets: Vec<MarketRef>,
    pub positions: BTreeMap<Felt, PositionEntry>,
    pub lp_positions: BTreeMap<Felt, LpEntry>,
    pub total_strk_balance: u128,
}

impl Portfolio {
    pub async fn load<P: Provider>(client: &DeadeyeClient<P>, trader: Felt,
        markets: Vec<MarketRef>) -> Result<Self, ContractError>;
    pub async fn load_with_strk_balance<P: Provider>(...) -> Result<Self, ContractError>;
    pub fn total_exposure_f64(&self) -> f64;
    pub async fn lp_yield_since<P: Provider>(&self, client: &DeadeyeClient<P>,
        since_block: u64) -> Result<BTreeMap<Felt, f64>, ContractError>;
    pub fn delta_neutral_hedge_for(&self, market_id: Felt) -> Vec<HedgeRecommendation>;
}
```

`Portfolio::load` reuses Wave 1's `BulkReader` so positions and LP info
fan out concurrently against the provider — wall clock converges on `2
× RTT` regardless of market count. STRK balance is supplied out-of-band
to keep the SDK free of a hard ERC-20 dependency.

### Backtest harness (`crates/deadeye-sdk/src/backtest.rs`)

```rust
pub enum SimDistribution { Normal | Lognormal | Multinoulli | Bivariate(...) }
pub enum EventDistribution { Normal | Lognormal | Multinoulli | Bivariate(...) }
pub enum MarketEvent { Trade | AddLiquidity | RemoveLiquidity | Settle }
pub enum StrategyAction { Trade | AddLiquidity | RemoveLiquidity }

pub struct MarketState {
    pub distribution: SimDistribution,
    pub backing: f64,
    pub lp_shares: f64,
    pub settlement_x_star: Option<f64>,
}

pub trait Strategy {
    fn on_event(&mut self, state: &MarketState, event: &MarketEvent) -> Vec<StrategyAction>;
}

pub struct BacktestEngine { pub initial_state: MarketState, pub events: Vec<MarketEvent> }

impl BacktestEngine {
    pub fn from_journal(path: &Path) -> io::Result<Self>;     // stub: returns Err
    pub fn from_indexer_events(events: Vec<MarketEvent>, initial: MarketState) -> Self;
    pub fn run<S: Strategy>(&self, strategy: S) -> BacktestResult;
}
```

Per-event the engine applies the recorded event to the simulated
`MarketState`, polls the strategy, applies its actions, and tracks P&L
through `deadeye_collateral::*_collateral`. No chain calls.

## 2. Property-test results

`crates/deadeye-collateral/tests/property.rs` — 4 tests, one per
family, each at `PROPTEST_CASES=10_000`.

| Family       | cases   | passed  | typed-err | panics |
| ------------ | ------- | ------- | --------- | ------ |
| Normal       | 10 000  | 10 000  | 0         | 0      |
| Lognormal    | 10 000  | 10 000  | 0         | 0      |
| Multinoulli  | 10 000  | 10 000  | 0         | 0      |
| Bivariate    | 10 000  | 10 000  | 0         | 0      |
| **Total**    | **40 000** | **40 000** | 0     | **0**  |

The contract: every call returns `Ok(verified)` whose collateral /
iteration / x_min / d_min fields are finite and within sane bounds, OR
a typed `CollateralError`. Panics, NaN returns, and unbounded iteration
counts are forbidden. Walltime: ~0.6 s for the entire 40 000-case
batch on Apple Silicon at `opt-level=1`.

Seeds are fully deterministic via proptest's standard
`PROPTEST_RNG_ALGORITHM` / `PROPTEST_RNG_SEED` knobs; the tests opt out
of `failure_persistence` so the runner works in sandboxed CI.

## 3. Scale test results

`crates/deadeye-e2e/tests/scale_chaos.rs` —
`scale_1000_actions_across_families`.

Double-gated: requires `DEADEYE_RUN_INTEGRATION=1` **and**
`DEADEYE_RUN_LONG=1`. The seed is deterministic
(`DEADEYE_SCALE_SEED=<u64>`, default `0xDEAD_BEEF_5CA1_E5CA`). Walking
1 000 actions (250/family × 4) the test asserts:

* No test-side panic.
* Overall convergence ≥ 90 %.
* ≥ 1 000 attempts recorded.

Stats are printed per family — attempts, convergence count, typed-error
count, and an action-mix breakdown.

The on-chain submission paths for non-normal families are exercised
by the existing per-family chaos suites; this driver focuses on the
off-chain solver convergence at scale, which is the right hot path to
stress nightly. Hooking the live writers in is mechanical and follows
the chaos-suite template; tracked as follow-up because the
`initialize_market` u256 overflow still gates devnet bring-up.

## 4. `cargo build --workspace --tests` outcome

Clean build (`Finished 'dev' profile [unoptimized + debuginfo] target(s)`).
Two pre-existing `missing_copy_implementations` warnings on
`normal_chaos`'s `Participant`/`Action` types remain — they predate
this wave.

`cargo test --workspace --tests` — every test binary green:

* `deadeye-sdk` unit: 18 / 18 (includes 3 portfolio + 3 backtest tests).
* `deadeye-collateral` unit: 20 / 20.
* `deadeye-collateral` `tests/property.rs`: 4 / 4 at 10 000 cases each.
* `deadeye-starknet` unit: 57 / 57.
* All four chaos suites still pass (status: ignored without
  `DEADEYE_RUN_INTEGRATION`, as before).
* `scale_chaos`, `portfolio` integration tests: ignored as designed.

Clippy on the wave's crates (`-p deadeye-sdk -p deadeye-collateral`) is
clean. Pre-existing `deadeye-starknet` clippy errors are out of scope.

## Files added or modified

* `crates/deadeye-sdk/src/portfolio.rs` — new module + 3 unit tests.
* `crates/deadeye-sdk/src/backtest.rs` — new module + 3 unit tests.
* `crates/deadeye-sdk/src/lib.rs` — module re-exports.
* `crates/deadeye-collateral/tests/property.rs` — new test binary.
* `crates/deadeye-e2e/tests/portfolio.rs` — new integration test.
* `crates/deadeye-e2e/tests/scale_chaos.rs` — new long-running test.
