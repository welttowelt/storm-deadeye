//! `deadeye trade …` — preview-first trading flow (Driver B).
//!
//! `quote` is read-only; it preflights via `quote_trade` and prints a
//! verdict plus a copy-pasteable execute hint. `execute` re-runs the
//! quote (chain state may have moved), confirms, and submits via the
//! family writer. `journal` opens / replays the on-disk journal.

use anyhow::{Context as _, Result};
use deadeye_core::Sq128;
use deadeye_sdk::{
    DeadeyeClient,
    bulk::Family,
    journal::{EntryKind, JournalEntry, TradeJournal},
    starknet::JsonRpcProvider,
};
use deadeye_starknet::{
    Account, Felt, LognormalMarketReader, LognormalMarketWriter, NormalMarketReader,
    NormalMarketWriter, TradeRejectionReason,
};
use serde_json::json;

use crate::{
    cli::{TradeCmd, TradeExecuteArgs, TradeJournalArgs, TradeQuoteArgs},
    commands::{
        render_helpers::{
            QuoteResult, SubmissionResult, pretty_rejection, submission_from_receipt,
            submission_from_trade_error,
        },
        runtime_resolver::{
            build_owned_account, build_provider, family_label, parse_felt, resolve_family,
            resolve_runtime, resolve_runtime_opt,
        },
    },
    context::AppContext,
    output::OutputMode,
};

pub(crate) async fn run(action: TradeCmd, ctx: &AppContext, confirm: bool) -> Result<()> {
    match action {
        TradeCmd::Quote(args) => quote(ctx, args).await,
        TradeCmd::Execute(args) => execute(ctx, args, confirm).await,
        TradeCmd::Journal(args) => journal_cmd(ctx, args),
    }
}

// ─── quote ────────────────────────────────────────────────────────────

async fn quote(ctx: &AppContext, args: TradeQuoteArgs) -> Result<()> {
    let market = parse_felt("market address", &args.market)?;
    let provider = build_provider(ctx)?;
    let client = DeadeyeClient::new(provider);
    let family = resolve_family(&client, market, args.family).await?;

    let result = match family {
        Family::Normal => quote_normal(&client, market, family, &args).await?,
        Family::Lognormal => quote_lognormal(&client, market, family, &args).await?,
        Family::Multinoulli | Family::Bivariate => {
            anyhow::bail!(
                "trade quote: only normal + lognormal families are wired in Driver B's first cut; \
                 multinoulli / bivariate forthcoming"
            );
        },
    };
    ctx.renderer.print(&result)
}

