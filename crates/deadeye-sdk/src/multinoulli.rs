//! Multinoulli (categorical) AMM market handle.
//!
//! Categorical markets have *N* outcomes and a probability vector
//! summing to 1. There are three trade shapes (dense / sparse /
//! transfers); the writer offers a helper per shape that handles
//! hint-fetching internally.
//!
//! ## Worked example — dense trade
//!
//! ```no_run
//! use deadeye_sdk::{
//!     core::{categorical::CategoricalDistributionRaw, sq128::Sq128Raw},
//!     starknet::{
//!         Felt, JsonRpcProvider, MultinoulliMarketReader, MultinoulliMarketWriter, OwnedAccount,
//!     },
//! };
//! use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let rpc = JsonRpcClient::new(HttpTransport::new(
//!     "http://localhost:5050".parse::<url::Url>()?,
//! ));
//! let provider = JsonRpcProvider::new(rpc);
//! let (market, runtime): (Felt, Felt) = (Felt::ZERO, Felt::ZERO);
//! let reader = MultinoulliMarketReader::new(&provider, market);
//! let signer = OwnedAccount::from_signing_key(
//!     JsonRpcClient::new(HttpTransport::new(
//!         "http://localhost:5050".parse::<url::Url>()?,
//!     )),
//!     Felt::ZERO,
//!     Felt::ZERO,
//!     Felt::ZERO,
//! );
//! let writer = MultinoulliMarketWriter::new(reader, signer);
//!
//! let candidate = CategoricalDistributionRaw {
//!     probs: vec![Sq128Raw::ZERO; 4],
//! };
//! let quote = writer
//!     .reader()
//!     .quote_trade(runtime, candidate, 0, Sq128Raw::ZERO)
//!     .await?;
//! writer.execute_quote(quote).await?;
//! writer.sell_position(0).await?;
//! # Ok(()) }
//! ```

use deadeye_collateral::{CategoricalVerifiedMinimum, categorical_collateral};
use deadeye_core::{CategoricalDistribution, Sq128};
use deadeye_starknet::{
    Account, ExecutionReceipt, Felt, MultinoulliMarketReader, MultinoulliMarketWriter, Provider,
};
use futures::future::join_all;
use tracing::instrument;

use crate::{
    error::{SdkError, SdkResult},
    legs::{LegInfo, LegValuation, PositionLegs, PositionValuation, SettlementPoint},
};

/// Handle to a deployed multinoulli AMM market.
#[derive(Debug)]
pub struct MultinoulliMarket<'p, P>
where
    P: Provider,
{
    reader: MultinoulliMarketReader<&'p P>,
}

