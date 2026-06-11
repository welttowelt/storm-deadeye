//! Common types for write-path invocations.
//!
//! Every write call (trade, sell, claim, liquidity, admin) flows through
//! the same shape: build a [`Call`], submit it via an [`crate::Account`], get
//! back an [`ExecutionReceipt`].

/// Re-export of [`starknet_core::types::Call`] — the unit of work submitted
/// to a Starknet account's `__execute__` entry-point.
pub use starknet_core::types::Call;
use starknet_core::types::Felt;

/// Result of submitting one or more [`Call`]s through an [`crate::Account`].
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

/// Verdict of a **gas-free** chain simulation of a multicall.
///
/// Produced by [`Account::simulate`](crate::Account::simulate) (which runs the
/// calls through the sequencer with `skip_validate` + `skip_fee_charge`, so it
/// needs no valid signature and spends no balance). The trade write-paths use
/// this to refuse a doomed submission *before* it burns gas — a reverting trade
/// is caught here and surfaced as a typed rejection instead of an on-chain
/// `Result::unwrap failed` panic that costs the trader a fee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimOutcome {
    /// `Some(reason)` if the sequencer reports the multicall would **revert**
    /// (the raw Cairo revert string, e.g. `Result::unwrap failed`); `None` when
    /// the call path executes cleanly.
    pub revert_reason: Option<String>,
    /// Estimated total fee in FRI (STRK) the real submission would pay.
    pub estimated_fee: u128,
}

impl SimOutcome {
    /// `true` iff the simulation executed without reverting.
    #[must_use]
    pub const fn would_succeed(&self) -> bool {
        self.revert_reason.is_none()
    }
}
