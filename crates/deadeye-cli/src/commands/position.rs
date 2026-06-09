//! `deadeye position …` — list / show read paths.
//!
//! The `Sell` variant is owned by Driver B; here it returns a friendly
//! "not implemented in this driver" message so the binary still builds
//! and `--help` still documents the command.

use anyhow::{Context as _, Result};
use deadeye_core::{Distribution as _, Sq128};
use deadeye_sdk::bulk::Family;
use deadeye_starknet::{
    BivariateMarketReader, LognormalMarketReader, MultinoulliMarketReader, NormalMarketReader,
};
use serde_json::json;

use crate::{
    cli::{FamilyArg, PositionCmd, PositionSellArgs},
    commands::{
        render_helpers::{
            LegRow, LegValueRow, PositionLegsView, PositionValueView, submission_from_receipt,
            submission_from_trade_error,
        },
        runtime_resolver::{
            build_owned_account, build_provider, family_label, parse_felt, resolve_runtime,
        },
    },
    context::{AppContext, parse_address},
    output::OutputMode,
    render::{PositionRow, PositionShowView},
};

pub(crate) async fn run(action: PositionCmd, ctx: &AppContext, confirm: bool) -> Result<()> {
    match action {
        PositionCmd::List {
            trader,
            family,
            limit,
        } => list(ctx, trader, family, limit).await,
        PositionCmd::Show {
            market,
            trader,
            family,
        } => show(ctx, &market, trader, family).await,
        PositionCmd::Value {
            market,
            trader,
            at,
            belief,
            belief_sigma,
            family,
        } => value(ctx, &market, trader, at, belief, belief_sigma, family).await,
        PositionCmd::Sell(args) => sell(ctx, args, confirm).await,
    }
}

/// Resolve the trader address from `--trader` or the active profile.
fn resolve_trader(ctx: &AppContext, trader_opt: Option<String>) -> Result<deadeye_starknet::Felt> {
    let trader_str = match trader_opt {
        Some(s) => s,
        None => ctx.config.address.clone().context(
            "no trader address — pass --trader, set DEADEYE_ADDRESS, or configure a profile",
        )?,
    };
    parse_address(&trader_str)
}

async fn sell(ctx: &AppContext, args: PositionSellArgs, confirm: bool) -> Result<()> {
    use deadeye_starknet::{
        BivariateMarketWriter, LognormalMarketWriter, MultinoulliMarketWriter, NormalMarketWriter,
    };

    let market = parse_felt("market address", &args.market)?;
    let provider = build_provider(ctx)?;
    let client = deadeye_sdk::DeadeyeClient::new(provider);

    let family = match args.family {
        Some(f) => f.as_sdk(),
        None => super::markets::detect_family(&client, market).await?,
    };
    let label = family_label(family);

    if !confirm
        && std::io::IsTerminal::is_terminal(&std::io::stdin())
        && ctx.renderer.mode() != OutputMode::Json
    {
        eprintln!(
            "About to sell-position on {label} market {market:#x} (min_out={}).",
            args.min_out
        );
        super::confirm_or_bail("Continue?")?;
    }

    let account = build_owned_account(ctx)?;
    let writer_provider = build_provider(ctx)?;
    let market_hex = format!("{market:#x}");

    let result = match family {
        Family::Normal => {
            let runtime = resolve_runtime(args.runtime.as_deref(), family)?;
            let writer =
                NormalMarketWriter::new(NormalMarketReader::new(&writer_provider, market), account);
            match writer.sell_position(runtime, args.min_out).await {
                Ok(receipt) => submission_from_receipt("sell", market_hex.clone(), receipt),
                Err(e) => submission_from_trade_error("sell", market_hex.clone(), &e),
            }
        },
        Family::Lognormal => {
            let runtime = resolve_runtime(args.runtime.as_deref(), family)?;
            let writer = LognormalMarketWriter::new(
                LognormalMarketReader::new(&writer_provider, market),
                account,
            );
            match writer.sell_position(runtime, args.min_out).await {
                Ok(receipt) => submission_from_receipt("sell", market_hex.clone(), receipt),
                Err(e) => submission_from_trade_error("sell", market_hex.clone(), &e),
            }
        },
        Family::Multinoulli => {
            let writer = MultinoulliMarketWriter::new(
                MultinoulliMarketReader::new(&writer_provider, market),
                account,
            );
            match writer.sell_position(args.min_out).await {
                Ok(receipt) => submission_from_receipt("sell", market_hex.clone(), receipt),
                Err(e) => submission_from_trade_error("sell", market_hex.clone(), &e),
            }
        },
        Family::Bivariate => {
            let runtime = resolve_runtime(args.runtime.as_deref(), family)?;
            let writer = BivariateMarketWriter::new(
                BivariateMarketReader::new(&writer_provider, market),
                account,
            );
            match writer.sell_position(runtime, args.min_out).await {
                Ok(receipt) => submission_from_receipt("sell", market_hex.clone(), receipt),
                Err(e) => submission_from_trade_error("sell", market_hex.clone(), &e),
            }
        },
    };

    let _ = args.journal; // journal hook deferred to v1.1
    ctx.renderer.print(&result)
}

