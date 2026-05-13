//! Multinoulli-market specific Cairo Serde shapes.
//!
//! Mirrors `@the-situation/abi`'s `multinoulli.ts`.

use deadeye_core::{
    categorical::{CategoricalDistributionRaw, CategoricalL2HintRaw},
    sq128::Sq128Raw,
};
use starknet_core::types::Felt;

use crate::{
    cairo_serde::{CairoSerde, CairoSerdeError},
    cairo_serde_unit_enum,
    types::common::ScaledBackingCheckRaw,
};

// ─── Categorical helpers ─────────────────────────────────────────────────────

impl CairoSerde for CategoricalDistributionRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.probs.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (probs, slice) = Vec::<Sq128Raw>::decode(slice)?;
        Ok((Self { probs }, slice))
    }
}

impl CairoSerde for CategoricalL2HintRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.l2_norm_hint.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (l2_norm_hint, slice) = Sq128Raw::decode(slice)?;
        Ok((Self { l2_norm_hint }, slice))
    }
}

/// Sparse-update payload for one outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CategoricalProbUpdateRaw {
    /// Outcome index being updated.
    pub outcome_index: u32,
    /// New probability for this outcome.
    pub prob: Sq128Raw,
}

impl CairoSerde for CategoricalProbUpdateRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.outcome_index.encode(out);
        self.prob.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (outcome_index, slice) = u32::decode(slice)?;
        let (prob, slice) = Sq128Raw::decode(slice)?;
        Ok((
            Self {
                outcome_index,
                prob,
            },
            slice,
        ))
    }
}

/// Mass-conserving transfer between two outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CategoricalProbTransferRaw {
    /// Source outcome index.
    pub from_outcome_index: u32,
    /// Destination outcome index.
    pub to_outcome_index: u32,
    /// Mass transferred (subtracted from `from`, added to `to`).
    pub delta: Sq128Raw,
}

impl CairoSerde for CategoricalProbTransferRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.from_outcome_index.encode(out);
        self.to_outcome_index.encode(out);
        self.delta.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (from_outcome_index, slice) = u32::decode(slice)?;
        let (to_outcome_index, slice) = u32::decode(slice)?;
        let (delta, slice) = Sq128Raw::decode(slice)?;
        Ok((
            Self {
                from_outcome_index,
                to_outcome_index,
                delta,
            },
            slice,
        ))
    }
}

// ─── Status + matrix constraints ─────────────────────────────────────────────

/// Multinoulli market status (`get_market_status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MultinoulliMarketStatusRaw {
    /// Whether `initialize()` has been called.
    pub is_initialised: bool,
    /// Whether the market is paused.
    pub is_paused: bool,
    /// Whether the market has been settled.
    pub is_settled: bool,
    /// Settlement outcome index (u32; only meaningful when `is_settled`).
    pub settlement_outcome_index: u32,
}

impl CairoSerde for MultinoulliMarketStatusRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.is_initialised.encode(out);
        self.is_paused.encode(out);
        self.is_settled.encode(out);
        self.settlement_outcome_index.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (is_initialised, slice) = bool::decode(slice)?;
        let (is_paused, slice) = bool::decode(slice)?;
        let (is_settled, slice) = bool::decode(slice)?;
        let (settlement_outcome_index, slice) = u32::decode(slice)?;
        Ok((
            Self {
                is_initialised,
                is_paused,
                is_settled,
                settlement_outcome_index,
            },
            slice,
        ))
    }
}

/// Grid constraint mode for multinoulli markets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MultinoulliMatrixConstraintMode {
    /// Flat simplex, no grid constraint.
    Disabled,
    /// Doubly-stochastic (rows AND columns sum to their marginals).
    RowAndCol,
    /// Each row sums to `1/rowCount`.
    RowOnly,
    /// Each column sums to `1/colCount`.
    ColOnly,
}

cairo_serde_unit_enum!(MultinoulliMatrixConstraintMode {
    Disabled = 0,
    RowAndCol = 1,
    RowOnly = 2,
    ColOnly = 3,
});

/// Matrix-constraint configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MultinoulliMatrixConstraintsRaw {
    /// Constraint mode.
    pub mode: MultinoulliMatrixConstraintMode,
    /// Row count (0 when `Disabled`).
    pub row_count: u32,
    /// Column count (0 when `Disabled`).
    pub col_count: u32,
}

