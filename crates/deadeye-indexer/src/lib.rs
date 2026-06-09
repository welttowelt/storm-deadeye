//! Typed HTTP client for the Deadeye indexer.
//!
//! The indexer is the canonical source of *aggregated* market data — it
//! mirrors on-chain events into a queryable HTTP surface and is the
//! recommended way for an MM bot to discover markets, watch positions,
//! and tail the activity feed without paying the per-RPC-call latency
//! cost of polling Starknet directly.
//!
//! Default deployment (matching the upstream `the-situation` stack):
//!
//! * Mainnet → <https://178-105-210-177.sslip.io>
//!
//! ## Module layout
//!
//! * [`dto`] — pure data-transfer objects, usable in `no_std`.
//! * [`client`] — async client that ties the DTOs to a real HTTP backend. Gated
//!   behind the `client` feature.

#![doc(html_no_source)]

pub mod dto;

#[cfg(feature = "client")]
pub mod client;

#[cfg(feature = "client")]
pub use client::{IndexerClient, IndexerError};
pub use dto::{
    ActivityFeedItem, AnalyticsDomain, AnalyticsOverview, AnalyticsTotals, DomainTimeSeriesRow,
    DomainVolumeRow, Health, LpEntry, LpHistoryEvent, MarketEvent, MarketSummary,
    MultinoulliSnapshot, MultinoulliState, NormalState, Paginated, Position, Ranking, TraderEntry,
    TraderEvent, TraderStats,
};

/// Canonical mainnet indexer URL (Hetzner, via sslip.io).
pub const MAINNET_URL: &str = "https://178-105-210-177.sslip.io";