async fn list(
    ctx: &AppContext,
    trader: Option<String>,
    family: Option<FamilyArg>,
    limit: usize,
) -> Result<()> {
    let trader_str = match trader {
        Some(s) => s,
        None => ctx.config.address.clone().context(
            "no trader address — pass --trader, set DEADEYE_ADDRESS, or configure a profile",
        )?,
    };
    let indexer = ctx.indexer_client()?;
    let positions = indexer
        .positions(&trader_str)
        .await
        .with_context(|| format!("indexer GET /api/positions/{trader_str} failed"))?;

    let markets = indexer.markets().await.unwrap_or_default();
    let family_lookup: std::collections::HashMap<String, String> = markets
        .into_iter()
        .map(|m| (m.address, m.market_type))
        .collect();

    let mut rows: Vec<PositionRow> = positions
        .iter()
        .filter(|p| p.has_position && !p.claimed)
        .map(|p| {
            let mut row = PositionRow::from_indexer(p);
            if let Some(fam) = family_lookup.get(&p.market_address) {
                row.family = fam.clone();
            }
            row
        })
        .collect();

    if let Some(f) = family {
        let slug = f.as_indexer_slug();
        rows.retain(|r| r.family == slug);
    }
    rows.truncate(limit);
    if rows.is_empty() {
        ctx.renderer.warning("no open positions found");
    }
    ctx.renderer.print_table(&rows)
}

async fn show(
    ctx: &AppContext,
    market_str: &str,
    trader_opt: Option<String>,
    family_override: Option<FamilyArg>,
) -> Result<()> {
    let market = parse_address(market_str)?;
    let trader_str = match trader_opt {
        Some(s) => s,
        None => ctx.config.address.clone().context(
            "no trader address — pass --trader, set DEADEYE_ADDRESS, or configure a profile",
        )?,
    };
    let trader = parse_address(&trader_str)?;
    let client = ctx.deadeye_client()?;

    let family = if let Some(f) = family_override {
        f.as_sdk()
    } else {
        super::markets::detect_family(&client, market).await?
    };

    // Normal-family markets use the trade-lot (multi-leg) model: enumerate
    // the trader's legs + summary rather than the removed compact position.
    if matches!(family, Family::Normal) {
        let pos = client
            .normal_market(market)
            .legs(trader)
            .await
            .map_err(|e| anyhow::anyhow!("reading normal position legs: {e}"))?;
        let view = PositionLegsView {
            market: market_str.to_owned(),
            trader: format!("{trader:#x}"),
            family: "normal",
            exists: pos.exists,
            claimed: pos.claimed,
            tracks_settlement_claim: pos.tracks_settlement_claim,
            total_collateral: pos.total_collateral,
            leg_count: pos.legs.len(),
            active_legs: pos.active_legs(),
            legs: pos
                .legs
                .iter()
                .map(|l| LegRow {
                    lot_id: l.lot_id,
                    settled: l.settled,
                    cancelled: l.cancelled,
                })
                .collect(),
        };
        return ctx.renderer.print(&view);
    }

    let view = match family {
        Family::Normal => unreachable!("normal handled above"),
        Family::Lognormal => {
            let reader = LognormalMarketReader::new(client.provider(), market);
            let pos = reader
                .position(trader)
                .await
                .map_err(|e| anyhow::anyhow!("reading lognormal position: {e}"))?;
            PositionShowView {
                market_address: market_str.to_owned(),
                trader: format!("{trader:#x}"),
                family: "lognormal".to_owned(),
                total_collateral: Sq128::from_raw(pos.total_collateral).to_f64(),
                flags: pos.flags,
                extra: json!({
                    "original_mu": Sq128::from_raw(pos.original_mu).to_f64(),
                    "original_sigma": Sq128::from_raw(pos.original_sigma).to_f64(),
                    "effective_mu": Sq128::from_raw(pos.effective_mu).to_f64(),
                    "effective_sigma": Sq128::from_raw(pos.effective_sigma).to_f64(),
                }),
            }
        },
        Family::Multinoulli => {
            let reader = MultinoulliMarketReader::new(client.provider(), market);
            let pos = reader
                .position(trader)
                .await
                .map_err(|e| anyhow::anyhow!("reading multinoulli position: {e}"))?;
            PositionShowView {
                market_address: market_str.to_owned(),
                trader: format!("{trader:#x}"),
                family: "multinoulli".to_owned(),
                total_collateral: Sq128::from_raw(pos.total_collateral).to_f64(),
                flags: pos.flags,
                extra: serde_json::Value::Object(Default::default()),
            }
        },
        Family::Bivariate => {
            let reader = BivariateMarketReader::new(client.provider(), market);
            let pos = reader
                .position(trader)
                .await
                .map_err(|e| anyhow::anyhow!("reading bivariate position: {e}"))?;
            PositionShowView {
                market_address: market_str.to_owned(),
                trader: format!("{trader:#x}"),
                family: "bivariate".to_owned(),
                total_collateral: Sq128::from_raw(pos.total_collateral).to_f64(),
                flags: pos.flags,
                extra: serde_json::Value::Object(Default::default()),
            }
        },
    };
    ctx.renderer.print(&view)
}

