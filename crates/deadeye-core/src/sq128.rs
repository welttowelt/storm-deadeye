//! Signed Q128.128 fixed-point numbers — the canonical numeric format used
//! by every Deadeye on-chain contract.
//!
//! # Representation
//!
//! A [`Sq128`] is stored as a [`U256`] magnitude plus a sign flag. The scale
//! factor is `2^128`, so the integer value `x` represents the rational
//! `x / 2^128`. The wire format ([`Sq128Raw`]) decomposes the magnitude into
//! four little-endian 64-bit limbs to match the Cairo `SQ128x128Raw` struct
//! used by Starknet RPC calldata.
//!
//! # Guarantees
//!
//! * **No silent truncation** — every fallible operation returns
//!   `Result<Self, CoreError>` and never panics on numeric edges.
//! * **Bit-identical limb encoding** — round-tripping `Sq128 -> Sq128Raw ->
//!   Sq128` is the identity for every representable value (property-tested).
//! * **Canonical zero** — `-0` and `+0` compare equal; both serialise to
//!   `neg = false`.

use core::{
    cmp::Ordering,
    fmt::{self, Debug, Display, Formatter},
    ops::Neg,
};

use ruint::aliases::U256;

use crate::error::CoreError;

/// Number of fractional bits in the Q-format.
const SCALE_BITS: u32 = 128;

/// `2^128`, the Q-format scale.
const SCALE: U256 = U256::from_limbs([0_u64, 0_u64, 1_u64, 0_u64]);

/// `2^64` as `f64`. The literal form is rejected by clippy as inexact, but
/// `2^64` is exactly representable in IEEE-754 (it's a power of two); we
/// spell it via `from_bits` to make that explicit.
const F64_2_POW_64: f64 = f64::from_bits(0x43F0_0000_0000_0000_u64);
/// `2^128` as `f64`. Exact (power of two), spelled via `from_bits`.
const F64_2_POW_128: f64 = f64::from_bits(0x47F0_0000_0000_0000_u64);

/// Wire-format limb decomposition of a magnitude — four little-endian 64-bit
/// limbs plus an explicit `neg` flag. Mirrors the Cairo `SQ128x128Raw` struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sq128Raw {
    /// Bits 0..64 of the magnitude.
    pub limb0: u64,
    /// Bits 64..128 of the magnitude.
    pub limb1: u64,
    /// Bits 128..192 of the magnitude.
    pub limb2: u64,
    /// Bits 192..256 of the magnitude.
    pub limb3: u64,
    /// `true` if the value is strictly negative.
    pub neg: bool,
}

impl Sq128Raw {
    /// Wire representation of `0`.
    pub const ZERO: Self = Self {
        limb0: 0,
        limb1: 0,
        limb2: 0,
        limb3: 0,
        neg: false,
    };
    /// Wire representation of `1`.
    pub const ONE: Self = Self {
        limb0: 0,
        limb1: 0,
        limb2: 1,
        limb3: 0,
        neg: false,
    };
    /// Wire representation of `-1`.
    pub const NEG_ONE: Self = Self {
        limb0: 0,
        limb1: 0,
        limb2: 1,
        limb3: 0,
        neg: true,
    };
}

/// Signed Q128.128 fixed-point number.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sq128 {
    magnitude: U256,
    negative: bool,
}

impl Sq128 {
    /// `0` in Q128.128.
    pub const ZERO: Self = Self {
        magnitude: U256::ZERO,
        negative: false,
    };

    /// Constructs a value from raw components, normalising `-0` to `+0`.
    #[inline]
    pub fn new(magnitude: U256, negative: bool) -> Self {
        if magnitude.is_zero() {
            Self::ZERO
        } else {
            Self {
                magnitude,
                negative,
            }
        }
    }

    /// Returns the underlying 256-bit magnitude.
    #[inline]
    #[must_use]
    pub const fn magnitude(self) -> U256 {
        self.magnitude
    }

    /// Returns `true` if the value is strictly negative.
    #[inline]
    #[must_use]
    pub const fn is_negative(self) -> bool {
        self.negative
    }

    /// Returns `true` if the value is exactly zero.
    #[inline]
    #[must_use]
    pub fn is_zero(self) -> bool {
        self.magnitude.is_zero()
    }

    /// `1` in Q128.128.
    #[inline]
    #[must_use]
    pub fn one() -> Self {
        Self::new(SCALE, false)
    }

