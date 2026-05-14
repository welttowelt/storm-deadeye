# Math Runtime Deploy — `deadeye admin deploy-math-runtime`

A new CLI subcommand that deploys an instance of one of the four math
runtime classes (normal / lognormal / multinoulli / bivariate) via the
legacy UDC and caches the address locally for idempotent re-runs.
Primary consumer: the cpi-arb bot's chain-faithful preflight on mainnet.

## 1. CLI surface

```
deadeye admin deploy-math-runtime --family <FAMILY> [--salt 0x...] [--class-hash 0x...] [--confirm]
deadeye admin deploy-math-runtime --status
```

`--help` excerpt:

```
Options:
      --family <FAMILY>       normal | lognormal | multinoulli | bivariate.
                              Required unless --status is set.
      --salt <FELT>           Deterministic salt. Defaults to a fresh random felt.
      --class-hash <0x...>    Override the canonical class hash for (chain_id, family).
      --confirm               Required for an on-chain deploy. Without it: dry-run.
      --status                Verify each cached entry via getClassHashAt; exit 1 on drift.
```

Standard global flags (`--rpc-url`, `--address`, `--profile`,
`--output {pretty,plain,json}`) work. `DEADEYE_PRIVATE_KEY` is read
from env for the deploy path.

Behaviour:

- `--family X` alone → dry-run; prints projected UDC address; no chain write.
- `--family X --confirm` → if cached + verified → no-op print. Else deploys, caches, prints.
- `--status` → walks the cache, runs `getClassHashAt` per entry, exits 1 on any drift.

## 2. Cache file

Path: `~/.config/deadeye/runtimes.toml` (override via `DEADEYE_RUNTIMES_PATH`).

```toml
[mainnet.normal]
address           = "0x05..."
class_hash        = "0x46d492bbef..."
deployed_at_block = 1234567
deployed_tx       = "0x..."

[sepolia.lognormal]
# ...
```

Top-level keys: `mainnet`, `sepolia`, `devnet`. Inner keys: the four
family slugs. Missing file → empty cache; malformed → typed
`DeployerError::InvalidFelt { field: "runtimes_toml", … }`.

## 3. Implementation summary

- **`deadeye-deployer` v0.1.1**: new `runtime` module — `Family`,
  `ChainKey`, `RuntimeCache`, `RuntimeEntry`, `runtime_class_hash`,
  `projected_deploy_address` (wraps
  `starknet_core::utils::get_udc_deployed_address`), and pinned
  mainnet class hashes as compile-time constants.
- **`deadeye-cli` v0.1.1**: new `AdminCmd::DeployMathRuntime`. Submits
  via `OwnedAccount::inner()` + `ContractFactory::new_with_udc(..,
  UdcSelector::Legacy).deploy_v3(vec![], salt, false)` so the salt is
  the *only* thing that affects the deployed address (true idempotency).
- **Idempotency:** before any submission, the cached `(chain, family)`
  entry is verified via `starknet_getClassHashAt`; on match → return.
- **Safety:** `--confirm` is mandatory + TTY prompt
  `Continue? This spends gas.` (auto-skipped under `--output json`).

## 4. Tests passed

| Crate              | Suite                                          | Result        |
| ------------------ | ---------------------------------------------- | ------------- |
| `deadeye-deployer` | 13 unit tests (5 pre-existing + 8 new)         | 13 / 13 pass  |
| `deadeye-cli`      | `cli_smoke`                                    | 4 / 4 pass    |
| `deadeye-cli`      | `deploy_math_runtime` (gated; devnet)          | 1 / 1 pass    |
| Both crates        | `cargo clippy --all-targets` (strict)          | clean         |

New deployer unit tests cover: TOML round-trip + missing/malformed
files; `projected_deploy_address` determinism (deployer ignored under
`unique = false`); `unique = true` branch differs; all four mainnet
class hashes round-trip; `ChainKey::from_chain_id_hex` recognises
`SN_MAIN` / `SN_SEPOLIA`; `Family::env_var_name` returns
`DEADEYE_<FAMILY>_RUNTIME_ADDR`.

## 5. Devnet integration test outcome

`crates/deadeye-cli/tests/deploy_math_runtime.rs`, gated behind
`DEADEYE_RUN_INTEGRATION=1` **and** `DEADEYE_RUN_DEPLOY_MATH_RUNTIME=1`.

Flow: bootstrap devnet → declare normal math runtime → run CLI with
`--family normal --salt 0xc1a55 --class-hash <fresh> --confirm` →
assert `on_chain_verified=true` + cache populated → re-run → assert
`cached=true` + address unchanged → run `--status` → assert verified.

**Result: pass.** Live output (devnet on `:5050`):

```json
{
  "mode": "deploy",
  "chain": "sepolia",
  "family": "normal",
  "address": "0x50ef518fb430f231b9f94ee5603838789aba1fde6ad36c50179d68b874e85d3",
  "class_hash": "0x46d492bbef6f8034b1647a95a96580555742fd4655e766dee04e442a778a753",
  "cached": false,
  "on_chain_verified": true,
  "tx_hash": "0x791a9920f6749e3e98b584897de24717f7e9ea83573dbeb623e3a660381b132",
  "salt": "0xc1a55"
}
```

Second invocation returned the same address with `"cached": true`.
`--status` listed the entry as verified.

## 6. Mainnet path (operator-driven; not run in this change)

Export `DEADEYE_RPC_URL=<mainnet>`,
`DEADEYE_CHAIN_ID=0x534e5f4d41494e`, `DEADEYE_PRIVATE_KEY`,
`DEADEYE_ADDRESS`. Dry-run per family to verify the projected class
hash; re-run with `--confirm`. Copy each printed
`DEADEYE_*_RUNTIME_ADDR` into the cpi-arb `.env`.

Version bumps: `deadeye-deployer` and `deadeye-cli` → `0.1.1`. Not
published — coordinated release with Driver B.
