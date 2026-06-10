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
    Account, Call, Felt, LognormalMarketReader, LognormalMarketWriter, NormalMarketReader,
    NormalMarketWriter, TradeRejectionReason,
};
use serde_json::json;

use crate::{
    cli::{TradeCmd, TradeExecuteArgs, TradeJournalArgs, TradeQuoteArgs},
    context::CliProvider,
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

/// Multiplier applied to the offline-computed required collateral when sizing
/// the amount the trade actually supplies. Collateral is a *returned* margin
/// lock (not a cost), so over-supplying is free; a margin is required because
/// the on-chain Q128.128 `collateral_sufficient` check rejects a supply that
/// equals the f64 estimate on any rounding gap. 5% comfortably covers the
/// fixed-point delta while staying close to the webapp's buffered collateral.
const COLLATERAL_BUFFER: f64 = 1.05;

pub(crate) async fn run(action: TradeCmd, ctx: &AppContext, confirm: bool) -> Result<()> {
    match action {
        TradeCmd::Quote(args) => quote(ctx, args).await,
        TradeCmd::Execute(args) => execute(ctx, args, confirm).await,
        TradeCmd::Journal(args) => journal_cmd(ctx, args),
    }
}

// ─── quote ────────────────────────────────────────────────────────────

pub(crate) async fn quote(ctx: &AppContext, args: TradeQuoteArgs) -> Result<()> {
    // Fetch-once path (issue #14): a saved snapshot makes the quote PURE —
    // zero RPC, so exploring N candidates costs one read total.
    if let Some(path) = &args.from_state {
        let result = quote_normal_from_state(path, &args)?;
        return ctx.renderer.print(&result);
    }
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

/// Risk/sizing/lint block shared by the live and `--from-state` quote paths
/// (issues #15 + #24). Pure f64 display math — never touches the verified
/// collateral path.
struct RiskExtras {
    downside_at_market_mean: Option<f64>,
    cvar_5pct: Option<f64>,
    stress_ev: Option<f64>,
    sizing: Option<super::risk::SizingAdvice>,
    warnings: Vec<String>,
}

#[expect(clippy::too_many_arguments, reason = "plain display-math inputs")]
fn compute_risk_extras(
    args: &TradeQuoteArgs,
    market_mean: f64,
    market_sigma: f64,
    effective_k: f64,
    cand_mean: f64,
    cand_sigma: f64,
    expected_value: Option<f64>,
    required_collateral: f64,
    sigma_floor: Option<f64>,
    belief: Option<(f64, f64)>,
    budget: Option<f64>,
) -> RiskExtras {
    use super::risk;
    let downside = Some(risk::pnl_at(
        market_mean, market_sigma, cand_mean, cand_sigma, effective_k, market_mean,
    ));
    let (cvar, stress) = belief.map_or((None, None), |(bm, bs)| {
        let cvar = risk::cvar_under_belief(
            market_mean, market_sigma, cand_mean, cand_sigma, effective_k, bm, bs, 0.05,
        );
        let stress = risk::expected_pnl(
            market_mean, market_sigma, cand_mean, cand_sigma, effective_k, bm, bs * 1.5,
        );
        (cvar.is_finite().then_some(cvar), Some(stress))
    });
    let kelly_multiplier = args.kelly.or_else(|| {
        args.risk.as_deref().and_then(risk::preset_fraction)
    });
    if let Some(preset) = args.risk.as_deref()
        && risk::preset_fraction(preset).is_none()
    {
        tracing::warn!(target: "deadeye::risk", preset, "unknown --risk preset; expected conservative|balanced|aggressive");
    }
    let ev_for_sizing = expected_value.or_else(|| {
        belief.map(|(bm, bs)| {
            risk::expected_pnl(market_mean, market_sigma, cand_mean, cand_sigma, effective_k, bm, bs)
        })
    });
    let sizing = match (args.bankroll, kelly_multiplier, ev_for_sizing) {
        (Some(bankroll), mult, Some(ev)) => {
            risk::sizing_advice(ev, required_collateral, bankroll, mult.unwrap_or(0.5))
        },
        _ => None,
    };
    let warnings = risk::lint_quote(
        belief,
        market_mean,
        market_sigma,
        cand_mean,
        cand_sigma,
        sigma_floor,
        budget,
        sizing.as_ref(),
    );
    RiskExtras {
        downside_at_market_mean: downside,
        cvar_5pct: cvar,
        stress_ev: stress,
        sizing,
        warnings,
    }
}

/// Pure quote from a saved snapshot (issue #14): zero RPC. Mirrors the
/// offline branches of `quote_normal`, sourcing state from the JSON that
/// `deadeye markets snapshot` produced instead of three live view calls.
fn quote_normal_from_state(
    path: &std::path::Path,
    args: &TradeQuoteArgs,
) -> Result<QuoteResult> {
    use deadeye_sdk::normal::{
        NormalMarketStateSnapshot, optimize_quote_from_state, quote_candidate_from_state,
    };
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading state snapshot {}", path.display()))?;
    let snapshot: NormalMarketStateSnapshot = serde_json::from_str(&raw)
        .context("parsing state snapshot (expected `deadeye markets snapshot` JSON)")?;
    let market_mean = snapshot.mean;
    let market_sigma = snapshot.sigma;

    let (quote, belief, budget, expected_value) =
        if let (Some(belief_mean), Some(budget)) = (args.belief, args.budget) {
            let belief_sigma = args.belief_sigma.unwrap_or(market_sigma);
            let (q, ev) = optimize_quote_from_state(&snapshot, belief_mean, belief_sigma, budget)
                .map_err(|e| anyhow::anyhow!("optimize_quote_from_state: {e}"))?;
            (q, Some((belief_mean, belief_sigma)), Some(budget), Some(ev))
        } else {
            let mean = args
                .mean
                .context("`--mean` is required (or pair --belief / --budget)")?;
            let variance = args
                .variance
                .context("`--variance` is required (or pair --belief / --budget)")?;
            let q = quote_candidate_from_state(&snapshot, mean, variance)
                .map_err(|e| anyhow::anyhow!("quote_candidate_from_state: {e}"))?;
            (q, None, None, None)
        };

    let market_hex = snapshot.market.clone();
    let cand_mean = Sq128::from_raw(quote.candidate.mean).to_f64();
    let cand_sigma = Sq128::from_raw(quote.candidate.sigma).to_f64();
    let cand_variance = Sq128::from_raw(quote.candidate.variance).to_f64();
    let req_collat = Sq128::from_raw(quote.required_collateral).to_f64();
    let extras = compute_risk_extras(
        args,
        market_mean,
        market_sigma,
        snapshot.effective_k,
        cand_mean,
        cand_sigma,
        expected_value,
        req_collat,
        None,
        belief,
        budget,
    );
    let execute_hint = format!(
        "deadeye trade execute {} --family normal --mean {:.6} --variance {:.6} --max-collateral {:.6}",
        market_hex,
        cand_mean,
        cand_variance,
        req_collat * 1.10
    );
    Ok(QuoteResult {
        family: "normal",
        market: market_hex,
        candidate_mean: Some(cand_mean),
        candidate_variance: Some(cand_variance),
        candidate_sigma: Some(cand_sigma),
        candidate_mu1: None,
        candidate_mu2: None,
        candidate_rho: None,
        x_star: Some(Sq128::from_raw(quote.x_star).to_f64()),
        required_collateral: Some(req_collat),
        padded_collateral: Some(Sq128::from_raw(quote.padded_collateral).to_f64()),
        // The snapshot has no live backing read; floor gating is offline-only
        // here and the execute path still chain-verifies.
        sigma_floor: None,
        market_mean: Some(market_mean),
        market_sigma: Some(market_sigma),
        belief_mean: belief.map(|(m, _)| m),
        belief_sigma: belief.map(|(_, s)| s),
        expected_value,
        budget,
        on_chain_will_accept: quote.on_chain_will_accept,
        rejection: quote.rejection.as_ref().map(pretty_rejection),
        downside_at_market_mean: extras.downside_at_market_mean,
        cvar_5pct: extras.cvar_5pct,
        stress_ev: extras.stress_ev,
        sizing: extras.sizing,
        warnings: extras.warnings,
        execute_hint,
    })
}

async fn quote_normal(
    client: &DeadeyeClient<CliProvider>,
    market: Felt,
    family: Family,
    args: &TradeQuoteArgs,
) -> Result<QuoteResult> {
    use deadeye_core::Distribution as _;
    let market_handle = client.normal_market(market);
    // Offline by default: a runtime address is an *optional* chain-faithful
    // override, never required for a read-only quote (issue #4).
    let runtime = resolve_runtime_opt(args.runtime.as_deref(), family)?;

    // ONE state fetch (issue #14): distribution + params + lp_info in a
    // single snapshot; σ-floor and effective-k derive locally from it.
    let snapshot = market_handle
        .state_snapshot()
        .await
        .context("reading market state snapshot")?;
    let current = snapshot
        .distribution()
        .context("reconstructing market distribution")?;
    let market_mean = snapshot.mean;
    let market_sigma = snapshot.sigma;
    let effective_k = snapshot.effective_k;
    let sigma_floor = Some(deadeye_sdk::normal::normal_sigma_floor(
        effective_k,
        snapshot.pool_backing_xp,
    ));

    let (quote, belief, budget, expected_value) =
        if let (Some(belief_mean), Some(budget)) = (args.belief, args.budget) {
            let belief_sigma = args.belief_sigma.unwrap_or(market_sigma);
            let (q, ev) = if let Some(rt) = runtime {
                // Chain-runtime path doesn't surface the optimizer EV.
                let q = market_handle
                    .optimize_quote(rt, belief_mean, belief_sigma, budget)
                    .await
                    .context("optimize_quote (chain runtime)")?;
                (q, None)
            } else {
                // Offline path returns the optimizer's expected value (XP).
                // Reuses the snapshot — no params/lp re-read (issue #14).
                let (q, ev) = deadeye_sdk::normal::optimize_quote_from_state(
                    &snapshot, belief_mean, belief_sigma, budget,
                )
                    .context("optimize_quote_offline")?;
                (q, Some(ev))
            };
            (q, Some((belief_mean, belief_sigma)), Some(budget), ev)
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
                deadeye_sdk::normal::quote_candidate_from_state(&snapshot, mean, variance)
                    .context("quote_candidate_from_state")?
            };
            // Fixed-candidate quote has no belief → no expected value.
            (q, None, None, None)
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

    let extras = compute_risk_extras(
        args,
        market_mean,
        market_sigma,
        effective_k,
        cand_mean,
        cand_sigma,
        expected_value,
        req_collat,
        sigma_floor,
        belief,
        budget,
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
        expected_value,
        budget,
        on_chain_will_accept: accept,
        rejection,
        downside_at_market_mean: extras.downside_at_market_mean,
        cvar_5pct: extras.cvar_5pct,
        stress_ev: extras.stress_ev,
        sizing: extras.sizing,
        warnings: extras.warnings,
        execute_hint,
    })
}

async fn quote_lognormal(
    client: &DeadeyeClient<CliProvider>,
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

    // Risk extras are normal-family math for now (issue #15) — lognormal
    // quotes render without them.
    Ok(QuoteResult {
        downside_at_market_mean: None,
        cvar_5pct: None,
        stress_ev: None,
        sizing: None,
        warnings: Vec::new(),
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

pub(crate) async fn execute(ctx: &AppContext, args: TradeExecuteArgs, confirm: bool) -> Result<()> {
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
    client: &DeadeyeClient<CliProvider>,
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

    let mut quote = if let Some(rt) = runtime {
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

    // Size the *supplied* collateral the trade locks. The offline quote's
    // `padded_collateral` defaults to the bare f64-computed required amount
    // with **no margin** — which the on-chain Q128.128 `collateral_sufficient`
    // check rejects (`VERIFICATION_FAILED`) on the slightest rounding gap.
    // Supply a buffered amount instead (collateral is a *returned* margin lock,
    // not a cost), capped by the trader's `--max-collateral` ceiling. This
    // mirrors `trade quote`'s `execute_hint` and the webapp's buffered trade
    // collateral. Skipped for the `--runtime` path, which already supplies
    // `--max-collateral` and was validated by `check_trade_view`.
    if runtime.is_none() && quote.on_chain_will_accept {
        let required = Sq128::from_raw(quote.required_collateral).to_f64();
        let target = required * COLLATERAL_BUFFER;
        let supplied = if args.max_collateral >= target {
            target
        } else if args.max_collateral >= required {
            args.max_collateral
        } else {
            anyhow::bail!(
                "--max-collateral {:.4} is below the required collateral {:.4}; \
                 raise it to at least ~{:.4} (required × {COLLATERAL_BUFFER}) so the \
                 on-chain collateral check clears",
                args.max_collateral,
                required,
                target,
            );
        };
        quote.padded_collateral = Sq128::from_f64(supplied)?.to_raw();
    }

    // Diagnostic override of x* (collateral point) to probe the on-chain
    // verifier's stationary check.
    if let Some(xs) = args.x_star {
        quote.x_star = Sq128::from_f64(xs)?.to_raw();
    }

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

    let account = build_owned_account(ctx)?;
    let writer_provider = build_provider(ctx)?;
    let writer =
        NormalMarketWriter::new(NormalMarketReader::new(&writer_provider, market), account);

    // Chain-probe `x*` refinement (issue #13 root cause). The AMM verifies
    // stationarity of the λ-scaled PDF difference in its own fixed-point
    // arithmetic, whose acceptance window (≈1e-7 wide in x) sits slightly off
    // the f64 root the off-chain solver finds — so a mathematically-perfect
    // x* still reverts with VERIFICATION_FAILED. Probe `check_trade_view`
    // (gas-free, simulated against the market's own runtime class) around the
    // f64 root and adopt the x* + collateral the chain itself certifies.
    if args.x_star.is_none() {
        match deadeye_starknet::chain_probe::refine_normal_quote(
            writer.account(),
            writer.reader(),
            &quote,
        )
        .await
        {
            Ok(Some(outcome)) => {
                let chain_required = Sq128::from_raw(outcome.computed_collateral).to_f64();
                // `execute_trade` deducts deposit fees from the supplied
                // amount and verifies the NET against the requirement —
                // gross up by the measured net rate, plus a thin margin.
                let gross_needed = chain_required / outcome.net_rate;
                let buffered = gross_needed * 1.002;
                if buffered > args.max_collateral {
                    anyhow::bail!(
                        "chain-verified collateral is {chain_required:.4} XP net \
                         (≈{buffered:.4} XP gross incl. deposit fees), which exceeds \
                         --max-collateral {:.4}; raise the ceiling",
                        args.max_collateral,
                    );
                }
                quote.x_star = outcome.x_star;
                quote.required_collateral = outcome.computed_collateral;
                quote.padded_collateral = Sq128::from_f64(buffered)?.to_raw();
                if ctx.renderer.mode() != OutputMode::Json {
                    eprintln!(
                        "chain probe: certified x* (offset {:+.3e}, {} round(s)); \
                         collateral {chain_required:.4} XP net → supplying {buffered:.4} \
                         XP gross (fees {:.2}%)",
                        outcome.offset,
                        outcome.rounds,
                        (1.0 - outcome.net_rate) * 100.0,
                    );
                }
            },
            Ok(None) => {
                ctx.renderer.warning(
                    "chain probe could not certify an x* near the off-chain solution; \
                     submitting unrefined (the pre-submit simulation still blocks a \
                     reverting trade before any gas is spent)",
                );
            },
            Err(e) => {
                ctx.renderer.warning(&format!(
                    "chain probe unavailable ({e}); submitting unrefined (the pre-submit \
                     simulation still blocks a reverting trade before any gas is spent)"
                ));
            },
        }
    }

    // Fresh-wallet bootstrap: if the wallet's XP balance can't cover the
    // gross supply and its one-shot initial grant is unclaimed, bundle
    // `claim_initial_grant()` into the same atomic multicall so a brand-new
    // agent wallet can claim + approve + trade in a single transaction.
    let leading = match writer.reader().config().await {
        Ok(config) => {
            bootstrap_grant_calls(
                &writer_provider,
                config.collateral_token,
                config.token_decimals,
                deadeye_starknet::Account::address(writer.account()),
                Sq128::from_raw(quote.padded_collateral).to_f64(),
                ctx,
            )
            .await
        },
        Err(_) => Vec::new(),
    };

    if !args.dry_run
        && !confirm
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
            "  required collateral: ~{:.4} XP",
            Sq128::from_raw(quote.required_collateral).to_f64()
        );
        eprintln!(
            "  supplied:  {:.4} XP",
            Sq128::from_raw(quote.padded_collateral).to_f64()
        );
        super::confirm_or_bail("Continue?")?;
    }

    // `--dry-run`: simulate the full multicall gas-free and stop.
    if args.dry_run {
        let mut calls = leading.clone();
        calls.extend(
            writer
                .build_trade_calls(&quote)
                .await
                .map_err(|e| anyhow::anyhow!("build trade calls: {e}"))?,
        );
        let result = dry_run_render(market, writer.account(), &calls).await;
        return ctx.renderer.print(&result);
    }

    let result = match writer.execute_quote_bundled(quote, leading).await {
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

/// Decide whether the trade multicall needs a leading `claim_initial_grant()`
/// to bootstrap a fresh wallet: returns `[claim]` when the trader's XP
/// balance cannot cover `gross_supply` AND the one-shot grant is unclaimed,
/// `[]` otherwise (including on read failures — the pre-submit simulation
/// remains the safety net).
async fn bootstrap_grant_calls<P>(
    provider: &P,
    collateral_token: Felt,
    token_decimals: u8,
    trader: Felt,
    gross_supply: f64,
    ctx: &AppContext,
) -> Vec<deadeye_starknet::Call>
where
    P: deadeye_starknet::Provider + Sync,
{
    let token = deadeye_starknet::CollateralTokenReader::new(provider, collateral_token);
    let (Ok(balance), Ok(claimed)) = (
        token.balance_of(trader).await,
        token.has_claimed_initial_grant(trader).await,
    ) else {
        return Vec::new();
    };
    #[expect(clippy::cast_precision_loss, reason = "balance compare is approximate")]
    let balance_xp = balance.low() as f64 / 10f64.powi(i32::from(token_decimals));
    if balance.high() > 0 || balance_xp >= gross_supply || claimed {
        return Vec::new();
    }
    if ctx.renderer.mode() != OutputMode::Json {
        eprintln!(
            "fresh wallet: balance {balance_xp:.4} XP < supply {gross_supply:.4} XP and the \
             initial grant is unclaimed — bundling claim_initial_grant() into the multicall"
        );
    }
    vec![deadeye_starknet::build_claim_initial_grant_call(
        collateral_token,
    )]
}

async fn execute_lognormal(
    ctx: &AppContext,
    client: &DeadeyeClient<CliProvider>,
    market: Felt,
    args: TradeExecuteArgs,
    confirm: bool,
    label: &'static str,
) -> Result<()> {
    // A math-runtime address is optional: with one, the legacy chain-preflight
    // quote path runs; without one (the out-of-box default — no runtime
    // instance is deployed on mainnet), the quote is drafted off-chain and the
    // chain probe below makes it submittable.
    let runtime = resolve_runtime_opt(args.runtime.as_deref(), Family::Lognormal)?;
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

    let mut quote = if let Some(rt) = runtime {
        let q = reader
            .quote_trade(rt, candidate, candidate.mu, supplied, supplied)
            .await
            .map_err(|e| anyhow::anyhow!("preflight quote_trade: {e}"))?;
        if !q.on_chain_will_accept {
            let rejection = q.rejection.as_ref().map(pretty_rejection);
            let result = SubmissionResult {
                action: "trade",
                market: format!("{market:#x}"),
                tx_hash: None,
                call_count: None,
                accepted: false,
                rejection,
                note: Some(
                    "preflight rejected — fix the cause and re-quote before retrying".into(),
                ),
            };
            return ctx.renderer.print(&result);
        }
        q
    } else {
        // Off-chain draft: solve x* with the f64 lognormal minimiser; the
        // hints + chain-exact x*/collateral come from the probe below.
        let current = reader
            .distribution()
            .await
            .map_err(|e| anyhow::anyhow!("reading market distribution: {e}"))?;
        let cand_dist = deadeye_core::LognormalDistribution::from_variance(
            Sq128::from_f64(mean)?,
            Sq128::from_f64(variance)?,
        )?;
        let solved = deadeye_sdk::collateral::lognormal_collateral(
            &current,
            &cand_dist,
            deadeye_sdk::collateral::LognormalOptions::default(),
        )
        .map_err(|e| anyhow::anyhow!("off-chain lognormal solver: {e}"))?;
        deadeye_starknet::LognormalTradeQuote {
            candidate,
            // Placeholder — replaced by the probe's chain-computed hints.
            candidate_hints: deadeye_starknet::types::lognormal::LognormalSqrtHintsRaw {
                l2_norm_denom: Sq128::ZERO.to_raw(),
                backing_denom: Sq128::ZERO.to_raw(),
            },
            x_star: Sq128::from_f64(solved.x_star)?.to_raw(),
            required_collateral: Sq128::from_f64(solved.collateral)?.to_raw(),
            padded_collateral: supplied,
            on_chain_will_accept: true,
            rejection: None,
        }
    };

    let account = build_owned_account(ctx)?;
    let writer_provider = build_provider(ctx)?;
    let writer = LognormalMarketWriter::new(
        LognormalMarketReader::new(&writer_provider, market),
        account,
    );

    // Chain-probe x* refinement (issue #13 root cause — same fixed-point
    // stationarity drift as normal markets). For the offline path this is
    // MANDATORY: it also supplies the chain-computed candidate hints, without
    // which `execute_trade` rejects the calldata.
    match deadeye_starknet::chain_probe::refine_lognormal_quote(
        writer.account(),
        writer.reader(),
        &quote,
    )
    .await
    {
        Ok(Some(outcome)) => {
            let chain_required = Sq128::from_raw(outcome.computed_collateral).to_f64();
            let gross_needed = chain_required / outcome.net_rate;
            let buffered = gross_needed * 1.002;
            if buffered > args.max_collateral {
                anyhow::bail!(
                    "chain-verified collateral is {chain_required:.4} XP net (≈{buffered:.4} XP \
                     gross incl. deposit fees), which exceeds --max-collateral {:.4}; raise the \
                     ceiling",
                    args.max_collateral,
                );
            }
            quote.x_star = outcome.x_star;
            quote.candidate_hints = outcome.candidate_hints;
            quote.required_collateral = outcome.computed_collateral;
            quote.padded_collateral = Sq128::from_f64(buffered)?.to_raw();
            if ctx.renderer.mode() != OutputMode::Json {
                eprintln!(
                    "chain probe: certified x* (offset {:+.3e}, {} round(s)); collateral \
                     {chain_required:.4} XP net → supplying {buffered:.4} XP gross (fees {:.2}%)",
                    outcome.offset,
                    outcome.rounds,
                    (1.0 - outcome.net_rate) * 100.0,
                );
            }
        },
        Ok(None) if runtime.is_none() => {
            anyhow::bail!(
                "the chain probe could not certify an x* for this candidate (and the offline \
                 path cannot construct chain-exact hints without it) — adjust the candidate \
                 (e.g. a smaller move) and retry"
            );
        },
        Err(e) if runtime.is_none() => {
            anyhow::bail!(
                "chain probe unavailable ({e}) — the offline lognormal path needs it to \
                 construct chain-exact hints; retry, or pass --runtime with a deployed \
                 math-runtime address"
            );
        },
        Ok(None) | Err(_) => {
            ctx.renderer.warning(
                "chain probe could not certify an x*; submitting the runtime-preflighted quote \
                 (the pre-submit simulation still blocks a reverting trade before any gas is \
                 spent)",
            );
        },
    }

    // Fresh-wallet bootstrap (see execute_normal).
    let leading = match writer.reader().config().await {
        Ok(config) => {
            bootstrap_grant_calls(
                &writer_provider,
                config.collateral_token,
                config.token_decimals,
                deadeye_starknet::Account::address(writer.account()),
                Sq128::from_raw(quote.padded_collateral).to_f64(),
                ctx,
            )
            .await
        },
        Err(_) => Vec::new(),
    };

    if !args.dry_run
        && !confirm
        && std::io::IsTerminal::is_terminal(&std::io::stdin())
        && ctx.renderer.mode() != OutputMode::Json
    {
        eprintln!("About to submit {label}-market trade:");
        eprintln!("  market:    {market:#x}");
        eprintln!(
            "  candidate: μ_log={:.4}, σ²_log={:.4}",
            Sq128::from_raw(quote.candidate.mu).to_f64(),
            Sq128::from_raw(quote.candidate.variance).to_f64()
        );
        eprintln!(
            "  required collateral: ~{:.4} XP",
            Sq128::from_raw(quote.required_collateral).to_f64()
        );
        eprintln!(
            "  supplied:  {:.4} XP",
            Sq128::from_raw(quote.padded_collateral).to_f64()
        );
        super::confirm_or_bail("Continue?")?;
    }

    // `--dry-run`: simulate the full multicall gas-free and stop.
    if args.dry_run {
        let mut calls = leading.clone();
        calls.extend(
            writer
                .build_trade_calls(&quote)
                .await
                .map_err(|e| anyhow::anyhow!("build trade calls: {e}"))?,
        );
        let result = dry_run_render(market, writer.account(), &calls).await;
        return ctx.renderer.print(&result);
    }

    let result = match writer.execute_quote_bundled(quote, leading).await {
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

/// Convert a fee in FRI (10⁻¹⁸ STRK) to a human STRK amount for display.
fn fri_to_strk(fri: u128) -> f64 {
    #[expect(clippy::cast_precision_loss, reason = "fee is for display only")]
    let strk = fri as f64 / 1e18_f64;
    strk
}

/// Run a **gas-free** chain simulation of the `[approve, trade]` multicall and
/// render the verdict — the `--dry-run` path. Never submits.
async fn dry_run_render<A: Account>(market: Felt, account: &A, calls: &[Call]) -> SubmissionResult {
    let market_s = format!("{market:#x}");
    let base = |accepted: bool, note: String| SubmissionResult {
        action: "trade(dry-run)",
        market: market_s.clone(),
        tx_hash: None,
        call_count: Some(calls.len()),
        accepted,
        rejection: None,
        note: Some(note),
    };
    match account.simulate(calls).await {
        Ok(Some(sim)) => match sim.revert_reason {
            Some(reason) => base(
                false,
                format!(
                    "DRY RUN — multicall WOULD REVERT on-chain: {reason}. \
                     No transaction submitted, no gas spent."
                ),
            ),
            None => base(
                true,
                format!(
                    "DRY RUN — simulation OK (≈{:.6} STRK est. fee). \
                     Re-run without --dry-run to submit.",
                    fri_to_strk(sim.estimated_fee)
                ),
            ),
        },
        Ok(None) => base(
            false,
            "DRY RUN — this account type cannot simulate (no provider-backed signer).".into(),
        ),
        Err(e) => base(false, format!("DRY RUN — simulation call failed: {e}")),
    }
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
