# CLI Wave A — Read paths + output framework

Driver A shipped `crates/deadeye-cli/` (binary name `deadeye`). It
wraps the SDK's read surface and provides the output framework
(`pretty` / `plain` / `json`) every command — including Driver B's
write paths — renders through.

## 1. Command tour

```
$ deadeye --help
Market-maker-grade CLI for the Deadeye Rust SDK

Usage: deadeye [OPTIONS] <COMMAND>

Commands:
  account   Inspect the active account / profile
  markets   Browse markets from the indexer and on-chain
  position  Read trader positions and LP shares
  config    Manage the on-disk configuration file
  trade     Trade preflight / execute / journal (Driver B)
  lp        LP add / remove (Driver B write paths)
  claim     Claim a (post-settlement) position
  admin     Admin (factory-owner) operations: …
  watch     Block-driven live stream for one market
  help      Print this message or the help of the given subcommand(s)

Options:
      --rpc-url <URL>          Override the Starknet JSON-RPC URL.
      --indexer-url <URL>      Override the indexer base URL
      --address <0x...>        Trader / account address
      --profile <NAME>         Use a named profile from ~/.config/deadeye/config.toml
      --output <MODE>          pretty | plain | json
      --no-color               Disable ANSI colors
  -v, --verbose                Enable verbose tracing (stderr; safe for JSON)
      --confirm                Skip the y/N prompt on destructive commands
```

The read commands this driver implements:

| Command | Description |
|---------|-------------|
| `account show`         | Resolved profile, address, STRK balance, chain id |
| `markets list`         | Indexer-sourced market table (`--family`, `--limit`) |
| `markets show <addr>`  | On-chain state with family auto-detection |
| `markets info <addr>`  | Indexer-side metadata (title, description, category) |
| `position list`        | Open positions for `--trader` (defaults to active address) |
| `position show <addr>` | Decoded compact-position record per family |
| `config init`          | Write `~/.config/deadeye/config.toml` |
| `config show`          | Print resolved config (private key redacted) |
| `config profile-list`  | List profiles |
| `config profile-use`   | Set the default profile |

`deadeye markets show --help` example (auto-detect documented):

```
$ deadeye markets show --help
Read a single market's on-chain state.

Family is auto-detected by trying each family's `params()` read.

# Example

    deadeye markets show 0x53e5…0fcf4

Usage: deadeye markets show [OPTIONS] <ADDRESS>
```

## 2. Sample output — `markets show` (pretty)

A live Sepolia normal-family market:

```
$ deadeye markets show 0x53e5ee2a3ff003fbcf7f96bba8370b833a06f4a8e23c055b91f1f9076a6fcf4 --output pretty

Market 0x53e5ee2a3ff003fbcf7f96bba8370b833a06f4a8e23c055b91f1f9076a6fcf4 (normal)
  family: normal
  status: init=yes paused=no settled=no
  dist.mu: 1.030000
  dist.sigma: 0.073305
  dist.variance: 0.005374
  params.k: 50.000000
  params.backing: 1000.000000
  params.tolerance: 1.000e-3
  params.min_trade_collateral: 0.010000
  params.payout_amplifier: 1.000000
  lp.total_shares: 1000.000000
  lp.total_backing_deposited: 1000.557137
  fees: lp=100bps, protocol=20bps, settlement=50bps (total 170bps)
```

(In a real terminal the header is bold cyan, keys are dimmed grey, and
`family: normal` is bold cyan via `--output pretty`.)

## 3. Sample output — `markets show` (plain, pipe-safe)

```
$ deadeye markets show 0x53e5...0fcf4 --output plain
address: 0x53e5ee2a3ff003fbcf7f96bba8370b833a06f4a8e23c055b91f1f9076a6fcf4
family: normal
is_initialised: true
is_paused: false
is_settled: false
settlement_value: 0.000000
dist.mu: 1.030000
dist.sigma: 0.073305
dist.variance: 0.005374
params.k: 50.000000
params.backing: 1000.000000
params.tolerance: 1.000e-3
params.min_trade_collateral: 0.010000
params.payout_amplifier: 1.000000
lp.total_shares: 1000.000000
lp.total_backing_deposited: 1000.557137
fees.lp_bps: 100
fees.protocol_bps: 20
fees.settlement_bps: 50
fees.total_bps: 170
```

## 4. Sample output — `markets show` (json)

