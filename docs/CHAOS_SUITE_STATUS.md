# Chaos suite — status & handoff

Status as of 2026-05-11. The full sub-agent-orchestrated chaos pipeline
ran end-to-end: 8 driver agents produced independent test files for the
4 distribution families, 8 reviewer agents flagged concrete bugs in
those drafts, and 4 integration agents merged each pair into a canonical
file with reviewer fixes applied.

## What ships

Canonical chaos tests, one per distribution family:

| File | LOC | Example question | Participants | Phases |
|------|-----|------------------|--------------|--------|
| `crates/deadeye-e2e/tests/normal_chaos.rs`        | 1185 | Anthropic Opus 4.7 ARC-AGI-3 score (%) on 2026-12-31. N(μ=42, σ²=64). | 5 (Eve / Alice / Bob / Charlie / Dana) | 12 + settle + claims |
| `crates/deadeye-e2e/tests/lognormal_chaos.rs`     | 1195 | BTC/USD spot 2026-12-31. μ=ln(80_000), σ²=0.04. | 6 (TraderBull / TraderBear / LpEarly / LpLate / Hybrid / AdminSettler) | 14 + settle + claims |
| `crates/deadeye-e2e/tests/multinoulli_chaos.rs`   | 1256 | 6-outcome election market, non-uniform priors [0.10,0.25,0.30,0.05,0.20,0.10]. | 6 | 15 actions across dense/sparse/transfers + longshot settle |
| `crates/deadeye-e2e/tests/bivariate_chaos.rs`     | 1139 | (eval %, p50 latency ms) for Opus 2026-12-31. μ=(82,120), σ²=(64,900), ρ=−0.4. | 5 | 8 sweep + 4 ρ-only + 2 named adversarial + settle + 5 claims |

All four files:
* build clean with the workspace strict lint posture (`-D warnings`
  except 2 cosmetic `missing_copy_implementations` in `normal_chaos.rs`).
* are gated with `#[ignore = "..."]` so CI shows them as **skipped**
  rather than false-green until the upstream blockers resolve.
* hold **hard** `assert!` invariants — none of the reviewer-identified
  green-CI hazards (`eprintln!`-instead-of-`assert!`) remain.

## Supporting testkit

| Module | Purpose |
|--------|---------|
| `fixture::artifacts` | Sierra+CASM loader for all 15 Deadeye contracts. |
| `fixture::declare` | Idempotent declare with chain-error-discovery cache (`/tmp/deadeye_casm_hashes.json`). |
| `fixture::deploy` | UDC deploys (Legacy UDC at `0x041a78…02bf`). |
| `fixture::factory_setup` | `configure_market_type`, `upsert_deploy_profile`. Discriminants verified: Normal=1, Multinoulli=2, Lognormal=4, BivariateNormal=5. |
| `fixture::erc20` | `balance_of`, `transfer`, `approve`, `operator_mint`, etc. STRK at `0x04718f…c938d` is the test collateral. |
| `fixture::env::bootstrap_devnet` | One-shot reset → declare 15 → deploy factory → 4 plugins → **4 math-runtime instances** → configure 4 market types. Returns a `TestEnv` with per-family `normal_runtime` / `lognormal_runtime` / `multinoulli_runtime` / `bivariate_runtime` addresses. ~25 s. |
| `fixture::lifecycle` | `upsert_<family>_profile_for_test`, `deploy_<family>_market_with_event`, `fetch_<family>_hints`, `initialize_market`, `build_initial_<family>_inputs` for all 4 families. |

## Verified invariants per chaos suite

All four suites assert (when helpers wired):

* **Collateral conservation** — exact u128 zero-drift between participant
  balances + market balance + treasury balance on every non-settlement
  phase. Settlement conservation `|Σ payouts − backing| < 1e-3 · backing`.
* **No participant drains below floor** (`≥ 1 STRK`).
* **Dust check** — market drains to `≤ 1000` base units after the
  full claim sweep.
* **Round-trip P&L ≤ 0 (modulo dust)** — a participant who returns to
  the initial distribution cannot have made tokens. Loosened to
  `after ≤ before + ROUND_TRIP_DUST` (100 base units) to absorb chain
  rounding without conflating solver bugs with rounding noise.

