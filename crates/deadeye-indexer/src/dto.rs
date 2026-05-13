//! Data-transfer objects for the indexer HTTP API.
//!
//! These types mirror the JSON returned by `situation-indexer.fly.dev`. We
//! deliberately keep them shallow — fields are `Option` whenever the
//! upstream payload is allowed to omit them, so a single DTO accommodates
//! every market shape (normal / lognormal / multinoulli / bivariate).

use serde::{Deserialize, Serialize};

/// Health-check response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Health {
    /// Status string; typically `"ok"`.
    pub status: String,
    /// Process uptime in seconds.
    pub uptime: u64,
    /// Database status: `"connected"` when healthy.
    #[serde(rename = "dbStatus")]
    pub db_status: String,
    /// Number of markets the indexer has snapshot so far.
    #[serde(rename = "marketsCount")]
    pub markets_count: u64,
    /// Number of events the indexer has ingested.
    #[serde(rename = "eventsCount")]
    pub events_count: u64,
    /// ISO timestamp of the last poll, if any.
    #[serde(rename = "lastPollAt")]
    pub last_poll_at: Option<String>,
}

/// One row of `/api/markets`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarketSummary {
    /// On-chain contract address (`0x…`).
    pub address: String,
    /// Operator-set market title.
    pub title: String,
    /// Operator-set market description.
    #[serde(default)]
    pub description: String,
    /// Operator-set category.
    #[serde(default)]
    pub category: Option<String>,
    /// Topic tags.
    #[serde(default)]
    pub topics: Vec<String>,
    /// Market family — `normal`, `lognormal`, `multinoulli`, `bivariate`.
    #[serde(rename = "marketType")]
    pub market_type: String,
    /// Whether the market is currently accepting trades.
    #[serde(rename = "isActive", default)]
    pub is_active: bool,
    /// State snapshot for normal markets (mean / sigma / k / backing).
    #[serde(default)]
    pub state: Option<NormalState>,
    /// State snapshot for multinoulli markets.
    #[serde(rename = "multinoulliState", default)]
    pub multinoulli_state: Option<MultinoulliState>,
    /// Creation timestamp (unix seconds).
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    /// Last update timestamp (unix seconds).
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
}

/// State snapshot for a normal / lognormal / bivariate AMM market.
///
/// Numeric fields are `Option<f64>` because the indexer emits `null` for
/// values that are not yet known (e.g. before the AMM is initialised) and
/// for fields that don't apply to the current market type (e.g. `mean2`
/// only exists for bivariate markets).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NormalState {
    /// Off-chain mean.
    #[serde(default)]
    pub mean: Option<f64>,
    /// Off-chain variance.
    #[serde(default)]
    pub variance: Option<f64>,
    /// Off-chain σ.
    #[serde(default)]
    pub sigma: Option<f64>,
    /// AMM `k` parameter.
    #[serde(default)]
    pub k: Option<f64>,
    /// `effective_k` derived from backing.
    #[serde(rename = "effectiveK", default)]
    pub effective_k: Option<f64>,
    /// Total backing (string-encoded fixed-point for precision).
    #[serde(rename = "totalBacking", default)]
    pub total_backing: Option<String>,
    /// Whether the market is initialised.
    #[serde(rename = "isInitialized")]
    pub is_initialised: bool,
    /// Whether the market is paused.
    #[serde(rename = "isPaused")]
    pub is_paused: bool,
    /// Whether the market is settled.
    #[serde(rename = "isSettled")]
    pub is_settled: bool,
    /// Unix timestamp of the snapshot.
    #[serde(rename = "fetchedAt")]
    pub fetched_at: u64,
}

