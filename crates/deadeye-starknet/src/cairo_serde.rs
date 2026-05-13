//! Cairo Serde encoders and decoders for moving values between Rust and
//! Starknet calldata.
//!
//! ## Layout convention
//!
//! Each implementation knows exactly how many `Felt`s it consumes and
//! produces. Encoders push into a [`Vec<Felt>`] in calldata order;
//! decoders take a `&[Felt]` slice and return `(Self, &[Felt])`,
//! where the returned slice points past the consumed felts. This avoids
//! per-field indexing on the hot path and makes it impossible to
//! accidentally double-consume a slot.

use deadeye_core::{
    distribution::{NormalDistributionRaw, NormalSqrtHintsRaw},
    sq128::{Sq128, Sq128Raw},
};
use starknet_core::types::Felt;
use thiserror::Error;

/// Errors emitted by [`CairoSerde`] implementations.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum CairoSerdeError {
    /// The input slice was shorter than the encoded type required.
    #[error("not enough felts: needed {needed}, found {found}")]
    Truncated {
        /// Felts the decoder needed.
        needed: usize,
        /// Felts available in the slice.
        found: usize,
    },

    /// A boolean felt was neither `0` nor `1`.
    #[error("invalid bool felt: {value}")]
    InvalidBool {
        /// Hex string of the felt that failed validation.
        value: String,
    },

    /// A u64 limb was set above the 64-bit boundary.
    #[error("u64 limb out of range: {value}")]
    U64OutOfRange {
        /// Hex string of the felt that failed validation.
        value: String,
    },

    /// A u128 value was set above the 128-bit boundary.
    #[error("u128 limb out of range: {value}")]
    U128OutOfRange {
        /// Hex string of the felt that failed validation.
        value: String,
    },

    /// An enum tag did not match any known variant.
    #[error("invalid enum tag `{tag}` for `{enum_name}`")]
    InvalidEnumTag {
        /// Symbolic name of the enum type.
        enum_name: &'static str,
        /// Numeric tag observed.
        tag: u64,
    },

    /// A sequence length exceeded a reasonable bound (sanity guard against
    /// hostile RPCs).
    #[error("sequence length {length} exceeds maximum {max}")]
    SequenceTooLong {
        /// Observed length.
        length: u64,
        /// Cap enforced by the decoder.
        max: u64,
    },
}

/// A type that can be (de)serialised as a sequence of Starknet field
/// elements.
pub trait CairoSerde: Sized {
    /// Append the encoding of `self` to `out`.
    fn encode(&self, out: &mut Vec<Felt>);

    /// Decode an instance from the start of `slice`. Returns the value plus
    /// the remaining slice.
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError>;

    /// Convenience: encode to a freshly allocated `Vec`.
    fn to_calldata(&self) -> Vec<Felt> {
        let mut out = Vec::new();
        self.encode(&mut out);
        out
    }
}

/// Helper macro for encoding Cairo unit-variant enums (no associated data).
///
/// Cairo serialises enum values as `[discriminant, ...variant_data]`; for
/// unit variants there is no variant data, so the encoding is a single
/// felt. The macro generates [`CairoSerde`] impls that match the
/// generated discriminant order in Cairo (variants must be declared in
/// the same order as the Cairo source).
#[macro_export]
macro_rules! cairo_serde_unit_enum {
    ($ty:ident { $($variant:ident = $val:literal,)* $(,)? }) => {
        impl $crate::cairo_serde::CairoSerde for $ty {
            fn encode(&self, out: &mut ::std::vec::Vec<::starknet_core::types::Felt>) {
                let tag: u64 = match self {
                    $(Self::$variant => $val,)*
                };
                out.push(::starknet_core::types::Felt::from(tag));
            }
            fn decode(
                slice: &[::starknet_core::types::Felt],
            ) -> ::core::result::Result<
                (Self, &[::starknet_core::types::Felt]),
                $crate::cairo_serde::CairoSerdeError,
            > {
                let (tag, rest) = <u64 as $crate::cairo_serde::CairoSerde>::decode(slice)?;
                let value = match tag {
                    $($val => Self::$variant,)*
                    other => return ::core::result::Result::Err(
                        $crate::cairo_serde::CairoSerdeError::InvalidEnumTag {
                            enum_name: stringify!($ty),
                            tag: other,
                        },
                    ),
                };
                ::core::result::Result::Ok((value, rest))
            }
        }
    };
}

