# SDK concurrency — wave 1

Three production-grade reliability primitives landed in this wave:
`NonceManager`, `MultiRpcProvider`, and `BulkReader`. The chaos suites
proved correctness in isolation; these primitives are the bridge to a
many-wallet, many-market, many-RPC operating mode without regressing
that correctness.

## New public API surfaces

### `crates/deadeye-starknet/src/nonce.rs`

* `NonceManager` anchors to chain via the narrow `NonceFetcher` trait
  (`starknet_providers::Provider` gets a blanket impl). An
  `Arc<tokio::sync::Mutex<Allocator>>` lets clones share the same
  allocator cheaply. `reserve()` returns a `NonceGuard`; dropping
  releases the nonce LIFO via a min-heap of released slots so dense
  workloads do not open gaps. `commit()` consumes the guard.
* `NonceSnapshot` reports next/outstanding/released/chain_anchor for
  diagnostics. `resync()` re-anchors after node restarts.
* `OwnedAccount::with_nonce_manager(NonceManager) -> AccountWithNonceManager`
  is the integration point. The default `OwnedAccount::execute` path is
  untouched. `AccountWithNonceManager` exposes `execute_managed` (auto-
  reserve) and `execute_managed_with_gas(calls, guard, GasParams)` (pre-
  set gas, skipping the upstream `estimate_fee`).
* `GasParams::generous_defaults()` ships MM-friendly gas limits.
* Internal retry loop (8 attempts, exponential backoff) absorbs
  `InvalidTransactionNonce` errors so devnet's missing future-nonce
  mempool does not reject legitimate dense submissions.

### `crates/deadeye-starknet/src/multi_rpc.rs`

* `MultiRpcProvider::new(endpoints, RpcConfig)` and `::with_defaults(endpoints)`.
* `RpcConfig` exposes `max_retries`, `initial_backoff`, `max_backoff`,
  `circuit_breaker_threshold`, `circuit_breaker_cooldown`,
  `timeout_per_call`. `market_maker_defaults()` returns sane values.
* Implements `starknet_providers::Provider` (30+ methods) — a drop-in
  replacement for `JsonRpcClient<HttpTransport>`. Also implements
  `crate::Provider` for SDK reader interop.
* Per call: pick least-recently-used healthy endpoint, run with timeout,
  classify errors as **transient** (timeout, rate-limit, transport) vs
  **deterministic** (`StarknetError`, `ArrayLengthMismatch`). Transient
  errors retry with exponential backoff and trigger circuit-breaker
  accounting; deterministic errors return immediately and do **not**
  penalise the endpoint.
* `EndpointHealthState::{Healthy, Down, HalfOpen}` with automatic
  promotion after `circuit_breaker_cooldown`. `endpoint_health()`
  returns a snapshot for `/health` endpoints.

### `crates/deadeye-sdk/src/bulk.rs`

* `BulkReader::new(DeadeyeClient<P>)`. Consumers call
  `bulk.positions`, `bulk.lp_infos`, `bulk.distributions`,
  `bulk.market_states`.
* `Family { Normal, Lognormal, Multinoulli, Bivariate }` tags queries so
  a batch can fan out across families.
* `Position`, `DistributionSnapshot`, `MarketStateSnapshot` cover the
  per-family return types. `market_states` returns a
  `Vec<MarketStateSnapshot>` with `Option<…>` fields so partial failure
  does not poison the batch.
* Internal `futures::future::join_all` so per-query latency, not
  call-count × RTT, is the bottleneck.

## Bench numbers (localhost devnet)

| Test | Result |
|---|---|
| `nonce_stress` (50 concurrent transfers, single account, concurrency=4) | **50/50 succeeded**, ~26 tx/s end-to-end (vs ~1 tx/s baseline using serial `get_nonce`); allocator handed out 50 distinct nonces with zero gaps |
| `multi_rpc_failover` (1 dead + 1 live endpoint, 20 calls) | **20/20 served**, dead endpoint marked `Down` after 2 transient failures; recovery ≈ initial_backoff + 1 RTT (~20 ms) |
| `bulk_reader` (100× `distribution()` reads) | **100/100 succeeded**; bulk ≈ 115 ms, serial ≈ 136 ms (1.19× on localhost). Production RTT (50 ms+) projects to ≥ 10× since the bulk path collapses to ~RTT regardless of N |

## `cargo build --workspace --tests`

Clean. Two pre-existing `missing-copy-implementations` warnings on
`Participant`/`Action` in `normal_chaos.rs` (untouched). Workspace
lints (deny-`all`, deny-pedantic, deny-nursery) pass.

Unit tests: 120 passed across all crates (incl. 5 new nonce, 4 new
multi-rpc, 2 new bulk-reader). Integration tests (under
`DEADEYE_RUN_INTEGRATION=1`): 3 new — all green.

## Caveat

`NonceManager` retries `InvalidTransactionNonce` internally because
`starknet-devnet-rs` does not buffer future nonces — production
sequencers (Madara, Juno, Pathfinder) accept N+k and order them in
their mempool, so the retry path is dead code there. The stress test
runs at concurrency=4 to stay inside devnet's accepted window; the SDK
itself has no such cap.