/// Per-trader position summary returned by `/api/positions/:trader`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Position {
    /// Market address.
    #[serde(rename = "marketAddress")]
    pub market_address: String,
    /// Trader address.
    pub trader: String,
    /// Whether the trader currently holds a position.
    #[serde(rename = "hasPosition")]
    pub has_position: bool,
    /// Collateral locked, string-encoded fixed-point.
    #[serde(rename = "collateralLocked", default)]
    pub collateral_locked: Option<String>,
    /// Position entry mean.
    #[serde(default)]
    pub mean: Option<f64>,
    /// Position entry σ.
    #[serde(default)]
    pub sigma: Option<f64>,
    /// Position entry variance.
    #[serde(default)]
    pub variance: Option<f64>,
    /// Settlement state (`pending`, `claimed`, `unclaimed`).
    #[serde(rename = "settlementState", default)]
    pub settlement_state: Option<String>,
    /// Unrealised P&L at last fetch.
    #[serde(rename = "unrealizedPnl", default)]
    pub unrealized_pnl: Option<f64>,
    /// Realised P&L (only meaningful after settlement).
    #[serde(rename = "realizedPnl", default)]
    pub realized_pnl: Option<f64>,
    /// Expected value at the current market state.
    #[serde(rename = "expectedValue", default)]
    pub expected_value: Option<f64>,
    /// Whether the position has been claimed.
    #[serde(default)]
    pub claimed: bool,
    /// Unix timestamp of the snapshot.
    #[serde(rename = "fetchedAt", default)]
    pub fetched_at: Option<u64>,
}

/// Single market event returned by `/api/markets/:address/events`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarketEvent {
    /// Indexer-assigned id.
    pub id: u64,
    /// Transaction hash.
    #[serde(rename = "txHash")]
    pub tx_hash: String,
    /// Block number.
    #[serde(rename = "blockNumber")]
    pub block_number: u64,
    /// Unix timestamp.
    pub timestamp: u64,
    /// Event type (`market_initialized`, `trade`, `sell`, `claim`, …).
    #[serde(rename = "eventType")]
    pub event_type: String,
    /// Trader address (when applicable).
    #[serde(default)]
    pub trader: Option<String>,
    /// Post-event mean.
    #[serde(default)]
    pub mean: Option<f64>,
    /// Post-event σ (the indexer reports it as `stdDev`).
    #[serde(rename = "stdDev", default)]
    pub std_dev: Option<f64>,
    /// Collateral posted (string-encoded).
    #[serde(rename = "collateralPosted", default)]
    pub collateral_posted: Option<String>,
}

/// Pagination wrapper for endpoints that return `{ data: [...] }`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Paginated<T> {
    /// Page rows.
    pub data: Vec<T>,
}

/// Leaderboard row returned by `/api/rankings`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Ranking {
    /// Trader address.
    pub trader: String,
    /// Total realised P&L.
    #[serde(rename = "totalPnl")]
    pub total_pnl: f64,
    /// Distinct markets traded.
    #[serde(rename = "marketsTraded")]
    pub markets_traded: u64,
    /// Total trades.
    #[serde(rename = "totalTrades")]
    pub total_trades: u64,
}

/// Aggregated trader statistics from `/api/positions/:trader/stats`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TraderStats {
    /// Trader address.
    pub trader: String,
    /// Total realised P&L.
    #[serde(rename = "totalPnl")]
    pub total_pnl: f64,
    /// Unrealised P&L.
    #[serde(rename = "unrealizedPnl", default)]
    pub unrealized_pnl: Option<f64>,
    /// Largest single-trade win.
    #[serde(rename = "biggestWin", default)]
    pub biggest_win: Option<f64>,
    /// Total trades.
    #[serde(rename = "totalTrades")]
    pub total_trades: u64,
    /// Markets the trader has touched.
    #[serde(rename = "marketsTraded")]
    pub markets_traded: u64,
    /// Earliest trade timestamp.
    #[serde(rename = "firstTradeAt", default)]
    pub first_trade_at: Option<u64>,
    /// Latest trade timestamp.
    #[serde(rename = "lastTradeAt", default)]
    pub last_trade_at: Option<u64>,
}

