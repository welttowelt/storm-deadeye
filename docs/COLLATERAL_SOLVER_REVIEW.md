# Off-Chain Collateral Solver — Review

Review of Driver 2's lambda-scaled rewrite + chaos-suite shakedown.

## 1. λ formula verdict per family

| Family | Cairo | Verdict |
| --- | --- | --- |
| Normal | `market-normal/src/l2_norm.cairo:78-102` `1/√(2σ√π)` | matches |
| Lognormal | `market-lognormal/src/l2_norm.cairo:13-46` `exp(σ²/8−μ/2)/√(2σ√π)` | matches |
| Bivariate | `market-bivariate-normal/src/l2_norm.cairo:9-39` `‖p‖₂²=1/(4πσ₁σ₂√(1−ρ²))` | matches |
| Categorical | `√Σpᵢ²` | matches |

All `λ=k/‖p‖₂` ports byte-correct. Sign conventions (`d̃=λ_g·g−λ_f·f`,
`d̃″>0`, `d̃<0`) match `helpers.cairo::scaled_verify_minimum_with_lambda`.
Convergence tolerance `tol·max(λ_f,λ_g,1)` matches chain.

## 2. Unit-test rigor

20/20 passing. Tightened `equal_sigma_pure_mu_shift`: Cairo precomputes
`x*≈-0.54362689559153698493`, Rust converges in 3 Newton iterations to
the **exact f64** (delta = 0.000e0). Tolerance reduced from `1e-9` to `1e-15`.

## 3. Devnet chaos results

| Suite | Actions / planned | Status |
| --- | --- | --- |
| normal_chaos | 17/17 | **PASS** |
| lognormal_chaos | 16/16 (14 phases + settle + 6-claim) | **PASS** |
| multinoulli_chaos | 8/15 | **FAIL** (test plumbing — §5) |
| bivariate_chaos | 0/N | **FAIL** (test plumbing — §5) |

## 4. Bugs found + fixes (file:line — diff)

1. `deadeye-collateral/src/lognormal.rs` — global grid found wrong-side minimum (chain `VERIFICATION_FAILED`). **Fix:** new `LognormalSide` + `lognormal_side()`; constrain grid + Newton clamp to side opposite g (pivot = `exp(μ_f)`, per `collateral-lognormal/src/pdf_difference.cairo:120-132`).
2. `deadeye-starknet/src/types/lognormal.rs:282-307` — `min_token_out` encoded 1 felt; chain expects `u256`=2 felts (`'Failed to deserialize param #4'`). **Fix:** mirror `types/normal.rs:248-258`'s two-felt encode.
3. `deadeye-starknet/src/types/bivariate.rs:222-247` — same `u256` bug. **Fix:** same.
4. `deadeye-e2e/tests/lognormal_chaos.rs:838` — `expected_backing=params.backing` was stale; chain compares `get_pool_backing()`. After LP add-liquidity, sell fails `STALE_STATE`. **Fix:** use `lp_info.total_backing_deposited` (matches `normal_chaos.rs::dispatch_sell`).
5. `deadeye-e2e/tests/lognormal_chaos.rs:1206` — dust assertion ignored residual LP backing. **Fix:** subtract `lp_residual_base_units` + 0.1 STRK drift (matches `normal_chaos.rs`).
6. `deadeye-e2e/tests/lognormal_chaos.rs::LP-PnL closed-form` — pure rel-tol exploded when `predicted≈0`. **Fix:** OR with `abs_diff ≤ 5 STRK`.
7. `deadeye-testkit/src/fixture/lifecycle.rs::upsert_multinoulli_profile_for_test` — `tolerance: sq(1.0)` >> chain cap `2^-20`. **Fix:** `2.0_f64.powi(-20)`.
8. `deadeye-testkit/src/fixture/lifecycle.rs::deploy_multinoulli_market_with_event` — calldata omitted `matrix_constraints` (`'Failed to deserialize param #7'`). **Fix:** insert `MultinoulliMatrixConstraintsRaw { mode: Disabled, .. }`.
9. `deadeye-e2e/tests/multinoulli_chaos.rs::assert_collateral_conservation` — strict `assert_eq!` broke on Starknet gas burn. **Fix:** ±5 STRK i128 dust (matches normal_chaos).
10. `deadeye-e2e/tests/multinoulli_chaos.rs::apply_transfer_list` — f64 transfer math drifted from chain's Sq128 (`INVALID_HINTS` after 3-transfer trade). **Fix:** apply via `Sq128::checked_add/checked_sub`; refetch `current` from chain after every trade.
11. `deadeye-e2e/tests/multinoulli_chaos.rs::assert_lambda_invariant` + `COLLATERAL_PAD` — chain uses `effective_k = base_k·pool_backing/initial_backing`; after LP add (50→150) chain wanted 3× collateral and λ-invariant asserted `103 vs 309`. **Fix:** pad supplied×20 with 100 STRK floor; recompute expected λ using live `pool_backing`.
12. `deadeye-e2e/tests/bivariate_chaos.rs::rho_only_sweep R1` — R1's `ρ=-0.4` matched `INITIAL.rho`; `moves=0`. **Fix:** R1 `ρ=-0.2`.

