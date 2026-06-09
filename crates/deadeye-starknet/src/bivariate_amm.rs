//! View + write clients for Deadeye's bivariate-normal AMM.

use deadeye_core::{
    BivariateNormalDistribution,
    bivariate::{
        BivariateNormalDistributionCoreRaw, BivariateNormalDistributionRaw,
        BivariateNormalSqrtHintsRaw, BivariatePointRaw,
    },
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
    runtime::{check_bivariate_trade, compute_bivariate_hints, expand_bivariate_distribution},
    selectors::amm,
    types::{
        bivariate::{
            BivariateMarketStatusRaw, BivariateNormalPositionCompactRaw,
            BivariateNormalSellExecutionGuardsRaw, BivariateTradeInput,
        },
        common::{AmmConfigRaw, AmmParamsRaw, FeeConfigRaw, LpInfoRaw, TradeRejection},
        normal::{PositionSummaryRaw, TradeCheckRaw},
    },
};

/// Typed reader for a bivariate AMM contract.
#[derive(Debug)]
pub struct BivariateMarketReader<P>
where
    P: Provider,
{
    provider: P,
    address: Felt,
}

impl<P> BivariateMarketReader<P>
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

    /// Borrow the provider.
    pub const fn provider(&self) -> &P {
        &self.provider
    }

    /// Reads the current bivariate distribution.
    #[instrument(skip(self), fields(market = %self.address))]
    pub async fn distribution(&self) -> ContractResult<BivariateNormalDistribution> {
        let raw = self.distribution_raw().await?;
        BivariateNormalDistribution::from_raw(raw).map_err(ContractError::Core)
    }

    /// Reads the raw on-chain distribution (no f64 round-trip). Useful
    /// for paths that need the chain's exact Sq128 limbs — e.g.
    /// constructing sell guards.
    #[instrument(skip(self), fields(market = %self.address))]
    pub async fn distribution_raw(&self) -> ContractResult<BivariateNormalDistributionRaw> {
        self.call_view::<BivariateNormalDistributionRaw>(
            "get_distribution",
            amm::get_distribution(),
            &[],
        )
        .await
    }

    /// Reads the bivariate market status.
    pub async fn market_status(&self) -> ContractResult<BivariateMarketStatusRaw> {
        self.call_view::<BivariateMarketStatusRaw>(
            "get_market_status",
            amm::get_market_status(),
            &[],
        )
        .await
    }

    /// Reads market parameters.
    pub async fn params(&self) -> ContractResult<AmmParamsRaw> {
        self.call_view::<AmmParamsRaw>("get_params", amm::get_params(), &[])
            .await
    }

    /// Reads the AMM configuration.
    pub async fn config(&self) -> ContractResult<AmmConfigRaw> {
        self.call_view::<AmmConfigRaw>("get_config", amm::get_config(), &[])
            .await
    }

    /// Reads fee configuration.
    pub async fn fee_config(&self) -> ContractResult<FeeConfigRaw> {
        self.call_view::<FeeConfigRaw>("get_fee_config", amm::get_fee_config(), &[])
            .await
    }

    /// Reads LP info.
    pub async fn lp_info(&self) -> ContractResult<LpInfoRaw> {
        self.call_view::<LpInfoRaw>("get_lp_info", amm::get_lp_info(), &[])
            .await
    }

    /// Reads a position summary.
    pub async fn position_summary(&self, trader: Felt) -> ContractResult<PositionSummaryRaw> {
        self.call_view::<PositionSummaryRaw>(
            "get_position_summary",
            amm::get_position_summary(),
            &[trader],
        )
        .await
    }

    /// Reads a compact position record.
    pub async fn position(
        &self,
        trader: Felt,
    ) -> ContractResult<BivariateNormalPositionCompactRaw> {
        self.call_view::<BivariateNormalPositionCompactRaw>(
            "get_position_compact",
            amm::get_position_compact(),
            &[trader],
        )
        .await
    }

    // ── Trade-lot (multi-leg) read surface ──────────────────────────────
    //
    // Each market-moving trade stores an explicit *trade lot* (leg). A
    // trader accumulates one lot per admitted delta; legs are valued and
    // settled independently. These low-level views mirror the on-chain lot
    // entrypoints; the SDK aggregates them into a `MultiLegPosition`. The
    // count/id/settled/cancelled selectors are family-agnostic and shared
    // with the normal AMM; only `trade_lot_value_at` differs, taking a 2-D
    // settlement point instead of a scalar.

    /// Number of trade lots (legs) this trader holds in the market.
    #[instrument(skip(self), fields(market = %self.address, %trader))]
    pub async fn trade_lot_count(&self, trader: Felt) -> ContractResult<u64> {
        self.call_view::<u64>(
            "get_trader_trade_lot_count",
            amm::get_trader_trade_lot_count(),
            &[trader],
        )
        .await
    }

    /// The `lot_id` at `index` in this trader's lot list (`index < count`).
    #[instrument(skip(self), fields(market = %self.address, %trader, index))]
    pub async fn trade_lot_id(&self, trader: Felt, index: u64) -> ContractResult<u64> {
        self.call_view::<u64>(
            "get_trader_trade_lot_id",
            amm::get_trader_trade_lot_id(),
            &[trader, Felt::from(index)],
        )
        .await
    }

    /// Whether a lot has already been settled (paid out).
    #[instrument(skip(self), fields(market = %self.address, lot_id))]
    pub async fn trade_lot_settled(&self, lot_id: u64) -> ContractResult<bool> {
        self.call_view::<bool>("get_trade_lot_settled", amm::get_trade_lot_settled(), &[
            Felt::from(lot_id),
        ])
        .await
    }

    /// Whether a lot was cancelled (terminalised without payout — collateral
    /// forfeit to the LP because it could not be valued at settlement).
    #[instrument(skip(self), fields(market = %self.address, lot_id))]
    pub async fn trade_lot_cancelled(&self, lot_id: u64) -> ContractResult<bool> {
        self.call_view::<bool>(
            "get_trade_lot_cancelled",
            amm::get_trade_lot_cancelled(),
            &[Felt::from(lot_id)],
        )
        .await
    }

    /// The lot's **signed** scoring-rule value at a settlement point `(x1, x2)`
    /// (`to_λ·pdf(x*; to) − from_λ·pdf(x*; from)`), read authoritatively from
    /// the chain. This is the per-leg position value; gross payout for the leg
    /// is `collateral_locked + value_at(x*)`. Unlike the normal AMM the
    /// settlement is a 2-D [`BivariatePointRaw`], encoded after the `lot_id`.
    #[instrument(skip(self, settlement), fields(market = %self.address, lot_id))]
    pub async fn trade_lot_value_at(
        &self,
        lot_id: u64,
        settlement: BivariatePointRaw,
    ) -> ContractResult<Sq128Raw> {
        let mut calldata = vec![Felt::from(lot_id)];
        settlement.encode(&mut calldata);
        self.call_view::<Sq128Raw>(
            "get_trade_lot_value_at",
            amm::get_trade_lot_value_at(),
            &calldata,
        )
        .await
    }

    /// Enumerate **all** of a trader's `lot_id`s (count → id-by-index). One
    /// RPC for the count plus one per lot; the SDK wraps this with concurrent
    /// fan-out for bulk valuation.
    #[instrument(skip(self), fields(market = %self.address, %trader))]
    pub async fn trade_lot_ids(&self, trader: Felt) -> ContractResult<Vec<u64>> {
        let count = self.trade_lot_count(trader).await?;
        let mut ids = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
        for index in 0..count {
            ids.push(self.trade_lot_id(trader, index).await?);
        }
        Ok(ids)
    }

    /// Preflight a bivariate trade. `core_candidate` is the bivariate
    /// core (μ₁, μ₂, σ₁², σ₂², ρ); the reader expands it via
    /// `expand_distribution_core_view` so the resulting candidate carries
    /// chain-correct `sigma_i`, `inv_one_minus_rho_sq` and
    /// `normalization` — without this the constructor's hint check
    /// rejects f64-derived inputs.
    ///
    /// After the candidate is expanded, the reader fetches chain-correct
    /// hints for both the current market dist and the candidate and then
    /// calls `check_trade_view` — so the returned quote's
    /// `on_chain_will_accept` is the authoritative chain verdict (not
    /// just "did hint computation succeed").
    #[instrument(skip(self), fields(market = %self.address, runtime = %runtime))]
    pub async fn quote_trade(
        &self,
        runtime: Felt,
        core_candidate: BivariateNormalDistributionCoreRaw,
        x_star: BivariatePointRaw,
        supplied_collateral: Sq128Raw,
    ) -> TradeResult<BivariateTradeQuote> {
        let current = self.distribution_raw().await?;
        let params = self.params().await?;
        let candidate =
            expand_bivariate_distribution(&self.provider, runtime, core_candidate).await?;
        let current_hints = compute_bivariate_hints(&self.provider, runtime, current).await?;
        let candidate_hints = compute_bivariate_hints(&self.provider, runtime, candidate).await?;
        let check = check_bivariate_trade(
            &self.provider,
            runtime,
            current,
            candidate,
            x_star,
            supplied_collateral,
            params.k,
            params.backing,
            params.tolerance,
            params.min_trade_collateral,
            current_hints,
            candidate_hints,
        )
        .await?;
        let rejection = map_bivariate_rejection(check.rejection_reason, &check);
        Ok(BivariateTradeQuote {
            candidate,
            candidate_hints,
            x_star,
            supplied_collateral,
            on_chain_will_accept: check.is_valid,
            rejection,
        })
    }

    /// Closed-form payout at `(x1, x2)` for a bivariate candidate.
    #[inline]
    #[must_use]
    pub fn payout_at(
        &self,
        x1: f64,
        x2: f64,
        candidate: &deadeye_core::BivariateNormalDistribution,
        k: f64,
    ) -> f64 {
        crate::pricing::payout_at_bivariate(candidate, k, x1, x2)
    }

    /// Estimate collateral required to shift `(μ₁, μ₂)` by
    /// `(delta_mu1, delta_mu2)`, keeping σ₁, σ₂, ρ constant.
    pub async fn impact_for_mu_shift(
        &self,
        delta_mu1: f64,
        delta_mu2: f64,
    ) -> ContractResult<crate::pricing::ImpactEstimate> {
        let current = self.distribution().await?;
        let candidate = deadeye_core::BivariateNormalDistribution::from_core(
            current.mu1() + delta_mu1,
            current.mu2() + delta_mu2,
            current.sigma1().powi(2),
            current.sigma2().powi(2),
            current.rho(),
        )
        .map_err(ContractError::Core)?;
        let solver = deadeye_collateral::bivariate_collateral(
            &current,
            &candidate,
            deadeye_collateral::BivariateOptions::default(),
        )
        .map_err(|e| ContractError::InvalidResponse {
            call: "impact_for_mu_shift",
            message: format!("bivariate solver: {e}"),
        })?;
        Ok(crate::pricing::ImpactEstimate {
            delta_mu: delta_mu1,
            x_star: solver.x1,
            required_collateral: solver.collateral,
            iterations: solver.iterations,
        })
    }

    /// Numerical greeks for `candidate` at `(x1, x2)`.
    #[inline]
    #[must_use]
    pub fn sensitivities_at(
        &self,
        candidate: &deadeye_core::BivariateNormalDistribution,
        x1: f64,
        x2: f64,
        k: f64,
    ) -> crate::pricing::BivariateSensitivities {
        crate::pricing::sensitivities_bivariate(candidate, k, x1, x2)
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

/// Preflighted bivariate-AMM trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BivariateTradeQuote {
    /// Chain-expanded full distribution.
    pub candidate: BivariateNormalDistributionRaw,
    /// Chain-correct sqrt hints.
    pub candidate_hints: BivariateNormalSqrtHintsRaw,
    /// `x_star` (2-D point).
    pub x_star: BivariatePointRaw,
    /// Collateral the writer should supply.
    pub supplied_collateral: Sq128Raw,
    /// `true` iff the chain's `check_trade_view` accepted the trade.
    pub on_chain_will_accept: bool,
    /// Typed rejection reason if `!on_chain_will_accept`.
    pub rejection: Option<TradeRejectionReason>,
}