/// One row of `/api/markets/:address/traders`.
///
/// Mirrors the indexer's per-trader-per-market summary. Same shape as
/// [`Position`] but anchored to a specific market — the `marketAddress`
/// field is always set.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TraderEntry {
    /// Market the entry belongs to.
    #[serde(rename = "marketAddress")]
    pub market_address: String,
    /// Trader address.
    pub trader: String,
    /// Whether the trader currently holds a position.
    #[serde(rename = "hasPosition")]
    pub has_position: bool,
    /// Collateral locked (string-encoded base units).
    #[serde(rename = "collateralLocked", default)]
    pub collateral_locked: Option<String>,
    /// Position entry mean.
    #[serde(default)]
    pub mean: Option<f64>,
    /// Position entry σ.
    #[serde(default)]
    pub sigma: Option<f64>,
    /// Position entry variance.
    #[serde(default)]
    pub variance: Option<f64>,
    /// Settlement state (`pending`, `active`, `claimed`, `unclaimed`).
    #[serde(rename = "settlementState", default)]
    pub settlement_state: Option<String>,
    /// Unrealised P&L at last fetch.
    #[serde(rename = "unrealizedPnl", default)]
    pub unrealized_pnl: Option<f64>,
    /// Realised P&L (only meaningful after settlement).
    #[serde(rename = "realizedPnl", default)]
    pub realized_pnl: Option<f64>,
    /// Expected value at the current market state.
    #[serde(rename = "expectedValue", default)]
    pub expected_value: Option<f64>,
    /// Whether the position has been claimed.
    #[serde(default)]
    pub claimed: bool,
    /// Unix timestamp of the snapshot.
    #[serde(rename = "fetchedAt", default)]
    pub fetched_at: Option<u64>,
}

/// One row of `/api/markets/:address/lps`.
///
/// Per-provider summary of LP exposure on a single market.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LpEntry {
    /// Market the entry belongs to.
    #[serde(rename = "marketAddress")]
    pub market_address: String,
    /// LP provider address.
    pub provider: String,
    /// String-encoded LP shares (base units).
    #[serde(default)]
    pub shares: Option<String>,
    /// Numeric f64 view of the LP shares (`None` when un-derivable).
    #[serde(rename = "sharesNumber", default)]
    pub shares_number: Option<f64>,
    /// Total tokens deposited across the LP's lifetime.
    #[serde(rename = "totalDeposited", default)]
    pub total_deposited: f64,
    /// Total tokens withdrawn across the LP's lifetime.
    #[serde(rename = "totalWithdrawn", default)]
    pub total_withdrawn: f64,
    /// Current claim value at last snapshot (if known).
    #[serde(rename = "currentValue", default)]
    pub current_value: Option<f64>,
    /// Unrealised P&L at last snapshot.
    #[serde(rename = "unrealizedPnl", default)]
    pub unrealized_pnl: Option<f64>,
    /// Number of `add_liquidity` events.
    #[serde(rename = "depositCount", default)]
    pub deposit_count: u64,
    /// Number of `remove_liquidity` events.
    #[serde(rename = "withdrawalCount", default)]
    pub withdrawal_count: u64,
    /// First deposit timestamp (unix seconds).
    #[serde(rename = "firstDepositAt", default)]
    pub first_deposit_at: Option<u64>,
    /// Most recent LP activity timestamp.
    #[serde(rename = "lastActivityAt", default)]
    pub last_activity_at: Option<u64>,
    /// Snapshot timestamp.
    #[serde(rename = "fetchedAt", default)]
    pub fetched_at: Option<u64>,
}

/// One row of `/api/markets/:address/lps/:provider/history`.
///
/// LP-side event log. Covers `liquidity_added`, `liquidity_removed`,
/// `lp_claim`, and `market_initialized` (when the admin seeds the pool
/// as its first LP).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LpHistoryEvent {
    /// `liquidity_added` | `liquidity_removed` | `lp_claim` |
    /// `market_initialized`.
    #[serde(rename = "eventType")]
    pub event_type: String,
    /// Unix timestamp.
    pub timestamp: u64,
    /// Token delta as a string of base units (18 decimals on STRK).
    /// Positive for deposits, positive for withdrawals (the sign is
    /// carried by `eventType`).
    #[serde(rename = "tokenAmount")]
    pub token_amount: String,
    /// LP-shares delta as a string of base units.
    #[serde(default)]
    pub shares: Option<String>,
    /// Originating transaction.
    #[serde(rename = "txHash")]
    pub tx_hash: String,
    /// Originating block.
    #[serde(rename = "blockNumber")]
    pub block_number: u64,
}

