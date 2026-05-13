# Wave 1 validation — 2026-05-12

Both reviewer agents timed out on the devnet test phase (10-min watchdog).
Validation re-run directly. Results:

## Chaos suites — all PASS (no regressions from Wave 1 changes)

| Suite | Result | Duration |
|------|--------|----------|
| `normal_chaos` | ✅ ok | 25.46s |
| `lognormal_chaos` | ✅ ok | 26.26s |
| `multinoulli_chaos` | ✅ ok | 26.05s |
| `bivariate_chaos` | ✅ ok (2 filtered) | 27.38s |

## New Wave 1 stress tests — all PASS

| Test | Result | Duration | Notes |
|------|--------|----------|-------|
| `nonce_stress::nonce_manager_fires_50_concurrent_trades` | ✅ ok | 26.47s | 50/50 trades, ~26 tx/s, zero nonce gaps |
| `bulk_reader::bulk_reader_beats_serial_baseline` | ✅ ok | 26.02s | Parallel fan-out outperforms serial baseline |
| `multi_rpc_failover::multi_rpc_recovers_when_primary_dies` | ✅ ok | 0.05s | 20/20 calls served by live endpoint after dead endpoint's ECONNREFUSED |

## Build + doc

- `cargo build --workspace --tests`: clean (2 pre-existing `missing_copy_implementations` warnings on `normal_chaos.rs` only).
- `cargo doc --no-deps --workspace`: clean (1 pre-existing warning on `deadeye-testkit`).

## Wave 1 surface summary

**Driver A (ergonomics)** — typed `TradeError` + `parse_revert_reason` (24 Cairo strings mapped), `quote_trade()` + `execute_quote()` per family, `sell_position(runtime, min_token_out)` (chaos sell sites reduced 53→19 / 52→16 lines), factory `settle_<family>` + `pause_market_typed` + `collect_protocol_fees`, rustdoc worked examples on 5 modules.

**Driver B (concurrency)** — `NonceManager` + `NonceGuard` reservation w/ commit/rollback, `MultiRpcProvider` with circuit breaker + exponential backoff, `BulkReader` for parallel position/lp_info/distribution/market_state queries.

## Follow-ups (non-blocking)

1. Multi-RPC failover test uses a closed port to simulate "down" — replace with a real mid-flight `pkill` test that starts both devnets up, runs 50 calls, kills one, verifies the remaining 50 succeed.
2. The `MultiRpcProvider` 0.05s test runtime is correct (ECONNREFUSED is instant) but should be measured against an unresponsive endpoint too (network timeout) — currently `timeout_per_call=2s` would dominate.
3. Driver B notes `concurrency_window=4` is needed because devnet doesn't buffer future nonces; production sequencers (Madara/Juno/Pathfinder) do. The SDK itself has no cap.

Wave 2 (items 8-14: pricing primitives, quote stream, portfolio aggregate,
trade journal, backtest harness, property tests, scale integration) ready
to dispatch.