async fn quote_normal(
    client: &DeadeyeClient<JsonRpcProvider>,
    market: Felt,
    family: Family,
    args: &TradeQuoteArgs,
) -> Result<QuoteResult> {
    use deadeye_core::Distribution as _;
    let market_handle = client.normal_market(market);
    // Offline by default: a runtime address is an *optional* chain-faithful
    // override, never required for a read-only quote (issue #4).
    let runtime = resolve_runtime_opt(args.runtime.as_deref(), family)?;

    // Read the live curve once — for surfacing and the belief_sigma default.
    let current = market_handle
        .distribution()
        .await
        .context("reading current market distribution")?;
    let market_mean = current.mean().to_f64();
    let market_sigma = current.sigma().to_f64();
    // Backing-derived σ-floor (issue: surface σ-min). Best-effort.
    let sigma_floor = market_handle.sigma_floor().await.ok();

    let (quote, belief, budget) =
        if let (Some(belief_mean), Some(budget)) = (args.belief, args.budget) {
            let belief_sigma = args.belief_sigma.unwrap_or(market_sigma);
            let q = if let Some(rt) = runtime {
                market_handle
                    .optimize_quote(rt, belief_mean, belief_sigma, budget)
                    .await
                    .context("optimize_quote (chain runtime)")?
            } else {
                market_handle
                    .optimize_quote_offline(belief_mean, belief_sigma, budget)
                    .await
                    .context("optimize_quote_offline")?
            };
            (q, Some((belief_mean, belief_sigma)), Some(budget))
        } else {
            let mean = args
                .mean
                .context("`--mean` is required (or pair --belief / --budget)")?;
            let variance = args
                .variance
                .context("`--variance` is required (or pair --belief / --budget)")?;
            let q = if let Some(rt) = runtime {
                // Optional chain-faithful path for a fixed candidate.
                let candidate = deadeye_core::distribution::NormalDistributionRaw {
                    mean: Sq128::from_f64(mean)?.to_raw(),
                    variance: Sq128::from_f64(variance)?.to_raw(),
                    sigma: Sq128::from_f64(variance.sqrt())?.to_raw(),
                };
                let cand_dist = deadeye_core::NormalDistribution::from_variance(
                    Sq128::from_f64(mean)?,
                    Sq128::from_f64(variance)?,
                )?;
                let x_star = match deadeye_sdk::collateral::normal_collateral(
                    &current,
                    &cand_dist,
                    deadeye_sdk::collateral::MinimizationPolicy::standard(),
                ) {
                    Ok(s) => Sq128::from_f64(s.x_min)?.to_raw(),
                    Err(_) => candidate.mean,
                };
                let supplied = Sq128::from_f64(args.pad.max(0.0))?.to_raw();
                market_handle
                    .reader()
                    .quote_trade(rt, candidate, x_star, supplied, supplied)
                    .await
                    .map_err(|e| anyhow::anyhow!("quote_trade: {e}"))?
            } else {
                // Default: fully client-side quote (no runtime, no tx, no gas).
                market_handle
                    .quote_candidate_offline(mean, variance)
                    .await
                    .context("quote_candidate_offline")?
            };
            (q, None, None)
        };

    let cand_mean = Sq128::from_raw(quote.candidate.mean).to_f64();
    let cand_sigma = Sq128::from_raw(quote.candidate.sigma).to_f64();
    let cand_variance = Sq128::from_raw(quote.candidate.variance).to_f64();
    let req_collat = Sq128::from_raw(quote.required_collateral).to_f64();

    // σ-floor gate at the CLI level too — covers the optimizer/belief path,
    // whose grid can otherwise propose a σ below the backing floor.
    let sub_floor = sigma_floor.is_some_and(|sf| cand_sigma + 1e-12 < sf);
    let accept = quote.on_chain_will_accept && !sub_floor;
    let rejection = if accept {
        None
    } else if sub_floor {
        Some(pretty_rejection(&TradeRejectionReason::SigmaTooLow))
    } else {
        quote.rejection.as_ref().map(pretty_rejection)
    };

    let execute_hint = format!(
        "deadeye trade execute {:#x} --family normal --mean {:.6} --variance {:.6} --max-collateral {:.6}",
        market,
        cand_mean,
        cand_variance,
        req_collat * 1.10
    );

    Ok(QuoteResult {
        family: family_label(family),
        market: format!("{market:#x}"),
        candidate_mean: Some(cand_mean),
        candidate_variance: Some(cand_variance),
        candidate_sigma: Some(cand_sigma),
        candidate_mu1: None,
        candidate_mu2: None,
        candidate_rho: None,
        x_star: Some(Sq128::from_raw(quote.x_star).to_f64()),
        required_collateral: Some(req_collat),
        padded_collateral: Some(Sq128::from_raw(quote.padded_collateral).to_f64()),
        sigma_floor,
        market_mean: Some(market_mean),
        market_sigma: Some(market_sigma),
        belief_mean: belief.map(|(m, _)| m),
        belief_sigma: belief.map(|(_, s)| s),
        expected_value: None,
        budget,
        on_chain_will_accept: accept,
        rejection,
        execute_hint,
    })
}

