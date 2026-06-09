# `initialize()` `u256_sub Overflow` — root cause + fix

Status: **Fix applied** to `crates/deadeye-testkit/src/fixture/lifecycle.rs`.
Date: 2026-05-11.

## TL;DR

The revert is **not** in the AMM. It originates inside the predeployed
STRK token's `transferFrom` implementation, where `_balances[from] -
amount` underflows when the admin's balance is smaller than the amount
the AMM is trying to pull during `initialize()`.

- Profile said: `backing = 1000` (Q128) with `token_decimals = 18`.
- AMM requested: `transferFrom(admin, market, 1000 × 10^18)` =
  `1 000 STRK` exactly.
- Admin had: ~966 STRK (1 000 STRK predeployed − ~34 STRK burned in
  declare + UDC-deploy + factory-config gas).
- `966 − 1 000 < 0` → `u256_sub Overflow` panic from the token contract.

The Cairo AMM is innocent. The Rust SDK encoding is correct. The
**Rust testkit profile was over-budgeted** for the devnet account
balance.

## 1. Call trace from `initialize()` to the underflow site

User invokes `account.execute_v3([Call { to: market, selector:
"initialize", calldata: [] }])` from
`crates/deadeye-testkit/src/fixture/lifecycle.rs:initialize_market`.

The market entry point is on the normal AMM:

```
the-situation/packages/onchain-normal-amm/src/contract.cairo:715
    fn initialize(ref self: ContractState) -> u256 {
        init::initialize_impl(ref self)
    }
```

`init::initialize_impl` body
(`the-situation/packages/onchain-normal-amm/src/internal/init.cairo`):

| Line | Action | Reverts with |
|------|--------|-----|
| 20 | `assert(!is_initialized)` | `'already initialized'` (assert string — not seen) |
| 21 | `enter_non_reentrant` | reentrancy assert |
| 26–29 | `authorized_initializer` gate | `'unauthorized initializer'` (zero ⇒ permissionless, so skipped) |
| 30–32 | Read `initial_backing`, `k`, `distribution` from storage | none |
| 35 | `compute_lambda_view(dist, k)` → `Option<Sq128Raw>` | `'dist violates L2 norm'` if `None` |
| 36 | `from_raw(lambda_raw)` | `'invalid lambda'` if `None` |
| 37 | `assert(lambda > ZERO)` | `'invalid lambda'` |
| 38 | `check_scaled_backing_view(dist, k, backing)` | (no panic — returns struct) |
| 39 | `assert(backing_check.is_valid)` | `'dist violates backing'` |
| 41–48 | `compute_deposit_fees_view(backing, internal_decimals, decimal_shift, zero-fee)` | `'conversion failed'` if `None` |
| 49 | assert no fees emitted | `'invalid state'` |
| 50 | `let token_amount = bootstrap_fees.token_amount;` | none |
| 52–60 | LP-share / claim-component storage writes + lifecycle transition | none |
| **62** | **`deposit(collateral_token, initializer, token_amount)`** | **u256_sub Overflow inside STRK `transferFrom`** |

`deposit` is at
`the-situation/packages/onchain-core/src/token/transfer.cairo:165`:

```cairo
pub fn deposit(token: ContractAddress, payer: ContractAddress, amount: u256) -> u256 {
    if amount.is_zero() { return 0; }
    let dispatcher = IERC20Dispatcher { contract_address: token };
    let this_address = get_contract_address();
    let allowance = dispatcher.allowance(payer, this_address);
    assert(allowance >= amount, 'INSUFFICIENT_ALLOWANCE');
    let balance_before = dispatcher.balanceOf(this_address);
    let success = dispatcher.transferFrom(payer, this_address, amount); // ← reverts here
    ...
}
```

The `transferFrom` call enters the predeployed OZ STRK ERC20 (`0x04718f…c938d`),
which performs an unchecked `u256` subtraction
`_balances[from] - amount` on the payer's balance. When `amount > balance`
this is exactly the `u256_sub Overflow` we observed.

(`compute_deposit_fees_view` itself has only **guarded** subtractions —
see `onchain-normal-math/src/contract.cairo:81-101::decompose_deposit_amount`,
where the only `-` is `token_amount - total_fee` inside an
`if total_fee >= token_amount { 0 } else { … }` gate. So the overflow is
not from the fee path either.)

## 2. Why this trips with the **normal** profile but not the others

The testkit ships a per-family profile installer
(`crates/deadeye-testkit/src/fixture/lifecycle.rs`). Three families used
`backing: sq(50.0)`, but the normal family used `sq(1000.0)`:

```
upsert_normal_profile_for_test       backing = sq(1000.0)  ← the bug
upsert_lognormal_profile_for_test    backing = sq(50.0)
upsert_bivariate_profile_for_test    backing = sq(50.0)
upsert_multinoulli_profile_for_test  backing = sq(50.0)
```

For `token_decimals=18` (STRK), the AMM converts the profile's `backing`
(Q128.128) to base units in `decompose_deposit_amount`:

```
internal_gross = sq128_to_token_amount_up(backing, internal_decimals)
                = backing × 10^internal_decimals          // 6 → ×10⁶
