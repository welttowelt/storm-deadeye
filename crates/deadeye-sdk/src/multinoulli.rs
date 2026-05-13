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
//! use deadeye_sdk::starknet::{
//!     Felt, JsonRpcProvider, MultinoulliMarketReader, MultinoulliMarketWriter, OwnedAccount,
//! };
//! use deadeye_sdk::core::{categorical::CategoricalDistributionRaw, sq128::Sq128Raw};
//! use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let rpc = JsonRpcClient::new(HttpTransport::new("http://localhost:5050".parse::<url::Url>()?));
//! let provider = JsonRpcProvider::new(rpc);
//! let (market, runtime): (Felt, Felt) = (Felt::ZERO, Felt::ZERO);
//! let reader = MultinoulliMarketReader::new(&provider, market);
//! let signer = OwnedAccount::from_signing_key(
//!     JsonRpcClient::new(HttpTransport::new("http://localhost:5050".parse::<url::Url>()?)),
//!     Felt::ZERO, Felt::ZERO, Felt::ZERO,
//! );
//! let writer = MultinoulliMarketWriter::new(reader, signer);
//!
//! let candidate = CategoricalDistributionRaw { probs: vec![Sq128Raw::ZERO; 4] };
//! let quote = writer.reader().quote_trade(runtime, candidate, 0, Sq128Raw::ZERO).await?;
//! writer.execute_quote(quote).await?;
//! writer.sell_position(0).await?;
//! # Ok(()) }
//! ```

use deadeye_collateral::{CategoricalVerifiedMinimum, categorical_collateral};
use deadeye_core::CategoricalDistribution;
use deadeye_starknet::{
    Account, ExecutionReceipt, Felt, MultinoulliMarketReader, MultinoulliMarketWriter, Provider,
};
use tracing::instrument;

use crate::error::SdkResult;

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
