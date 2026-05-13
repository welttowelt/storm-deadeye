# Off-Chain Collateral Solver — Lambda-Scaled Rewrite

## 1. Cairo source: lambda formula and scaled minimisation

* **Lambda** `λ = k / ‖p‖₂`:
  `the-situation/packages/market-normal/src/invariant.cairo:65-86`.
* **Normal `‖p‖₂`** `1 / √(2σ√π)`:
  `the-situation/packages/market-normal/src/l2_norm.cairo:78-102`.
* **Lognormal `‖p‖₂`** `exp(σ²/8 − μ/2) / √(2σ√π)`:
  `the-situation/packages/market-lognormal/src/l2_norm.cairo:13-46`.
* **Bivariate `‖p‖₂`** `√(1 / (4π σ₁ σ₂ √(1−ρ²)))`:
  `the-situation/packages/market-bivariate-normal/src/l2_norm.cairo:9-39`.
* **Scaled minimum verifier** `scaled_verify_minimum_with_lambda`:
  `the-situation/packages/onchain-normal-math/src/helpers.cairo:190-230`.
  Verifies `λ_g·g − λ_f·f` is stationary, has `d''>0`, and `d<0` at `x*`.

## 2. Newton iteration: before / after

| Step | Old (`g − f`) | New (`λ_g·g − λ_f·f`) |
| --- | --- | --- |
| Seed | `μ_f.min(μ_g)` or σ-weighted midpoint | Coarse grid scan (96 samples, ±6σ_max⊕μ-span) on the **scaled** difference |
| Step | `Δx = d'/d''` raw | `Δx = clamp(d'/d'', ±0.5·max(1,|x|))` — mirrors Cairo `clamp_step` (`packages/newton/src/lib.cairo:142-153`) |
| Convergence | `|Δx| < tol` | `|d'| < tol·max(λ_f, λ_g, 1)` — matches Cairo |
| `d''` gate | `f64::EPSILON` | `2⁻⁶⁰` — matches Cairo `MIN_SECOND_DERIV_RAW` |
| Post-check | unscaled `d'`, `d''`, `d` | scaled `d̃'`, `d̃''`, `d̃` |

The grid seed is the load-bearing fix. Equal-σ and σ-shrink trades have
proper minima of `d̃` only inside a narrow curvature basin around `μ_f`.
The old `μ_f − 2σ_f` start lands in a region where `d̃''<0` so Newton
drifts into the tail; the grid finds the right basin in O(N) work.

## 3. New public API

```rust
impl MinimizationPolicy {
    pub const fn standard() -> Self;     // signature unchanged; envelopes = ∞
    pub const fn unrestricted() -> Self; // NEW — all envelopes = ∞
    pub const fn relaxed() -> Self;      // alias for standard()
}
pub fn lambda(sigma: f64, k: f64) -> f64;
pub fn lognormal_lambda(mu, var, k) -> f64;          // NEW
pub fn lognormal_l2_norm(mu, var) -> f64;            // NEW
pub fn bivariate_lambda(σ1, σ2, ρ, k) -> f64;        // NEW
pub fn bivariate_l2_norm(σ1, σ2, ρ) -> f64;          // NEW
```

Envelope variants (`SigmaRatioTooLarge`, ...) remain in
`PolicyRejection` but never fire under the default policy.

## 4. Unit tests

`cargo test -p deadeye-collateral` — **20 passed, 0 failed**.

* `equal_sigma_pure_mu_shift` `N(0,1)→N(1,1)` — converges to `x* ≈ −0.5436` within 1e-9 of the precomputed Cairo limbs.
* `equal_sigma_pure_mu_shift_at_scale` `N(42,64)→N(43,64)` — previously `NewtonDidNotConverge`.
* `shrinking_sigma` `N(0,4)→N(0,1)` — previously `NotPositiveCurvature`.
* `shrinking_sigma_with_mu_shift` `N(43,81)→N(45,49)` — previously rejected.
* `widening_sigma_opposite_mu` `N(45,100)→N(38,144)` — previously rejected.
* `degenerate_round_trip` `N(42,64)→N(42,64)` — identity fast path.
* `standard_policy_accepts_large_sigma_ratio` — previously gated.

## 5. Devnet outcome

`DEADEYE_RUN_INTEGRATION=1 cargo test -p deadeye-e2e --test
normal_full_lifecycle` against a live `starknet-devnet` on :5050: the
bootstrap pipeline succeeded (factory + 4 plugins + ERC-20). The
`normal_market_full_lifecycle` test then failed during **market deploy**
with `'initial backing invalid'`. This is a chain-side precondition in
the deployer constructor — unrelated to the off-chain solver, which is
never consulted before `execute_trade`. The failure reproduces on a
clean checkout of `main`.

## 6. Chaos schedules

Restored mixed-direction transitions in `normal_chaos.rs` and
`lognormal_chaos.rs` — equal-σ μ-shifts, σ-shrinks, opposite-direction
trades. `assert_sigma_safe` no longer gates on σ ratio or mean
separation. `solve_trade` calls `MinimizationPolicy::unrestricted()`.