impl CairoSerde for MultinoulliMatrixConstraintsRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.mode.encode(out);
        self.row_count.encode(out);
        self.col_count.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (mode, slice) = MultinoulliMatrixConstraintMode::decode(slice)?;
        let (row_count, slice) = u32::decode(slice)?;
        let (col_count, slice) = u32::decode(slice)?;
        Ok((
            Self {
                mode,
                row_count,
                col_count,
            },
            slice,
        ))
    }
}

// ─── Position types ──────────────────────────────────────────────────────────

/// Lightweight summary of a multinoulli trader's position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MultinoulliPositionSummaryRaw {
    /// Total collateral locked.
    pub total_collateral_locked: Sq128Raw,
    /// Position flags bitfield.
    pub flags: u32,
    /// Whether the position record exists.
    pub exists: bool,
    /// Whether the position has been claimed.
    pub claimed: bool,
    /// Whether the position tracks a pending settlement claim.
    pub tracks_settlement_claim: bool,
    /// Outcome count of the underlying market.
    pub outcome_count: u32,
}

impl CairoSerde for MultinoulliPositionSummaryRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.total_collateral_locked.encode(out);
        self.flags.encode(out);
        self.exists.encode(out);
        self.claimed.encode(out);
        self.tracks_settlement_claim.encode(out);
        self.outcome_count.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (total_collateral_locked, slice) = Sq128Raw::decode(slice)?;
        let (flags, slice) = u32::decode(slice)?;
        let (exists, slice) = bool::decode(slice)?;
        let (claimed, slice) = bool::decode(slice)?;
        let (tracks_settlement_claim, slice) = bool::decode(slice)?;
        let (outcome_count, slice) = u32::decode(slice)?;
        Ok((
            Self {
                total_collateral_locked,
                flags,
                exists,
                claimed,
                tracks_settlement_claim,
                outcome_count,
            },
            slice,
        ))
    }
}

/// Compact position record for a multinoulli AMM trader.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MultinoulliPositionCompactRaw {
    /// Entry distribution.
    pub original_distribution: CategoricalDistributionRaw,
    /// Entry λ.
    pub original_lambda: Sq128Raw,
    /// Current effective distribution.
    pub effective_distribution: CategoricalDistributionRaw,
    /// Current effective λ.
    pub effective_lambda: Sq128Raw,
    /// Total collateral.
    pub total_collateral: Sq128Raw,
    /// Position flags.
    pub flags: u32,
}

impl CairoSerde for MultinoulliPositionCompactRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.original_distribution.encode(out);
        self.original_lambda.encode(out);
        self.effective_distribution.encode(out);
        self.effective_lambda.encode(out);
        self.total_collateral.encode(out);
        self.flags.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (original_distribution, slice) = CategoricalDistributionRaw::decode(slice)?;
        let (original_lambda, slice) = Sq128Raw::decode(slice)?;
        let (effective_distribution, slice) = CategoricalDistributionRaw::decode(slice)?;
        let (effective_lambda, slice) = Sq128Raw::decode(slice)?;
        let (total_collateral, slice) = Sq128Raw::decode(slice)?;
        let (flags, slice) = u32::decode(slice)?;
        Ok((
            Self {
                original_distribution,
                original_lambda,
                effective_distribution,
                effective_lambda,
                total_collateral,
                flags,
            },
            slice,
        ))
    }
}

// ─── Trade + sell flows ──────────────────────────────────────────────────────

/// Multinoulli verification result (discrete enumeration).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MultinoulliCollateralVerificationRaw {
    /// Whether the supplied minimum-outcome hint was valid.
    pub minimum_valid: bool,
    /// Collateral the runtime computed.
    pub computed_collateral: Sq128Raw,
    /// Whether the supplied collateral was sufficient.
    pub collateral_sufficient: bool,
    /// Whether the overall computation succeeded.
    pub computation_valid: bool,
}

impl CairoSerde for MultinoulliCollateralVerificationRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.minimum_valid.encode(out);
        self.computed_collateral.encode(out);
        self.collateral_sufficient.encode(out);
        self.computation_valid.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (minimum_valid, slice) = bool::decode(slice)?;
        let (computed_collateral, slice) = Sq128Raw::decode(slice)?;
        let (collateral_sufficient, slice) = bool::decode(slice)?;
        let (computation_valid, slice) = bool::decode(slice)?;
        Ok((
            Self {
                minimum_valid,
                computed_collateral,
                collateral_sufficient,
                computation_valid,
            },
            slice,
        ))
    }
}

