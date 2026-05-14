# `deadeye admin deploy-math-runtime` — review

Reviewer: Driver C • Date: 2026-05-14
Scope: `deadeye-deployer/src/runtime.rs`, `deadeye-cli/src/commands/admin.rs`,
`deadeye-cli/src/cli.rs`, `deadeye-cli/tests/{cli_smoke,deploy_math_runtime}.rs`.

## Verdict — ship it (with two follow-ups landed here)

The implementation is sound. Idempotency works end-to-end on devnet, the
cache survives crashes after this review, and all four mainnet class
hashes match the canonical manifest byte-for-byte.

## Findings

1. **Class hashes — match.** The four pinned constants in
   `runtime.rs::mainnet_class_hashes` are byte-equal to the
   math-runtime entries in `deployment-mainnet-01.json` (and to the
   bundled `crates/deadeye-artifacts/abis/deployment-mainnet.json`
   under `deadeye_artifacts::MAINNET_DEPLOYMENT_BYTES`). Hex
   zero-padding differs (`0x046d…` vs `0x46d…`) but `Felt::from_hex`
   normalises it. Added a drift-guard unit test —
   `pinned_mainnet_constants_match_bundled_manifest` — that decodes both
   forms and asserts equality.

2. **Cache file format.** TOML round-trip, missing-file→empty, and
   malformed-file→typed-error all already had tests. The only honest
   gap was **concurrent / crash-mid-write corruption**: `save` used a
   plain `fs::write`. Replaced with a temp-file + atomic-rename pattern
   (`.runtimes.toml.<pid>.<nanos>.tmp` → `rename`), so a torn write or
   two CLIs racing on the same path can never leave a half-written
   file. Cleanup on rename-failure is best-effort.

3. **`--status` correctness.** Path verified:
   - empty cache → single "no cached runtimes" row, exit 0, no RPC;
   - cached + on-chain match → `cached=true, on_chain_verified=true`;
   - cached + drift → `cached=true, on_chain_verified=false`, sets
     `any_drift`, exits non-zero with a clear error;
   - missing entry → not represented in the per-row iteration (correct).

4. **`--confirm` gate.** Code-path traced and proven by 3 new CLI
   smoke tests (`cli_smoke.rs`):
   - **No `--confirm`** → emits `mode: dry_run`, cache file never
     created, no `account.execute()` reachable.
   - **`--confirm --output json`** with no `DEADEYE_PRIVATE_KEY` →
     fails *before* deploy with "private key required" — proves the
     deploy submission lives behind the OwnedAccount build.
   - The TTY prompt is correctly gated on `IsTerminal` **and**
     `OutputMode != Json`, so scripted JSON callers never block.
   - Cache-hit + verified short-circuits *before* any deploy attempt
     (covered by the devnet integration test's second invocation).

5. **Idempotency with `unique=false`.** UDC `unique=false` →
   `address = pedersen(class_hash, salt, calldata_hash, …)`, deployer
   not mixed in. The CLI's **default salt is `fresh_random_salt()`**, so
   re-running *without* `--salt` produces a fresh address each run. This
   means **the cache is the source of truth, not the salt**. That is the
   correct trade-off (a fixed-zero salt would let any other deployer
   front-run the slot once they know the class), but the docstring
   should be louder about it. Recommend documenting in the CLI help
   that "to migrate the cache between machines, copy
   `~/.config/deadeye/runtimes.toml` or pass `--salt` from the original
   deploy." No code change beyond the help text already present.

6. **Devnet integration test — pass.** Ran with
   `DEADEYE_RUN_INTEGRATION=1 DEADEYE_RUN_DEPLOY_MATH_RUNTIME=1`
   against `starknet-devnet --seed 0 --port 5050 --account-class
   cairo1`. Three CLI invocations: deploy →
   `address=0x50ef518fb430f231b9f94ee5603838789aba1fde6ad36c50179d68b874e85d3,
   on_chain_verified=true`; idempotent re-run → `cached=true`,
   same address; `--status` → array with one verified row. Total runtime
   31.7 s.

7. **`AdminCmd` exhaustiveness.** `commands/admin.rs::run` matches all
   five variants (`Settle`, `Pause`, `Unpause`, `CollectFees`,
   `DeployMathRuntime`). The `E0004` diagnostic in the brief was a
   stale snapshot — the match is exhaustive now.

8. **`cargo clippy --workspace --all-targets -- -D warnings`** — clean
   for `deadeye-deployer` and `deadeye-cli`. Pre-existing
   `clippy::pedantic` failures in `deadeye-optimizer` are unrelated
   and out of scope for this driver.

## Test counts (post-review)

- `deadeye-deployer` unit tests: **14** (was 13; +1 drift guard).
- `deadeye-cli` smoke tests: **8** running + 2 ignored (was 4; +3
  confirm/dry-run/status gates + the existing 4 markets/config tests).
- `deadeye-cli` integration test `deploy_math_runtime_devnet_idempotent`:
  passes when both env gates are set, otherwise skipped.

## Changes made

- `crates/deadeye-deployer/src/runtime.rs`: atomic rename in
  `RuntimeCache::save`; new `pinned_mainnet_constants_match_bundled_manifest`
  test.
- `crates/deadeye-cli/tests/cli_smoke.rs`: 3 new offline smoke tests
  covering the `--confirm` gate.
