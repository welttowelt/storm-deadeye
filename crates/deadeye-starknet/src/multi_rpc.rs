//! Multi-endpoint JSON-RPC provider with circuit-breaker + retry/backoff.
//!
//! A single upstream RPC is a single point of failure: cloud-provider
//! outages, transient 5xx, geographic latency spikes, deliberate rate
//! limiting. Production market makers point their bots at three or more
//! independent RPC endpoints and round-robin between them, demoting
//! endpoints that repeatedly fail and promoting them back after a
//! cooldown.
//!
//! [`MultiRpcProvider`] is a drop-in [`starknet_providers::Provider`]
//! implementation that wraps `N` [`JsonRpcClient`]s. Each call:
//!
//! 1. Picks the least-loaded *healthy* endpoint.
//! 2. Issues the request with a per-call timeout.
//! 3. On a **transient** error (timeout, connection refused, 5xx,
//!    network), bumps that endpoint's failure counter, exponentially
//!    backs off, and tries the next healthy endpoint.
//! 4. On a **non-transient** Starknet error (reverts, bad requests),
//!    returns immediately — those are deterministic and retrying just
//!    burns latency.
//! 5. Once an endpoint hits `circuit_breaker_threshold` consecutive
//!    failures it is marked **down**; after `circuit_breaker_cooldown`
//!    it enters **half-open** and the next probe call decides.
//!
//! The whole thing uses `tokio::sync::Mutex` so it can be shared across
//! threads via `Arc<MultiRpcProvider>`.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use starknet_core::types::{
    BlockHashAndNumber, BlockId, BroadcastedDeclareTransaction,
    BroadcastedDeployAccountTransaction, BroadcastedInvokeTransaction, BroadcastedTransaction,
    ConfirmedBlockId, ContractClass, ContractStorageKeys, DeclareTransactionResult,
    DeployAccountTransactionResult, EventFilter, EventsPage, FeeEstimate, Felt, FunctionCall,
    Hash256, InvokeTransactionResult, MaybePreConfirmedBlockWithReceipts,
    MaybePreConfirmedBlockWithTxHashes, MaybePreConfirmedBlockWithTxs,
    MaybePreConfirmedStateUpdate, MessageFeeEstimate, MessageStatus, MsgFromL1,
    SimulatedTransaction, SimulationFlag, SimulationFlagForEstimateFee, StorageProof,
    SyncStatusType, Transaction, TransactionReceiptWithBlockInfo, TransactionStatus,
    TransactionTrace, TransactionTraceWithHash,
};
use starknet_providers::{
    JsonRpcClient, Provider as StarknetProvider, ProviderError, ProviderRequestData,
    ProviderResponseData, jsonrpc::HttpTransport,
};
use tokio::sync::Mutex;
use tracing::{debug, warn};
use url::Url;

// ─── Config + Health state ────────────────────────────────────────────────────

/// Tunables for [`MultiRpcProvider`].
///
/// Defaults are picked for an MM hitting devnet / a private node where
/// transient failures should clear within seconds. Production deployments
/// against shared infrastructure (Infura, Alchemy, `BlastAPI`) want a
/// longer cooldown and a higher threshold.
#[derive(Debug, Clone, Copy)]
pub struct RpcConfig {
    /// Maximum number of retry attempts across *all* healthy endpoints
    /// before bubbling the last error to the caller.
    pub max_retries: u32,
    /// Initial backoff between retries. Doubles every retry up to
    /// `max_backoff`.
    pub initial_backoff: Duration,
    /// Upper bound on per-retry sleep.
    pub max_backoff: Duration,
    /// Consecutive transient failures that flip an endpoint to "down".
    pub circuit_breaker_threshold: u32,
    /// Time an endpoint stays down before becoming half-open.
    pub circuit_breaker_cooldown: Duration,
    /// Per-call timeout applied to every individual RPC invocation.
    pub timeout_per_call: Duration,
}