async fn quote_lognormal(
    client: &DeadeyeClient<JsonRpcProvider>,
    market: Felt,
    family: Family,
    args: &TradeQuoteArgs,
) -> Result<QuoteResult> {
    let runtime = resolve_runtime(args.runtime.as_deref(), family)?;
    let provider = client.provider();
    let reader = LognormalMarketReader::new(provider, market);
    let mean = args
        .mean
        .context("--mean is required for lognormal quote")?;
    let variance = args
        .variance
        .context("--variance is required for lognormal quote")?;
    let sigma = variance.sqrt();
    let candidate = deadeye_core::distribution::LognormalDistributionRaw {
        mu: Sq128::from_f64(mean)?.to_raw(),
        variance: Sq128::from_f64(variance)?.to_raw(),
        sigma: Sq128::from_f64(sigma)?.to_raw(),
    };
    let supplied = Sq128::from_f64(args.pad.max(0.0))?.to_raw();
    let quote = reader
        .quote_trade(runtime, candidate, candidate.mu, supplied, supplied)
        .await
        .map_err(|e| anyhow::anyhow!("quote_trade: {e}"))?;

    let cand_mu = Sq128::from_raw(quote.candidate.mu).to_f64();
    let cand_sigma = Sq128::from_raw(quote.candidate.sigma).to_f64();
    let req_collat = Sq128::from_raw(quote.required_collateral).to_f64();
    let execute_hint = format!(
        "deadeye trade execute {:#x} --family lognormal --mean {:.6} --variance {:.6} --max-collateral {:.6}",
        market,
        cand_mu,
        cand_sigma * cand_sigma,
        req_collat * 1.10
    );
    let rejection = if quote.on_chain_will_accept {
        None
    } else {
        quote.rejection.as_ref().map(pretty_rejection)
    };

    Ok(QuoteResult {
        family: family_label(family),
        market: format!("{market:#x}"),
        candidate_mean: Some(cand_mu),
        candidate_variance: Some(Sq128::from_raw(quote.candidate.variance).to_f64()),
        candidate_sigma: Some(cand_sigma),
        candidate_mu1: None,
        candidate_mu2: None,
        candidate_rho: None,
        x_star: Some(Sq128::from_raw(quote.x_star).to_f64()),
        required_collateral: Some(req_collat),
        padded_collateral: Some(Sq128::from_raw(quote.padded_collateral).to_f64()),
        sigma_floor: None,
        market_mean: None,
        market_sigma: None,
        belief_mean: None,
        belief_sigma: None,
        expected_value: None,
        budget: None,
        on_chain_will_accept: quote.on_chain_will_accept,
        rejection,
        execute_hint,
    })
}

// ─── execute ───────────────────────────────────────────────────────────

async fn execute(ctx: &AppContext, args: TradeExecuteArgs, confirm: bool) -> Result<()> {
    let market = parse_felt("market address", &args.market)?;
    let provider = build_provider(ctx)?;
    let client = DeadeyeClient::new(provider);
    let family = resolve_family(&client, market, args.family).await?;
    let label = family_label(family);

    match family {
        Family::Normal => execute_normal(ctx, &client, market, args, confirm, label).await,
        Family::Lognormal => execute_lognormal(ctx, &client, market, args, confirm, label).await,
        Family::Multinoulli | Family::Bivariate => {
            anyhow::bail!(
                "trade execute: only normal + lognormal are wired in Driver B's first cut"
            );
        },
    }
}