/// One row of `/api/positions/:trader/events`.
///
/// Trader-side event log — every market-affecting event the indexer
/// has observed for `:trader`, across every market they touched.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TraderEvent {
    /// Indexer-assigned id.
    pub id: u64,
    /// Market this event landed on.
    #[serde(rename = "marketAddress")]
    pub market_address: String,
    /// Market title (cached snapshot at event time).
    #[serde(rename = "marketTitle", default)]
    pub market_title: Option<String>,
    /// Transaction hash.
    #[serde(rename = "txHash")]
    pub tx_hash: String,
    /// Block number.
    #[serde(rename = "blockNumber")]
    pub block_number: u64,
    /// Unix timestamp.
    pub timestamp: u64,
    /// Event type — `trade_executed`, `liquidity_added`, …
    #[serde(rename = "eventType")]
    pub event_type: String,
    /// Trader address (always the path arg; mirrored for downstream
    /// consumers that fan trader events together with market events).
    #[serde(default)]
    pub trader: Option<String>,
    /// Post-event mean (normal / lognormal markets).
    #[serde(default)]
    pub mean: Option<f64>,
    /// Post-event σ.
    #[serde(rename = "stdDev", default)]
    pub std_dev: Option<f64>,
    /// Collateral posted, string-encoded base units.
    #[serde(rename = "collateralPosted", default)]
    pub collateral_posted: Option<String>,
}

/// One row of `/api/activity`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ActivityFeedItem {
    /// Indexer-assigned id.
    pub id: u64,
    /// Unix timestamp.
    pub timestamp: u64,
    /// Originating transaction.
    #[serde(rename = "txHash")]
    pub tx_hash: String,
    /// Originating block.
    #[serde(rename = "blockNumber")]
    pub block_number: u64,
    /// Coarse activity bucket (`trade_move`, `liquidity_add`, …).
    #[serde(rename = "activityType")]
    pub activity_type: String,
    /// Raw on-chain event type, e.g. `trade_executed`.
    #[serde(rename = "rawEventType", default)]
    pub raw_event_type: Option<String>,
    /// Market address.
    #[serde(rename = "marketAddress")]
    pub market_address: String,
    /// Market title (cached at event time).
    #[serde(rename = "marketTitle", default)]
    pub market_title: Option<String>,
    /// Market category.
    #[serde(rename = "marketCategory", default)]
    pub market_category: Option<String>,
    /// Market family.
    #[serde(rename = "marketType", default)]
    pub market_type: Option<String>,
    /// Acting trader (when applicable).
    #[serde(default)]
    pub trader: Option<String>,
    /// Collateral posted in this event, string-encoded base units.
    #[serde(rename = "collateralPosted", default)]
    pub collateral_posted: Option<String>,
    /// Collateral as f64 STRK.
    #[serde(rename = "collateralFloat", default)]
    pub collateral_float: Option<f64>,
    /// Free-form human summary the indexer renders for UI feeds.
    #[serde(default)]
    pub summary: Option<String>,
}

/// One row of `/api/markets/:address/multinoulli-snapshots`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MultinoulliSnapshot {
    /// Unix timestamp.
    pub timestamp: u64,
    /// Per-outcome probabilities at the snapshot's time.
    #[serde(default)]
    pub probs: Vec<f64>,
    /// Number of outcomes.
    #[serde(rename = "outcomeCount")]
    pub outcome_count: u64,
    /// Number of matrix rows (0 when matrix-mode disabled).
    #[serde(rename = "matrixRows", default)]
    pub matrix_rows: u64,
    /// Number of matrix columns (0 when matrix-mode disabled).
    #[serde(rename = "matrixCols", default)]
    pub matrix_cols: u64,
}

/// `/api/analytics/overview` payload.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnalyticsOverview {
    /// Top-level aggregate counters.
    pub totals: AnalyticsTotals,
    /// Percentage of traders with positive ROI.
    #[serde(rename = "positiveRoiPct")]
    pub positive_roi_pct: f64,
    /// New traders in the last 7 days.
    #[serde(rename = "newTradersLast7d", default)]
    pub new_traders_last_7d: u64,
    /// New LPs in the last 7 days.
    #[serde(rename = "newLpsLast7d", default)]
    pub new_lps_last_7d: u64,
    /// Top domains by volume.
    #[serde(rename = "topDomainsByVolume", default)]
    pub top_domains_by_volume: Vec<DomainVolumeRow>,
    /// ISO-8601 computation timestamp.
    #[serde(rename = "computedAt", default)]
    pub computed_at: Option<String>,
}