impl RpcConfig {
    /// Sensible defaults for market-maker workloads.
    #[must_use]
    pub const fn market_maker_defaults() -> Self {
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_millis(50),
            max_backoff: Duration::from_millis(800),
            circuit_breaker_threshold: 5,
            circuit_breaker_cooldown: Duration::from_secs(10),
            timeout_per_call: Duration::from_secs(5),
        }
    }
}

impl Default for RpcConfig {
    fn default() -> Self {
        Self::market_maker_defaults()
    }
}

/// Health state of a single endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointHealthState {
    /// Endpoint is in regular rotation.
    Healthy,
    /// Endpoint is currently blocked from rotation.
    /// `since` is the instant it was downed.
    Down,
    /// Endpoint may have recovered; the next probe decides.
    HalfOpen,
}

/// Per-endpoint health snapshot returned by [`MultiRpcProvider::endpoint_health`].
#[derive(Debug, Clone, Copy)]
pub struct EndpointHealth {
    /// Current state.
    pub state: EndpointHealthState,
    /// Consecutive transient-failure counter (cleared on success).
    pub failures: u32,
    /// Total number of successful calls served by this endpoint.
    pub successes: u64,
    /// Total number of transient failures across the endpoint's lifetime.
    pub total_failures: u64,
}

#[derive(Debug)]
struct EndpointState {
    state: EndpointHealthState,
    failures: u32,
    successes: u64,
    total_failures: u64,
    last_failure_at: Option<Instant>,
}

impl EndpointState {
    const fn fresh() -> Self {
        Self {
            state: EndpointHealthState::Healthy,
            failures: 0,
            successes: 0,
            total_failures: 0,
            last_failure_at: None,
        }
    }
}

#[derive(Debug)]
struct Endpoint {
    url: Url,
    client: JsonRpcClient<HttpTransport>,
    health: Mutex<EndpointState>,
}

// ─── MultiRpcProvider ─────────────────────────────────────────────────────────

/// Multi-endpoint JSON-RPC provider with circuit-breaker + retry.
///
/// Construct via [`Self::new`] or [`Self::with_defaults`], then use it
/// anywhere a [`starknet_providers::Provider`] is expected.
#[derive(Debug)]
pub struct MultiRpcProvider {
    endpoints: Vec<Endpoint>,
    config: RpcConfig,
    /// Round-robin starting index. Atomic so concurrent callers don't
    /// all stampede the same endpoint on the first attempt.
    cursor: AtomicUsize,
}

impl MultiRpcProvider {
    /// Construct a provider over `endpoints` with the supplied config.
    ///
    /// # Panics
    /// Panics if `endpoints` is empty.
    #[must_use]
    pub fn new(endpoints: Vec<Url>, config: RpcConfig) -> Self {
        assert!(
            !endpoints.is_empty(),
            "MultiRpcProvider requires at least one endpoint",
        );
        let endpoints = endpoints
            .into_iter()
            .map(|url| {
                let client = JsonRpcClient::new(HttpTransport::new(url.clone()));
                Endpoint {
                    url,
                    client,
                    health: Mutex::new(EndpointState::fresh()),
                }
            })
            .collect();
        Self {
            endpoints,
            config,
            cursor: AtomicUsize::new(0),
        }
    }

    /// Construct a provider with [`RpcConfig::market_maker_defaults`].
    ///
    /// # Panics
    /// Panics if `endpoints` is empty.
    #[must_use]
    pub fn with_defaults(endpoints: Vec<Url>) -> Self {
        Self::new(endpoints, RpcConfig::market_maker_defaults())
    }

    /// Active configuration.
    pub const fn config(&self) -> &RpcConfig {
        &self.config
    }

    /// Snapshot every endpoint's health. Useful for `/health` endpoints
    /// in MM gateways.
    pub async fn endpoint_health(&self) -> Vec<(Url, EndpointHealth)> {
        let mut out = Vec::with_capacity(self.endpoints.len());
        for ep in &self.endpoints {
            let h = ep.health.lock().await;
            out.push((
                ep.url.clone(),
                EndpointHealth {
                    state: h.state,
                    failures: h.failures,
                    successes: h.successes,
                    total_failures: h.total_failures,
                },
            ));
        }
        out
    }

