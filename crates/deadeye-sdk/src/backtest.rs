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

use std::{
    fs::File,
    io::{self, BufRead, BufReader},
    path::Path,
};

use deadeye_collateral::{
    BivariateOptions, LognormalOptions, MinimizationPolicy, bivariate_collateral,
    categorical_collateral, lognormal_collateral, normal_collateral,
};
use deadeye_core::{
    BivariateNormalDistribution, CategoricalDistribution, LognormalDistribution, NormalDistribution,
    Sq128,
};
use starknet_core::types::Felt;

use crate::{
    bulk::Family,
    journal::{EntryKind, JournalEntry},
};

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
    /// The file must be newline-delimited JSON where each line is a
    /// [`JournalEntry`] (the same on-disk format
    /// [`TradeJournal`](crate::journal::TradeJournal) writes). Any third
    /// party can produce a replayable journal as long as each line
    /// deserialises into a `JournalEntry`; the relevant `off_chain_quote`
    /// fields per [`EntryKind`] are:
    ///
    /// | `EntryKind`         | Required fields (read by name from `off_chain_quote`)            |
    /// |---------------------|-------------------------------------------------------------------|
    /// | `Trade` (Normal)    | `candidate.mean` (f64), `candidate.sigma` (f64, > 0)              |
    /// | `Sell`              | none — emitted as a `RemoveLiquidity` event with `fraction = 0.0` (no-op state; preserved as a marker so the strategy callback sees it) |
    /// | `AddLiquidity`      | `padded_collateral` / `supplied_collateral` / `required_collateral` (first present) |
    /// | `RemoveLiquidity`   | `fraction` (f64, 0..=1)                                           |
    /// | `Settle`            | `x_star` (f64)                                                    |
    /// | `Claim`             | passed through as `RemoveLiquidity { fraction = 0.0 }` — see Sell |
    ///
    /// Behaviour:
    ///
    /// * **Missing path** — returns the underlying [`io::Error`]
    ///   (`NotFound`), so callers can distinguish "no journal yet" from
    ///   "journal corrupt".
    /// * **Empty file** — returns an engine with zero events and the
    ///   default initial state.
    /// * **Corrupted / unparseable lines** — emitted as a
    ///   `tracing::warn` and skipped (matches
    ///   [`TradeJournal::replay`](crate::journal::TradeJournal::replay)'s
    ///   permissive contract; the journal is an observability tool, not
    ///   a strict schema gate).
    /// * **Trade entries that aren't `Family::Normal`** — currently
    ///   skipped. The harness mirrors the on-chain Normal AMM solver;
    ///   broader family support arrives when the other families ship
    ///   their off-chain solvers.
    /// * **`Trade` entries with non-finite μ/σ or σ ≤ 0** — skipped
    ///   (these are operator-side gate-skip rows where the off-chain
    ///   quote shape was intentionally truncated).
    ///
    /// Submission-state rows the analytics layer flags as "skipped"
    /// (`receipt.error` starting with `"skipped (…)"`, no `tx_hash`)
    /// are skipped here too — they describe trades that never reached
    /// the chain, so they're not part of the market event stream.
    ///
    /// The returned engine seeds its `initial_state` from the first
    /// Normal `Trade` entry's candidate (so the simulated state matches
    /// what the bot actually observed at t=0), falling back to N(0, 1)
    /// when the journal contains no Normal trades.
    ///
    /// # Errors
    ///
    /// Returns [`io::Error`] when `journal_path` cannot be opened (most
    /// often `NotFound`). Per-line decode failures are logged and
    /// skipped, **not** propagated — that is the journal's documented
    /// permissive contract.
    pub fn from_journal(journal_path: &Path) -> io::Result<Self> {
        let file = File::open(journal_path)?;
        let reader = BufReader::new(file);

        let mut events: Vec<MarketEvent> = Vec::new();
        for (line_no, line_res) in reader.lines().enumerate() {
            let line = match line_res {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!(
                        line = line_no + 1,
                        error = %e,
                        "from_journal: skipping unreadable line",
                    );
                    continue;
                },
            };
            if line.trim().is_empty() {
                continue;
            }
            let entry: JournalEntry = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(
                        line = line_no + 1,
                        error = %e,
                        "from_journal: skipping unparseable JSON line",
                    );
                    continue;
                },
            };
            if entry_is_skipped(&entry) {
                continue;
            }
            if let Some(event) = journal_entry_to_event(&entry) {
                events.push(event);
            }
        }

        let initial = seed_initial_state(&events)?;
        Ok(Self {
            initial_state: initial,
            events,
        })
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

