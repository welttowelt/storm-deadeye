//! View + write clients for the Deadeye distribution factory.
//!
//! The factory owns every deployed market — `pause_market`, every
//! `settle_*`, and `collect_protocol_fees` route through it. The
//! [`FactoryWriter`] exposes a typed wrapper per Cairo entrypoint;
//! every wrapper returns a [`crate::TradeResult`] so callers can branch
//! on [`crate::TradeRejectionReason::OnlyOwner`] /
//! [`crate::TradeRejectionReason::MarketSettled`] / etc. without
//! parsing revert text by hand.
//!
//! ## Worked example — deploy + settle + collect fees
//!
//! ```no_run
//! use deadeye_core::sq128::Sq128Raw;
//! use deadeye_starknet::{FactoryReader, FactoryWriter, Felt, JsonRpcProvider, OwnedAccount};
//! use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let rpc = JsonRpcClient::new(HttpTransport::new(
//!     "http://localhost:5050".parse::<url::Url>()?,
//! ));
//! let provider = JsonRpcProvider::new(rpc);
//! let (factory, market): (Felt, Felt) = (Felt::ZERO, Felt::ZERO);
//!
//! let admin = OwnedAccount::from_signing_key(
//!     JsonRpcClient::new(HttpTransport::new(
//!         "http://localhost:5050".parse::<url::Url>()?,
//!     )),
//!     Felt::ZERO,
//!     Felt::ZERO,
//!     Felt::ZERO,
//! );
//! let writer = FactoryWriter::new(FactoryReader::new(provider, factory), admin);
//!
//! // 1) (Declare contracts via deadeye-deployer — out of scope for this example.)
//! //
//! // 2) Deploy a market from an installed profile.
//! //    let _ = writer.deploy_normal(&input).await?;
//!
//! // 3) Settle the market at x*.
//! let x_star = Sq128Raw::ZERO;
//! writer.settle_normal_market(market, x_star).await?;
//!
//! // 4) Collect accrued protocol fees into the factory treasury.
//! writer
//!     .collect_protocol_fees(market, /* recipient= */ Felt::ZERO)
//!     .await?;
//! # Ok(()) }
//! ```

use deadeye_core::bivariate::BivariatePointRaw;
use starknet_core::types::{Felt, FunctionCall};
use tracing::instrument;

use crate::{
    account::Account,
    cairo_serde::CairoSerde,
    error::{ContractError, ContractResult, TradeError, TradeResult},
    execution::{Call, ExecutionReceipt},
    provider::Provider,
    selectors::factory as f,
    types::factory::{
        DeployBivariateNormalMarketFromProfileInput, DeployLognormalMarketFromProfileInput,
        DeployMultinoulliMarketFromProfileInput, DeployNormalMarketFromProfileInput,
        FactoryBatchOpResultRaw, MarketDeployProfileRaw, MarketTypeConfigRaw,
    },
};

/// Typed reader for the factory contract.
#[derive(Debug)]
pub struct FactoryReader<P>
where
    P: Provider,
{
    provider: P,
    address: Felt,
}

