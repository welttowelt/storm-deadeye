#![allow(
    clippy::print_stderr,
    clippy::tests_outside_test_module,
    reason = "integration tests in tests/ are top-level — printing aids debugging"
)]

//! Smoke tests against the production Sepolia indexer at
//! `situation-indexer.fly.dev`.
//!
//! Enable with:
//!
//! ```bash
//! DEADEYE_RUN_INTEGRATION=1 cargo test -p deadeye-e2e --test indexer_smoke -- --nocapture
//! ```

use deadeye_indexer::IndexerClient;

fn integration_enabled() -> bool {
    std::env::var("DEADEYE_RUN_INTEGRATION").is_ok()
}

#[tokio::test]
async fn sepolia_indexer_is_healthy() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 to enable");
        return;
    }
    let client = IndexerClient::sepolia().expect("client builds");
    let health = client.health().await.expect("health responds");
    assert_eq!(
        health.status, "ok",
        "indexer reports status={}",
        health.status
    );
    assert_eq!(
        health.db_status, "connected",
        "db reports {}",
        health.db_status
    );
    eprintln!(
        "sepolia indexer: uptime={}s markets={} events={}",
        health.uptime, health.markets_count, health.events_count
    );
}

#[tokio::test]
async fn sepolia_indexer_lists_markets() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 to enable");
        return;
    }
    let client = IndexerClient::sepolia().expect("client builds");
    let markets = client.markets().await.expect("markets responds");
    assert!(!markets.is_empty(), "indexer must have at least one market");
    let first = &markets[0];
    eprintln!(
        "first market: address={} type={} title=\"{}\"",
        first.address, first.market_type, first.title
    );

    // Round-trip: fetch the single-market endpoint and confirm it matches.
    let detail = client
        .market(&first.address)
        .await
        .expect("detail responds");
    assert_eq!(detail.address, first.address);
    assert_eq!(detail.market_type, first.market_type);
}

#[tokio::test]
async fn sepolia_indexer_full_endpoint_surface() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 to enable");
        return;
    }
    let client = IndexerClient::sepolia().expect("client builds");

    // Rankings.
    let rankings = client.rankings(5).await.expect("rankings responds");
    eprintln!("rankings: {} rows", rankings.len());
    assert!(!rankings.is_empty());
    let top = &rankings[0];
    eprintln!("top trader: {} pnl={}", top.trader, top.total_pnl);

    // Trader stats for the top trader.
    let stats = client
        .trader_stats(&top.trader)
        .await
        .expect("trader_stats responds");
    assert_eq!(stats.trader, top.trader);
    eprintln!(
        "stats: total_pnl={} unrealized={:?} markets={} trades={}",
        stats.total_pnl, stats.unrealized_pnl, stats.markets_traded, stats.total_trades
    );

    // Positions for the top trader.
    let positions = client
        .positions(&top.trader)
        .await
        .expect("positions responds");
    eprintln!("positions: {} entries", positions.len());

    // Market events for the first listed market.
    let markets = client.markets().await.expect("markets responds");
    if let Some(market) = markets.first() {
        let events = client
            .market_events(&market.address, 0, 5)
            .await
            .expect("market_events responds");
        eprintln!(
            "market events for {}: {} rows",
            market.address,
            events.data.len()
        );
    }
}

#[tokio::test]
async fn sepolia_indexer_extended_endpoints() {
    if !integration_enabled() {
        eprintln!("skip: set DEADEYE_RUN_INTEGRATION=1 to enable");
        return;
    }
    let client = IndexerClient::sepolia().expect("client builds");
    let markets = client.markets().await.expect("markets responds");
    assert!(!markets.is_empty(), "indexer must have at least one market");
    let market_addr = markets[0].address.clone();

    // Per-market traders + LPs.
    let traders = client
        .market_traders(&market_addr)
        .await
        .expect("market_traders responds");
    eprintln!(
        "market_traders for {market_addr}: {} entries",
        traders.len()
    );

    let lps = client
        .market_lps(&market_addr)
        .await
        .expect("market_lps responds");
    eprintln!("market_lps for {market_addr}: {} entries", lps.len());

    // LP history (use the first LP if any).
    if let Some(lp) = lps.first() {
        let history = client
            .lp_history(&market_addr, &lp.provider)
            .await
            .expect("lp_history responds");
        eprintln!(
            "lp_history for {provider} on {market_addr}: {} events",
            history.len(),
            provider = lp.provider,
        );
    }

    // Market events with time-range filter.
    let events = client
        .market_events_in_range(&market_addr, 0, 5, 0, u64::MAX)
        .await
        .expect("market_events_in_range responds");
    eprintln!("market_events_in_range: {} rows", events.data.len());

    // Multinoulli snapshots (only meaningful for multinoulli markets;
    // probe the first multinoulli we find, fall through silently for
    // continuous families).
    if let Some(mn_market) = markets.iter().find(|m| m.market_type == "multinoulli") {
        let snaps = client
            .multinoulli_snapshots(&mn_market.address, 5)
            .await
            .expect("multinoulli_snapshots responds");
        eprintln!(
            "multinoulli_snapshots for {addr}: {} rows",
            snaps.data.len(),
            addr = mn_market.address,
        );
    }

    // Trader events.
    let rankings = client.rankings(1).await.expect("rankings responds");
    if let Some(top) = rankings.first() {
        let trader_events = client
            .trader_events(&top.trader, 0, 5)
            .await
            .expect("trader_events responds");
        eprintln!(
            "trader_events for {trader}: {} rows",
            trader_events.data.len(),
            trader = top.trader,
        );

        // Filtered trader stats (domain + window).
        let stats_filtered = client
            .trader_stats_filtered(&top.trader, Some("economics"), None, None)
            .await
            .expect("trader_stats_filtered responds");
        eprintln!(
            "trader_stats_filtered economics for {trader}: total_pnl={}",
            stats_filtered.total_pnl,
            trader = top.trader,
        );
    }

    // Filtered rankings.
    let rankings_dom = client
        .rankings_filtered(5, Some("other"), None, None)
        .await
        .expect("rankings_filtered responds");
    eprintln!(
        "rankings_filtered domain=other: {} rows",
        rankings_dom.len()
    );

    // Activity feed.
    let activity = client
        .activity_feed(Some(5), None, None, None)
        .await
        .expect("activity_feed responds");
    eprintln!("activity_feed: {} rows", activity.data.len());

    // Analytics.
    let overview = client
        .analytics_overview()
        .await
        .expect("analytics_overview responds");
    eprintln!(
        "analytics_overview: traders={} trades={} volume={}",
        overview.totals.traders, overview.totals.trades, overview.totals.volume,
    );

    let domain = client
        .analytics_domain("economics")
        .await
        .expect("analytics_domain responds");
    eprintln!(
        "analytics_domain economics: active_markets={} traders={} trades={}",
        domain.active_markets, domain.unique_traders, domain.total_trades,
    );

    eprintln!("✅ all extended indexer endpoints deserialised");
}
