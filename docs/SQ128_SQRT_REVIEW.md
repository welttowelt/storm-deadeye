# `Sq128::sqrt` — Review

## 1. Bit-exact verdict: **confirmed**

The Rust `u512_floor_sqrt` (`crates/deadeye-core/src/sq128.rs:478-545`) is
bit-for-bit identical with Cairo `u512_sqrt`
(`the-situation/contracts/src/types/sq128/advanced.cairo:301-352`).

* **Seed table** (`sq128.rs:488-506`): Cairo uses four branches keyed on
  which 128-bit `u512` limb is highest non-zero. Rust translates by checking
  whether the corresponding *pair* of u64 limbs is non-zero, with the same
  four seeds (`2^192`, `2^128`, `2^64`, exact u128 root).
* **Newton step**: Cairo computes `sum = a + b` as a u512 with `limb2 =
  carry`, then `u256_shr_1` shifts the u513 right by 1 to land in u256.
  Rust uses `overflowing_add` + a manual carry re-insert at bit 255
  (`sq128.rs:530-536`), producing the same result.
* **Convergence**: Both stop on `new_guess == guess || new_guess ==
  prev_guess`, then return `min(new_guess, guess)` (the floor).
* **Negative input**: Cairo `sqrt` returns `Option::None`; Rust returns
  `Err(InvalidInput)` — behavioural equivalent.
* **`div_512_by_256` divergence is unreachable from `Sq128::sqrt`**: Cairo
  silently truncates a u512 quotient to u256; Rust returns `Err` and forces
  `guess = U256::MAX`. The divergence only fires when the true quotient
  exceeds u256, which requires the `cairo_limb3_nonzero` seed branch. In
  `Sq128::sqrt`, the input is `magnitude << 128` with `magnitude ≤ U256::MAX`,
  so `hi ≤ 2^128 − 1` — the limb3 branch is dead. Confirmed.

## 2. Coverage gaps (added)

* `sqrt_of_one_ulp_is_floor` — smallest positive Sq128 (`mag = 1`); asserts
  `r.magnitude() == U256::from_limbs([0, 1, 0, 0])` (= `2^64`) limb-for-limb.
* `sqrt_of_max_sq128_does_not_panic` — `U256::MAX` magnitude; asserts the
  exact chain `gap < 2σ + ε` bound.
* `sqrt_regression_0_04_matches_chain_sigma` — explicit regression test for
  the previously-failing `variance = 0.04` case; asserts the bit-exact
  invariant *and* that our σ ≤ the f64-mediated σ (i.e. we floor, not round
  up like f64 did).

The driver's original `sqrt_floor_invariant` test used the stricter
`(σ + ε)² > variance` bound, which is wrong for very small σ because
`mul_down` truncation collapses adjacent squares. The new tests use the
correct chain invariant (`gap < 2σ + ε`), matching `sqrt_verified` exactly.

## 3. Devnet parity test count

* `sq128_sqrt_parity_against_normal_runtime`: **20/20** accepted.
* New `sq128_sqrt_parity_stress_sweep` (100 variances spanning `2^-50` …
  `2^50`, each decade paired with a `π/3` irrational neighbour): **100/100**
  accepted. No rejections.

## 4. Bug fixes applied

None to `Sq128::sqrt` itself — the implementation is bit-correct.

Test-suite fixes (one-line diff summary):

* `crates/deadeye-core/src/sq128.rs`: add three sqrt tests
  (`sqrt_of_one_ulp_is_floor`, `sqrt_of_max_sq128_does_not_panic`,
  `sqrt_regression_0_04_matches_chain_sigma`), all asserting the exact
  chain invariant `gap < 2σ + ε`.
* `crates/deadeye-e2e/tests/sq128_sqrt_parity.rs`: add 100-variance stress
  sweep (`sq128_sqrt_parity_stress_sweep`) hitting the same
  `compute_hints_view` endpoint as the 20-case test.

## 5. Integration-test honesty

`sq128_sqrt_parity.rs` iterates all 20 variances in a `for`-loop with no
early returns; `passed` only increments on `Ok(hints)`; the final
`assert_eq!(passed, total)` fails the test if any variance is rejected.
`fetch_normal_hints` returns `Err` when `compute_hints_view` returns
`None`, so a chain rejection of our σ propagates as a test failure. The
gate (`DEADEYE_RUN_INTEGRATION=1`) is checked once at test entry and skips
cleanly with an `eprintln!` if unset. Honest.
