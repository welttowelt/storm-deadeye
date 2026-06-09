//! Async HTTP client for the Deadeye indexer.

use std::time::Duration;

use reqwest::StatusCode;
use thiserror::Error;
use url::Url;

use crate::dto::{
    ActivityFeedItem, AnalyticsDomain, AnalyticsOverview, Health, LpEntry, LpHistoryEvent,
    MarketEvent, MarketSummary, MultinoulliSnapshot, Paginated, Position, Ranking, TraderEntry,
    TraderEvent, TraderStats,
};

/// Errors emitted by [`IndexerClient`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum IndexerError {
    /// Transport-level error from `reqwest`.
    #[error("indexer HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    /// Indexer returned a non-2xx status code.
    #[error("indexer returned HTTP {status} for {path}: {body}")]
    Status {
        /// HTTP status code.
        status: StatusCode,
        /// Request path that failed.
        path: String,
        /// First 256 bytes of the response body, for diagnostics.
        body: String,
    },
    /// JSON deserialisation failed.
    #[error("failed to decode {path}: {source}")]
    Decode {
        /// Request path whose body failed to parse.
        path: String,
        /// Underlying serde error.
        #[source]
        source: serde_json::Error,
    },
    /// Base URL parsing failed.
    #[error("invalid indexer base URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
}

/// Typed HTTP client for the Deadeye indexer.
#[derive(Debug, Clone)]
pub struct IndexerClient {
    base: Url,
    http: reqwest::Client,
}

impl IndexerClient {
    /// Construct a client against the canonical mainnet indexer.
    pub fn mainnet() -> Result<Self, IndexerError> {
        Self::new(crate::MAINNET_URL)
    }

