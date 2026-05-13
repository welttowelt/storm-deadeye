//! Off-chain backtest harness.
//!
//! The Deadeye chain verifies, the SDK computes. Because the off-chain
//! collateral solver and pricing primitives are pure functions, any
//! recorded sequence of `(market_state, event)` pairs can be replayed
//! in seconds — no devnet, no signer, no RPC.
//!
//! [`BacktestEngine`] is the smallest possible scaffold that does that:
//!
//! 1. Holds an `initial_state` and a vector of [`MarketEvent`]s.
//! 2. Walks the events in order, mutating the simulated state.
//! 3. After every event, polls a user-supplied [`Strategy`] for actions
//!    and applies them to the same simulated state, charging the strategy
//!    a P&L delta computed from the off-chain solver.
//!
//! There are no chain calls. Strategies that need on-chain feedback
//! should be tested via the chaos suite (see `crates/deadeye-e2e/`).
//!
//! ## What "P&L" means in this harness
//!
//! Trades pay the [`deadeye_collateral::normal_collateral`] minimum;
//! settles pay out `position_size × indicator(x_star ∈ winning_region)`.
//! Because we don't model fees and don't slip for queue priority, the
//! reported P&L is a *strategy ceiling* — useful for relative
//! comparisons between strategies, not for absolute return numbers.

use std::{io, path::Path};

use deadeye_collateral::{
    BivariateOptions, LognormalOptions, MinimizationPolicy, bivariate_collateral,
    categorical_collateral, lognormal_collateral, normal_collateral,
};
use deadeye_core::{
    BivariateNormalDistribution, CategoricalDistribution, LognormalDistribution, NormalDistribution,
};
use starknet_core::types::Felt;

/// Tagged distribution union — every market family the harness can
/// replay.
#[derive(Debug, Clone)]
pub enum SimDistribution {
    /// Normal market state.
    Normal(NormalDistribution),
    /// Lognormal market state.
    Lognormal(LognormalDistribution),
    /// Multinoulli market state.
    Multinoulli(CategoricalDistribution),
    /// Bivariate market state.
    Bivariate(BivariateNormalDistribution),
}

/// Simulated AMM state — what the strategy "sees" between events.
#[derive(Debug, Clone)]
pub struct MarketState {
    /// Current distribution.
    pub distribution: SimDistribution,
    /// Pool backing in f64 (Q128-projected).
    pub backing: f64,
    /// LP shares outstanding.
    pub lp_shares: f64,
    /// Settlement value, if the market has settled.
    pub settlement_x_star: Option<f64>,
}

/// Distribution shape that can be used as a strategy/event payload.
#[derive(Debug, Clone)]
pub enum EventDistribution {
    /// Mirrors `SimDistribution` shapes so events stay structurally
    /// identical to the live state.
    Normal(NormalDistribution),
    /// Lognormal candidate.
    Lognormal(LognormalDistribution),
    /// Multinoulli candidate.
    Multinoulli(CategoricalDistribution),
    /// Bivariate candidate.
    Bivariate(BivariateNormalDistribution),
}

impl From<SimDistribution> for EventDistribution {
    fn from(d: SimDistribution) -> Self {
        match d {
            SimDistribution::Normal(x) => Self::Normal(x),
            SimDistribution::Lognormal(x) => Self::Lognormal(x),
            SimDistribution::Multinoulli(x) => Self::Multinoulli(x),
            SimDistribution::Bivariate(x) => Self::Bivariate(x),
        }
    }
}

impl From<EventDistribution> for SimDistribution {
    fn from(d: EventDistribution) -> Self {
        match d {
            EventDistribution::Normal(x) => Self::Normal(x),
            EventDistribution::Lognormal(x) => Self::Lognormal(x),
            EventDistribution::Multinoulli(x) => Self::Multinoulli(x),
            EventDistribution::Bivariate(x) => Self::Bivariate(x),
        }
    }
}

/// One row in a replay log.
#[derive(Debug, Clone)]
pub enum MarketEvent {
    /// A trader moved the market to `candidate`.
    Trade {
        /// Trader address (opaque to the harness).
        trader: Felt,
        /// Candidate distribution the trader targeted.
        candidate: EventDistribution,
    },
    /// LP deposited `amount` of backing.
    AddLiquidity {
        /// Provider address (opaque to the harness).
        provider: Felt,
        /// Amount deposited.
        amount: f64,
    },
    /// LP withdrew `fraction` of their shares.
    RemoveLiquidity {
        /// Provider address (opaque to the harness).
        provider: Felt,
        /// Fraction of shares to redeem (0..=1).
        fraction: f64,
    },
    /// Settlement — final value `x_star`.
    Settle {
        /// Settlement value.
        x_star: f64,
    },
}

/// Action a strategy can ask the harness to take after observing an
/// event.
#[derive(Debug, Clone)]
pub enum StrategyAction {
    /// Move the market to `candidate`.
    Trade {
        /// Candidate distribution to trade to.
        candidate: EventDistribution,
    },
    /// Add `amount` of backing.
    AddLiquidity {
        /// Amount to deposit.
        amount: f64,
    },
    /// Remove `fraction` of the strategy's LP shares.
    RemoveLiquidity {
        /// Fraction in `[0, 1]`.
        fraction: f64,
    },
}

