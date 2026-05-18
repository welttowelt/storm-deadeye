# Review — Item 3 (Optimizer Grid-Existence Proptest), Driver A

Driver A's 5,000/5,000 pass rate was a **false positive**: the ground
truth replicated the same unit mismatch the optimizer has, so both
sides agreed on the wrong answer. Re-anchoring the ground truth to
chain semantics surfaced the two pre-existing bugs Reviewer B
identified. Both are now patched and the corrected 5,000-case
proptest passes against the fixed optimizer.

## 1. Diagnosis — Hypothesis A confirmed

Driver A's ground truth and SUT mix the same units:

- `collateral_at` (lines 79–105 of original `grid_existence.rs`)
  returns `verified.collateral` from `normal_collateral`. Per
  `lib.rs:356`, that is the **unscaled** `max(0, −d_min)` with
  `d = g − f` — not the λ-scaled chain charge.
- `expected_value_at` (lines 107–122) returns `λ_g · GPI_g − λ_f · GPI_f`
  — fully λ-scaled.
- `net = ev − coll` (line 163) mixes a λ-scaled EV with an unscaled
  cost. The SUT does the same mix at `normal.rs:239`.

Both compute the *same* fictitious "net". Hypothesis A confirmed.
Hypothesis B (input ranges miss the bug) is rejected: `k=75, σ=0.35`
gives `λ ≈ 83.6`, well inside `k ∈ [1, 1000]`, `var ∈ [0.01, 1000]`.
Hypothesis C is a related framing: the assertion is too weak *because*
both sides share the bug.

Empirical confirmation: corrected ground truth against the
**unmodified** optimizer fails on a shrunk minimum `μ_b=μ_m=0,
σ_m≈9.07, k≈134.5, budget=0.1` — optimizer returns `coll=0.027`, but
chain-frame ground truth says no trade because λ-scaled cost exceeds
0.1 by far.

## 2. Ground-truth correction

Chain semantics (`helpers.cairo:50-176`, `helpers.cairo:198-230`, also
in `optimize_quote_offline`):

- **Cost:** `max(0, λ_f · f(x*) − λ_g · g(x*))` at the audited
  stationary point from `normal_collateral`, `λ = k / ‖p‖₂`.
- **EV:** `λ_g · 𝒩(μ_m; μ_g, σ_b² + σ_g²) − λ_f · 𝒩(μ_m; μ_f, σ_b² + σ_f²)`.
- **Budget filter:** user budget vs. λ-scaled cost.

`tests/grid_existence.rs`:

- `collateral_at` → `lambda_scaled_collateral_at`: runs
  `normal_collateral` for `x*`, then re-evaluates `λ_f f(x*) − λ_g g(x*)`.
- `grid_scan_ground_truth`: cost gate and `net = ev − coll` both in
  λ-scaled frame.
- Assertion: the returned trade is re-evaluated **independently** in
  chain frame; both `chain_net > 0` and `chain_coll ≤ budget` are
  asserted.

## 3. Optimizer patch — both bugs fixed

`src/normal.rs::collateral_number` now returns the λ-scaled chain
charge instead of `verified.collateral`. Single change repairs both
loop failure points:

- **Bug A (budget filter):** `coll > input.budget` now compares
  chain-frame cost to user budget.
- **Bug B (mixed-units selection):** `net = ev − coll` is now
  `λ-scaled EV − λ-scaled cost`.

Public API (`NormalOptimizationInput`, `NormalOptimizationResult`,
`optimize_normal_trade`) is byte-identical; only numeric semantics
shift to chain frame. `cpi-bot` consumes the optimizer exclusively
through `NormalMarket::optimize_quote_offline` / `::optimize_quote`,
both of which already re-scale — `cargo build -p cpi-bot` succeeds
clean. Bot collateral numbers now match what the chain actually
levies. (`deadeye-sdk` had a pre-existing `match-same-arms` clippy
warning in Driver B's no-trade fallback; merged into `Ok(_) | Err(_)`
to keep workspace lint clean.)

## 4. Regression anchor tests added

- `test_budget_filter_must_use_lambda_scaled_cost`: budget=5,
  CPI-style params where σ-arb's λ-scaled cost ≈ 16 STRK. Asserts
  `collateral_required ≤ budget` and rejects the pre-fix path that
  accepted the trade on unscaled cost 0.4 < 5.
- `test_candidate_selection_must_use_lambda_scaled_units`:
  re-evaluates the returned candidate's λ-scaled cost independently;
  asserts agreement with the optimizer's reported value to float
  tolerance. Pre-fix this would disagree by ~200×.
- `low_k_sigma_arb_finds_trade`: confirms the positive case still
  works (low-k σ-shrink remains profitable in chain frame).

The two pre-existing CPI σ-arb tests are renamed to
`..._returns_no_trade_under_chain_pricing` — the σ-arb is correctly
negative-net at chain pricing (`CHAIN_ACCEPTANCE_PARITY.md` §2). The
pre-fix tests were asserting against the bug, not the contract.
`sigma_only_arb_chain_frame_outcomes` (in `grid_existence.rs`)
chain-frame-rewrites Driver A's anchor block: 4 no-trade, 1 trade.

## 5. Final pass count

- **Proptest**: 5,000 / 5,000 pass.
- **Lib tests**: 12 / 12 (was 9; +3 new).
- **Anchor block**: 5 / 5.
- **Total**: 12 lib + 5,000 proptest + 5 anchor = **5,017 tests
  pass**.

## 6. Coverage

`cargo llvm-cov --package deadeye-optimizer --summary-only` post-fix
(brief's `--include-tests` flag doesn't exist in `cargo-llvm-cov
0.8.7`; this is the equivalent Driver A used):

```
Filename     Regions Missed   Cover   Lines Missed   Cover  Fns  Cover
lp.rs           69      0   100.00%    58     0   100.00%   8/8  100%
normal.rs      361     20    94.46%   235    19    91.91%  14/14 100%
TOTAL          430     20    95.35%   293    19    93.52%  22/22 100%
```

`src/normal.rs` is at **91.91 % line / 94.46 % region / 100 %
function** — above the 90 % bar.

## 7. Verdict

The proptest false-passed because its ground truth co-mismatched
units with the SUT. After re-anchoring to chain semantics, the
proptest failed immediately (~1 case in 200), exposing Reviewer B's
two bugs. Fixing `collateral_number` to return the λ-scaled chain
charge restores 5,000 / 5,000 passes and makes the SDK's
`optimize_quote_offline` re-scaling cosmetically redundant — both
layers now agree.

Property tests should drive bug fixes, not paper over them. v0.1.3
delivers on that contract.