    /// Construct a client against an arbitrary base URL.
    pub fn new(base: &str) -> Result<Self, IndexerError> {
        let base = Url::parse(base)?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("deadeye-indexer/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { base, http })
    }

    /// Reads `/health`.
    pub async fn health(&self) -> Result<Health, IndexerError> {
        self.get_json("/health").await
    }

    /// Reads `/api/markets`.
    pub async fn markets(&self) -> Result<Vec<MarketSummary>, IndexerError> {
        self.get_json("/api/markets").await
    }

    /// Reads `/api/markets/:address`.
    pub async fn market(&self, address: &str) -> Result<MarketSummary, IndexerError> {
        self.get_json(&format!("/api/markets/{address}")).await
    }

    /// Reads `/api/positions/:trader` — every open position for a trader.
    pub async fn positions(&self, trader: &str) -> Result<Vec<Position>, IndexerError> {
        self.get_json(&format!("/api/positions/{trader}")).await
    }

    /// Reads `/api/positions/:trader/stats`.
    pub async fn trader_stats(&self, trader: &str) -> Result<TraderStats, IndexerError> {
        self.get_json(&format!("/api/positions/{trader}/stats"))
            .await
    }

    /// Reads `/api/rankings?limit=N`.
    pub async fn rankings(&self, limit: u32) -> Result<Vec<Ranking>, IndexerError> {
        self.get_json(&format!("/api/rankings?limit={limit}")).await
    }

    /// Reads `/api/markets/:address/events?page=&pageSize=`.
    pub async fn market_events(
        &self,
        market: &str,
        page: u32,
        page_size: u32,
    ) -> Result<Paginated<MarketEvent>, IndexerError> {
        self.get_json(&format!(
            "/api/markets/{market}/events?page={page}&pageSize={page_size}"
        ))
        .await
    }

    /// Reads `/api/markets/:address/events?page=&pageSize=&from=&to=`.
    ///
    /// Filtered counterpart of [`Self::market_events`]; `from` and `to`
    /// are unix-second timestamps (inclusive).
    pub async fn market_events_in_range(
        &self,
        market: &str,
        page: u32,
        page_size: u32,
        from: u64,
        to: u64,
    ) -> Result<Paginated<MarketEvent>, IndexerError> {
        self.get_json(&format!(
            "/api/markets/{market}/events?page={page}&pageSize={page_size}&from={from}&to={to}"
        ))
        .await
    }

    /// Reads `/api/markets/:address/traders` — per-trader summaries on
    /// the given market.
    pub async fn market_traders(&self, market: &str) -> Result<Vec<TraderEntry>, IndexerError> {
        self.get_json(&format!("/api/markets/{market}/traders"))
            .await
    }

    /// Reads `/api/markets/:address/lps` — per-provider LP summaries.
    pub async fn market_lps(&self, market: &str) -> Result<Vec<LpEntry>, IndexerError> {
        self.get_json(&format!("/api/markets/{market}/lps")).await
    }

    /// Reads `/api/markets/:address/lps/:provider/history` — full LP
    /// event log for one provider on one market.
    pub async fn lp_history(
        &self,
        market: &str,
        provider: &str,
    ) -> Result<Vec<LpHistoryEvent>, IndexerError> {
        self.get_json(&format!("/api/markets/{market}/lps/{provider}/history"))
            .await
    }

    /// Reads `/api/markets/:address/multinoulli-snapshots?limit=N`.
    pub async fn multinoulli_snapshots(
        &self,
        market: &str,
        limit: u32,
    ) -> Result<Paginated<MultinoulliSnapshot>, IndexerError> {
        self.get_json(&format!(
            "/api/markets/{market}/multinoulli-snapshots?limit={limit}"
        ))
        .await
    }

    /// Reads `/api/positions/:trader/events?page=&pageSize=`.
    pub async fn trader_events(
        &self,
        trader: &str,
        page: u32,
        page_size: u32,
    ) -> Result<Paginated<TraderEvent>, IndexerError> {
        self.get_json(&format!(
            "/api/positions/{trader}/events?page={page}&pageSize={page_size}"
        ))
        .await
    }

    /// Reads `/api/positions/:trader/stats?domain=&from=&to=` —
    /// domain-scoped trader statistics.
    ///
    /// Any of `domain`, `from`, `to` may be `None`; the indexer
    /// treats omitted params as "no filter".
    pub async fn trader_stats_filtered(
        &self,
        trader: &str,
        domain: Option<&str>,
        from: Option<u64>,
        to: Option<u64>,
    ) -> Result<TraderStats, IndexerError> {
        use std::fmt::Write as _;
        let mut path = format!("/api/positions/{trader}/stats");
        let mut sep = '?';
        if let Some(d) = domain {
            let _ = write!(path, "{sep}domain={d}");
            sep = '&';
        }
        if let Some(f) = from {
            let _ = write!(path, "{sep}from={f}");
            sep = '&';
        }
        if let Some(t) = to {
            let _ = write!(path, "{sep}to={t}");
        }
        self.get_json(&path).await
    }

    /// Reads `/api/rankings?limit=N&domain=…&from=…&to=…` —
    /// leaderboard filtered by an optional domain / time window.
    pub async fn rankings_filtered(
        &self,
        limit: u32,
        domain: Option<&str>,
        from: Option<u64>,
        to: Option<u64>,
    ) -> Result<Vec<Ranking>, IndexerError> {
        use std::fmt::Write as _;
        let mut path = format!("/api/rankings?limit={limit}");
        if let Some(d) = domain {
            let _ = write!(path, "&domain={d}");
        }
        if let Some(f) = from {
            let _ = write!(path, "&from={f}");
        }
        if let Some(t) = to {
            let _ = write!(path, "&to={t}");
        }
        self.get_json(&path).await
    }

    /// Reads `/api/activity?limit=N&type=…&marketAddress=…&trader=…`.
    ///
    /// Filter params are optional — `None` means "no filter".
    pub async fn activity_feed(
        &self,
        limit: Option<u32>,
        activity_type: Option<&str>,
        market: Option<&str>,
        trader: Option<&str>,
    ) -> Result<Paginated<ActivityFeedItem>, IndexerError> {
        use std::fmt::Write as _;
        let mut path = String::from("/api/activity");
        let mut sep = '?';
        if let Some(l) = limit {
            let _ = write!(path, "{sep}limit={l}");
            sep = '&';
        }
        if let Some(t) = activity_type {
            let _ = write!(path, "{sep}type={t}");
            sep = '&';
        }
        if let Some(m) = market {
            let _ = write!(path, "{sep}marketAddress={m}");
            sep = '&';
        }
        if let Some(tr) = trader {
            let _ = write!(path, "{sep}trader={tr}");
        }
        self.get_json(&path).await
    }

    /// Reads `/api/analytics/overview` — top-level aggregate counters.
    pub async fn analytics_overview(&self) -> Result<AnalyticsOverview, IndexerError> {
        self.get_json("/api/analytics/overview").await
    }

    /// Reads `/api/analytics/domains/:slug` — per-domain analytics.
    pub async fn analytics_domain(&self, slug: &str) -> Result<AnalyticsDomain, IndexerError> {
        self.get_json(&format!("/api/analytics/domains/{slug}"))
            .await
    }

    async fn get_json<T>(&self, path: &str) -> Result<T, IndexerError>
    where
        T: serde::de::DeserializeOwned,
    {
        let url = self.base.join(path)?;
        let response = self.http.get(url).send().await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            let truncated = text.chars().take(256).collect::<String>();
            return Err(IndexerError::Status {
                status,
                path: path.to_owned(),
                body: truncated,
            });
        }
        serde_json::from_str(&text).map_err(|source| IndexerError::Decode {
            path: path.to_owned(),
            source,
        })
    }
}
