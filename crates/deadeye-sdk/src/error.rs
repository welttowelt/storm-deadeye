//! SDK-level error hierarchy.

use deadeye_collateral::CollateralError;
use deadeye_core::CoreError;
use deadeye_starknet::{ContractError, TradeError};
use thiserror::Error;

/// Convenience alias.
pub type SdkResult<T> = core::result::Result<T, SdkError>;

/// All failure modes that the high-level SDK surfaces to a caller.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SdkError {
    /// Domain / numeric error from [`deadeye_core`].
    #[error(transparent)]
    Core(#[from] CoreError),

    /// Off-chain collateral solver failed.
    #[error(transparent)]
    Collateral(#[from] CollateralError),

    /// A contract call failed.
    #[error(transparent)]
    Contract(#[from] ContractError),

    /// A write-path call was rejected by an on-chain guard with a typed
    /// reason (see
    /// [`deadeye_starknet::TradeRejectionReason`]).
    #[error(transparent)]
    Trade(#[from] TradeError),

    /// Required wallet / account context was missing for a write call.
    #[error("operation requires a connected account: {0}")]
    AccountRequired(&'static str),
}
