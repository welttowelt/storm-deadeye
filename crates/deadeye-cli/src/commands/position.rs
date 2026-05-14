//! `deadeye position …` — list / show read paths.
//!
//! The `Sell` variant is owned by Driver B; here it returns a friendly
//! "not implemented in this driver" message so the binary still builds
//! and `--help` still documents the command.

use anyhow::{Context as _, Result};
use deadeye_core::Sq128;
use deadeye_sdk::bulk::Family;
use deadeye_starknet::{
    BivariateMarketReader, LognormalMarketReader, MultinoulliMarketReader, NormalMarketReader,
};
use serde_json::json;

use crate::{
    cli::{FamilyArg, PositionCmd, PositionSellArgs},
    commands::{
        render_helpers::{submission_from_receipt, submission_from_trade_error},
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
        PositionCmd::Sell(args) => sell(ctx, args, confirm).await,
    }
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

    let view = match family {
        Family::Normal => {
            let reader = NormalMarketReader::new(client.provider(), market);
            let pos = reader
                .position(trader)
                .await
                .map_err(|e| anyhow::anyhow!("reading normal position: {e}"))?;
            PositionShowView {
                market_address: market_str.to_owned(),
                trader: format!("{trader:#x}"),
                family: "normal".to_owned(),
                total_collateral: Sq128::from_raw(pos.total_collateral).to_f64(),
                flags: pos.flags,
                extra: json!({
                    "original_mean": Sq128::from_raw(pos.original_mean).to_f64(),
                    "original_sigma": Sq128::from_raw(pos.original_sigma).to_f64(),
                    "original_variance": Sq128::from_raw(pos.original_variance).to_f64(),
                    "original_lambda": Sq128::from_raw(pos.original_lambda).to_f64(),
                    "effective_mean": Sq128::from_raw(pos.effective_mean).to_f64(),
                    "effective_sigma": Sq128::from_raw(pos.effective_sigma).to_f64(),
                    "effective_variance": Sq128::from_raw(pos.effective_variance).to_f64(),
                    "effective_lambda": Sq128::from_raw(pos.effective_lambda).to_f64(),
                }),
            }
        },
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
