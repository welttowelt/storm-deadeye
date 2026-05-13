//! Normal-distribution AMM market handle.
//!
//! Provides the "read-then-quote-then-execute" loop a market-maker drives
//! against a Gaussian AMM. The handle wraps [`deadeye_starknet`]'s
//! lower-level reader / writer pair and adds the off-chain collateral
//! solver — most callers want exactly this.
//!
//! ## Worked example — quote, execute, sell
//!
//! ```no_run
//! use deadeye_sdk::normal::NormalMarket;
//! use deadeye_sdk::starknet::{
//!     Felt, JsonRpcProvider, NormalMarketReader, NormalMarketWriter, OwnedAccount,
//! };
//! use deadeye_sdk::core::{distribution::NormalDistributionRaw, sq128::Sq128Raw};
//! use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let rpc = JsonRpcClient::new(HttpTransport::new("http://localhost:5050".parse::<url::Url>()?));
//! let provider = JsonRpcProvider::new(rpc);
//! let (market, runtime): (Felt, Felt) = (Felt::ZERO, Felt::ZERO);
//!
//! let reader = NormalMarketReader::new(&provider, market);
//! let signer = OwnedAccount::from_signing_key(
//!     JsonRpcClient::new(HttpTransport::new("http://localhost:5050".parse::<url::Url>()?)),
//!     Felt::ZERO, Felt::ZERO, Felt::ZERO,
//! );
//! let writer = NormalMarketWriter::new(reader, signer);
//!
//! let candidate = NormalDistributionRaw {
//!     mean: Sq128Raw::ZERO, variance: Sq128Raw::ZERO, sigma: Sq128Raw::ZERO,
//! };
//! let quote = writer
//!     .reader()
//!     .quote_trade(runtime, candidate, Sq128Raw::ZERO, Sq128Raw::ZERO, Sq128Raw::ZERO)
//!     .await?;
//! if quote.on_chain_will_accept {
//!     writer.execute_quote(quote).await?;
//! }
//! writer.sell_position(runtime, 0).await?;
//! # Ok(()) }
//! ```

use deadeye_collateral::{MinimizationPolicy, normal_collateral};
use deadeye_core::{NormalDistribution, Sq128};
use deadeye_optimizer::{NormalOptimizationInput, optimize_normal_trade};
use deadeye_starknet::{
    Account, ExecutionReceipt, Felt, NormalMarketReader, NormalMarketWriter, NormalTradeQuote,
    Provider,
    types::normal::TradeInput,
};
use tracing::instrument;

use crate::{error::SdkResult, quote::PreparedQuote};

/// Handle to a deployed normal AMM market.
///
/// A handle bundles a reader (and optionally a writer) bound to a single
/// market address. It is the canonical entry point for off-chain quote
/// preparation + on-chain execution.
#[derive(Debug)]
pub struct NormalMarket<'p, P>
where
    P: Provider,
{
    reader: NormalMarketReader<&'p P>,
}

