# deadeye-cli

The `deadeye` binary — a market-maker-grade CLI for the
[Deadeye Rust SDK](https://github.com/teddyjfpender/deadeye-rs).

Read paths wrap the SDK's view surface (markets, positions, LP). Write
paths cover trade, sell, LP add/remove, claim, factory-admin operations,
and infrastructure deploys (math runtimes).

Every command supports `--output {pretty,plain,json}` and auto-detects
whether stdout is a TTY.

## Math runtime deploy (`deadeye admin deploy-math-runtime`)

### Why this exists

Each Deadeye market family (normal, lognormal, multinoulli, bivariate)
ships a **math runtime** contract that implements the chain-faithful
sqrt-hint oracle (`compute_hints_view`) plus a few helper views. The
upstream `the-situation` deployer **declares** all four classes on
mainnet but does **not** deploy instances — markets get their runtime
binding from the factory's per-family configuration. Off-chain consumers
that want to do chain-faithful preflight (notably the cpi-arb bot) need
a standalone runtime instance to call.

This command deploys an instance via the legacy Universal Deployer
Contract (UDC) and caches the address locally so subsequent runs are
**idempotent and gas-free**.

### Cost

- One-time per family.
- ~$1 in STRK mainnet gas (a `deployContract` invoke with empty calldata).

### Idempotency

State is kept in `~/.config/deadeye/runtimes.toml` (override path with
`DEADEYE_RUNTIMES_PATH`). On every invocation the command:

1. Reads the cache entry for `(chain_id, family)`.
2. Verifies it via `starknet_getClassHashAt` — class hash must match.
3. If verified, returns immediately without touching the network for a
   second call.
4. If the cache is empty or the cached address has drifted, falls
   through to the deploy path (which itself only runs with `--confirm`).

`unique = false` on the UDC means the same `--salt` always lands at the
same address — re-running with the same salt is a content-addressed
no-op even **without** the cache.

### Usage

```
# Dry-run (no chain action; prints projected address).
deadeye admin deploy-math-runtime --family normal

# Real deploy (requires --confirm + DEADEYE_PRIVATE_KEY env var).
deadeye admin deploy-math-runtime --family normal --confirm

# Use a deterministic salt for cross-machine reproducibility.
deadeye admin deploy-math-runtime --family lognormal --salt 0xdeadbeef --confirm

# Verify every cached entry is still alive on-chain.
deadeye admin deploy-math-runtime --status
```

### Consumer wiring

After a successful deploy the CLI prints a hint like:

```text
set DEADEYE_NORMAL_RUNTIME_ADDR=0x… in your consumer's .env
```

Set the corresponding env var (`DEADEYE_<FAMILY>_RUNTIME_ADDR`) in your
`.env` and the cpi-arb bot (and every other downstream consumer that
calls `quote_trade` / `compute_hints_view`) will pick it up
automatically.

### Cache file format

```toml
[mainnet.normal]
address = "0x..."
class_hash = "0x46d492bbef6f8034b1647a95a96580555742fd4655e766dee04e442a778a753"
deployed_at_block = 1234567
deployed_tx = "0x..."

[mainnet.lognormal]
# ...

[devnet.normal]
# ...
```

Top-level keys: `mainnet`, `devnet` (anything else is grouped
under `devnet`). Inner keys: `normal`, `lognormal`, `multinoulli`,
`bivariate`. Safe to delete — the next `--confirm` re-derives it.
