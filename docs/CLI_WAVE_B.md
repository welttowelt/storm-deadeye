# CLI Wave B — Write Paths + Streaming

Wave B layers `trade`, `position sell`, `lp`, `claim`, `admin`, and
a block-driven `watch` on top of Wave A. Shares Wave A's `Renderer` +
`OutputMode` + `ResolvedConfig`.

## 1. Commands

```
trade quote   <MKT> [--family F] (--mean M --variance V | --belief M --budget B)
              [--rho R --mu2 X] [--runtime 0x...] [--pad 0.0]
trade execute <MKT> [--family F] --mean M --variance V --max-collateral C [--journal P]
trade journal [--path P] [--tail N]
position sell <MKT> [--min-out u128]
lp add        <MKT> --amount N
lp remove     <MKT> --fraction F                     # F ∈ (0,1]
claim         <MKT> [--trader 0x...]
admin settle  <MKT> --family F (--x-star X | --outcome U | --point X1,X2)
admin pause/unpause/collect-fees <MKT> [--recipient 0x...]
watch         <MKT> [--poll-interval-ms N] [--show-quote-for "mean=..,variance=.."]
```

Globals: `--rpc-url`, `--profile`, `--output {pretty,plain,json}`, `-v`,
`--confirm`. Key from `DEADEYE_PRIVATE_KEY`. `trade quote --help` flags:
`--mean`, `--variance`, `--belief`, `--budget`, `--belief-sigma`,
`--runtime`, `--pad`.

## 2. Sample outputs

### `trade quote --output json` (devnet, μ=43, σ²=81)
```json
{
  "family":"normal","market":"0x7fd5e8a3...671a6ab",
  "candidate_mean":43.0,"candidate_variance":81.0,"candidate_sigma":9.0,
  "x_star":39.365,"required_collateral":1.039,"padded_collateral":5.0,
  "on_chain_will_accept":true,"rejection":null,
  "execute_hint":"deadeye trade execute 0x7fd5... --family normal --mean 43.0 --variance 81.0 --max-collateral 1.14"
}
```

### `trade quote` pretty:
```
✓ Preflight: chain will accept
  family: normal   candidate: μ=43.0, σ²=81.0
  x_star: 39.365   required_collateral: 1.039 STRK   padded: 5.0 STRK
  to execute, run: deadeye trade execute 0x7fd5... --family normal --mean 43.0 --variance 81.0 --max-collateral 1.14
```

### `trade execute --output json`
```json
{"action":"trade","market":"0x7fd5e8a3...","tx_hash":"0x7a2c1793...3da9d2f","call_count":1,"accepted":true,"rejection":null}
```

### `watch --output json` (NDJSON)
```
{"family":"normal","market":"0x4de6c4...","block_number":33,"mean":42.0,"sigma":8.0,"lp_total_backing":50.0,...}
{"family":"normal","market":"0x4de6c4...","block_number":34,...}
{"family":"normal","market":"0x4de6c4...","block_number":35,...}
```

## 3. Rejection → human-readable

`pretty_rejection` in `commands/render_helpers.rs` covers every variant.
Highlights:

| Variant | What this means | Suggested fix |
|---|---|---|
| `InvalidDistribution` | σ² ≠ σ·σ or σ ≤ 0 | Re-check mean/variance |
| `InvalidHints` | Sqrt hints didn't round-trip | Let `quote_trade` fetch them |
| `BackingFail` | AMM can't absorb trade | Wait for LP / reduce \|Δμ\| |
| `SigmaTooLow` / `LowCollateral` | Below market floor | Widen variance / raise collateral |
| `VerificationFailed/SideInvalid` | Side rejected | Move candidate inside policy |
| `VerificationFailed/StationaryInvalid` | `d'(x*)` off tol | Let SDK derive x* |
| `VerificationFailed/CurvatureInvalid` | `d''(x*) ≤ 0` | Raise collateral / widen σ |
| `VerificationFailed/CollateralInsufficient` | Chain re-comp > supplied | `--max-collateral` +10% |
| `VerificationFailed/MinimumInvalid` | Solver diverged | Re-quote; reduce \|Δμ\| |
| `VerificationFailed` (no sub) | Generic | Re-quote; widen σ + pad |
| `StaleState` | `expected_*` guard mismatched | Re-quote + resubmit |
| `MarketSettled`/`MarketPaused` | Trading closed | `claim`/wait unpause |
| `NoPosition`/`AlreadyClaimed`/`NoCollateralOut` | Nothing to do | Skip |
| `RequiresAdditionalCollateral` | More collateral | Raise `--max-collateral` |
| `ConversionFailed` | Q128 overflow | Smaller trade |
| `OnlyOwner`/`NotAuthorized`/`OnlyFactory` | Privilege | Use admin |
| `MinOutNotMet` | Slippage | Lower `--min-out` |
| `InvalidMinOutcome` | Multinoulli arg | Let SDK derive |
| `Reentrant`/`MarketNotInitialized` | AMM not ready | Retry/init |
| `AlreadySettled/AlreadyPaused/NotPaused` | Idempotency | No-op |
| `MarketNotSettled`/`NoClaim`/`TraderClaimsPending` | Timing | Wait |
| `InvalidMatrixMode`/`InvalidSettlementMode`/`MissingSnapshotRef` | SDK invariant | File bug |
| `Other{raw}` | Unmapped | Capture raw |

## 4. Build / clippy

```
$ cargo build --workspace --tests              → clean
$ cargo clippy --workspace --all-targets -- -D warnings → clean
```

Workspace lib tests: 65 starknet, 44 collateral, 20 sdk — all green.

## 5. Devnet results

`crates/deadeye-cli/tests/cli_write_paths.rs` gated
`DEADEYE_RUN_INTEGRATION=1`, devnet `--seed 0 --accounts 10 --port 5050`:

```
test deadeye_trade_quote_and_execute_devnet ... ok
test deadeye_watch_emits_json_updates       ... ok
test result: ok. 2 passed; 0 failed (54.6s)
```

`trade quote` → exit 0, `on_chain_will_accept: true`. `trade execute
--confirm --max-collateral 100`: tx `0x7a2c...d2f`, `accepted: true`.
`claim` on un-settled market: exit 0 with friendly `MarketNotSettled`.
`watch --max-updates 3`: 3 NDJSON lines while approve-trades minted
blocks (devnet doesn't auto-mine).
`crates/deadeye-e2e/tests/normal_lifecycle.rs` still green.

## 6. Notes

* Per-subcommand modules under `commands/`. `render_helpers.rs` owns
  `pretty_rejection`, `QuoteResult`, `SubmissionResult`, `WatchUpdate`.
  `runtime_resolver.rs` handles family/runtime/account resolution.
* `confirm_or_bail` gates only on destructive cmd + TTY + non-JSON
  + no `--confirm`. `watch` handles SIGINT via `tokio::signal::ctrl_c`;
  non-TTY = NDJSON.
* Multinoulli LP add/remove bails (writer lacks those methods).
  Quote/execute today wire normal + lognormal end-to-end; multinoulli /
  bivariate quote/execute are a follow-up. The collateral solver is
  invoked off-chain to compute the true `x_star`.
