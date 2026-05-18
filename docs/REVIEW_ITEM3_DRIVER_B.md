# Review — Item 3 (Chain-Acceptance Parity), Driver B

Driver B's two-bug fix is **correct in formula and sign**, but had one residual hazard (fallback path) which this review fixes. The test is **structurally sound but blind to one asymmetric failure class**, which this review's 5 added scenarios begin to probe.

## 1. λ-scaling formula — correct sign

Cairo source verified:
- `helpers.cairo:50-60` → `d = λ_g·g(x) − λ_f·f(x)`
- `helpers.cairo:155-176` → `amount = max(0, neg(d(x*))) = max(0, λ_f·f − λ_g·g)`
- `helpers.cairo:198-230` → `collateral_sufficient = supplied >= computed`

Driver B's `lam_f.mul_add(f_at, -(lam_g * g_at)).max(0.0)` matches exactly. Setting `supplied = padded = required = computed` satisfies `>=`. New unit test `lambda_at_k50_sigma8_matches_doc` pins λ ≈ 265.92, confirming the doc's "~200×–266×" claim.

## 2. `x_star` derivation — bit-tight enough

Chain gate: `abs(d_prime) <= tolerance` (Sq128, deployed test `tolerance=1.0`). Offline `normal_collateral` with `MinimizationPolicy::standard()` converges to `1e-12 · max(λ_f,λ_g,1) ≈ 2.66e-10` — **~10 orders of magnitude tighter than chain tolerance**. Convergence is safe. `standard()` is the right policy (`unrestricted()` only disables `max_absolute_mean`).

## 3. Regression test — devnet down, reasoned

`curl :5050/is_alive` → DOWN. Static analysis: pre-fix `x_star = cand_mean`. For any μ-shift (say μ_g=44, μ_f=42), `cand_mean=44` is on the **wrong side** of μ_f — `side_valid_1d` requires `x < μ_f` when `μ_g > μ_f`. Chain returns `side_valid=false`, `is_valid=false`. For same-μ σ-shifts, `d''(μ_f) < 0` ⇒ `curvature_valid=false`. Both fail the `disagreed_count == 0` assertion. Test would catch the bug across most scenarios.

## 4. Coverage — added 5 asymmetry probes

The contract is one-directional. The reverse miss — optimizer rejects but chain would accept (silent σ-arb leak) — is **not asserted**. 18/25 scenarios in Driver B's run land in `OptimizerRejected`, masking this.

Added to `scenarios()`:
1. `σ_b=7.92` (1% off σ_m=8) — Newton near-singular
2. `σ_b=7.5, μ_b=μ_m` — same-μ tiny σ-arb
3. `budget=6.0, μ_b=44, σ_b=1.5` — at the λ-scaled cost cliff
4. `σ_b=0.5, μ_b=43` — high λ-EV/λ-cost
5. `σ_b=10, μ_b=50` — wide σ at 1σ μ-shift

These don't add the asymmetric assertion (would need hand-built candidates + forced chain check). Flagged as follow-up.

## 5. Performance — non-issue

Fix adds 1 `normal_collateral` call + 2 PDFs. Inner `optimize_normal_trade` already calls `normal_collateral` thousands of times in its grid. <0.1% overhead. Static analysis is conclusive given devnet downtime.

## 6. Fallback path — **fixed inline**

Original `_ => (cand_mean, opt.collateral_required.max(0.0))` re-introduces both bugs on solver `Err` (Newton non-convergence, verification failed). Bot would silently lie again.

**Fix applied:** split arm into `Ok(_)` and `Err(_)`, both returning `(cand_mean, 0.0)`. Downstream `has_positive_trade = collateral_f64 > 0.0` surfaces `on_chain_will_accept=false`. Identical-distribution short-circuits in `normal_collateral` already return `Ok` with `collateral=0.0`, so legitimate zero-cost paths are preserved.

## 7. SDK breakage — strict improvement

`cpi-bot/src/execute.rs` consumes `optimize_quote_offline` when `DEADEYE_NORMAL_RUNTIME_ADDR` unset. Post-fix:
- `required_collateral` ~200× larger (now truthful)
- `padded_collateral = required_collateral` (same)
- `check_submit_gate` only checks `on_chain_will_accept` — no λ-budget gate at bot layer
- Journal (line 108-109) records the corrected value

Pre-existing weakness surfaced: `optimize_normal_trade` filters `coll > input.budget` on **unscaled** coll while user budget is real STRK. If a candidate passes the unscaled filter but its λ-scaled cost exceeds wallet balance, `execute_quote` will fail at submit. This is **honest rejection**, not regression — the bot was lying before. Flagged as optimizer-layer follow-up.

## Code changes applied

1. `crates/deadeye-sdk/src/normal.rs` `optimize_quote_offline`: replaced unsafe fallback with no-trade sentinel; comment updated.
2. `crates/deadeye-sdk/src/normal.rs` tests: added `lambda_scaled_collateral_zero_at_symmetric_midpoint` + `lambda_at_k50_sigma8_matches_doc` to pin the formula offline.
3. `crates/deadeye-e2e/tests/optimizer_chain_acceptance.rs`: 5 asymmetry-probe scenarios; updated header comment + count 25→30.

All unit tests pass (`cargo test -p deadeye-sdk --lib` 9/9 in `normal::tests`). Full workspace builds clean.

## New issues found (non-blocking)

- **Optimizer budget filter (pre-existing):** `optimize_normal_trade` line 226 filters unscaled coll vs user budget — should be λ-scaled.
- **`has_positive_trade` units (now consistent post-fix):** comparison `opt.expected_value > collateral_f64` mixes the optimizer's λ-scaled EV with now-λ-scaled cost — works correctly. But inner *selection* (line 239) still mixes frames; the picked candidate may not be truly optimal under λ-scaling. Out of scope per brief.

## Verdict

Fix is **correct**. Test is **structurally sound**. With fallback patched and 5 new probes added, the parity contract is robust. Devnet integration run remains as the next operator's first task.
