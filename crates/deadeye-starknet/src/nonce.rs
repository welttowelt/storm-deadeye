//! Concurrent nonce allocator.
//!
//! Production market makers fan dozens of trades out across many markets
//! from a single wallet. Each trade needs a distinct INVOKE-v3 nonce —
//! the chain rejects duplicates with `InvalidTransactionNonce`. The
//! upstream `SingleOwnerAccount` resolves the nonce by calling
//! `get_nonce` on every submission, which serialises the bot to roughly
//! one in-flight tx per account.
//!
//! [`NonceManager`] hands out monotonically increasing nonces from an
//! in-memory allocator anchored to chain state. Each reservation returns
//! a [`NonceGuard`]; the strategy submits the tx with that exact nonce
//! and then calls [`NonceGuard::commit`]. Guards that drop without
//! committing release the nonce back to the front of the queue so a
//! later reservation gets the same value (canonical use case: the
//! strategy decided not to submit after all).
//!
//! ## Failure handling
//!
//! If a submission fails *after* the nonce was committed but the
//! transaction never reached the mempool, the in-memory allocator is
//! one ahead of the chain. Call [`NonceManager::resync`] to re-anchor
//! to chain state.

use std::{collections::BinaryHeap, sync::Arc};

use async_trait::async_trait;
use starknet_core::types::{BlockId, BlockTag, Felt};
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::instrument;

/// Read-side trait used by [`NonceManager`] to anchor to chain state.
///
/// The Starknet ecosystem has at least three plausible providers
/// (`JsonRpcClient`, `AnyProvider`, custom multi-RPC), so we parametrise
/// over a tiny narrow trait rather than depending on the full
/// `starknet_providers::Provider` surface. Two stock implementations
/// are gated behind features: `JsonRpcClient<HttpTransport>` via the
/// `provider` feature, and any [`starknet_providers::Provider`] via the
/// blanket impl below.
#[async_trait]
pub trait NonceFetcher: Send + Sync {
    /// Fetch the current chain nonce for `address`. Implementations
    /// should hit the pre-confirmed block so the allocator sees its own
    /// in-flight transactions reflected in chain state.
    async fn fetch_nonce(&self, address: Felt) -> Result<Felt, NonceError>;
}

#[cfg(feature = "provider")]
#[async_trait]
impl<P> NonceFetcher for P
where
    P: starknet_providers::Provider + Send + Sync + ?Sized,
{
    async fn fetch_nonce(&self, address: Felt) -> Result<Felt, NonceError> {
        starknet_providers::Provider::get_nonce(self, BlockId::Tag(BlockTag::PreConfirmed), address)
            .await
            .map_err(|e| NonceError::Fetch(format!("{e}")))
    }
}

/// Snapshot of the allocator's internal state — useful for diagnostics
/// (e.g. logging a "nonce in-flight gap" metric).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonceSnapshot {
    /// The next nonce the allocator will hand out on a fresh `reserve()`
    /// call (after exhausting the released queue).
    pub next: u64,
    /// Number of reservations currently outstanding (handed out but not
    /// yet committed or dropped).
    pub outstanding: usize,
    /// Number of released slots waiting to be re-issued.
    pub released: usize,
    /// Chain-anchored baseline from the last `resync` (or `new`).
    pub chain_anchor: u64,
}

/// Errors that can occur while reserving or resyncing a nonce.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NonceError {
    /// Reading the on-chain nonce failed.
    #[error("nonce fetch failed: {0}")]
    Fetch(String),

    /// The on-chain nonce returned a value that does not fit in a u64.
    /// Practically impossible in 2026 — included for completeness.
    #[error("on-chain nonce overflows u64: {felt}")]
    Overflow {
        /// The felt the chain returned.
        felt: Felt,
    },
}

/// Internal allocator state, guarded by an async mutex.
#[derive(Debug)]
struct Allocator {
    /// Smallest nonce the allocator has *not yet handed out*.
    next: u64,
    /// Released nonces (dropped without commit) — re-issued LIFO so a
    /// dropped guard gets reused immediately, avoiding a permanent gap.
    /// We use a max-heap inverted via [`std::cmp::Reverse`] to drain in
    /// ascending order — the chain only accepts the smallest gap-filling
    /// nonce, never out-of-order.
    released: BinaryHeap<std::cmp::Reverse<u64>>,
    /// Number of guards currently outstanding (handed out but neither
    /// committed nor dropped).
    outstanding: usize,
    /// Chain anchor at last `resync`/`new`. Reservations smaller than
    /// the anchor are rejected on `resync` (would be rejected on-chain
    /// anyway).
    chain_anchor: u64,
}

