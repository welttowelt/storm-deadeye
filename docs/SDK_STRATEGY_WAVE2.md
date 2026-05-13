# SDK strategy — wave 2

Items 8, 9, 11. Adds closed-form pricing, impact / sensitivity
primitives, block-driven state streaming, and a structured trade
journal.

## 1. Public API surface

### Item 8 — pricing (`deadeye_starknet::pricing`)

```rust
// payout = λ · pdf(x*) (continuous) / λ · p_i (multinoulli). Pure f64.
pub fn payout_at_normal     (&NormalDistribution,        k, x_star) -> f64;
pub fn payout_at_lognormal  (&LognormalDistribution,     k, x_star) -> f64;
pub fn payout_at_bivariate  (&BivariateNormalDistribution, k, x1, x2) -> f64;
pub fn payout_at_multinoulli(&CategoricalDistribution,   k, outcome) -> f64;
// + *_raw overloads accepting chain-stored Sq128Raw limbs.

pub fn sensitivities_normal      (...) -> NormalSensitivities;       // dμ, dσ
pub fn sensitivities_lognormal   (...) -> LognormalSensitivities;    // dμ, dσ
pub fn sensitivities_bivariate   (...) -> BivariateSensitivities;    // dμ1,dμ2,dσ1,dσ2,dρ
pub fn sensitivities_multinoulli (...) -> MultinoulliSensitivities;  // dp_i per outcome
```

Each `MarketReader` gains:

```rust
pub fn payout_at(...) -> f64;                                       // pure
pub async fn impact_for_mu_shift(delta_mu) -> ContractResult<ImpactEstimate>;
pub fn sensitivities_at(...) -> {Family}Sensitivities;
// Bivariate: impact_for_mu_shift(Δμ1, Δμ2).
// Multinoulli: impact_for_outcome_tilt(tilted, Δprob).
```

### Item 9 — quote stream (`deadeye_sdk::stream`)

```rust
pub struct MarketStateStream;   // futures::Stream<Item = MarketStateUpdate>
impl MarketStateStream {
    pub fn subscribe<P,B>(DeadeyeClient<P>, B, Family, market, StreamConfig) -> Self
        where P: Provider + 'static, B: BlockNumberSource + 'static;
}
pub struct StreamConfig { poll_interval, include_distribution,
                          include_lp_info, include_quote_for_candidate }
pub enum CandidateQuote { Normal{..}, Lognormal{..}, Multinoulli{..}, Bivariate{..} }
pub enum QuoteSnapshot;
pub struct MarketStateUpdate { block_number, family, market,
                               distribution, lp_info, quote }
pub trait BlockNumberSource;                     // small adapter trait
pub struct StarknetBlockSource<P: starknet_providers::Provider>;
```

Yields one update per *block-height transition*. Dropping the handle
stops the poller via `tokio::sync::Notify`.

### Item 11 — trade journal (`deadeye_sdk::journal`)

```rust
pub struct TradeJournal;        // BufWriter<File> JSONL append-only
pub trait JournalSink: Send { fn append(...); fn flush(...); }
pub enum EntryKind { Trade, Sell, AddLiquidity, RemoveLiquidity, Claim, Settle }
pub struct JournalEntry { timestamp, family, market, trader, kind,
                          off_chain_quote, tx_hash, receipt,
                          realized_pnl_at_settlement }
impl TradeJournal {
    pub fn open(&Path);
    pub fn append(...);
    pub fn replay(&Path) -> JournalReplay;  // Iterator<io::Result<JournalEntry>>
}
// Opt-in wrappers — plain writers untouched.
pub struct Journalled{Normal,Lognormal,Bivariate,Multinoulli}Writer<P,A,S>;
```

`Felt`s serialize as `"0x..."` hex; `Family` derives serde (snake_case).

## 2. `spread_at` — **skipped**

The AMM has no CLOB-style bid/ask. The cost of moving μ by ε is a
*function* of σ, backing, x\*, and trade direction — not a constant.
Reconstructing a notional spread requires inverting the collateral
solver for ±ε trades (as expensive as the trade itself), and the
AMM is one-sided per direction so the result has no liquidity-curve
meaning. Rationale: `crate::pricing::spread_at_normal_skipped`.
Callers can compose `impact_for_mu_shift(+ε)` /
`impact_for_mu_shift(−ε)` themselves.

## 3. `payout_at` bench

```
payout_at_normal: 3.5 ns/call over 200_000 iterations  (release)
```

**~280× under the 1 µs budget.** Zero per-call allocation: `#[inline]`,
single PDF evaluation, no Sq128 round-trip. Test asserts `< 1 µs` so
regressions surface in CI.

## 4. Devnet e2e

`DEADEYE_RUN_INTEGRATION=1 cargo test -p deadeye-e2e --test <X> -- --ignored`:

| Test | Result |
|---|---|
| `quote_stream` | **PASS** — caught all 3 N(43,49) → N(44,36) → N(45,25) transitions |
| `journal_roundtrip` | **PASS** — 1 trade → 1 JSONL entry → replay round-trips |
| `normal_chaos` / `lognormal_chaos` / `multinoulli_chaos` / `bivariate_chaos` | **all PASS** |

## 5. `cargo build --workspace --tests`

Clean. Only pre-existing `missing-copy-implementations` warnings on
`normal_chaos::Participant` / `Action`. `cargo test --lib`: **153
passed, 0 failed** (up from 120) — 9 new pricing tests, 11 new AMM
tests, 4 new stream tests, 3 new journal tests.

Clippy (`all + pedantic + nursery + restriction`): my new code is
fully clean. 18 pre-existing violations in `error.rs`, `multi_rpc.rs`,
`nonce.rs`, `runtime.rs` are out of scope.

## Architecture notes

* `deadeye-starknet` now depends on `deadeye-collateral`. No cycle.
* `deadeye-sdk` gained `serde` / `serde_json` / `tokio` (sync, rt,
  time); `tempfile` dev-dep.
* `BlockNumberSource` keeps the stream provider-agnostic;
  `StarknetBlockSource` adapts any `starknet_providers::Provider`.
* Journal wrappers are *opt-in* — plain `execute_*` paths unchanged.
