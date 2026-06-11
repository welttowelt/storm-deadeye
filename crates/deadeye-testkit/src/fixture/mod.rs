//! End-to-end fixture pipeline for devnet chaos testing.
//!
//! Composes [`artifacts`] (Sierra/CASM loading), [`declare`] (idempotent
//! class declaration), [`deploy`] (UDC-based instantiation),
//! [`factory_setup`] (market-type configuration + deploy profiles),
//! [`erc20`] (test collateral token), and [`env`](mod@env) (the [`TestEnv`]
//! one-shot bootstrap).

pub mod artifacts;
pub mod declare;
pub mod deploy;
pub mod env;
pub mod erc20;
pub mod factory_setup;
pub mod lifecycle;

pub use env::{TestEnv, TestEnvError, bootstrap_devnet};