/// Trade rejection reasons for multinoulli AMMs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MultinoulliTradeRejection {
    /// Accepted.
    None,
    /// Distribution failed validation.
    InvalidDistribution,
    /// L2 hint did not validate.
    InvalidHints,
    /// `min_outcome_index` hint was wrong.
    InvalidMinOutcome,
    /// Backing check failed.
    BackingFail,
    /// Collateral below `min_trade_collateral`.
    LowCollateral,
    /// Verification failed.
    VerificationFailed,
}

cairo_serde_unit_enum!(MultinoulliTradeRejection {
    None = 0,
    InvalidDistribution = 1,
    InvalidHints = 2,
    InvalidMinOutcome = 3,
    BackingFail = 4,
    LowCollateral = 5,
    VerificationFailed = 6,
});

/// Multinoulli trade check payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MultinoulliTradeCheckRaw {
    /// Embedded backing check.
    pub backing_check: ScaledBackingCheckRaw,
    /// Embedded verification.
    pub verification: MultinoulliCollateralVerificationRaw,
    /// Per-market floor on trade collateral.
    pub min_trade_collateral: Sq128Raw,
    /// Whether supplied collateral cleared the floor.
    pub collateral_above_min: bool,
    /// Aggregate validity bit.
    pub is_valid: bool,
    /// Symbolic rejection reason.
    pub rejection_reason: MultinoulliTradeRejection,
}

impl CairoSerde for MultinoulliTradeCheckRaw {
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
        let (verification, slice) = MultinoulliCollateralVerificationRaw::decode(slice)?;
        let (min_trade_collateral, slice) = Sq128Raw::decode(slice)?;
        let (collateral_above_min, slice) = bool::decode(slice)?;
        let (is_valid, slice) = bool::decode(slice)?;
        let (rejection_reason, slice) = MultinoulliTradeRejection::decode(slice)?;
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

/// Trade execution payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MultinoulliTradeExecutionRaw {
    /// Embedded check outcome.
    pub check: MultinoulliTradeCheckRaw,
    /// ERC20 amount transferred.
    pub token_amount: u128,
}

impl CairoSerde for MultinoulliTradeExecutionRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.check.encode(out);
        self.token_amount.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (check, slice) = MultinoulliTradeCheckRaw::decode(slice)?;
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

// ─── Trade inputs (3 variants) ───────────────────────────────────────────────

/// Input to `execute_trade` on a multinoulli AMM.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MultinoulliTradeInput {
    /// Candidate distribution.
    pub candidate: CategoricalDistributionRaw,
    /// Outcome that minimises `λ_g·g_i − λ_f·f_i`.
    pub min_outcome_index: u32,
    /// Collateral supplied with this trade.
    pub supplied_collateral: Sq128Raw,
    /// L2 norm hint for the candidate.
    pub candidate_hint: CategoricalL2HintRaw,
}

impl CairoSerde for MultinoulliTradeInput {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.candidate.encode(out);
        self.min_outcome_index.encode(out);
        self.supplied_collateral.encode(out);
        self.candidate_hint.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (candidate, slice) = CategoricalDistributionRaw::decode(slice)?;
        let (min_outcome_index, slice) = u32::decode(slice)?;
        let (supplied_collateral, slice) = Sq128Raw::decode(slice)?;
        let (candidate_hint, slice) = CategoricalL2HintRaw::decode(slice)?;
        Ok((
            Self {
                candidate,
                min_outcome_index,
                supplied_collateral,
                candidate_hint,
            },
            slice,
        ))
    }
}

/// Input to `execute_trade_sparse` (only the changed outcomes).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MultinoulliTradeSparseInput {
    /// Sparse list of changed outcomes.
    pub candidate_updates: Vec<CategoricalProbUpdateRaw>,
    /// Outcome that minimises `λ_g·g_i − λ_f·f_i`.
    pub min_outcome_index: u32,
    /// Collateral supplied.
    pub supplied_collateral: Sq128Raw,
    /// L2 norm hint.
    pub candidate_hint: CategoricalL2HintRaw,
}

