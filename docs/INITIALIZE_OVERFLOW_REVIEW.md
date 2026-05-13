# `initialize()` `u256_sub Overflow` — Driver 1 review

Reviewer: Driver 2. Date: 2026-05-11.

## 1. Verdict on diagnosis: CONFIRMED

Cairo trace verified:
- `internal/init.cairo:62` → `token/transfer.cairo:179`
  `dispatcher.transferFrom(payer, this, amount)` — the only un-guarded
  ERC20 hop in `initialize()`.
- `normal-math/contract.cairo:81-101`: `internal_gross =
  sq128_to_token_amount_up(backing, internal_decimals)` (×10⁶) →
  `scale_to_native(internal_gross, decimal_shift)` (×10¹²). Net =
  `backing × 10^18`. **No hidden 10¹² factor.**
- The only `-` in `decompose_deposit_amount` is guarded
  (`if total_fee >= token_amount`), ruling out the fee path.
- `min_trade_collateral` is **not** read in `initialize_impl`; only at
  construction (contract.cairo:383) and on trade / `update_parameters`.

`backing=1000`, `token_decimals=18` ⇒ pulls `10^21` against admin's ~966 STRK
⇒ underflow in any standard u256 ERC20.

## 2. Verdict on fix: INCOMPLETE — paper-over for 3 of 4 chaos files

Per-suite max single-actor transfer:

| Suite | Max | Status |
|-|-|-|
| `normal_chaos.rs` | Charlie +750 | safe |
| `lognormal_chaos.rs` | LpLate +750 | safe |
| `multinoulli_chaos.rs` | Cara +100 | safe |
| `bivariate_chaos.rs:716` | **PureLp +1000** | **still underflows** |

`bivariate_chaos.rs:716` does `add_liquidity(sq(1_000.0), …)` from
`participants[2]`. That participant has 1000 STRK predeployed, minus tx gas,
so the call requests `10^21` base units against ≤ ~999 STRK. The driver did
not enumerate per-suite ceilings and missed this.

## 3. Concrete bugs the driver missed

1. **`bivariate_chaos.rs:716`** still requests 1000 STRK in one call. Fix:
   drop to `sq(500.0)`, or top-up the participant. Currently masked because
   the bivariate suite cannot compile (blocker #3).
2. **`FeeConfigRaw` width mismatch — real wire hazard, now fixed.** Cairo
   `onchain-core/common.cairo:104-108` declares each `*_bps` as `u16`; the
   ABI JSONs (e.g. `deadeye-artifacts/abis/normal_amm.abi.json:81`) confirm
   `core::integer::u16`. Rust pre-fix used `u32`. With test values = 0 the
   encoding accidentally agrees, but a Rust caller can construct
   `lp_fee_bps = 70_000` and silently submit a felt Cairo's `u16::Serde`
   rejects mid-deserialize.
3. **Driver's "`cargo build -p deadeye-e2e --tests` PASS"** is wrong on a
   clean cache. Pre-existing signature drift in `normal_chaos.rs:835` /
   `:865` / `:886` and `lognormal_chaos.rs:691` blocks compilation. Verified
   by reverting my change: identical errors at identical lines.

## 4. Code I changed

`crates/deadeye-starknet/src/types/common.rs` — `FeeConfigRaw` fields
`u32 → u16` (x3); `MAX_FEE_BPS: u32 → u16`; `total_bps` widens via casts to
preserve the cap check. Wire format identical (one felt each); construction
of out-of-range values now caught at compile time.

I did **not** modify `lifecycle.rs`. The driver's `backing=50` default + the
`_with_params` escape hatch are sound as far as they go. Skipping balance
validation in `_with_params` is consistent with the no-validation pattern in
the other `upsert_*_profile_for_test` helpers; the validation surfaces at
`initialize_market` (which now carries the gotcha doc-comment).

## 5. Recommendations

1. **Patch `bivariate_chaos.rs:716`**: `sq(1_000.0) → sq(500.0)`, or
   pre-fund the LP participant. Don't call the diagnosis closed until this
   is done.
2. **Unblock chaos compile** (status-doc items 2/3/5): the SDK changed to
   `add_liquidity(share)` / `remove_liquidity(share)` and
   `sell_position_guarded(dist, x_star, hints, guards)`, but the four chaos
   test files still pass the old shapes. Either re-add hint-accepting
   wrappers or strip the trailing args in the tests.
3. **Pre-fund mechanism for production-grade backings**: deploy a
   `MockToken` with `operator_mint` wired to the admin, or `erc20::transfer`
   from unused predeployed slots (only `participants[0..4]` + Eve are
   bound today). This decouples test backing from devnet seed.
4. **Tighten the `_with_params` doc** to say explicitly: no balance
   pre-flight. The current "keep backing × 10^token_decimals ≤ balance"
   note implies the helper might check.
5. **Cover `_with_params`** with at least one passing integration test
   before merge; it compiles but has zero callers today.

Build state after my edits: `cargo build -p deadeye-e2e --tests` fails, but
only on pre-existing SDK/test signature drift (verified by reverting my
change and seeing identical errors at identical lines). My `u16` change is
build-neutral.
