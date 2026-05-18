# Optimizer Property Tests — σ-Arb Coverage Gap Closure

`deadeye-optimizer` v0.1.2 adds the property test that would have caught
the pre-v0.1.1 σ-arbitrage bug. Lives in
`crates/deadeye-optimizer/tests/grid_existence.rs`.

## 1. The property assertion (verbatim)

> **Given any (belief, market, budget), if a positive-EV trade exists at
> any grid point, the optimizer must return it.**

Two-sided:

- Ground truth witnesses `net > 0` ⇒ optimizer must return some trade
  with `net > 0` (not necessarily the same lattice point — ties exist).
- Ground truth witnesses no positive-net point ⇒ optimizer must return
  the no-trade sentinel (`collateral_required == 0`).

### How the ground truth is defined

`grid_scan_ground_truth(...)` walks the same 51 × 51 (μ, σ) lattice the
optimizer iterates and computes `net = EV − collateral` via the same
`deadeye_collateral` primitives. Lattice constants are **replicated** in
the test (with a `MUST stay in sync` comment) to keep ground truth
independent of the SUT. The pre-v0.1.1 bug would have made the
optimizer's filter reject points the independent scanner accepts —
firing the proptest on every such pair.

## 2. Cases run

| Suite | Cases | Outcome |
|---|---|---|
| `optimizer_returns_a_trade_when_ground_truth_says_one_exists` (proptest) | 5 000 | **5 000 / 5 000 pass** |
| `sigma_only_arb_at_equal_mu_must_trade_if_belief_tighter` (regression-anchor) | 4 explicit cases | **4 / 4 pass** |
| Pre-existing optimizer unit tests | 9 | **9 / 9 pass** |

Regression-anchor cases (verbatim from the brief):

1. `live-CPI-2026-05-14` — μ_b=4.3274 σ_b=0.2143, μ_m=4.29 σ_m=0.35, budget=50, k=75.07
2. `pure σ-arb equal-μ` — μ_b=4.29 σ_b=0.2143, μ_m=4.29 σ_m=0.35, budget=50, k=75.07
3. `σ-tightening more` — μ_b=4.29 σ_b=0.05, μ_m=4.29 σ_m=0.35, budget=50, k=50.00
4. `σ-widening (counterintuitive)` — μ_b=4.29 σ_b=0.70, μ_m=4.29 σ_m=0.35, budget=50, k=50.00

Generator ranges (matching the brief):
`mu_b, mu_m ∈ [-100, 100]`, `var_b, var_m ∈ [0.01, 1000]`,
`budget ∈ [0.1, 10 000]`, `k ∈ [1, 1000]`. Sigmas are derived as
`var.sqrt()`.

Proptest config: `failure_persistence: None` so the runner is
sandbox-friendly; deterministic via the standard
`PROPTEST_RNG_ALGORITHM` / `PROPTEST_RNG_SEED` knobs.

## 3. Coverage

Measured with `cargo llvm-cov --package deadeye-optimizer
--summary-only` (cargo-llvm-cov v0.8.7 against the local stable
toolchain). Result, post-this-work:

```
Filename       Regions    Missed     Cover   Lines   Missed   Cover   Functions   Cover
lp.rs              69         0    100.00%      58        0  100.00%      8/8     100.00%
normal.rs         258        13     94.96%     199       13   93.01%    11/11     100.00%
TOTAL             327        13     96.02%     257       13   94.67%    19/19     100.00%
```

`src/normal.rs` is at **93.01 % line / 94.96 % region / 100 % function**
coverage — well above the 90 % target. The 13 uncovered lines are
unreachable error-return paths in `collateral_number`'s
`Sq128::from_f64` / `NormalDistribution::from_variance` arms (only
triggered by NaN / out-of-Q128.128 inputs, which the proptest input
ranges don't produce).

The previously-uncovered branches in `optimize_normal_trade` — the
σ-only / equal-μ corner of the lattice, the `coll > budget` filter
rejection path, and the `best_net <= 0` no-trade sentinel — are all
exercised by the 5 000-case proptest.

## 4. New bugs surfaced

**None.** All 5 000 + 4 cases passed against the v0.1.2 optimizer (same
code as v0.1.1 modulo version — body untouched per constraint). The
proptest now **locks in** post-fix behavior; any future regression of
the `collateral_number` path will trip immediately.

## 5. Bumps

- `deadeye-optimizer`: `0.1.1` → `0.1.2`.
- Workspace `Cargo.toml`: `deadeye-optimizer` dep pinned to `0.1.2`.
- No publish — coordinated at end of Item 3.

## 6. Files touched

- `crates/deadeye-optimizer/Cargo.toml` — version bump, `proptest`
  added to `[dev-dependencies]`.
- `crates/deadeye-optimizer/tests/grid_existence.rs` — new (this work).
- `Cargo.toml` (workspace) — `deadeye-optimizer` version pin.
