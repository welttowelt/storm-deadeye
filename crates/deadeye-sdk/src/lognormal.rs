//! Lognormal AMM market handle.
//!
//! Same shape as [`crate::normal`] but parameterised on the lognormal
//! distribution: μ + σ² live in log-space, the chain expects the dist's
//! σ as a separate field, and `x*` is always positive. Use this family
//! for markets whose payouts are non-negative and skewed (yield curves,
//! mcap targets, etc).
//!
//! ## Worked example — quote + `execute_quote` + sell
//!
//! ```no_run
//! use deadeye_sdk::starknet::{
//!     Felt, JsonRpcProvider, LognormalMarketReader, LognormalMarketWriter, OwnedAccount,
//! };
//! use deadeye_sdk::core::{distribution::LognormalDistributionRaw, sq128::Sq128Raw};
//! use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let rpc = JsonRpcClient::new(HttpTransport::new("http://localhost:5050".parse::<url::Url>()?));
//! let provider = JsonRpcProvider::new(rpc);
//! let (market, runtime): (Felt, Felt) = (Felt::ZERO, Felt::ZERO);
//! let reader = LognormalMarketReader::new(&provider, market);
//! let signer = OwnedAccount::from_signing_key(
//!     JsonRpcClient::new(HttpTransport::new("http://localhost:5050".parse::<url::Url>()?)),
//!     Felt::ZERO, Felt::ZERO, Felt::ZERO,
//! );
//! let writer = LognormalMarketWriter::new(reader, signer);
//!
//! let candidate = LognormalDistributionRaw {
//!     mu: Sq128Raw::ZERO, variance: Sq128Raw::ZERO, sigma: Sq128Raw::ZERO,
//! };
//! let quote = writer.reader()
//!     .quote_trade(runtime, candidate, Sq128Raw::ZERO, Sq128Raw::ZERO, Sq128Raw::ZERO)
//!     .await?;
//! writer.execute_quote(quote).await?;
//! writer.sell_position(runtime, 0).await?;
//! # Ok(()) }
//! ```

use deadeye_collateral::{LognormalOptions, LognormalVerifiedMinimum, lognormal_collateral};
use deadeye_core::{
    LognormalDistribution, distribution::LognormalDistributionRaw, sq128::Sq128Raw,
};
use deadeye_starknet::{
    Account, ExecutionReceipt, Felt, LognormalMarketReader, LognormalMarketWriter, Provider,
    types::lognormal::{
        LognormalSellExecutionGuardsRaw, LognormalSqrtHintsRaw, LognormalTradeInput,
    },
};
use tracing::instrument;

use crate::error::SdkResult;

/// Handle to a deployed lognormal AMM market.
#[derive(Debug)]
pub struct LognormalMarket<'p, P>
where
    P: Provider,
{
    reader: LognormalMarketReader<&'p P>,
}

impl<'p, P> LognormalMarket<'p, P>
where
    P: Provider,
{
    /// Construct a handle.
    pub fn new(provider: &'p P, address: Felt) -> Self {
        Self {
            reader: LognormalMarketReader::new(provider, address),
        }
    }

    /// Underlying read-only reader.
    pub const fn reader(&self) -> &LognormalMarketReader<&'p P> {
        &self.reader
    }

    /// Contract address.
    pub const fn address(&self) -> Felt {
        self.reader.address()
    }

    /// Reads the current market distribution.
    pub async fn distribution(&self) -> SdkResult<LognormalDistribution> {
        Ok(self.reader.distribution().await?)
    }

    /// Prepares an off-chain quote for moving the market from its current
    /// state to `candidate`.
    #[instrument(skip(self, candidate), fields(market = %self.reader.address()))]
    pub async fn prepare_quote(
        &self,
        candidate: &LognormalDistribution,
        opts: LognormalOptions,
    ) -> SdkResult<LognormalVerifiedMinimum> {
        let current = self.distribution().await?;
        Ok(lognormal_collateral(&current, candidate, opts)?)
    }

    /// Bind an account for writes.
    pub fn with_account<A>(self, account: A) -> LognormalMarketSigned<'p, P, A>
    where
        A: Account,
    {
        LognormalMarketSigned {
            writer: LognormalMarketWriter::new(self.reader, account),
        }
    }
}

/// Account-bound companion.
#[derive(Debug)]
pub struct LognormalMarketSigned<'p, P, A>
where
    P: Provider,
    A: Account,
{
    writer: LognormalMarketWriter<&'p P, A>,
}

impl<P, A> LognormalMarketSigned<'_, P, A>
where
    P: Provider,
    A: Account,
{
    /// Borrow the underlying writer.
    pub const fn writer(&self) -> &LognormalMarketWriter<&P, A> {
        &self.writer
    }

    /// Contract address.
    pub const fn address(&self) -> Felt {
        self.writer.reader().address()
    }

    /// Execute a previously-prepared trade (caller supplies a full
    /// [`LognormalTradeInput`] — typically constructed from the
    /// [`LognormalVerifiedMinimum`]).
    pub async fn execute_trade(&self, input: LognormalTradeInput) -> SdkResult<ExecutionReceipt> {
        Ok(self.writer.execute_trade(input).await?)
    }

    /// Submit a guarded sell. ABI:
    /// `sell_position_guarded(candidate, x_star, candidate_hints, guards)`.
    pub async fn sell_position_guarded(
        &self,
        candidate: LognormalDistributionRaw,
        x_star: Sq128Raw,
        candidate_hints: LognormalSqrtHintsRaw,
        guards: &LognormalSellExecutionGuardsRaw,
    ) -> SdkResult<ExecutionReceipt> {
        Ok(self
            .writer
            .sell_position_guarded(candidate, x_star, candidate_hints, guards)
            .await?)
    }

    /// Claim the caller's settled position.
    pub async fn claim(&self) -> SdkResult<ExecutionReceipt> {
        Ok(self.writer.claim().await?)
    }

    /// Claim a settled position on behalf of `trader`.
    pub async fn claim_for(&self, trader: Felt) -> SdkResult<ExecutionReceipt> {
        Ok(self.writer.claim_for(trader).await?)
    }

    /// Add liquidity to the lognormal pool. ABI takes `share_amount` only.
    pub async fn add_liquidity(&self, share_amount: Sq128Raw) -> SdkResult<ExecutionReceipt> {
        Ok(self.writer.add_liquidity(share_amount).await?)
    }

    /// Remove a fraction of the caller's liquidity. ABI takes
    /// `share_amount` only.
    pub async fn remove_liquidity(&self, share_amount: Sq128Raw) -> SdkResult<ExecutionReceipt> {
        Ok(self.writer.remove_liquidity(share_amount).await?)
    }
}
