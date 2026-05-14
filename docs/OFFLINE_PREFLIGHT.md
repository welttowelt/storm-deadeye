# Offline Preflight — Chain-Bit-Exact σ + Hints Without a Math Runtime

`deadeye-sdk` v0.1.1 ships `NormalMarket::optimize_quote_offline`, a
chain-faithful EV optimizer for the case where **no math-runtime contract
instance is deployed** (mainnet today — the normal AMM ships as a
library-dispatch class hash with no separately deployed instance).

## 1. New public API

```rust
impl<P: Provider> NormalMarket<P> {
    /// Off-chain-only EV optimizer. Output σ + hints are bit-identical
    /// with what the on-chain `compute_hints_view` would emit, so the
    /// candidate distribution survives `INVALID_DISTRIBUTION` /
    /// `INVALID_HINTS` checks by construction.
    pub async fn optimize_quote_offline(
        &self,
        belief_mean: f64,
        belief_sigma: f64,
        budget_xp: f64,
    ) -> SdkResult<NormalTradeQuote>;
}
```

Implementation (see `crates/deadeye-sdk/src/normal.rs`):

1. Read live `(distribution, params, lp_info)` from chain.
2. Derive `effective_k = max(base_k, mul_down(base_k, pool_backing) /
   initial_backing)` — the same formula `compute_effective_trade_k_view`
   runs on-chain, with `pool_backing = params.backing` and
   `initial_backing = lp_info.total_backing_deposited`.
3. Hand `(μ_market, σ_market, effective_k, budget, belief_*)` to
   `deadeye_optimizer::optimize_normal_trade`.
4. Promote `optimized_variance` to `Sq128`, construct the candidate via
   `NormalDistribution::from_variance` so σ is **derived via
   `Sq128::sqrt`** (not f64). This is the critical bit-parity step.
5. Compute hints offline via the chain formulas at `Sq128` precision:
   - `l2_norm_denom = sqrt(mul_down(mul_down(2, σ), √π))`
   - `backing_denom = sqrt(mul_down(σ, √π))`
   - `√π` taken from Cairo's `SQRT_PI_RAW` limb-for-limb.

`Sq128::checked_mul` matches Cairo `mul_down`, and `Sq128::sqrt` matches
`sqrt_verified`/`sqrt_unchecked` (proven 20/20 on devnet — see
[`SQ128_SQRT.md`](SQ128_SQRT.md)).

The companion `optimize_quote` was also rewired to construct the
candidate via `from_variance` rather than `from_sigma`, so both paths
share the same Sq128 σ-derivation. The signature is unchanged.

## 2. Bit-exactness test outcome

`crates/deadeye-e2e/tests/offline_optimize_quote_parity.rs` bootstraps
a fresh starknet-devnet, deploys a normal market with the chaos profile,
and runs 10 `(μ_b, σ_b, budget)` scenarios. For each scenario it:

1. Calls `optimize_quote(runtime, ...)` — the chain path with full
   `compute_hints_view` + `check_trade_view` preflight.
2. Calls `optimize_quote_offline(...)` — pure off-chain.
3. Asserts limb-for-limb equality on `(μ_g, variance_g, σ_g)` and
   `(l2_norm_denom, backing_denom)`.
4. Asserts `required_collateral` agrees to within 1 ULP when both paths
   accept.

**Result: 10/10 scenarios match.** The candidate distribution and hints
are limb-for-limb identical between the chain and off-chain paths for
every scenario. Sample log:

```
[3/10] μ_b=42, σ_b=2, budget=25:
  chain σ=2.000000000000000000 | off σ=2.000000000000000000
  dist=OK hints=OK
[9/10] μ_b=48, σ_b=8, budget=75:
  chain σ=8.000000000000000000 | off σ=8.000000000000000000
  dist=OK hints=OK

🔍 parity: 10/10 scenarios matched
```

Run locally with:

```bash
starknet-devnet --port 5050 --seed 0 --accounts 10 --initial-balance 9999999999999999999999 &
DEADEYE_RUN_INTEGRATION=1 cargo test -p deadeye-e2e \
  --test offline_optimize_quote_parity -- --nocapture
```

## 3. cpi-arb-demo diff summary

- `cpi-arb-demo/crates/cpi-bot/src/execute.rs::quote_trade` — the
  off-chain fallback branch (lines ~190-225) now calls
  `NormalMarket::optimize_quote_offline` instead of inlining
  `optimize_normal_trade`. Net effect: the bot reads live `effective_k`
  from chain (no more hard-coded `75.07` placeholder), derives σ via
  `Sq128::sqrt`, and emits chain-bit-exact hints — even when
  `DEADEYE_NORMAL_RUNTIME_ADDR` is unset.
- `cpi-arb-demo/Cargo.toml` — `[patch.crates-io]` points the workspace
  at the local `deadeye-rs` workspace until v0.1.1 is published to
  crates.io.

The bot has two clean paths:

| `DEADEYE_NORMAL_RUNTIME_ADDR` | Method called | Chain check |
| --- | --- | --- |
| Set | `NormalMarket::optimize_quote` | `check_trade_view` (full) |
| Unset | `NormalMarket::optimize_quote_offline` | None — σ/hints bit-exact |

## 4. Build + test outcomes

```
$ cargo build -p deadeye-sdk
   Compiling deadeye-sdk v0.1.1
    Finished `dev` profile

$ cargo test -p deadeye-sdk --lib
test result: ok. 24 passed; 0 failed

$ cargo clippy -p deadeye-sdk --all-features --tests
    Finished `dev` profile (no warnings)

$ DEADEYE_RUN_INTEGRATION=1 cargo test -p deadeye-e2e \
    --test offline_optimize_quote_parity
test offline_optimize_quote_parity_against_runtime ... ok
  🔍 parity: 10/10 scenarios matched

$ cargo build -p cpi-bot
   Compiling deadeye-sdk v0.1.1
   Compiling cpi-bot v0.1.0
    Finished `dev` profile

$ cargo test -p cpi-bot
test result: ok. 17 passed; 0 failed
```

## 5. Versioning

- `deadeye-sdk` bumped to **v0.1.1**.
- Workspace package version remains `0.1.0`; only the SDK crate's
  version was bumped to scope this release narrowly (Driver A is
  bumping `deadeye-cli` independently — coordinate when both land).
- **Not published** in this run. The crate is consumed locally by
  `cpi-arb-demo` via `[patch.crates-io]` until both drivers are
  approved and we publish v0.1.1 atomically.