    /// Run `f` against a healthy endpoint, applying retry + circuit
    /// breaker. The closure receives a reference to the inner JSON-RPC
    /// client and must return a boxed future bound to that borrow.
    ///
    /// This is the workhorse used by every trait method below.
    async fn dispatch<'a, F, T>(&'a self, op: F) -> Result<T, ProviderError>
    where
        F: for<'c> FnMut(
            &'c JsonRpcClient<HttpTransport>,
        ) -> futures::future::BoxFuture<'c, Result<T, ProviderError>>,
    {
        self.dispatch_with_method(None, op).await
    }

    #[allow(
        clippy::too_many_lines,
        reason = "single hot path; splitting hurts readability"
    )]
    async fn dispatch_with_method<'a, F, T>(
        &'a self,
        method: Option<&'static str>,
        mut op: F,
    ) -> Result<T, ProviderError>
    where
        F: for<'c> FnMut(
            &'c JsonRpcClient<HttpTransport>,
        ) -> futures::future::BoxFuture<'c, Result<T, ProviderError>>,
    {
        let n = self.endpoints.len();
        let start = self.cursor.fetch_add(1, Ordering::Relaxed);
        let mut backoff = self.config.initial_backoff;
        let mut last_err: Option<ProviderError> = None;
        let method_label = method.unwrap_or("unspecified");
        let mut prev_endpoint: Option<String> = None;

        // Total attempts = max_retries + 1 (initial). Each attempt picks
        // the next healthy endpoint in round-robin order.
        for attempt in 0..=self.config.max_retries {
            let mut chosen: Option<usize> = None;
            for offset in 0..n {
                let idx = (start.wrapping_add(offset).wrapping_add(attempt as usize)) % n;
                if self.is_eligible(idx).await {
                    chosen = Some(idx);
                    break;
                }
            }
            let Some(idx) = chosen else {
                // All endpoints down. Sleep and try again — some may
                // come out of cooldown on the next iteration.
                if attempt == self.config.max_retries {
                    break;
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(self.config.max_backoff);
                continue;
            };
            let ep: &Endpoint = &self.endpoints[idx];
            let endpoint_label = ep.url.to_string();
            if let Some(prev) = prev_endpoint.as_ref()
                && prev != &endpoint_label
            {
                metrics::counter!(
                    "deadeye.rpc.failover_total",
                    "from_endpoint" => prev.clone(),
                    "to_endpoint" => endpoint_label.clone(),
                )
                .increment(1);
                tracing::info!(
                    from = %prev,
                    to = %endpoint_label,
                    attempt,
                    "multi-rpc failover",
                );
            }
            debug!(endpoint = %ep.url, attempt, method = method_label, "multi-rpc dispatch");
            let fut = op(&ep.client);
            let started = Instant::now();
            let res = tokio::time::timeout(self.config.timeout_per_call, fut).await;
            let elapsed = started.elapsed();
            metrics::histogram!(
                "deadeye.rpc.latency_seconds",
                "endpoint" => endpoint_label.clone(),
                "method" => method_label,
            )
            .record(elapsed.as_secs_f64());
            match res {
                Ok(Ok(value)) => {
                    self.record_success(idx).await;
                    return Ok(value);
                },
                Ok(Err(e)) => {
                    if is_transient(&e) {
                        warn!(endpoint = %ep.url, error = %e, "transient rpc failure");
                        self.record_failure(idx).await;
                        self.maybe_emit_breaker_trip(idx, &endpoint_label).await;
                        last_err = Some(e);
                    } else {
                        // Deterministic error — return immediately,
                        // don't penalize the endpoint.
                        debug!(endpoint = %ep.url, error = %e, "non-transient rpc error");
                        return Err(e);
                    }
                },
                Err(_) => {
                    warn!(endpoint = %ep.url, timeout = ?self.config.timeout_per_call, "rpc timeout");
                    self.record_failure(idx).await;
                    self.maybe_emit_breaker_trip(idx, &endpoint_label).await;
                    last_err = Some(ProviderError::Other(Box::new(TimeoutError {
                        timeout: self.config.timeout_per_call,
                    })));
                },
            }
            prev_endpoint = Some(endpoint_label);
            if attempt < self.config.max_retries {
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(self.config.max_backoff);
            }
        }
        Err(last_err.unwrap_or_else(|| {
            ProviderError::Other(Box::new(NoHealthyEndpointError { total_endpoints: n }))
        }))
    }

    async fn maybe_emit_breaker_trip(&self, idx: usize, endpoint_label: &str) {
        let h = self.endpoints[idx].health.lock().await;
        if h.state == EndpointHealthState::Down {
            tracing::warn!(
                endpoint = %endpoint_label,
                failures = h.failures,
                "circuit breaker tripped",
            );
        }
    }

    async fn is_eligible(&self, idx: usize) -> bool {
        let ep = &self.endpoints[idx];
        let mut h = ep.health.lock().await;
        match h.state {
            EndpointHealthState::Healthy | EndpointHealthState::HalfOpen => true,
            EndpointHealthState::Down => {
                // Promote to half-open if cooldown has elapsed.
                let cooldown_elapsed = h
                    .last_failure_at
                    .is_some_and(|t| t.elapsed() >= self.config.circuit_breaker_cooldown);
                if cooldown_elapsed {
                    h.state = EndpointHealthState::HalfOpen;
                    h.failures = 0;
                    true
                } else {
                    false
                }
            },
        }
    }

    async fn record_success(&self, idx: usize) {
        let mut h = self.endpoints[idx].health.lock().await;
        h.failures = 0;
        h.successes = h.successes.saturating_add(1);
        h.state = EndpointHealthState::Healthy;
    }

    async fn record_failure(&self, idx: usize) {
        let mut h = self.endpoints[idx].health.lock().await;
        h.failures = h.failures.saturating_add(1);
        h.total_failures = h.total_failures.saturating_add(1);
        h.last_failure_at = Some(Instant::now());
        if h.failures >= self.config.circuit_breaker_threshold {
            h.state = EndpointHealthState::Down;
        }
    }
}