// ─── primitives ──────────────────────────────────────────────────────────────

fn take_one(slice: &[Felt]) -> Result<(Felt, &[Felt]), CairoSerdeError> {
    slice
        .split_first()
        .map(|(head, rest)| (*head, rest))
        .ok_or(CairoSerdeError::Truncated {
            needed: 1,
            found: 0,
        })
}

impl CairoSerde for Felt {
    fn encode(&self, out: &mut Vec<Felt>) {
        out.push(*self);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        take_one(slice)
    }
}

impl CairoSerde for bool {
    fn encode(&self, out: &mut Vec<Felt>) {
        out.push(if *self { Felt::ONE } else { Felt::ZERO });
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (f, rest) = take_one(slice)?;
        if f == Felt::ZERO {
            Ok((false, rest))
        } else if f == Felt::ONE {
            Ok((true, rest))
        } else {
            Err(CairoSerdeError::InvalidBool {
                value: format!("{f:#x}"),
            })
        }
    }
}

impl CairoSerde for u64 {
    fn encode(&self, out: &mut Vec<Felt>) {
        out.push(Felt::from(*self));
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (f, rest) = take_one(slice)?;
        let bytes = f.to_bytes_be();
        let (high, low) = bytes.split_at(24);
        if high.iter().any(|b| *b != 0) {
            return Err(CairoSerdeError::U64OutOfRange {
                value: format!("{f:#x}"),
            });
        }
        let mut buf = [0_u8; 8];
        buf.copy_from_slice(low);
        #[expect(
            clippy::big_endian_bytes,
            reason = "Felt::to_bytes_be is big-endian by spec"
        )]
        Ok((Self::from_be_bytes(buf), rest))
    }
}

impl CairoSerde for u32 {
    fn encode(&self, out: &mut Vec<Felt>) {
        out.push(Felt::from(*self));
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (value, rest) = u64::decode(slice)?;
        let v32 = Self::try_from(value).map_err(|_| CairoSerdeError::U64OutOfRange {
            value: format!("{value}"),
        })?;
        Ok((v32, rest))
    }
}

impl CairoSerde for u8 {
    fn encode(&self, out: &mut Vec<Felt>) {
        out.push(Felt::from(*self));
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (value, rest) = u64::decode(slice)?;
        let v8 = Self::try_from(value).map_err(|_| CairoSerdeError::U64OutOfRange {
            value: format!("{value}"),
        })?;
        Ok((v8, rest))
    }
}

impl CairoSerde for u128 {
    fn encode(&self, out: &mut Vec<Felt>) {
        out.push(Felt::from(*self));
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (f, rest) = take_one(slice)?;
        let bytes = f.to_bytes_be();
        let (high, low) = bytes.split_at(16);
        if high.iter().any(|b| *b != 0) {
            return Err(CairoSerdeError::U128OutOfRange {
                value: format!("{f:#x}"),
            });
        }
        let mut buf = [0_u8; 16];
        buf.copy_from_slice(low);
        #[expect(
            clippy::big_endian_bytes,
            reason = "Felt::to_bytes_be is big-endian by spec"
        )]
        Ok((Self::from_be_bytes(buf), rest))
    }
}

/// Hard upper bound on decoded `Vec<T>` length — prevents an adversarial
/// RPC from triggering a 4-billion-felt allocation.
const MAX_SEQUENCE_LEN: u64 = 1 << 20;

