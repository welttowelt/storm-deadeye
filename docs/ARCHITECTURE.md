# deadeye-rs Architecture

## Layering

```
                          ┌──────────────────────┐
   user binary ───────▶   │   deadeye-sdk        │   high-level facade
                          ├──────────────────────┤
                          │   deadeye-collateral │   off-chain numerics
                          ├──────────────────────┤
                          │   deadeye-starknet   │   calldata + view calls
                          ├──────────────────────┤
                          │   deadeye-artifacts  │   embedded ABIs
                          ├──────────────────────┤
                          │   deadeye-core       │   Sq128, distributions, errors
                          └──────────────────────┘
```

Each layer depends only on layers below it. The arrows do not skip: e.g.
`deadeye-sdk` never imports `starknet-providers` directly — it goes
through `deadeye-starknet`.

## Indexer access

`deadeye-indexer` sits alongside the RPC stack — it's a thin HTTP client
for the Deadeye mainnet indexer (`https://178-105-210-177.sslip.io`). For
MMs the workflow typically is:

1. Use `deadeye-indexer` to discover markets and watch aggregated
   activity (this is the cheap, batched, eventually-consistent path).
2. Use `deadeye-starknet` + `deadeye-sdk` to read authoritative state and
   submit trades (the expensive, immediate, strongly-consistent path).

CI runs a `mainnet-indexer` smoke job on every workflow_dispatch and on
the weekly schedule to catch upstream schema drift early.

## Why a workspace?

* **Selective compile**: an indexer can pull in only `deadeye-core +
  deadeye-artifacts` and avoid the entire RPC + signing tree.
* **`no_std` reach**: `deadeye-core` compiles without `std`. We can
  reuse the exact same `Sq128` type inside on-chain verifier tooling
  in the future.
* **Independent versioning**: numeric / distribution changes can ship
  without re-publishing the SDK facade and vice versa.

## Async posture

* `deadeye-core`, `deadeye-collateral`, `deadeye-artifacts`: sync.
* `deadeye-starknet`: defines an async `Provider` trait. Hard-depends on
  `tokio` only via `async-trait`.
* `deadeye-sdk`: builds on `Provider`.

Runtime-agnostic by default; consumers pick `tokio`, `monoio`, or any
custom runtime that implements `Provider`.

## Numerics

`Sq128` (signed Q128.128) is the canonical wire type. Operations are
allocation-free, return `Result<_, CoreError>` on overflow, and
round-trip bit-identical with the Cairo `SQ128x128Raw` struct.

PDF / derivative calculations currently delegate to `f64` for
transcendental functions (`exp`, `ln`, `sqrt`). The on-chain math runtime
re-verifies the result, so off-chain f64 is an acceptable hint precision;
the bit-exact path is reserved for tooling that needs to reproduce chain
state.

## Lint posture

`clippy::all + pedantic + nursery` plus a hand-curated set of
`clippy::restriction` lints. Notable choices:

* `unsafe_code = "forbid"` — no escape hatch.
* `unwrap_used = "deny"`, `unwrap_in_result = "deny"`,
  `panic_in_result_fn = "deny"` — propagate errors, never panic from
  library code. Tests opt in via `#[expect(clippy::unwrap_used)]`.
* `clone_on_ref_ptr = "deny"` — keeps Arc/Rc usage intentional.
* `cast_possible_truncation`/`cast_precision_loss` are **allowed**
  because the math crate has many deliberate float ↔ int conversions
  whose precision loss is documented; per-call-site `#[expect]` would
  drown the code.

## Testing

* **Unit tests**: in each crate's `src/`, gated by `#[cfg(test)]`.
* **Property tests**: `proptest` covers numeric round-trips and
  commutativity laws.
* **Integration tests**: `crates/deadeye-e2e/tests/`. Skipped unless
  `DEADEYE_RUN_INTEGRATION=1`. Target is selected by
  `DEADEYE_TEST_TARGET=devnet|hosted` (default `devnet`).
* **CI**: GitHub Actions runs fmt + clippy + test + docs on every PR.
  Integration suite runs weekly + on demand against a hosted public
  mainnet RPC and a `starknet-devnet-rs` service container.