impl Allocator {
    fn new(chain_anchor: u64) -> Self {
        Self {
            next: chain_anchor,
            released: BinaryHeap::new(),
            outstanding: 0,
            chain_anchor,
        }
    }

    fn snapshot(&self) -> NonceSnapshot {
        NonceSnapshot {
            next: self.next,
            outstanding: self.outstanding,
            released: self.released.len(),
            chain_anchor: self.chain_anchor,
        }
    }

    fn reserve_one(&mut self) -> u64 {
        let value = if let Some(std::cmp::Reverse(v)) = self.released.pop() {
            v
        } else {
            let v = self.next;
            self.next = self.next.saturating_add(1);
            v
        };
        self.outstanding = self.outstanding.saturating_add(1);
        value
    }

    fn release(&mut self, value: u64) {
        self.outstanding = self.outstanding.saturating_sub(1);
        self.released.push(std::cmp::Reverse(value));
    }

    fn commit(&mut self) {
        self.outstanding = self.outstanding.saturating_sub(1);
    }

    fn resync_to(&mut self, chain_anchor: u64) {
        self.chain_anchor = chain_anchor;
        self.next = chain_anchor;
        self.released.clear();
        // Outstanding reservations remain — their guards still
        // hold valid nonces from the caller's perspective. The caller
        // is responsible for dropping them if they're stale.
    }
}

/// Concurrent nonce allocator anchored to chain state.
///
/// Clone-able via the `Arc` inside — clones share the same allocator.
#[derive(Clone)]
pub struct NonceManager {
    fetcher: Arc<dyn NonceFetcher>,
    address: Felt,
    state: Arc<Mutex<Allocator>>,
}

impl core::fmt::Debug for NonceManager {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // `state: Arc<Mutex<Allocator>>` is intentionally elided: locking
        // it from a `Debug` impl would either block the current task or
        // print a stale snapshot. `finish_non_exhaustive()` flags the
        // omission to readers.
        f.debug_struct("NonceManager")
            .field("address", &self.address)
            .field("fetcher", &"<dyn NonceFetcher>")
            .finish_non_exhaustive()
    }
}

impl NonceManager {
    /// Construct a new manager and anchor it to the current chain nonce.
    pub async fn new(fetcher: Arc<dyn NonceFetcher>, address: Felt) -> Result<Self, NonceError> {
        let anchor = fetch_chain_nonce(&*fetcher, address).await?;
        Ok(Self {
            fetcher,
            address,
            state: Arc::new(Mutex::new(Allocator::new(anchor))),
        })
    }

    /// Address the manager is bound to.
    pub const fn address(&self) -> Felt {
        self.address
    }

    /// Reserve the next nonce. Returns a [`NonceGuard`].
    ///
    /// If the strategy ultimately submits the transaction with this
    /// nonce, call [`NonceGuard::commit`]. Otherwise just drop the guard
    /// — the nonce is released back to the queue and will be reused
    /// by the next caller.
    #[instrument(skip(self), fields(address = %self.address))]
    pub async fn reserve(&self) -> NonceGuard {
        let value = {
            let mut state = self.state.lock().await;
            state.reserve_one()
        };
        NonceGuard {
            value,
            state: Some(Arc::clone(&self.state)),
        }
    }

    /// Reserve `n` contiguous nonces in a single critical section.
    ///
    /// Useful when a strategy wants to batch-submit and guarantee the
    /// returned nonces are increasing. Each guard behaves identically
    /// to one returned by [`Self::reserve`].
    #[instrument(skip(self), fields(address = %self.address, %n))]
    pub async fn reserve_batch(&self, n: usize) -> Vec<NonceGuard> {
        let mut guards = Vec::with_capacity(n);
        // Single lock acquisition for the batch — drop before building the
        // guard vec so the critical section stays tight.
        let values: Vec<u64> = {
            let mut state = self.state.lock().await;
            (0..n).map(|_| state.reserve_one()).collect()
        };
        for value in values {
            guards.push(NonceGuard {
                value,
                state: Some(Arc::clone(&self.state)),
            });
        }
        guards
    }