    /// `-1` in Q128.128.
    #[inline]
    #[must_use]
    pub fn neg_one() -> Self {
        Self::new(SCALE, true)
    }

    /// Lossless conversion from a signed 128-bit integer.
    #[inline]
    #[must_use]
    pub fn from_i128(value: i128) -> Self {
        let negative = value < 0;
        let abs = value.unsigned_abs();
        Self::new(U256::from(abs) << SCALE_BITS, negative)
    }

    /// Lossless conversion from a non-negative 64-bit integer.
    #[inline]
    #[must_use]
    pub fn from_u64(value: u64) -> Self {
        Self::new(U256::from(value) << SCALE_BITS, false)
    }

    /// Conversion from `f64`. Errors on `NaN`, `±inf`, or out-of-range.
    pub fn from_f64(value: f64) -> Result<Self, CoreError> {
        if !value.is_finite() {
            return Err(CoreError::invalid_input(
                "value",
                alloc::format!("expected finite f64, got {value}"),
            ));
        }
        let negative = value.is_sign_negative();
        let abs = value.abs();
        if abs >= F64_2_POW_128 {
            return Err(CoreError::overflow("Sq128::from_f64"));
        }
        // Decompose abs into integer and fractional parts; scale to Q128.128.
        let int_part = abs.trunc();
        let frac_part = abs - int_part;
        // int_part fits in u128 because abs < 2^128.
        let int_u128 = int_part as u128;
        // frac_part * 2^128 fits in u128 (frac_part < 1).
        let frac_u128 = (frac_part * F64_2_POW_128) as u128;
        let magnitude = (U256::from(int_u128) << SCALE_BITS) + U256::from(frac_u128);
        Ok(Self::new(magnitude, negative))
    }

    /// Lossy conversion to `f64`. Precision is limited by f64's 53-bit
    /// mantissa, so values larger than `2^53` lose precision in the high
    /// limbs but the result is monotonic and exact for sub-2^53 magnitudes.
    #[must_use]
    pub fn to_f64(self) -> f64 {
        let [l0, l1, l2, l3] = self.magnitude.into_limbs();
        // Reassociate the sum using fused multiply-add for the leading limb
        // to avoid a redundant rounding step at high magnitudes.
        let value = (l3 as f64).mul_add(
            F64_2_POW_64,
            (l0 as f64) / F64_2_POW_128 + (l1 as f64) / F64_2_POW_64 + (l2 as f64),
        );
        if self.negative { -value } else { value }
    }

    /// Absolute value.
    #[inline]
    #[must_use]
    pub fn abs(self) -> Self {
        Self::new(self.magnitude, false)
    }

    /// Negation (`-self`). `Sq128::ZERO` is its own negation.
    #[inline]
    #[must_use]
    pub fn negate(self) -> Self {
        if self.is_zero() {
            self
        } else {
            Self {
                magnitude: self.magnitude,
                negative: !self.negative,
            }
        }
    }

    /// Checked addition with full range checking.
    pub fn checked_add(self, other: Self) -> Result<Self, CoreError> {
        if self.negative == other.negative {
            let sum = self
                .magnitude
                .checked_add(other.magnitude)
                .ok_or(CoreError::overflow("Sq128::add"))?;
            Ok(Self::new(sum, self.negative))
        } else {
            match self.magnitude.cmp(&other.magnitude) {
                Ordering::Greater => Ok(Self::new(self.magnitude - other.magnitude, self.negative)),
                Ordering::Less => Ok(Self::new(other.magnitude - self.magnitude, other.negative)),
                Ordering::Equal => Ok(Self::ZERO),
            }
        }
    }

    /// Checked subtraction.
    #[inline]
    pub fn checked_sub(self, other: Self) -> Result<Self, CoreError> {
        self.checked_add(other.negate())
    }

    /// Checked Q-format multiplication.
    ///
    /// `(a / 2^128) * (b / 2^128) = (a * b) / 2^256`. We compute the full
    /// 512-bit product, then shift right by 128 bits to land back in
    /// Q128.128.
    pub fn checked_mul(self, other: Self) -> Result<Self, CoreError> {
        let (lo, hi) = widening_mul_u256(self.magnitude, other.magnitude);
        // Result magnitude = (product >> 128). Anything above bit 384 of the
        // 512-bit product makes the Q128.128 result overflow U256.
        if (hi >> SCALE_BITS) != U256::ZERO {
            return Err(CoreError::overflow("Sq128::mul"));
        }
        let magnitude = (lo >> SCALE_BITS) | (hi << SCALE_BITS);
        Ok(Self::new(magnitude, self.negative ^ other.negative))
    }