impl<T> CairoSerde for Vec<T>
where
    T: CairoSerde,
{
    fn encode(&self, out: &mut Vec<Felt>) {
        // Cairo `Array<T>` length is a felt; we serialise as the unsigned
        // numeric value of `self.len()` cast to u64.
        out.push(Felt::from(self.len() as u64));
        for item in self {
            item.encode(out);
        }
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (len, mut rest) = u64::decode(slice)?;
        if len > MAX_SEQUENCE_LEN {
            return Err(CairoSerdeError::SequenceTooLong {
                length: len,
                max: MAX_SEQUENCE_LEN,
            });
        }
        let len_us = len as usize;
        let mut items = Self::with_capacity(len_us);
        for _ in 0..len_us {
            let (item, r) = T::decode(rest)?;
            items.push(item);
            rest = r;
        }
        Ok((items, rest))
    }
}

// ─── Sq128 ───────────────────────────────────────────────────────────────────

impl CairoSerde for Sq128Raw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.limb0.encode(out);
        self.limb1.encode(out);
        self.limb2.encode(out);
        self.limb3.encode(out);
        self.neg.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (limb0, slice) = u64::decode(slice)?;
        let (limb1, slice) = u64::decode(slice)?;
        let (limb2, slice) = u64::decode(slice)?;
        let (limb3, slice) = u64::decode(slice)?;
        let (neg, slice) = bool::decode(slice)?;
        Ok((
            Self {
                limb0,
                limb1,
                limb2,
                limb3,
                neg,
            },
            slice,
        ))
    }
}

impl CairoSerde for Sq128 {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.to_raw().encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (raw, slice) = Sq128Raw::decode(slice)?;
        Ok((Self::from_raw(raw), slice))
    }
}

// ─── Distributions ───────────────────────────────────────────────────────────

impl CairoSerde for NormalDistributionRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.mean.encode(out);
        self.variance.encode(out);
        self.sigma.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (mean, slice) = Sq128Raw::decode(slice)?;
        let (variance, slice) = Sq128Raw::decode(slice)?;
        let (sigma, slice) = Sq128Raw::decode(slice)?;
        Ok((
            Self {
                mean,
                variance,
                sigma,
            },
            slice,
        ))
    }
}

impl CairoSerde for NormalSqrtHintsRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.l2_norm_denom.encode(out);
        self.backing_denom.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (l2_norm_denom, slice) = Sq128Raw::decode(slice)?;
        let (backing_denom, slice) = Sq128Raw::decode(slice)?;
        Ok((
            Self {
                l2_norm_denom,
                backing_denom,
            },
            slice,
        ))
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use super::*;

    #[test]
    fn bool_round_trip() {
        for value in [false, true] {
            let cd = value.to_calldata();
            assert_eq!(cd.len(), 1);
            let (back, rest) = bool::decode(&cd).unwrap();
            assert_eq!(back, value);
            assert!(rest.is_empty());
        }
    }

    #[test]
    fn u64_round_trip() {
        for value in [0_u64, 1, u64::MAX] {
            let cd = value.to_calldata();
            let (back, rest) = u64::decode(&cd).unwrap();
            assert_eq!(back, value);
            assert!(rest.is_empty());
        }
    }

    #[test]
    fn sq128_round_trip() {
        let raw = Sq128Raw {
            limb0: 42,
            limb1: 0,
            limb2: 1,
            limb3: 7,
            neg: true,
        };
        let cd = raw.to_calldata();
        assert_eq!(cd.len(), 5, "Sq128Raw must serialise to 5 felts");
        let (back, rest) = Sq128Raw::decode(&cd).unwrap();
        assert_eq!(back, raw);
        assert!(rest.is_empty());
    }

    #[test]
    fn truncated_input_rejected() {
        let result = Sq128Raw::decode(&[]);
        assert!(matches!(result, Err(CairoSerdeError::Truncated { .. })));
    }

    #[test]
    fn invalid_bool_rejected() {
        let arr = [Felt::from(2_u64)];
        let result = bool::decode(&arr);
        assert!(matches!(result, Err(CairoSerdeError::InvalidBool { .. })));
    }

    #[test]
    fn u64_high_bits_rejected() {
        // Felt::from(u128::MAX) has bits 64..128 set — rejected.
        let arr = [Felt::from(u128::MAX)];
        let result = u64::decode(&arr);
        assert!(matches!(result, Err(CairoSerdeError::U64OutOfRange { .. })));
    }
}