/// Classify a [`ProviderError`] as transient (retry / failover) vs
/// deterministic (return immediately).
fn is_transient(err: &ProviderError) -> bool {
    match err {
        // RateLimited and Other (transport-level) errors are transient.
        ProviderError::RateLimited | ProviderError::Other(_) => true,
        // Array-length mismatches indicate a real protocol-level
        // mismatch — retrying won't help. StarknetError covers reverts,
        // bad requests, missing resources — also deterministic.
        ProviderError::ArrayLengthMismatch | ProviderError::StarknetError(_) => false,
    }
}

#[derive(Debug, thiserror::Error)]
#[error("rpc call timed out after {timeout:?}")]
struct TimeoutError {
    timeout: Duration,
}

impl starknet_providers::ProviderImplError for TimeoutError {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Debug, thiserror::Error)]
#[error("no healthy endpoint available across {total_endpoints} configured endpoints")]
struct NoHealthyEndpointError {
    total_endpoints: usize,
}

impl starknet_providers::ProviderImplError for NoHealthyEndpointError {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ─── starknet_providers::Provider impl ────────────────────────────────────────

#[async_trait]
impl StarknetProvider for MultiRpcProvider {
    async fn spec_version(&self) -> Result<String, ProviderError> {
        self.dispatch(|c| Box::pin(async move { c.spec_version().await }))
            .await
    }

    async fn get_block_with_tx_hashes<B>(
        &self,
        block_id: B,
    ) -> Result<MaybePreConfirmedBlockWithTxHashes, ProviderError>
    where
        B: AsRef<BlockId> + Send + Sync,
    {
        let block_id = *block_id.as_ref();
        self.dispatch(move |c| Box::pin(async move { c.get_block_with_tx_hashes(block_id).await }))
            .await
    }

    async fn get_block_with_txs<B>(
        &self,
        block_id: B,
    ) -> Result<MaybePreConfirmedBlockWithTxs, ProviderError>
    where
        B: AsRef<BlockId> + Send + Sync,
    {
        let block_id = *block_id.as_ref();
        self.dispatch(move |c| Box::pin(async move { c.get_block_with_txs(block_id).await }))
            .await
    }

