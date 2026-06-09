//! `deadeye position …` — list / show read paths.
//!
//! The `Sell` variant is owned by Driver B; here it returns a friendly
//! "not implemented in this driver" message so the binary still builds
//! and `--help` still documents the command.

use anyhow::{Context as _, Result};
use deadeye_core::Distribution as _;
use deadeye_sdk::bulk::Family;
use deadeye_starknet::{
    BivariateMarketReader, LognormalMarketReader, MultinoulliMarketReader, NormalMarketReader,
};

use crate::{
    cli::{FamilyArg, PositionCmd, PositionSellArgs, PositionValueArgs},
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
    render::PositionRow,
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
        PositionCmd::Value(args) => value(ctx, args).await,
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

    // Every family now uses the trade-lot (multi-leg) model: enumerate the
    // trader's legs + summary (the compact `get_position_compact` is gone).
    let pos = match family {
        Family::Normal => client.normal_market(market).legs(trader).await,
        Family::Lognormal => client.lognormal_market(market).legs(trader).await,
        Family::Bivariate => client.bivariate_market(market).legs(trader).await,
        Family::Multinoulli => client.multinoulli_market(market).legs(trader).await,
    }
    .with_context(|| format!("reading {} position legs", family_label(family)))?;

    ctx.renderer
        .print(&legs_view(market_str, trader, family_label(family), &pos))
}

/// Build the [`PositionLegsView`] for `position show` from an SDK
/// [`deadeye_sdk::PositionLegs`] (family-agnostic).
fn legs_view(
    market_str: &str,
    trader: deadeye_starknet::Felt,
    family: &'static str,
    pos: &deadeye_sdk::PositionLegs,
) -> PositionLegsView {
    PositionLegsView {
        market: market_str.to_owned(),
        trader: format!("{trader:#x}"),
        family,
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
    }
}

