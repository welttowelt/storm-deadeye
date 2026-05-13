# Chaos suite — driver brief

Shared context for every driving test engineer (one of 2 per family).

## What is deadeye-rs

A Rust SDK at `/Users/theodorepender/Coding/the-situation-stack/deadeye-rs/`
for Deadeye prediction markets on Starknet. Workspace layout:

```
crates/
  deadeye-core/        Sq128, distributions, errors
  deadeye-collateral/  4 collateral solvers (normal/lognormal/multinoulli/bivariate)
  deadeye-artifacts/   embedded ABIs
  deadeye-starknet/    CairoSerde + Provider + Account + per-family readers/writers + factory + oracle
  deadeye-sdk/         high-level facade (NormalMarket, LognormalMarket, MultinoulliMarket, BivariateMarket)
  deadeye-optimizer/   EV maximizer
  deadeye-indexer/     production indexer HTTP client
  deadeye-testkit/     devnet fixture pipeline (see below)
  deadeye-e2e/         integration tests
```

## Working primitives (devnet integration)

`deadeye-testkit::fixture::*` provides:

- `bootstrap_devnet(BootstrapConfig)` → `TestEnv`. Resets devnet → declares
  15 classes (with on-chain CASM hash discovery) → deploys factory + 4
  family plugins → deploys a standalone normal-math-runtime for hint
  computation → configures all 4 market types. Uses **STRK at
  `0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d`**
  as the collateral token (predeployed devnet token, vanilla ERC20, every
  predeployed account starts with ~1000 STRK).

- `TestEnv` fields: `url`, `chain_id`, `declared` (DeclaredHashes),
  `factory`, `normal_plugin`, `normal_runtime`, `lognormal_plugin`,
  `multinoulli_plugin`, `bivariate_plugin`, `collateral` (STRK), `admin`
  (DevnetAccount), `participants: Vec<DevnetAccount>` (4 by default).

- `TestEnv::account_handle(&DevnetAccount)` → starknet-rs `SingleOwnerAccount`.
- `TestEnv::owned_account(&DevnetAccount)` → SDK `OwnedAccount` wrapper.

- `lifecycle::upsert_normal_profile_for_test(account, factory, collateral, profile_id)` —
  installs profile 1 with k=50, backing=50, tolerance=1e-3,
  min_trade_collateral=0.1, internal_decimals=6, fees=0.

