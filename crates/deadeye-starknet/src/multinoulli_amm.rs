//! View + write clients for Deadeye's multinoulli (categorical) AMM.

use deadeye_core::{
    CategoricalDistribution,
    categorical::{CategoricalDistributionRaw, CategoricalL2HintRaw},
    sq128::Sq128Raw,
};
use starknet_core::types::{Felt, FunctionCall};
use tracing::instrument;

use crate::{
    account::Account,
    cairo_serde::CairoSerde,
    error::{ContractError, ContractResult, TradeError, TradeRejectionReason, TradeResult},
    execution::{Call, ExecutionReceipt},
    provider::Provider,
    runtime::{check_multinoulli_trade, compute_multinoulli_hint},
    selectors::{amm, multinoulli},
    types::{
        common::{AmmConfigRaw, AmmParamsRaw, FeeConfigRaw, LpInfoRaw},
        multinoulli::{
            CategoricalProbTransferRaw, CategoricalProbUpdateRaw, MultinoulliMarketStatusRaw,
            MultinoulliMatrixConstraintsRaw, MultinoulliPositionCompactRaw,
            MultinoulliPositionSummaryRaw, MultinoulliSellExecutionGuardsRaw,
            MultinoulliSellPositionSparseInput, MultinoulliTradeInput, MultinoulliTradeRejection,
            MultinoulliTradeSparseInput, MultinoulliTradeTransfersInput,
        },
    },
};

/// Typed read accessors for a deployed multinoulli AMM.
#[derive(Debug)]
pub struct MultinoulliMarketReader<P>
where
    P: Provider,
{
    provider: P,
    address: Felt,
}

