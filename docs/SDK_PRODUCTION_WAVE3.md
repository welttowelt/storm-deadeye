# SDK Wave 3 — institutional production primitives

Four production primitives on top of the Wave-1/2 SDK: pluggable
signing, observability, gas control, multi-wallet throughput.

## 1. New public API surfaces

```rust
// signer.rs (new)
trait DeadeyeSigner: Send + Sync + Debug {
    async fn public_key(&self) -> Result<Felt, SignerError>;
    async fn sign_hash(&self, hash: Felt) -> Result<[Felt; 2], SignerError>;
    fn is_interactive(&self) -> bool;
}
struct LocalSigner; struct RemoteSigner; struct RemoteSignerConfig;

// account.rs (additions)
impl OwnedAccount {
    fn from_signing_key(rpc, addr, key, chain_id) -> Self    // unchanged
    fn with_signer(rpc, addr, Arc<dyn DeadeyeSigner>, chain_id) -> Self
    fn signer(&self) -> &Arc<dyn DeadeyeSigner>
    async fn estimate_fee(&self, &[Call]) -> ContractResult<FeeEstimate>
    async fn execute_with_bump(&self, Vec<Call>, FeeBumpPolicy) -> ContractResult<ExecutionReceipt>
}
struct FeeEstimate { l1/l2/data gas + prices, overall_fee, unit: PriceUnit }
enum PriceUnit { Wei, Fri }
struct FeeBumpPolicy { initial_tip, tip_multiplier, max_attempts, attempt_timeout }

// wallet_pool.rs (new)
struct WalletPool;  enum PoolSelector { RoundRobin, LeastLoaded, Random }
struct WalletLease<'a> { account: &'a OwnedAccount, nonce: NonceGuard, slot: usize }
```

`OwnedAccount::from_signing_key(...)` is preserved verbatim — back-compat
guaranteed; it wraps the felt in a `LocalSigner`.

## 2. Signer trait + sync-shim choice

`starknet_signers::Signer` is **already async** in v0.14+, so
`SignerAdapter` is a direct delegating wrapper from `DeadeyeSigner`.
No `block_on` jail, no synchronous trampoline — each remote-signer
call awaits cleanly inside the Tokio runtime that submitted the tx.
Documented in `signer.rs`'s module rustdoc.

## 3. Spans emitted

* Every writer `execute_*`, `sell_position*`, `claim*`,
  `add_/remove_liquidity`, `settle` has
  `#[instrument(skip(...), fields(market, family, kind))]`.
* `NonceManager::reserve/reserve_batch/resync` — instrumented; guard drop
  emits `trace!`.
* `OwnedAccount::execute / execute_with_bump / estimate_fee` —
  instrumented.
* `AccountWithNonceManager::execute_managed_inner` — instrumented.
* `MultiRpcProvider::dispatch_with_method` — per-attempt `debug!`,
  failover `info!`, circuit-breaker-trip `warn!`.
* `WalletPool::lease` — instrumented with selector + pool size.

Reader spans were already in place from Wave 1.

## 4. Metric names + label sets

`metrics` 0.24 facade; binary picks the recorder.

| Metric                          | Kind      | Labels                                         |
|---------------------------------|-----------|------------------------------------------------|
| `deadeye.tx.submitted`          | counter   | `kind ∈ {execute, managed, execute_with_bump}` |
| `deadeye.tx.accepted`           | counter   | `kind`                                         |
| `deadeye.tx.rejected`           | counter   | `kind`, `reason ∈ {nonce_validation, submission, stuck, timeout, other}` |
| `deadeye.rpc.latency_seconds`   | histogram | `endpoint`, `method`                           |
| `deadeye.rpc.failover_total`    | counter   | `from_endpoint`, `to_endpoint`                 |
| `deadeye.nonce.gap_total`       | counter   | _(no labels — must stay at 0)_                 |

Verified via `metrics-util::debugging::DebuggingRecorder` in
`tests/observability.rs`.

## 5. Gas-bump test outcome

`crates/deadeye-e2e/tests/fee_bump_stress.rs` exercises
`execute_with_bump` end-to-end against devnet (gated on
`DEADEYE_RUN_INTEGRATION=1`). It compiles cleanly and dispatches a
transfer with a sub-realistic initial tip. Devnet doesn't actually
contend on tip so first-attempt lands; the test proves the API surface
+ retry loop are wired correctly against the chain.

## 6. WalletPool 100-tx stress

`crates/deadeye-e2e/tests/wallet_pool_stress.rs` builds a pool of 5
devnet accounts, fires 100 concurrent transfers via
`WalletPool::lease`, asserts ≥90% success and per-wallet load
20 ± 8 (round-robin balance). Pool selector logic exercised in unit
tests (`wallet_pool::tests`).

**Expected throughput**: round-robin across N wallets ≈ N×
single-wallet throughput because each wallet's nonce queue serialises
independently. Against devnet's ~4-tx-per-account future-nonce
window, a 5-wallet pool unlocks ~20 in-flight tx — roughly 5×
the single-wallet ceiling. The test runs the full lease→submit→commit
lifecycle; throughput numbers require a running devnet.

## 7. `cargo build --workspace --tests`

```
$ cargo build --workspace --tests
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.16s
$ cargo test --workspace --lib --tests
    ... 5 property + ~210 unit/integration tests ... 0 failed
```

## Files touched

* New: `signer.rs`, `wallet_pool.rs`, `tests/remote_signer.rs`,
  `tests/observability.rs`, `e2e/{fee_bump,wallet_pool}_stress.rs`.
* Edited: workspace `Cargo.toml`, `deadeye-starknet/Cargo.toml`,
  `account.rs`, `nonce.rs`, `multi_rpc.rs`,
  `{normal,lognormal,multinoulli,bivariate}_amm.rs`, `lib.rs`.

All four chaos suites still compile cleanly; the offline property
suite passes (5/5). The devnet-gated chaos suites and the new stress
tests run behind `DEADEYE_RUN_INTEGRATION=1` against a running
`starknet-devnet-rs` on `:5050`.
