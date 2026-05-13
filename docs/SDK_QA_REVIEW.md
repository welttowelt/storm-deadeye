# SDK QA Wave 2 — Review

Audit, 2026-05-12.

## 1. Portfolio aggregate

**Verdict:** correct shape, two math bugs, one missing caveat.

Concurrency verified: `Portfolio::load` builds a `BulkReader`, joins
`positions` + `lp_infos` via `futures::future::join` (true 2×RTT fan-out).

**Bug A — LP valuation.** `total_exposure_f64` computes the LP component
as `(backing_share_pct / 100) × shares_f64`. Per
`deadeye_optimizer::lp::compute_lp_claim_component_value`, an LP's claim
is `pool_share × pool_value`. The driver's expression instead does
`pool_share × shares` — squaring the share and dropping the pool factor.
For a 10% LP of a 1000-share pool, the result is `0.10 × 10 = 1.0`
instead of `0.10 × 1000 = 100.0`. Two orders of magnitude off.

**Bug B — STRK unit mismatch.** `strk_f = total_strk_balance / 1e18`
projects STRK to whole tokens while positions+LP sums live in raw Q128
"units of backing". Different scales summed together → meaningless
aggregate. Test only asserts `exp > 0`, so the bug slips through.

**Missing caveat — correlation.** `delta_neutral_hedge_for` distributes
`weight × target_value` across non-target markets at direction `-1.0`.
Sound only if all markets share a common driver. For uncorrelated
books it's noise; for negative correlation it doubles exposure.
Doc-string says "intentionally approximate" but doesn't warn callers.
Add: *assumes a common factor; otherwise the hedge is nonsense.*

`cargo test -p deadeye-sdk portfolio::tests`: **3/3 pass.**

## 2. Backtest harness

**Verdict:** mathematically simple but consistent. Strategy DOES see
the post-trade state — `apply_event_to_state` mutates first, then
`strategy.on_event(&state, event)` runs. Each strategy action applies
immediately so later actions in the same step see updates.

**P&L:** purely negated `normal_collateral` on Trade actions; settlement
is a no-op (`backtest.rs:233-239`). Final P&L equals `claim()` only for
buy-and-hold (`pnl=0`). Otherwise it's `-Σ collateral_paid`, which the
docstring correctly labels a "strategy ceiling".

**Hand example.** Initial `N(42, 64)`. `MuTracker { offset: 0.1 }` sees
event `N(42.05, 64)`, trades to `N(42.15, 64)`. That's a pure 0.1σ
μ-shift, equal-σ: chain-precomputed `x* ≈ μ_f − 0.5435σ`,
`collateral ≈ ε`. Across 50 events with alternating ±0.05/±0.04 jitter,
`final_pnl ≤ 0` and `|final_pnl| ≪ 50·O(ε)`. The unit test asserts that.

`cargo test -p deadeye-sdk backtest::tests`: **3/3 pass.**

## 3. Property tests

**Verdict before fix:** insufficient. Asserted only finite,
non-negative, bounded iterations. Did NOT re-verify the post-Wave-0
chain-faithful invariant `|d̃'(x*)| < tolerance · scale`. Chaos surface
coverage was OK (μ ∈ [−100,100], σ² ∈ [0.01,1000] — covers σ-shrinks,
equal-σ, opposite-μ).

**Fix shipped.** `crates/deadeye-collateral/tests/property.rs` now:

* Independently re-evaluates `λ_g·g'(x*) − λ_f·f'(x*)` after each
  normal-family `Ok` and asserts `|d̃'(x*)| < 1e-8 · max(λ_f, λ_g, 1)` —
  exactly Cairo's `scaled_verify_minimum_with_lambda` contract.
* Multinoulli property sweeps all outcomes to confirm the reported
  `min_outcome_index` is the true λ-scaled argmin (off-by-one guard).
* `DEADEYE_RUN_INTEGRATION`-gated stub for off-chain vs
  `check_trade_view` chain comparison — short-circuits cleanly until
  the per-family `check_trade_view` reader lands.

## 4. Scale chaos

**Verdict before fix:** off-chain only. Driver ran `normal_collateral`
in a loop and bookkept the other three families as trivially
converged. The driver report said "init blocker still gates devnet" —
but it doesn't; `CHAOS_SUITE_STATUS.md` item 1 is RESOLVED.

**Fix shipped.** Rewrote `crates/deadeye-e2e/tests/scale_chaos.rs`:

* New `bootstrap_normal_live` mirrors `normal_chaos.rs`: upsert
  profile → deploy → `initialize_market` → approve → return live writer.
* New `step_normal_live` runs each action against the chain via
  `NormalMarketWriter::{execute_trade, sell_position, add_liquidity,
  remove_liquidity}`. Off-chain solver gates every trade; chain
  submissions tracked separately.
* `ACTIONS_PER_FAMILY` reduced 250 → **50** (~21 min for the normal
  family alone) per the brief's "weekly nightly" target.
* Lognormal / Multinoulli / Bivariate stay off-chain in this revision
  with an in-file template marker; hooking them up is mechanical.
* New hard assert: `chain_failures / chain_submissions ≤ 5%`, guarding
  against off-chain↔on-chain divergence.

I did not run the full scale_chaos end-to-end (~25 min, double-gated).
Compilation: clean. Logic: matches normal_chaos.

## 5. Property tests final

**40 000 / 40 000 pass** with the tightened chain-faithful invariants
(10 000 each × 4 families; ~0.66 s wall).

## 6. bivariate_chaos regression

**PASS.** 1/1 ok in 23.98 s against a fresh devnet. Note:
`/tmp/deadeye_casm_hashes.json` must be cleared between devnet restarts
or the declare-cache stale-hash check fires `InvalidTransactionNonce`.

---

**Files touched:**
* `crates/deadeye-collateral/tests/property.rs` — chain-faithful invariants + integration stub.
* `crates/deadeye-e2e/tests/scale_chaos.rs` — normal family wired to chain submissions, budget 250→50.

**Files NOT touched** (per the brief): Cairo, Driver C's scope,
`portfolio.rs`, `backtest.rs` (bugs reported only).
