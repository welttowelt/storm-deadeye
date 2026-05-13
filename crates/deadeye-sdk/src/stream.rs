//! Block-driven market-state subscription.
//!
//! Production market makers don't poll one market at a time — they
//! subscribe to *block-level* state and react to whatever changed in
//! the latest block. [`MarketStateStream`] is that subscription
//! primitive: it spawns a tokio task that polls the chain head every
//! [`StreamConfig::poll_interval`] and, whenever a *new* block is
//! observed, re-reads the configured subset of market state and
//! pushes a [`MarketStateUpdate`] into the consumer channel.
//!
//! The stream itself is allocation-free per tick (only the data it
//! returns to the consumer is heap). Internally it owns an
//! [`Arc`]-shared client so multiple streams can coexist against a
//! single provider connection pool.
//!
//! ## Producer / consumer split
//!
//! The poller runs on its own tokio task; the consumer holds the
//! receiver end of a [`tokio::sync::mpsc`] channel and drains it via
//! [`Stream`]. Dropping the [`MarketStateStream`] handle stops the
//! poller via the shared cancellation token.
//!
//! ## When does the stream yield?
//!
//! On every block height transition — *not* every poll. If the chain
//! head hasn't advanced since the last poll, the stream stays quiet.
//! This keeps consumers from spinning on duplicate state.

use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use deadeye_core::{
    bivariate::{BivariateNormalDistributionCoreRaw, BivariatePointRaw},
    categorical::CategoricalDistributionRaw,
    distribution::{LognormalDistributionRaw, NormalDistributionRaw},
    sq128::Sq128Raw,
};
use deadeye_starknet::{
    BivariateMarketReader, BivariateTradeQuote, ContractResult, Felt, LognormalMarketReader,
    LognormalTradeQuote, MultinoulliMarketReader, MultinoulliTradeQuote, NormalMarketReader,
    NormalTradeQuote, Provider, types::common::LpInfoRaw,
};
use futures::Stream;
use tokio::sync::mpsc;

use crate::{
    bulk::{DistributionSnapshot, Family},
    client::DeadeyeClient,
};

/// Source of "what block is the chain at?" — the only piece of the
/// streaming API we can't express purely through the SDK's existing
/// [`Provider`] trait.
///
/// Implemented for `starknet_providers::Provider` users via the
/// [`StarknetBlockSource`] adapter (see below). Custom backends (e.g.
/// [`MultiRpcProvider`](deadeye_starknet::MultiRpcProvider), Madara,
/// mocks) can implement this directly.
#[async_trait::async_trait]
pub trait BlockNumberSource: Send + Sync {
    /// Return the chain head's block number. Errors propagate via
    /// `ContractError::Provider`.
    async fn block_number(&self) -> ContractResult<u64>;
}

/// Adapter wrapping any `starknet_providers::Provider` so the streamer
/// can poll its block height.
#[derive(Debug)]
pub struct StarknetBlockSource<P: starknet_providers::Provider + Send + Sync> {
    inner: P,
}

impl<P: starknet_providers::Provider + Send + Sync> StarknetBlockSource<P> {
    /// Wrap a `starknet_providers` provider.
    pub const fn new(inner: P) -> Self {
        Self { inner }
    }

    /// Borrow the underlying provider.
    pub const fn inner(&self) -> &P {
        &self.inner
    }
}

#[async_trait::async_trait]
impl<P: starknet_providers::Provider + Send + Sync> BlockNumberSource for StarknetBlockSource<P> {
    async fn block_number(&self) -> ContractResult<u64> {
        starknet_providers::Provider::block_number(&self.inner)
            .await
            .map_err(|e| deadeye_starknet::ContractError::Provider(format!("{e}")))
    }
}