    /// Re-anchor the allocator to chain state.
    ///
    /// Call this after a known-failed submission left the in-memory
    /// state inconsistent (e.g. node restart, mempool flush, the bot
    /// crashed and re-started with stale state).
    ///
    /// **Caller contract:** any [`NonceGuard`] outstanding at the time
    /// of `resync()` must be dropped (and its tx confirmed *or*
    /// abandoned) *before* the next [`Self::reserve`] call. The
    /// allocator rewinds `next` to the chain anchor and clears the
    /// released queue, so a still-outstanding guard with nonce ≥ anchor
    /// would collide with the new reservation. The released queue is
    /// cleared because anything in it is, by definition, ≥ anchor.
    #[instrument(skip(self), fields(address = %self.address))]
    pub async fn resync(&self) -> Result<(), NonceError> {
        let anchor = fetch_chain_nonce(&*self.fetcher, self.address).await?;
        // Lock just long enough to re-anchor; the read happened above.
        self.state.lock().await.resync_to(anchor);
        Ok(())
    }

    /// Returns the chain-anchored u64 baseline captured at the last
    /// resync (or construction).
    pub async fn chain_anchor(&self) -> u64 {
        self.state.lock().await.chain_anchor
    }

    /// Diagnostic snapshot of the allocator.
    pub async fn snapshot(&self) -> NonceSnapshot {
        self.state.lock().await.snapshot()
    }
}

async fn fetch_chain_nonce(
    fetcher: &(dyn NonceFetcher + 'static),
    address: Felt,
) -> Result<u64, NonceError> {
    let felt = fetcher.fetch_nonce(address).await?;
    // A Felt that doesn't fit in u64 means the on-chain nonce wrapped
    // past 2^64. Realistically impossible (~5e11 yrs at 1k tps) but we
    // still surface a typed error so callers can bail loudly.
    let bytes = felt.to_bytes_be();
    let (high, low) = bytes.split_at(24);
    if high.iter().any(|b| *b != 0) {
        return Err(NonceError::Overflow { felt });
    }
    let mut buf = [0_u8; 8];
    buf.copy_from_slice(low);
    // BE is deliberate: Starknet felts are big-endian by definition
    // (`Felt::to_bytes_be`). Mirroring that here keeps the bit-pattern
    // round-trip exact; using `from_le_bytes` would silently corrupt.
    #[allow(
        clippy::big_endian_bytes,
        reason = "Starknet felt encoding is big-endian — see Felt::to_bytes_be"
    )]
    Ok(u64::from_be_bytes(buf))
}

/// A reserved nonce. Drop without [`Self::commit`] to release it.
#[derive(Debug)]
pub struct NonceGuard {
    value: u64,
    /// `None` once the guard has been committed (so `Drop` is a no-op).
    state: Option<Arc<Mutex<Allocator>>>,
}

impl NonceGuard {
    /// The reserved nonce, as a Felt for direct calldata use.
    pub fn value(&self) -> Felt {
        Felt::from(self.value)
    }

    /// The reserved nonce as a raw u64.
    pub const fn raw(&self) -> u64 {
        self.value
    }

    /// Commit the reservation — the nonce will *not* be released on drop.
    /// Call this after the transaction has been successfully submitted
    /// (or at least: after the caller is sure they want to consume this
    /// slot on-chain).
    pub fn commit(mut self) {
        // Detach the state pointer; Drop becomes a no-op.
        if let Some(state) = self.state.take() {
            // Fast path: non-blocking try_lock. The `outstanding`
            // counter is diagnostic-only — the reserved nonce was
            // already taken off `released`/`next` at `reserve` time, so
            // racing the counter update is harmless.
            if let Ok(mut state) = state.try_lock() {
                state.commit();
            } else if let Ok(handle) = tokio::runtime::Handle::try_current() {
                // Inside a tokio runtime: schedule the update.
                handle.spawn(async move {
                    let mut state = state.lock().await;
                    state.commit();
                });
            }
            // No runtime + contended lock: silently skip the counter
            // update. Functionally safe — the nonce is already consumed.
        }
    }
}

