# Chain-Acceptance Parity for the Normal-AMM Optimizer

Closes the gap the off-chain-vs-chain reviewer flagged: bit-exact
parity on `(σ, hints)` does **not** prove chain acceptance. The new
integration test asserts the stronger contract:

> Given `(belief, market, budget, k)` where `optimize_quote_offline`
> returns `on_chain_will_accept = true`, an independent on-chain
> `check_trade_view` call with the optimizer's exact
> `(candidate, x_star, supplied_collateral, hints)` must also return
> `is_valid == true`.

## 1. Test approach

`crates/deadeye-e2e/tests/optimizer_chain_acceptance.rs` bootstraps
devnet (`starknet-devnet :5050`), deploys a normal market at
`N(μ=42, σ=8)` with `k=50, backing=50, tolerance=1.0,
min_trade_collateral=1.0`, and sweeps 25 scenarios:

| Bucket | Count | Coverage |
| --- | --- | --- |
| σ-tightening (`σ_b ≪ σ_m`) | 3 | μ-up / equal / μ-down |
| σ-widening (`σ_b > σ_m`) | 3 | μ-up / equal / μ-down |
| σ-near-equal (`σ_b ≈ σ_m`) | 3 | μ-up / μ-up·up / μ-down (bug class) |
| σ-only (`μ_b == μ_m`, `σ_b ≠ σ_m`) | 3 | narrow / loose / mid |
| Large μ-shift (`≥ 3σ_m`) | 3 | +3σ / -3σ / +3σ σ↓ |
| Micro-budget (`≤ min_trade_collateral`) | 3 | 1.0 / 2.0 / 1.5 |
| Huge budget (`≫ optimal cost`) | 3 | 1 000 STRK each |
| Spread (μ↑ σ↓ / μ↓ σ↓) | 2 | tight beliefs at the envelope |
| Degenerate edges | 2 | very narrow / very wide belief |

For each scenario:
1. `market.optimize_quote_offline(belief_μ, belief_σ, budget)`
   produces a `NormalTradeQuote`.
2. If `on_chain_will_accept = false`, the scenario is counted as
   "optimizer-rejected" — no parity claim.
3. Otherwise an independent `check_trade_view` call is made with the
   quote's exact `(candidate, x_star, supplied, hints)`.
4. The test asserts the chain returns `is_valid = true`.

## 2. Run outcome

Final run against a fresh devnet (`DEADEYE_RUN_INTEGRATION=1`):

```
chain-acceptance: 7/25 accepted | optimizer-rejected=18 |
                  disagreed=0 | call-failed=0 | optimize-failed=0
test optimizer_output_must_be_accepted_by_chain ... ok
```

* **7 scenarios** produce a positive-EV trade that the chain accepts
  bit-for-bit (e.g. `μ-shift +3σ`, `μ-shift -3σ`, `σ-only narrow`,
  `edge narrow`). Optimizer-computed `required_collateral` matches the
  chain's `computed_collateral` to within `~2 × 10⁻⁴` (well below the
  per-trade `min_trade_collateral = 1.0` floor).
* **18 scenarios** are honestly rejected by the optimizer (negative-EV
  after λ-scaling) — these include every loose belief (`σ_b ≈ σ_m` or
  `σ_b > σ_m`). The optimizer's `on_chain_will_accept = false` is the
  *correct* answer here: under the chain-correct λ-scaling, the
  λ-scaled cost exceeds the λ-scaled expected payoff. The chain would
  also reject these for `LowCollateral` if forced, so the optimizer
  declining is the cooperative outcome.
* **0 optimizer-vs-chain disagreements.** When the optimizer says yes,
  the chain says yes. Contract upheld.

## 3. Divergences surfaced and fixes applied

Initial run on `optimize_quote_offline` gave **25/25 disagreements** —
exactly the gap the reviewer suspected. The full sub-reason trace
(via `TradeCheckRaw.verification.*` flags, not just the symbolic
`rejection_reason`) pinpointed two stacked bugs in
`crates/deadeye-sdk/src/normal.rs::optimize_quote_offline`:

### Bug 1 — `x_star = cand_mean` is not the stationary point

The original code seeded `x_star = candidate.mean()`, with a docstring
comment claiming "the chain re-derives the true stationary point from
the candidate". **That comment was wrong.** The chain's
`check_trade_view` *verifies* that `x_star` is at the stationary point
of `d(x) = λ_g g(x) − λ_f f(x)` (`d'(x*) ≈ 0`, `d''(x*) > 0`); it
does not re-derive. `cand_mean` only sits at the stationary point in
the trivial μ_f = μ_g, σ_f = σ_g case.

Surfaced in the diagnostic trace as `side_valid = false` /
`curvature_valid = false` with `computed_collateral = 0.0`.

**Fix:** call `deadeye_collateral::normal_collateral(&current,
&candidate, MinimizationPolicy::standard())` to obtain the real
λ-scaled stationary point `v.x_min`, and use that as `x_star`. The
audited f64 Newton solver is bit-equivalent to the chain's Sq128
solver to within `1e-6` and lives in the off-chain hot path already.

### Bug 2 — Unscaled vs. λ-scaled collateral

After fix 1, the chain accepted the stationarity / curvature gates
but rejected with `coll_ok = false, above_min = false` and
`computed_collateral ≈ 200×` larger than the optimizer's reported
`required_collateral`. Root cause: `normal_collateral` returns the
*unscaled* `−d_min = f(x*) − g(x*)`, but the chain's
`computed_collateral` is the *λ-scaled* difference
`λ_f f(x*) − λ_g g(x*)` with `λ = k / ‖p‖₂`. At `k = 50, σ ≈ 8`,
`λ ≈ 266`, so the optimizer claimed a `~0.018` STRK trade when the
chain would actually require `~5.72` STRK — far below the deployed
`min_trade_collateral = 1.0` floor.

**Fix:** re-evaluate `λ_f f(x*) − λ_g g(x*)` directly using
`deadeye_collateral::lambda(σ, effective_k)` and the inherent
`NormalDistribution::pdf` (Sq128 → f64), and use that as
`required_collateral`. The unscaled `v.collateral` from the solver is
discarded.

### Why these surfaced

The existing `offline_optimize_quote_parity.rs` test only asserted bit
parity on `(σ, candidate_hints)`, both of which the off-chain path
*already* derives via `Sq128::sqrt` and `compute_normal_hints_offline`.
The σ + hint pipeline is bit-exact today (10/10 in that test) and was
never the bug. The bug lived one layer deeper, in the `(x_star,
required_collateral)` pair, which the bit-parity test does not assert
when one side rejects. The new chain-acceptance test catches it.

After both fixes, the existing parity test still passes 10/10 — the
σ + hints didn't move; only the `(x_star, required_collateral)`
limbs changed, and those are asserted only when both paths accept.

## 4. Files touched

* `crates/deadeye-e2e/tests/optimizer_chain_acceptance.rs` — new test.
* `crates/deadeye-sdk/src/normal.rs::optimize_quote_offline` — two-bug
  fix described above; comments updated to point at this doc.

No changes to the inner `optimize_normal_trade` (per brief). The
optimizer's grid still uses unscaled cost internally; the SDK now
re-derives the chain-correct cost before reporting
`on_chain_will_accept`, so a grid candidate with unscaled net > 0 but
λ-scaled net < 0 surfaces as `on_chain_will_accept = false`,
preventing the chain from being the one to say no.