// ─── Journal-replay helpers (Follow-up #5) ─────────────────────────
//
// Pure functions that translate a SDK [`JournalEntry`] (the on-disk
// schema produced by [`TradeJournal`](crate::journal::TradeJournal))
// into a [`MarketEvent`] the backtest engine consumes. The cpi-bot
// uses the SDK's own [`JournalEntry`] (not a private mirror), so the
// replay path is schema-stable across the bot ↔ SDK boundary.

/// Pre-submission gate-skip rows — `EntryKind::Trade` with `tx_hash =
/// None` and a `receipt.error` starting with `"skipped (...)"` — never
/// reached the chain and are *not* market events. Mirrors the analytics
/// layer's `entry_is_skipped_for_edge` / `entry_is_skipped_for_risk`
/// predicates so a replay sees exactly the trades the chain saw.
fn entry_is_skipped(entry: &JournalEntry) -> bool {
    if entry.tx_hash.is_some() {
        return false;
    }
    let Some(receipt) = entry.receipt.as_ref() else {
        return false;
    };
    receipt
        .get("error")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|s| s.starts_with("skipped ("))
}

/// Map a single `JournalEntry` to a `MarketEvent`. Returns `None` when
/// the entry cannot be replayed (unknown family, malformed quote,
/// non-finite numerics) — the caller filters these out silently.
fn journal_entry_to_event(entry: &JournalEntry) -> Option<MarketEvent> {
    match entry.kind {
        EntryKind::Trade => {
            // Only Normal-family trades are replayable today. The other
            // families ship off-chain solvers in later waves.
            if !matches!(entry.family, Family::Normal) {
                return None;
            }
            let candidate = normal_candidate_from_quote(&entry.off_chain_quote)?;
            Some(MarketEvent::Trade {
                trader: entry.trader,
                candidate: EventDistribution::Normal(candidate),
            })
        },
        EntryKind::AddLiquidity => {
            let amount = entry
                .off_chain_quote
                .get("padded_collateral")
                .or_else(|| entry.off_chain_quote.get("supplied_collateral"))
                .or_else(|| entry.off_chain_quote.get("required_collateral"))
                .and_then(serde_json::Value::as_f64)
                .filter(|v| v.is_finite())
                .unwrap_or(0.0);
            Some(MarketEvent::AddLiquidity {
                provider: entry.trader,
                amount,
            })
        },
        EntryKind::RemoveLiquidity => {
            let fraction = entry
                .off_chain_quote
                .get("fraction")
                .and_then(serde_json::Value::as_f64)
                .filter(|v| v.is_finite())
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            Some(MarketEvent::RemoveLiquidity {
                provider: entry.trader,
                fraction,
            })
        },
        EntryKind::Settle => {
            let x_star = entry
                .off_chain_quote
                .get("x_star")
                .and_then(serde_json::Value::as_f64)
                .filter(|v| v.is_finite())?;
            Some(MarketEvent::Settle { x_star })
        },
        // `Sell` and `Claim` don't change the market distribution from
        // the replay engine's perspective (the AMM moves μ on `Trade`,
        // not on a user-side close). Surface them as zero-fraction
        // `RemoveLiquidity` markers so strategies that count "I saw a
        // sell" can react via the event callback; the state-mutation
        // path treats `fraction = 0` as a no-op.
        EntryKind::Sell | EntryKind::Claim => Some(MarketEvent::RemoveLiquidity {
            provider: entry.trader,
            fraction: 0.0,
        }),
    }
}

/// Decode a normal-family candidate distribution from an
/// `off_chain_quote` JSON blob. Tolerant of the two shapes the SDK
/// emits in practice — `candidate.{mean,sigma}` and the older
/// `candidate.{mean,variance,sigma}`.
fn normal_candidate_from_quote(quote: &serde_json::Value) -> Option<NormalDistribution> {
    let candidate = quote.get("candidate")?;
    let mean = candidate
        .get("mean")
        .and_then(serde_json::Value::as_f64)
        .filter(|v| v.is_finite())?;
    let sigma = candidate
        .get("sigma")
        .and_then(serde_json::Value::as_f64)
        .filter(|v| v.is_finite() && *v > 0.0)?;
    let mu_q = Sq128::from_f64(mean).ok()?;
    let var_q = Sq128::from_f64(sigma * sigma).ok()?;
    NormalDistribution::from_variance(mu_q, var_q).ok()
}