impl<'p, P> NormalMarket<'p, P>
where
    P: Provider,
{
    /// Construct a handle bound to `provider` and the on-chain `address`.
    pub fn new(provider: &'p P, address: Felt) -> Self {
        Self {
            reader: NormalMarketReader::new(provider, address),
        }
    }

    /// Underlying read-only reader.
    pub const fn reader(&self) -> &NormalMarketReader<&'p P> {
        &self.reader
    }

    /// Returns the contract address.
    pub const fn address(&self) -> Felt {
        self.reader.address()
    }

    /// Fetch the current market distribution.
    pub async fn distribution(&self) -> SdkResult<NormalDistribution> {
        Ok(self.reader.distribution().await?)
    }

    /// Prepares a quote that moves the market from its current distribution
    /// to `candidate`. Returns the [`PreparedQuote`] containing the off-chain
    /// `x*` and required collateral.
    #[instrument(skip(self), fields(market = %self.reader.address()))]
    pub async fn prepare_quote(
        &self,
        candidate: NormalDistribution,
        policy: MinimizationPolicy,
    ) -> SdkResult<PreparedQuote> {
        let current = self.distribution().await?;
        let verified = normal_collateral(&current, &candidate, policy)?;
        Ok(PreparedQuote {
            x_star: Sq128::from_f64(verified.x_min)?,
            collateral: Sq128::from_f64(verified.collateral)?,
            iterations: verified.iterations,
        })
    }

    /// Pick the EV-maximizing candidate distribution given a belief +
    /// budget, then quote it against `runtime`.
    ///
    /// Use this when you have a directional view (the trader's belief
    /// about the true outcome) and a budget, but don't want to hand-pick
    /// the candidate `(μ_g, σ_g)`. The optimizer searches a fixed grid
    /// inside the policy region for the highest net-EV trade, then
    /// hands the winner to [`NormalMarketReader::quote_trade`] for a
    /// full chain preflight.
    ///
    /// # Inputs
    ///
    /// * `runtime` — math runtime contract address (used by `quote_trade`).
    /// * `belief_mean` / `belief_sigma` — the trader's belief about the
    ///   true outcome distribution.
    /// * `budget` — maximum collateral the trader is willing to risk.
    ///
    /// # Returns
    ///
    /// A [`NormalTradeQuote`]. Inspect `on_chain_will_accept`; if `false`
    /// the budget was too tight for any in-policy candidate (and the
    /// quote will carry the optimizer's "no trade" candidate equal to
    /// the current market distribution). If `true`, hand the quote to
    /// [`NormalMarketWriter::execute_quote`] from a signed handle.
    ///
    /// # Worked example
    ///
    /// ```no_run
    /// # use deadeye_sdk::normal::NormalMarket;
    /// # use deadeye_sdk::starknet::{Felt, JsonRpcProvider};
    /// # use deadeye_sdk::core::Sq128;
    /// # use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let rpc = JsonRpcClient::new(HttpTransport::new(
    ///     "http://localhost:5050".parse::<url::Url>()?,
    /// ));
    /// let provider = JsonRpcProvider::new(rpc);
    /// let market = NormalMarket::new(&provider, Felt::ZERO);
    /// // I believe outcome ≈ 50 (±2σ confidence), can spend up to 10 STRK.
    /// let quote = market.optimize_quote(Felt::ZERO, 50.0, 2.0, 10.0).await?;
    /// if quote.on_chain_will_accept {
    ///     // hand to a signed handle to execute
    /// }
    /// # Ok(()) }
    /// ```
    #[instrument(skip(self), fields(market = %self.reader.address(), runtime = %runtime))]
    pub async fn optimize_quote(
        &self,
        runtime: Felt,
        belief_mean: f64,
        belief_sigma: f64,
        budget: f64,
    ) -> SdkResult<NormalTradeQuote> {
        let current = self.distribution().await?;
        let params = self.reader.params().await?;
        let market_mean = current.mean().to_f64();
        let market_sigma = current.sigma().to_f64();
        // TODO(v1.1): scale `k` by `pool_backing / initial_backing` once
        // `lp_info` exposes the immutable initial backing distinctly from
        // `params.backing`. Today `params.k` reflects the AMM's stored k;
        // the on-chain verifier re-checks any candidate against the live
        // effective k, so a slightly-stale k here only widens the
        // optimizer's search region — never approves a bad trade.
        let effective_k = Sq128::from_raw(params.k).to_f64();
        let opt = optimize_normal_trade(NormalOptimizationInput::new(
            budget,
            belief_mean,
            belief_sigma,
            market_mean,
            market_sigma,
            effective_k,
        ));
        // If the optimizer returns the "no trade" sentinel (collateral=0,
        // candidate == current), short-circuit with a synthetic
        // `on_chain_will_accept=false` quote so callers see a stable shape.
        let cand_mean = Sq128::from_f64(opt.optimized_mean)?;
        let cand_sigma = Sq128::from_f64(opt.optimized_sigma)?;
        let candidate =
            deadeye_core::NormalDistribution::from_sigma(cand_mean, cand_sigma)?;
        let supplied = Sq128::from_f64(opt.collateral_required.max(0.0))?;
        let pad = supplied;
        // `x_star` = the optimizer's μ_g is a reasonable seed; the chain
        // re-derives the true stationary point from the candidate, so a
        // seed inside the candidate's support is sufficient.
        let x_star = cand_mean;
        Ok(self
            .reader
            .quote_trade(
                runtime,
                candidate.to_raw(),
                x_star.to_raw(),
                supplied.to_raw(),
                pad.to_raw(),
            )
            .await?)
    }

    /// Bind an [`Account`] to this market, returning a write-capable handle.
    ///
    /// The returned [`NormalMarketSigned`] holds the same reader plus the
    /// signer, so reads remain cheap and writes funnel through a single
    /// place.
    pub fn with_account<A>(self, account: A) -> NormalMarketSigned<'p, P, A>
    where
        A: Account,
    {
        NormalMarketSigned {
            writer: NormalMarketWriter::new(self.reader, account),
        }
    }
}