    async fn get_block_with_receipts<B>(
        &self,
        block_id: B,
    ) -> Result<MaybePreConfirmedBlockWithReceipts, ProviderError>
    where
        B: AsRef<BlockId> + Send + Sync,
    {
        let block_id = *block_id.as_ref();
        self.dispatch(move |c| Box::pin(async move { c.get_block_with_receipts(block_id).await }))
            .await
    }

    async fn get_state_update<B>(
        &self,
        block_id: B,
    ) -> Result<MaybePreConfirmedStateUpdate, ProviderError>
    where
        B: AsRef<BlockId> + Send + Sync,
    {
        let block_id = *block_id.as_ref();
        self.dispatch(move |c| Box::pin(async move { c.get_state_update(block_id).await }))
            .await
    }

    async fn get_storage_at<A, K, B>(
        &self,
        contract_address: A,
        key: K,
        block_id: B,
    ) -> Result<Felt, ProviderError>
    where
        A: AsRef<Felt> + Send + Sync,
        K: AsRef<Felt> + Send + Sync,
        B: AsRef<BlockId> + Send + Sync,
    {
        let addr = *contract_address.as_ref();
        let key = *key.as_ref();
        let block = *block_id.as_ref();
        self.dispatch(move |c| Box::pin(async move { c.get_storage_at(addr, key, block).await }))
            .await
    }

    async fn get_messages_status(
        &self,
        transaction_hash: Hash256,
    ) -> Result<Vec<MessageStatus>, ProviderError> {
        self.dispatch(move |c| {
            Box::pin(async move { c.get_messages_status(transaction_hash).await })
        })
        .await
    }

    async fn get_transaction_status<H>(
        &self,
        transaction_hash: H,
    ) -> Result<TransactionStatus, ProviderError>
    where
        H: AsRef<Felt> + Send + Sync,
    {
        let h = *transaction_hash.as_ref();
        self.dispatch(move |c| Box::pin(async move { c.get_transaction_status(h).await }))
            .await
    }

    async fn get_transaction_by_hash<H>(
        &self,
        transaction_hash: H,
    ) -> Result<Transaction, ProviderError>
    where
        H: AsRef<Felt> + Send + Sync,
    {
        let h = *transaction_hash.as_ref();
        self.dispatch(move |c| Box::pin(async move { c.get_transaction_by_hash(h).await }))
            .await
    }

    async fn get_transaction_by_block_id_and_index<B>(
        &self,
        block_id: B,
        index: u64,
    ) -> Result<Transaction, ProviderError>
    where
        B: AsRef<BlockId> + Send + Sync,
    {
        let b = *block_id.as_ref();
        self.dispatch(move |c| {
            Box::pin(async move { c.get_transaction_by_block_id_and_index(b, index).await })
        })
        .await
    }

    async fn get_transaction_receipt<H>(
        &self,
        transaction_hash: H,
    ) -> Result<TransactionReceiptWithBlockInfo, ProviderError>
    where
        H: AsRef<Felt> + Send + Sync,
    {
        let h = *transaction_hash.as_ref();
        self.dispatch(move |c| Box::pin(async move { c.get_transaction_receipt(h).await }))
            .await
    }

    async fn get_class<B, H>(
        &self,
        block_id: B,
        class_hash: H,
    ) -> Result<ContractClass, ProviderError>
    where
        B: AsRef<BlockId> + Send + Sync,
        H: AsRef<Felt> + Send + Sync,
    {
        let b = *block_id.as_ref();
        let h = *class_hash.as_ref();
        self.dispatch(move |c| Box::pin(async move { c.get_class(b, h).await }))
            .await
    }

    async fn get_class_hash_at<B, A>(
        &self,
        block_id: B,
        contract_address: A,
    ) -> Result<Felt, ProviderError>
    where
        B: AsRef<BlockId> + Send + Sync,
        A: AsRef<Felt> + Send + Sync,
    {
        let b = *block_id.as_ref();
        let a = *contract_address.as_ref();
        self.dispatch(move |c| Box::pin(async move { c.get_class_hash_at(b, a).await }))
            .await
    }