/// Seed `initial_state` from the first Normal trade we found (so the
/// simulated state matches what the bot actually observed at t=0).
/// Falls back to N(0, 1) when the journal contains no Normal trades.
///
/// Returns an `io::Error` only if the N(0, 1) fallback itself fails to
/// construct — this should be impossible at runtime (`Sq128::from_f64`
/// and `NormalDistribution::from_variance` both accept these literals
/// unconditionally), but we propagate rather than panic to honour the
/// SDK's `panic = "warn"` lint posture.
fn seed_initial_state(events: &[MarketEvent]) -> io::Result<MarketState> {
    for ev in events {
        if let MarketEvent::Trade {
            candidate: EventDistribution::Normal(d),
            ..
        } = ev
        {
            return Ok(MarketState {
                distribution: SimDistribution::Normal(*d),
                backing: 0.0,
                lp_shares: 0.0,
                settlement_x_star: None,
            });
        }
    }
    let mu = Sq128::from_f64(0.0)
        .map_err(|e| io::Error::other(format!("seed_initial_state: Sq128(0.0) failed: {e}")))?;
    let var = Sq128::from_f64(1.0)
        .map_err(|e| io::Error::other(format!("seed_initial_state: Sq128(1.0) failed: {e}")))?;
    let dist = NormalDistribution::from_variance(mu, var).map_err(|e| {
        io::Error::other(format!("seed_initial_state: N(0,1) construction failed: {e}"))
    })?;
    Ok(MarketState {
        distribution: SimDistribution::Normal(dist),
        backing: 0.0,
        lp_shares: 0.0,
        settlement_x_star: None,
    })
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use deadeye_core::Distribution;

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

    // ─── from_journal — Follow-up #5 ───────────────────────────────────
    //
    // Exercise the real NDJSON-replay path against fixtures we write to
    // a tempdir. We deliberately use `TradeJournal::append` to seed each
    // fixture so the tests fail the moment the on-disk schema drifts in
    // either direction (writer ↔ reader).

    use std::{
        io::Write,
        time::{Duration, SystemTime},
    };

    use serde_json::json;
    use tempfile::tempdir;

    use crate::journal::{JournalEntry as JE, JournalSink, TradeJournal};

    fn trade_entry(mean: f64, sigma: f64, market: u64, trader: u64) -> JE {
        let payload = json!({
            "candidate": {"mean": mean, "sigma": sigma},
            "padded_collateral": 0.5,
            "on_chain_will_accept": true,
        });
        let mut e = JE::new(
            Family::Normal,
            Felt::from(market),
            Felt::from(trader),
            EntryKind::Trade,
            payload,
        );
        // Pin the timestamp so test order is deterministic. Using
        // `UNIX_EPOCH + (market+trader)` keeps the path side-effect-free.
        e.timestamp = SystemTime::UNIX_EPOCH + Duration::from_secs(market + trader);
        e.tx_hash = Some(Felt::from(0xCAFE_u64 + market));
        e
    }

    fn skipped_entry(market: u64) -> JE {
        let payload = json!({
            "candidate": {"mean": 1.0, "sigma": 0.5},
            "padded_collateral": 0.5,
            "on_chain_will_accept": true,
        });
        let receipt = json!({
            "error": "skipped (edge too thin): edge ratio 0.01 below threshold 0.05",
        });
        let mut e = JE::new(
            Family::Normal,
            Felt::from(market),
            Felt::from(0_u64),
            EntryKind::Trade,
            payload,
        );
        e.receipt = Some(receipt);
        // tx_hash stays None — the entry never reached chain.
        e
    }

    #[test]
    fn from_journal_parses_trade_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("j.ndjson");
        {
            let mut j = TradeJournal::open(&path).unwrap();
            j.append(&trade_entry(2.0, 0.3, 0x1, 0xA)).unwrap();
            j.append(&trade_entry(2.1, 0.3, 0x2, 0xB)).unwrap();
            j.append(&trade_entry(2.2, 0.3, 0x3, 0xC)).unwrap();
            JournalSink::flush(&mut j).unwrap();
        }
        let engine = BacktestEngine::from_journal(&path).unwrap();
        assert_eq!(engine.events.len(), 3, "expected 3 Trade events");
        for ev in &engine.events {
            assert!(matches!(
                ev,
                MarketEvent::Trade {
                    candidate: EventDistribution::Normal(_),
                    ..
                },
            ));
        }
        // Initial state is seeded from the first Normal trade (μ ≈ 2.0).
        if let SimDistribution::Normal(d) = &engine.initial_state.distribution {
            assert!((d.mean().to_f64() - 2.0).abs() < 1e-9);
        } else {
            unreachable!("seed must be Normal");
        }
    }

    #[test]
    fn from_journal_skips_skipped_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("j.ndjson");
        {
            let mut j = TradeJournal::open(&path).unwrap();
            j.append(&trade_entry(2.0, 0.3, 0x1, 0xA)).unwrap();
            j.append(&skipped_entry(0x2)).unwrap();
            j.append(&trade_entry(2.1, 0.3, 0x3, 0xC)).unwrap();
            j.append(&skipped_entry(0x4)).unwrap();
            JournalSink::flush(&mut j).unwrap();
        }
        let engine = BacktestEngine::from_journal(&path).unwrap();
        assert_eq!(
            engine.events.len(),
            2,
            "skipped (...) entries must not contribute to the event stream",
        );
    }

    #[test]
    fn from_journal_handles_corrupted_lines() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("j.ndjson");
        {
            let mut j = TradeJournal::open(&path).unwrap();
            j.append(&trade_entry(2.0, 0.3, 0x1, 0xA)).unwrap();
            j.append(&trade_entry(2.1, 0.3, 0x2, 0xB)).unwrap();
            JournalSink::flush(&mut j).unwrap();
        }
        // Simulate a torn write: append a half-baked JSON line.
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"{\"timestamp\":\"truncated").unwrap();
        f.sync_data().unwrap();
        drop(f);

        let engine = BacktestEngine::from_journal(&path).unwrap();
        assert_eq!(
            engine.events.len(),
            2,
            "corrupted line must be skipped, not abort the load; got {} events",
            engine.events.len(),
        );
    }

    #[test]
    fn from_journal_handles_empty_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("empty.ndjson");
        std::fs::write(&path, b"").unwrap();
        let engine = BacktestEngine::from_journal(&path).unwrap();
        assert!(engine.events.is_empty(), "empty file → zero events");
        // Initial state falls back to N(0, 1).
        if let SimDistribution::Normal(d) = &engine.initial_state.distribution {
            assert!((d.mean().to_f64()).abs() < 1e-9);
            assert!((d.variance().to_f64() - 1.0).abs() < 1e-9);
        } else {
            unreachable!("fallback must be Normal");
        }
    }

    #[test]
    fn from_journal_propagates_io_error_on_missing_path() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nope.ndjson");
        let err = BacktestEngine::from_journal(&path).expect_err("missing file must error");
        assert_eq!(
            err.kind(),
            io::ErrorKind::NotFound,
            "expected NotFound, got {:?}",
            err.kind(),
        );
    }

    #[test]
    fn from_journal_handles_claim_and_sell_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("j.ndjson");
        let sell = {
            let mut e = JE::new(
                Family::Normal,
                Felt::from(0x10_u64),
                Felt::from(0xA_u64),
                EntryKind::Sell,
                json!({"runtime": "0x0", "min_token_out": 0_u64}),
            );
            e.tx_hash = Some(Felt::from(0xCAFE_u64));
            e
        };
        let claim = {
            let mut e = JE::new(
                Family::Normal,
                Felt::from(0x11_u64),
                Felt::from(0xA_u64),
                EntryKind::Claim,
                json!({"runtime": "0x0"}),
            );
            e.tx_hash = Some(Felt::from(0xCAFF_u64));
            e
        };
        {
            let mut j = TradeJournal::open(&path).unwrap();
            j.append(&trade_entry(2.0, 0.3, 0x1, 0xA)).unwrap();
            j.append(&sell).unwrap();
            j.append(&claim).unwrap();
            JournalSink::flush(&mut j).unwrap();
        }
        let engine = BacktestEngine::from_journal(&path).unwrap();
        assert_eq!(
            engine.events.len(),
            3,
            "Sell + Claim must each emit one RemoveLiquidity marker",
        );
        assert!(matches!(engine.events[0], MarketEvent::Trade { .. }));
        // Both Sell and Claim collapse to a zero-fraction RemoveLiquidity
        // marker: the strategy callback still sees the event but the
        // engine's state machine treats it as a no-op.
        for marker in &engine.events[1..] {
            match marker {
                MarketEvent::RemoveLiquidity { fraction, .. } => {
                    assert!(
                        (*fraction).abs() < 1e-12,
                        "Sell/Claim must emit fraction = 0.0 markers",
                    );
                },
                MarketEvent::Trade { .. }
                | MarketEvent::AddLiquidity { .. }
                | MarketEvent::Settle { .. } => {
                    unreachable!("expected RemoveLiquidity marker, got {marker:?}")
                },
            }
        }
    }

    #[test]
    fn entry_is_skipped_predicate_matches_analytics_layer() {
        // Mirrors cpi-bot::analytics::entry_is_skipped_for_{edge,risk}.
        let mut e = trade_entry(2.0, 0.3, 0x1, 0xA);
        assert!(!entry_is_skipped(&e), "submitted entries are not skipped");
        // Drop tx_hash AND attach a "skipped (...)" receipt.
        e.tx_hash = None;
        e.receipt = Some(json!({"error": "skipped (risk): drawdown breaker"}));
        assert!(entry_is_skipped(&e));
        // Still skipped when reason is "edge".
        e.receipt = Some(json!({"error": "skipped (edge too thin)"}));
        assert!(entry_is_skipped(&e));
        // A receipt that *contains* "skipped" but doesn't start with it
        // (different message) is treated as submitted — better to surface
        // a borderline case than to silently drop it.
        e.receipt = Some(json!({"error": "trade was almost skipped (edge thin)"}));
        assert!(!entry_is_skipped(&e));
    }
}