impl<P> MultinoulliMarketReader<P>
where
    P: Provider,
{
    /// Bind a reader to a market address.
    pub const fn new(provider: P, address: Felt) -> Self {
        Self { provider, address }
    }

    /// Contract address.
    pub const fn address(&self) -> Felt {
        self.address
    }

    /// Borrow the underlying provider.
    pub const fn provider(&self) -> &P {
        &self.provider
    }

    /// Reads the current market distribution.
    #[instrument(skip(self), fields(market = %self.address))]
    pub async fn distribution(&self) -> ContractResult<CategoricalDistribution> {
        let raw = self
            .call_view::<CategoricalDistributionRaw>(
                "get_distribution",
                amm::get_distribution(),
                &[],
            )
            .await?;
        CategoricalDistribution::from_raw(&raw).map_err(ContractError::Core)
    }

    /// Reads market status.
    #[instrument(skip(self), fields(market = %self.address))]
    pub async fn market_status(&self) -> ContractResult<MultinoulliMarketStatusRaw> {
        self.call_view::<MultinoulliMarketStatusRaw>(
            "get_market_status",
            amm::get_market_status(),
            &[],
        )
        .await
    }

    /// Reads matrix constraints (or `Disabled` if none).
    #[instrument(skip(self), fields(market = %self.address))]
    pub async fn matrix_constraints(&self) -> ContractResult<MultinoulliMatrixConstraintsRaw> {
        self.call_view::<MultinoulliMatrixConstraintsRaw>(
            "get_matrix_constraints",
            multinoulli::get_matrix_constraints(),
            &[],
        )
        .await
    }

    /// Reads the AMM parameters.
    #[instrument(skip(self), fields(market = %self.address))]
    pub async fn params(&self) -> ContractResult<AmmParamsRaw> {
        self.call_view::<AmmParamsRaw>("get_params", amm::get_params(), &[])
            .await
    }

    /// Reads the full AMM configuration.
    #[instrument(skip(self), fields(market = %self.address))]
    pub async fn config(&self) -> ContractResult<AmmConfigRaw> {
        self.call_view::<AmmConfigRaw>("get_config", amm::get_config(), &[])
            .await
    }

    /// Reads the fee configuration.
    #[instrument(skip(self), fields(market = %self.address))]
    pub async fn fee_config(&self) -> ContractResult<FeeConfigRaw> {
        self.call_view::<FeeConfigRaw>("get_fee_config", amm::get_fee_config(), &[])
            .await
    }

    /// Reads LP pool info.
    #[instrument(skip(self), fields(market = %self.address))]
    pub async fn lp_info(&self) -> ContractResult<LpInfoRaw> {
        self.call_view::<LpInfoRaw>("get_lp_info", amm::get_lp_info(), &[])
            .await
    }

    /// Reads a trader's position summary.
    #[instrument(skip(self), fields(market = %self.address, %trader))]
    pub async fn position_summary(
        &self,
        trader: Felt,
    ) -> ContractResult<MultinoulliPositionSummaryRaw> {
        self.call_view::<MultinoulliPositionSummaryRaw>(
            "get_position_summary",
            amm::get_position_summary(),
            &[trader],
        )
        .await
    }

    /// Reads a trader's compact position.
    #[instrument(skip(self), fields(market = %self.address, %trader))]
    pub async fn position(&self, trader: Felt) -> ContractResult<MultinoulliPositionCompactRaw> {
        self.call_view::<MultinoulliPositionCompactRaw>(
            "get_position_compact",
            amm::get_position_compact(),
            &[trader],
        )
        .await
    }

    /// Reads the current distribution snapshot count (used by guarded sells).
    #[instrument(skip(self), fields(market = %self.address))]
    pub async fn distribution_snapshot_count(&self) -> ContractResult<u64> {
        self.call_view::<u64>(
            "get_distribution_snapshot_count",
            multinoulli::get_distribution_snapshot_count(),
            &[],
        )
        .await
    }

    /// Preflight a dense multinoulli trade.
    ///
    /// Mirrors the normal/lognormal contract — the reader fetches the
    /// current distribution, builds chain-correct L2 hints for both
    /// `current` and `candidate`, and then calls `check_trade_view` so
    /// the returned [`MultinoulliTradeQuote`]'s `on_chain_will_accept`
    /// flag is the authoritative chain verdict (not just "did hint
    /// computation succeed").
    ///
    /// Multinoulli markets accept three trade shapes (dense / sparse /
    /// transfers). The dense variant is the only one that needs the
    /// full distribution on chain; sparse + transfers ride the chain's
    /// `apply_*` routines and just need the candidate hint, which this
    /// method also returns when fed an already-derived candidate.
    #[instrument(skip(self, candidate), fields(market = %self.address, runtime = %runtime))]
    pub async fn quote_trade(
        &self,
        runtime: Felt,
        candidate: CategoricalDistributionRaw,
        min_outcome_index: u32,
        supplied_collateral: Sq128Raw,
    ) -> TradeResult<MultinoulliTradeQuote> {
        let current_dist = self
            .distribution()
            .await?
            .to_raw()
            .map_err(ContractError::Core)?;
        let params = self.params().await?;
        let current_hint = compute_multinoulli_hint(&self.provider, runtime, &current_dist).await?;
        let candidate_hint = compute_multinoulli_hint(&self.provider, runtime, &candidate).await?;
        let check = check_multinoulli_trade(
            &self.provider,
            runtime,
            current_dist,
            candidate.clone(),
            min_outcome_index,
            supplied_collateral,
            params.k,
            params.backing,
            params.tolerance,
            params.min_trade_collateral,
            current_hint,
            candidate_hint,
        )
        .await?;
        let rejection = map_multinoulli_rejection(check.rejection_reason, &check);
        Ok(MultinoulliTradeQuote {
            candidate,
            candidate_hint,
            min_outcome_index,
            supplied_collateral,
            on_chain_will_accept: check.is_valid,
            rejection,
        })
    }

    /// Closed-form payout at outcome `i` for a multinoulli candidate.
    ///
    /// Returns `λ · p_i` (the multinoulli analogue of
    /// `λ · pdf(x*)` for continuous markets).
    #[inline]
    #[must_use]
    pub fn payout_at(
        &self,
        outcome: usize,
        candidate: &deadeye_core::CategoricalDistribution,
        k: f64,
    ) -> f64 {
        crate::pricing::payout_at_multinoulli(candidate, k, outcome)
    }

    /// Estimate collateral required to tilt outcome `tilted_outcome` up
    /// by `delta_prob` (compensating mass spread evenly across the
    /// other outcomes).
    pub async fn impact_for_outcome_tilt(
        &self,
        tilted_outcome: usize,
        delta_prob: f64,
    ) -> ContractResult<crate::pricing::MultinoulliImpactEstimate> {
        let current = self.distribution().await?;
        let params = self.params().await?;
        let k = deadeye_core::Sq128::from_raw(params.k).to_f64();
        let n = current.outcome_count();
        if tilted_outcome >= n || n < 2 {
            return Err(ContractError::InvalidResponse {
                call: "impact_for_outcome_tilt",
                message: format!("tilted_outcome {tilted_outcome} out of range for n={n}",),
            });
        }
        let share = delta_prob / (n as f64 - 1.0_f64);
        let mut new_probs = current.probs().to_vec();
        for (i, p) in new_probs.iter_mut().enumerate() {
            if i == tilted_outcome {
                *p += delta_prob;
            } else {
                *p -= share;
            }
            if *p < 0.0 {
                *p = 0.0;
            }
        }
        let sum: f64 = new_probs.iter().sum();
        if sum <= 0.0 {
            return Err(ContractError::InvalidResponse {
                call: "impact_for_outcome_tilt",
                message: "candidate probabilities sum to zero".into(),
            });
        }
        for p in &mut new_probs {
            *p /= sum;
        }
        let candidate = deadeye_core::CategoricalDistribution::from_probs(new_probs)
            .map_err(ContractError::Core)?;
        let solver =
            deadeye_collateral::categorical_collateral(&current, &candidate, k).map_err(|e| {
                ContractError::InvalidResponse {
                    call: "impact_for_outcome_tilt",
                    message: format!("categorical solver: {e}"),
                }
            })?;
        Ok(crate::pricing::MultinoulliImpactEstimate {
            tilted_outcome,
            delta_prob,
            min_outcome_index: solver.min_outcome_index,
            required_collateral: solver.collateral,
        })
    }

    /// Numerical greeks for `candidate` at evaluation outcome
    /// `eval_outcome`.
    ///
    /// Returns one `∂payout/∂p_i` per outcome.
    #[inline]
    #[must_use]
    pub fn sensitivities_at(
        &self,
        candidate: &deadeye_core::CategoricalDistribution,
        eval_outcome: usize,
        k: f64,
    ) -> crate::pricing::MultinoulliSensitivities {
        crate::pricing::sensitivities_multinoulli(candidate, k, eval_outcome)
    }

    async fn call_view<T>(
        &self,
        call_name: &'static str,
        selector: Felt,
        calldata: &[Felt],
    ) -> ContractResult<T>
    where
        T: CairoSerde,
    {
        let response = self
            .provider
            .call(
                FunctionCall {
                    contract_address: self.address,
                    entry_point_selector: selector,
                    calldata: calldata.to_vec(),
                },
                self.provider.default_block(),
            )
            .await?;
        let (value, rest) = T::decode(&response)?;
        if !rest.is_empty() {
            return Err(ContractError::UnexpectedReturnSize {
                call: call_name,
                actual: response.len(),
                expected: response.len() - rest.len(),
            });
        }
        Ok(value)
    }
}

