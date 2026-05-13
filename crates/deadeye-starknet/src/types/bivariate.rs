//! Bivariate-market specific Cairo Serde shapes.
//!
//! Mirrors `@the-situation/abi`'s `bivariate.ts`.

use deadeye_core::{
    bivariate::{
        BivariateNormalDistributionCoreRaw, BivariateNormalDistributionRaw,
        BivariateNormalSqrtHintsRaw, BivariatePointRaw,
    },
    sq128::Sq128Raw,
};
use starknet_core::types::Felt;

use crate::{
    cairo_serde::{CairoSerde, CairoSerdeError},
    types::common::PositionSellRejection,
};

impl CairoSerde for BivariatePointRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.x1.encode(out);
        self.x2.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (x1, slice) = Sq128Raw::decode(slice)?;
        let (x2, slice) = Sq128Raw::decode(slice)?;
        Ok((Self { x1, x2 }, slice))
    }
}

impl CairoSerde for BivariateNormalDistributionCoreRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.mu1.encode(out);
        self.mu2.encode(out);
        self.variance1.encode(out);
        self.variance2.encode(out);
        self.rho.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (mu1, slice) = Sq128Raw::decode(slice)?;
        let (mu2, slice) = Sq128Raw::decode(slice)?;
        let (variance1, slice) = Sq128Raw::decode(slice)?;
        let (variance2, slice) = Sq128Raw::decode(slice)?;
        let (rho, slice) = Sq128Raw::decode(slice)?;
        Ok((
            Self {
                mu1,
                mu2,
                variance1,
                variance2,
                rho,
            },
            slice,
        ))
    }
}

impl CairoSerde for BivariateNormalDistributionRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        // Cairo order: mu1, mu2, variance1, variance2, sigma1, sigma2, rho,
        // inv_one_minus_rho_sq, normalization.
        self.mu1.encode(out);
        self.mu2.encode(out);
        self.variance1.encode(out);
        self.variance2.encode(out);
        self.sigma1.encode(out);
        self.sigma2.encode(out);
        self.rho.encode(out);
        self.inv_one_minus_rho_sq.encode(out);
        self.normalization.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (mu1, slice) = Sq128Raw::decode(slice)?;
        let (mu2, slice) = Sq128Raw::decode(slice)?;
        let (variance1, slice) = Sq128Raw::decode(slice)?;
        let (variance2, slice) = Sq128Raw::decode(slice)?;
        let (sigma1, slice) = Sq128Raw::decode(slice)?;
        let (sigma2, slice) = Sq128Raw::decode(slice)?;
        let (rho, slice) = Sq128Raw::decode(slice)?;
        let (inv_one_minus_rho_sq, slice) = Sq128Raw::decode(slice)?;
        let (normalization, slice) = Sq128Raw::decode(slice)?;
        Ok((
            Self {
                mu1,
                mu2,
                variance1,
                variance2,
                rho,
                sigma1,
                sigma2,
                inv_one_minus_rho_sq,
                normalization,
            },
            slice,
        ))
    }
}

impl CairoSerde for BivariateNormalSqrtHintsRaw {
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

/// Bivariate market status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BivariateMarketStatusRaw {
    /// Whether `initialize()` has been called.
    pub is_initialised: bool,
    /// Whether the market is paused.
    pub is_paused: bool,
    /// Whether the market has been settled.
    pub is_settled: bool,
    /// Settlement point.
    pub settlement_point: BivariatePointRaw,
}

impl CairoSerde for BivariateMarketStatusRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.is_initialised.encode(out);
        self.is_paused.encode(out);
        self.is_settled.encode(out);
        self.settlement_point.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (is_initialised, slice) = bool::decode(slice)?;
        let (is_paused, slice) = bool::decode(slice)?;
        let (is_settled, slice) = bool::decode(slice)?;
        let (settlement_point, slice) = BivariatePointRaw::decode(slice)?;
        Ok((
            Self {
                is_initialised,
                is_paused,
                is_settled,
                settlement_point,
            },
            slice,
        ))
    }
}

/// Bivariate AMM `execute_trade` input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BivariateTradeInput {
    /// Candidate distribution (full 9-field shape).
    pub candidate: BivariateNormalDistributionRaw,
    /// `x*` location of the minimum (2D point).
    pub x_star: BivariatePointRaw,
    /// Collateral supplied.
    pub supplied_collateral: Sq128Raw,
    /// Sqrt hints.
    pub candidate_hints: BivariateNormalSqrtHintsRaw,
}