impl CairoSerde for MultinoulliTradeSparseInput {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.candidate_updates.encode(out);
        self.min_outcome_index.encode(out);
        self.supplied_collateral.encode(out);
        self.candidate_hint.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (candidate_updates, slice) = Vec::<CategoricalProbUpdateRaw>::decode(slice)?;
        let (min_outcome_index, slice) = u32::decode(slice)?;
        let (supplied_collateral, slice) = Sq128Raw::decode(slice)?;
        let (candidate_hint, slice) = CategoricalL2HintRaw::decode(slice)?;
        Ok((
            Self {
                candidate_updates,
                min_outcome_index,
                supplied_collateral,
                candidate_hint,
            },
            slice,
        ))
    }
}

/// Input to `execute_trade_transfers` (mass-conserving transfers).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MultinoulliTradeTransfersInput {
    /// Transfers to apply.
    pub transfers: Vec<CategoricalProbTransferRaw>,
    /// Outcome that minimises `λ_g·g_i − λ_f·f_i`.
    pub min_outcome_index: u32,
    /// Collateral supplied.
    pub supplied_collateral: Sq128Raw,
    /// L2 norm hint.
    pub candidate_hint: CategoricalL2HintRaw,
}

impl CairoSerde for MultinoulliTradeTransfersInput {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.transfers.encode(out);
        self.min_outcome_index.encode(out);
        self.supplied_collateral.encode(out);
        self.candidate_hint.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (transfers, slice) = Vec::<CategoricalProbTransferRaw>::decode(slice)?;
        let (min_outcome_index, slice) = u32::decode(slice)?;
        let (supplied_collateral, slice) = Sq128Raw::decode(slice)?;
        let (candidate_hint, slice) = CategoricalL2HintRaw::decode(slice)?;
        Ok((
            Self {
                transfers,
                min_outcome_index,
                supplied_collateral,
                candidate_hint,
            },
            slice,
        ))
    }
}

// ─── Sell-side ───────────────────────────────────────────────────────────────

/// Multinoulli position-sell rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MultinoulliPositionSellRejection {
    /// Accepted.
    None,
    /// No position.
    NoPosition,
    /// Market settled.
    MarketSettled,
    /// Market paused.
    MarketPaused,
    /// Already claimed.
    AlreadyClaimed,
    /// Position state invalid.
    InvalidPositionState,
    /// `min_outcome_index` hint was wrong.
    InvalidMinOutcome,
    /// Requires additional collateral.
    RequiresAdditionalCollateral,
    /// No collateral could be released.
    NoCollateralOut,
    /// Invalid hints.
    InvalidHints,
    /// Backing check failed.
    BackingFail,
    /// Verification failed.
    VerificationFailed,
    /// Conversion failed.
    ConversionFailed,
}

cairo_serde_unit_enum!(MultinoulliPositionSellRejection {
    None = 0,
    NoPosition = 1,
    MarketSettled = 2,
    MarketPaused = 3,
    AlreadyClaimed = 4,
    InvalidPositionState = 5,
    InvalidMinOutcome = 6,
    RequiresAdditionalCollateral = 7,
    NoCollateralOut = 8,
    InvalidHints = 9,
    BackingFail = 10,
    VerificationFailed = 11,
    ConversionFailed = 12,
});

/// Result of `sell_position_guarded` on a multinoulli AMM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MultinoulliPositionSellResultRaw {
    /// Whether the sell executed.
    pub sell_executed: bool,
    /// Embedded trade-execution payload.
    pub trade_result: MultinoulliTradeExecutionRaw,
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
    /// Symbolic rejection reason.
    pub rejection_reason: MultinoulliPositionSellRejection,
}

impl CairoSerde for MultinoulliPositionSellResultRaw {
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
        let (trade_result, slice) = MultinoulliTradeExecutionRaw::decode(slice)?;
        let (is_flat, slice) = bool::decode(slice)?;
        let (collateral_before, slice) = Sq128Raw::decode(slice)?;
        let (collateral_after, slice) = Sq128Raw::decode(slice)?;
        let (collateral_out, slice) = Sq128Raw::decode(slice)?;
        let (token_out, slice) = u128::decode(slice)?;
        let (rejection_reason, slice) = MultinoulliPositionSellRejection::decode(slice)?;
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

/// Guards supplied to `sell_position_guarded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MultinoulliSellExecutionGuardsRaw {
    /// Distribution snapshot id the caller expected to be current.
    pub expected_distribution_snapshot_id: u64,
    /// Expected backing.
    pub expected_backing: Sq128Raw,
    /// Expected tolerance.
    pub expected_tolerance: Sq128Raw,
    /// Expected min trade collateral.
    pub expected_min_trade_collateral: Sq128Raw,
    /// Minimum token amount the caller is willing to accept.
    pub min_token_out: u128,
}