fn map_bivariate_rejection(
    raw: TradeRejection,
    check: &TradeCheckRaw,
) -> Option<TradeRejectionReason> {
    use crate::error::VerificationSubReason as Sub;
    match raw {
        TradeRejection::None => None,
        TradeRejection::InvalidDistribution => Some(TradeRejectionReason::InvalidDistribution),
        TradeRejection::InvalidHints => Some(TradeRejectionReason::InvalidHints),
        TradeRejection::BackingFail => Some(TradeRejectionReason::BackingFail),
        TradeRejection::SigmaTooLow => Some(TradeRejectionReason::SigmaTooLow),
        TradeRejection::LowCollateral => Some(TradeRejectionReason::LowCollateral),
        TradeRejection::VerificationFailed => {
            let sub = if !check.verification.side_valid {
                Some(Sub::SideInvalid)
            } else if !check.verification.stationary_valid {
                Some(Sub::StationaryInvalid)
            } else if !check.verification.curvature_valid {
                Some(Sub::CurvatureInvalid)
            } else if !check.verification.collateral_sufficient {
                Some(Sub::CollateralInsufficient)
            } else {
                None
            };
            Some(TradeRejectionReason::VerificationFailed { sub_reason: sub })
        },
    }
}

/// Write-capable companion.
#[derive(Debug)]
pub struct BivariateMarketWriter<P, A>
where
    P: Provider,
    A: Account,
{
    reader: BivariateMarketReader<P>,
    account: A,
}