impl CairoSerde for BivariateTradeInput {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.candidate.encode(out);
        self.x_star.encode(out);
        self.supplied_collateral.encode(out);
        self.candidate_hints.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (candidate, slice) = BivariateNormalDistributionRaw::decode(slice)?;
        let (x_star, slice) = BivariatePointRaw::decode(slice)?;
        let (supplied_collateral, slice) = Sq128Raw::decode(slice)?;
        let (candidate_hints, slice) = BivariateNormalSqrtHintsRaw::decode(slice)?;
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

/// Bivariate compact position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BivariateNormalPositionCompactRaw {
    /// Entry distribution (core fields only).
    pub original_dist: BivariateNormalDistributionCoreRaw,
    /// Entry λ.
    pub original_lambda: Sq128Raw,
    /// Effective distribution.
    pub effective_dist: BivariateNormalDistributionCoreRaw,
    /// Effective λ.
    pub effective_lambda: Sq128Raw,
    /// Total collateral.
    pub total_collateral: Sq128Raw,
    /// Flags bitfield.
    pub flags: u32,
}

impl CairoSerde for BivariateNormalPositionCompactRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.original_dist.encode(out);
        self.original_lambda.encode(out);
        self.effective_dist.encode(out);
        self.effective_lambda.encode(out);
        self.total_collateral.encode(out);
        self.flags.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (original_dist, slice) = BivariateNormalDistributionCoreRaw::decode(slice)?;
        let (original_lambda, slice) = Sq128Raw::decode(slice)?;
        let (effective_dist, slice) = BivariateNormalDistributionCoreRaw::decode(slice)?;
        let (effective_lambda, slice) = Sq128Raw::decode(slice)?;
        let (total_collateral, slice) = Sq128Raw::decode(slice)?;
        let (flags, slice) = u32::decode(slice)?;
        Ok((
            Self {
                original_dist,
                original_lambda,
                effective_dist,
                effective_lambda,
                total_collateral,
                flags,
            },
            slice,
        ))
    }
}

/// Bivariate sell guards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BivariateNormalSellExecutionGuardsRaw {
    /// Expected market distribution (core shape).
    pub expected_market_dist: BivariateNormalDistributionCoreRaw,
    /// Expected backing.
    pub expected_backing: Sq128Raw,
    /// Expected tolerance.
    pub expected_tolerance: Sq128Raw,
    /// Expected min trade collateral.
    pub expected_min_trade_collateral: Sq128Raw,
    /// Min token out.
    pub min_token_out: u128,
}

impl CairoSerde for BivariateNormalSellExecutionGuardsRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.expected_market_dist.encode(out);
        self.expected_backing.encode(out);
        self.expected_tolerance.encode(out);
        self.expected_min_trade_collateral.encode(out);
        // `min_token_out` is `core::integer::u256` on chain — encode 2
        // felts (low, high). Same fix as
        // `types/normal.rs::SellExecutionGuardsRaw::encode`.
        out.push(Felt::from(self.min_token_out));
        out.push(Felt::ZERO);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (expected_market_dist, slice) = BivariateNormalDistributionCoreRaw::decode(slice)?;
        let (expected_backing, slice) = Sq128Raw::decode(slice)?;
        let (expected_tolerance, slice) = Sq128Raw::decode(slice)?;
        let (expected_min_trade_collateral, slice) = Sq128Raw::decode(slice)?;
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

/// Bivariate position-sell result. Re-uses [`PositionSellRejection`] from
/// `common` (same enum is shared across normal / lognormal / bivariate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BivariatePositionSellResultRaw {
    /// Whether the sell executed.
    pub sell_executed: bool,
    /// Is now flat.
    pub is_flat: bool,
    /// Token amount transferred.
    pub token_out: u128,
    /// Collateral before.
    pub collateral_before: Sq128Raw,
    /// Collateral after.
    pub collateral_after: Sq128Raw,
    /// Collateral released.
    pub collateral_out: Sq128Raw,
    /// Symbolic rejection reason.
    pub rejection_reason: PositionSellRejection,
}

impl CairoSerde for BivariatePositionSellResultRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.sell_executed.encode(out);
        self.is_flat.encode(out);
        self.token_out.encode(out);
        self.collateral_before.encode(out);
        self.collateral_after.encode(out);
        self.collateral_out.encode(out);
        self.rejection_reason.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (sell_executed, slice) = bool::decode(slice)?;
        let (is_flat, slice) = bool::decode(slice)?;
        let (token_out, slice) = u128::decode(slice)?;
        let (collateral_before, slice) = Sq128Raw::decode(slice)?;
        let (collateral_after, slice) = Sq128Raw::decode(slice)?;
        let (collateral_out, slice) = Sq128Raw::decode(slice)?;
        let (rejection_reason, slice) = PositionSellRejection::decode(slice)?;
        Ok((
            Self {
                sell_executed,
                is_flat,
                token_out,
                collateral_before,
                collateral_after,
                collateral_out,
                rejection_reason,
            },
            slice,
        ))
    }
}