impl CairoSerde for MultinoulliSellExecutionGuardsRaw {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.expected_distribution_snapshot_id.encode(out);
        self.expected_backing.encode(out);
        self.expected_tolerance.encode(out);
        self.expected_min_trade_collateral.encode(out);
        self.min_token_out.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (expected_distribution_snapshot_id, slice) = u64::decode(slice)?;
        let (expected_backing, slice) = Sq128Raw::decode(slice)?;
        let (expected_tolerance, slice) = Sq128Raw::decode(slice)?;
        let (expected_min_trade_collateral, slice) = Sq128Raw::decode(slice)?;
        let (min_token_out, slice) = u128::decode(slice)?;
        Ok((
            Self {
                expected_distribution_snapshot_id,
                expected_backing,
                expected_tolerance,
                expected_min_trade_collateral,
                min_token_out,
            },
            slice,
        ))
    }
}

/// Sparse-update payload for `sell_position_guarded_sparse`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MultinoulliSellPositionSparseInput {
    /// Sparse list of changed outcomes.
    pub candidate_updates: Vec<CategoricalProbUpdateRaw>,
    /// Outcome that minimises `λ_g·g_i − λ_f·f_i`.
    pub min_outcome_index: u32,
    /// L2 norm hint.
    pub candidate_hint: CategoricalL2HintRaw,
}

impl CairoSerde for MultinoulliSellPositionSparseInput {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.candidate_updates.encode(out);
        self.min_outcome_index.encode(out);
        self.candidate_hint.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), CairoSerdeError> {
        let (candidate_updates, slice) = Vec::<CategoricalProbUpdateRaw>::decode(slice)?;
        let (min_outcome_index, slice) = u32::decode(slice)?;
        let (candidate_hint, slice) = CategoricalL2HintRaw::decode(slice)?;
        Ok((
            Self {
                candidate_updates,
                min_outcome_index,
                candidate_hint,
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
    fn categorical_distribution_round_trip() {
        let raw = CategoricalDistributionRaw {
            probs: vec![sq(1), sq(2), sq(3), sq(4)],
        };
        let cd = raw.to_calldata();
        // Encoding = [length, ...elements] = 1 + 4*5 = 21 felts
        assert_eq!(cd.len(), 21);
        let (back, rest) = CategoricalDistributionRaw::decode(&cd).unwrap();
        assert!(rest.is_empty());
        assert_eq!(back, raw);
    }

    #[test]
    fn multinoulli_trade_input_round_trip() {
        let input = MultinoulliTradeInput {
            candidate: CategoricalDistributionRaw {
                probs: vec![sq(25), sq(25), sq(25), sq(25)],
            },
            min_outcome_index: 2,
            supplied_collateral: sq(5),
            candidate_hint: CategoricalL2HintRaw {
                l2_norm_hint: sq(50),
            },
        };
        let cd = input.to_calldata();
        let (back, rest) = MultinoulliTradeInput::decode(&cd).unwrap();
        assert!(rest.is_empty());
        assert_eq!(back, input);
    }

    #[test]
    fn matrix_mode_round_trips() {
        for mode in [
            MultinoulliMatrixConstraintMode::Disabled,
            MultinoulliMatrixConstraintMode::RowAndCol,
            MultinoulliMatrixConstraintMode::RowOnly,
            MultinoulliMatrixConstraintMode::ColOnly,
        ] {
            let cd = mode.to_calldata();
            let (back, rest) = MultinoulliMatrixConstraintMode::decode(&cd).unwrap();
            assert!(rest.is_empty());
            assert_eq!(back, mode);
        }
    }

    #[test]
    fn sparse_input_round_trips() {
        let input = MultinoulliTradeSparseInput {
            candidate_updates: vec![
                CategoricalProbUpdateRaw {
                    outcome_index: 0,
                    prob: sq(10),
                },
                CategoricalProbUpdateRaw {
                    outcome_index: 3,
                    prob: sq(40),
                },
            ],
            min_outcome_index: 0,
            supplied_collateral: sq(7),
            candidate_hint: CategoricalL2HintRaw {
                l2_norm_hint: sq(30),
            },
        };
        let cd = input.to_calldata();
        let (back, rest) = MultinoulliTradeSparseInput::decode(&cd).unwrap();
        assert!(rest.is_empty());
        assert_eq!(back, input);
    }
}
