//! `deadeye admin …` — factory-owner write paths (settle / pause /
//! unpause / collect-fees).

use anyhow::{Context as _, Result};
use deadeye_core::{Sq128, bivariate::BivariatePointRaw};
use deadeye_sdk::bulk::Family;
use deadeye_starknet::{FactoryReader, FactoryWriter};

use crate::{
    cli::{
        AdminCmd, AdminCollectFeesArgs, AdminPauseArgs, AdminSettleArgs, FamilyArg,
    },
    commands::{
        render_helpers::{submission_from_receipt, submission_from_trade_error},
        runtime_resolver::{build_owned_account, build_provider, parse_felt},
    },
    context::AppContext,
    output::OutputMode,
};

pub(crate) async fn run(action: AdminCmd, ctx: &AppContext, confirm: bool) -> Result<()> {
    match action {
        AdminCmd::Settle(args) => settle(ctx, args, confirm).await,
        AdminCmd::Pause(args) => pause(ctx, args, confirm, /* unpause */ false).await,
        AdminCmd::Unpause(args) => pause(ctx, args, confirm, /* unpause */ true).await,
        AdminCmd::CollectFees(args) => collect_fees(ctx, args, confirm).await,
    }
}

fn resolve_factory(arg: &Option<String>) -> Result<deadeye_starknet::Felt> {
    if let Some(s) = arg {
        return parse_felt("factory address", s);
    }
    let env = std::env::var("DEADEYE_FACTORY_ADDR")
        .context("factory address required: pass --factory 0x... or set DEADEYE_FACTORY_ADDR")?;
    parse_felt("factory address", &env)
}

async fn settle(ctx: &AppContext, args: AdminSettleArgs, confirm: bool) -> Result<()> {
    let factory = resolve_factory(&args.factory)?;
    let market = parse_felt("market address", &args.market)?;
    let provider = build_provider(ctx)?;
    let account = build_owned_account(ctx)?;
    let writer = FactoryWriter::new(FactoryReader::new(provider, factory), account);

    if !confirm
        && std::io::IsTerminal::is_terminal(&std::io::stdin())
        && ctx.renderer.mode() != OutputMode::Json
    {
        eprintln!(
            "About to settle {:?} market {market:#x} via factory {factory:#x}.",
            args.family
        );
        super::confirm_or_bail("Continue?")?;
    }

    let market_hex = format!("{market:#x}");
    let family = args.family;
    let result = match family {
        FamilyArg::Normal => {
            let x = args
                .x_star
                .context("`--x-star <f64>` is required for normal settle")?;
            let x_star = Sq128::from_f64(x)?.to_raw();
            match writer.settle_normal_market(market, x_star).await {
                Ok(receipt) => submission_from_receipt("settle", market_hex, receipt),
                Err(e) => submission_from_trade_error("settle", market_hex, &e),
            }
        },
        FamilyArg::Lognormal => {
            let x = args
                .x_star
                .context("`--x-star <f64>` is required for lognormal settle")?;
            let x_star = Sq128::from_f64(x)?.to_raw();
            match writer.settle_lognormal_market(market, x_star).await {
                Ok(receipt) => submission_from_receipt("settle", market_hex, receipt),
                Err(e) => submission_from_trade_error("settle", market_hex, &e),
            }
        },
        FamilyArg::Multinoulli => {
            let outcome = args
                .outcome
                .context("`--outcome <u32>` is required for multinoulli settle")?;
            match writer.settle_multinoulli_market(market, outcome).await {
                Ok(receipt) => submission_from_receipt("settle", market_hex, receipt),
                Err(e) => submission_from_trade_error("settle", market_hex, &e),
            }
        },
        FamilyArg::Bivariate => {
            let raw = args
                .point
                .context("`--point X1,X2` is required for bivariate settle")?;
            let (x1_str, x2_str) = raw
                .split_once(',')
                .context("`--point` must be of the form `X1,X2`")?;
            let x1: f64 = x1_str.trim().parse().context("parsing point.x1")?;
            let x2: f64 = x2_str.trim().parse().context("parsing point.x2")?;
            let point = BivariatePointRaw {
                x1: Sq128::from_f64(x1)?.to_raw(),
                x2: Sq128::from_f64(x2)?.to_raw(),
            };
            match writer.settle_bivariate_market(market, point).await {
                Ok(receipt) => submission_from_receipt("settle", market_hex, receipt),
                Err(e) => submission_from_trade_error("settle", market_hex, &e),
            }
        },
    };
    ctx.renderer.print(&result)
}

async fn pause(
    ctx: &AppContext,
    args: AdminPauseArgs,
    confirm: bool,
    unpause: bool,
) -> Result<()> {
    let factory = resolve_factory(&args.factory)?;
    let market = parse_felt("market address", &args.market)?;
    let provider = build_provider(ctx)?;
    let account = build_owned_account(ctx)?;
    let writer = FactoryWriter::new(FactoryReader::new(provider, factory), account);

    let action_name = if unpause { "unpause" } else { "pause" };
    if !confirm
        && std::io::IsTerminal::is_terminal(&std::io::stdin())
        && ctx.renderer.mode() != OutputMode::Json
    {
        eprintln!("About to {action_name} market {market:#x} via factory {factory:#x}.");
        super::confirm_or_bail("Continue?")?;
    }

    let market_hex = format!("{market:#x}");
    let outcome = if unpause {
        writer.unpause_market_typed(market).await
    } else {
        writer.pause_market_typed(market).await
    };
    let result = match outcome {
        Ok(receipt) => submission_from_receipt(
            if unpause { "unpause" } else { "pause" },
            market_hex,
            receipt,
        ),
        Err(e) => submission_from_trade_error(
            if unpause { "unpause" } else { "pause" },
            market_hex,
            &e,
        ),
    };
    ctx.renderer.print(&result)
}

async fn collect_fees(
    ctx: &AppContext,
    args: AdminCollectFeesArgs,
    confirm: bool,
) -> Result<()> {
    let factory = resolve_factory(&args.factory)?;
    let market = parse_felt("market address", &args.market)?;
    let recipient = parse_felt("recipient address", &args.recipient)?;
    let provider = build_provider(ctx)?;
    let account = build_owned_account(ctx)?;
    let writer = FactoryWriter::new(FactoryReader::new(provider, factory), account);

    if !confirm
        && std::io::IsTerminal::is_terminal(&std::io::stdin())
        && ctx.renderer.mode() != OutputMode::Json
    {
        eprintln!(
            "About to collect protocol fees on market {market:#x} → recipient {recipient:#x}."
        );
        super::confirm_or_bail("Continue?")?;
    }

    let market_hex = format!("{market:#x}");
    let result = match writer.collect_protocol_fees(market, recipient).await {
        Ok(receipt) => submission_from_receipt("collect_fees", market_hex, receipt),
        Err(e) => submission_from_trade_error("collect_fees", market_hex, &e),
    };
    let _ = Family::Normal;
    ctx.renderer.print(&result)
}