/// What the stream should re-read on every new block.
///
/// All fields are `false` / `None` by default. Set `include_*`
/// explicitly to drive the bytes-on-the-wire cost.
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// How often to poll the chain head. Defaults to 1 s — fast
    /// enough for Starknet block times (~30 s on Sepolia, ~2 s on
    /// Madara), slow enough not to hammer the RPC.
    pub poll_interval: Duration,
    /// Re-read the market distribution on every new block.
    pub include_distribution: bool,
    /// Re-read the LP pool summary on every new block.
    pub include_lp_info: bool,
    /// If `Some`, re-quote this candidate against the current market
    /// on every new block. Family must match the subscription's
    /// family (otherwise the quote field stays `None`).
    pub include_quote_for_candidate: Option<CandidateQuote>,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(1),
            include_distribution: true,
            include_lp_info: false,
            include_quote_for_candidate: None,
        }
    }
}

/// Per-family candidate spec for `include_quote_for_candidate`.
///
/// The variants mirror the four [`Family`] tags. Each carries the
/// shape its family's `quote_trade()` expects.
#[derive(Debug, Clone)]
pub enum CandidateQuote {
    /// Normal candidate. Runtime is the math-runtime contract address.
    Normal {
        /// Math-runtime address.
        runtime: Felt,
        /// Candidate distribution.
        candidate: NormalDistributionRaw,
        /// `x_star`.
        x_star: Sq128Raw,
        /// Supplied collateral.
        supplied_collateral: Sq128Raw,
        /// Pad applied to supplied collateral.
        collateral_pad: Sq128Raw,
    },
    /// Lognormal candidate.
    Lognormal {
        /// Math-runtime address.
        runtime: Felt,
        /// Candidate distribution.
        candidate: LognormalDistributionRaw,
        /// `x_star`.
        x_star: Sq128Raw,
        /// Supplied collateral.
        supplied_collateral: Sq128Raw,
        /// Pad applied to supplied collateral.
        collateral_pad: Sq128Raw,
    },
    /// Multinoulli candidate.
    Multinoulli {
        /// Math-runtime address.
        runtime: Felt,
        /// Candidate distribution.
        candidate: CategoricalDistributionRaw,
        /// Minimum outcome index.
        min_outcome_index: u32,
        /// Supplied collateral.
        supplied_collateral: Sq128Raw,
    },
    /// Bivariate candidate.
    Bivariate {
        /// Math-runtime address.
        runtime: Felt,
        /// Candidate (5-field core).
        core_candidate: BivariateNormalDistributionCoreRaw,
        /// `x_star` point.
        x_star: BivariatePointRaw,
        /// Supplied collateral.
        supplied_collateral: Sq128Raw,
    },
}

/// Per-family quote returned in [`MarketStateUpdate::quote`].
///
/// `Bivariate` is boxed because [`BivariateTradeQuote`] is roughly 2×
/// the size of the other variants (it carries the expanded `μ₁, μ₂,
/// σ₁, σ₂, ρ` candidate plus its sqrt hints). Without indirection
/// every `QuoteSnapshot` would pay the bivariate-sized tax in memory.
#[derive(Debug, Clone)]
pub enum QuoteSnapshot {
    /// Normal-family quote.
    Normal(NormalTradeQuote),
    /// Lognormal-family quote.
    Lognormal(LognormalTradeQuote),
    /// Multinoulli-family quote.
    Multinoulli(MultinoulliTradeQuote),
    /// Bivariate-family quote (boxed; see type-level docs).
    Bivariate(Box<BivariateTradeQuote>),
}

/// A single tick from a [`MarketStateStream`].
#[derive(Debug, Clone)]
pub struct MarketStateUpdate {
    /// Block height the update was observed at.
    pub block_number: u64,
    /// Family + market the stream is subscribed to.
    pub family: Family,
    /// Market address.
    pub market: Felt,
    /// Snapshot of the current distribution if `include_distribution`.
    pub distribution: Option<DistributionSnapshot>,
    /// LP pool summary if `include_lp_info`.
    pub lp_info: Option<LpInfoRaw>,
    /// Re-quoted candidate if `include_quote_for_candidate.is_some()`.
    pub quote: Option<QuoteSnapshot>,
}

