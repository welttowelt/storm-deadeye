//! Shapes shared across every market family.
//!
//! Mirrors `@the-situation/abi`'s `common.ts`. Each struct here maps
//! one-to-one with a Cairo struct used in calldata / return slots.

use deadeye_core::sq128::Sq128Raw;
use starknet_core::types::Felt;

use crate::{
    cairo_serde::{CairoSerde, CairoSerdeError},
    cairo_serde_unit_enum,
};

/// Maximum total fee in basis points (10% = 1000 bps).
pub const MAX_FEE_BPS: u16 = 1_000;

// ─── Fee config ──────────────────────────────────────────────────────────────

/// Per-market fee configuration in basis points.
///
/// Field widths mirror the on-chain Cairo `FeeConfigRaw` (every `*_bps` is
/// `u16` in `onchain-core/src/common.cairo`). Keeping the widths in lockstep
/// makes the calldata encoding bit-exact and prevents a Rust caller from
/// silently submitting a value `≥ 2^16` that Cairo's `u16::Serde` would
/// reject at decode time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FeeConfigRaw {
    /// LP fee in bps (stays in the pool).
    pub lp_fee_bps: u16,
    /// Protocol fee in bps (forwarded to the factory treasury).
    pub protocol_fee_bps: u16,
    /// Settlement fee in bps (applied to trader winnings at settlement).
    pub settlement_fee_bps: u16,
}

impl FeeConfigRaw {
    /// Returns the sum of all configured fees.
    #[must_use]
    pub const fn total_bps(self) -> u32 {
        self.lp_fee_bps as u32 + self.protocol_fee_bps as u32 + self.settlement_fee_bps as u32
    }

    /// Returns `true` iff the fee config respects the global cap.
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.total_bps() <= MAX_FEE_BPS as u32
    }
}

impl CairoSerde for FeeConfigRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.lp_fee_bps.encode(out);
        self.protocol_fee_bps.encode(out);
        self.settlement_fee_bps.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (lp_fee_bps, slice) = u16::decode(slice)?;
        let (protocol_fee_bps, slice) = u16::decode(slice)?;
        let (settlement_fee_bps, slice) = u16::decode(slice)?;
        Ok((
            Self {
                lp_fee_bps,
                protocol_fee_bps,
                settlement_fee_bps,
            },
            slice,
        ))
    }
}

// ─── AMM params / config ─────────────────────────────────────────────────────

/// Mutable AMM parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AmmParamsRaw {
    /// Invariant parameter `k`.
    pub k: Sq128Raw,
    /// Total backing supplied to the pool.
    pub backing: Sq128Raw,
    /// Tolerance used during the on-chain stationary check.
    pub tolerance: Sq128Raw,
    /// Minimum collateral a single trade must supply.
    pub min_trade_collateral: Sq128Raw,
    /// Payout amplifier applied at settlement (defaults to 1.0).
    pub payout_amplifier: Sq128Raw,
}

impl CairoSerde for AmmParamsRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.k.encode(out);
        self.backing.encode(out);
        self.tolerance.encode(out);
        self.min_trade_collateral.encode(out);
        self.payout_amplifier.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (k, slice) = Sq128Raw::decode(slice)?;
        let (backing, slice) = Sq128Raw::decode(slice)?;
        let (tolerance, slice) = Sq128Raw::decode(slice)?;
        let (min_trade_collateral, slice) = Sq128Raw::decode(slice)?;
        let (payout_amplifier, slice) = Sq128Raw::decode(slice)?;
        Ok((
            Self {
                k,
                backing,
                tolerance,
                min_trade_collateral,
                payout_amplifier,
            },
            slice,
        ))
    }
}

/// AMM configuration: collateral token, decimal handling, parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AmmConfigRaw {
    /// Collateral token contract address.
    pub collateral_token: Felt,
    /// On-chain token decimals (e.g. 18 for STRK).
    pub token_decimals: u8,
    /// Internal precision used by the AMM (typically 18).
    pub internal_decimals: u8,
    /// Decimal shift applied between token and internal precision.
    /// Cairo encodes this as `u8`; treat it as non-negative.
    pub decimal_shift: u8,
    /// Embedded AMM parameters.
    pub params: AmmParamsRaw,
}

impl CairoSerde for AmmConfigRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.collateral_token.encode(out);
        self.token_decimals.encode(out);
        self.internal_decimals.encode(out);
        self.decimal_shift.encode(out);
        self.params.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (collateral_token, slice) = Felt::decode(slice)?;
        let (token_decimals, slice) = u8::decode(slice)?;
        let (internal_decimals, slice) = u8::decode(slice)?;
        let (decimal_shift, slice) = u8::decode(slice)?;
        let (params, slice) = AmmParamsRaw::decode(slice)?;
        Ok((
            Self {
                collateral_token,
                token_decimals,
                internal_decimals,
                decimal_shift,
                params,
            },
            slice,
        ))
    }
}

// ─── LP info ─────────────────────────────────────────────────────────────────

/// Total LP state for a pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LpInfoRaw {
    /// Total LP shares outstanding.
    pub total_shares: Sq128Raw,
    /// Cumulative backing deposited (gross of withdrawals).
    pub total_backing_deposited: Sq128Raw,
}

impl CairoSerde for LpInfoRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.total_shares.encode(out);
        self.total_backing_deposited.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (total_shares, slice) = Sq128Raw::decode(slice)?;
        let (total_backing_deposited, slice) = Sq128Raw::decode(slice)?;
        Ok((
            Self {
                total_shares,
                total_backing_deposited,
            },
            slice,
        ))
    }
}

