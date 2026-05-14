# Offline Preflight Review — `NormalMarket::optimize_quote_offline`

**Verdict:** code is correct on the dimensions Driver B claims (σ + hints +
`effective_k` formula are chain-bit-exact). One semantic divergence in
`on_chain_will_accept` is worth flagging. Clippy was dirty on three files —
fixed.

## 1. `SQRT_PI_RAW` — MATCH

`crates/deadeye-sdk/src/normal.rs:67-73` vs.
`contracts/src/market/normal/constants.cairo:45-47`:
```
limb0 = 0xC3B0520D5DB9383F, limb1 = 0xC5BF891B4EF6AA79,
limb2 = 0x1, limb3 = 0x0, neg = false
```
Byte-for-byte identical. Floor-encoded √π, strict lower bound.

## 2. `live_effective_k` — MATCH (one fallback nit)

Cairo `markets/normal/math/contract.cairo:161-182`:
```
scaled   = mul_down(base_k, pool_backing)
scaled_k = div_down(scaled, initial_backing)
return max(scaled_k, base_k)
```
Rust uses `Sq128::checked_mul` (right-shift = floor for positives) and
`checked_div` (integer floor for positives). Operator order + rounding
direction match.

**Minor:** Cairo returns `Option::None` when `initial == 0`, which
`state.cairo:215` unwraps via `.expect('k scaling failed')` — hard panic.
Rust silently falls back to `base_k`. Fine off-chain (the on-chain trade
would panic anyway), but worth noting.

## 3. 10-input parity test — passes, but acceptance is one-sided

`crates/deadeye-e2e/tests/offline_optimize_quote_parity.rs`. 10 diverse
scenarios; assertion is bit-exact on `(μ, σ², σ, l2_norm_denom,
backing_denom)`. Independent re-run:
```
🔍 parity: 10/10 scenarios matched
test result: ok. 2 passed; 0 failed
```
**Caveat:** for all 10 scenarios `chain accept=false | off accept=true`.
The chain's `check_trade_view` rejects every off-chain "positive-EV"
candidate (likely policy-envelope / min-trade-collateral on a fresh
deployed market). The test only asserts collateral when *both* paths accept
(lines 203-207), so chain rejections never fail the test. Bit-exact
`(σ, hints)` parity is real; "chain will accept" parity is **not** proven
here. Off-chain `on_chain_will_accept` means "optimizer found EV > cost",
not "chain admits the trade".

## 4. cpi-bot integration — clean

`cpi-arb-demo/crates/cpi-bot/src/execute.rs:194-225`: off-chain branch
calls `optimize_quote_offline(belief_μ, σ_b, budget)`. `75.07` survives
only in doc comments. Flow: bot → `optimize_quote_offline` → submit (no
`compute_hints_view` round-trip). `on_chain_will_accept` plumbed straight
through.

## 5. Live mainnet quote — PASSES
```
μ_market=4.29 σ_market=0.35 | μ_model=4.3274 σ_model=0.2143
candidate: μ=4.318 σ²=0.047852 σ=0.2188
on_chain_will_accept = true; required=0.3227 XP
```
Real σ-arb edge captured (model σ tighter by 1.63×).

## 6. Clippy fixes (mine, minimal)

`cargo clippy --workspace --all-targets -- -D warnings` was failing on:
- `crates/deadeye-optimizer/examples/sigma_arb_probe.rs` — added
  `#![allow(clippy::print_stdout)]` (dev binary; stdout *is* the UX) +
  back-ticked identifiers in module docs.
- `crates/deadeye-optimizer/examples/sigma_arb_debug.rs` — same allow set
  + `unwrap_used` / `suboptimal_flops`; parenthesised `s1*s1 + s2*s2`.
- `crates/deadeye-optimizer/src/normal.rs:305-310` — back-ticked
  identifiers in a doc comment.

Workspace clippy clean in both deadeye-rs and cpi-arb-demo.

## Bottom line

- σ + hints + `effective_k` formula: chain-bit-exact.
- 10/10 parity holds on independent re-run.
- Live cpi-bot quote against mainnet: passes, positive-EV σ-arb at
  `(μ=4.318, σ=0.2188)`, collateral 0.32 XP.
- Caveat: `on_chain_will_accept=true` from `optimize_quote_offline`
  encodes "optimizer found EV > cost", not "chain admits the trade".
  Driver B's docstring (`normal.rs:344-360`) is candid about this; the
  parity-test framing is not. Recommend a follow-up that submits one of
  the 10 off-chain "accepted" candidates and inspects the chain rejection
  enum.