/// Account-bound companion to [`NormalMarket`].
#[derive(Debug)]
pub struct NormalMarketSigned<'p, P, A>
where
    P: Provider,
    A: Account,
{
    writer: NormalMarketWriter<&'p P, A>,
}

impl<P, A> NormalMarketSigned<'_, P, A>
where
    P: Provider,
    A: Account,
{
    /// Borrow the underlying writer.
    pub const fn writer(&self) -> &NormalMarketWriter<&P, A> {
        &self.writer
    }

    /// Returns the market address.
    pub const fn address(&self) -> Felt {
        self.writer.reader().address()
    }

    /// Fetch the current market distribution (read passthrough).
    pub async fn distribution(&self) -> SdkResult<NormalDistribution> {
        Ok(self.writer.reader().distribution().await?)
    }

    /// Execute a previously-prepared quote.
    #[instrument(skip(self, candidate), fields(market = %self.writer.reader().address()))]
    pub async fn execute(
        &self,
        candidate: NormalDistribution,
        quote: PreparedQuote,
    ) -> SdkResult<ExecutionReceipt> {
        // Build the on-chain trade input from the off-chain quote +
        // candidate. Hints default to zero — the on-chain math runtime
        // will fall back to its internal computation if the hints don't
        // validate. Callers that want deterministic gas should supply
        // their own hints via `writer().build_trade_call(...)`.
        let input = TradeInput {
            candidate: candidate.to_raw(),
            x_star: quote.x_star.to_raw(),
            supplied_collateral: quote.collateral.to_raw(),
            candidate_hints: deadeye_core::distribution::NormalSqrtHintsRaw {
                l2_norm_denom: Sq128::ZERO.to_raw(),
                backing_denom: Sq128::ZERO.to_raw(),
            },
        };
        Ok(self.writer.execute_trade(input).await?)
    }

    /// Pass-through to the underlying writer for advanced calldata building.
    pub const fn writer_ref(&self) -> &NormalMarketWriter<&P, A> {
        &self.writer
    }
}

// We re-export `Distribution` so callers that need to read `.mean()` /
// `.variance()` on the returned distribution can do so without an extra
// import.
pub use deadeye_core::Distribution;

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {
        let _ = core::any::type_name::<T>();
    }

    #[test]
    fn normal_market_is_send_sync() {
        assert_send_sync::<NormalMarket<'_, MockProvider>>();
    }

    use async_trait::async_trait;
    use deadeye_starknet::ContractResult;
    use starknet_core::types::{BlockId, FunctionCall};

    #[derive(Debug)]
    struct MockProvider;

    #[async_trait]
    impl Provider for MockProvider {
        async fn call(&self, _call: FunctionCall, _block: BlockId) -> ContractResult<Vec<Felt>> {
            unreachable!("mock provider only used for trait wiring")
        }
    }
}
