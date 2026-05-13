//! TTL-bounded caching layer over [`deadeye_starknet::Provider`].
//!
//! Mirrors the TS `ReadModels` Service. The cache is keyed by `(call name,
//! contract address, calldata felts)` and stores the raw felt response with
//! a wall-clock deadline. On a hit, the cached felts are returned without
//! touching the network; on a miss, the underlying provider is invoked
//! and the result memoised.
//!
//! Single-flight is **not** implemented — under heavy concurrency,
//! duplicate requests may race the same RPC call. For an MM loop reading
//! the same market at 10 Hz that's a no-op concern; for a high-throughput
//! dashboard it's a future improvement.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use deadeye_starknet::{ContractResult, Provider};
use starknet_core::types::{BlockId, Felt, FunctionCall};

/// Cache TTL defaults — tuned for an MM loop where state is allowed to
/// be a couple of seconds stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheTtls {
    /// TTL for market reads (distribution, params, fees).
    pub market: Duration,
    /// TTL for position reads.
    pub position: Duration,
    /// TTL for LP reads.
    pub lp: Duration,
}

impl Default for CacheTtls {
    fn default() -> Self {
        Self {
            market: Duration::from_secs(5),
            position: Duration::from_secs(2),
            lp: Duration::from_secs(2),
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    value: Vec<Felt>,
    expires_at: Instant,
}

/// A [`Provider`] wrapper that memoises every view call for a fixed TTL.
#[derive(Debug)]
pub struct ReadModelsCached<P>
where
    P: Provider,
{
    inner: P,
    ttl: Duration,
    cache: Mutex<HashMap<CacheKey, Entry>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    contract: Felt,
    selector: Felt,
    calldata: Vec<Felt>,
}

impl<P> ReadModelsCached<P>
where
    P: Provider,
{
    /// Wrap an existing provider with a uniform TTL.
    pub fn new(inner: P, ttl: Duration) -> Self {
        Self {
            inner,
            ttl,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Wrap with the default 5-second TTL.
    pub fn with_default_ttl(inner: P) -> Self {
        Self::new(inner, CacheTtls::default().market)
    }

    /// Drop all cached entries.
    pub fn clear(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }

    /// Cache size — useful in tests.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cache.lock().map(|c| c.len()).unwrap_or(0)
    }

    /// Returns `true` if the cache currently holds no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl<P> Provider for ReadModelsCached<P>
where
    P: Provider,
{
    async fn call(&self, call: FunctionCall, block: BlockId) -> ContractResult<Vec<Felt>> {
        let key = CacheKey {
            contract: call.contract_address,
            selector: call.entry_point_selector,
            calldata: call.calldata.clone(),
        };
        let now = Instant::now();
        if let Ok(cache) = self.cache.lock()
            && let Some(entry) = cache.get(&key)
            && entry.expires_at > now
        {
            return Ok(entry.value.clone());
        }
        let value = self.inner.call(call, block).await?;
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(
                key,
                Entry {
                    value: value.clone(),
                    expires_at: now + self.ttl,
                },
            );
        }
        Ok(value)
    }

    fn default_block(&self) -> BlockId {
        self.inner.default_block()
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use deadeye_starknet::ContractError;

    use super::*;

    #[derive(Debug, Default)]
    struct CountingProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Provider for CountingProvider {
        async fn call(&self, _call: FunctionCall, _block: BlockId) -> ContractResult<Vec<Felt>> {
            let _ = self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![Felt::from(42_u64)])
        }
    }

    #[tokio::test]
    async fn cache_hits_avoid_provider_calls() {
        let inner = CountingProvider::default();
        let cache = ReadModelsCached::new(inner, Duration::from_secs(60));
        let call = FunctionCall {
            contract_address: Felt::from(1_u64),
            entry_point_selector: Felt::from(2_u64),
            calldata: vec![],
        };
        for _ in 0..5 {
            let r = cache
                .call(call.clone(), cache.default_block())
                .await
                .unwrap();
            assert_eq!(r, vec![Felt::from(42_u64)]);
        }
        assert_eq!(cache.inner.calls.load(Ordering::SeqCst), 1);
        assert_eq!(cache.len(), 1);
    }

    #[tokio::test]
    async fn cache_evicts_after_ttl() {
        let inner = CountingProvider::default();
        let cache = ReadModelsCached::new(inner, Duration::from_millis(50));
        let call = FunctionCall {
            contract_address: Felt::from(1_u64),
            entry_point_selector: Felt::from(2_u64),
            calldata: vec![],
        };
        let _ = cache
            .call(call.clone(), cache.default_block())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        let _ = cache
            .call(call.clone(), cache.default_block())
            .await
            .unwrap();
        // Second call missed the cache.
        assert_eq!(cache.inner.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn clear_drops_all_entries() {
        let inner = CountingProvider::default();
        let cache = ReadModelsCached::new(inner, Duration::from_secs(60));
        let call = FunctionCall {
            contract_address: Felt::from(1_u64),
            entry_point_selector: Felt::from(2_u64),
            calldata: vec![],
        };
        let _ = cache
            .call(call.clone(), cache.default_block())
            .await
            .unwrap();
        cache.clear();
        assert_eq!(cache.len(), 0);
    }

    // Silence the unused-import warning on ContractError under cfg(test).
    const _: fn() = || {
        let _ = core::mem::size_of::<ContractError>();
    };
}