    async fn get_class_at<B, A>(
        &self,
        block_id: B,
        contract_address: A,
    ) -> Result<ContractClass, ProviderError>
    where
        B: AsRef<BlockId> + Send + Sync,
        A: AsRef<Felt> + Send + Sync,
    {
        let b = *block_id.as_ref();
        let a = *contract_address.as_ref();
        self.dispatch(move |c| Box::pin(async move { c.get_class_at(b, a).await }))
            .await
    }

    async fn get_block_transaction_count<B>(&self, block_id: B) -> Result<u64, ProviderError>
    where
        B: AsRef<BlockId> + Send + Sync,
    {
        let b = *block_id.as_ref();
        self.dispatch(move |c| Box::pin(async move { c.get_block_transaction_count(b).await }))
            .await
    }

    async fn call<R, B>(&self, request: R, block_id: B) -> Result<Vec<Felt>, ProviderError>
    where
        R: AsRef<FunctionCall> + Send + Sync,
        B: AsRef<BlockId> + Send + Sync,
    {
        let req = Arc::new(request.as_ref().clone());
        let b = *block_id.as_ref();
        self.dispatch_with_method(Some("call"), move |c| {
            let req = Arc::clone(&req);
            Box::pin(async move { c.call((*req).clone(), b).await })
        })
        .await
    }

    async fn estimate_fee<R, S, B>(
        &self,
        request: R,
        simulation_flags: S,
        block_id: B,
    ) -> Result<Vec<FeeEstimate>, ProviderError>
    where
        R: AsRef<[BroadcastedTransaction]> + Send + Sync,
        S: AsRef<[SimulationFlagForEstimateFee]> + Send + Sync,
        B: AsRef<BlockId> + Send + Sync,
    {
        let req: Arc<Vec<BroadcastedTransaction>> = Arc::new(request.as_ref().to_vec());
        let flags: Arc<Vec<SimulationFlagForEstimateFee>> =
            Arc::new(simulation_flags.as_ref().to_vec());
        let b = *block_id.as_ref();
        self.dispatch(move |c| {
            let req = Arc::clone(&req);
            let flags = Arc::clone(&flags);
            Box::pin(async move { c.estimate_fee((*req).clone(), (*flags).clone(), b).await })
        })
        .await
    }

    async fn estimate_message_fee<M, B>(
        &self,
        message: M,
        block_id: B,
    ) -> Result<MessageFeeEstimate, ProviderError>
    where
        M: AsRef<MsgFromL1> + Send + Sync,
        B: AsRef<BlockId> + Send + Sync,
    {
        let m: Arc<MsgFromL1> = Arc::new(message.as_ref().clone());
        let b = *block_id.as_ref();
        self.dispatch(move |c| {
            let m = Arc::clone(&m);
            Box::pin(async move { c.estimate_message_fee((*m).clone(), b).await })
        })
        .await
    }

    async fn block_number(&self) -> Result<u64, ProviderError> {
        self.dispatch(|c| Box::pin(async move { c.block_number().await }))
            .await
    }

    async fn block_hash_and_number(&self) -> Result<BlockHashAndNumber, ProviderError> {
        self.dispatch(|c| Box::pin(async move { c.block_hash_and_number().await }))
            .await
    }

    async fn chain_id(&self) -> Result<Felt, ProviderError> {
        self.dispatch(|c| Box::pin(async move { c.chain_id().await }))
            .await
    }

    async fn syncing(&self) -> Result<SyncStatusType, ProviderError> {
        self.dispatch(|c| Box::pin(async move { c.syncing().await }))
            .await
    }

    async fn get_events(
        &self,
        filter: EventFilter,
        continuation_token: Option<String>,
        chunk_size: u64,
    ) -> Result<EventsPage, ProviderError> {
        let f = Arc::new(filter);
        let t = Arc::new(continuation_token);
        self.dispatch(move |c| {
            let f = Arc::clone(&f);
            let t = Arc::clone(&t);
            Box::pin(async move { c.get_events((*f).clone(), (*t).clone(), chunk_size).await })
        })
        .await
    }

