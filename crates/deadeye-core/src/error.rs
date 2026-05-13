//! Domain error hierarchy shared by every Deadeye crate.
//!
//! All errors are `#[non_exhaustive]` so we can grow variants without a
//! semver break, and they implement [`core::error::Error`] via
//! [`thiserror`]. Higher layers (collateral, starknet, sdk) wrap
//! [`CoreError`] inside their own typed errors rather than re-defining it.

use alloc::string::String;

use thiserror::Error;

/// Errors produced by the numeric and distribution primitives.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum CoreError {
    /// A user-supplied value violated an invariant (e.g. negative variance).
    #[error("invalid input ({field}): {message}")]
    InvalidInput {
        /// Identifier of the field, parameter, or expression that failed.
        field: &'static str,
        /// Human-readable description of the violation.
        message: String,
    },

    /// Arithmetic produced a value outside the representable range of
    /// [`Sq128`](crate::sq128::Sq128).
    #[error("arithmetic overflow in {operation}")]
    Overflow {
        /// Name of the operation that overflowed.
        operation: &'static str,
    },

    /// A divisor evaluated to zero.
    #[error("division by zero in {operation}")]
    DivisionByZero {
        /// Name of the operation that attempted the division.
        operation: &'static str,
    },

    /// An iterative numerical method (e.g. Newton-Raphson) failed to
    /// converge within the allotted iterations.
    #[error("solver `{name}` did not converge after {iterations} iterations")]
    SolverDidNotConverge {
        /// Name of the iterative method.
        name: &'static str,
        /// Number of iterations attempted before giving up.
        iterations: u32,
    },

    /// A computation that requires positive support (e.g. lognormal PDF)
    /// received an out-of-support input.
    #[error("value `{value}` lies outside the support of {distribution}")]
    OutOfSupport {
        /// Name of the distribution.
        distribution: &'static str,
        /// String-encoded value for diagnostics.
        value: String,
    },
}

impl CoreError {
    /// Convenience constructor for [`CoreError::InvalidInput`].
    #[inline]
    pub fn invalid_input(field: &'static str, message: impl Into<String>) -> Self {
        Self::InvalidInput {
            field,
            message: message.into(),
        }
    }

    /// Convenience constructor for [`CoreError::Overflow`].
    #[inline]
    pub const fn overflow(operation: &'static str) -> Self {
        Self::Overflow { operation }
    }

    /// Convenience constructor for [`CoreError::DivisionByZero`].
    #[inline]
    pub const fn division_by_zero(operation: &'static str) -> Self {
        Self::DivisionByZero { operation }
    }
}
