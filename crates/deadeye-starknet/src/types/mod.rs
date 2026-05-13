//! Typed Cairo Serde shapes mirroring the on-chain contract ABIs.
//!
//! Each sub-module mirrors the corresponding TS package layout from
//! `@the-situation/abi` and implements [`CairoSerde`](crate::CairoSerde)
//! for every input / output struct used by the SDK.

pub mod bivariate;
pub mod common;
pub mod factory;
pub mod lognormal;
pub mod multinoulli;
pub mod normal;
pub mod oracle;
