# Review — P4 #67 fix (Driver A): chain-runtime `optimize_quote`

Scope: `deadeye-sdk v0.1.4`. Verified against
`crates/deadeye-sdk/src/normal.rs::optimize_quote_inner` vs
`optimize_quote_offline_inner`, the parity sweep in
`crates/deadeye-e2e/tests/optimizer_chain_acceptance.rs`, and the
`CHANGELOG.md` Unreleased entry.

## 1. Byte-equal-to-offline claim — partial; CHANGELOG updated

Line-by-line diff of the math sections (offline 187-275 vs chain 490-547):

| Step                                 | Offline                                | Chain-runtime                       | Match |
| ------------------------------------ | -------------------------------------- | ----------------------------------- | ----- |
| `market_mean`/`market_sigma`         | `current.{mean,sigma}().to_f64()`      | same                                | YES   |
| `optimize_normal_trade(..)`          | identical args                         | identical args                      | YES   |
| `cand_mean = Sq128::from_f64(.)`     | line 213                               | line 514                            | YES   |
| `cand_variance`                      | line 214                               | line 515                            | YES   |
| `from_variance`                      | `NormalDistribution::from_variance`    | `deadeye_core::NormalDistribution::from_variance` (same fn, FQ path) | YES |
| `sigma_f_f64`/`sigma_g_f64`          | lines 252-253                          | lines 533-534                       | YES   |
| `normal_collateral(.., standard())`  | line 255                               | line 536                            | YES   |
| λ-scaling (`lam_f`, `lam_g`, x_q, f_at, g_at, `mul_add(.. -lam_g*g_at).max(0)`) | lines 259-264 | lines 538-543 | YES |
| No-trade fallback `(cand_mean, 0.0)` | line 274                               | line 546                            | YES   |

The math producing `(x_star_local, collateral_f64)` is **byte-identical**.

**Caveat:** the returned `NormalTradeQuote.required_collateral` is **not**
byte-identical. The chain path forwards `supplied = Sq128::from_f64(collateral_f64)`
to `reader.quote_trade(..)`, and `quote_trade` (in
`crates/deadeye-starknet/src/normal_amm.rs:165-173`) returns
`required_collateral = check.verification.computed_collateral` — the
chain's Sq128 re-computation, not the optimizer's local value. The
offline path returns the optimizer's local value directly. The two
agree to within 1 ULP, as already pinned by
`offline_optimize_quote_parity.rs:171-172` (`ulp = max(|a|,|b|,1) *
1e-9`), but the strict "byte-identical (x_star, required_collateral)"
phrasing in the CHANGELOG was inaccurate. **CHANGELOG amended** (see §3).

`x_star` is byte-identical between the two paths because the chain
forwards `x_star = quote.x_star` unchanged into the returned
`NormalTradeQuote` (normal_amm.rs:168).

## 2. Unit test added

No pre-existing unit test exercised `optimize_quote_inner` (the
function is async + Provider-bound; the math is duplicated, not
extracted to a free function). The CHANGELOG's only proof was Driver
B's devnet-gated parity test at
`optimizer_chain_acceptance.rs:512`, which runs only when
`DEADEYE_RUN_INTEGRATION=1`.

Added two regression pins to `crates/deadeye-sdk/src/normal.rs::tests`:

- `offline_inner_x_star_matches_normal_collateral_not_cand_mean` —
  reconstructs the math the inner runs, asserts `x_star ==
  normal_collateral(..).x_min` and `x_star != cand_mean`. Pin is
  non-vacuous: the assert pair fires if either inner regresses to
  `x_star = cand_mean`.
- `offline_inner_no_trade_fallback_is_cand_mean_zero_collateral` —
  pins the no-trade sentinel `(cand_mean, 0)` via `belief == market`.

Both target the offline inner because that's where the math is
testable without a Provider mock. The chain-runtime inner runs the
same math (verified §1) and forwards `x_star` unchanged, so either
regresssion is caught. Extracting `optimize_quote_inner`'s math to a
free function is the right v0.2 follow-up — flagged, not done now.

## 3. CHANGELOG accuracy

Pre-review wording claimed "byte-identical `(x_star,
required_collateral)`". `x_star` is byte-identical; the offline
`required_collateral` round-trips through f64 while the chain
returns the chain's Sq128 `computed_collateral`. Edited
`CHANGELOG.md` to call out the byte-identical math up to the chain
hand-off, then explain the 1-ULP residual in
`required_collateral`. The fix's substantive contract (`x_star`
correctness, λ-scaled collateral, no-trade fallback) is preserved.

## 4. Side-effect check

`optimize_quote` (chain-runtime) callers outside the parity test:
- `crates/deadeye-cli/src/commands/trade.rs:88` — CLI `trade`
  command. No hardcoded expectations on `x_star`/`required_collateral`.
- `crates/deadeye-e2e/tests/offline_optimize_quote_parity.rs:149` —
  compares chain vs offline; already tolerates 1 ULP and the new
  `x_star` matches both sides.
- `crates/deadeye-e2e/tests/optimizer_chain_acceptance.rs:605` —
  Driver B's new test; verified independently in §5.

No pinned `(x_star, collateral)` expectations broke. Workspace
clippy `-D warnings` clean. Workspace lib + integration tests pass.

## 5. Driver B's devnet parity test

`optimize_quote_chain_runtime_must_be_accepted_by_chain` —
verified:
- Uses the same `scenarios()` (30 entries) as the offline twin —
  consistency confirmed.
- Asserts `disagreed_count == 0` (line 728) and `accepted_count > 0`
  (line 734-737).
- Passes `env.normal_runtime` (real address, not `Felt::ZERO`) on
  line 601, so it routes through the chain-runtime path.
- Located at the end of the file (line 511, file ends line 738),
  not interleaved.
- Gated via `integration_enabled()` (`DEADEYE_RUN_INTEGRATION=1`)
  rather than `#[ignore]` — same convention as
  `optimizer_output_must_be_accepted_by_chain`. The env-var gate is
  greppable and consistent with the rest of `deadeye-e2e`.

## 6. cpi-bot revert (§6 of brief)

`cpi-arb-demo/.../devnet_e2e_failure_modes.rs` is **not in this
workspace** — it's a sibling repo. The reference at
`optimizer_chain_acceptance.rs:484-487` is purely historical context;
no in-tree change required.

## 7. Offline-was-broken-before-FU2

Confirmed via the in-tree
`optimize_quote_offline_inner` doc-comment (lines 221-247): the
"two fixes vs. the previous heuristics" wording explicitly states
the pre-FU2 offline was emitting `x_star = cand_mean` + unscaled
collateral. Pre-FU2, **both** paths had the bug; FU2 fixed offline,
P4 #67 fixed chain-runtime — completing the pair.

## 8. Final test count

`deadeye-sdk` lib tests: pre-fix **44 passed**, post-fix **46 passed**
(two new pins added in `normal::tests`).
`cargo test --workspace`: **253 passed, 0 failed, 18 ignored**
across 53 test binaries.
`cargo clippy --workspace --all-targets -- -D warnings`: clean.
The new pins fail closed on regression (asserts `x_star ≠ cand_mean`
and `x_star == normal_collateral(..).x_min`).