Per-family invariants:

* **Normal** — `assert_sigma_safe` (σ-ratio ≤ 4×, |Δμ| < 4σ, no
  equal-variance saddle) gates every candidate.
* **Lognormal** — `LpBoundedScenario` records per-LP `(pool_share,
  entry_μ, entry_σ, entry_k, supplied_backing)` slices, rescaling prior
  open slices on new opens. LP P&L closed-form reconstruction via
  `deadeye_optimizer::lp::compute_lp_claim_component_value`.
* **Multinoulli** — λ-invariant per trade (`|λ − k/‖p‖₂| < 1e-4 abs OR
  rel 1e-6`); Σp=1 + p∈[0,1] preflight; argmax-flip hard-asserted with
  near-tie gap guard (only on inversion trades); transfer-list ↔
  `new_probs` drift guard via `assert_eq!`.
* **Bivariate** — compile-time axis-move ≥2 guard (≥1 in ρ-only mode);
  σ-asymmetric-corner stress (σ₁/σ₂=4 AND step-stretch ≥ 4 in the same
  phase); ρ-only 4-step sweep that fixes marginals and perturbs only ρ
  (−0.4 → 0 → +0.4 → +0.9); per-role position accumulator
  (`Vec<CompactPos>`) so the **oldest** position's ρ can be checked
  against survival of mid-life ρ-flips.

## Open blockers — RESOLVED 2026-05-11

All five upstream blockers were resolved by a 4-subagent session
(2 drivers + 2 reviewers):

1. **`initialize_market` u256_sub Overflow — ROOT-CAUSED.** Not a Cairo
   bug. The OZ ERC20 `transferFrom` does an unchecked
   `_balances[from] - amount` subtraction that underflows when the
   admin's balance is less than the requested `backing × 10^token_decimals`.
   On devnet the admin starts with 1000 STRK and burns ~30 STRK on
   bootstrap gas, so a profile `backing: sq(1000.0)` was 34 STRK short.
   **Fix:** `upsert_normal_profile_for_test` default changed from
   `backing=1000` to `backing=50`; bivariate chaos LP seed reduced from
   1000 → 50 STRK. Diagnosis: `docs/INITIALIZE_OVERFLOW_DIAGNOSIS.md`.
   Review: `docs/INITIALIZE_OVERFLOW_REVIEW.md`.

2. **Lognormal writer paths — SHIPPED.** `LognormalMarketWriter` now
   exposes `execute_trade`, `sell_position_guarded`, `claim`,
   `claim_for`, `add_liquidity`, `remove_liquidity`. `lognormal_chaos.rs`
   converted 3 `// once writer lands` TODOs to live calls; the LP
   `LpBoundedScenario` rel-tol assertion is active.

3. **Bivariate writer paths — SHIPPED.** `BivariateMarketWriter` now
   exposes all six normal-mirror methods plus a per-market `settle`.
   `bivariate_chaos.rs` flipped `has_real_helpers() = true`, converted
   6 conversion sites (initialize, PureLp seed, 8 three-axis trades,
   4 ρ-only trades, S2 + S3 named adversarials, settle, 5 claims).

4. **Per-trader `position(...)` reader** — already present on all 4 family
   readers; the ρ-round-trip assertion in bivariate_chaos.rs upgrades
   from "Sq128 round-trip only" to true on-chain preservation when
   `BivariateMarketReader::position(trader)` returns.

5. **`lp_info()` reader** — present on all 4 family readers. Bivariate
   snapshot's `lp_total_shares` / `lp_total_backing` now populate.

**Bonus fix:** `FeeConfigRaw` width was `u32` in Rust vs `u16` in the
Cairo ABI (latent wire-format bug; worked by accident with zero fees).
Tightened to `u16` in `crates/deadeye-starknet/src/types/common.rs`.

**Bonus fix from Reviewer 2:** Driver 2's first pass mirrored two
calldata-shape bugs from `NormalMarketWriter`:
- `add_liquidity` / `remove_liquidity` was encoding 7 felts
  (`share_amount + hints`); ABI declares only `share_amount` (5 felts).
- `sell_position_guarded` was encoding only `guards`; ABI declares
  `(candidate, x_star, candidate_hints, guards)`.