/// Preflighted multinoulli trade (dense candidate variant).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MultinoulliTradeQuote {
    /// Candidate distribution.
    pub candidate: CategoricalDistributionRaw,
    /// Chain-correct L2-norm hint for `candidate`.
    pub candidate_hint: CategoricalL2HintRaw,
    /// Outcome index minimizing `λ_g·g_i − λ_f·f_i`.
    pub min_outcome_index: u32,
    /// Collateral the writer should supply.
    pub supplied_collateral: Sq128Raw,
    /// `true` iff the chain's `check_trade_view` accepted the trade.
    pub on_chain_will_accept: bool,
    /// Typed rejection reason if `!on_chain_will_accept`.
    pub rejection: Option<TradeRejectionReason>,
}

fn map_multinoulli_rejection(
    raw: MultinoulliTradeRejection,
    check: &crate::types::multinoulli::MultinoulliTradeCheckRaw,
) -> Option<TradeRejectionReason> {
    use crate::error::VerificationSubReason as Sub;
    match raw {
        MultinoulliTradeRejection::None => None,
        MultinoulliTradeRejection::InvalidDistribution => {
            Some(TradeRejectionReason::InvalidDistribution)
        },
        MultinoulliTradeRejection::InvalidHints => Some(TradeRejectionReason::InvalidHints),
        MultinoulliTradeRejection::InvalidMinOutcome => {
            Some(TradeRejectionReason::InvalidMinOutcome)
        },
        MultinoulliTradeRejection::BackingFail => Some(TradeRejectionReason::BackingFail),
        MultinoulliTradeRejection::LowCollateral => Some(TradeRejectionReason::LowCollateral),
        MultinoulliTradeRejection::VerificationFailed => {
            // Drill into the embedded multinoulli verification flags to
            // recover the refined sub-reason without round-tripping a
            // revert string.
            let sub = if !check.verification.minimum_valid {
                Some(Sub::MinimumInvalid)
            } else if !check.verification.collateral_sufficient {
                Some(Sub::CollateralInsufficient)
            } else {
                None
            };
            Some(TradeRejectionReason::VerificationFailed { sub_reason: sub })
        },
    }
}

