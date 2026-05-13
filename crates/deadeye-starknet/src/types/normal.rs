//! Normal-market specific Cairo Serde shapes.
//!
//! Mirrors `@the-situation/abi`'s `amm.ts`.

use deadeye_core::{
    distribution::{NormalDistributionRaw, NormalSqrtHintsRaw},
    sq128::Sq128Raw,
};
use starknet_core::types::Felt;

use crate::{
    cairo_serde::{CairoSerde, CairoSerdeError},
    types::common::{
        CollateralVerificationRaw, PositionSellRejection, ScaledBackingCheckRaw, TradeRejection,
    },
};

// ─── execute_trade input ─────────────────────────────────────────────────────

/// Input to `execute_trade` on a normal AMM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TradeInput {
    /// Candidate distribution the MM wants to move the market to.
    pub candidate: NormalDistributionRaw,
    /// `x*` — location of the minimum of `d(x) = g(x) − f(x)`.
    pub x_star: Sq128Raw,
    /// Collateral supplied with this trade.
    pub supplied_collateral: Sq128Raw,
    /// Pre-computed square-root hints for the candidate.
    pub candidate_hints: NormalSqrtHintsRaw,
}

impl CairoSerde for TradeInput {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.candidate.encode(out);
        self.x_star.encode(out);
        self.supplied_collateral.encode(out);
        self.candidate_hints.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (candidate, slice) = NormalDistributionRaw::decode(slice)?;
        let (x_star, slice) = Sq128Raw::decode(slice)?;
        let (supplied_collateral, slice) = Sq128Raw::decode(slice)?;
        let (candidate_hints, slice) = NormalSqrtHintsRaw::decode(slice)?;
        Ok((
            Self {
                candidate,
                x_star,
                supplied_collateral,
                candidate_hints,
            },
            slice,
        ))
    }
}

// ─── Trade check + execution ─────────────────────────────────────────────────

/// Outcome of the on-chain trade validation pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TradeCheckRaw {
    /// Scaled backing check.
    pub backing_check: ScaledBackingCheckRaw,
    /// Collateral verification.
    pub verification: CollateralVerificationRaw,
    /// Per-market floor on trade collateral.
    pub min_trade_collateral: Sq128Raw,
    /// Whether the supplied collateral cleared the floor.
    pub collateral_above_min: bool,
    /// Aggregate validity bit.
    pub is_valid: bool,
    /// Symbolic rejection reason (None if valid).
    pub rejection_reason: TradeRejection,
}

impl CairoSerde for TradeCheckRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.backing_check.encode(out);
        self.verification.encode(out);
        self.min_trade_collateral.encode(out);
        self.collateral_above_min.encode(out);
        self.is_valid.encode(out);
        self.rejection_reason.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (backing_check, slice) = ScaledBackingCheckRaw::decode(slice)?;
        let (verification, slice) = CollateralVerificationRaw::decode(slice)?;
        let (min_trade_collateral, slice) = Sq128Raw::decode(slice)?;
        let (collateral_above_min, slice) = bool::decode(slice)?;
        let (is_valid, slice) = bool::decode(slice)?;
        let (rejection_reason, slice) = TradeRejection::decode(slice)?;
        Ok((
            Self {
                backing_check,
                verification,
                min_trade_collateral,
                collateral_above_min,
                is_valid,
                rejection_reason,
            },
            slice,
        ))
    }
}

/// Result of executing a trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TradeExecutionRaw {
    /// Embedded check outcome.
    pub check: TradeCheckRaw,
    /// ERC20 token amount transferred (u128).
    pub token_amount: u128,
}

impl CairoSerde for TradeExecutionRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.check.encode(out);
        self.token_amount.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (check, slice) = TradeCheckRaw::decode(slice)?;
        let (token_amount, slice) = u128::decode(slice)?;
        Ok((
            Self {
                check,
                token_amount,
            },
            slice,
        ))
    }
}

// ─── Position types ──────────────────────────────────────────────────────────

/// Lightweight summary of a trader's position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PositionSummaryRaw {
    /// Total collateral locked in the position.
    pub total_collateral_locked: Sq128Raw,
    /// Position flags bitfield.
    pub flags: u32,
    /// `true` when the position record exists.
    pub exists: bool,
    /// `true` once the position has been claimed.
    pub claimed: bool,
    /// `true` when the position currently tracks a pending settlement claim.
    pub tracks_settlement_claim: bool,
}

