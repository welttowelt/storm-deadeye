//! Integration-test harness for Deadeye.
//!
//! This crate is unpublished and exists solely to support the `tests/it/`
//! suite. It provides:
//!
//! * [`devnet`] — health checks and lifecycle helpers for
//!   `starknet-devnet` (or `katana` as a fall-back).
//! * [`cartridge`] — discovery helpers for Cartridge's hosted Sepolia /
//!   mainnet RPC endpoints.
//! * [`harness`] — high-level [`Harness`] struct that
//!   integration tests use to acquire a provider with sensible defaults.

#![doc(html_no_source)]

pub mod account;
pub mod cartridge;
pub mod devnet;
pub mod fixture;
pub mod harness;

pub use account::{AccountError, DevnetAccount, predeployed, predeployed_one};
pub use harness::{Harness, HarnessError, HarnessKind};
