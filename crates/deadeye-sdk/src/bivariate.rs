//! Bivariate-normal AMM market handle.
//!
//! Two correlated Gaussians (μ₁, μ₂, σ₁², σ₂², ρ). The constructor's
//! hint check requires byte-exact Sq128 derivations of σ₁, σ₂,
//! `1/(1−ρ²)`, and the joint normalization — see
//! `docs/DEVNET_SHAKEDOWN.md` for the gotcha. The writer's
//! `quote_trade` runs `expand_distribution_core_view` first so f64
//! inputs are safely promoted to chain-exact full distributions.
//!
//! ## Worked example
//!
//! ```no_run
//! use deadeye_sdk::starknet::{
//!     BivariateMarketReader, BivariateMarketWriter, Felt, JsonRpcProvider, OwnedAccount,
//! };
//! use deadeye_sdk::core::{
//!     bivariate::{BivariateNormalDistributionCoreRaw, BivariatePointRaw},
//!     sq128::Sq128Raw,
//! };
//! use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let rpc = JsonRpcClient::new(HttpTransport::new("http://localhost:5050".parse::<url::Url>()?));
//! let provider = JsonRpcProvider::new(rpc);
//! let (market, runtime): (Felt, Felt) = (Felt::ZERO, Felt::ZERO);
//! let reader = BivariateMarketReader::new(&provider, market);
//! let signer = OwnedAccount::from_signing_key(
//!     JsonRpcClient::new(HttpTransport::new("http://localhost:5050".parse::<url::Url>()?)),
//!     Felt::ZERO, Felt::ZERO, Felt::ZERO,
//! );
//! let writer = BivariateMarketWriter::new(reader, signer);
//!
//! let core = BivariateNormalDistributionCoreRaw {
//!     mu1: Sq128Raw::ZERO, mu2: Sq128Raw::ZERO,
//!     variance1: Sq128Raw::ZERO, variance2: Sq128Raw::ZERO,
//!     rho: Sq128Raw::ZERO,
//! };
//! let x_star = BivariatePointRaw { x1: Sq128Raw::ZERO, x2: Sq128Raw::ZERO };
//! let quote = writer.reader()
//!     .quote_trade(runtime, core, x_star, Sq128Raw::ZERO)
//!     .await?;
//! writer.execute_quote(quote).await?;
//! writer.sell_position(runtime, 0).await?;
//! # Ok(()) }
//! ```

use deadeye_collateral::{BivariateOptions, BivariateVerifiedMinimum, bivariate_collateral};
use deadeye_core::{
    BivariateNormalDistribution,
    bivariate::{BivariateNormalDistributionRaw, BivariateNormalSqrtHintsRaw, BivariatePointRaw},
    sq128::Sq128Raw,
};
use deadeye_starknet::{
    Account, BivariateMarketReader, BivariateMarketWriter, ExecutionReceipt, Felt, Provider,
    types::bivariate::{BivariateNormalSellExecutionGuardsRaw, BivariateTradeInput},
};
use tracing::instrument;

use crate::error::SdkResult;

/// Handle to a deployed bivariate AMM market.
#[derive(Debug)]
pub struct BivariateMarket<'p, P>
where
    P: Provider,
{
    reader: BivariateMarketReader<&'p P>,
}

impl<'p, P> BivariateMarket<'p, P>
where
    P: Provider,
{
    /// Construct a handle.
    pub fn new(provider: &'p P, address: Felt) -> Self {
        Self {
            reader: BivariateMarketReader::new(provider, address),
        }
    }

    /// Underlying read-only reader.
    pub const fn reader(&self) -> &BivariateMarketReader<&'p P> {
        &self.reader
    }

    /// Contract address.
    pub const fn address(&self) -> Felt {
        self.reader.address()
    }

    /// Reads the current market distribution.
    pub async fn distribution(&self) -> SdkResult<BivariateNormalDistribution> {
        Ok(self.reader.distribution().await?)
    }

    /// Prepares an off-chain quote for moving the market from its current
    /// state to `candidate`.
    #[instrument(skip(self, candidate), fields(market = %self.reader.address()))]
    pub async fn prepare_quote(
        &self,
        candidate: &BivariateNormalDistribution,
        opts: BivariateOptions,
    ) -> SdkResult<BivariateVerifiedMinimum> {
        let current = self.distribution().await?;
        Ok(bivariate_collateral(&current, candidate, opts)?)
    }

    /// Bind an account for writes.
    pub fn with_account<A>(self, account: A) -> BivariateMarketSigned<'p, P, A>
    where
        A: Account,
    {
        BivariateMarketSigned {
            writer: BivariateMarketWriter::new(self.reader, account),
        }
    }
}

/// Account-bound companion.
#[derive(Debug)]
pub struct BivariateMarketSigned<'p, P, A>
where
    P: Provider,
    A: Account,
{
    writer: BivariateMarketWriter<&'p P, A>,
}

impl<P, A> BivariateMarketSigned<'_, P, A>
where
    P: Provider,
    A: Account,
{
    /// Borrow the underlying writer.
    pub const fn writer(&self) -> &BivariateMarketWriter<&P, A> {
        &self.writer
    }

    /// Contract address.
    pub const fn address(&self) -> Felt {
        self.writer.reader().address()
    }

    /// Execute a trade.
    pub async fn execute_trade(&self, input: BivariateTradeInput) -> SdkResult<ExecutionReceipt> {
        Ok(self.writer.execute_trade(input).await?)
    }

    /// Submit a guarded sell. ABI:
    /// `sell_position_guarded(candidate, x_star, candidate_hints, guards)`.
    pub async fn sell_position_guarded(
        &self,
        candidate: BivariateNormalDistributionRaw,
        x_star: BivariatePointRaw,
        candidate_hints: BivariateNormalSqrtHintsRaw,
        guards: BivariateNormalSellExecutionGuardsRaw,
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

    /// Add liquidity to the bivariate pool. ABI takes `share_amount` only.
    pub async fn add_liquidity(&self, share_amount: Sq128Raw) -> SdkResult<ExecutionReceipt> {
        Ok(self.writer.add_liquidity(share_amount).await?)
    }

    /// Remove a fraction of the caller's liquidity. ABI takes
    /// `share_amount` only.
    pub async fn remove_liquidity(&self, share_amount: Sq128Raw) -> SdkResult<ExecutionReceipt> {
        Ok(self.writer.remove_liquidity(share_amount).await?)
    }

    /// Settle the bivariate market at `settlement_point`.
    pub async fn settle(&self, settlement_point: BivariatePointRaw) -> SdkResult<ExecutionReceipt> {
        Ok(self.writer.settle(settlement_point).await?)
    }
}