    /// Checked Q-format division.
    pub fn checked_div(self, other: Self) -> Result<Self, CoreError> {
        if other.magnitude.is_zero() {
            return Err(CoreError::division_by_zero("Sq128::div"));
        }
        // (a / 2^128) / (b / 2^128) = a / b, but to preserve fractional
        // precision we left-shift `a` by 128 bits before dividing.
        let (num_lo, num_hi) = shift_left_512_by_128(self.magnitude, U256::ZERO);
        let quotient = div_512_by_256(num_lo, num_hi, other.magnitude)
            .map_err(|()| CoreError::overflow("Sq128::div"))?;
        Ok(Self::new(quotient, self.negative ^ other.negative))
    }

    /// Bit-exact floor square root in Q128.128.
    ///
    /// Returns the largest `r` such that `r * r <= self` when treating both
    /// operands as Q128.128 magnitudes (i.e. with floor / truncating
    /// multiplication, matching the on-chain `mul_down`). For non-negative
    /// inputs the result satisfies the chain-side `sqrt_verified` invariant:
    ///
    /// * `r² ≤ self` (with `mul_down`), and
    /// * `self − r² < 2·r + ε` where `ε` is one ULP (`Sq128` magnitude 1).
    ///
    /// # Algorithm
    ///
    /// The implementation mirrors `u512_sqrt` /
    /// [`sqrt`](https://github.com/the-situation/contracts) in
    /// `the-situation/contracts/src/types/sq128/advanced.cairo:301-352` and
    /// `:409-436`. We compute `floor(sqrt(mag << 128))` over a 512-bit
    /// magnitude with the same Newton-Raphson iteration the chain runs:
    ///
    /// 1. Identify the highest non-zero 128-bit limb of the shifted value.
    /// 2. Seed Newton with `2^192`, `2^128`, `2^64`, or fall back to the
    ///    u64 `Sqrt` for low magnitudes — matching the Cairo seed table.
    /// 3. Iterate `g_next = (g + v / g) / 2` until `g_next == g` or
    ///    oscillates by ±1, then return the smaller of the converged pair.
    ///
    /// The f64 sqrt is **not** consulted; the iteration runs entirely on
    /// `Sq128`'s 256/512-bit integer helpers. This guarantees bit-for-bit
    /// agreement with the chain's `sqrt_verified` check for every variance,
    /// including non-perfect squares like `0.04` or `0.13`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidInput`] when `self` is strictly negative,
    /// since the square root of a negative `Sq128` is undefined (matching
    /// the on-chain `Option::None` branch for `value.raw.neg`).
    pub fn sqrt(self) -> Result<Self, CoreError> {
        if self.negative {
            return Err(CoreError::invalid_input(
                "sqrt",
                "square root of a negative Sq128 is undefined",
            ));
        }
        if self.magnitude.is_zero() {
            return Ok(Self::ZERO);
        }

        // Compute v = magnitude << 128 as a 512-bit (lo, hi) pair. Because
        // the magnitude already lives in 256 bits, the shifted value occupies
        // at most 384 bits: hi = magnitude >> 128 (top 128 bits land in hi's
        // low half), lo = magnitude << 128 (bottom 128 bits land in lo's
        // high half).
        let (v_lo, v_hi) = shift_left_512_by_128(self.magnitude, U256::ZERO);

        let root_mag = u512_floor_sqrt(v_lo, v_hi);
        Ok(Self::new(root_mag, false))
    }

    /// Total signed ordering.
    #[inline]
    #[must_use]
    pub fn cmp_signed(self, other: Self) -> Ordering {
        match (self.negative, other.negative) {
            (false, false) => self.magnitude.cmp(&other.magnitude),
            (true, true) => other.magnitude.cmp(&self.magnitude),
            (true, false) | (false, true) if self.is_zero() && other.is_zero() => Ordering::Equal,
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
        }
    }

    /// Converts to the on-wire [`Sq128Raw`] limb representation.
    #[inline]
    #[must_use]
    pub fn to_raw(self) -> Sq128Raw {
        let [limb0, limb1, limb2, limb3] = self.magnitude.into_limbs();
        Sq128Raw {
            limb0,
            limb1,
            limb2,
            limb3,
            neg: self.negative && !self.magnitude.is_zero(),
        }
    }