impl<P, A> BivariateMarketWriter<P, A>
where
    P: Provider,
    A: Account,
{
    /// Pair a reader with an account.
    pub const fn new(reader: BivariateMarketReader<P>, account: A) -> Self {
        Self { reader, account }
    }

    /// Borrow the reader.
    pub const fn reader(&self) -> &BivariateMarketReader<P> {
        &self.reader
    }

    /// Borrow the account.
    pub const fn account(&self) -> &A {
        &self.account
    }

    /// Build a Call for `execute_trade`.
    pub fn build_trade_call(&self, input: BivariateTradeInput) -> Call {
        Call {
            to: self.reader.address(),
            selector: amm::execute_trade(),
            calldata: input.to_calldata(),
        }
    }

    /// Build a Call for `sell_position_guarded`.
    ///
    /// ABI signature (`bivariate_amm.abi.json`):
    /// `sell_position_guarded(candidate, x_star, candidate_hints, guards)`,
    /// where `x_star` is a [`BivariatePointRaw`]. All four are encoded in
    /// order.
    pub fn build_sell_call(
        &self,
        candidate: BivariateNormalDistributionRaw,
        x_star: BivariatePointRaw,
        candidate_hints: BivariateNormalSqrtHintsRaw,
        guards: BivariateNormalSellExecutionGuardsRaw,
    ) -> Call {
        let mut calldata = Vec::with_capacity(48);
        candidate.encode(&mut calldata);
        x_star.encode(&mut calldata);
        candidate_hints.encode(&mut calldata);
        guards.encode(&mut calldata);
        Call {
            to: self.reader.address(),
            selector: amm::sell_position_guarded(),
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

    /// Build the [`Call`] for `claim_for(trader)`.
    pub fn build_claim_for_call(&self, trader: Felt) -> Call {
        Call {
            to: self.reader.address(),
            selector: amm::claim_for(),
            calldata: vec![trader],
        }
    }

    /// Build the [`Call`] for `add_liquidity(share_amount)`.
    ///
    /// The bivariate ABI takes only `share_amount`. Nothing else is encoded.
    pub fn build_add_liquidity_call(&self, share_amount: deadeye_core::sq128::Sq128Raw) -> Call {
        Call {
            to: self.reader.address(),
            selector: amm::add_liquidity(),
            calldata: share_amount.to_calldata(),
        }
    }

    /// Build the [`Call`] for `remove_liquidity(share_amount)`.
    pub fn build_remove_liquidity_call(&self, share_amount: deadeye_core::sq128::Sq128Raw) -> Call {
        Call {
            to: self.reader.address(),
            selector: amm::remove_liquidity(),
            calldata: share_amount.to_calldata(),
        }
    }

    /// Build the [`Call`] for `settle(settlement_point)` — bivariate-only.
    ///
    /// Unlike the normal AMM (which settles via the factory's
    /// `settle_normal_markets_*` batch entrypoint), the bivariate ABI
    /// exposes `settle(settlement_point: BivariatePointRaw)` directly on
    /// the market. See `bivariate_amm.abi.json` → `settle`.
    pub fn build_settle_call(&self, settlement_point: BivariatePointRaw) -> Call {
        Call {
            to: self.reader.address(),
            selector: starknet_core::utils::get_selector_from_name("settle")
                .expect("entry-point name `settle` is a valid Cairo identifier"),
            calldata: settlement_point.to_calldata(),
        }
    }

    /// Execute a trade.
    #[instrument(skip(self, input), fields(market = %self.reader.address(), family = "bivariate", kind = "trade"))]
    pub async fn execute_trade(
        &self,
        input: BivariateTradeInput,
    ) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_trade_call(input)])
            .await
    }

    /// Submit a guarded sell. Mirrors the ABI:
    /// `sell_position_guarded(candidate, x_star, candidate_hints, guards)`.
    #[instrument(skip(self, candidate, x_star, candidate_hints, guards), fields(market = %self.reader.address(), family = "bivariate", kind = "sell"))]
    pub async fn sell_position_guarded(
        &self,
        candidate: BivariateNormalDistributionRaw,
        x_star: BivariatePointRaw,
        candidate_hints: BivariateNormalSqrtHintsRaw,
        guards: BivariateNormalSellExecutionGuardsRaw,
    ) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_sell_call(
                candidate,
                x_star,
                candidate_hints,
                guards,
            )])
            .await
    }

    /// Claim the caller's settled position.
    #[instrument(skip(self), fields(market = %self.reader.address(), family = "bivariate", kind = "claim"))]
    pub async fn claim(&self) -> ContractResult<ExecutionReceipt> {
        self.account.execute(vec![self.build_claim_call()]).await
    }

    /// Claim a settled position on behalf of `trader`.
    #[instrument(skip(self), fields(market = %self.reader.address(), family = "bivariate", kind = "claim_for", %trader))]
    pub async fn claim_for(&self, trader: Felt) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_claim_for_call(trader)])
            .await
    }

    /// Add liquidity to the pool. ABI takes a single `share_amount`.
    #[instrument(skip(self, share_amount), fields(market = %self.reader.address(), family = "bivariate", kind = "lp_add"))]
    pub async fn add_liquidity(
        &self,
        share_amount: deadeye_core::sq128::Sq128Raw,
    ) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_add_liquidity_call(share_amount)])
            .await
    }

    /// Remove a fraction of the caller's liquidity. ABI takes a single
    /// `share_amount`.
    #[instrument(skip(self, share_amount), fields(market = %self.reader.address(), family = "bivariate", kind = "lp_remove"))]
    pub async fn remove_liquidity(
        &self,
        share_amount: deadeye_core::sq128::Sq128Raw,
    ) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_remove_liquidity_call(share_amount)])
            .await
    }

    /// Settle the bivariate market at `settlement_point`. Admin-only on
    /// the contract side; will revert if the caller lacks permission.
    #[instrument(skip(self, settlement_point), fields(market = %self.reader.address(), family = "bivariate", kind = "settle"))]
    pub async fn settle(
        &self,
        settlement_point: BivariatePointRaw,
    ) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_settle_call(settlement_point)])
            .await
    }

    /// Submit a bivariate quote prepared by
    /// [`BivariateMarketReader::quote_trade`].
    #[instrument(skip(self, quote), fields(market = %self.reader.address(), family = "bivariate", kind = "trade", accepts = quote.on_chain_will_accept))]
    pub async fn execute_quote(&self, quote: BivariateTradeQuote) -> TradeResult<ExecutionReceipt> {
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
        let input = BivariateTradeInput {
            candidate: quote.candidate,
            x_star: quote.x_star,
            supplied_collateral: quote.supplied_collateral,
            candidate_hints: quote.candidate_hints,
        };
        self.account
            .execute(vec![self.build_trade_call(input)])
            .await
            .map_err(TradeError::from_contract)
    }

    /// Close out the caller's bivariate position. Reads live market
    /// distribution, params, and LP backing for the guards.
    #[instrument(skip(self), fields(market = %self.reader.address(), family = "bivariate", kind = "sell", runtime = %runtime))]
    pub async fn sell_position(
        &self,
        runtime: Felt,
        min_token_out: u128,
    ) -> TradeResult<ExecutionReceipt> {
        let market_dist = self.reader.distribution_raw().await?;
        let params = self.reader.params().await?;
        let lp_info = self.reader.lp_info().await?;
        let candidate_hints =
            compute_bivariate_hints(self.reader.provider(), runtime, market_dist).await?;
        let core = BivariateNormalDistributionCoreRaw {
            mu1: market_dist.mu1,
            mu2: market_dist.mu2,
            variance1: market_dist.variance1,
            variance2: market_dist.variance2,
            rho: market_dist.rho,
        };
        let guards = BivariateNormalSellExecutionGuardsRaw {
            expected_market_dist: core,
            expected_backing: lp_info.total_backing_deposited,
            expected_tolerance: params.tolerance,
            expected_min_trade_collateral: params.min_trade_collateral,
            min_token_out,
        };
        let x_star = BivariatePointRaw {
            x1: market_dist.mu1,
            x2: market_dist.mu2,
        };
        let call = self.build_sell_call(market_dist, x_star, candidate_hints, guards);
        self.account
            .execute(vec![call])
            .await
            .map_err(TradeError::from_contract)
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use std::{sync::Mutex, vec::Vec};

    use async_trait::async_trait;
    use deadeye_core::BivariateNormalDistribution;
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
    fn payout_at_peaks_at_mean() {
        let provider = CannedProvider {
            responses: Mutex::new(Vec::new()),
        };
        let reader = BivariateMarketReader::new(provider, Felt::from(0x1234_u64));
        let dist = BivariateNormalDistribution::from_core(0.0, 0.0, 1.0, 1.0, 0.0).unwrap();
        let at_mean = reader.payout_at(0.0, 0.0, &dist, 1.0);
        let off = reader.payout_at(1.0, 1.0, &dist, 1.0);
        assert!(at_mean > off, "{at_mean} vs {off}");
    }

    #[test]
    fn sensitivities_at_finite_all_components() {
        let provider = CannedProvider {
            responses: Mutex::new(Vec::new()),
        };
        let reader = BivariateMarketReader::new(provider, Felt::from(0x1234_u64));
        let dist = BivariateNormalDistribution::from_core(0.0, 0.0, 1.0, 1.0, 0.0).unwrap();
        let s = reader.sensitivities_at(&dist, 0.5, 0.5, 1.0);
        assert!(s.d_payout_d_mu1.is_finite());
        assert!(s.d_payout_d_mu2.is_finite());
        assert!(s.d_payout_d_sigma1.is_finite());
        assert!(s.d_payout_d_sigma2.is_finite());
        assert!(s.d_payout_d_rho.is_finite());
    }
}