// ─── Writer ──────────────────────────────────────────────────────────────────

/// Write-capable companion to [`MultinoulliMarketReader`].
#[derive(Debug)]
pub struct MultinoulliMarketWriter<P, A>
where
    P: Provider,
    A: Account,
{
    reader: MultinoulliMarketReader<P>,
    account: A,
}

impl<P, A> MultinoulliMarketWriter<P, A>
where
    P: Provider,
    A: Account,
{
    /// Pair a reader with an account.
    pub const fn new(reader: MultinoulliMarketReader<P>, account: A) -> Self {
        Self { reader, account }
    }

    /// Borrow the underlying reader.
    pub const fn reader(&self) -> &MultinoulliMarketReader<P> {
        &self.reader
    }

    /// Borrow the underlying account.
    pub const fn account(&self) -> &A {
        &self.account
    }

    /// Build a Call for `execute_trade` (dense candidate).
    pub fn build_trade_call(&self, input: &MultinoulliTradeInput) -> Call {
        Call {
            to: self.reader.address(),
            selector: amm::execute_trade(),
            calldata: input.to_calldata(),
        }
    }

    /// Build a Call for `execute_trade_sparse`.
    pub fn build_trade_sparse_call(&self, input: &MultinoulliTradeSparseInput) -> Call {
        Call {
            to: self.reader.address(),
            selector: multinoulli::execute_trade_sparse(),
            calldata: input.to_calldata(),
        }
    }

    /// Build a Call for `execute_trade_transfers`.
    pub fn build_trade_transfers_call(&self, input: &MultinoulliTradeTransfersInput) -> Call {
        Call {
            to: self.reader.address(),
            selector: multinoulli::execute_trade_transfers(),
            calldata: input.to_calldata(),
        }
    }

    /// Build a Call for `sell_position_guarded`.
    pub fn build_sell_call(&self, guards: &MultinoulliSellExecutionGuardsRaw) -> Call {
        Call {
            to: self.reader.address(),
            selector: amm::sell_position_guarded(),
            calldata: guards.to_calldata(),
        }
    }

    /// Build a Call for `sell_position_guarded_sparse`.
    pub fn build_sell_sparse_call(
        &self,
        guards: &MultinoulliSellExecutionGuardsRaw,
        input: &MultinoulliSellPositionSparseInput,
    ) -> Call {
        let mut calldata = guards.to_calldata();
        input.encode(&mut calldata);
        Call {
            to: self.reader.address(),
            selector: multinoulli::sell_position_guarded_sparse(),
            calldata,
        }
    }

    /// Build a Call for `claim`.
    pub fn build_claim_call(&self) -> Call {
        Call {
            to: self.reader.address(),
            selector: amm::claim(),
            calldata: Vec::new(),
        }
    }

    /// Build a Call for `claim_for(trader)`.
    pub fn build_claim_for_call(&self, trader: Felt) -> Call {
        Call {
            to: self.reader.address(),
            selector: amm::claim_for(),
            calldata: vec![trader],
        }
    }

    /// Execute a dense trade.
    #[instrument(skip(self, input), fields(market = %self.reader.address(), family = "multinoulli", kind = "trade"))]
    pub async fn execute_trade(
        &self,
        input: &MultinoulliTradeInput,
    ) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_trade_call(input)])
            .await
    }

    /// Execute a sparse trade.
    #[instrument(skip(self, input), fields(market = %self.reader.address(), family = "multinoulli", kind = "trade"))]
    pub async fn execute_trade_sparse(
        &self,
        input: &MultinoulliTradeSparseInput,
    ) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_trade_sparse_call(input)])
            .await
    }

    /// Execute a mass-conserving transfer trade.
    #[instrument(skip(self, input), fields(market = %self.reader.address(), family = "multinoulli", kind = "trade"))]
    pub async fn execute_trade_transfers(
        &self,
        input: &MultinoulliTradeTransfersInput,
    ) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_trade_transfers_call(input)])
            .await
    }

    /// Submit a guarded sell.
    #[instrument(skip(self, guards), fields(market = %self.reader.address(), family = "multinoulli", kind = "sell"))]
    pub async fn sell_position_guarded(
        &self,
        guards: &MultinoulliSellExecutionGuardsRaw,
    ) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_sell_call(guards)])
            .await
    }

    /// Submit a sparse guarded sell.
    #[instrument(skip(self, guards, input), fields(market = %self.reader.address(), family = "multinoulli", kind = "sell"))]
    pub async fn sell_position_guarded_sparse(
        &self,
        guards: &MultinoulliSellExecutionGuardsRaw,
        input: &MultinoulliSellPositionSparseInput,
    ) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_sell_sparse_call(guards, input)])
            .await
    }

    /// Claim the caller's settled position.
    #[instrument(skip(self), fields(market = %self.reader.address(), family = "multinoulli", kind = "claim"))]
    pub async fn claim(&self) -> ContractResult<ExecutionReceipt> {
        self.account.execute(vec![self.build_claim_call()]).await
    }

    /// Claim a settled position on behalf of `trader`.
    #[instrument(skip(self), fields(market = %self.reader.address(), family = "multinoulli", kind = "claim_for", %trader))]
    pub async fn claim_for(&self, trader: Felt) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_claim_for_call(trader)])
            .await
    }

    /// Submit a dense quote prepared by
    /// [`MultinoulliMarketReader::quote_trade`].
    #[instrument(skip(self, quote), fields(market = %self.reader.address(), family = "multinoulli", kind = "trade", accepts = quote.on_chain_will_accept))]
    pub async fn execute_quote(
        &self,
        quote: MultinoulliTradeQuote,
    ) -> TradeResult<ExecutionReceipt> {
        if !quote.on_chain_will_accept {
            return Err(TradeError::Rejected {
                reason: quote.rejection.unwrap_or(TradeRejectionReason::Other {
                    raw: "quote not acceptable",
                }),
                source: ContractError::InvalidResponse {
                    call: "execute_quote",
                    message: "quote.on_chain_will_accept = false".into(),
                },
            });
        }
        let input = MultinoulliTradeInput {
            candidate: quote.candidate,
            min_outcome_index: quote.min_outcome_index,
            supplied_collateral: quote.supplied_collateral,
            candidate_hint: quote.candidate_hint,
        };
        self.account
            .execute(vec![self.build_trade_call(&input)])
            .await
            .map_err(TradeError::from_contract)
    }

    /// Submit a sparse multinoulli trade with chain-correct hint fetched
    /// internally.
    #[instrument(skip(self, candidate, candidate_updates, supplied_collateral), fields(market = %self.reader.address(), family = "multinoulli", kind = "trade", runtime = %runtime))]
    pub async fn execute_sparse_with_runtime(
        &self,
        runtime: Felt,
        candidate: CategoricalDistributionRaw,
        candidate_updates: Vec<CategoricalProbUpdateRaw>,
        min_outcome_index: u32,
        supplied_collateral: Sq128Raw,
    ) -> TradeResult<ExecutionReceipt> {
        let candidate_hint =
            compute_multinoulli_hint(self.reader.provider(), runtime, &candidate).await?;
        let input = MultinoulliTradeSparseInput {
            candidate_updates,
            min_outcome_index,
            supplied_collateral,
            candidate_hint,
        };
        self.account
            .execute(vec![self.build_trade_sparse_call(&input)])
            .await
            .map_err(TradeError::from_contract)
    }

    /// Submit a transfers multinoulli trade. `candidate_after_apply` is
    /// the chain-correct distribution after replaying `transfers` (use
    /// `apply_transfers_to_distribution`-equivalent logic off-chain — see
    /// the multinoulli chaos test for the canonical Sq128-exact replay).
    #[instrument(skip(self, candidate_after_apply, transfers, supplied_collateral), fields(market = %self.reader.address(), family = "multinoulli", kind = "trade", runtime = %runtime))]
    pub async fn execute_transfers_with_runtime(
        &self,
        runtime: Felt,
        candidate_after_apply: &CategoricalDistributionRaw,
        transfers: Vec<CategoricalProbTransferRaw>,
        min_outcome_index: u32,
        supplied_collateral: Sq128Raw,
    ) -> TradeResult<ExecutionReceipt> {
        let candidate_hint =
            compute_multinoulli_hint(self.reader.provider(), runtime, candidate_after_apply)
                .await?;
        let input = MultinoulliTradeTransfersInput {
            transfers,
            min_outcome_index,
            supplied_collateral,
            candidate_hint,
        };
        self.account
            .execute(vec![self.build_trade_transfers_call(&input)])
            .await
            .map_err(TradeError::from_contract)
    }

    /// Close out the caller's multinoulli position using live snapshot
    /// id + LP backing for the guards.
    #[instrument(skip(self), fields(market = %self.reader.address(), family = "multinoulli", kind = "sell"))]
    pub async fn sell_position(&self, min_token_out: u128) -> TradeResult<ExecutionReceipt> {
        let snapshot_id = self.reader.distribution_snapshot_count().await?;
        let params = self.reader.params().await?;
        let lp_info = self.reader.lp_info().await?;
        let guards = MultinoulliSellExecutionGuardsRaw {
            expected_distribution_snapshot_id: snapshot_id,
            expected_backing: lp_info.total_backing_deposited,
            expected_tolerance: params.tolerance,
            expected_min_trade_collateral: params.min_trade_collateral,
            min_token_out,
        };
        let call = self.build_sell_call(&guards);
        self.account
            .execute(vec![call])
            .await
            .map_err(TradeError::from_contract)
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
#[expect(
    clippy::float_cmp,
    reason = "tests assert exact 0.0 sentinel intentionally"
)]
mod tests {
    use std::{sync::Mutex, vec, vec::Vec};