async fn execute_normal(
    ctx: &AppContext,
    client: &DeadeyeClient<JsonRpcProvider>,
    market: Felt,
    args: TradeExecuteArgs,
    confirm: bool,
    label: &'static str,
) -> Result<()> {
    // Offline preflight by default (no runtime / no gas); `--runtime` opts
    // into the chain-faithful path. The offline quote also enforces the
    // σ-floor, so a sub-σ-min candidate is rejected before submission.
    let runtime = resolve_runtime_opt(args.runtime.as_deref(), Family::Normal)?;
    let market_handle = client.normal_market(market);

    let mean = args.mean.context("--mean required for normal execute")?;
    let variance = args
        .variance
        .context("--variance required for normal execute")?;

    let quote = if let Some(rt) = runtime {
        let candidate = deadeye_core::distribution::NormalDistributionRaw {
            mean: Sq128::from_f64(mean)?.to_raw(),
            variance: Sq128::from_f64(variance)?.to_raw(),
            sigma: Sq128::from_f64(variance.sqrt())?.to_raw(),
        };
        let cand_dist = deadeye_core::NormalDistribution::from_variance(
            Sq128::from_f64(mean)?,
            Sq128::from_f64(variance)?,
        )?;
        let current = market_handle.distribution().await?;
        let solver = deadeye_sdk::collateral::normal_collateral(
            &current,
            &cand_dist,
            deadeye_sdk::collateral::MinimizationPolicy::standard(),
        )
        .map_err(|e| anyhow::anyhow!("off-chain collateral solver: {e}"))?;
        let x_star = Sq128::from_f64(solver.x_min)?.to_raw();
        let supplied = Sq128::from_f64(args.max_collateral)?.to_raw();
        market_handle
            .reader()
            .quote_trade(rt, candidate, x_star, supplied, supplied)
            .await
            .map_err(|e| anyhow::anyhow!("preflight quote_trade: {e}"))?
    } else {
        market_handle
            .quote_candidate_offline(mean, variance)
            .await
            .context("quote_candidate_offline preflight")?
    };

    if !quote.on_chain_will_accept {
        let rejection = quote.rejection.as_ref().map(pretty_rejection);
        let result = SubmissionResult {
            action: "trade",
            market: format!("{market:#x}"),
            tx_hash: None,
            call_count: None,
            accepted: false,
            rejection,
            note: Some("preflight rejected — fix the cause and re-quote before retrying".into()),
        };
        return ctx.renderer.print(&result);
    }

    if !confirm
        && std::io::IsTerminal::is_terminal(&std::io::stdin())
        && ctx.renderer.mode() != OutputMode::Json
    {
        eprintln!("About to submit {label}-market trade:");
        eprintln!("  market:    {market:#x}");
        eprintln!(
            "  candidate: μ={:.4}, σ²={:.4}",
            Sq128::from_raw(quote.candidate.mean).to_f64(),
            Sq128::from_raw(quote.candidate.variance).to_f64()
        );
        eprintln!(
            "  required collateral: ~{:.4} STRK",
            Sq128::from_raw(quote.required_collateral).to_f64()
        );
        eprintln!("  supplied:  {:.4} STRK", args.max_collateral);
        super::confirm_or_bail("Continue?")?;
    }

    let account = build_owned_account(ctx)?;
    let writer_provider = build_provider(ctx)?;
    let writer =
        NormalMarketWriter::new(NormalMarketReader::new(&writer_provider, market), account);

    let result = match writer.execute_quote(quote).await {
        Ok(receipt) => {
            if let Some(path) = &args.journal {
                let _ = append_normal_journal(path, market, &writer, &quote, receipt);
            }
            submission_from_receipt("trade", format!("{market:#x}"), receipt)
        },
        Err(e) => submission_from_trade_error("trade", format!("{market:#x}"), &e),
    };
    ctx.renderer.print(&result)
}

async fn execute_lognormal(
    ctx: &AppContext,
    client: &DeadeyeClient<JsonRpcProvider>,
    market: Felt,
    args: TradeExecuteArgs,
    confirm: bool,
    label: &'static str,
) -> Result<()> {
    let runtime = resolve_runtime(args.runtime.as_deref(), Family::Lognormal)?;
    let reader = LognormalMarketReader::new(client.provider(), market);

    let mean = args.mean.context("--mean required for lognormal execute")?;
    let variance = args
        .variance
        .context("--variance required for lognormal execute")?;
    let sigma = variance.sqrt();
    let candidate = deadeye_core::distribution::LognormalDistributionRaw {
        mu: Sq128::from_f64(mean)?.to_raw(),
        variance: Sq128::from_f64(variance)?.to_raw(),
        sigma: Sq128::from_f64(sigma)?.to_raw(),
    };
    let supplied = Sq128::from_f64(args.max_collateral)?.to_raw();
    let quote = reader
        .quote_trade(runtime, candidate, candidate.mu, supplied, supplied)
        .await
        .map_err(|e| anyhow::anyhow!("preflight quote_trade: {e}"))?;

    if !quote.on_chain_will_accept {
        let rejection = quote.rejection.as_ref().map(pretty_rejection);
        let result = SubmissionResult {
            action: "trade",
            market: format!("{market:#x}"),
            tx_hash: None,
            call_count: None,
            accepted: false,
            rejection,
            note: Some("preflight rejected — fix the cause and re-quote before retrying".into()),
        };
        return ctx.renderer.print(&result);
    }

    if !confirm
        && std::io::IsTerminal::is_terminal(&std::io::stdin())
        && ctx.renderer.mode() != OutputMode::Json
    {
        eprintln!("About to submit {label}-market trade for {market:#x}.");
        super::confirm_or_bail("Continue?")?;
    }

    let account = build_owned_account(ctx)?;
    let writer_provider = build_provider(ctx)?;
    let writer = LognormalMarketWriter::new(
        LognormalMarketReader::new(&writer_provider, market),
        account,
    );

    let result = match writer.execute_quote(quote).await {
        Ok(receipt) => submission_from_receipt("trade", format!("{market:#x}"), receipt),
        Err(e) => submission_from_trade_error("trade", format!("{market:#x}"), &e),
    };
    let _ = args.journal;
    ctx.renderer.print(&result)
}