    async fn get_nonce<B, A>(&self, block_id: B, contract_address: A) -> Result<Felt, ProviderError>
    where
        B: AsRef<BlockId> + Send + Sync,
        A: AsRef<Felt> + Send + Sync,
    {
        let b = *block_id.as_ref();
        let a = *contract_address.as_ref();
        self.dispatch(move |c| Box::pin(async move { c.get_nonce(b, a).await }))
            .await
    }

    async fn get_storage_proof<B, H, A, K>(
        &self,
        block_id: B,
        class_hashes: H,
        contract_addresses: A,
        contracts_storage_keys: K,
    ) -> Result<StorageProof, ProviderError>
    where
        B: AsRef<ConfirmedBlockId> + Send + Sync,
        H: AsRef<[Felt]> + Send + Sync,
        A: AsRef<[Felt]> + Send + Sync,
        K: AsRef<[ContractStorageKeys]> + Send + Sync,
    {
        let b = *block_id.as_ref();
        let h: Arc<Vec<Felt>> = Arc::new(class_hashes.as_ref().to_vec());
        let a: Arc<Vec<Felt>> = Arc::new(contract_addresses.as_ref().to_vec());
        let k: Arc<Vec<ContractStorageKeys>> = Arc::new(contracts_storage_keys.as_ref().to_vec());
        self.dispatch(move |c| {
            let h = Arc::clone(&h);
            let a = Arc::clone(&a);
            let k = Arc::clone(&k);
            Box::pin(async move {
                c.get_storage_proof(b, (*h).clone(), (*a).clone(), (*k).clone())
                    .await
            })
        })
        .await
    }

    async fn add_invoke_transaction<I>(
        &self,
        invoke_transaction: I,
    ) -> Result<InvokeTransactionResult, ProviderError>
    where
        I: AsRef<BroadcastedInvokeTransaction> + Send + Sync,
    {
        let i: Arc<BroadcastedInvokeTransaction> = Arc::new(invoke_transaction.as_ref().clone());
        self.dispatch(move |c| {
            let i = Arc::clone(&i);
            Box::pin(async move { c.add_invoke_transaction((*i).clone()).await })
        })
        .await
    }

    async fn add_declare_transaction<D>(
        &self,
        declare_transaction: D,
    ) -> Result<DeclareTransactionResult, ProviderError>
    where
        D: AsRef<BroadcastedDeclareTransaction> + Send + Sync,
    {
        let d: Arc<BroadcastedDeclareTransaction> = Arc::new(declare_transaction.as_ref().clone());
        self.dispatch(move |c| {
            let d = Arc::clone(&d);
            Box::pin(async move { c.add_declare_transaction((*d).clone()).await })
        })
        .await
    }

    async fn add_deploy_account_transaction<D>(
        &self,
        deploy_account_transaction: D,
    ) -> Result<DeployAccountTransactionResult, ProviderError>
    where
        D: AsRef<BroadcastedDeployAccountTransaction> + Send + Sync,
    {
        let d: Arc<BroadcastedDeployAccountTransaction> =
            Arc::new(deploy_account_transaction.as_ref().clone());
        self.dispatch(move |c| {
            let d = Arc::clone(&d);
            Box::pin(async move { c.add_deploy_account_transaction((*d).clone()).await })
        })
        .await
    }

    async fn trace_transaction<H>(
        &self,
        transaction_hash: H,
    ) -> Result<TransactionTrace, ProviderError>
    where
        H: AsRef<Felt> + Send + Sync,
    {
        let h = *transaction_hash.as_ref();
        self.dispatch(move |c| Box::pin(async move { c.trace_transaction(h).await }))
            .await
    }

