//! Core types and primitives for the Deadeye prediction-market SDK.
//!
//! This crate is the foundation of the workspace: no async, no I/O, no
//! Starknet dependencies. It is intentionally `no_std`-friendly so the
//! arithmetic and distribution types can be reused inside on-chain
//! verifier tooling, indexers, and HFT-style market-making bots alike.
//!
//! ## Modules
//!
//! * [`sq128`] — signed Q128.128 fixed-point numbers, the on-wire numeric
//!   representation used by every Deadeye market contract.
//! * [`distribution`] — generic [`Distribution`] trait plus concrete
//!   value-objects for the four market families currently supported (normal,
//!   lognormal, multinoulli, bivariate normal).
//! * [`error`] — the [`CoreError`] hierarchy that bubbles up through every
//!   layer of the SDK.

#![cfg_attr(not(feature = "std"), no_std)]
#![doc(html_no_source)]

extern crate alloc;

pub mod bivariate;
pub mod categorical;
pub mod distribution;
pub mod error;
pub mod sq128;

pub use crate::{
    bivariate::{
        BivariateNormalDistribution, BivariateNormalDistributionCoreRaw,
        BivariateNormalDistributionRaw, BivariateNormalSqrtHintsRaw, BivariatePointRaw,
    },
    categorical::{CategoricalDistribution, CategoricalDistributionRaw, CategoricalL2HintRaw},
    distribution::{
        Distribution, LognormalDistribution, LognormalDistributionRaw, NormalDistribution,
        NormalDistributionRaw, NormalSqrtHintsRaw,
    },
    error::CoreError,
    sq128::{Sq128, Sq128Raw, scale, unscale},
};
