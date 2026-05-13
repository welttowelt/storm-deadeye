//! Common types for write-path invocations.
//!
//! Every write call (trade, sell, claim, liquidity, admin) flows through
//! the same shape: build a [`Call`], submit it via an [`Account`], get
//! back an [`ExecutionReceipt`].

use starknet_core::types::Felt;

/// Re-export of [`starknet_core::types::Call`] — the unit of work submitted
/// to a Starknet account's `__execute__` entry-point.
pub use starknet_core::types::Call;

/// Result of submitting one or more [`Call`]s through an [`Account`].
///
/// We intentionally keep this minimal: we surface the on-chain transaction
/// hash and the number of calls bundled. Higher layers can poll for
/// confirmation themselves via the [`Provider`](crate::Provider).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExecutionReceipt {
    /// Hash of the submitted INVOKE v3 transaction.
    pub transaction_hash: Felt,
    /// Number of [`Call`]s that were bundled into the transaction.
    pub call_count: usize,
}

impl ExecutionReceipt {
    /// Convenience constructor.
    #[must_use]
    pub const fn new(transaction_hash: Felt, call_count: usize) -> Self {
        Self {
            transaction_hash,
            call_count,
        }
    }
}