    /// Reconstructs a [`Sq128`] from its wire-format limbs.
    #[inline]
    #[must_use]
    pub fn from_raw(raw: Sq128Raw) -> Self {
        let magnitude = U256::from_limbs([raw.limb0, raw.limb1, raw.limb2, raw.limb3]);
        Self::new(magnitude, raw.neg)
    }
}

impl PartialOrd for Sq128 {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(Ord::cmp(self, other))
    }
}

impl Ord for Sq128 {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.cmp_signed(*other)
    }
}

impl Default for Sq128 {
    #[inline]
    fn default() -> Self {
        Self::ZERO
    }
}

impl Neg for Sq128 {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self::Output {
        self.negate()
    }
}

impl Debug for Sq128 {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let [l0, l1, l2, l3] = self.magnitude.into_limbs();
        write!(
            f,
            "Sq128 {{ neg: {}, limbs: [{l0:#x}, {l1:#x}, {l2:#x}, {l3:#x}] }}",
            self.negative
        )
    }
}

impl Display for Sq128 {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.to_f64(), f)
    }
}

/// Returns the integer part of a Q128.128 value (truncates fractional bits).
#[inline]
#[must_use]
pub fn unscale(value: Sq128) -> U256 {
    value.magnitude >> SCALE_BITS
}

/// Scales an unsigned integer by `2^128`, producing a Q128.128 magnitude.
#[inline]
#[must_use]
pub fn scale(integer: U256) -> U256 {
    integer << SCALE_BITS
}

// ─── 512-bit helpers ─────────────────────────────────────────────────────────

/// Full 512-bit product `(a * b)`, returned as `(lo, hi)` with
/// `result = (hi << 256) | lo`.
fn widening_mul_u256(a: U256, b: U256) -> (U256, U256) {
    let a_l = a.into_limbs();
    let b_l = b.into_limbs();
    let mut out = [0_u64; 8];
    for i in 0..4 {
        let mut carry: u128 = 0;
        for j in 0..4 {
            let cur = u128::from(out[i + j]) + u128::from(a_l[i]) * u128::from(b_l[j]) + carry;
            out[i + j] = cur as u64;
            carry = cur >> 64;
        }
        out[i + 4] = carry as u64;
    }
    (
        U256::from_limbs([out[0], out[1], out[2], out[3]]),
        U256::from_limbs([out[4], out[5], out[6], out[7]]),
    )
}

/// Left-shifts a 512-bit value (lo, hi) by 128 bits.
fn shift_left_512_by_128(lo: U256, hi: U256) -> (U256, U256) {
    let new_lo = lo << SCALE_BITS;
    let new_hi = (hi << SCALE_BITS) | (lo >> SCALE_BITS);
    (new_lo, new_hi)
}

/// Divides a 512-bit numerator (lo, hi) by a 256-bit divisor, returning the
/// 256-bit quotient. Returns `Err(())` when the quotient does not fit in
/// 256 bits.
fn div_512_by_256(lo: U256, hi: U256, d: U256) -> Result<U256, ()> {
    if d.is_zero() {
        return Err(());
    }
    if hi >= d {
        return Err(());
    }
    let lo_limbs = lo.into_limbs();
    let hi_limbs = hi.into_limbs();
    let mut numerator = [0_u64; 8];
    numerator[0..4].copy_from_slice(&lo_limbs);
    numerator[4..8].copy_from_slice(&hi_limbs);

    let mut remainder = U256::ZERO;
    let mut q_limbs = [0_u64; 4];
    for bit_idx in (0..512_usize).rev() {
        let n_limb = bit_idx / 64;
        let n_shift = bit_idx % 64;
        let bit = (numerator[n_limb] >> n_shift) & 1;
        remainder = (remainder << 1_u32) | U256::from(bit);
        if remainder >= d {
            remainder -= d;
            if bit_idx < 256 {
                q_limbs[bit_idx / 64] |= 1_u64 << (bit_idx % 64);
            }
        }
    }
    Ok(U256::from_limbs(q_limbs))
}