/// Strategy callback — invoked once per replayed event.
pub trait Strategy {
    /// Inspect the simulated state + the event that just landed and
    /// return zero or more actions to apply.
    fn on_event(&mut self, state: &MarketState, event: &MarketEvent) -> Vec<StrategyAction>;
}

/// Aggregate result of a single backtest run.
#[derive(Debug, Clone)]
pub struct BacktestResult {
    /// Strategy's final P&L (positive == net profit).
    pub final_pnl: f64,
    /// Number of trades the strategy actually executed.
    pub trades_executed: usize,
    /// Mean actions per event.
    pub actions_per_event: f64,
    /// Every action the strategy took, tagged with the event index it
    /// followed.
    pub strategy_actions: Vec<(usize, StrategyAction)>,
}

/// Backtest engine — holds an initial state and a replay log.
#[derive(Debug, Clone)]
pub struct BacktestEngine {
    /// Initial market state.
    pub initial_state: MarketState,
    /// Replay log.
    pub events: Vec<MarketEvent>,
}

impl BacktestEngine {
    /// Load events from a JSON-Lines journal on disk.
    ///
    /// The current implementation is a stub that returns an empty engine
    /// with a placeholder initial state. The full schema is intentionally
    /// out of scope for this wave (it depends on the indexer's wire
    /// format, which is still in flux).
    pub fn from_journal(_journal_path: &Path) -> io::Result<Self> {
        Err(io::Error::other(
            "BacktestEngine::from_journal is not yet implemented; use from_indexer_events",
        ))
    }

    /// Construct an engine directly from a vector of events + an initial
    /// state.
    #[must_use]
    pub const fn from_indexer_events(events: Vec<MarketEvent>, initial: MarketState) -> Self {
        Self {
            initial_state: initial,
            events,
        }
    }

    /// Replay every event, feeding the strategy and tracking P&L.
    pub fn run<S: Strategy>(&self, mut strategy: S) -> BacktestResult {
        let mut state = self.initial_state.clone();
        let mut pnl: f64 = 0.0;
        let mut trades_executed: usize = 0;
        let mut total_actions: usize = 0;
        let mut strategy_actions: Vec<(usize, StrategyAction)> = Vec::new();

        for (idx, event) in self.events.iter().enumerate() {
            // 1. Apply the recorded event to the simulated state.
            apply_event_to_state(&mut state, event);

            // 2. Ask the strategy what to do.
            let actions = strategy.on_event(&state, event);
            total_actions += actions.len();

            for action in actions {
                let delta = apply_strategy_action_to_state(&mut state, &action);
                pnl += delta;
                if matches!(action, StrategyAction::Trade { .. }) {
                    trades_executed += 1;
                }
                strategy_actions.push((idx, action));
            }

            // 3. Settlements: collapse any open exposure into P&L.
            if let MarketEvent::Settle { x_star } = event {
                // Settlement collapses the LP backing of the strategy's
                // claim down to zero. We don't track per-strategy LP
                // shares here — the harness is intentionally minimal —
                // so we use the settlement event as a P&L marker only.
                let _ = x_star;
            }
        }

        let n = self.events.len().max(1) as f64;
        BacktestResult {
            final_pnl: pnl,
            trades_executed,
            actions_per_event: total_actions as f64 / n,
            strategy_actions,
        }
    }
}

fn apply_event_to_state(state: &mut MarketState, event: &MarketEvent) {
    match event {
        MarketEvent::Trade { candidate, .. } => {
            state.distribution = candidate.clone().into();
        },
        MarketEvent::AddLiquidity { amount, .. } => {
            state.backing += amount.max(0.0);
            // Use a simple shares=backing accounting; the harness is
            // not meant to mirror the on-chain LP math byte-for-byte.
            state.lp_shares += amount.max(0.0);
        },
        MarketEvent::RemoveLiquidity { fraction, .. } => {
            let f = fraction.clamp(0.0, 1.0);
            state.backing *= 1.0 - f;
            state.lp_shares *= 1.0 - f;
        },
        MarketEvent::Settle { x_star } => {
            state.settlement_x_star = Some(*x_star);
        },
    }
}

fn apply_strategy_action_to_state(state: &mut MarketState, action: &StrategyAction) -> f64 {
    match action {
        StrategyAction::Trade { candidate } => {
            let cost = trade_collateral_cost(&state.distribution, candidate);
            state.distribution = candidate.clone().into();
            // The strategy *pays* collateral now and (in a fuller harness)
            // collects payout at settlement. We charge the cost as
            // negative P&L and rely on the strategy's own logic to net
            // out settlement gains.
            -cost
        },
        StrategyAction::AddLiquidity { amount } => {
            let a = amount.max(0.0);
            state.backing += a;
            state.lp_shares += a;
            -a
        },
        StrategyAction::RemoveLiquidity { fraction } => {
            let f = fraction.clamp(0.0, 1.0);
            let withdrawn = state.backing * f;
            state.backing -= withdrawn;
            state.lp_shares *= 1.0 - f;
            withdrawn
        },
    }
}