impl Drop for NonceGuard {
    fn drop(&mut self) {
        // If `commit()` was called, `state` is None and we do nothing.
        if let Some(state) = self.state.take() {
            let value = self.value;
            tracing::trace!(nonce = value, "nonce guard dropped without commit");
            // Best-effort: try a non-blocking release first.
            if let Ok(mut state) = state.try_lock() {
                state.release(value);
            } else if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let mut state = state.lock().await;
                    state.release(value);
                });
            } else {
                // No tokio runtime + contended lock: the nonce slot
                // leaks for this allocator instance. Surface the leak
                // via a counter the binary can scrape.
                metrics::counter!("deadeye.nonce.gap_total").increment(1);
            }
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[derive(Debug)]
    struct StaticFetcher {
        value: AtomicU64,
    }

    #[async_trait]
    impl NonceFetcher for StaticFetcher {
        async fn fetch_nonce(&self, _address: Felt) -> Result<Felt, NonceError> {
            Ok(Felt::from(self.value.load(Ordering::SeqCst)))
        }
    }

    fn fetcher(value: u64) -> Arc<dyn NonceFetcher> {
        Arc::new(StaticFetcher {
            value: AtomicU64::new(value),
        })
    }

    #[tokio::test]
    async fn reserve_hands_out_monotonic_nonces() {
        let nm = NonceManager::new(fetcher(7), Felt::from(0x42_u64))
            .await
            .unwrap();
        let g1 = nm.reserve().await;
        let g2 = nm.reserve().await;
        let g3 = nm.reserve().await;
        assert_eq!(g1.raw(), 7);
        assert_eq!(g2.raw(), 8);
        assert_eq!(g3.raw(), 9);
        // Don't commit — they'll be released.
    }

    #[tokio::test]
    async fn dropped_guards_release_nonce() {
        let nm = NonceManager::new(fetcher(0), Felt::from(0x42_u64))
            .await
            .unwrap();
        let g1 = nm.reserve().await;
        let g2 = nm.reserve().await;
        assert_eq!(g1.raw(), 0);
        assert_eq!(g2.raw(), 1);
        drop(g1);
        // Let the spawned release task run.
        tokio::task::yield_now().await;
        // Give the spawned commit/release a chance.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let g3 = nm.reserve().await;
        // g3 should reuse nonce 0.
        assert_eq!(g3.raw(), 0);
        drop(g2);
        drop(g3);
    }

    #[tokio::test]
    async fn commit_does_not_release() {
        let nm = NonceManager::new(fetcher(5), Felt::from(0x42_u64))
            .await
            .unwrap();
        let g1 = nm.reserve().await;
        g1.commit();
        let g2 = nm.reserve().await;
        assert_eq!(g2.raw(), 6);
        g2.commit();
    }

    #[tokio::test]
    async fn batch_reserves_contiguous_nonces() {
        let nm = NonceManager::new(fetcher(100), Felt::from(0x42_u64))
            .await
            .unwrap();
        let guards = nm.reserve_batch(5).await;
        assert_eq!(guards.len(), 5);
        for (i, g) in guards.iter().enumerate() {
            assert_eq!(g.raw(), 100 + i as u64);
        }
        for g in guards {
            g.commit();
        }
    }

    #[tokio::test]
    async fn concurrent_reserves_are_unique() {
        let nm = NonceManager::new(fetcher(0), Felt::from(0x42_u64))
            .await
            .unwrap();
        let nm = Arc::new(nm);
        let mut tasks = Vec::new();
        for _ in 0..50 {
            let nm = Arc::clone(&nm);
            tasks.push(tokio::spawn(async move {
                let g = nm.reserve().await;
                let v = g.raw();
                g.commit();
                v
            }));
        }
        let mut values: Vec<u64> = Vec::new();
        for t in tasks {
            values.push(t.await.unwrap());
        }
        values.sort_unstable();
        for (i, v) in values.iter().enumerate() {
            assert_eq!(*v, i as u64, "expected dense range [0, 50)");
        }
    }

    #[tokio::test]
    async fn resync_re_anchors_allocator() {
        let fetcher_ref = StaticFetcher {
            value: AtomicU64::new(10),
        };
        let fetcher: Arc<dyn NonceFetcher> = Arc::new(fetcher_ref);
        let nm = NonceManager::new(Arc::clone(&fetcher), Felt::from(0x42_u64))
            .await
            .unwrap();
        let g1 = nm.reserve().await;
        assert_eq!(g1.raw(), 10);
        g1.commit();
        // Bump the chain-side fetcher and resync.
        // (downcast not possible through trait object — exercise the
        // public resync against the same fetcher value to confirm the
        // allocator re-anchors to it).
        nm.resync().await.unwrap();
        let g2 = nm.reserve().await;
        // Resync rewound `next` to anchor=10 (and the freed nonces
        // queue is cleared). So we get 10 again.
        assert_eq!(g2.raw(), 10);
        g2.commit();
    }
}