/// Aggregate counters returned inside [`AnalyticsOverview`].
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct AnalyticsTotals {
    /// Distinct trader addresses ever seen.
    pub traders: u64,
    /// Distinct LP addresses ever seen.
    pub lps: u64,
    /// Total trade count.
    pub trades: u64,
    /// Total cumulative volume in STRK.
    pub volume: f64,
    /// Currently-active market count.
    #[serde(rename = "activeMarkets")]
    pub active_markets: u64,
}

/// One row inside [`AnalyticsOverview::top_domains_by_volume`].
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DomainVolumeRow {
    /// Domain slug (`economics`, `sports`, …).
    pub slug: String,
    /// Human-readable name.
    pub name: String,
    /// Cumulative volume.
    pub volume: f64,
    /// Trade count in the domain.
    pub trades: u64,
    /// Distinct traders in the domain.
    #[serde(rename = "uniqueTraders")]
    pub unique_traders: u64,
}

/// `/api/analytics/domains/:slug` payload.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnalyticsDomain {
    /// Slug.
    pub slug: String,
    /// Human-readable name.
    pub name: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Active-market count in the domain.
    #[serde(rename = "activeMarkets")]
    pub active_markets: u64,
    /// Distinct traders.
    #[serde(rename = "uniqueTraders")]
    pub unique_traders: u64,
    /// Distinct LPs.
    #[serde(rename = "uniqueLps")]
    pub unique_lps: u64,
    /// Total volume.
    #[serde(rename = "totalVolume")]
    pub total_volume: f64,
    /// Total trade count.
    #[serde(rename = "totalTrades")]
    pub total_trades: u64,
    /// Positive-ROI percentage.
    #[serde(rename = "positiveRoiPct")]
    pub positive_roi_pct: f64,
    /// Median ROI.
    #[serde(rename = "medianRoiPct", default)]
    pub median_roi_pct: Option<f64>,
    /// Time-series of daily aggregates.
    #[serde(rename = "timeSeries", default)]
    pub time_series: Vec<DomainTimeSeriesRow>,
}

/// One row of [`AnalyticsDomain::time_series`].
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DomainTimeSeriesRow {
    /// ISO-8601 date (`YYYY-MM-DD`).
    pub date: String,
    /// Trade count.
    pub trades: u64,
    /// Volume.
    pub volume: f64,
    /// Distinct traders that day.
    #[serde(rename = "uniqueTraders")]
    pub unique_traders: u64,
    /// Distinct LPs that day.
    #[serde(rename = "uniqueLps")]
    pub unique_lps: u64,
    /// Net liquidity added (LP+ minus LP-).
    #[serde(rename = "liquidityNet", default)]
    pub liquidity_net: f64,
}

/// State snapshot for a multinoulli market.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MultinoulliState {
    /// Number of outcomes.
    #[serde(rename = "outcomeCount")]
    pub outcome_count: u64,
    /// Per-outcome probabilities.
    #[serde(default)]
    pub probs: Vec<f64>,
    /// AMM `k` parameter.
    #[serde(default)]
    pub k: Option<f64>,
    /// `effective_k` derived from backing.
    #[serde(rename = "effectiveK", default)]
    pub effective_k: Option<f64>,
    /// String-encoded total backing (preserves on-chain precision).
    #[serde(rename = "totalBacking", default)]
    pub total_backing: Option<String>,
    /// Whether the market is initialised.
    #[serde(rename = "isInitialized")]
    pub is_initialised: bool,
    /// Whether the market is paused.
    #[serde(rename = "isPaused")]
    pub is_paused: bool,
    /// Whether the market is settled.
    #[serde(rename = "isSettled")]
    pub is_settled: bool,
    /// Unix timestamp of the snapshot.
    #[serde(rename = "fetchedAt")]
    pub fetched_at: u64,
}