// ─── Claim result ────────────────────────────────────────────────────────────

/// Payload returned by `claim` / `claim_for`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClaimResultRaw {
    /// Position value evaluated at settlement (in Q128.128).
    pub position_value: Sq128Raw,
    /// Collateral released back to the trader.
    pub collateral_returned: Sq128Raw,
    /// Token payout amount (u128 to fit ERC20 amounts).
    pub token_payout: u128,
    /// `true` if the claim succeeded.
    pub success: bool,
}

impl CairoSerde for ClaimResultRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.position_value.encode(out);
        self.collateral_returned.encode(out);
        self.token_payout.encode(out);
        self.success.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (position_value, slice) = Sq128Raw::decode(slice)?;
        let (collateral_returned, slice) = Sq128Raw::decode(slice)?;
        let (token_payout, slice) = u128::decode(slice)?;
        let (success, slice) = bool::decode(slice)?;
        Ok((
            Self {
                position_value,
                collateral_returned,
                token_payout,
                success,
            },
            slice,
        ))
    }
}

// ─── Backing & verification ──────────────────────────────────────────────────

/// Scaled backing check returned by the math runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScaledBackingCheckRaw {
    /// Upper bound on the candidate's maximum value.
    pub max_value_upper: Sq128Raw,
    /// Whether the bound check succeeded.
    pub is_valid: bool,
    /// Whether the underlying computation succeeded.
    pub computation_succeeded: bool,
}

impl CairoSerde for ScaledBackingCheckRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.max_value_upper.encode(out);
        self.is_valid.encode(out);
        self.computation_succeeded.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (max_value_upper, slice) = Sq128Raw::decode(slice)?;
        let (is_valid, slice) = bool::decode(slice)?;
        let (computation_succeeded, slice) = bool::decode(slice)?;
        Ok((
            Self {
                max_value_upper,
                is_valid,
                computation_succeeded,
            },
            slice,
        ))
    }
}

/// Collateral verification result returned by the math runtime.
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the on-chain Cairo struct field-for-field"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CollateralVerificationRaw {
    /// Whether the chosen side is correct.
    pub side_valid: bool,
    /// Whether `d'(x*)` is within tolerance.
    pub stationary_valid: bool,
    /// Whether `d''(x*) > 0`.
    pub curvature_valid: bool,
    /// Collateral the runtime computed.
    pub computed_collateral: Sq128Raw,
    /// Whether the supplied collateral was sufficient.
    pub collateral_sufficient: bool,
    /// Whether the overall computation succeeded.
    pub computation_valid: bool,
}

impl CairoSerde for CollateralVerificationRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.side_valid.encode(out);
        self.stationary_valid.encode(out);
        self.curvature_valid.encode(out);
        self.computed_collateral.encode(out);
        self.collateral_sufficient.encode(out);
        self.computation_valid.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (side_valid, slice) = bool::decode(slice)?;
        let (stationary_valid, slice) = bool::decode(slice)?;
        let (curvature_valid, slice) = bool::decode(slice)?;
        let (computed_collateral, slice) = Sq128Raw::decode(slice)?;
        let (collateral_sufficient, slice) = bool::decode(slice)?;
        let (computation_valid, slice) = bool::decode(slice)?;
        Ok((
            Self {
                side_valid,
                stationary_valid,
                curvature_valid,
                computed_collateral,
                collateral_sufficient,
                computation_valid,
            },
            slice,
        ))
    }
}

// ─── Enums shared with normal markets ────────────────────────────────────────

/// Reason a trade was rejected by the on-chain verifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TradeRejection {
    /// Trade was accepted.
    None,
    /// Distribution failed validation.
    InvalidDistribution,
    /// Sqrt hints did not validate.
    InvalidHints,
    /// Backing check failed.
    BackingFail,
    /// Sigma fell below the minimum.
    SigmaTooLow,
    /// Collateral was below `min_trade_collateral`.
    LowCollateral,
    /// Generic verification failure.
    VerificationFailed,
}

cairo_serde_unit_enum!(TradeRejection {
    None = 0,
    InvalidDistribution = 1,
    InvalidHints = 2,
    BackingFail = 3,
    SigmaTooLow = 4,
    LowCollateral = 5,
    VerificationFailed = 6,
});

/// Reason a position-sell call was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PositionSellRejection {
    /// Accepted.
    None,
    /// No open position.
    NoPosition,
    /// Market is settled.
    MarketSettled,
    /// Market is paused.
    MarketPaused,
    /// Already claimed.
    AlreadyClaimed,
    /// Position state was invalid.
    InvalidPositionState,
    /// Required additional collateral.
    RequiresAdditionalCollateral,
    /// No collateral could be released.
    NoCollateralOut,
    /// Invalid hints.
    InvalidHints,
    /// Backing check failed.
    BackingFail,
    /// Sigma too low.
    SigmaTooLow,
    /// Verification failed.
    VerificationFailed,
    /// Conversion failed.
    ConversionFailed,
}

cairo_serde_unit_enum!(PositionSellRejection {
    None = 0,
    NoPosition = 1,
    MarketSettled = 2,
    MarketPaused = 3,
    AlreadyClaimed = 4,
    InvalidPositionState = 5,
    RequiresAdditionalCollateral = 6,
    NoCollateralOut = 7,
    InvalidHints = 8,
    BackingFail = 9,
    SigmaTooLow = 10,
    VerificationFailed = 11,
    ConversionFailed = 12,
});
