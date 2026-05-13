# SDK strategy — wave 2 review (Driver C)

## 1. `payout_at` correctness

Driver's `payout_at_*` matches chaos-suite closed-form
`λ · pdf(x*; μ, σ, …)` to ≤ 4 ulp (bit-exact in practice) for all
four families. Added bit-exact tests to
`crates/deadeye-starknet/src/pricing.rs`:

- `payout_at_{normal,lognormal,bivariate,multinoulli}_bit_exact_vs_chaos_form`

Each runs 3 distributions, picks `k = λ · ‖p‖₂`, then asserts
`|driver − chaos| ≤ 4·ε·payout`. All pass. The bivariate path uses
the same `dist.pdf(x1,x2)` call as
`bivariate_chaos::closed_form_payout`. Math identity for normal:
`λ·pdf = k·√(2σ√π)·exp(-z²/2)/(σ·√2π) = k/√(σ√π)·exp(-z²/2) = f_at`
— same value LP-claim code uses.

## 2. Bench variance (3 release runs)

`payout_at_normal_meets_perf_budget` — 200 000 iterations, M-class
chip, release:

| Run | ns/call |
|----:|--------:|
| 1   | 2.8     |
| 2   | 2.8     |
| 3   | 2.7     |

Faster than the claimed 3.5 ns; variance ≤ 0.1 ns. Honest: input
and return are wrapped in `black_box`, `acc` is consumed by
`eprintln!`, so the call isn't DCE'd. Distribution is constructed
once outside the timed loop — disclosed; representative of real
strategy use.

## 3. `spread_at` skip verdict: **principled**

The reviewer's "bid = cost of −ε / ask = cost of +ε" construction
*is* state-dependent and asymmetric — but that's exactly the
driver's point. Synthesising it requires inverting the collateral
solver (1D root-find wrapping `normal_collateral`), costing as much
as the trade itself, with no shared-book meaning. Driver exposed
`impact_for_mu_shift(±ε)` so strategies can compose a notional
spread on demand. Skip is correct, not lazy.

## 4. `sensitivities_at` numerical validation

- `FD_EPS = 1e-4`. The reviewer's hint `h≈√ε≈1e-8` is the
  **forward-diff** optimum; for **central diff** the optimum is
  `ε^(1/3) ≈ 6e-6`. `1e-4` is a hair high but yields ~1e-8
  relative truncation — fine.
- Central, not forward: confirmed (`(plus − minus)/(2·eps)`, O(h²)).
- Closed-form match: added
  `sensitivities_normal_matches_closed_form_{d_mu,d_sigma}`. For
  `payout = λ·pdf`:
  - `∂payout/∂μ = (x−μ)/σ² · payout`
  - `∂payout/∂σ = payout · [−1/(2σ) + (x−μ)²/σ³]`

  FD matches both to **rel err < 1e-6** across 3 x* — confirms
  central-diff accuracy.

## 5. Stream + journal soundness

**Stream.** `tokio::time::interval` with `MissedTickBehavior::Skip`.
Polls `block_number()`; yields only on block-height transition.
Subscriber-drop is caught two ways: `tx.send` returns Err *and* the
DropGuard notifies. All 3 stream unit tests pass.

**Bug fixed**: `DropGuard::drop` used `notify_waiters()`, which
silently drops the signal if no task is parked at
`.notified().await`. Switched to `notify_one()`, which persists a
permit. Pre-fix the shutdown relied on the channel-closed
fallback — works, but non-deterministic.

**Journal.** Replay skips empty lines; bad/truncated lines yielded
as Err (caller filters — matches the documented intent). Felts as
`"0x..."` hex; serde round-trip verified.

**Bug fixed**: `append` only called `BufWriter::flush` — leaves
data in the OS page cache. We now call `sync_data()` after flush;
survives kernel/power crash, not just process crash. Adds ≈
0.1–1 ms/append, dwarfed by RPC RTT.

## 6. Bugs + fixes

1. `crates/deadeye-sdk/src/stream.rs` — `DropGuard::drop`:
   `notify_waiters` → `notify_one`.
2. `crates/deadeye-sdk/src/journal.rs` — `TradeJournal::append`:
   adds `sync_data()` after flush.
3. `crates/deadeye-starknet/src/pricing.rs` — 6 new tests
   (4 bit-exact-vs-chaos, 2 analytical-FD).

No public-API breaks; build clean (only pre-existing chaos warnings).

## Runs

- Unit: `cargo test -p deadeye-{sdk,starknet}` — 86 passed (was 80).
- Devnet: `normal_chaos` **PASS** (26 s); `quote_stream` **PASS**
  (25 s); `journal_roundtrip` **PASS** (26 s).
