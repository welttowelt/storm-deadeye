//! View client for the Deadeye oracle extension.

use deadeye_core::sq128::Sq128Raw;
use starknet_core::types::{Felt, FunctionCall};
use tracing::instrument;

use crate::{
    cairo_serde::CairoSerde,
    error::{ContractError, ContractResult},
    provider::Provider,
    selectors::oracle as o,
    types::oracle::{MarketKey, SnapshotRaw},
};

/// Typed read client for the oracle contract.
#[derive(Debug)]
pub struct OracleClient<P>
where
    P: Provider,
{
    provider: P,
    address: Felt,
}

impl<P> OracleClient<P>
where
    P: Provider,
{
    /// Bind to an oracle address.
    pub const fn new(provider: P, address: Felt) -> Self {
        Self { provider, address }
    }

    /// Contract address.
    pub const fn address(&self) -> Felt {
        self.address
    }

    /// Reads the AMM address backing this oracle key.
    #[instrument(skip(self, market_key), fields(oracle = %self.address))]
    pub async fn amm(&self, market_key: &MarketKey) -> ContractResult<Felt> {
        let calldata = market_key.to_calldata();
        self.call_view::<Felt>("get_amm", o::get_amm(), &calldata)
            .await
    }

    /// Reads the average mean over `[start, end]`.
    pub async fn average_mean_over_period(
        &self,
        market_key: &MarketKey,
        start: u64,
        end: u64,
    ) -> ContractResult<Sq128Raw> {
        let mut calldata = market_key.to_calldata();
        start.encode(&mut calldata);
        end.encode(&mut calldata);
        self.call_view::<Sq128Raw>(
            "get_average_mean_over_period",
            o::get_average_mean_over_period(),
            &calldata,
        )
        .await
    }

    /// Reads the average mean over the trailing `period_seconds`.
    pub async fn average_mean_over_last(
        &self,
        market_key: &MarketKey,
        period_seconds: u64,
    ) -> ContractResult<Sq128Raw> {
        let mut calldata = market_key.to_calldata();
        period_seconds.encode(&mut calldata);
        self.call_view::<Sq128Raw>(
            "get_average_mean_over_last",
            o::get_average_mean_over_last(),
            &calldata,
        )
        .await
    }

    /// Reads the average variance over `[start, end]`.
    pub async fn average_variance_over_period(
        &self,
        market_key: &MarketKey,
        start: u64,
        end: u64,
    ) -> ContractResult<Sq128Raw> {
        let mut calldata = market_key.to_calldata();
        start.encode(&mut calldata);
        end.encode(&mut calldata);
        self.call_view::<Sq128Raw>(
            "get_average_variance_over_period",
            o::get_average_variance_over_period(),
            &calldata,
        )
        .await
    }

    /// Reads the snapshot count for a market key.
    pub async fn snapshot_count(&self, market_key: &MarketKey) -> ContractResult<u64> {
        let calldata = market_key.to_calldata();
        self.call_view::<u64>("get_snapshot_count", o::get_snapshot_count(), &calldata)
            .await
    }

    /// Reads a single snapshot at `index`.
    pub async fn snapshot(
        &self,
        market_key: &MarketKey,
        index: u64,
    ) -> ContractResult<SnapshotRaw> {
        let mut calldata = market_key.to_calldata();
        index.encode(&mut calldata);
        self.call_view::<SnapshotRaw>("get_snapshot", o::get_snapshot(), &calldata)
            .await
    }

    /// Reads the earliest observation timestamp.
    pub async fn earliest_observation_time(&self, market_key: &MarketKey) -> ContractResult<u64> {
        let calldata = market_key.to_calldata();
        self.call_view::<u64>(
            "get_earliest_observation_time",
            o::get_earliest_observation_time(),
            &calldata,
        )
        .await
    }

    /// Reads the latest observation timestamp.
    pub async fn latest_observation_time(&self, market_key: &MarketKey) -> ContractResult<u64> {
        let calldata = market_key.to_calldata();
        self.call_view::<u64>(
            "get_latest_observation_time",
            o::get_latest_observation_time(),
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