/// Copy an SDK [`deadeye_sdk::PositionValuation`] (settlement path) onto the
/// view.
fn apply_valuation(view: &mut PositionValueView, v: &deadeye_sdk::PositionValuation) {
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

/// `position value` — value a trader's multi-leg position at a settlement
/// outcome and/or compute its expected P&L under a forecast. Dispatches per
/// family (scalar / 2D point / categorical settlement).
async fn value(ctx: &AppContext, args: PositionValueArgs) -> Result<()> {
    let market = parse_address(&args.market)?;
    let trader = resolve_trader(ctx, args.trader.clone())?;
    let client = ctx.deadeye_client()?;
    let family = match args.family {
        Some(f) => f.as_sdk(),
        None => super::markets::detect_family(&client, market).await?,
    };

    let mut view = PositionValueView {
        market: args.market.clone(),
        trader: format!("{trader:#x}"),
        family: family_label(family),
        exists: false,
        total_collateral: 0.0,
        settlement: None,
        total_position_value: None,
        gross_return: None,
        legs: Vec::new(),
        belief: None,
        expected_pnl: None,
    };

    match family {
        Family::Normal => {
            let mkt = client.normal_market(market);
            // Default settlement = current market mean (unless a belief is asked).
            let at = match (args.at, args.belief) {
                (Some(x), _) => Some(x),
                (None, None) => Some(
                    mkt.distribution()
                        .await
                        .context("reading distribution")?
                        .mean()
                        .to_f64(),
                ),
                (None, Some(_)) => None,
            };
            if let Some(x) = at {
                let v = mkt
                    .position_value_at(trader, x)
                    .await
                    .context("valuing position")?;
                apply_valuation(&mut view, &v);
            }
            if let Some(bm) = args.belief {
                let bs = match args.belief_sigma {
                    Some(s) => s,
                    None => mkt
                        .distribution()
                        .await
                        .context("reading distribution")?
                        .sigma()
                        .to_f64(),
                };
                let ev = mkt
                    .expected_value_under_belief(trader, bm, bs)
                    .await
                    .context("computing expected value")?;
                view.belief = Some(format!("μ={bm:.6}, σ={bs:.6}"));
                view.expected_pnl = Some(ev);
                if view.settlement.is_none() {
                    let legs = mkt.legs(trader).await.context("reading legs")?;
                    view.exists = legs.exists;
                    view.total_collateral = legs.total_collateral;
                }
            }
        },
        Family::Lognormal => {
            let mkt = client.lognormal_market(market);
            let at = match (args.at, args.belief) {
                (Some(x), _) => Some(x),
                (None, None) => Some(
                    mkt.distribution()
                        .await
                        .context("reading distribution")?
                        .mean()
                        .to_f64(),
                ),
                (None, Some(_)) => None,
            };
            if let Some(x) = at {
                let v = mkt
                    .position_value_at(trader, x)
                    .await
                    .context("valuing position")?;
                apply_valuation(&mut view, &v);
            }
            if let Some(bm) = args.belief {
                let bs = match args.belief_sigma {
                    Some(s) => s,
                    None => mkt
                        .distribution()
                        .await
                        .context("reading distribution")?
                        .sigma()
                        .to_f64(),
                };
                let ev = mkt
                    .expected_value_under_belief(trader, bm, bs)
                    .await
                    .context("computing expected value")?;
                view.belief = Some(format!("μ_log={bm:.6}, σ_log={bs:.6}"));
                view.expected_pnl = Some(ev);
                if view.settlement.is_none() {
                    let legs = mkt.legs(trader).await.context("reading legs")?;
                    view.exists = legs.exists;
                    view.total_collateral = legs.total_collateral;
                }
            }
        },
        Family::Bivariate => {
            let mkt = client.bivariate_market(market);
            if let (Some(x1), Some(x2)) = (args.at_x1, args.at_x2) {
                let v = mkt
                    .position_value_at(trader, x1, x2)
                    .await
                    .context("valuing position")?;
                apply_valuation(&mut view, &v);
            }
            if let (Some(m1), Some(m2), Some(s1), Some(s2)) = (
                args.belief_mu1,
                args.belief_mu2,
                args.belief_sigma1,
                args.belief_sigma2,
            ) {
                let ev = mkt
                    .expected_value_under_belief(trader, m1, m2, s1, s2, args.belief_rho)
                    .await
                    .context("computing expected value")?;
                view.belief = Some(format!(
                    "μ₁={m1:.4}, μ₂={m2:.4}, σ₁={s1:.4}, σ₂={s2:.4}, ρ={:.3}",
                    args.belief_rho
                ));
                view.expected_pnl = Some(ev);
                if view.settlement.is_none() {
                    let legs = mkt.legs(trader).await.context("reading legs")?;
                    view.exists = legs.exists;
                    view.total_collateral = legs.total_collateral;
                }
            }
            if view.settlement.is_none() && view.expected_pnl.is_none() {
                anyhow::bail!(
                    "bivariate: pass a settlement `--at-x1 <X1> --at-x2 <X2>` or a belief \
                     `--belief-mu1 --belief-mu2 --belief-sigma1 --belief-sigma2 [--belief-rho]`"
                );
            }
        },
        Family::Multinoulli => {
            let mkt = client.multinoulli_market(market);
            if let Some(outcome) = args.outcome {
                let v = mkt
                    .position_value_at(trader, outcome)
                    .await
                    .context("valuing position")?;
                apply_valuation(&mut view, &v);
            }
            if !args.belief_probs.is_empty() {
                let ev = mkt
                    .expected_value_under_belief(trader, &args.belief_probs)
                    .await
                    .context("computing expected value")?;
                let probs = args
                    .belief_probs
                    .iter()
                    .map(|p| format!("{p:.3}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                view.belief = Some(format!("probs=[{probs}]"));
                view.expected_pnl = Some(ev);
                if view.settlement.is_none() {
                    let legs = mkt.legs(trader).await.context("reading legs")?;
                    view.exists = legs.exists;
                    view.total_collateral = legs.total_collateral;
                }
            }
            if view.settlement.is_none() && view.expected_pnl.is_none() {
                anyhow::bail!(
                    "multinoulli: pass a settlement `--outcome <i>` or a belief \
                     `--belief-probs p0,p1,…`"
                );
            }
        },
    }

    ctx.renderer.print(&view)
}
