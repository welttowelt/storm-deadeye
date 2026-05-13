//! Lognormal-market specific Cairo Serde shapes.
//!
//! Mirrors `@the-situation/abi`'s `lognormal.ts`.

use deadeye_core::{distribution::LognormalDistributionRaw, sq128::Sq128Raw};
use starknet_core::types::Felt;

use crate::{
    cairo_serde::{CairoSerde, CairoSerdeError},
    types::common::{
        CollateralVerificationRaw, PositionSellRejection, ScaledBackingCheckRaw, TradeRejection,
    },
};

impl CairoSerde for LognormalDistributionRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.mu.encode(out);
        self.variance.encode(out);
        self.sigma.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (mu, slice) = Sq128Raw::decode(slice)?;
        let (variance, slice) = Sq128Raw::decode(slice)?;
        let (sigma, slice) = Sq128Raw::decode(slice)?;
        Ok((
            Self {
                mu,
                variance,
                sigma,
            },
            slice,
        ))
    }
}

/// Lognormal core (variance only, σ not stored).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LognormalDistributionCoreRaw {
    /// Log-space mean.
    pub mu: Sq128Raw,
    /// Log-space variance.
    pub variance: Sq128Raw,
}

impl CairoSerde for LognormalDistributionCoreRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.mu.encode(out);
        self.variance.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (mu, slice) = Sq128Raw::decode(slice)?;
        let (variance, slice) = Sq128Raw::decode(slice)?;
        Ok((Self { mu, variance }, slice))
    }
}

/// Sqrt hints for lognormal markets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LognormalSqrtHintsRaw {
    /// `1 / ||f||₂²`.
    pub l2_norm_denom: Sq128Raw,
    /// `1 / backing`.
    pub backing_denom: Sq128Raw,
}

impl CairoSerde for LognormalSqrtHintsRaw {
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

/// Lognormal AMM `execute_trade` input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LognormalTradeInput {
    /// Candidate distribution.
    pub candidate: LognormalDistributionRaw,
    /// `x* > 0` for the minimum of `d(x) = g(x) - f(x)`.
    pub x_star: Sq128Raw,
    /// Collateral supplied.
    pub supplied_collateral: Sq128Raw,
    /// Square-root hints.
    pub candidate_hints: LognormalSqrtHintsRaw,
}

impl CairoSerde for LognormalTradeInput {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.candidate.encode(out);
        self.x_star.encode(out);
        self.supplied_collateral.encode(out);
        self.candidate_hints.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (candidate, slice) = LognormalDistributionRaw::decode(slice)?;
        let (x_star, slice) = Sq128Raw::decode(slice)?;
        let (supplied_collateral, slice) = Sq128Raw::decode(slice)?;
        let (candidate_hints, slice) = LognormalSqrtHintsRaw::decode(slice)?;
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

/// Lognormal trade check + lambdas (lognormal runtime returns extra lambda projections).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LognormalTradeCheckWithLambdasRaw {
    /// Embedded standard trade check.
    pub check: LognormalTradeCheckRaw,
    /// Lambda for the current distribution.
    pub current_lambda: Sq128Raw,
    /// Lambda for the candidate distribution.
    pub candidate_lambda: Sq128Raw,
    /// Whether the lambda projections validated.
    pub lambdas_valid: bool,
}

impl CairoSerde for LognormalTradeCheckWithLambdasRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.check.encode(out);
        self.current_lambda.encode(out);
        self.candidate_lambda.encode(out);
        self.lambdas_valid.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (check, slice) = LognormalTradeCheckRaw::decode(slice)?;
        let (current_lambda, slice) = Sq128Raw::decode(slice)?;
        let (candidate_lambda, slice) = Sq128Raw::decode(slice)?;
        let (lambdas_valid, slice) = bool::decode(slice)?;
        Ok((
            Self {
                check,
                current_lambda,
                candidate_lambda,
                lambdas_valid,
            },
            slice,
        ))
    }
}

/// Lognormal AMM trade check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LognormalTradeCheckRaw {
    /// Backing check.
    pub backing_check: ScaledBackingCheckRaw,
    /// Collateral verification.
    pub verification: CollateralVerificationRaw,
    /// Floor on trade collateral.
    pub min_trade_collateral: Sq128Raw,
    /// Whether the supplied collateral cleared the floor.
    pub collateral_above_min: bool,
    /// Aggregate validity.
    pub is_valid: bool,
    /// Symbolic rejection.
    pub rejection_reason: TradeRejection,
}