```
$ deadeye markets show 0x53e5...0fcf4 --output json
{
  "address": "0x53e5ee2a3ff003fbcf7f96bba8370b833a06f4a8e23c055b91f1f9076a6fcf4",
  "family": "normal",
  "distribution": { "mu": 1.03, "sigma": 0.073304724150762, "variance": 0.00537358258281931 },
  "params": {
    "k": 50.0, "backing": 1000.0, "tolerance": 0.001,
    "min_trade_collateral": 0.01, "payout_amplifier": 1.0
  },
  "lp_info": { "total_shares": 1000.0, "total_backing_deposited": 1000.557137 },
  "fee_config": { "lp_fee_bps": 100, "protocol_fee_bps": 20,
                  "settlement_fee_bps": 50, "total_bps": 170 },
  "status": { "is_initialised": true, "is_paused": false,
              "is_settled": false, "settlement_value": 0.0 }
}
```

JSON falls through to `serde_json::to_writer_pretty(stdout, …)` so the
wire format is one grep away from the table layout. `pipe stdout |
jq …` works out of the box; tracing is on stderr so `-v` never poisons
the JSON stream.

## 5. Architecture

- `cli.rs` — clap derives, no logic.
- `output.rs` — `OutputMode::detect()` (TTY → Pretty, pipe → Plain;
  `--output` overrides; `NO_COLOR` + `--no-color` force off). The
  `Render` trait splits pretty / plain; JSON is serde.
- `render.rs` — serializable view types + `Render` impls. One per
  command output. Pretty mode uses `comfy-table` for `--output pretty`
  table rendering with `UTF8_FULL` borders.
- `config.rs` — TOML at `~/.config/deadeye/config.toml`. Resolution
  order: CLI flag → env (`DEADEYE_RPC_URL`, `DEADEYE_INDEXER_URL`,
  `DEADEYE_ADDRESS`, `DEADEYE_CHAIN_ID`, `DEADEYE_PROFILE`) → profile
  → built-in Sepolia defaults. `private_key` is **only** read from
  `DEADEYE_PRIVATE_KEY` (`config show` redacts to `***`).
- `context.rs` — per-invocation `AppContext` (resolved config +
  renderer + lazy SDK / indexer clients).
- `commands/` — one module per subcommand; family auto-detection is
  in `commands/markets.rs::detect_family` and reused by
  `commands/position.rs`.

## 6. Build / lint / test outcomes

```
$ cargo build --workspace --tests
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.26s
                                                                    → clean

$ cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.23s
                                                                    → clean

$ cargo test -p deadeye-cli --test cli_smoke
running 6 tests
test markets_list_sepolia_gated ... ignored
test markets_show_json_sepolia_gated ... ignored
test help_mentions_deadeye ... ok
test config_show_with_env_overrides ... ok
test config_init_then_show ... ok
test profile_list_json_is_valid_array ... ok
test result: ok. 4 passed; 0 failed; 2 ignored

$ cargo test --workspace --lib | tail -1
test result: ok. 65 passed; 0 failed                          (per-crate suites)

$ cargo test -p deadeye-collateral --test property
test result: ok. 5 passed; 0 failed                          (40 000 cases)
```

The pedantic lints that fight a CLI binary's shape
(`print_stdout`, `print_stderr`, `redundant_pub_crate`,
`trivially_copy_pass_by_ref`, …) are allowed only inside
`crates/deadeye-cli/src/main.rs` with a reasoned `#[allow]` block —
every other workspace crate keeps the strict posture.

## 7. Driver B handoff

Driver B owns `commands/{trade,lp,claim,admin,watch,render_helpers,
runtime_resolver}.rs` and the `Sell` arm of `position.rs`. Driver A
provided:

- `output::Renderer`, the `Render` trait, and `OutputMode::detect`.
- `config::{load, save, ResolvedConfig}` and the resolution order.
- `context::AppContext` (Driver B reuses `build_provider` /
  `build_owned_account` via `runtime_resolver`).
- `commands::markets::detect_family` (Driver B's position-sell and
  watch reuse it).
- The CLI scaffold and `confirm_or_bail` interactive prompt.

`cargo install --path crates/deadeye-cli` succeeds (release profile,
~43 s), drops `deadeye` into `~/.cargo/bin/`, and `deadeye --help`
prints the surface above on the first invocation.