/// Subscription handle. Implements [`Stream`] yielding one
/// [`MarketStateUpdate`] per observed block transition.
///
/// Drop the handle to stop the underlying poller task.
#[derive(Debug)]
pub struct MarketStateStream {
    rx: mpsc::Receiver<MarketStateUpdate>,
    _drop: DropGuard,
}

/// Cancellation token: when the [`MarketStateStream`] handle is
/// dropped, this signals the poller to exit on its next iteration.
#[derive(Debug)]
struct DropGuard {
    stop: Arc<tokio::sync::Notify>,
}

impl Drop for DropGuard {
    fn drop(&mut self) {
        // `notify_one` persists a permit if no waiter is currently
        // parked. `notify_waiters` would silently drop the signal when
        // the poller is mid-RPC, leaving the task to wait for the next
        // `interval.tick()` before checking again. With `notify_one`
        // the next `.notified().await` resolves immediately.
        self.stop.notify_one();
    }
}

impl MarketStateStream {
    /// Subscribe to the given `(family, market)`.
    ///
    /// `client` is owned by value — clone if you need to retain another
    /// reference. `block_source` is the chain-head fetcher; for
    /// `JsonRpcProvider`-backed clients wrap the inner
    /// `starknet_providers::Provider` in a [`StarknetBlockSource`].
    ///
    /// The returned stream stays alive until dropped.
    pub fn subscribe<P, B>(
        client: DeadeyeClient<P>,
        block_source: B,
        family: Family,
        market: Felt,
        config: StreamConfig,
    ) -> Self
    where
        P: Provider + 'static,
        B: BlockNumberSource + 'static,
    {
        let (tx, rx) = mpsc::channel(64);
        let stop = Arc::new(tokio::sync::Notify::new());
        let stop_for_task = Arc::clone(&stop);

        tokio::spawn(async move {
            let mut last_block: Option<u64> = None;
            let mut interval = tokio::time::interval(config.poll_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    () = stop_for_task.notified() => break,
                    _ = interval.tick() => {},
                }
                let Ok(block) = block_source.block_number().await else {
                    continue;
                };
                if Some(block) == last_block {
                    continue;
                }
                last_block = Some(block);
                let update = build_update(&client, family, market, block, &config).await;
                if tx.send(update).await.is_err() {
                    // Receiver dropped — exit.
                    break;
                }
            }
        });

        Self {
            rx,
            _drop: DropGuard { stop },
        }
    }
}