impl<'p, P> MultinoulliMarket<'p, P>
where
    P: Provider,
{
    /// Construct a handle.
    pub fn new(provider: &'p P, address: Felt) -> Self {
        Self {
            reader: MultinoulliMarketReader::new(provider, address),
        }
    }

    /// Underlying read-only reader.
    pub const fn reader(&self) -> &MultinoulliMarketReader<&'p P> {
        &self.reader
    }

    /// Returns the contract address.
    pub const fn address(&self) -> Felt {
        self.reader.address()
    }

    /// Fetch the current market distribution.
    pub async fn distribution(&self) -> SdkResult<CategoricalDistribution> {
        Ok(self.reader.distribution().await?)
    }

    /// Computes the collateral required to move the market from its current
    /// distribution to `candidate`. Returns the off-chain solver's
    /// [`CategoricalVerifiedMinimum`], from which the caller derives the
    /// `min_outcome_index` hint and supplied collateral.
    #[instrument(skip(self, candidate), fields(market = %self.reader.address()))]
    pub async fn prepare_quote(
        &self,
        candidate: &CategoricalDistribution,
        k: f64,
    ) -> SdkResult<CategoricalVerifiedMinimum> {
        let current = self.distribution().await?;
        Ok(categorical_collateral(&current, candidate, k)?)
    }

    // ── Multi-leg (trade-lot) position tracking + valuation ─────────────

    /// Enumerate a trader's legs (lot ids + lifecycle flags) and read the
    /// position summary. Lifecycle flags are fetched concurrently.
    #[instrument(skip(self), fields(market = %self.reader.address(), %trader))]
    pub async fn legs(&self, trader: Felt) -> SdkResult<PositionLegs> {
        let summary = self.reader.position_summary(trader).await?;
        let ids = self.reader.trade_lot_ids(trader).await?;
        let legs = join_all(ids.iter().map(|&lot_id| async move {
            let settled = self.reader.trade_lot_settled(lot_id).await?;
            let cancelled = self.reader.trade_lot_cancelled(lot_id).await?;
            Ok::<LegInfo, SdkError>(LegInfo {
                lot_id,
                settled,
                cancelled,
            })
        }))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
        Ok(PositionLegs {
            trader: format!("{trader:#x}"),
            legs,
            total_collateral: Sq128::from_raw(summary.total_collateral_locked).to_f64(),
            exists: summary.exists,
            claimed: summary.claimed,
            tracks_settlement_claim: summary.tracks_settlement_claim,
        })
    }

    /// Value a trader's whole position at a settlement `outcome` index,
    /// authoritatively (each active leg via the on-chain `value_at` view).
    /// `total_position_value` is the position's P&L if the market settles at
    /// `outcome`; `gross_return` adds back the locked collateral.
    #[instrument(skip(self), fields(market = %self.reader.address(), %trader, outcome))]
    pub async fn position_value_at(
        &self,
        trader: Felt,
        outcome: u32,
    ) -> SdkResult<PositionValuation> {
        let summary = self.reader.position_summary(trader).await?;
        let ids = self.reader.trade_lot_ids(trader).await?;
        let legs = join_all(ids.iter().map(|&lot_id| async move {
            let settled = self.reader.trade_lot_settled(lot_id).await?;
            let cancelled = self.reader.trade_lot_cancelled(lot_id).await?;
            // Settled/cancelled legs have no future payout.
            let value_at = if settled || cancelled {
                0.0
            } else {
                Sq128::from_raw(self.reader.trade_lot_value_at(lot_id, outcome).await?).to_f64()
            };
            Ok::<LegValuation, SdkError>(LegValuation {
                lot_id,
                settled,
                cancelled,
                value_at,
            })
        }))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
        let total_position_value: f64 = legs.iter().map(|l| l.value_at).sum();
        let total_collateral = Sq128::from_raw(summary.total_collateral_locked).to_f64();
        Ok(PositionValuation {
            trader: format!("{trader:#x}"),
            settlement: SettlementPoint::Outcome(outcome),
            legs,
            total_collateral,
            total_position_value,
            gross_return: total_collateral + total_position_value,
            exists: summary.exists,
            claimed: summary.claimed,
        })
    }

    /// Expected position value (P&L) under a categorical belief — a finite
    /// sum `Σ_{i<n} belief[i] · Σ_{active legs} value_at(lot, i)`. Returns the
    /// expected P&L in XP. `belief` must have one entry per outcome
    /// (`belief.len() == outcome_count`); costs `n × active legs` `value_at`
    /// reads (fanned out concurrently).
    ///
    /// # Errors
    ///
    /// Returns [`SdkError::Core`] with a `CoreError::InvalidInput` if
    /// `belief.len()` does not equal the market's outcome count.
    #[instrument(skip(self, belief), fields(market = %self.reader.address(), %trader))]
    pub async fn expected_value_under_belief(
        &self,
        trader: Felt,
        belief: &[f64],
    ) -> SdkResult<f64> {
        let summary = self.reader.position_summary(trader).await?;
        let outcome_count = summary.outcome_count as usize;
        if belief.len() != outcome_count {
            return Err(SdkError::from(deadeye_core::CoreError::invalid_input(
                "belief",
                "length must equal the market's outcome count",
            )));
        }
        let ids = self.reader.trade_lot_ids(trader).await?;
        // Keep only claimable legs.
        let flags = join_all(ids.iter().map(|&lot_id| async move {
            let settled = self.reader.trade_lot_settled(lot_id).await?;
            let cancelled = self.reader.trade_lot_cancelled(lot_id).await?;
            Ok::<(u64, bool), SdkError>((lot_id, !settled && !cancelled))
        }))
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
        let active: Vec<u64> = flags
            .into_iter()
            .filter_map(|(id, ok)| ok.then_some(id))
            .collect();
        if active.is_empty() {
            return Ok(0.0);
        }
        // EV = Σ_i belief[i] · Σ_legs value_at(lot, i). Fan out every
        // (outcome, leg) read concurrently, weighting each by belief[i].
        let mut futs = Vec::with_capacity(outcome_count * active.len());
        for (i, &weight) in belief.iter().enumerate() {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "outcome index bounded by outcome_count (u32 on chain)"
            )]
            let outcome = i as u32;
            for &lot_id in &active {
                futs.push(async move {
                    let v = Sq128::from_raw(self.reader.trade_lot_value_at(lot_id, outcome).await?)
                        .to_f64();
                    Ok::<f64, SdkError>(weight * v)
                });
            }
        }
        let parts = join_all(futs)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        Ok(parts.into_iter().sum())
    }

    /// Bind an account to enable write paths.
    pub fn with_account<A>(self, account: A) -> MultinoulliMarketSigned<'p, P, A>
    where
        A: Account,
    {
        MultinoulliMarketSigned {
            writer: MultinoulliMarketWriter::new(self.reader, account),
        }
    }
}

/// Account-bound companion to [`MultinoulliMarket`].
#[derive(Debug)]
pub struct MultinoulliMarketSigned<'p, P, A>
where
    P: Provider,
    A: Account,
{
    writer: MultinoulliMarketWriter<&'p P, A>,
}

impl<P, A> MultinoulliMarketSigned<'_, P, A>
where
    P: Provider,
    A: Account,
{
    /// Borrow the underlying writer for direct calldata construction.
    pub const fn writer(&self) -> &MultinoulliMarketWriter<&P, A> {
        &self.writer
    }

    /// Returns the market address.
    pub const fn address(&self) -> Felt {
        self.writer.reader().address()
    }

    /// Fetch the current market distribution (read passthrough).
    pub async fn distribution(&self) -> SdkResult<CategoricalDistribution> {
        Ok(self.writer.reader().distribution().await?)
    }

    /// Execute a dense trade. The caller is responsible for constructing
    /// the candidate distribution and the L2 hint; `min_outcome_index`
    /// must be the on-chain-verifiable minimum (we recommend computing
    /// it via `prepare_quote` on a [`MultinoulliMarket`]).
    pub async fn execute_trade(
        &self,
        candidate: &CategoricalDistribution,
        min_outcome_index: u32,
        supplied_collateral: deadeye_core::sq128::Sq128Raw,
        l2_norm_hint: deadeye_core::sq128::Sq128Raw,
    ) -> SdkResult<ExecutionReceipt> {
        let input = deadeye_starknet::types::multinoulli::MultinoulliTradeInput {
            candidate: candidate.to_raw()?,
            min_outcome_index,
            supplied_collateral,
            candidate_hint: deadeye_core::categorical::CategoricalL2HintRaw { l2_norm_hint },
        };
        Ok(self.writer.execute_trade(&input).await?)
    }
}
