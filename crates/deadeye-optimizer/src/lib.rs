//! EV-maximizing trade picker + LP profitability helpers for Deadeye
//! normal markets.
//!
//! Mirrors `@the-situation/optimizer` from the TypeScript SDK. All math
//! runs in `f64` — the on-chain math runtime re-verifies the answer in
//! Q128.128 once the trade is submitted, so off-chain we trade a tiny
//! amount of bit-exactness for ~1000× speed-up.
//!
//! Two-line user mental model:
//!
//! 1. Call [`optimize_normal_trade`] to pick the highest-net-EV trade in
//!    the policy region given a budget and a belief.
//! 2. Call [`f_at`] / [`compute_lp_claim_component_value`] to reason
//!    about LP profitability at a settlement outcome.

#![doc(html_no_source)]

pub mod lp;
pub mod normal;

pub use lp::{compute_lp_claim_component_value, compute_total_lp_claim_value, f_at};
pub use normal::{
    NormalOptimizationInput, NormalOptimizationResult, OptimizerConstraints, normal_sigma_floor,
    optimize_normal_trade,
};