    async fn simulate_transactions<B, T, S>(
        &self,
        block_id: B,
        transactions: T,
        simulation_flags: S,
    ) -> Result<Vec<SimulatedTransaction>, ProviderError>
    where
        B: AsRef<BlockId> + Send + Sync,
        T: AsRef<[BroadcastedTransaction]> + Send + Sync,
        S: AsRef<[SimulationFlag]> + Send + Sync,
    {
        let b = *block_id.as_ref();
        let txs: Arc<Vec<BroadcastedTransaction>> = Arc::new(transactions.as_ref().to_vec());
        let flags: Arc<Vec<SimulationFlag>> = Arc::new(simulation_flags.as_ref().to_vec());
        self.dispatch(move |c| {
            let txs = Arc::clone(&txs);
            let flags = Arc::clone(&flags);
            Box::pin(async move {
                c.simulate_transactions(b, (*txs).clone(), (*flags).clone())
                    .await
            })
        })
        .await
    }

    async fn trace_block_transactions<B>(
        &self,
        block_id: B,
    ) -> Result<Vec<TransactionTraceWithHash>, ProviderError>
    where
        B: AsRef<ConfirmedBlockId> + Send + Sync,
    {
        let b = *block_id.as_ref();
        self.dispatch(move |c| Box::pin(async move { c.trace_block_transactions(b).await }))
            .await
    }

    async fn batch_requests<R>(
        &self,
        requests: R,
    ) -> Result<Vec<ProviderResponseData>, ProviderError>
    where
        R: AsRef<[ProviderRequestData]> + Send + Sync,
    {
        let reqs: Arc<Vec<ProviderRequestData>> = Arc::new(requests.as_ref().to_vec());
        self.dispatch(move |c| {
            let reqs = Arc::clone(&reqs);
            Box::pin(async move { c.batch_requests((*reqs).clone()).await })
        })
        .await
    }
}

// ─── crate::Provider impl ─────────────────────────────────────────────────────

#[async_trait]
impl crate::Provider for MultiRpcProvider {
    async fn call(&self, call: FunctionCall, block: BlockId) -> crate::ContractResult<Vec<Felt>> {
        <Self as StarknetProvider>::call(self, &call, block)
            .await
            .map_err(|e| crate::ContractError::Provider(format!("{e}")))
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_reasonable() {
        let c = RpcConfig::market_maker_defaults();
        assert!(c.max_retries >= 1);
        assert!(c.initial_backoff < c.max_backoff);
        assert!(c.circuit_breaker_threshold >= 1);
        assert!(c.timeout_per_call >= Duration::from_millis(100));
    }

    #[test]
    fn classifies_transient_vs_deterministic() {
        let transient = ProviderError::RateLimited;
        assert!(is_transient(&transient));
        let mismatch = ProviderError::ArrayLengthMismatch;
        assert!(!is_transient(&mismatch));
    }

    #[tokio::test]
    async fn endpoint_health_reflects_initial_state() {
        let urls = vec![
            Url::parse("http://127.0.0.1:65000").unwrap(),
            Url::parse("http://127.0.0.1:65001").unwrap(),
        ];
        let provider = MultiRpcProvider::with_defaults(urls);
        let health = provider.endpoint_health().await;
        assert_eq!(health.len(), 2);
        for (_, h) in health {
            assert_eq!(h.state, EndpointHealthState::Healthy);
            assert_eq!(h.failures, 0);
            assert_eq!(h.successes, 0);
        }
    }

    #[tokio::test]
    async fn dead_endpoints_eventually_marked_down() {
        // Both endpoints point at closed ports — every dispatch should
        // fail, the failure counter should climb, and the endpoint
        // should be marked Down once the threshold is reached.
        let urls = vec![
            Url::parse("http://127.0.0.1:65000").unwrap(),
            Url::parse("http://127.0.0.1:65001").unwrap(),
        ];
        let cfg = RpcConfig {
            max_retries: 0,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(10),
            circuit_breaker_threshold: 2,
            circuit_breaker_cooldown: Duration::from_millis(100),
            timeout_per_call: Duration::from_millis(200),
        };
        let provider = MultiRpcProvider::new(urls, cfg);
        for _ in 0..6 {
            let _ = StarknetProvider::block_number(&provider).await;
        }
        let health = provider.endpoint_health().await;
        let downed = health
            .iter()
            .filter(|(_, h)| h.state == EndpointHealthState::Down)
            .count();
        assert!(
            downed >= 1,
            "expected at least one endpoint to be marked Down, got: {health:?}"
        );
    }
}
