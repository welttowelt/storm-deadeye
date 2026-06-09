//! Superforecasting toolkit + per-market forecast workspace.
//!
//! [`bayes`] is the pure math (likelihood-ratio updates, log-odds pooling,
//! base-rate blending, evidence weighting, market de-vig, normal aggregation).
//! [`ledger`] is the file-backed workspace where evidence and reference classes
//! accumulate and curate into a committed `(mean, σ)` snapshot that feeds the
//! trade optimizer.

pub(crate) mod bayes;
pub(crate) mod ledger;