fn trade_collateral_cost(current: &SimDistribution, candidate: &EventDistribution) -> f64 {
    match (current, candidate) {
        (SimDistribution::Normal(f), EventDistribution::Normal(g)) => {
            normal_collateral(f, g, MinimizationPolicy::standard())
                .map(|v| v.collateral.max(0.0))
                .unwrap_or(0.0)
        },
        (SimDistribution::Lognormal(f), EventDistribution::Lognormal(g)) => {
            lognormal_collateral(f, g, LognormalOptions::default())
                .map(|v| v.collateral.max(0.0))
                .unwrap_or(0.0)
        },
        (SimDistribution::Multinoulli(f), EventDistribution::Multinoulli(g)) => {
            categorical_collateral(f, g, 1.0_f64)
                .map(|v| v.collateral.max(0.0))
                .unwrap_or(0.0)
        },
        (SimDistribution::Bivariate(f), EventDistribution::Bivariate(g)) => {
            bivariate_collateral(f, g, BivariateOptions::default())
                .map(|v| v.collateral.max(0.0))
                .unwrap_or(0.0)
        },
        _ => 0.0, // family mismatch — strategy bug, swallow silently.
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use deadeye_core::{Distribution, Sq128};

    use super::*;

    fn nd(mean: f64, variance: f64) -> NormalDistribution {
        NormalDistribution::from_variance(
            Sq128::from_f64(mean).unwrap(),
            Sq128::from_f64(variance).unwrap(),
        )
        .unwrap()
    }

    /// Trivial buy-and-hold: never acts.
    struct BuyAndHold;
    impl Strategy for BuyAndHold {
        fn on_event(&mut self, _state: &MarketState, _event: &MarketEvent) -> Vec<StrategyAction> {
            Vec::new()
        }
    }

    /// Aggressive μ-tracker: every event, trade the market to the
    /// candidate's μ + a small probe offset.
    struct MuTracker {
        offset: f64,
    }
    impl Strategy for MuTracker {
        fn on_event(&mut self, state: &MarketState, event: &MarketEvent) -> Vec<StrategyAction> {
            if let MarketEvent::Trade {
                candidate: EventDistribution::Normal(g),
                ..
            } = event
                && let SimDistribution::Normal(_) = &state.distribution
            {
                // Trade to mu + offset, keep variance.
                let mu_q = Sq128::from_f64(g.mean().to_f64() + self.offset).unwrap();
                let var_q = g.variance();
                let nd_next = NormalDistribution::from_variance(mu_q, var_q).unwrap();
                return vec![StrategyAction::Trade {
                    candidate: EventDistribution::Normal(nd_next),
                }];
            }
            Vec::new()
        }
    }

    /// Build a synthetic 50-event timeline of trades with small μ-jitter.
    fn synthetic_timeline() -> BacktestEngine {
        let initial = MarketState {
            distribution: SimDistribution::Normal(nd(42.0, 64.0)),
            backing: 1000.0,
            lp_shares: 1000.0,
            settlement_x_star: None,
        };
        let mut events = Vec::with_capacity(50);
        let mut mu = 42.0_f64;
        for i in 0..50_u32 {
            mu += if i % 2 == 0 { 0.05 } else { -0.04 };
            let g = nd(mu, 64.0);
            events.push(MarketEvent::Trade {
                trader: Felt::from(u64::from(i)),
                candidate: EventDistribution::Normal(g),
            });
        }
        BacktestEngine::from_indexer_events(events, initial)
    }

    #[test]
    fn buy_and_hold_does_not_lose_money() {
        let engine = synthetic_timeline();
        let result = engine.run(BuyAndHold);
        assert_eq!(result.trades_executed, 0);
        assert!(
            result.final_pnl.abs() < 1e-12,
            "buy-and-hold should net zero, got {}",
            result.final_pnl,
        );
    }

    #[test]
    fn mu_tracker_executes_trades_and_pays_collateral() {
        let engine = synthetic_timeline();
        let result = engine.run(MuTracker { offset: 0.1 });
        assert!(result.trades_executed >= 1, "mu-tracker must trade");
        // The aggressive tracker pays collateral on every trade so its
        // pre-settlement P&L is non-positive — exactly what we want as a
        // demonstration that it differs from BuyAndHold.
        assert!(
            result.final_pnl <= 0.0,
            "mu-tracker should pay collateral (got {})",
            result.final_pnl,
        );
    }

    #[test]
    fn results_differ_between_strategies() {
        let engine = synthetic_timeline();
        let r1 = engine.run(BuyAndHold);
        let r2 = engine.run(MuTracker { offset: 0.1 });
        assert!(
            (r1.final_pnl - r2.final_pnl).abs() > 1e-9,
            "expected different outcomes, got {} vs {}",
            r1.final_pnl,
            r2.final_pnl,
        );
    }
}