/// Floor of the integer square root of a 512-bit unsigned value.
///
/// Mirrors `u512_sqrt` in
/// `the-situation/contracts/src/types/sq128/advanced.cairo:301-352`. The
/// initial guess is selected by the highest non-zero 128-bit limb of the
/// input — the Cairo `u512` decomposes into four 128-bit limbs, so we
/// translate that into our 64-bit-limb world:
///
/// * `limb3 != 0` (top 128 bits set, value in `[2^384, 2^512)`) → `2^192`.
/// * `limb2 != 0` (value in `[2^256, 2^384)`) → `2^128`.
/// * `limb1 != 0` (value in `[2^128, 2^256)`) → `2^64`.
/// * Otherwise (value in `[0, 2^128)`) → exact u128 sqrt via Newton on u128.
///
/// Newton iterates `g_next = (g + v / g) / 2`; convergence stops when
/// `g_next == g` or `g_next == prev_guess` (the classic oscillate-by-1
/// case), at which point we return the smaller of the converged pair to
/// guarantee the floor.
fn u512_floor_sqrt(lo: U256, hi: U256) -> U256 {
    if lo.is_zero() && hi.is_zero() {
        return U256::ZERO;
    }

    let lo_limbs = lo.into_limbs();
    let hi_limbs = hi.into_limbs();
    // The Cairo implementation reads "limb0..limb3" as four 128-bit chunks.
    // Translate that view: Cairo limbN is non-zero iff our `n*2`/`n*2+1`
    // u64 limbs are non-zero.
    let cairo_limb1_nonzero = lo_limbs[2] != 0 || lo_limbs[3] != 0;
    let cairo_limb2_nonzero = hi_limbs[0] != 0 || hi_limbs[1] != 0;
    let cairo_limb3_nonzero = hi_limbs[2] != 0 || hi_limbs[3] != 0;

    let mut guess: U256 = if cairo_limb3_nonzero {
        // Seed 2^192 — same as Cairo "u256 { low: 0, high: 0x1<<64 }".
        U256::from_limbs([0, 0, 0, 1])
    } else if cairo_limb2_nonzero {
        // Seed 2^128 — Cairo "u256 { low: 0, high: 1 }".
        U256::from_limbs([0, 0, 1, 0])
    } else if cairo_limb1_nonzero {
        // Seed 2^64 — Cairo "u256 { low: 1<<64, high: 0 }".
        U256::from_limbs([0, 1, 0, 0])
    } else {
        // Value fits in 128 bits — exact integer sqrt via u128 Newton.
        let v = (u128::from(lo_limbs[1]) << 64) | u128::from(lo_limbs[0]);
        let root = u128_floor_sqrt(v);
        return U256::from(root);
    };

    let mut prev_guess: U256 = U256::ZERO;

    // Bounded outer loop so the lint posture stays happy; convergence is
    // quadratic and 256 iterations is enormous overkill (the chain's
    // unbounded loop converges in 6-8). `bounded_loop_max` mirrors the
    // upper bound we'd ever expect from quadratic Newton on 256 bits.
    let bounded_loop_max: u32 = 256;
    for _ in 0..bounded_loop_max {
        // `value / guess` is a 512-bit dividend divided by a 256-bit
        // divisor. The Cairo path uses `u512_safe_div_rem_by_u256` and
        // truncates to u256 limbs 0-1; we use `div_512_by_256`. Once Newton
        // gets past the initial seed, the quotient is always ≤ sqrt(value)
        // and so always fits in u256. If it doesn't, promote `guess` to
        // `U256::MAX` to drive Newton downward on the next iteration.
        let current_guess = guess;
        let Ok(quotient) = div_512_by_256(lo, hi, current_guess) else {
            guess = U256::MAX;
            continue;
        };
        // `guess + quotient` can overflow u256 (when guess is close to
        // MAX and quotient is similarly large). Track the carry bit so the
        // subsequent `>> 1` lands the value back in u256 cleanly.
        let (sum, carry) = guess.overflowing_add(quotient);
        let new_guess = if carry {
            // Re-insert the carry as the top bit after shifting right.
            (sum >> 1_u32) | (U256::from(1_u64) << 255_u32)
        } else {
            sum >> 1_u32
        };
        if new_guess == guess || new_guess == prev_guess {
            return if new_guess < guess { new_guess } else { guess };
        }
        prev_guess = guess;
        guess = new_guess;
    }
    // Defensive: pick the smaller of the last two iterates.
    if prev_guess.is_zero() || guess < prev_guess {
        guess
    } else {
        prev_guess
    }
}

