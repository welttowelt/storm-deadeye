# FU3 + FU5 — Driver A

Two related deadeye-sdk changes shipped in v0.1.3 alongside
Driver B's FU#6 (`optimize_quote*_with_override`). No conflicts —
different functions, separate modules.

## FU#5 — `BacktestEngine::from_journal`

### API

```rust
pub fn from_journal(journal_path: &Path) -> io::Result<Self>
```

Replaces the v0.1.2 stub that returned
`Err(io::Error::other("not implemented"))`. Reads a newline-delimited
file of `deadeye_sdk::journal::JournalEntry` records and produces a
`BacktestEngine` ready for `engine.run(strategy)`.

### Schema (per-line)

Each line is a `JournalEntry` (the on-disk format `TradeJournal::append`
writes). Field requirements per `EntryKind`:

| `EntryKind`         | Required fields in `off_chain_quote`                    |
|---------------------|---------------------------------------------------------|
| `Trade` (Normal)    | `candidate.mean` (f64), `candidate.sigma` (f64, > 0)    |
| `Sell`              | none — emitted as `RemoveLiquidity { fraction: 0.0 }`   |
| `AddLiquidity`      | `padded_collateral` / `supplied_collateral` / `required_collateral` (first present) |
| `RemoveLiquidity`   | `fraction` (f64, 0..=1)                                 |
| `Settle`            | `x_star` (f64)                                          |
| `Claim`             | as `Sell` — zero-fraction `RemoveLiquidity` marker      |

Only `Family::Normal` trades are replayed today (the harness mirrors
the chain's Normal AMM solver; broader-family support arrives when
the other off-chain solvers ship).

### Failure semantics

- **Missing path** → `io::Error::NotFound` propagated unchanged.
- **Empty file** → `Ok(engine)` with `events.len() == 0`; initial state
  falls back to N(0, 1).
- **Corrupted / unparseable line** → `tracing::warn`, skipped (matches
  `TradeJournal::replay`'s permissive contract).
- **`skipped (...)` rows** (no `tx_hash`, `receipt.error` starts with
  `"skipped ("`) → filtered out before event emission so the replay
  sees only what reached the chain. Mirrors
  `cpi-bot::analytics::entry_is_skipped_for_{edge,risk}`.

The cpi-bot's `analytics::cmd_replay` workaround
(`entries_to_events`) still works unchanged; a P2 cleanup can
delegate to this SDK function.

## FU#3 — `live_effective_k` doc-comment fix

The doc-comment in `crates/deadeye-sdk/src/normal.rs` had the two
backings swapped. The function body has always been correct
(`max(base_k, base_k * pool / initial)`); only the comment lied.

### Before

```text
pool_backing    := params.backing (live, post-LP-flows).
initial_backing := lp_info.total_backing_deposited (the cumulative
                   backing deposited gross of withdrawals — i.e. the
                   immutable reference against which `k` is scaled).
```

### After

```text
pool_backing    := lp_info.total_backing_deposited (live; numerator).
initial_backing := params.backing (immutable; denominator). The
                   on-chain `params.backing` ABI field name is a
                   leftover misnomer.
```

Per `REVIEW_ITEM5` §1 (Cairo storage + on-chain math runtime + TS
indexer all agree).

## Tests

**FU#5** — 7 new tests in `backtest::tests`:

- `from_journal_parses_trade_entries`
- `from_journal_skips_skipped_entries`
- `from_journal_handles_corrupted_lines`
- `from_journal_handles_empty_file`
- `from_journal_propagates_io_error_on_missing_path`
- `from_journal_handles_claim_and_sell_entries`
- `entry_is_skipped_predicate_matches_analytics_layer`

**FU#3** — 2 new tests in `normal::tests`:

- `live_effective_k_convention_pool_is_current_initial_is_historical`
  — pins (`base_k=50, pool=20_000, initial=10_000`) → `effective_k =
  100`, and asserts the swapped pairing floors at `base_k = 50`.
- `live_effective_k_mainnet_ratio_scales_above_base` — mainnet CPI
  YoY ratio (`pool ≈ 1.0009 × initial`) rises above `base_k = 75`
  (matches the byte-exact 75.06806 from `REVIEW_ITEM5` §2).

`cargo test -p deadeye-sdk --all-features --lib` → **43 / 43 pass**
(8 + 9 pre-existing + the new ones). `cargo check -p cpi-bot` clean;
`cargo test -p cpi-bot --bins` → **227 / 227 pass** (bot's workaround
keeps working unchanged).

`cargo clippy -p deadeye-sdk --all-features --lib -- -D warnings`
clean. Test-target clippy has 4 pre-existing failures in Driver B's
`validate_effective_k_override_*` tests (Result-state asserts) —
flagged for Driver B; not introduced here.

## Version bump

- `crates/deadeye-sdk/Cargo.toml` already at `0.1.3` (Driver B's
  FU#6).
- Workspace `Cargo.toml` `[workspace.dependencies]` pin bumped
  `0.1.2 → 0.1.3` so consumers resolve the same version.
- `CHANGELOG.md` v0.1.3 section extended with `from_journal` under
  *Added* and the doc-comment correction under *Fixed*.
- **Do not publish**; coordinate at end of FU3 + FU5 + FU6.

## Constraints honoured

- `-D warnings` lib-clean (all my code).
- No optimizer bump.
- No change to `optimize_quote*` (Driver B's territory).
- cpi-bot's `analytics::cmd_replay` workaround still works as-is.