Both fixed across all 3 writers (normal, lognormal, bivariate) so the
calldata now matches the ABI. Review: `docs/WRITERS_REVIEW.md`.

**To run the chaos suite:**

```bash
starknet-devnet --seed 0 --accounts 10 --port 5050 &
DEADEYE_RUN_INTEGRATION=1 cargo test -p deadeye-e2e --release -- --ignored --nocapture
```

The `#[ignore]` attributes remain on each chaos test as a safety until
a green devnet run validates the writers end-to-end against a running
chain. Drop them after one successful execution.

## Outstanding follow-ups (non-blocking)

- `min_token_out` typed as `u128` in lognormal/bivariate guards but
  declared `u256` in Cairo — flagged by Reviewer 2.
- Multinoulli `sell_position_guarded` has the same 3-arg omission as
  the other families' first pass — flagged by Reviewer 2.
- Bivariate's inline `settle` selector should move into
  `selectors::amm` — flagged by Reviewer 2.
- Tests that exercise production-style backings (≫ 50 STRK) need
  explicit admin pre-funding via `erc20::transfer` from a richer
  predeployed account or restricted-collateral `operator_mint` —
  flagged by Driver 1.

## How to resume

```bash
# 1. Start a fresh devnet (separate terminal)
starknet-devnet --seed 0 --accounts 10 --port 5050

# 2. From deadeye-rs root:
DEADEYE_RUN_INTEGRATION=1 cargo test -p deadeye-e2e --test devnet_bootstrap -- --nocapture

# Once blockers lift, drop --ignored:
DEADEYE_RUN_INTEGRATION=1 cargo test -p deadeye-e2e --test normal_chaos -- --ignored --nocapture
DEADEYE_RUN_INTEGRATION=1 cargo test -p deadeye-e2e --test lognormal_chaos -- --ignored --nocapture
DEADEYE_RUN_INTEGRATION=1 cargo test -p deadeye-e2e --test multinoulli_chaos -- --ignored --nocapture
DEADEYE_RUN_INTEGRATION=1 cargo test -p deadeye-e2e --test bivariate_chaos -- --ignored --nocapture
```

## Notable bugs / surprises captured along the way

1. **starknet-rs 0.16 CASM-hash incompatibility with Cairo 2.14.0** — handled via chain-error-discovery cache in `fixture::declare`.
2. **Bivariate Cairo struct field order** — `mu1, mu2, var1, var2, sigma1, sigma2, rho, inv_one_minus_rho_sq, normalization` (was assumed `mu/var/rho/sigma`).
3. **`decimal_shift` is u8, not i8**.
4. **MarketKind discriminants are non-contiguous** — 3 reserved for skew-normal.
5. **Restricted collateral token semantics** — `transfer` requires sender/recipient on system-transfer allowlist unless one is a registered market. Switched to predeployed STRK to sidestep.
6. **starknet-devnet 0.7 dropped `/predeployed_accounts`** — moved to JSON-RPC `devnet_getPredeployedAccounts`.
7. **Hint sqrt formulas in the contract are `sqrt(2σ√π)` and `sqrt(σ√π)`** — the on-chain check is byte-for-byte against Q128 sqrt, so f64 sqrt won't round-trip. Use `compute_hints_view` on the runtime to get authoritative hints.
8. **σ-ratio off-by-one** — `(1.0..4.0).contains(&ratio)` excludes ratio=1.0; fixed to `(1.0..=4.0).contains(&ratio)`.
9. **Treasury double-count in conservation** — when admin is also in the participants vector, summing `Σ Δparticipants + Δtreasury` double-counts admin's STRK. Fixed via `treasury_is_participant: bool` flag.
10. **`saturating_sub` in P&L tracking** — masks losses as zero. Fixed to `i128` deltas.
11. **`LpExposureSlice.pool_share` capture without dilution** — fixed: `LpBoundedScenario::open` rescales all open slices on each new join.
12. **λ-tolerance too tight** — `1e-6` absolute fails on Sq128→f64 sqrt round-trip; relaxed to `1e-4` abs OR `1e-6 * expected` relative.
13. **`#[ignore]` discipline** — every chaos test must be `#[ignore]`-gated until blockers resolve, otherwise green-CI lies about coverage.
