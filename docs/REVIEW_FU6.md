# Review — FU6 Driver B (`effective_k` override + bot integration)

**Verdict.** Ship it. Override validation is tight, the refactor is
parity-correct, the bot integration removes banner-vs-optimizer drift
for the gate path, and the SDK is additive-only at `0.1.2 → 0.1.3`.
One pre-existing in-test doc-comment tripped the strict workspace
clippy posture under `--all-targets --all-features`; fixed inline.

## 1. Validation completeness

`validate_effective_k_override` (`normal.rs:126`) returns
`CoreError::InvalidInput { field: "effective_k_override", … }` when
`!value.is_finite() || value <= 0.0` — one guard, full coverage:

* `0.0`, negative — `..._rejects_zero`, `..._rejects_negative`.
* `NaN`, `±∞` — `..._rejects_non_finite`.
* `MIN_POSITIVE`, `1.0`, `75.07`, `1e9` accepted — `..._accepts_positive`.

Message reads "must be a finite, strictly positive value"; does **not**
echo the offending value. Acceptable — the field name + constraint is
enough; matches the in-repo `InvalidInput` convention.

`CoreError::InvalidInput` already exists (`error.rs:18`); no new
variant, no downstream error-handling churn.

## 2. Refactoring correctness — inner vs outer parity

* `NormalMarket::optimize_quote_inner` (method, `normal.rs:476`) —
  shared by `optimize_quote` (405) and `_with_override` (453).
* `optimize_quote_offline_inner` (free fn, `normal.rs:187`) — shared
  by `optimize_quote_offline` (589) and `_with_override` (634).

Both inners take the **already-resolved** `effective_k: f64`.
Validation lives at the outer wrappers and runs before any `await`
(461, 641). `offline_inner_is_deterministic_in_effective_k` (1147)
pins override = chain-read parity by asserting byte-identical
`NormalTradeQuote` across two same-k calls — the chain path just
supplies `k` from `(params, lp_info)`; downstream math is one code
path. Parity claim is sound.

**Pre-existing chain-path quirk.** `optimize_quote` uses
`Sq128::from_raw(params.k).to_f64()` directly (419); it does **not**
apply `live_effective_k(base, pool, initial)` the way
`optimize_quote_offline` does (TODO at 413). So
`optimize_quote_with_override(k=75.07)` matches `optimize_quote`
against raw `params.k=75.07` bit-for-bit, while the offline override
path matches `live_effective_k`. Net: the bot's `read_effective_k`
override is strictly more accurate than `optimize_quote`'s internal
read — improves correctness, never regresses.

## 3. Bot integration verification

`cpi-bot/src/execute.rs::quote_trade` (156-300):

1. `read_effective_k(cfg, market_felt)` once (181).
2. Branch on `DEADEYE_NORMAL_RUNTIME_ADDR`: runtime present →
   `optimize_quote_with_override(runtime, μ, σ, budget,
   effective_k.effective_k)` (232); absent →
   `optimize_quote_offline_with_override(…)` (261).
3. Re-runs `optimize_normal_trade` for the EV gate using the **same**
   `effective_k.effective_k` (295).

Both branches pass `effective_k.effective_k` correctly. "Saved 1
chain read per quote" is true: `_with_override` skips the internal
`params().await` (and for offline, also `lp_info().await`). The
distribution read is unchanged.

`estimate_gas_for_quote` (312) and `execute_trade_inner` writer
re-derive (636) intentionally keep the legacy `optimize_quote` /
`_offline` calls — post-gate, consistency less critical. Brief endorses.

## 4. Banner-vs-optimizer `k` drift

**Gone in `quote_trade`** — one read, one value, fed to the SDK
override **and** to the parallel EV optimizer that fills `summary.
expected_value` / `summary.edge_ratio`. The QUOTE banner sees exactly
the `k` the optimizer used.

**Remaining (by design):** gas-estimate and writer-bound re-derive
paths still call the original SDK functions, which re-read `params.k`
internally. These paths run *after* the edge-ratio gate, so the
operator-visible drift is gone.

## 5. Edge cases pinned

* **Override == live value.** Determinism test covers it; same inputs
  produce byte-identical `NormalTradeQuote`. No `params()` or
  `lp_info()` call in the `_with_override` body after validation.
* **`MIN_POSITIVE`.** Optimizer never divides by `k` (λ = k / ‖p‖₂);
  tiny `k` ⇒ tiny λ ⇒ near-zero collateral. No div-by-zero.
* **`1e9`.** λ ≲ 5.3e9 at σ=8; × PDF ≤ 0.05 stays well inside f64 /
  Sq128. No overflow.
* **`0`, negative, NaN, ±∞.** Rejected before any `await`, pinned by
  `override_validation_short_circuits_before_chain_read`.

## 6. SDK additivity / CHANGELOG

Original `optimize_quote` / `_offline` signatures unchanged.
`Cargo.toml` at `0.1.3`. CHANGELOG `[Unreleased]` has a clean `###
Added — deadeye-sdk v0.1.3` block naming both new functions, the
validation behaviour, the inner extraction, and the 8 new tests, plus
a `### Notes` subsection calling out the additive / non-breaking
posture explicitly. The bot integration is bundled in the same
section — the brief asked for it as a separate note, but the inline
mention covers the operator-relevant fact.

## 7. Final test count + lint

* **deadeye-sdk `normal::tests`:** 20 passed / 0 failed. Brief said
  19/19 — file actually has 20; `live_effective_k_call_site_wiring_
  matches_chain_after_lp_grow` arrived in the same patch.
* **cpi-bot:** 234 passed / 0 failed (brief said 227; HEAD has 234,
  Driver B subtracted none).
* **Clippy** (`cargo clippy -p deadeye-sdk --all-targets
  --all-features`, `cargo clippy -p cpi-bot --all-targets`): clean
  after one inline fix.

## Inline fix applied

`crates/deadeye-sdk/src/normal.rs:961-969` — test doc on
`live_effective_k_call_site_wiring_matches_chain_after_lp_grow`:
backticked `effective_k` and added a blank line before the trailing
sentence so it's a paragraph, not a malformed list item. Doc-only;
zero behavioural change.