impl<P> FactoryReader<P>
where
    P: Provider,
{
    /// Bind to a factory address.
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

    /// Reads the factory owner.
    #[instrument(skip(self), fields(factory = %self.address))]
    pub async fn owner(&self) -> ContractResult<Felt> {
        self.call_view::<Felt>("get_owner", f::get_owner(), &[])
            .await
    }

    /// Reads the factory treasury.
    pub async fn treasury(&self) -> ContractResult<Felt> {
        self.call_view::<Felt>("get_treasury", f::get_treasury(), &[])
            .await
    }

    /// Total markets deployed by this factory.
    pub async fn market_count(&self) -> ContractResult<u64> {
        self.call_view::<u64>("get_market_count", f::get_market_count(), &[])
            .await
    }

    /// Returns the market address at zero-based `index`.
    pub async fn market_at(&self, index: u64) -> ContractResult<Felt> {
        let mut calldata = Vec::with_capacity(1);
        index.encode(&mut calldata);
        self.call_view::<Felt>("get_market_at", f::get_market_at(), &calldata)
            .await
    }

    /// Returns the market-type discriminant (`u8`) associated with `market`.
    pub async fn market_type_for_market(&self, market: Felt) -> ContractResult<u8> {
        self.call_view::<u8>(
            "get_market_type_for_market",
            f::get_market_type_for_market(),
            &[market],
        )
        .await
    }

    /// Returns whether the deployment profile is currently enabled.
    pub async fn is_profile_enabled(&self, profile_id: u32) -> ContractResult<bool> {
        let mut calldata = Vec::with_capacity(1);
        profile_id.encode(&mut calldata);
        self.call_view::<bool>("is_profile_enabled", f::is_profile_enabled(), &calldata)
            .await
    }

    /// Reads a deploy profile.
    pub async fn deploy_profile(&self, profile_id: u32) -> ContractResult<MarketDeployProfileRaw> {
        let mut calldata = Vec::with_capacity(1);
        profile_id.encode(&mut calldata);
        self.call_view::<MarketDeployProfileRaw>(
            "get_deploy_profile",
            f::get_deploy_profile(),
            &calldata,
        )
        .await
    }

    /// Reads the per-market-type configuration.
    pub async fn market_type_config(&self, market_type: u8) -> ContractResult<MarketTypeConfigRaw> {
        let mut calldata = Vec::with_capacity(1);
        market_type.encode(&mut calldata);
        self.call_view::<MarketTypeConfigRaw>(
            "get_market_type_config",
            f::get_market_type_config(),
            &calldata,
        )
        .await
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

/// Write-capable companion.
#[derive(Debug)]
pub struct FactoryWriter<P, A>
where
    P: Provider,
    A: Account,
{
    reader: FactoryReader<P>,
    account: A,
}

impl<P, A> FactoryWriter<P, A>
where
    P: Provider,
    A: Account,
{
    /// Pair a reader with an account.
    pub const fn new(reader: FactoryReader<P>, account: A) -> Self {
        Self { reader, account }
    }

    /// Borrow the reader.
    pub const fn reader(&self) -> &FactoryReader<P> {
        &self.reader
    }

    /// Borrow the account.
    pub const fn account(&self) -> &A {
        &self.account
    }

    /// Build a Call for `deploy_normal_market_from_profile`.
    pub fn build_deploy_normal_call(&self, input: &DeployNormalMarketFromProfileInput) -> Call {
        Call {
            to: self.reader.address(),
            selector: f::deploy_normal_market_from_profile(),
            calldata: input.to_calldata(),
        }
    }

    /// Build a Call for `deploy_lognormal_market_from_profile`.
    pub fn build_deploy_lognormal_call(
        &self,
        input: &DeployLognormalMarketFromProfileInput,
    ) -> Call {
        Call {
            to: self.reader.address(),
            selector: f::deploy_lognormal_market_from_profile(),
            calldata: input.to_calldata(),
        }
    }

    /// Build a Call for `deploy_bivariate_normal_market_from_profile`.
    pub fn build_deploy_bivariate_call(
        &self,
        input: &DeployBivariateNormalMarketFromProfileInput,
    ) -> Call {
        Call {
            to: self.reader.address(),
            selector: f::deploy_bivariate_normal_market_from_profile(),
            calldata: input.to_calldata(),
        }
    }

    /// Build a Call for `deploy_multinoulli_market_from_profile`.
    pub fn build_deploy_multinoulli_call(
        &self,
        input: &DeployMultinoulliMarketFromProfileInput,
    ) -> Call {
        Call {
            to: self.reader.address(),
            selector: f::deploy_multinoulli_market_from_profile(),
            calldata: input.to_calldata(),
        }
    }

    /// Build a Call to batch-settle a slice of normal markets (best-effort —
    /// per-market status is reported but the transaction succeeds even when
    /// some markets fail).
    pub fn build_settle_normal_best_effort_call(
        &self,
        markets: &[Felt],
        settlement_value: deadeye_core::sq128::Sq128Raw,
    ) -> Call {
        let mut calldata = Vec::with_capacity(markets.len() + 6);
        markets.to_vec().encode(&mut calldata);
        settlement_value.encode(&mut calldata);
        Call {
            to: self.reader.address(),
            selector: f::settle_normal_markets_best_effort(),
            calldata,
        }
    }

    /// Build a Call for the strict (all-or-revert) variant.
    pub fn build_settle_normal_strict_call(
        &self,
        markets: &[Felt],
        settlement_value: deadeye_core::sq128::Sq128Raw,
    ) -> Call {
        let mut calldata = Vec::with_capacity(markets.len() + 6);
        markets.to_vec().encode(&mut calldata);
        settlement_value.encode(&mut calldata);
        Call {
            to: self.reader.address(),
            selector: f::settle_normal_markets_strict(),
            calldata,
        }
    }

    /// Build a Call for `pause_market`.
    pub fn build_pause_market_call(&self, market: Felt) -> Call {
        Call {
            to: self.reader.address(),
            selector: f::pause_market(),
            calldata: vec![market],
        }
    }

    /// Build a Call for `unpause_market`.
    pub fn build_unpause_market_call(&self, market: Felt) -> Call {
        Call {
            to: self.reader.address(),
            selector: f::unpause_market(),
            calldata: vec![market],
        }
    }

    /// Submit a single normal-market deployment.
    pub async fn deploy_normal(
        &self,
        input: &DeployNormalMarketFromProfileInput,
    ) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_deploy_normal_call(input)])
            .await
    }

    /// Submit a single lognormal-market deployment.
    pub async fn deploy_lognormal(
        &self,
        input: &DeployLognormalMarketFromProfileInput,
    ) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_deploy_lognormal_call(input)])
            .await
    }

    /// Submit a single bivariate-market deployment.
    pub async fn deploy_bivariate(
        &self,
        input: &DeployBivariateNormalMarketFromProfileInput,
    ) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_deploy_bivariate_call(input)])
            .await
    }

    /// Submit a single multinoulli-market deployment.
    pub async fn deploy_multinoulli(
        &self,
        input: &DeployMultinoulliMarketFromProfileInput,
    ) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_deploy_multinoulli_call(input)])
            .await
    }

    /// Submit a best-effort batch settlement for normal markets. The chain
    /// returns per-market statuses; callers can fetch them via the
    /// transaction receipt's emitted `FactoryBatchOpResultRaw` events.
    pub async fn settle_normal_markets_best_effort(
        &self,
        markets: &[Felt],
        settlement_value: deadeye_core::sq128::Sq128Raw,
    ) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![
                self.build_settle_normal_best_effort_call(markets, settlement_value),
            ])
            .await
    }

    /// Pause a market.
    pub async fn pause_market(&self, market: Felt) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_pause_market_call(market)])
            .await
    }

    /// Unpause a market.
    pub async fn unpause_market(&self, market: Felt) -> ContractResult<ExecutionReceipt> {
        self.account
            .execute(vec![self.build_unpause_market_call(market)])
            .await
    }

    // ─── Typed admin helpers ─────────────────────────────────────────────────
    //
    // Every wrapper below uses the strict or best-effort batch entrypoint
    // appropriate to the family, returns a [`TradeResult`] so callers can
    // branch on `TradeRejectionReason::OnlyOwner` / `MarketSettled` / etc.
    // without parsing revert text by hand, and accepts the bare per-family
    // settlement payload (no manual calldata construction).

    /// Settle a batch of **normal** markets at `x_star`.
    ///
    /// `strict = true` reverts the whole transaction if any market fails;
    /// `false` returns per-market statuses via emitted events.
    pub async fn settle_normal(
        &self,
        markets: &[Felt],
        x_star: deadeye_core::sq128::Sq128Raw,
        strict: bool,
    ) -> TradeResult<ExecutionReceipt> {
        let selector = if strict {
            f::settle_normal_markets_strict()
        } else {
            f::settle_normal_markets_best_effort()
        };
        let mut calldata = Vec::with_capacity(markets.len() + 6);
        markets.to_vec().encode(&mut calldata);
        x_star.encode(&mut calldata);
        self.dispatch(selector, calldata).await
    }

    /// Settle a batch of **lognormal** markets at `x_star`.
    pub async fn settle_lognormal(
        &self,
        markets: &[Felt],
        x_star: deadeye_core::sq128::Sq128Raw,
        strict: bool,
    ) -> TradeResult<ExecutionReceipt> {
        let selector = if strict {
            f::settle_lognormal_markets_strict()
        } else {
            f::settle_lognormal_markets_best_effort()
        };
        let mut calldata = Vec::with_capacity(markets.len() + 6);
        markets.to_vec().encode(&mut calldata);
        x_star.encode(&mut calldata);
        self.dispatch(selector, calldata).await
    }

    /// Settle a batch of **multinoulli** markets at the same outcome
    /// index.
    pub async fn settle_multinoulli(
        &self,
        markets: &[Felt],
        outcome: u32,
        strict: bool,
    ) -> TradeResult<ExecutionReceipt> {
        let selector = if strict {
            f::settle_multinoulli_markets_strict()
        } else {
            f::settle_multinoulli_markets_best_effort()
        };
        let mut calldata = Vec::with_capacity(markets.len() + 2);
        markets.to_vec().encode(&mut calldata);
        outcome.encode(&mut calldata);
        self.dispatch(selector, calldata).await
    }

    /// Settle a batch of **bivariate** markets at the same point.
    pub async fn settle_bivariate(
        &self,
        markets: &[Felt],
        point: BivariatePointRaw,
        strict: bool,
    ) -> TradeResult<ExecutionReceipt> {
        let selector = if strict {
            f::settle_bivariate_normal_markets_strict()
        } else {
            f::settle_bivariate_normal_markets_best_effort()
        };
        let mut calldata = Vec::with_capacity(markets.len() + 11);
        markets.to_vec().encode(&mut calldata);
        point.encode(&mut calldata);
        self.dispatch(selector, calldata).await
    }

    /// Settle a single normal market at `x_star`.
    pub async fn settle_normal_market(
        &self,
        market: Felt,
        x_star: deadeye_core::sq128::Sq128Raw,
    ) -> TradeResult<ExecutionReceipt> {
        let mut calldata = Vec::with_capacity(6);
        calldata.push(market);
        x_star.encode(&mut calldata);
        self.dispatch(f::settle_normal_market(), calldata).await
    }

    /// Settle a single lognormal market at `x_star`.
    pub async fn settle_lognormal_market(
        &self,
        market: Felt,
        x_star: deadeye_core::sq128::Sq128Raw,
    ) -> TradeResult<ExecutionReceipt> {
        let mut calldata = Vec::with_capacity(6);
        calldata.push(market);
        x_star.encode(&mut calldata);
        self.dispatch(f::settle_lognormal_market(), calldata).await
    }

    /// Settle a single multinoulli market at `outcome`.
    pub async fn settle_multinoulli_market(
        &self,
        market: Felt,
        outcome: u32,
    ) -> TradeResult<ExecutionReceipt> {
        let mut calldata = Vec::with_capacity(2);
        calldata.push(market);
        outcome.encode(&mut calldata);
        self.dispatch(f::settle_multinoulli_market(), calldata)
            .await
    }

    /// Settle a single bivariate market at `point`.
    pub async fn settle_bivariate_market(
        &self,
        market: Felt,
        point: BivariatePointRaw,
    ) -> TradeResult<ExecutionReceipt> {
        let mut calldata = Vec::with_capacity(11);
        calldata.push(market);
        point.encode(&mut calldata);
        self.dispatch(f::settle_bivariate_normal_market(), calldata)
            .await
    }

    /// Pause `market` via the factory (returns a typed error so callers
    /// can branch on [`crate::TradeRejectionReason::OnlyOwner`] etc.).
    pub async fn pause_market_typed(&self, market: Felt) -> TradeResult<ExecutionReceipt> {
        self.dispatch(f::pause_market(), vec![market]).await
    }

    /// Unpause `market` via the factory.
    pub async fn unpause_market_typed(&self, market: Felt) -> TradeResult<ExecutionReceipt> {
        self.dispatch(f::unpause_market(), vec![market]).await
    }

    /// Collect accrued protocol fees from `market` and forward them to
    /// the factory's treasury.
    ///
    /// Per `factory.abi.json`, the ABI is `collect_protocol_fees(market)`
    /// returning `u256` — the factory treasury is fixed at construction.
    /// The `_recipient` argument is accepted for API symmetry but
    /// ignored at the calldata layer (kept for future-proofing if the
    /// chain adds a recipient parameter).
    pub async fn collect_protocol_fees(
        &self,
        market: Felt,
        _recipient: Felt,
    ) -> TradeResult<ExecutionReceipt> {
        self.dispatch(f::collect_protocol_fees(), vec![market])
            .await
    }

    /// Internal: submit a one-call transaction and promote any revert
    /// reason into a [`TradeError`].
    async fn dispatch(&self, selector: Felt, calldata: Vec<Felt>) -> TradeResult<ExecutionReceipt> {
        let call = Call {
            to: self.reader.address(),
            selector,
            calldata,
        };
        match self.account.execute(vec![call]).await {
            Ok(receipt) => Ok(receipt),
            Err(err) => Err(TradeError::from_contract(err)),
        }
    }
}

// Convenience re-export so downstream tests can decode batch-result arrays.
pub use crate::types::factory::FactoryBatchOpResultRaw as BatchOpResult;

// Mark the import used.
const _: fn() = || {
    let _ = core::mem::size_of::<FactoryBatchOpResultRaw>();
};
