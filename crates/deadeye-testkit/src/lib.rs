//! Integration-test harness for Deadeye.
//!
//! This crate is unpublished and exists solely to support the `tests/it/`
//! suite. It provides:
//!
//! * [`devnet`] — health checks and lifecycle helpers for `starknet-devnet` (or
//!   `katana` as a fall-back).
//! * [`harness`] — high-level [`Harness`] struct that integration tests use to
//!   acquire a provider with sensible defaults (local devnet, or a hosted
//!   mainnet RPC via [`harness::default_mainnet_rpc`]).

#![doc(html_no_source)]

pub mod account;
pub mod devnet;
pub mod fixture;
pub mod harness;

pub use account::{AccountError, DevnetAccount, predeployed, predeployed_one};
pub use harness::{DEFAULT_HOSTED_RPC, Harness, HarnessError, HarnessKind, default_mainnet_rpc};
