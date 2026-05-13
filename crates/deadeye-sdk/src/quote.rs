//! Quote primitives shared across market families.

use deadeye_core::Sq128;

/// A prepared trade quote — the off-chain decision the MM has made.
#[derive(Debug, Clone, Copy)]
pub struct PreparedQuote {
    /// Where the minimum of `d(x) = g(x) - f(x)` was found (off-chain).
    pub x_star: Sq128,
    /// Collateral the MM must supply.
    pub collateral: Sq128,
    /// `iterations` Newton-Raphson took to converge (diagnostics).
    pub iterations: u32,
}

/// Side of a trade from the MM's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// MM is moving the market away from its current state.
    Open,
    /// MM is closing out a previously-opened position.
    Close,
}