/// `position value` — value a trader's multi-leg position at a settlement
/// outcome and/or compute its expected P&L under a forecast.
async fn value(
    ctx: &AppContext,
    market_str: &str,
    trader_opt: Option<String>,
    at: Option<f64>,
    belief: Option<f64>,
    belief_sigma: Option<f64>,
    family_override: Option<FamilyArg>,
) -> Result<()> {
    let market = parse_address(market_str)?;
    let trader = resolve_trader(ctx, trader_opt)?;
    let client = ctx.deadeye_client()?;
    let family = if let Some(f) = family_override {
        f.as_sdk()
    } else {
        super::markets::detect_family(&client, market).await?
    };
    if !matches!(family, Family::Normal) {
        anyhow::bail!(
            "`position value` currently supports normal-family markets; {family:?} not yet wired"
        );
    }
    let mkt = client.normal_market(market);

    let mut view = PositionValueView {
        market: market_str.to_owned(),
        trader: format!("{trader:#x}"),
        family: "normal",
        exists: false,
        total_collateral: 0.0,
        settlement: None,
        total_position_value: None,
        gross_return: None,
        legs: Vec::new(),
        belief: None,
        expected_pnl: None,
    };

    // With neither --at nor --belief, value at the current market mean.
    let settlement = match (at, belief) {
        (Some(x), _) => Some(x),
        (None, None) => Some(
            mkt.distribution()
                .await
                .map_err(|e| anyhow::anyhow!("reading market distribution: {e}"))?
                .mean()
                .to_f64(),
        ),
        (None, Some(_)) => None,
    };

    if let Some(x) = settlement {
        let v = mkt
            .position_value_at(trader, x)
            .await
            .map_err(|e| anyhow::anyhow!("valuing position at x*={x}: {e}"))?;
        view.exists = v.exists;
        view.total_collateral = v.total_collateral;
        view.settlement = Some(v.settlement);
        view.total_position_value = Some(v.total_position_value);
        view.gross_return = Some(v.gross_return);
        view.legs = v
            .legs
            .iter()
            .map(|l| LegValueRow {
                lot_id: l.lot_id,
                settled: l.settled,
                cancelled: l.cancelled,
                value_at: l.value_at,
            })
            .collect();
    }

    if let Some(bm) = belief {
        let bs = match belief_sigma {
            Some(s) => s,
            None => mkt
                .distribution()
                .await
                .map_err(|e| anyhow::anyhow!("reading market distribution: {e}"))?
                .sigma()
                .to_f64(),
        };
        let ev = mkt
            .expected_value_under_belief(trader, bm, bs)
            .await
            .map_err(|e| anyhow::anyhow!("computing expected value: {e}"))?;
        view.belief = Some(format!("μ={bm:.6}, σ={bs:.6}"));
        view.expected_pnl = Some(ev);
        if view.settlement.is_none() {
            let legs = mkt
                .legs(trader)
                .await
                .map_err(|e| anyhow::anyhow!("reading position legs: {e}"))?;
            view.exists = legs.exists;
            view.total_collateral = legs.total_collateral;
        }
    }

    ctx.renderer.print(&view)
}