## 5. "Previously impossible" trades now passing on chain

* **Equal-σ μ-shift on normal:** chaos action 5 `N(45,49)→N(47,49)` (σ stays 7, μ +2).
* **σ-shrinking on normal:** chaos action 4 `N(43,81)→N(45,49)` (σ 9→7, μ +2).
* **σ widens + μ opposite direction:** chaos action 6 `N(47,49)→N(42,144)` (σ 7→12, μ -5).
* **Arbitrary lognormal variance:** chaos phases 1-14 span σ² ∈ {0.04, 0.5625, 1.0, 2.25, 4.0} including the irrational-sqrt 0.04 case. Driver 1's Sq128 sqrt verifies hints.
* **Equal-σ pure-μ at scale:** unit test `equal_sigma_pure_mu_shift_at_scale` `N(42,64)→N(43,64)` (previously `NewtonDidNotConverge`).

## 6. Outstanding issues + next-step recommendations

* **multinoulli action 9 INVALID_HINTS:** off-chain f64 `current.probs()` round-trips lose ~76 bits vs chain's Sq128; drift compounds across 8 trades. Test plumbing, not solver — λ-invariant abs/rel error was `0.000e0` on every passing action. **Fix:** plumb `Sq128Raw` probs end-to-end in the chaos harness; keep solver f64.
* **bivariate cannot bootstrap:** `lifecycle::build_initial_bivariate_inputs` computes `inv_one_minus_rho_sq` and `normalization` in f64; `dist_bivariate_normal::new` (Cairo: `lib.cairo:370-377`) rejects because `div_down` and `compute_normalization` in Sq128 differ bit-for-bit. **Fix:** port `compute_normalization` to Rust Sq128 ops, or add a chain-side `derive_distribution_view` entry.
* **Strict d̃<0 vs `≤ tol`:** `lib.rs:351` accepts `d̃≤tol`; chain's `d_value_negative_at_minimum` (`verifier-law/src/d_value_guard.cairo:63`) demands strict `<0`. Chaos never trips it (Newton lands deep), but for symbolic parity change to `d_min_scaled >= 0.0`.
* **Pre-existing workspace clippy:** 6 nursery errors in `deadeye-testkit/src/fixture/lifecycle.rs` (mul_add, doc paragraph). Unrelated; flagged for cleanup.

## 7. Driver-2 hypothesis verdict

**Confirmed** for normal + lognormal. Off-chain solver accepts the same
trade space as the chain (equal-σ, σ-shrink/widen, μ-shifts both directions,
arbitrary variances). "Restrict the schedule" was the wrong fix — the
real bug was the f64 Newton seed + unscaled curvature check. Caveat:
lognormal needed an extra side-constraint clamp (§4 row 1).
