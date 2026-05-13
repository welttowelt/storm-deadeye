//! Parallel fan-out reader for multi-market / multi-trader queries.
//!
//! The naive way to read 10 positions × 5 markets is 50 sequential RPC
//! calls — at 50 ms RTT that's 2.5 s of wall-clock latency for a single
//! snapshot. A market maker tracking 200 wallets across 30 markets
//! would be eternally stale.
//!
//! [`BulkReader`] wraps a [`DeadeyeClient`] and exposes vector versions
//! of every common read path. Each entry in the input slice fans out to
//! its own `Provider::call`; the futures are joined concurrently via
//! [`futures::future::join_all`]. Wall-clock latency converges on the
//! single-call RTT plus a small per-call overhead.
//!
//! The provider passed to `DeadeyeClient::new` is the only concurrency
//! constraint — point it at a `MultiRpcProvider` and the SDK
//! automatically rotates the load across endpoints.

use deadeye_core::{
    Distribution,
    bivariate::BivariateNormalDistributionRaw,
    categorical::CategoricalDistributionRaw,
    distribution::{LognormalDistributionRaw, NormalDistributionRaw},
};
use deadeye_starknet::{
    BivariateMarketReader, ContractError, ContractResult, Felt, LognormalMarketReader,
    MultinoulliMarketReader, NormalMarketReader, Provider,
    types::{
        bivariate::BivariateNormalPositionCompactRaw, common::LpInfoRaw,
        lognormal::LognormalPositionCompactRaw, multinoulli::MultinoulliPositionCompactRaw,
        normal::PositionCompactRaw,
    },
};
use futures::future::join_all;

use crate::client::DeadeyeClient;

/// Logical AMM family.
///
/// Used as a tag in the `(family, market, trader)` tuples the
/// [`BulkReader`] consumes so a single call can fan out across all four
/// market families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    /// Normal (Gaussian) AMM.
    Normal,
    /// Lognormal AMM.
    Lognormal,
    /// Multinoulli (categorical) AMM.
    Multinoulli,
    /// Bivariate-normal AMM.
    Bivariate,
}

/// Per-family compact-position record returned by [`BulkReader::positions`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Position {
    /// Normal-family position.
    Normal(PositionCompactRaw),
    /// Lognormal-family position.
    Lognormal(LognormalPositionCompactRaw),
    /// Multinoulli-family position.
    Multinoulli(MultinoulliPositionCompactRaw),
    /// Bivariate-family position.
    Bivariate(BivariateNormalPositionCompactRaw),
}

/// Per-family market distribution snapshot.
///
/// `Bivariate` is boxed because [`BivariateNormalDistributionRaw`]
/// carries nine `Sq128Raw` fields (~ 296 bytes), an order of magnitude
/// larger than the other variants. Without indirection every
/// [`DistributionSnapshot`] would pay the bivariate-sized tax in memory.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DistributionSnapshot {
    /// Normal market distribution.
    Normal(NormalDistributionRaw),
    /// Lognormal market distribution.
    Lognormal(LognormalDistributionRaw),
    /// Multinoulli market distribution.
    Multinoulli(CategoricalDistributionRaw),
    /// Bivariate-normal market distribution. Boxed to keep the enum
    /// payload from growing to the bivariate's footprint.
    Bivariate(Box<BivariateNormalDistributionRaw>),
}

/// Top-level market-state snapshot — what every "market tape" loop
/// reads once per tick.
///
/// The struct is intentionally an opt-in bundle of the most common
/// reads; callers that need only one shape should use the per-family
/// reader directly. Every field is `Option` so a partial failure on
/// one of the underlying reads doesn't poison the whole snapshot.
#[derive(Debug, Clone)]
pub struct MarketStateSnapshot {
    /// Family that owns this market.
    pub family: Family,
    /// Market address.
    pub market: Felt,
    /// Snapshot of the current distribution (None on RPC failure).
    pub distribution: Option<DistributionSnapshot>,
    /// LP pool summary.
    pub lp_info: Option<LpInfoRaw>,
}