/// Exact floor of the integer square root of a `u128`. Used as the base
/// case for [`u512_floor_sqrt`]. Newton converges in ≤ 64 iterations.
fn u128_floor_sqrt(value: u128) -> u128 {
    if value < 2 {
        return value;
    }
    // Seed from the highest bit: x0 = 2^ceil(bits / 2). For 128 bits this
    // is at most 2^64, which fits in u128 trivially.
    let leading = value.leading_zeros();
    let bits = 128_u32 - leading;
    let mut guess: u128 = 1_u128 << bits.div_ceil(2);
    loop {
        let next = guess.midpoint(value / guess);
        if next >= guess {
            return guess;
        }
        guess = next;
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use proptest::prelude::*;
    use ruint::aliases::U256;

    use super::*;

    #[test]
    fn zero_invariants() {
        let z = Sq128::ZERO;
        assert!(z.is_zero(), "ZERO must be zero");
        assert!(!z.is_negative(), "ZERO must canonicalise to non-negative");
        assert_eq!(z, z.negate(), "negating zero is zero");
    }

    #[test]
    fn one_round_trips_via_raw() {
        let one = Sq128::one();
        assert_eq!(one.to_raw(), Sq128Raw::ONE);
        assert_eq!(Sq128::from_raw(Sq128Raw::ONE), one);
    }

    #[test]
    fn neg_one_round_trips_via_raw() {
        let v = Sq128::neg_one();
        assert_eq!(v.to_raw(), Sq128Raw::NEG_ONE);
        assert_eq!(Sq128::from_raw(Sq128Raw::NEG_ONE), v);
    }

    #[test]
    fn add_inverse_yields_zero() {
        let a = Sq128::from_i128(17);
        assert!(a.checked_add(a.negate()).unwrap().is_zero());
    }

    #[test]
    fn mul_by_one_is_identity() {
        let a = Sq128::from_i128(42);
        assert_eq!(a.checked_mul(Sq128::one()).unwrap(), a);
    }

    #[test]
    fn mul_by_zero_is_zero() {
        let a = Sq128::from_i128(-99);
        assert!(a.checked_mul(Sq128::ZERO).unwrap().is_zero());
    }

    #[test]
    fn div_by_one_is_identity() {
        let a = Sq128::from_i128(-123);
        assert_eq!(a.checked_div(Sq128::one()).unwrap(), a);
    }

    #[test]
    fn div_by_zero_errors() {
        let result = Sq128::from_i128(1).checked_div(Sq128::ZERO);
        assert!(matches!(result, Err(CoreError::DivisionByZero { .. })));
    }

    #[test]
    fn signed_ordering() {
        assert!(Sq128::from_i128(-1) < Sq128::from_i128(1));
        assert!(Sq128::from_i128(1) > Sq128::from_i128(-1));
        assert_eq!(Sq128::from_i128(0), Sq128::from_i128(-0_i128));
    }

    #[test]
    fn negative_zero_canonicalises() {
        let raw = Sq128Raw {
            limb0: 0,
            limb1: 0,
            limb2: 0,
            limb3: 0,
            neg: true,
        };
        let v = Sq128::from_raw(raw);
        assert_eq!(v.to_raw(), Sq128Raw::ZERO);
    }

    #[test]
    fn unscale_strips_fractional_bits() {
        assert_eq!(unscale(Sq128::from_i128(7)), U256::from(7_u64));
    }

    #[test]
    fn f64_round_trip_within_tolerance() {
        for sample in [0.0_f64, 1.0, -1.0, 3.5, -7.25, 1234.5678, -9876.54321] {
            let v = Sq128::from_f64(sample).unwrap();
            let back = v.to_f64();
            let diff = (back - sample).abs();
            assert!(diff < 1e-12, "round-trip drift for {sample}: {diff}");
        }
    }

    #[test]
    fn sqrt_rejects_negative() {
        let err = Sq128::from_i128(-1)
            .sqrt()
            .expect_err("must reject negative");
        assert!(matches!(err, CoreError::InvalidInput { .. }));
    }

    #[test]
    fn sqrt_of_zero_is_zero() {
        assert!(Sq128::ZERO.sqrt().unwrap().is_zero());
    }

    #[test]
    fn sqrt_perfect_squares() {
        // Bit-exact: every value whose σ is exactly representable in Sq128
        // must yield exactly that σ.
        for (value, expected) in [
            (4_i128, 2_i128),
            (9, 3),
            (16, 4),
            (25, 5),
            (36, 6),
            (49, 7),
            (64, 8),
            (81, 9),
            (100, 10),
            (10_000, 100),
            (1_000_000, 1_000),
        ] {
            let v = Sq128::from_i128(value);
            let r = v.sqrt().unwrap();
            assert_eq!(
                r,
                Sq128::from_i128(expected),
                "sqrt({value}) must be exactly {expected}"
            );
            // And the round-trip σ × σ == variance holds.
            assert_eq!(r.checked_mul(r).unwrap(), v);
        }
    }

    #[test]
    fn sqrt_fractional_perfect_squares() {
        // 0.25, 0.5625 etc. are exact in Sq128 (their σ is exact too).
        let quarter = Sq128::from_f64(0.25).unwrap();
        let half = Sq128::from_f64(0.5).unwrap();
        assert_eq!(quarter.sqrt().unwrap(), half);

        let nine_sixteenths = Sq128::from_f64(0.5625).unwrap();
        let three_quarters = Sq128::from_f64(0.75).unwrap();
        assert_eq!(nine_sixteenths.sqrt().unwrap(), three_quarters);
    }

    #[test]
    fn sqrt_floor_invariant() {
        // For arbitrary variances, σ × σ ≤ variance and (σ+ε) × (σ+ε) > variance.
        // This is the exact `sqrt_verified` contract from the chain.
        for v_int in [4_i128, 9, 13, 50, 100, 12345, 99999, 1_000_001] {
            let variance = Sq128::from_i128(v_int);
            let sigma = variance.sqrt().unwrap();
            let sigma_sq = sigma.checked_mul(sigma).unwrap();
            assert!(
                sigma_sq <= variance,
                "σ² > variance for v={v_int}: σ²={}, variance={v_int}",
                sigma_sq.to_f64()
            );
            // (σ + 1 ULP)² > variance — i.e. σ is the *floor* of the true sqrt.
            let sigma_plus = Sq128::new(sigma.magnitude() + U256::from(1_u64), false);
            let next_sq = sigma_plus.checked_mul(sigma_plus).unwrap();
            assert!(next_sq > variance, "σ is not the floor sqrt for v={v_int}");
        }
    }

    #[test]
    fn sqrt_of_one_ulp_is_floor() {
        // 1 ULP variance has raw magnitude 1 (true value `2^-128`). After the
        // 128-bit shift the integer sqrt input is `2^128`, whose exact root
        // is `2^64`. So sqrt(1 ULP) has magnitude `2^64` — i.e. Sq128 value
        // `2^-64`. Verify both the limb and the `mul_down` round-trip.
        let one_ulp = Sq128::new(U256::from(1_u64), false);
        let r = one_ulp.sqrt().unwrap();
        // `2^64` lives entirely in limb1 (since limb0 is bits 0..64).
        let expected_mag = U256::from_limbs([0, 1, 0, 0]);
        assert_eq!(r.magnitude(), expected_mag, "sqrt(1 ULP) ≠ 2^-64");
        // σ² == variance (exact, since 2^128 is a perfect square).
        assert_eq!(r.checked_mul(r).unwrap(), one_ulp);
    }

    #[test]
    fn sqrt_of_max_sq128_does_not_panic() {
        // Largest possible Sq128 magnitude — exercises the Cairo seed-table
        // branch that picks 2^128 (since shifted hi sits in [2^128, 2^256)).
        let max = Sq128::new(U256::MAX, false);
        let r = max.sqrt().unwrap();
        // Floor invariant must hold against the on-chain `mul_down`: σ² ≤ MAX.
        let r_sq = r.checked_mul(r).unwrap();
        assert!(r_sq <= max, "sqrt(MAX)² > MAX");
        // And the chain's exact upper bound: gap < 2σ + ε. This is the
        // sqrt_verified contract, equivalent to "no larger σ works".
        let gap = max.checked_sub(r_sq).unwrap();
        let two_sigma = r.checked_add(r).unwrap();
        let threshold = Sq128::new(two_sigma.magnitude() + U256::from(1_u64), false);
        assert!(gap < threshold, "gap ≥ 2σ + ε at MAX");
    }

    #[test]
    fn sqrt_regression_0_04_matches_chain_sigma() {
        // Previously-failing case: f64-mediated σ for variance = 0.04
        // rounded to ≈0.20000000298…, which the chain's `sqrt_verified`
        // rejected. With bit-exact Sq128 sqrt the σ satisfies the chain's
        // exact invariant `gap = variance − mul_down(σ, σ) < 2σ + ε`.
        let variance = Sq128::from_f64(0.04).unwrap();
        let sigma = variance.sqrt().unwrap();
        let sigma_sq = sigma.checked_mul(sigma).unwrap();
        assert!(sigma_sq <= variance, "σ² > variance");
        // Chain invariant: `gap < 2σ + ε`.
        let gap = variance.checked_sub(sigma_sq).unwrap();
        let two_sigma = sigma.checked_add(sigma).unwrap();
        let threshold = Sq128::new(two_sigma.magnitude() + U256::from(1_u64), false);
        assert!(gap < threshold, "gap ≥ 2σ + ε");
        // Sanity: ours is below or equal to the f64-mediated sqrt (which
        // rounds up in the last bit for 0.04 — that's what used to be
        // rejected by sqrt_verified).
        let f64_sigma = Sq128::from_f64(0.04_f64.sqrt()).unwrap();
        assert!(
            sigma.magnitude() <= f64_sigma.magnitude(),
            "Sq128 floor sqrt must be ≤ f64-mediated sqrt for 0.04"
        );
    }

    #[test]
    fn sqrt_arbitrary_variance_satisfies_chain_invariant() {
        // 0.04 = 1/25 is *not* a perfect Sq128 square: σ = 0.2 isn't
        // representable exactly because 0.2 = 1/5 has an infinite binary
        // expansion. The chain's `sqrt_verified` accepts any hint h
        // satisfying h² ≤ value < (h+ε)² — assert our sqrt produces exactly
        // such a hint.
        let variance = Sq128::from_f64(0.04).unwrap();
        let sigma = variance.sqrt().unwrap();
        let sigma_sq = sigma.checked_mul(sigma).unwrap();
        assert!(sigma_sq <= variance, "σ² > variance");

        // Gap = variance - σ² < 2σ + ε (the `sqrt_verified` upper bound).
        let gap = variance.checked_sub(sigma_sq).unwrap();
        let two_sigma = sigma.checked_add(sigma).unwrap();
        let threshold = Sq128::new(two_sigma.magnitude() + U256::from(1_u64), false);
        assert!(gap < threshold, "gap exceeds 2σ + ε");

        // f64 σ would have been ≈0.20000000298…; ours is the true floor.
        // Sanity: |σ - 0.2| < 1e-15 in f64 terms.
        assert!((sigma.to_f64() - 0.2_f64).abs() < 1e-15);
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

        #[test]
        fn raw_round_trip(
            limb0 in any::<u64>(),
            limb1 in any::<u64>(),
            limb2 in any::<u64>(),
            limb3 in any::<u64>(),
            neg in any::<bool>(),
        ) {
            let raw = Sq128Raw { limb0, limb1, limb2, limb3, neg };
            let v = Sq128::from_raw(raw);
            let back = v.to_raw();
            if v.is_zero() {
                prop_assert!(!back.neg);
            } else {
                prop_assert_eq!(back, raw);
            }
        }

        #[test]
        fn add_is_commutative(a in any::<i64>(), b in any::<i64>()) {
            let x = Sq128::from_i128(i128::from(a));
            let y = Sq128::from_i128(i128::from(b));
            prop_assert_eq!(x.checked_add(y).unwrap(), y.checked_add(x).unwrap());
        }

        #[test]
        fn add_sub_inverse(a in any::<i64>(), b in any::<i32>()) {
            let x = Sq128::from_i128(i128::from(a));
            let y = Sq128::from_i128(i128::from(b));
            let sum = x.checked_add(y).unwrap();
            prop_assert_eq!(sum.checked_sub(y).unwrap(), x);
        }

        #[test]
        fn mul_is_commutative(a in -1_000_000_i32..1_000_000_i32, b in -1_000_000_i32..1_000_000_i32) {
            let x = Sq128::from_i128(i128::from(a));
            let y = Sq128::from_i128(i128::from(b));
            prop_assert_eq!(x.checked_mul(y).unwrap(), y.checked_mul(x).unwrap());
        }

        #[test]
        fn div_then_mul_round_trips(
            a in 1_i32..1_000_000_i32,
            b in 1_i32..1_000_000_i32,
        ) {
            let x = Sq128::from_i128(i128::from(a));
            let y = Sq128::from_i128(i128::from(b));
            let q = x.checked_div(y).unwrap();
            let back = q.checked_mul(y).unwrap();
            let diff = (back.to_f64() - x.to_f64()).abs();
            prop_assert!(diff < 1e-20, "diff = {diff}");
        }
    }
}
