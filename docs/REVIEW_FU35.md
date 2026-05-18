# Review — FU3 + FU5 — Driver A

Reviewer beat: schema fidelity, corruption-recovery semantics,
doc-comment correctness against the chain.

## 1. Schema fidelity vs cpi-bot

The SDK *owns* `JournalEntry` (`deadeye_sdk::journal::EntryKind` enum)
and cpi-bot imports it — there is no schema mirror, so by definition
the writer and reader cannot drift. All six `EntryKind` variants —
`Trade`, `Sell`, `AddLiquidity`, `RemoveLiquidity`, `Claim`, `Settle`
— are handled in `journal_entry_to_event` (`backtest.rs:454-512`).
The bot's `approve` CLI is **not journaled** (verified — no
`EntryKind::Approve` exists and `approve.rs` doesn't touch the journal),
so there's nothing to drop.

`error_note` is serialized into `receipt.error`. The SDK's
`entry_is_skipped` predicate matches the bot's analytics-layer
`entry_is_skipped_for_{edge,risk}` (test
`entry_is_skipped_predicate_matches_analytics_layer` pins this).

## 2. Corruption recovery

Driver A's tests cover the four corruption modes I'd flag:

| Mode | Test | Verdict |
|------|------|---------|
| Bad JSON line | `from_journal_handles_corrupted_lines` (real torn-write `{"timestamp":"truncated`) | OK |
| Empty file | `from_journal_handles_empty_file` | OK |
| Missing path → `Err(NotFound)` | `from_journal_propagates_io_error_on_missing_path` | OK |
| Skipped entries → no event | `from_journal_skips_skipped_entries` | OK |

Unknown `EntryKind` discriminant is not separately tested but is
covered by the serde-fail path (an unknown tag fails deserialization
→ `warn` + skip), and adding a fixture would just duplicate
`from_journal_handles_corrupted_lines`. Acceptable.

## 3. Doc-comment correctness — **DOC OK, BUT CALL-SITE WAS STILL SWAPPED**

The doc on `live_effective_k` (`normal.rs:137-161`) now reads
correctly: `pool_backing := lp_info.total_backing_deposited`
(numerator), `initial_backing := params.backing` (denominator).
This matches FU1 Reviewer A's verdict, Cairo
(`onchain-normal-amm/contract.cairo:178, 412, 513`:
`params.backing = self.initial_backing`,
`lp_info.total_backing_deposited = self.total_lp_backing`), and the
cpi-bot's `effective_k::chain_read_effective_k` (`effective_k.rs:397-398`).

**But the SDK call site contradicted the doc.** At
`normal.rs:600-601` (pre-fix):

```rust
let pool_backing    = Sq128::from_raw(params.backing);                  // wrong
let initial_backing = Sq128::from_raw(lp_info.total_backing_deposited); // wrong
```

This is the exact "swapped mapping" REVIEW_ITEM5 §1 warned about:
when LPs have grown the pool (`total_backing_deposited > params.backing`),
the SDK silently floors `effective_k` to `base_k` instead of scaling
above it. The bot's `optimize_quote_offline` users (`execute.rs:349`)
would have been under-pricing the chain's λ-scaled collateral.

**The devnet parity test
(`offline_optimize_quote_parity.rs`) doesn't catch this** because it
runs against a freshly-initialised market where
`total_lp_backing == initial_backing` (ratio = 1, both wirings agree).
The bug surfaces only after the first `add_liquidity` mutation.

**Fix applied** (`normal.rs:599-612`): swap the local-var assignments
and add a comment pinning the convention to the doc + Cairo source
line numbers. Also added a call-site wiring pin test
`live_effective_k_call_site_wiring_matches_chain_after_lp_grow` that
mimics the call-site argument mapping with a 2× pool/initial ratio
and asserts `effective_k ≈ 2 × base_k`.

## 4. Test quality

| Test | Tautological? | Notes |
|------|---------------|-------|
| `from_journal_parses_trade_entries` | No | Hand-written via `TradeJournal::append`, asserts seed comes from first Normal trade (μ = 2.0). |
| `from_journal_handles_corrupted_lines` | No | Real raw-write torn-line fixture. |
| `from_journal_skips_skipped_entries` | No | Realistic `receipt.error: "skipped (edge too thin)"` payload. |
| `from_journal_handles_claim_and_sell_entries` | No | Pins the zero-fraction-`RemoveLiquidity` marker convention. |
| `live_effective_k_convention_pool_is_current_initial_is_historical` | No | Hand-computed (base=50, pool=20k, initial=10k → 100). Asymmetric inputs catch a swap. |
| `live_effective_k_mainnet_ratio_scales_above_base` | No | Hand-pinned 75.0680585 from REVIEW_ITEM5 §3. |

All non-tautological. The convention pin tests use values where
`pool ≠ initial`, so a swap is observable.

## 5. Bugs found + fixes

**B1 (P0, fixed)** — `optimize_quote_offline` call-site swap. See §3.
Fix: `normal.rs:599-612` (swap assignments) + new test
`live_effective_k_call_site_wiring_matches_chain_after_lp_grow`.

**P3 follow-up (not done)** — cpi-bot's `analytics.rs::entries_to_events`
is now strictly redundant with `BacktestEngine::from_journal`. The bot
could delegate; flagged for cleanup but left untouched per scope. Note
the bot drops `Sell`/`Claim` while the SDK emits zero-fraction
`RemoveLiquidity` markers — when the bot migrates, expect a few
analytics tests to need new assertions for the extra events.

**P3 follow-up (not done)** — `offline_optimize_quote_parity` devnet
test should add a scenario that runs `add_liquidity` between
deploy and `optimize_quote` so chain/SDK divergence under LP-grow is
caught at integration time (not just by the wiring pin unit test).

## 6. Final test count

- `cargo test -p deadeye-sdk --lib --all-features` →
  **44 / 44** pass (43 prior + 1 wiring pin).
- `cargo test -p cpi-bot --bins` → **227 / 227** pass (bot's
  analytics workaround unchanged; no regression).
- `cargo clippy -p deadeye-sdk --all-features --lib -- -D warnings`
  clean.

## 7. Schema documentation

Doc on `from_journal` (`backtest.rs:194-247`) includes:

- A per-`EntryKind` field table.
- Failure-mode bullets (missing / empty / corrupt / non-Normal /
  non-finite / skipped).
- Pointer to `TradeJournal::replay`'s permissive contract.
- Explicit `# Errors` section.

The SDK does not link to a canonical journal fixture file, but the
schema *is* the SDK's own `JournalEntry`, so there's nothing
external to point at. Acceptable.

## 8. Performance

`from_journal` uses `BufReader::lines()` — streaming, O(line) memory.
A 10 MB synthetic-journal stress test would be a P3 add-on but
the implementation is correct.
