//! View + write clients for Deadeye's normal (Gaussian) AMM contract.

use deadeye_core::{
    Distribution,
    distribution::{NormalDistribution, NormalDistributionRaw, NormalSqrtHintsRaw},
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
    runtime::compute_normal_hints,
    selectors::amm,
    types::{
        common::{AmmConfigRaw, AmmParamsRaw, FeeConfigRaw, LpInfoRaw, TradeRejection},
        normal::{
            PositionCompactRaw, PositionSummaryRaw, SellExecutionGuardsRaw, TradeCheckRaw,
            TradeInput,
        },
    },
};

/// Typed read accessors for a deployed normal AMM contract.
#[derive(Debug)]
pub struct NormalMarketReader<P>
where
    P: Provider,
{
    provider: P,
    address: Felt,
}

impl<P> NormalMarketReader<P>
where
    P: Provider,
{
    /// Construct a reader bound to a specific market address.
    pub const fn new(provider: P, address: Felt) -> Self {
        Self { provider, address }
    }

    /// Contract address this reader targets.
    pub const fn address(&self) -> Felt {
        self.address
    }

    /// Borrow the underlying [`Provider`].
    pub const fn provider(&self) -> &P {
        &self.provider
    }

    /// Reads the current market distribution.
    #[instrument(skip(self), fields(market = %self.address))]
    pub async fn distribution(&self) -> ContractResult<NormalDistribution> {
        let raw = self
            .call_view::<NormalDistributionRaw>("get_distribution", amm::get_distribution(), &[])
            .await?;
        NormalDistribution::from_raw(raw).map_err(ContractError::Core)
    }

    /// Reads market status (initialised / paused / settled).
    #[instrument(skip(self), fields(market = %self.address))]
    pub async fn market_status(&self) -> ContractResult<MarketStatus> {
        self.call_view::<MarketStatus>("get_market_status", amm::get_market_status(), &[])
            .await
    }

    /// Reads the AMM parameters (k, backing, tolerance, …).
    #[instrument(skip(self), fields(market = %self.address))]
    pub async fn params(&self) -> ContractResult<AmmParamsRaw> {
        self.call_view::<AmmParamsRaw>("get_params", amm::get_params(), &[])
            .await
    }

    /// Reads the full AMM configuration (params + collateral token + decimals).
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

    /// Reads the LP pool info (total shares + total backing).
    #[instrument(skip(self), fields(market = %self.address))]
    pub async fn lp_info(&self) -> ContractResult<LpInfoRaw> {
        self.call_view::<LpInfoRaw>("get_lp_info", amm::get_lp_info(), &[])
            .await
    }

    /// Reads a trader's position summary.
    #[instrument(skip(self), fields(market = %self.address, %trader))]
    pub async fn position_summary(&self, trader: Felt) -> ContractResult<PositionSummaryRaw> {
        self.call_view::<PositionSummaryRaw>(
            "get_position_summary",
            amm::get_position_summary(),
            &[trader],
        )
        .await
    }

    /// Reads a trader's compact position record.
    #[instrument(skip(self), fields(market = %self.address, %trader))]
    pub async fn position(&self, trader: Felt) -> ContractResult<PositionCompactRaw> {
        self.call_view::<PositionCompactRaw>(
            "get_position_compact",
            amm::get_position_compact(),
            &[trader],
        )
        .await
    }

    /// Quote a candidate trade against the **current** market.
    ///
    /// This is the preflight a production market-maker takes before
    /// every submission. The reader:
    /// 1. Reads the current distribution + AMM params.
    /// 2. Fetches chain-correct sqrt hints for `candidate` from
    ///    `runtime`'s `compute_hints_view`.
    /// 3. Calls `check_trade_view` to obtain the chain's verdict
    ///    (`is_valid`, `rejection_reason`, computed collateral).
    ///
    /// The returned [`NormalTradeQuote`] carries everything
    /// [`NormalMarketWriter::execute_quote`] needs to submit the trade —
    /// no further chain round-trips required.
    #[instrument(skip(self), fields(market = %self.address, runtime = %runtime))]
    pub async fn quote_trade(
        &self,
        runtime: Felt,
        candidate: NormalDistributionRaw,
        x_star: Sq128Raw,
        supplied_collateral: Sq128Raw,
        collateral_pad: Sq128Raw,
    ) -> TradeResult<NormalTradeQuote> {
        let current = self.distribution().await?.to_raw();
        let params = self.params().await?;
        let current_hints = compute_normal_hints(&self.provider, runtime, current).await?;
        let candidate_hints = compute_normal_hints(&self.provider, runtime, candidate).await?;
        let check = crate::runtime::check_normal_trade(
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
        let rejection = map_trade_rejection(check.rejection_reason, &check);
        Ok(NormalTradeQuote {
            candidate,
            candidate_hints,
            x_star,
            required_collateral: check.verification.computed_collateral,
            padded_collateral: max_sq(supplied_collateral, collateral_pad),
            on_chain_will_accept: check.is_valid,
            rejection,
        })
    }

    /// Closed-form payout at observation point `x_star` for the given
    /// candidate distribution.
    ///
    /// Returns `λ · pdf(x*; μ_eff, σ_eff)` in f64 (no chain round-trip,
    /// no Sq128 arithmetic — this is the hot-path pricing call). The
    /// caller supplies `k` so the same reader can re-price the same
    /// distribution at multiple `k` values without re-fetching params.
    ///
    /// Pair with [`Self::params`] to fetch the live `k` once, then
    /// price many candidates against it.
    #[inline]
    #[must_use]
    pub fn payout_at(
        &self,
        x_star: f64,
        candidate: &deadeye_core::NormalDistribution,
        k: f64,
    ) -> f64 {
        crate::pricing::payout_at_normal(candidate, k, x_star)
    }

    /// Estimate the collateral required to shift the market mean by
    /// `delta_mu`, keeping σ constant.
    ///
    /// Reads current distribution + params, builds the candidate
    /// `(μ_current + Δ, σ_current)`, and runs the off-chain
    /// [`deadeye_collateral::normal_collateral`] solver. Returns the
    /// `x*` and the implied collateral — exactly what a strategy needs
    /// to decide whether the move is in budget.
    ///
    /// This is f64 throughout; the on-chain verifier re-checks the
    /// result with Q128.128 arithmetic at trade submission time.
    pub async fn impact_for_mu_shift(
        &self,
        delta_mu: f64,
    ) -> ContractResult<crate::pricing::ImpactEstimate> {
        let current = self.distribution().await?;
        let params = self.params().await?;
        let mu_current = current.mean().to_f64();
        let sigma = current.sigma().to_f64();
        let target_mu = mu_current + delta_mu;
        let mu_q = deadeye_core::Sq128::from_f64(target_mu).map_err(ContractError::Core)?;
        let sigma_q = deadeye_core::Sq128::from_f64(sigma).map_err(ContractError::Core)?;
        let candidate = deadeye_core::NormalDistribution::from_sigma(mu_q, sigma_q)
            .map_err(ContractError::Core)?;
        let _ = params; // params kept available for callers via reader.params().
        let solver = deadeye_collateral::normal_collateral(
            &current,
            &candidate,
            deadeye_collateral::MinimizationPolicy::standard(),
        )
        .map_err(|e| ContractError::InvalidResponse {
            call: "impact_for_mu_shift",
            message: format!("collateral solver: {e}"),
        })?;
        Ok(crate::pricing::ImpactEstimate {
            delta_mu,
            x_star: solver.x_min,
            required_collateral: solver.collateral,
            iterations: solver.iterations,
        })
    }

    /// Numerical greeks for `candidate` at `x_star`.
    ///
    /// Pure off-chain; safe to call inside a tight loop (every greek is
    /// a constant number of [`Self::payout_at`] evaluations).
    #[inline]
    #[must_use]
    pub fn sensitivities_at(
        &self,
        candidate: &deadeye_core::NormalDistribution,
        x_star: f64,
        k: f64,
    ) -> crate::pricing::NormalSensitivities {
        crate::pricing::sensitivities_normal(candidate, k, x_star)
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

/// A preflighted normal-AMM trade.
///
/// Carries the chain-correct hints + `x_star` + collateral the writer
/// will submit. Construct via [`NormalMarketReader::quote_trade`]; execute
/// via [`NormalMarketWriter::execute_quote`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NormalTradeQuote {
    /// Candidate distribution the MM wants to move the market to.
    pub candidate: NormalDistributionRaw,
    /// Chain-correct sqrt hints for `candidate`, ready for submission.
    pub candidate_hints: NormalSqrtHintsRaw,
    /// `x_star` the off-chain solver (or the caller) picked.
    pub x_star: Sq128Raw,
    /// Collateral the chain computed as required for this trade.
    pub required_collateral: Sq128Raw,
    /// Collateral the writer should actually supply (≥ required, with
    /// pad).
    pub padded_collateral: Sq128Raw,
    /// `true` iff the chain's `check_trade_view` accepted the trade.
    pub on_chain_will_accept: bool,
    /// Typed rejection reason if `!on_chain_will_accept`.
    pub rejection: Option<TradeRejectionReason>,
}

/// Pick the larger of two `Sq128Raw` values via signed comparison.
fn max_sq(a: Sq128Raw, b: Sq128Raw) -> Sq128Raw {
    let lhs = deadeye_core::Sq128::from_raw(a);
    let rhs = deadeye_core::Sq128::from_raw(b);
    if lhs.cmp_signed(rhs) == core::cmp::Ordering::Less {
        b
    } else {
        a
    }
}

fn map_trade_rejection(raw: TradeRejection, check: &TradeCheckRaw) -> Option<TradeRejectionReason> {
    use crate::error::VerificationSubReason as Sub;
    match raw {
        TradeRejection::None => None,
        TradeRejection::InvalidDistribution => Some(TradeRejectionReason::InvalidDistribution),
        TradeRejection::InvalidHints => Some(TradeRejectionReason::InvalidHints),
        TradeRejection::BackingFail => Some(TradeRejectionReason::BackingFail),
        TradeRejection::SigmaTooLow => Some(TradeRejectionReason::SigmaTooLow),
        TradeRejection::LowCollateral => Some(TradeRejectionReason::LowCollateral),
        TradeRejection::VerificationFailed => {
            // Drill into the embedded verification flags to recover the
            // refined sub-reason without round-tripping a revert string.
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

/// Decoded market status (`get_market_status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketStatus {
    /// Whether `initialize()` has been called.
    pub is_initialised: bool,
    /// Whether the market is currently paused.
    pub is_paused: bool,
    /// Whether the market has been settled.
    pub is_settled: bool,
    /// Settlement value (only meaningful if `is_settled`).
    pub settlement_value: deadeye_core::sq128::Sq128Raw,
}

impl CairoSerde for MarketStatus {
    fn encode(&self, out: &mut Vec<Felt>) {
        self.is_initialised.encode(out);
        self.is_paused.encode(out);
        self.is_settled.encode(out);
        self.settlement_value.encode(out);
    }
    fn decode(slice: &[Felt]) -> Result<(Self, &[Felt]), crate::cairo_serde::CairoSerdeError> {
        let (is_initialised, slice) = bool::decode(slice)?;
        let (is_paused, slice) = bool::decode(slice)?;
        let (is_settled, slice) = bool::decode(slice)?;
        let (settlement_value, slice) = deadeye_core::sq128::Sq128Raw::decode(slice)?;
        Ok((
            Self {
                is_initialised,
                is_paused,
                is_settled,
                settlement_value,
            },
            slice,
        ))
    }
}

// ─── Writer ──────────────────────────────────────────────────────────────────

/// Write-capable companion to [`NormalMarketReader`]. Pairs the reader with
/// an [`Account`] so the same handle can run pre-flight reads, build
/// calldata, and submit the trade in one place.
#[derive(Debug)]
pub struct NormalMarketWriter<P, A>
where
    P: Provider,
    A: Account,
{
    reader: NormalMarketReader<P>,
    account: A,
}

impl<P, A> NormalMarketWriter<P, A>
where
    P: Provider,
    A: Account,
{
    /// Construct a writer from a reader and an account.
    pub const fn new(reader: NormalMarketReader<P>, account: A) -> Self {
        Self { reader, account }
    }

    /// Borrow the underlying reader.
    pub const fn reader(&self) -> &NormalMarketReader<P> {
        &self.reader
    }

    /// Borrow the underlying account.
    pub const fn account(&self) -> &A {
        &self.account
    }

    /// Build the [`Call`] for an `execute_trade` invocation without submitting.
    /// Useful when the caller wants to bundle additional calls (e.g. an
    /// ERC20 `approve`) into a single multi-call transaction.
    pub fn build_trade_call(&self, input: TradeInput) -> Call {
        Call {
            to: self.reader.address(),
            selector: amm::execute_trade(),
            calldata: input.to_calldata(),
        }
    }

    /// Build the [`Call`] for `sell_position_guarded`.
    ///
    /// On-chain signature (per `normal_amm.abi.json`):
    /// `sell_position_guarded(candidate, x_star, candidate_hints, guards)`.
    /// All four are forwarded as concatenated calldata in that order.
    pub fn build_sell_call(
        &self,
        candidate: NormalDistributionRaw,
        x_star: deadeye_core::sq128::Sq128Raw,
        candidate_hints: deadeye_core::distribution::NormalSqrtHintsRaw,
        guards: SellExecutionGuardsRaw,
    ) -> Call {
        let mut calldata = Vec::with_capacity(32);
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

    /// Build the [`Call`] for `claim` (the caller's own position).
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
    /// The ABI declares only `share_amount`; nothing else is encoded into the
    /// calldata. Starknet rejects extra calldata, so the prior "trail the
    /// hints" shape would always revert.
    pub fn build_add_liquidity_call(&self, share_amount: deadeye_core::sq128::Sq128Raw) -> Call {
        Call {
            to: self.reader.address(),
            selector: amm::add_liquidity(),
            calldata: share_amount.to_calldata(),
        }
    }

    /// Build the [`Call`] for `remove_liquidity(share_amount)`.
    ///
    /// The ABI declares only `share_amount`; see [`Self::build_add_liquidity_call`].
    pub fn build_remove_liquidity_call(&self, share_amount: deadeye_core::sq128::Sq128Raw) -> Call {
        Call {
            to: self.reader.address(),
            selector: amm::remove_liquidity(),
            calldata: share_amount.to_calldata(),
        }
    }

    /// Execute a trade.
    #[instrument(skip(self, input), fields(market = %self.reader.address(), family = "normal", kind = "trade"))]
    pub async fn execute_trade(&self, input: TradeInput) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_trade_call(input)])
            .await
    }

    /// Execute a guarded sell. The four arguments mirror the ABI:
    /// `sell_position_guarded(candidate, x_star, candidate_hints, guards)`.
    #[instrument(skip(self, candidate, x_star, candidate_hints, guards), fields(market = %self.reader.address(), family = "normal", kind = "sell"))]
    pub async fn sell_position_guarded(
        &self,
        candidate: NormalDistributionRaw,
        x_star: deadeye_core::sq128::Sq128Raw,
        candidate_hints: deadeye_core::distribution::NormalSqrtHintsRaw,
        guards: SellExecutionGuardsRaw,
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
    #[instrument(skip(self), fields(market = %self.reader.address(), family = "normal", kind = "claim"))]
    pub async fn claim(&self) -> ContractResult<ExecutionReceipt> {
        self.account.execute(vec![self.build_claim_call()]).await
    }

    /// Claim a settled position on behalf of `trader`.
    #[instrument(skip(self), fields(market = %self.reader.address(), family = "normal", kind = "claim_for", %trader))]
    pub async fn claim_for(&self, trader: Felt) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_claim_for_call(trader)])
            .await
    }

    /// Add liquidity to the pool. ABI takes a single `share_amount`.
    #[instrument(skip(self, share_amount), fields(market = %self.reader.address(), family = "normal", kind = "lp_add"))]
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
    #[instrument(skip(self, share_amount), fields(market = %self.reader.address(), family = "normal", kind = "lp_remove"))]
    pub async fn remove_liquidity(
        &self,
        share_amount: deadeye_core::sq128::Sq128Raw,
    ) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_remove_liquidity_call(share_amount)])
            .await
    }

    /// Submit a quote previously prepared by
    /// [`NormalMarketReader::quote_trade`].
    ///
    /// Returns [`TradeError::Submission`] if the writer refuses the quote
    /// (`on_chain_will_accept = false`) or if the on-chain call reverts;
    /// reverts get promoted into a typed [`TradeError::Rejected`] arm so
    /// callers can branch on `TradeRejectionReason`.
    #[instrument(skip(self, quote), fields(market = %self.reader.address(), family = "normal", kind = "trade", accepts = quote.on_chain_will_accept))]
    pub async fn execute_quote(&self, quote: NormalTradeQuote) -> TradeResult<ExecutionReceipt> {
        if !quote.on_chain_will_accept {
            return Err(TradeError::Rejected {
                reason: quote.rejection.unwrap_or(TradeRejectionReason::Other {
                    raw: "quote not acceptable on-chain",
                }),
                source: ContractError::InvalidResponse {
                    call: "execute_quote",
                    message: "quote.on_chain_will_accept = false".into(),
                },
            });
        }
        let call = self.build_trade_call(TradeInput {
            candidate: quote.candidate,
            x_star: quote.x_star,
            supplied_collateral: quote.padded_collateral,
            candidate_hints: quote.candidate_hints,
        });
        self.account
            .execute(vec![call])
            .await
            .map_err(TradeError::from_contract)
    }

    /// Close out the caller's full position with a single high-level
    /// call.
    ///
    /// Internally:
    /// * reads live `distribution`, `params`, `lp_info` (so the on-chain
    ///   `expected_*` guards see byte-exact values);
    /// * fetches chain-correct sqrt hints for the current dist;
    /// * builds [`SellExecutionGuardsRaw`] using live LP backing (the
    ///   AMM guards against `get_pool_backing()` — see
    ///   `docs/DEVNET_SHAKEDOWN.md` for why `params.backing` is wrong);
    /// * submits `sell_position_guarded(current_dist, x* = current.mean,
    ///   current_hints, guards)`.
    ///
    /// The caller specifies only the minimum-token-out slippage floor.
    #[instrument(skip(self), fields(market = %self.reader.address(), family = "normal", kind = "sell", runtime = %runtime))]
    pub async fn sell_position(
        &self,
        runtime: Felt,
        min_token_out: u128,
    ) -> TradeResult<ExecutionReceipt> {
        let market_dist = self.reader.distribution().await?.to_raw();
        let params = self.reader.params().await?;
        let lp_info = self.reader.lp_info().await?;
        let candidate_hints =
            compute_normal_hints(self.reader.provider(), runtime, market_dist).await?;
        let guards = SellExecutionGuardsRaw {
            expected_market_dist: market_dist,
            expected_backing: lp_info.total_backing_deposited,
            expected_tolerance: params.tolerance,
            expected_min_trade_collateral: params.min_trade_collateral,
            min_token_out,
        };
        let call = self.build_sell_call(market_dist, market_dist.mean, candidate_hints, guards);
        self.account
            .execute(vec![call])
            .await
            .map_err(TradeError::from_contract)
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use std::{sync::Mutex, vec, vec::Vec};

    use async_trait::async_trait;
    use deadeye_core::{Distribution, sq128::Sq128Raw};
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

    #[tokio::test]
    async fn distribution_decodes_correctly() {
        let raw = NormalDistributionRaw {
            mean: Sq128Raw {
                limb0: 0,
                limb1: 0,
                limb2: 100,
                limb3: 0,
                neg: false,
            },
            variance: Sq128Raw {
                limb0: 0,
                limb1: 0,
                limb2: 4,
                limb3: 0,
                neg: false,
            },
            sigma: Sq128Raw {
                limb0: 0,
                limb1: 0,
                limb2: 2,
                limb3: 0,
                neg: false,
            },
        };
        let canned = raw.to_calldata();
        let provider = CannedProvider {
            responses: Mutex::new(vec![canned]),
        };
        let reader = NormalMarketReader::new(provider, Felt::from(0x1234_u64));
        let dist = reader.distribution().await.unwrap();
        assert_eq!(dist.mean().to_raw(), raw.mean);
        assert_eq!(dist.variance().to_raw(), raw.variance);
    }

    #[tokio::test]
    async fn market_status_decodes_correctly() {
        let status = MarketStatus {
            is_initialised: true,
            is_paused: false,
            is_settled: true,
            settlement_value: Sq128Raw {
                limb0: 0,
                limb1: 0,
                limb2: 42,
                limb3: 0,
                neg: false,
            },
        };
        let canned = status.to_calldata();
        let provider = CannedProvider {
            responses: Mutex::new(vec![canned]),
        };
        let reader = NormalMarketReader::new(provider, Felt::from(0x1234_u64));
        let observed = reader.market_status().await.unwrap();
        assert_eq!(observed, status);
    }

    struct MockAccount;
    #[async_trait]
    impl Account for MockAccount {
        fn address(&self) -> Felt {
            Felt::from(0x0_u64)
        }
        async fn execute(&self, _calls: Vec<Call>) -> ContractResult<ExecutionReceipt> {
            unreachable!()
        }
    }

    #[test]
    fn build_trade_call_uses_correct_selector() {
        let provider = CannedProvider {
            responses: Mutex::new(vec![]),
        };
        let reader = NormalMarketReader::new(provider, Felt::from(0x1234_u64));
        let writer = NormalMarketWriter::new(reader, MockAccount);
        let input = TradeInput {
            candidate: NormalDistributionRaw {
                mean: Sq128Raw {
                    limb0: 0,
                    limb1: 0,
                    limb2: 100,
                    limb3: 0,
                    neg: false,
                },
                variance: Sq128Raw {
                    limb0: 0,
                    limb1: 0,
                    limb2: 4,
                    limb3: 0,
                    neg: false,
                },
                sigma: Sq128Raw {
                    limb0: 0,
                    limb1: 0,
                    limb2: 2,
                    limb3: 0,
                    neg: false,
                },
            },
            x_star: Sq128Raw {
                limb0: 0,
                limb1: 0,
                limb2: 101,
                limb3: 0,
                neg: false,
            },
            supplied_collateral: Sq128Raw {
                limb0: 0,
                limb1: 0,
                limb2: 5,
                limb3: 0,
                neg: false,
            },
            candidate_hints: deadeye_core::distribution::NormalSqrtHintsRaw {
                l2_norm_denom: Sq128Raw {
                    limb0: 0,
                    limb1: 0,
                    limb2: 7,
                    limb3: 0,
                    neg: false,
                },
                backing_denom: Sq128Raw {
                    limb0: 0,
                    limb1: 0,
                    limb2: 9,
                    limb3: 0,
                    neg: false,
                },
            },
        };
        let call = writer.build_trade_call(input);
        assert_eq!(call.to, Felt::from(0x1234_u64));
        assert_eq!(call.selector, amm::execute_trade());
        // calldata = 5 felts/Sq128Raw * 3 distribution + 5 + 5 + 2*5 hints = 15 + 5 + 5 + 10 = 35
        assert_eq!(call.calldata.len(), 35);
    }

    // ─── Pricing primitives (Wave 2 Item 8) ─────────────────────────

    #[test]
    fn payout_at_matches_closed_form() {
        // N(0, 1): payout = λ · pdf(0). For k=1, σ=1 →
        // λ = √(2·1·√π); pdf(0) = 1/√(2π); product = √(1/√π) ≈ 0.7511
        let provider = CannedProvider {
            responses: Mutex::new(vec![]),
        };
        let reader = NormalMarketReader::new(provider, Felt::from(0x1234_u64));
        let dist = deadeye_core::NormalDistribution::from_sigma(
            deadeye_core::Sq128::ZERO,
            deadeye_core::Sq128::from_f64(1.0).unwrap(),
        )
        .unwrap();
        let payout = reader.payout_at(0.0, &dist, 1.0);
        let expected = (1.0_f64 / core::f64::consts::PI.sqrt()).sqrt();
        assert!(
            (payout - expected).abs() < 1e-9,
            "got {payout}, expected {expected}"
        );
    }

    #[tokio::test]
    async fn impact_for_mu_shift_returns_positive_collateral() {
        // Canned provider returns N(0,1) for `get_distribution` and an
        // AmmParams blob for `get_params`. impact_for_mu_shift(1.0) must
        // yield positive collateral (equal-σ μ-shift).
        let dist_raw = NormalDistributionRaw {
            mean: deadeye_core::Sq128::ZERO.to_raw(),
            variance: deadeye_core::Sq128::from_f64(1.0).unwrap().to_raw(),
            sigma: deadeye_core::Sq128::from_f64(1.0).unwrap().to_raw(),
        };
        // Params: k, backing, tolerance, min_trade_collateral, payout_amplifier
        let params = crate::types::common::AmmParamsRaw {
            k: deadeye_core::Sq128::from_f64(1.0).unwrap().to_raw(),
            backing: deadeye_core::Sq128::from_f64(1000.0).unwrap().to_raw(),
            tolerance: deadeye_core::Sq128::from_f64(1.0).unwrap().to_raw(),
            min_trade_collateral: deadeye_core::Sq128::from_f64(0.001).unwrap().to_raw(),
            payout_amplifier: deadeye_core::Sq128::from_f64(1.0).unwrap().to_raw(),
        };
        // Mutex pops responses LIFO — push params *first* then dist so
        // distribution() (first await) gets dist, and params() gets params.
        let provider = CannedProvider {
            responses: Mutex::new(vec![params.to_calldata(), dist_raw.to_calldata()]),
        };
        let reader = NormalMarketReader::new(provider, Felt::from(0x1234_u64));
        let estimate = reader.impact_for_mu_shift(1.0_f64).await.unwrap();
        assert!(
            estimate.required_collateral > 0.0,
            "expected positive collateral, got {estimate:?}",
        );
        assert!(estimate.x_star.is_finite());
    }

    #[test]
    fn sensitivities_at_finite_and_signed() {
        let provider = CannedProvider {
            responses: Mutex::new(vec![]),
        };
        let reader = NormalMarketReader::new(provider, Felt::from(0x1234_u64));
        let dist = deadeye_core::NormalDistribution::from_sigma(
            deadeye_core::Sq128::ZERO,
            deadeye_core::Sq128::from_f64(1.0).unwrap(),
        )
        .unwrap();
        let s = reader.sensitivities_at(&dist, 0.5, 1.0);
        assert!(s.d_payout_d_mu.is_finite());
        assert!(s.d_payout_d_sigma.is_finite());
        // At x* = 0.5 > μ = 0, raising μ moves it closer to x*, raising payout.
        assert!(s.d_payout_d_mu > 0.0, "got {s:?}");
    }
}
