//! Multi-leg (trade-lot) position tracking, settlement valuation, and EV/P&L.
//!
//! Each market-moving trade stores an explicit **trade lot** (leg) on-chain.
//! A trader accumulates one lot per admitted delta; legs are valued and
//! settled independently. This module aggregates the low-level lot views
//! ([`deadeye_starknet::NormalMarketReader`]) into:
//!
//! * [`PositionLegs`] — the trader's legs + summary (cheap enumeration).
//! * [`PositionValuation`] — the position valued at a settlement outcome `x*`:
//!   per-leg + total position value (the P&L if it settles at `x*`) and the
//!   gross collateral return.
//!
//! Per-leg value at `x*` is read authoritatively from the chain
//! (`get_trade_lot_value_at`), so settlement valuation is exact. Expected
//! value under a forecast is a normal-pdf-weighted integral of that value
//! over the belief — computed by sampling the on-chain value on a grid (the
//! leg distributions are not individually exposed, so quadrature is the
//! honest path).

/// A settlement outcome to value a position at.
///
/// Its shape depends on the market family: normal / lognormal settle to a
/// scalar `x*`; bivariate to a 2D point `(x1, x2)`; multinoulli to a
/// categorical outcome index.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettlementPoint {
    /// Scalar outcome `x*` (normal, lognormal — lognormal in log-space).
    Scalar(f64),
    /// 2D outcome `(x1, x2)` (bivariate).
    Point {
        /// First-axis outcome.
        x1: f64,
        /// Second-axis outcome.
        x2: f64,
    },
    /// Categorical outcome index (multinoulli).
    Outcome(u32),
}

impl SettlementPoint {
    /// Compact human label, e.g. `x*=4.2`, `(x1,x2)=(1.0, 2.0)`, `outcome #3`.
    #[must_use]
    pub fn label(&self) -> String {
        match *self {
            Self::Scalar(x) => format!("x*={x:.6}"),
            Self::Point { x1, x2 } => format!("(x1,x2)=({x1:.6}, {x2:.6})"),
            Self::Outcome(i) => format!("outcome #{i}"),
        }
    }
}

/// Lifecycle of one trade lot (leg), without valuation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct LegInfo {
    /// On-chain lot identifier.
    pub lot_id: u64,
    /// Already settled (paid out) — no further claimable value.
    pub settled: bool,
    /// Cancelled (collateral forfeit to the LP; could not be valued).
    pub cancelled: bool,
}

impl LegInfo {
    /// Whether this leg is still claimable (neither settled nor cancelled).
    #[must_use]
    pub const fn is_active(&self) -> bool {
        !self.settled && !self.cancelled
    }
}

/// A trader's legs in one market plus the position summary. Cheap to fetch:
/// `1 + count` lot reads + one summary read.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PositionLegs {
    /// Trader address (hex felt).
    pub trader: String,
    /// Every leg the trader holds, in on-chain order.
    pub legs: Vec<LegInfo>,
    /// Total collateral locked across the position (XP).
    pub total_collateral: f64,
    /// Whether any position history exists.
    pub exists: bool,
    /// Whether the position has been fully claimed post-settlement.
    pub claimed: bool,
    /// Whether the trader still tracks a pending settlement claim.
    pub tracks_settlement_claim: bool,
}

impl PositionLegs {
    /// Count of still-claimable legs.
    #[must_use]
    pub fn active_legs(&self) -> usize {
        self.legs.iter().filter(|l| l.is_active()).count()
    }
}

/// One leg valued at a settlement outcome.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct LegValuation {
    /// On-chain lot identifier.
    pub lot_id: u64,
    /// Already settled.
    pub settled: bool,
    /// Cancelled.
    pub cancelled: bool,
    /// Signed position value of this leg at the settlement outcome (XP).
    /// `0.0` for settled/cancelled legs (no future payout).
    pub value_at: f64,
}

/// A trader's whole position valued at a settlement outcome `x*`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PositionValuation {
    /// Trader address (hex felt).
    pub trader: String,
    /// The settlement outcome the legs were valued at.
    pub settlement: SettlementPoint,
    /// Per-leg valuations.
    pub legs: Vec<LegValuation>,
    /// Total collateral locked across the position (XP).
    pub total_collateral: f64,
    /// Σ leg value over active legs — the position's **P&L** if the market
    /// settles at `settlement` (collateral is returned on top).
    pub total_position_value: f64,
    /// What the trader receives if settled at `settlement`:
    /// `total_collateral + total_position_value`.
    pub gross_return: f64,
    /// Whether any position history exists.
    pub exists: bool,
    /// Whether the position has been fully claimed.
    pub claimed: bool,
}

impl PositionValuation {
    /// Count of active (claimable) legs.
    #[must_use]
    pub fn active_legs(&self) -> usize {
        self.legs
            .iter()
            .filter(|l| !l.settled && !l.cancelled)
            .count()
    }
}

/// Normal-pdf-weighted integration grid over `[mean − span·σ, mean + span·σ]`.
///
/// Returns `(x_i, w_i)` pairs where `w_i` are normalised weights summing to
/// 1 — i.e. `Σ w_i f(x_i) ≈ E_{x∼N(mean,σ)}[f(x)]`. Midpoint rule with the
/// normal density as the weight; robust and grid-controlled (the leg
/// distributions aren't exposed, so a closed form isn't available).
#[must_use]
pub fn belief_grid(mean: f64, sigma: f64, span: f64, nodes: usize) -> Vec<(f64, f64)> {
    if sigma <= 0.0 || !sigma.is_finite() || nodes == 0 {
        return vec![(mean, 1.0)];
    }
    let lo = span.mul_add(-sigma, mean);
    let hi = span.mul_add(sigma, mean);
    #[expect(
        clippy::cast_precision_loss,
        reason = "node count is small; f64 is exact well past any practical grid"
    )]
    let n = nodes as f64;
    let step = (hi - lo) / n;
    let inv_2s2 = 1.0 / (2.0 * sigma * sigma);
    let mut pts = Vec::with_capacity(nodes);
    let mut wsum = 0.0;
    for i in 0..nodes {
        #[expect(clippy::cast_precision_loss, reason = "small index")]
        let idx = i as f64;
        let x = step.mul_add(idx + 0.5, lo);
        let d = x - mean;
        let w = (-d * d * inv_2s2).exp(); // ∝ normal density (constant factor cancels)
        pts.push((x, w));
        wsum += w;
    }
    if wsum > 0.0 {
        for p in &mut pts {
            p.1 /= wsum;
        }
    }
    pts
}
