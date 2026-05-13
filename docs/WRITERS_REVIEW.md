# SDK Writer Review (lognormal / bivariate) + Cross-Family Fixes

Independent validation of Driver 2's lognormal/bivariate writer additions
and the chaos-test wiring, cross-checked against the authoritative ABI
JSONs in `crates/deadeye-artifacts/abis/`.

## 1. Per-method calldata-shape verdict (post-fix)

### Lognormal (`lognormal_amm.rs`)

| method | ABI input(s) | verdict |
|---|---|---|
| `execute_trade` | `(candidate, x_star, supplied_collateral, candidate_hints)` | matches ABI |
| `sell_position_guarded` | `(candidate, x_star, candidate_hints, guards)` | matches ABI (post-fix) |
| `claim` | `()` | matches ABI |
| `claim_for` | `(trader)` | matches ABI |
| `add_liquidity` | `(share_amount)` | matches ABI (post-fix) |
| `remove_liquidity` | `(share_amount)` | matches ABI (post-fix) |

### Bivariate (`bivariate_amm.rs`)

| method | ABI input(s) | verdict |
|---|---|---|
| `execute_trade` | `(candidate, x_star, supplied_collateral, candidate_hints)` | matches ABI |
| `sell_position_guarded` | `(candidate, x_star, candidate_hints, guards)` | matches ABI (post-fix) |
| `claim` | `()` | matches ABI |
| `claim_for` | `(trader)` | matches ABI |
| `add_liquidity` | `(share_amount)` | matches ABI (post-fix) |
| `remove_liquidity` | `(share_amount)` | matches ABI (post-fix) |
| `settle` | `(settlement_point: BivariatePointRaw)` | matches ABI |

Selectors are identical to the normal AMM (Hashed `add_liquidity`,
`remove_liquidity`, `claim`, `claim_for`, `sell_position_guarded`,
`execute_trade`) plus the inline `settle` selector. Confirmed against
all three ABI JSONs.

## 2. The two cross-family bugs Driver 2 propagated

Driver 2 explicitly preserved the `NormalMarketWriter` shape and
acknowledged in doc-comments that the ABI disagrees. The acknowledgement
does not fix anything — Starknet rejects extra calldata and silently
drops missing calldata, so both shapes would revert on devnet.

**(a) `add_liquidity` / `remove_liquidity`.** ABI declares ONE arg
(`share_amount: SQ128x128Raw`, 5 felts). Driver 2's writer (mirroring
the legacy normal writer) encoded `share_amount` + a 2-Sq128 hint pair
(7 felts). Cross-checked against the Cairo source:
`onchain-normal-amm/src/contract.cairo:683`,
`onchain-lognormal-amm/src/contract.cairo:618`,
`onchain-bivariate-amm/src/contract.cairo:583` — all three take only
`share_amount`. Fixed for all three writers + both SDK plumbing
wrappers + all three chaos test call sites.

**(b) `sell_position_guarded`.** ABI declares 4 args: `(candidate,
x_star, candidate_hints, guards)`. `guards` is a flat struct
(`expected_market_dist, expected_backing, expected_tolerance,
expected_min_trade_collateral, min_token_out`) — it does NOT wrap the
preceding three. Confirmed in `onchain-core/src/common.cairo:1629`
(normal), `:1644` (lognormal), `:1654` (bivariate). All three Rust
writers were sending `guards.to_calldata()` only — 3 args silently
missing. Fixed for normal, lognormal, bivariate.

Note: `multinoulli_amm.rs` has the same `sell_position_guarded` bug
(also missing 3 args per `multinoulli_amm.abi.json`) but it was out of
scope for this review.

## 3. Chaos test wiring verdict

- **`lognormal_chaos.rs`**: 3 conversion sites verified. `run_trade`
  uses the new writer correctly. `run_add_liquidity` /
  `run_remove_liquidity` now pass only `share_amount` (hints kept as
  smoke probes — driver bug fixed). `run_sell_all` now passes all 4
  args. The LP rel-tol assertion is active and not trivially passing —
  `realised_total` reads the actual post-claim balance delta via
  `balance_of`, which IS populated end-to-end now that the writers are
  wired.
- **`bivariate_chaos.rs`**: `plan_step` closure structure inspected.
  It is sync, returns the planned `BivariateTradeInput`, then the
  caller awaits the writer at each call site. (a) all 14 trade phases
  feed through `plan_step` + a fresh `BivariateMarketWriter`; no phase
  dropped. (b) no `let _ = …` swallows writer errors — every writer
  call site uses `.await.expect(...)`. (c) each `await` is at the
  outer call site, after the sync planner returns. (d) `prev = snap`
  is reassigned after the post-call snapshot capture, so no race.
  `has_real_helpers()` is now correctly `true`: `initialize_market`,
  `deploy_bivariate_market_with_event`, `fetch_bivariate_hints`,
  `upsert_bivariate_profile_for_test`, and the bivariate writer's
  trade/LP/settle/claim paths are all live.
- **`normal_chaos.rs`**: updated to consume the new normal writer
  signatures (`add_liquidity(share_amount)`,
  `remove_liquidity(share_amount)`, full 4-arg sell). Hint fetches
  retained as smoke probes.

## 4. Code changes applied

1. `crates/deadeye-starknet/src/normal_amm.rs` — `build_sell_call` +
   `sell_position_guarded` take 4 args; `build_add_liquidity_call` /
   `build_remove_liquidity_call` + their async wrappers drop hints.
2. `crates/deadeye-starknet/src/lognormal_amm.rs` — same fix shape.
3. `crates/deadeye-starknet/src/bivariate_amm.rs` — same fix shape;
   `settle` was already correct.
4. `crates/deadeye-sdk/src/lognormal.rs` — plumbing matches new writer
   signatures; imports `LognormalDistributionRaw`.
5. `crates/deadeye-sdk/src/bivariate.rs` — same; imports
   `BivariateNormalDistributionRaw`.
6. `crates/deadeye-e2e/tests/normal_chaos.rs` — `dispatch_sell` passes
   `(candidate, x_star, candidate_hints, guards)`; LP dispatchers drop
   hints.
7. `crates/deadeye-e2e/tests/lognormal_chaos.rs` — `run_sell_all`
   takes `rpc` + `runtime` + fetches `candidate_hints`; LP runners drop
   hints from the writer call (smoke-fetch retained).
8. `crates/deadeye-e2e/tests/bivariate_chaos.rs` — LP seed call drops
   `initial_hints` arg.

## 5. Build state

`cargo build --workspace --tests` → clean (only pre-existing
`missing-copy-implementations` warnings on `normal_chaos.rs`,
unrelated). `cargo build -p deadeye-e2e --tests` also clean.

## 6. Caveats out of scope

- `min_token_out` is declared `u256` in the lognormal/bivariate Cairo
  guards but typed `u128` in the Rust mirrors. Encoding is wrong by 1
  felt for those two families but the bug pre-existed and was not part
  of Driver 2's diff. Flagged for follow-up.
- `multinoulli_amm.rs` shares the `sell_position_guarded` 3-arg
  omission. Out of scope per the review brief but worth a follow-up.
- The bivariate `settle` selector is computed inline via
  `get_selector_from_name("settle")` rather than added to
  `selectors::amm`. Functionally correct; flagged for cleanup.
