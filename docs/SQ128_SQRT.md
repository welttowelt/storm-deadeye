# `Sq128::sqrt` — Chain-Bit-Exact Q128.128 Floor Square Root

## Why

`NormalDistribution::from_variance` and `LognormalDistribution::from_variance`
used to derive σ as `Sq128::from_f64(variance.to_f64().sqrt())`. f64's
53-bit mantissa rounds the true σ in the last bit for any non-perfect IEEE
square (`0.04`, `0.13`, `100.7`, …). The on-chain `compute_hints_view`
verifies σ via `sqrt_verified(variance, σ)`, which requires
`σ² ≤ variance < (σ + ε)²` *at Q128.128 precision*. f64-derived σ failed
that check for every realistic market-maker variance, returning `Option::None`.

## Mirrored chain algorithm

Cairo source: `the-situation/contracts/src/types/sq128/advanced.cairo:301-352`
(`u512_sqrt`) and `:409-436` (`sqrt`).

Reproduced 1:1 in `crates/deadeye-core/src/sq128.rs::u512_floor_sqrt` /
`Sq128::sqrt`:

1. Shift the magnitude left by 128 bits to form `v = mag << 128` (a ≤384-bit
   value held as `(lo, hi)` u256 pair).
2. Seed Newton from the highest non-zero 128-bit Cairo limb of `v`:
   `2^192` / `2^128` / `2^64` / exact u128 sqrt. Same constants as
   `u512_sqrt`'s seed table.
3. Iterate `g_next = (g + v/g) / 2` using the existing `div_512_by_256`
   helper plus a carry-aware u257-bit add → shift-right-by-1.
4. Stop on `g_next == g` or oscillation (`g_next == prev_guess`); return
   the smaller of the two — the floor sqrt.

No f64 is consulted anywhere in the iteration (the spec mentioned an f64
seed; I dropped it in favour of the chain's exact bit-position seed for
strict bit-parity).

## Bit-parity contract

For all `value ≥ 0` in Sq128:

* `Sq128::sqrt(value)` returns `r` with `r ≥ 0` and
* **`floor(r × r) ≤ value < floor((r + ε) × (r + ε))`**
  where ε is the smallest Sq128 step (magnitude 1) and `×` is `Sq128`'s
  truncating `checked_mul` — identical to the chain's `mul_down`.

This is **exactly** the contract of Cairo `sqrt_verified`. Devnet
confirmed: 20/20 variances pass `compute_hints_view` on the deployed
`normal_math_runtime`.

Negative inputs return `Err(CoreError::InvalidInput)` (matches the chain's
`Option::None` branch for `value.raw.neg`).

## New public API

| Symbol | Crate / file | Purpose |
| --- | --- | --- |
| `Sq128::sqrt(self) -> Result<Sq128, CoreError>` | `deadeye-core::sq128` | Bit-exact floor sqrt; sole public API for off-chain σ derivation. |
| `NormalDistribution::from_sigma(mean, sigma)` | `deadeye-core::distribution` | Canonical MM constructor — variance = σ × σ by construction. |
| `LognormalDistribution::from_sigma(mu, sigma)` | `deadeye-core::distribution` | Same for log-space σ. |

`NormalDistribution::from_variance` and `LognormalDistribution::from_variance`
were rewired to call `Sq128::sqrt`; signatures unchanged, no caller breakage.
The f64 `sqrt_f64` shim was removed (now dead).

## Tests

* `sq128::tests::sqrt_perfect_squares` — bit-exact integer round-trips.
* `sq128::tests::sqrt_fractional_perfect_squares` — `0.25 → 0.5`, `0.5625 → 0.75`.
* `sq128::tests::sqrt_floor_invariant` — `σ² ≤ variance < (σ+ε)²`.
* `sq128::tests::sqrt_arbitrary_variance_satisfies_chain_invariant` — `0.04`.
* `sq128::tests::sqrt_rejects_negative` / `sqrt_of_zero_is_zero`.
* `distribution::tests::{normal,lognormal}_from_variance_round_trip` — preserves variance exactly across `[0.04, 0.09, 0.13, 0.5, 1, 4, 100.7, 1e-6, 1e10]`.
* `distribution::tests::{normal,lognormal}_from_sigma_round_trip` — variance = σ × σ exactly.

## `cargo build --workspace --tests`

Clean (`Finished dev profile`). Only pre-existing warnings on
`deadeye-collateral::probe` and `deadeye-e2e::normal_chaos` Copy-implementation
nags. Driver 2 owns those.

## Devnet `compute_hints_view` parity

`crates/deadeye-e2e/tests/sq128_sqrt_parity.rs`, gated on
`DEADEYE_RUN_INTEGRATION=1`. Picks 20 variances (perfect + non-perfect
squares: `0.04`, `0.13`, `100.7`, `1e-6`, …), constructs
`NormalDistribution::from_variance(0, v)`, calls `compute_hints_view` on
the bootstrapped `normal_math_runtime`.

**Outcome: 20/20 accepted.** Every distribution survived `to_normal`'s
`sqrt_verified` and the runtime emitted valid `l2_norm_denom` /
`backing_denom` hints. The previously-failing `0.04` case now produces
`σ = 0.200000000000000011` (floor sqrt in Q128.128) and clears the chain
check.