impl CairoSerde for LognormalTradeCheckRaw {
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

/// Lognormal AMM trade execution payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LognormalTradeExecutionRaw {
    /// Embedded check.
    pub check: LognormalTradeCheckRaw,
    /// ERC20 amount transferred.
    pub token_amount: u128,
}

impl CairoSerde for LognormalTradeExecutionRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.check.encode(out);
        self.token_amount.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (check, slice) = LognormalTradeCheckRaw::decode(slice)?;
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

/// Lognormal compact position record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LognormalPositionCompactRaw {
    /// μ at entry.
    pub original_mu: Sq128Raw,
    /// σ² at entry.
    pub original_variance: Sq128Raw,
    /// σ at entry.
    pub original_sigma: Sq128Raw,
    /// λ at entry.
    pub original_lambda: Sq128Raw,
    /// Effective μ.
    pub effective_mu: Sq128Raw,
    /// Effective σ².
    pub effective_variance: Sq128Raw,
    /// Effective σ.
    pub effective_sigma: Sq128Raw,
    /// Effective λ.
    pub effective_lambda: Sq128Raw,
    /// Total collateral.
    pub total_collateral: Sq128Raw,
    /// Flags.
    pub flags: u32,
}

impl CairoSerde for LognormalPositionCompactRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.original_mu.encode(out);
        self.original_variance.encode(out);
        self.original_sigma.encode(out);
        self.original_lambda.encode(out);
        self.effective_mu.encode(out);
        self.effective_variance.encode(out);
        self.effective_sigma.encode(out);
        self.effective_lambda.encode(out);
        self.total_collateral.encode(out);
        self.flags.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (original_mu, slice) = Sq128Raw::decode(slice)?;
        let (original_variance, slice) = Sq128Raw::decode(slice)?;
        let (original_sigma, slice) = Sq128Raw::decode(slice)?;
        let (original_lambda, slice) = Sq128Raw::decode(slice)?;
        let (effective_mu, slice) = Sq128Raw::decode(slice)?;
        let (effective_variance, slice) = Sq128Raw::decode(slice)?;
        let (effective_sigma, slice) = Sq128Raw::decode(slice)?;
        let (effective_lambda, slice) = Sq128Raw::decode(slice)?;
        let (total_collateral, slice) = Sq128Raw::decode(slice)?;
        let (flags, slice) = u32::decode(slice)?;
        Ok((
            Self {
                original_mu,
                original_variance,
                original_sigma,
                original_lambda,
                effective_mu,
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

/// Lognormal sell guards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LognormalSellExecutionGuardsRaw {
    /// Expected (core) distribution.
    pub expected_market_dist: LognormalDistributionCoreRaw,
    /// Expected backing.
    pub expected_backing: Sq128Raw,
    /// Expected tolerance.
    pub expected_tolerance: Sq128Raw,
    /// Expected min trade collateral.
    pub expected_min_trade_collateral: Sq128Raw,
    /// Minimum token out.
    pub min_token_out: u128,
}

impl CairoSerde for LognormalSellExecutionGuardsRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.expected_market_dist.encode(out);
        self.expected_backing.encode(out);
        self.expected_tolerance.encode(out);
        self.expected_min_trade_collateral.encode(out);
        // `min_token_out` is `core::integer::u256` on chain — 2 felts
        // (low, high). The Rust field is `u128`, which always fits in
        // the low limb; the high limb is zero. Same fix as
        // `SellExecutionGuardsRaw` for the normal AMM (see
        // `types/normal.rs:248-258`). Without this, the chain decoder
        // panics with `'Failed to deserialize param #4'` on
        // `sell_position_guarded`.
        out.push(Felt::from(self.min_token_out));
        out.push(Felt::ZERO);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (expected_market_dist, slice) = LognormalDistributionCoreRaw::decode(slice)?;
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

/// Lognormal position-sell result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LognormalPositionSellResultRaw {
    /// Whether the sell actually executed.
    pub sell_executed: bool,
    /// Trade-execution payload.
    pub trade_result: LognormalTradeExecutionRaw,
    /// Whether the position is now flat.
    pub is_flat: bool,
    /// Collateral before.
    pub collateral_before: Sq128Raw,
    /// Collateral after.
    pub collateral_after: Sq128Raw,
    /// Collateral released.
    pub collateral_out: Sq128Raw,
    /// Tokens transferred.
    pub token_out: u128,
    /// Symbolic rejection reason.
    pub rejection_reason: PositionSellRejection,
}

impl CairoSerde for LognormalPositionSellResultRaw {
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
        let (trade_result, slice) = LognormalTradeExecutionRaw::decode(slice)?;
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