/// Parallel fan-out reader over a [`DeadeyeClient`].
///
/// Owns the client by value so each [`BulkReader`] has a stable
/// provider handle — instantiate one per (provider, MM strategy) pair.
#[derive(Debug)]
pub struct BulkReader<P>
where
    P: Provider,
{
    client: DeadeyeClient<P>,
}

impl<P> BulkReader<P>
where
    P: Provider,
{
    /// Construct from an existing [`DeadeyeClient`].
    pub fn new(client: DeadeyeClient<P>) -> Self {
        Self { client }
    }

    /// Borrow the inner client.
    pub fn client(&self) -> &DeadeyeClient<P> {
        &self.client
    }

    /// Consume and return the inner client.
    pub fn into_client(self) -> DeadeyeClient<P> {
        self.client
    }

    /// Fetch every `(family, market, trader)` position concurrently.
    ///
    /// Each result is `Ok` / `Err` independently — a single bad query
    /// does not poison the whole batch.
    pub async fn positions(
        &self,
        queries: &[(Family, Felt, Felt)],
    ) -> Vec<ContractResult<Position>> {
        let provider = self.client.provider();
        let futures = queries.iter().map(|(family, market, trader)| {
            let market = *market;
            let trader = *trader;
            let family = *family;
            async move {
                match family {
                    Family::Normal => NormalMarketReader::new(provider, market)
                        .position(trader)
                        .await
                        .map(Position::Normal),
                    Family::Lognormal => LognormalMarketReader::new(provider, market)
                        .position(trader)
                        .await
                        .map(Position::Lognormal),
                    Family::Multinoulli => MultinoulliMarketReader::new(provider, market)
                        .position(trader)
                        .await
                        .map(Position::Multinoulli),
                    Family::Bivariate => BivariateMarketReader::new(provider, market)
                        .position(trader)
                        .await
                        .map(Position::Bivariate),
                }
            }
        });
        join_all(futures).await
    }

    /// Fetch every `(family, market)` LP pool summary concurrently.
    pub async fn lp_infos(&self, queries: &[(Family, Felt)]) -> Vec<ContractResult<LpInfoRaw>> {
        let provider = self.client.provider();
        let futures = queries.iter().map(|(family, market)| {
            let market = *market;
            let family = *family;
            async move {
                match family {
                    Family::Normal => NormalMarketReader::new(provider, market).lp_info().await,
                    Family::Lognormal => {
                        LognormalMarketReader::new(provider, market).lp_info().await
                    },
                    Family::Multinoulli => {
                        MultinoulliMarketReader::new(provider, market)
                            .lp_info()
                            .await
                    },
                    Family::Bivariate => {
                        BivariateMarketReader::new(provider, market).lp_info().await
                    },
                }
            }
        });
        join_all(futures).await
    }

    /// Fetch every `(family, market)` distribution concurrently.
    ///
    /// Each entry returns the *raw* distribution (`Distribution::to_raw`
    /// equivalent) so the caller can pick whether to materialise the
    /// f64-projected form per market.
    pub async fn distributions(
        &self,
        queries: &[(Family, Felt)],
    ) -> Vec<ContractResult<DistributionSnapshot>> {
        let provider = self.client.provider();
        let futures = queries.iter().map(|(family, market)| {
            let market = *market;
            let family = *family;
            async move {
                match family {
                    Family::Normal => NormalMarketReader::new(provider, market)
                        .distribution()
                        .await
                        .map(|d| DistributionSnapshot::Normal(d.to_raw())),
                    Family::Lognormal => LognormalMarketReader::new(provider, market)
                        .distribution()
                        .await
                        .map(|d| DistributionSnapshot::Lognormal(d.to_raw())),
                    Family::Multinoulli => MultinoulliMarketReader::new(provider, market)
                        .distribution()
                        .await
                        .and_then(|d| {
                            d.to_raw()
                                .map(DistributionSnapshot::Multinoulli)
                                .map_err(ContractError::Core)
                        }),
                    Family::Bivariate => BivariateMarketReader::new(provider, market)
                        .distribution()
                        .await
                        .and_then(|d| {
                            d.to_raw()
                                .map(|raw| DistributionSnapshot::Bivariate(Box::new(raw)))
                                .map_err(ContractError::Core)
                        }),
                }
            }
        });
        join_all(futures).await
    }

    /// Fetch a combined market-state snapshot for every `(family, market)`.
    ///
    /// Two sub-reads are dispatched per query (distribution + `lp_info`)
    /// and merged into a single [`MarketStateSnapshot`]. The whole batch
    /// runs concurrently so wall-clock latency converges on `2× RTT`
    /// for any batch size that fits inside the provider's connection
    /// pool.
    pub async fn market_states(&self, queries: &[(Family, Felt)]) -> Vec<MarketStateSnapshot> {
        let dist_fut = self.distributions(queries);
        let lp_fut = self.lp_infos(queries);
        let (dists, lps) = futures::future::join(dist_fut, lp_fut).await;
        queries
            .iter()
            .zip(dists)
            .zip(lps)
            .map(|(((family, market), dist), lp)| MarketStateSnapshot {
                family: *family,
                market: *market,
                distribution: dist.ok(),
                lp_info: lp.ok(),
            })
            .collect()
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use deadeye_core::sq128::Sq128Raw;
    use deadeye_starknet::CairoSerde;
    use starknet_core::types::{BlockId, FunctionCall};

    use super::*;

    #[derive(Debug, Default)]
    struct ConcurrencyProvider {
        calls: AtomicUsize,
        in_flight: AtomicUsize,
        peak_in_flight: Mutex<usize>,
    }

    #[async_trait]
    impl Provider for ConcurrencyProvider {
        async fn call(&self, _call: FunctionCall, _block: BlockId) -> ContractResult<Vec<Felt>> {
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            {
                let mut peak = self.peak_in_flight.lock().unwrap();
                if now > *peak {
                    *peak = now;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            // Return a minimal LpInfoRaw (10 felts = 2× Sq128Raw).
            let lp = LpInfoRaw {
                total_shares: Sq128Raw {
                    limb0: 0,
                    limb1: 0,
                    limb2: 7,
                    limb3: 0,
                    neg: false,
                },
                total_backing_deposited: Sq128Raw {
                    limb0: 0,
                    limb1: 0,
                    limb2: 7,
                    limb3: 0,
                    neg: false,
                },
            };
            Ok(lp.to_calldata())
        }
    }

    #[tokio::test]
    async fn lp_infos_run_concurrently() {
        let provider = ConcurrencyProvider::default();
        let client = DeadeyeClient::new(provider);
        let bulk = BulkReader::new(client);
        let queries = (0..16)
            .map(|i| (Family::Normal, Felt::from(i as u64)))
            .collect::<Vec<_>>();
        let results = bulk.lp_infos(&queries).await;
        assert_eq!(results.len(), 16);
        for r in &results {
            assert!(r.is_ok());
        }
        let peak = *bulk.client().provider().peak_in_flight.lock().unwrap();
        assert!(
            peak >= 4,
            "expected concurrent fan-out, peak in-flight was {peak}"
        );
    }

    #[tokio::test]
    async fn family_tag_is_carried_through() {
        let provider = ConcurrencyProvider::default();
        let client = DeadeyeClient::new(provider);
        let bulk = BulkReader::new(client);
        let queries = vec![
            (Family::Normal, Felt::from(0x1_u64)),
            (Family::Bivariate, Felt::from(0x2_u64)),
        ];
        let snaps = bulk.market_states(&queries).await;
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].family, Family::Normal);
        assert_eq!(snaps[1].family, Family::Bivariate);
    }
}