impl CairoSerde for PositionSummaryRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.total_collateral_locked.encode(out);
        self.flags.encode(out);
        self.exists.encode(out);
        self.claimed.encode(out);
        self.tracks_settlement_claim.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (total_collateral_locked, slice) = Sq128Raw::decode(slice)?;
        let (flags, slice) = u32::decode(slice)?;
        let (exists, slice) = bool::decode(slice)?;
        let (claimed, slice) = bool::decode(slice)?;
        let (tracks_settlement_claim, slice) = bool::decode(slice)?;
        Ok((
            Self {
                total_collateral_locked,
                flags,
                exists,
                claimed,
                tracks_settlement_claim,
            },
            slice,
        ))
    }
}

/// Compact position record for a normal AMM trader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PositionCompactRaw {
    /// μ at entry.
    pub original_mean: Sq128Raw,
    /// σ² at entry.
    pub original_variance: Sq128Raw,
    /// σ at entry.
    pub original_sigma: Sq128Raw,
    /// λ at entry.
    pub original_lambda: Sq128Raw,
    /// Effective μ (after subsequent updates).
    pub effective_mean: Sq128Raw,
    /// Effective σ².
    pub effective_variance: Sq128Raw,
    /// Effective σ.
    pub effective_sigma: Sq128Raw,
    /// Effective λ.
    pub effective_lambda: Sq128Raw,
    /// Total collateral committed.
    pub total_collateral: Sq128Raw,
    /// Position flags bitfield.
    pub flags: u32,
}

impl CairoSerde for PositionCompactRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.original_mean.encode(out);
        self.original_variance.encode(out);
        self.original_sigma.encode(out);
        self.original_lambda.encode(out);
        self.effective_mean.encode(out);
        self.effective_variance.encode(out);
        self.effective_sigma.encode(out);
        self.effective_lambda.encode(out);
        self.total_collateral.encode(out);
        self.flags.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (original_mean, slice) = Sq128Raw::decode(slice)?;
        let (original_variance, slice) = Sq128Raw::decode(slice)?;
        let (original_sigma, slice) = Sq128Raw::decode(slice)?;
        let (original_lambda, slice) = Sq128Raw::decode(slice)?;
        let (effective_mean, slice) = Sq128Raw::decode(slice)?;
        let (effective_variance, slice) = Sq128Raw::decode(slice)?;
        let (effective_sigma, slice) = Sq128Raw::decode(slice)?;
        let (effective_lambda, slice) = Sq128Raw::decode(slice)?;
        let (total_collateral, slice) = Sq128Raw::decode(slice)?;
        let (flags, slice) = u32::decode(slice)?;
        Ok((
            Self {
                original_mean,
                original_variance,
                original_sigma,
                original_lambda,
                effective_mean,
                effective_variance,
                effective_sigma,
                effective_lambda,
                total_collateral,
                flags,
            },
            slice,
        ))
    }
}

// ─── Sell-side guards + result ───────────────────────────────────────────────

/// Pre-flight guards supplied to `sell_position_guarded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SellExecutionGuardsRaw {
    /// Distribution the caller expected the market to be in.
    pub expected_market_dist: NormalDistributionRaw,
    /// Expected backing.
    pub expected_backing: Sq128Raw,
    /// Expected tolerance.
    pub expected_tolerance: Sq128Raw,
    /// Expected min trade collateral.
    pub expected_min_trade_collateral: Sq128Raw,
    /// Minimum token amount the caller is willing to accept.
    pub min_token_out: u128,
}

impl CairoSerde for SellExecutionGuardsRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.expected_market_dist.encode(out);
        self.expected_backing.encode(out);
        self.expected_tolerance.encode(out);
        self.expected_min_trade_collateral.encode(out);
        // `min_token_out` is `core::integer::u256` on chain — 2 felts
        // (low, high). The Rust field is `u128`, which always fits in
        // the low limb; the high limb is zero.
        out.push(Felt::from(self.min_token_out));
        out.push(Felt::ZERO);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (expected_market_dist, slice) = NormalDistributionRaw::decode(slice)?;
        let (expected_backing, slice) = Sq128Raw::decode(slice)?;
        let (expected_tolerance, slice) = Sq128Raw::decode(slice)?;
        let (expected_min_trade_collateral, slice) = Sq128Raw::decode(slice)?;
        // u256 low / high — assume high == 0 (our Rust field is u128).
        let (min_token_out, slice) = u128::decode(slice)?;
        let (_high, slice) = u128::decode(slice)?;
        Ok((
            Self {
                expected_market_dist,
                expected_backing,
                expected_tolerance,
                expected_min_trade_collateral,
                min_token_out,
            },
            slice,
        ))
    }
}