// ─── journal ──────────────────────────────────────────────────────────

fn journal_cmd(ctx: &AppContext, args: TradeJournalArgs) -> Result<()> {
    let path = match args.path {
        Some(p) => p,
        None => default_journal_path()?,
    };
    if !path.exists() {
        ctx.renderer
            .warning(&format!("journal {} does not exist", path.display()));
        return Ok(());
    }
    let entries: Vec<JournalEntry> = TradeJournal::replay(&path)
        .with_context(|| format!("opening journal {}", path.display()))?
        .filter_map(Result::ok)
        .collect();
    let tail_start = entries.len().saturating_sub(args.tail);
    let slice = &entries[tail_start..];
    match ctx.renderer.mode() {
        OutputMode::Json => {
            let json = serde_json::to_string_pretty(slice)?;
            println!("{json}");
        },
        OutputMode::Pretty | OutputMode::Plain => {
            ctx.renderer.header(&format!(
                "Journal {} — {} entries",
                path.display(),
                slice.len()
            ));
            for entry in slice {
                println!(
                    "{:?} family={:?} market={:#x} tx={}",
                    entry.kind,
                    entry.family,
                    entry.market,
                    entry
                        .tx_hash
                        .map(|h| format!("{h:#x}"))
                        .unwrap_or_else(|| "(none)".into()),
                );
            }
        },
    }
    Ok(())
}

fn default_journal_path() -> Result<std::path::PathBuf> {
    let mut dir =
        dirs::data_dir().context("could not locate user data dir; pass --path explicitly")?;
    dir.push("deadeye");
    std::fs::create_dir_all(&dir).ok();
    dir.push("journal.jsonl");
    Ok(dir)
}

fn append_normal_journal<P, A>(
    path: &std::path::Path,
    market: Felt,
    writer: &NormalMarketWriter<P, A>,
    quote: &deadeye_starknet::NormalTradeQuote,
    receipt: deadeye_starknet::ExecutionReceipt,
) -> Result<()>
where
    P: deadeye_starknet::Provider,
    A: Account,
{
    let mut journal =
        TradeJournal::open(path).with_context(|| format!("opening journal {}", path.display()))?;
    let entry = JournalEntry::new(
        Family::Normal,
        market,
        Account::address(writer.account()),
        EntryKind::Trade,
        json!({
            "candidate_mean": Sq128::from_raw(quote.candidate.mean).to_f64(),
            "candidate_variance": Sq128::from_raw(quote.candidate.variance).to_f64(),
            "x_star": Sq128::from_raw(quote.x_star).to_f64(),
            "required_collateral": Sq128::from_raw(quote.required_collateral).to_f64(),
            "padded_collateral": Sq128::from_raw(quote.padded_collateral).to_f64(),
        }),
    )
    .with_tx_hash(receipt.transaction_hash)
    .with_receipt(json!({
        "transaction_hash": format!("{:#x}", receipt.transaction_hash),
        "call_count": receipt.call_count,
    }));
    journal
        .append(&entry)
        .with_context(|| format!("appending to journal {}", path.display()))
}