token_amount   = scale_to_native(internal_gross, decimal_shift)
                = internal_gross × 10^decimal_shift       // 12 → ×10¹²
                = backing × 10^token_decimals             // 18 → ×10¹⁸
```

So the request expressed in STRK base units is exactly
`backing × 10^18` for the normal market:

| Profile | Request (STRK) | Admin balance (STRK) | `balance - amount` |
|---------|---------------|----------------------|---------------------|
| `normal` (`backing=1000`) | **1 000.000 STRK** | ~966 STRK | **underflow** |
| `lognormal` / `bivariate` / `multinoulli` (`backing=50`) | 50.000 STRK | ~966 STRK | 916 STRK (ok) |

If you previously tried lowering to `backing=50` and still got
`u256_sub Overflow`, the most likely cause is that the edit didn't reach
the testkit binary on the test run — e.g. running a cached build,
running a test that doesn't go through `upsert_normal_profile_for_test`,
or running with `cargo test --release` after editing without rebuilding
release artifacts. Confirm by `grep -n backing
crates/deadeye-testkit/src/fixture/lifecycle.rs` immediately before
`cargo test`.

## 3. Cross-check: Rust profile encoding vs Cairo struct

I verified `MarketDeployProfileRaw` field-by-field between the Rust SDK
and the on-chain Cairo struct. Both serialize **identically** — same
order, same widths, same Cairo `Serde` semantics:

| Field | Cairo type | Rust type | Match |
|-------|------------|-----------|-------|
| `market_type` | `u8` | `u8` | ✓ |
| `collateral_token` | `ContractAddress` | `Felt` | ✓ |
| `token_decimals` | `u8` | `u8` | ✓ |
| `internal_decimals` | `u8` | `u8` | ✓ |
| `k` | `SQ128x128Raw` (`u64;u64;u64;u64;bool`) | `Sq128Raw` (same) | ✓ |
| `backing` | `SQ128x128Raw` | `Sq128Raw` | ✓ |
| `tolerance` | `SQ128x128Raw` | `Sq128Raw` | ✓ |
| `min_trade_collateral` | `SQ128x128Raw` | `Sq128Raw` | ✓ |
| `fee_config` | `FeeConfigRaw` (`u16;u16;u16`) | `FeeConfigRaw` (`u32;u32;u32`) | width mismatch but compatible¹ |
| `extension` | `ContractAddress` | `Felt` | ✓ |
| `extension_call_points` | `u16` | `u16` | ✓ |
| `payout_amplifier` | `SQ128x128Raw` | `Sq128Raw` | ✓ |

¹ Cairo `u16::Serde` and Rust `u32::encode` both serialize as a single
felt (Cairo widens via `Felt::from`). On the wire the values are zero so
no truncation issue. This is worth tracking as a latent type-precision
hazard but is **not** the bug here.

So the previous-cited `decimal_shift: u8` fix (item 3 in
`CHAOS_SUITE_STATUS.md` "Notable bugs") was correct and stays correct.
There's no analogous mis-typed field surfacing right now.

## 4. Parity reference: the TS SDK

The TypeScript e2e fixtures
(`the-situation-sdk/packages/test-e2e/src/setup/fixtures.ts`) initialize
with very small backing values: `STANDARD_FIXTURE` uses `backing: sq(10)`,
`MINIMAL_FIXTURE` uses `backing: sq(5)`. They run against the same OZ
ERC20 abstraction. The Cairo factory tests
(`packages/factory/src/tests/test_factory_contract.cairo`) likewise use
small backings and a fully funded snforge test caller.

Our testkit pulled the value `1000` from production-style configs;
production-style accounts on mainnet hold far more than 1 000 STRK
so the underflow never surfaced there.

## 5. Fix applied (Rust)

Apply the same parity as the other three families: lower the normal
profile's backing from `sq(1000.0)` to `sq(50.0)` and add a parameterised
helper for callers who need a different value.

Patch:

```
--- a/crates/deadeye-testkit/src/fixture/lifecycle.rs
+++ b/crates/deadeye-testkit/src/fixture/lifecycle.rs
@@ -98,7 +98,7 @@
 pub async fn upsert_normal_profile_for_test<A>(
     ...
     ) -> Result<(), LifecycleError>
 {
     let params = DeployProfileParams {
         ...
         k: sq(50.0),
-        backing: sq(1000.0),
+        backing: sq(50.0),
         tolerance: sq(0.001),
         ...
     };
```

In addition, `initialize_market`'s doc-comment now spells out the gotcha
(caller must hold ≥ `backing × 10^token_decimals` base units), and a new
`upsert_normal_profile_for_test_with_params` is exposed for tests that
need a custom `(k, backing, internal_decimals, token_decimals)`.

The Cairo source was **not** modified.

## 6. Verification

Build (sandboxed):

```
cargo build -p deadeye-testkit --tests           # PASS (clean)
cargo build -p deadeye-e2e --test normal_full_lifecycle  # PASS
cargo build -p deadeye-e2e --test normal_chaos    # PASS (pre-existing 2 cosmetic warnings)
cargo build -p deadeye-e2e --test lognormal_chaos # PASS
cargo build -p deadeye-e2e --test multinoulli_chaos # PASS
cargo build -p deadeye-e2e --test normal_lifecycle ... # PASS
```

Pre-existing unrelated error: `bivariate_chaos.rs:927` references
`drive_step` (renamed elsewhere to `plan_step`). Not introduced by this
change and outside the scope of the diagnosis.

Devnet run not executed in this session (auto-mode sandbox; no devnet
process). After ensuring `starknet-devnet --seed 0 --accounts 10 --port
5050` is up, the canonical reproduction is:

```
DEADEYE_RUN_INTEGRATION=1 cargo test -p deadeye-e2e \
    --test normal_full_lifecycle -- --nocapture
```

Expected outcome with the fix: `initialize_market` succeeds (admin pulls
50 × 10^18 base units = 50 STRK, leaving admin at ~916 STRK), and the
test proceeds to the SDK `distribution()` read and the trade step. Note
that subsequent steps in `normal_full_lifecycle.rs` may still fail on
unrelated wiring (the test was originally drafted alongside the chaos
suite which is still gated on additional writer plumbing — see
`CHAOS_SUITE_STATUS.md` items 2–5), but the `initialize()`
`u256_sub Overflow` blocker is removed.

## 7. Risk / follow-ups

- The `FeeConfigRaw` Rust width (`u32` per field) vs Cairo (`u16` per
  field) is silently compatible only because the tested values fit in
  `u16`. Tighten the Rust struct to `u16` to make the boundary
  bit-exact — small refactor, not blocking.
- The bootstrap helper accepts an `admin_initial_collateral` config
  field that's effectively dead code with predeployed STRK (we don't
  mint into the admin). Either wire it to a `transfer` from another
  predeployed account or delete the field.
- Production profiles routinely use `backing >> 50`. Any future test
  that exercises a production profile must pre-fund the admin via
  `erc20::transfer` from a richer predeployed account or by deploying
  the restricted-collateral token with `operator_mint`. The new
  `upsert_normal_profile_for_test_with_params` is the entry point for
  that path.