/// Result returned by `sell_position_guarded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PositionSellResultRaw {
    /// Whether the sell actually executed.
    pub sell_executed: bool,
    /// Embedded trade-execution payload.
    pub trade_result: TradeExecutionRaw,
    /// Whether the resulting position is now flat.
    pub is_flat: bool,
    /// Collateral before the sell.
    pub collateral_before: Sq128Raw,
    /// Collateral after the sell.
    pub collateral_after: Sq128Raw,
    /// Collateral released to the trader.
    pub collateral_out: Sq128Raw,
    /// Token amount transferred (u128).
    pub token_out: u128,
    /// Symbolic rejection reason (None if executed).
    pub rejection_reason: PositionSellRejection,
}

impl CairoSerde for PositionSellResultRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.sell_executed.encode(out);
        self.trade_result.encode(out);
        self.is_flat.encode(out);
        self.collateral_before.encode(out);
        self.collateral_after.encode(out);
        self.collateral_out.encode(out);
        self.token_out.encode(out);
        self.rejection_reason.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (sell_executed, slice) = bool::decode(slice)?;
        let (trade_result, slice) = TradeExecutionRaw::decode(slice)?;
        let (is_flat, slice) = bool::decode(slice)?;
        let (collateral_before, slice) = Sq128Raw::decode(slice)?;
        let (collateral_after, slice) = Sq128Raw::decode(slice)?;
        let (collateral_out, slice) = Sq128Raw::decode(slice)?;
        let (token_out, slice) = u128::decode(slice)?;
        let (rejection_reason, slice) = PositionSellRejection::decode(slice)?;
        Ok((
            Self {
                sell_executed,
                trade_result,
                is_flat,
                collateral_before,
                collateral_after,
                collateral_out,
                token_out,
                rejection_reason,
            },
            slice,
        ))
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use super::*;

    fn sq(limb2: u64) -> Sq128Raw {
        Sq128Raw {
            limb0: 0,
            limb1: 0,
            limb2,
            limb3: 0,
            neg: false,
        }
    }

    #[test]
    fn trade_input_round_trip() {
        let input = TradeInput {
            candidate: NormalDistributionRaw {
                mean: sq(100),
                variance: sq(4),
                sigma: sq(2),
            },
            x_star: sq(101),
            supplied_collateral: sq(5),
            candidate_hints: NormalSqrtHintsRaw {
                l2_norm_denom: sq(7),
                backing_denom: sq(9),
            },
        };
        let cd = input.to_calldata();
        let (back, rest) = TradeInput::decode(&cd).unwrap();
        assert!(rest.is_empty());
        assert_eq!(back, input);
    }

    #[test]
    fn trade_rejection_round_trips() {
        for variant in [
            TradeRejection::None,
            TradeRejection::InvalidDistribution,
            TradeRejection::InvalidHints,
            TradeRejection::BackingFail,
            TradeRejection::SigmaTooLow,
            TradeRejection::LowCollateral,
            TradeRejection::VerificationFailed,
        ] {
            let cd = variant.to_calldata();
            let (back, rest) = TradeRejection::decode(&cd).unwrap();
            assert!(rest.is_empty());
            assert_eq!(back, variant);
        }
    }

    #[test]
    fn position_compact_round_trip() {
        let pos = PositionCompactRaw {
            original_mean: sq(100),
            original_variance: sq(4),
            original_sigma: sq(2),
            original_lambda: sq(50),
            effective_mean: sq(101),
            effective_variance: sq(4),
            effective_sigma: sq(2),
            effective_lambda: sq(60),
            total_collateral: sq(7),
            flags: 0b101,
        };
        let cd = pos.to_calldata();
        let (back, rest) = PositionCompactRaw::decode(&cd).unwrap();
        assert!(rest.is_empty());
        assert_eq!(back, pos);
    }

    #[test]
    fn invalid_enum_tag_rejected() {
        let arr = [Felt::from(42_u64)];
        let result = TradeRejection::decode(&arr);
        assert!(matches!(
            result,
            Err(CairoSerdeError::InvalidEnumTag { .. })
        ));
    }
}