    use async_trait::async_trait;
    use deadeye_core::CategoricalDistribution;
    use starknet_core::types::{BlockId, FunctionCall};

    use super::*;

    struct CannedProvider {
        responses: Mutex<Vec<Vec<Felt>>>,
    }

    #[async_trait]
    impl Provider for CannedProvider {
        async fn call(&self, _call: FunctionCall, _block: BlockId) -> ContractResult<Vec<Felt>> {
            self.responses
                .lock()
                .expect("mutex")
                .pop()
                .ok_or_else(|| ContractError::Provider("no canned response".into()))
        }
    }

    #[test]
    fn payout_at_picks_correct_outcome() {
        let provider = CannedProvider {
            responses: Mutex::new(Vec::new()),
        };
        let reader = MultinoulliMarketReader::new(provider, Felt::from(0x1234_u64));
        let dist = CategoricalDistribution::from_probs(vec![0.1, 0.7, 0.2]).unwrap();
        let p0 = reader.payout_at(0, &dist, 1.0);
        let p1 = reader.payout_at(1, &dist, 1.0);
        let p2 = reader.payout_at(2, &dist, 1.0);
        assert!(p1 > p0);
        assert!(p1 > p2);
        // out-of-range returns 0
        assert_eq!(reader.payout_at(99, &dist, 1.0), 0.0);
    }

    #[test]
    fn sensitivities_at_returns_one_per_outcome() {
        let provider = CannedProvider {
            responses: Mutex::new(Vec::new()),
        };
        let reader = MultinoulliMarketReader::new(provider, Felt::from(0x1234_u64));
        let dist = CategoricalDistribution::uniform(4).unwrap();
        let s = reader.sensitivities_at(&dist, 0, 1.0);
        assert_eq!(s.d_payout_d_prob.len(), 4);
        for v in &s.d_payout_d_prob {
            assert!(v.is_finite(), "got {v}");
        }
    }
}