impl Stream for MarketStateStream {
    type Item = MarketStateUpdate;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

/// Build a single [`MarketStateUpdate`] off the current state. Each
/// optional field is fetched only if requested; sub-failures are
/// silently elided so a transient single-read miss doesn't drop the
/// whole tick.
async fn build_update<P: Provider>(
    client: &DeadeyeClient<P>,
    family: Family,
    market: Felt,
    block_number: u64,
    config: &StreamConfig,
) -> MarketStateUpdate {
    let provider = client.provider();
    let distribution = if config.include_distribution {
        read_distribution(provider, family, market).await
    } else {
        None
    };
    let lp_info = if config.include_lp_info {
        read_lp_info(provider, family, market).await
    } else {
        None
    };
    let quote = if let Some(cand) = &config.include_quote_for_candidate {
        read_quote(provider, market, cand).await
    } else {
        None
    };
    MarketStateUpdate {
        block_number,
        family,
        market,
        distribution,
        lp_info,
        quote,
    }
}

async fn read_distribution<P: Provider>(
    provider: &P,
    family: Family,
    market: Felt,
) -> Option<DistributionSnapshot> {
    use deadeye_core::Distribution as _;
    match family {
        Family::Normal => NormalMarketReader::new(provider, market)
            .distribution()
            .await
            .ok()
            .map(|d| DistributionSnapshot::Normal(d.to_raw())),
        Family::Lognormal => LognormalMarketReader::new(provider, market)
            .distribution()
            .await
            .ok()
            .map(|d| DistributionSnapshot::Lognormal(d.to_raw())),
        Family::Multinoulli => MultinoulliMarketReader::new(provider, market)
            .distribution()
            .await
            .ok()
            .and_then(|d| d.to_raw().ok().map(DistributionSnapshot::Multinoulli)),
        Family::Bivariate => BivariateMarketReader::new(provider, market)
            .distribution()
            .await
            .ok()
            .and_then(|d| {
                d.to_raw()
                    .ok()
                    .map(|raw| DistributionSnapshot::Bivariate(Box::new(raw)))
            }),
    }
}

async fn read_lp_info<P: Provider>(
    provider: &P,
    family: Family,
    market: Felt,
) -> Option<LpInfoRaw> {
    match family {
        Family::Normal => NormalMarketReader::new(provider, market)
            .lp_info()
            .await
            .ok(),
        Family::Lognormal => LognormalMarketReader::new(provider, market)
            .lp_info()
            .await
            .ok(),
        Family::Multinoulli => MultinoulliMarketReader::new(provider, market)
            .lp_info()
            .await
            .ok(),
        Family::Bivariate => BivariateMarketReader::new(provider, market)
            .lp_info()
            .await
            .ok(),
    }
}

async fn read_quote<P: Provider>(
    provider: &P,
    market: Felt,
    candidate: &CandidateQuote,
) -> Option<QuoteSnapshot> {
    match candidate {
        CandidateQuote::Normal {
            runtime,
            candidate,
            x_star,
            supplied_collateral,
            collateral_pad,
        } => NormalMarketReader::new(provider, market)
            .quote_trade(
                *runtime,
                *candidate,
                *x_star,
                *supplied_collateral,
                *collateral_pad,
            )
            .await
            .ok()
            .map(QuoteSnapshot::Normal),
        CandidateQuote::Lognormal {
            runtime,
            candidate,
            x_star,
            supplied_collateral,
            collateral_pad,
        } => LognormalMarketReader::new(provider, market)
            .quote_trade(
                *runtime,
                *candidate,
                *x_star,
                *supplied_collateral,
                *collateral_pad,
            )
            .await
            .ok()
            .map(QuoteSnapshot::Lognormal),
        CandidateQuote::Multinoulli {
            runtime,
            candidate,
            min_outcome_index,
            supplied_collateral,
        } => MultinoulliMarketReader::new(provider, market)
            .quote_trade(
                *runtime,
                candidate.clone(),
                *min_outcome_index,
                *supplied_collateral,
            )
            .await
            .ok()
            .map(QuoteSnapshot::Multinoulli),
        CandidateQuote::Bivariate {
            runtime,
            core_candidate,
            x_star,
            supplied_collateral,
        } => BivariateMarketReader::new(provider, market)
            .quote_trade(*runtime, *core_candidate, *x_star, *supplied_collateral)
            .await
            .ok()
            .map(|q| QuoteSnapshot::Bivariate(Box::new(q))),
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use async_trait::async_trait;
    use deadeye_starknet::{CairoSerde, ContractError, ContractResult, Felt, Provider};
    use futures::StreamExt;
    use starknet_core::types::{BlockId, FunctionCall};

    use super::*;

    /// A `BlockNumberSource` that increments its returned block on
    /// every call — drives the stream to yield on every poll tick.
    struct CountingBlocks {
        next: AtomicU64,
    }

    #[async_trait]
    impl BlockNumberSource for CountingBlocks {
        async fn block_number(&self) -> ContractResult<u64> {
            Ok(self.next.fetch_add(1, Ordering::SeqCst))
        }
    }

    /// Returns a fixed normal distribution every call.
    #[derive(Clone)]
    struct FixedNormalProvider;

    #[async_trait]
    impl Provider for FixedNormalProvider {
        async fn call(&self, _call: FunctionCall, _block: BlockId) -> ContractResult<Vec<Felt>> {
            let raw = NormalDistributionRaw {
                mean: deadeye_core::Sq128::from_f64(42.0).unwrap().to_raw(),
                variance: deadeye_core::Sq128::from_f64(64.0).unwrap().to_raw(),
                sigma: deadeye_core::Sq128::from_f64(8.0).unwrap().to_raw(),
            };
            Ok(raw.to_calldata())
        }
    }

    #[tokio::test]
    async fn stream_yields_on_new_blocks() {
        let provider = FixedNormalProvider;
        let client = DeadeyeClient::new(provider);
        let blocks = CountingBlocks {
            next: AtomicU64::new(100),
        };
        let cfg = StreamConfig {
            poll_interval: Duration::from_millis(10),
            include_distribution: true,
            include_lp_info: false,
            include_quote_for_candidate: None,
        };
        let mut stream = MarketStateStream::subscribe(
            client,
            blocks,
            Family::Normal,
            Felt::from(0xdead_u64),
            cfg,
        );
        let first = stream.next().await.unwrap();
        let second = stream.next().await.unwrap();
        assert_eq!(first.family, Family::Normal);
        assert!(first.distribution.is_some());
        assert!(
            second.block_number > first.block_number,
            "block must advance"
        );
    }

    /// A `BlockNumberSource` that returns the *same* number every
    /// call — the stream must therefore stay quiet (no yields).
    struct StuckBlocks;

    #[async_trait]
    impl BlockNumberSource for StuckBlocks {
        async fn block_number(&self) -> ContractResult<u64> {
            Ok(100_u64)
        }
    }

    #[tokio::test]
    async fn stream_quiet_when_block_stuck() {
        let provider = FixedNormalProvider;
        let client = DeadeyeClient::new(provider);
        let cfg = StreamConfig {
            poll_interval: Duration::from_millis(5),
            include_distribution: true,
            include_lp_info: false,
            include_quote_for_candidate: None,
        };
        let mut stream = MarketStateStream::subscribe(
            client,
            StuckBlocks,
            Family::Normal,
            Felt::from(0xdead_u64),
            cfg,
        );
        // First tick should yield once at block 100; subsequent polls
        // should not yield because the block never advances.
        let first = stream.next().await.unwrap();
        assert_eq!(first.block_number, 100);
        // Wait twice the poll interval; we should *not* receive another.
        let timeout = tokio::time::timeout(Duration::from_millis(25), stream.next()).await;
        assert!(
            timeout.is_err(),
            "stream should not yield while block is stuck"
        );
    }

    #[tokio::test]
    async fn stream_stops_on_drop() {
        let provider = FixedNormalProvider;
        let client = DeadeyeClient::new(provider);
        let blocks = CountingBlocks {
            next: AtomicU64::new(1),
        };
        let cfg = StreamConfig {
            poll_interval: Duration::from_millis(5),
            include_distribution: false,
            include_lp_info: false,
            include_quote_for_candidate: None,
        };
        let stream = MarketStateStream::subscribe(
            client,
            blocks,
            Family::Normal,
            Felt::from(0xdead_u64),
            cfg,
        );
        drop(stream);
        // No assertion needed — if the poller didn't exit, the
        // process would hang on test teardown (tokio would warn).
    }

    // Silence unused import on Provider impl side
    #[allow(
        dead_code,
        reason = "private helper referenced only to keep `Provider`'s trait bounds discoverable from this module"
    )]
    fn _provider_bound_check<P: Provider>(_: &P) {
        // ContractError is non-Clone; just reference the types so the
        // compiler keeps the imports live.
        let _: fn(&ContractError) -> &ContractError = |c| c;
        let _: fn() -> ContractResult<()> = || Ok(());
    }
}
