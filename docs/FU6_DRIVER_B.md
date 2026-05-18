# Follow-up #6 — `effective_k_override` for `optimize_quote*`

**Driver B**. **deadeye-sdk v0.1.2 → v0.1.3** (additive, backward-
compatible). cpi-bot integration applied (local-path dep — no bump).

## New public surface

```rust
impl<'p, P: Provider> NormalMarket<'p, P> {
    // Existing — unchanged signatures.
    pub async fn optimize_quote(
        &self, runtime: Felt, belief_mean: f64, belief_sigma: f64, budget: f64,
    ) -> SdkResult<NormalTradeQuote>;
    pub async fn optimize_quote_offline(
        &self, belief_mean: f64, belief_sigma: f64, budget_xp: f64,
    ) -> SdkResult<NormalTradeQuote>;

    // New — caller-supplied effective_k.
    pub async fn optimize_quote_with_override(
        &self, runtime: Felt, belief_mean: f64, belief_sigma: f64,
        budget: f64, effective_k_override: f64,
    ) -> SdkResult<NormalTradeQuote>;
    pub async fn optimize_quote_offline_with_override(
        &self, belief_mean: f64, belief_sigma: f64, budget_xp: f64,
        effective_k_override: f64,
    ) -> SdkResult<NormalTradeQuote>;
}
```

Both new methods call `validate_effective_k_override` before any chain
I/O — `≤ 0`, `NaN`, `±∞` surface `CoreError::InvalidInput` immediately.

## Why Option B (additive) over Option A (breaking)

1. **No churn on existing callers.** deadeye-cli, cpi-bot, and the
   warehouse replay all consume `optimize_quote` today; additive lets
   each migrate on its own schedule.
2. **`Option<f64>` is worst-of-both-worlds.** Forcing `None` at every
   old call site is breaking with no upside; an unconditional `f64`
   is a stronger type-level "opt-in" signal.
3. **Pre-1.0 but published.** `0.1.2 → 0.1.3` is patch-correct semver.

Implementation: private `optimize_quote_inner` (method) plus a free
`optimize_quote_offline_inner` function carry the shared math; both
public entry points forward. Offline inner is a free function so it
isn't monomorphized per `Provider`.

## Tests added (`src/normal.rs`, +8)

1. `validate_effective_k_override_rejects_zero` — `k=0` → `Err`.
2. `..._rejects_negative` — `k<0` → `Err`.
3. `..._rejects_non_finite` — NaN, ±∞ → `Err`.
4. `..._accepts_positive` — `1.0`, `75.07`, `1e9`, `MIN_POSITIVE` `Ok`.
5. `offline_inner_with_override_produces_positive_collateral` — proves
   the override threads to the optimizer + λ-scaled collateral math.
6. `offline_inner_collateral_responds_to_effective_k` — `k=50` vs
   `k=200` give strictly different `required_collateral` (probe
   against accidental hard-coding).
7. `offline_inner_is_deterministic_in_effective_k` — same inputs →
   byte-identical `NormalTradeQuote`. Justifies the "override ==
   chain-read when chain returns same `k`" parity claim.
8. `override_validation_short_circuits_before_chain_read` — public
   `_with_override` validates before any `await`.

19/19 normal-module tests pass (8 new + 11 existing). cpi-bot's 227
tests still pass after the integration.

## Use cases (in doc-comments)

- **Backtest** — replay with historical `effective_k` from a journal
  snapshot.
- **Simulation** — sweep `k` parameter space without LP movement.
- **Offline mode** — bot already reads `k` for its banner; passing it
  through saves a chain read.
- **Testing** — fix `k` without mocking the chain reader.

## Bot integration — DONE

`cpi-bot/src/execute.rs::quote_trade` now calls both
`optimize_quote_with_override` (runtime path) and
`optimize_quote_offline_with_override` (offline path) using the
already-read `effective_k.effective_k`. Wins:

- One fewer chain read per quote (~150ms saved on indexer cache miss).
- QUOTE banner's `k` and the optimizer's `k` are byte-equal — no
  "indexer said 75.07 / SDK re-read 75.13" drift.

`estimate_gas_for_quote` and the writer-bound re-derive in
`execute_trade_inner` still use the original `optimize_quote` — they
run *after* the gate where consistency matters less. A future cleanup
can migrate them.

## Files touched

- `crates/deadeye-sdk/src/normal.rs` — new functions, refactor, +8 tests.
- `crates/deadeye-sdk/Cargo.toml` — `0.1.2 → 0.1.3`.
- `CHANGELOG.md` — v0.1.3 entry.
- `cpi-arb-demo/crates/cpi-bot/src/execute.rs` — call-site migration.