- `lifecycle::fetch_normal_hints(provider, runtime, dist)` — calls
  `compute_hints_view` on the math runtime to get **on-chain-correct**
  Q128 sqrt hints (the on-chain hint check is byte-by-byte; f64 sqrt
  doesn't round-trip).

- `lifecycle::deploy_normal_market_with_event(account, factory, profile_id,
  salt, metadata, dist, hints)` → deploys via factory + parses
  `MarketDeployed` event for the market address.

- `lifecycle::initialize_market(account, market, collateral, approve_amount)` →
  approve + call `market.initialize()`. **⚠️ Currently fails at
  `transferFrom` with `u256_sub Overflow` — see KNOWN ISSUES.**

- `erc20::{balance_of, transfer, approve}` — STRK helpers.

- SDK: `DeadeyeClient::new(JsonRpcProvider)` → `.normal_market(addr)`,
  `.lognormal_market(addr)`, `.multinoulli_market(addr)`,
  `.bivariate_market(addr)`. Each has `.distribution()`, `.reader()`,
  `.with_account(account)` to get a write-capable handle.

- `deadeye_collateral::normal_collateral(f, g, policy)` → `VerifiedMinimum
  { x_min, d_min, collateral, iterations }`. Equal-variance pairs are a
  saddle case — avoid by perturbing both μ and σ.

- `deadeye_starknet::types::normal::TradeInput` (+ analogues per family) —
  CairoSerde input for `execute_trade`.

- `deadeye_starknet::NormalMarketWriter` (+ analogues) — `execute_trade`,
  `sell_position_guarded`, `claim`, `add_liquidity`, `remove_liquidity`.

## Per-family primitives still TODO (your suite assumes they exist)

The bootstrap currently only deploys a **normal** math runtime + profile.
For your family (lognormal/multinoulli/bivariate), assume the equivalent
helpers exist with these names:

- `lifecycle::upsert_<family>_profile_for_test(account, factory, collateral, profile_id)`
- `lifecycle::deploy_<family>_market_with_event(account, factory, profile_id, salt, metadata, dist, hints)`
- `lifecycle::fetch_<family>_hints(provider, runtime, dist)`
- `TestEnv::<family>_runtime` field

Multinoulli uses `CategoricalDistributionRaw + CategoricalL2HintRaw` (no
sqrt hints); bivariate uses `BivariateNormalDistributionRaw +
BivariateNormalSqrtHintsRaw` and a `BivariatePointRaw` for `x_star`.

## Known issues you should document but not block on

1. **`initialize_market` reverts** with `u256_sub Overflow` inside STRK's
   transferFrom. Root cause not yet diagnosed — likely backing-to-token
   conversion produces an amount larger than expected. Write your chaos
   suite assuming this gets fixed; mark the relevant step with
   `// TODO: blocked on initialize_market u256 overflow — see CHAOS_SUITE_STATUS.md`.
2. **Off-chain normal collateral solver fails on equal-variance pairs**
   (saddle of `d''`). When generating candidates, perturb both μ and σ
   (e.g. σ_g ≠ σ_f, ideally σ_g ∈ [0.7 σ_f, 1.4 σ_f] and μ_g ≠ μ_f).

## What your chaos suite must do

Create the file `crates/deadeye-e2e/tests/<family>_chaos.rs` (one file
per family). It must:

1. **Pick a realistic example question** for the family. Examples:
   - Normal: "Anthropic Opus 4.7 ARC-AGI-3 score at 2026-12-31, in percent."
   - Lognormal: "BTC/USD close on 2026-12-31, in USD." (heavy right tail)
   - Multinoulli: "Which of {GTM, Product, Growth, Goats} wins the
     2026-Q4 internal hackathon?" (4 outcomes)
   - Bivariate: "(eval score %, p50 latency ms) for the same model at
     2026-12-31." (correlated μ₁,μ₂; choose ρ ∈ (-1, 1))

2. **Use ≥ 5 participants** with different roles. Mix:
   - 2 traders who only execute trades
   - 1 LP who only adds/removes liquidity
   - 1 trader-LP hybrid (does both)
   - 1 admin (settles + claims)

3. **Run ≥ 12 actions across multiple parameters.** Don't just sweep μ;
   also vary σ (or σ₂/ρ for bivariate; or multiple outcome probs for
   multinoulli). Each trader does ≥ 2 trades. Interleave LP add/remove
   with trades.

4. **Take a `BalanceSnapshot` before and after each phase.** A snapshot
   collects:
   - STRK balance for every participant
   - STRK balance of the market contract
   - STRK balance of the factory treasury (env.admin in our setup)
   - `market.lp_info()` (`total_shares`, `total_backing_deposited`)
   - per-trader `market.position(trader)` (compact position)

5. **Assert conservation invariants:**
   - **Collateral conservation**: across any phase that doesn't include
     settlement/claim, `Σ(participant_balance_delta) +
     market_balance_delta == 0`. Tolerance: 0 (exact, in u128 base units).
   - **LP backing conservation**: `total_lp_backing_delta ==
     Σ(LP_deposit_token_delta)`.
   - **Position-value conservation at settlement**: `Σ trader payouts +
     Σ LP claim components == total_backing_at_settlement` to 1e-3
     relative tolerance (f64).
   - **No participant ends with negative balance.**
   - **Final balances match closed-form predictions** for at least one
     scenario (e.g., the "no-trade" case where every trader's trade
     cancels out: each participant's final balance should equal initial
     minus fees).

6. **Settle the market** at a believable outcome:
   - Normal: an x* between min(μ_market_history) and max + a couple σ
   - Lognormal: a price near the median (exp(μ))
   - Multinoulli: a single outcome index
   - Bivariate: a `BivariatePointRaw { x1, x2 }`

7. **Every participant claims their position** after settlement.

8. **Assert** that post-claim, the market's STRK balance is ~0 (within
   dust tolerance — say, 1000 base units) and every participant's claim
   payout matches their position's `compute_position_value` to 1e-3.

9. **Output**: print every snapshot diff at every phase boundary using
   `eprintln!`. The test should read like a transaction tape.

10. **Bug hunting**: include at least 2 deliberately tricky scenarios:
    - One that exercises the **policy envelope** (σ ratio at 3.9×;
      mean separation at 3.9 σ).
    - One that exercises a **degenerate case** (a participant trades
      back to the original distribution; their net P&L should be ≤ 0).

## Implementation guidance

- The file should be 500–800 LOC. Helper modules can hold scenario
  generators if it helps readability.
- Use `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]` and
  gate on `DEADEYE_RUN_INTEGRATION=1`.
- Use `#![allow(clippy::print_stderr, clippy::tests_outside_test_module,
  clippy::unwrap_used, clippy::panic, reason = "...")]` at the top.
- Build a small `BalanceSnapshot` struct + `diff` impl inline if
  `deadeye_testkit::fixture::chaos` doesn't have one yet (it doesn't —
  your file can ship that struct).
- Build a small `Participant` struct that bundles an `OwnedAccount` plus
  a role tag.
- Do NOT skip the assertion phase. If an assertion isn't yet checkable
  because of a blocked dependency, comment it out with the issue tag
  rather than removing it.

## Linting

The workspace runs `clippy::all + pedantic + nursery + restriction
(curated)`. Your test file already has a permissive `#![allow]` block
above. Common issues:
- Use `assert!((a - b).abs() < 1e-9)` not `assert_eq!(a, b)` for f64.
- `mul_add` for FMA where clippy nags.
- `#[expect(reason = "...")]` for unavoidable suppressions.

## Deliverable

Write the file at:
`/Users/theodorepender/Coding/the-situation-stack/deadeye-rs/crates/deadeye-e2e/tests/<family>_chaos.rs`

(Or, if you are driver #1, write to `<family>_chaos_driver1.rs`; driver
#2 writes to `<family>_chaos_driver2.rs`. The reviewer + integrator will
pick the better and merge.)

Report at the end: a short summary listing the 12+ actions you scheduled
and the invariants you assert.
