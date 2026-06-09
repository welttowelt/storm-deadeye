//! `deadeye forecast …` — the superforecasting workspace + Bayesian toolkit.
//!
//! Workspace ops accumulate evidence and reference classes per market and
//! commit a curated `(mean, σ)` snapshot. The `bayes` subcommand exposes the
//! pure toolkit (JSON in, JSON + rationale out) so an agent can aggregate
//! insights — pool estimates, blend base rates, weight evidence, de-vig the
//! market — without reimplementing the math.

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::{
    cli::{
        BayesRoutine, ForecastBaseRateAddArgs, ForecastBayesArgs, ForecastCmd,
        ForecastEvidenceAddArgs, ForecastNewArgs, ForecastQuoteArgs, ForecastSnapshotArgs,
        ForecastTradeArgs, TradeExecuteArgs, TradeQuoteArgs,
    },
    context::AppContext,
    forecast::{
        bayes,
        ledger::{self, EvidenceItem, Question, ReferenceClass, Snapshot, Workspace, now_unix},
    },
};

pub(crate) async fn run(action: ForecastCmd, ctx: &AppContext, confirm: bool) -> Result<()> {
    match action {
        ForecastCmd::New(args) => new(args),
        ForecastCmd::List => list(),
        ForecastCmd::Show { market } => show(&market),
        ForecastCmd::Evidence(args) => evidence_add(args),
        ForecastCmd::BaseRate(args) => base_rate_add(args),
        ForecastCmd::BlendBaseRates { market } => blend_base_rates(&market),
        ForecastCmd::Snapshot(args) => snapshot(args),
        ForecastCmd::Quote(args) => quote_from_snapshot(ctx, args).await,
        ForecastCmd::Trade(args) => trade_from_snapshot(ctx, args, confirm).await,
        ForecastCmd::Bayes(args) => bayes_routine(args),
    }
}

/// Load a market's committed snapshot or fail with an actionable message.
fn load_snapshot_or_bail(market: &str) -> Result<Snapshot> {
    let ws = Workspace::resolve(market)?;
    ws.load_snapshot()?.with_context(|| {
        format!(
            "no committed snapshot for {market} — create one with \
             `deadeye forecast snapshot {market} --mean <μ> --sd <σ>`"
        )
    })
}

/// `forecast quote` — quote a trade from the committed snapshot.
async fn quote_from_snapshot(ctx: &AppContext, args: ForecastQuoteArgs) -> Result<()> {
    let snap = load_snapshot_or_bail(&args.market)?;
    eprintln!(
        "Using snapshot for {}: μ={:.6}, σ={:.6} (variance {:.6})",
        args.market, snap.mean, snap.sd, snap.variance
    );
    let quote_args = if args.budget.is_some() {
        // Optimizer path — snapshot is the belief.
        TradeQuoteArgs {
            market: args.market,
            family: args.family,
            mean: None,
            variance: None,
            rho: None,
            mu2: None,
            belief: Some(snap.mean),
            budget: args.budget,
            belief_sigma: Some(args.belief_sigma.unwrap_or(snap.sd)),
            runtime: args.runtime,
            pad: args.pad,
        }
    } else {
        // Fixed-candidate path — quote the snapshot distribution directly.
        TradeQuoteArgs {
            market: args.market,
            family: args.family,
            mean: Some(snap.mean),
            variance: Some(snap.variance),
            rho: None,
            mu2: None,
            belief: None,
            budget: None,
            belief_sigma: None,
            runtime: args.runtime,
            pad: args.pad,
        }
    };
    super::trade::quote(ctx, quote_args).await
}

/// `forecast trade` — execute a trade from the committed snapshot.
async fn trade_from_snapshot(
    ctx: &AppContext,
    args: ForecastTradeArgs,
    confirm: bool,
) -> Result<()> {
    let snap = load_snapshot_or_bail(&args.market)?;
    eprintln!(
        "Using snapshot for {}: μ={:.6}, σ={:.6} (variance {:.6})",
        args.market, snap.mean, snap.sd, snap.variance
    );
    // Execute trades the snapshot distribution as a fixed candidate (the
    // execute path is fixed-candidate; the optimizer lives in `quote`).
    let execute_args = TradeExecuteArgs {
        market: args.market,
        family: args.family,
        mean: Some(snap.mean),
        variance: Some(snap.variance),
        rho: None,
        mu2: None,
        belief: None,
        budget: None,
        max_collateral: args.max_collateral,
        runtime: args.runtime,
        journal: args.journal,
    };
    super::trade::execute(ctx, execute_args, confirm).await
}

fn new(args: ForecastNewArgs) -> Result<()> {
    let ws = Workspace::resolve(&args.market)?;
    if ws.exists() {
        bail!(
            "a forecast workspace for {} already exists at {}",
            ws.market(),
            ws.dir().display()
        );
    }
    let title = args.title.unwrap_or_else(|| ws.market().to_owned());
    ws.save_question(&Question {
        market: ws.market().to_owned(),
        title: title.clone(),
        resolution_criteria: args.resolution.unwrap_or_default(),
        lower_bound: args.lower,
        upper_bound: args.upper,
        created_at: now_unix(),
    })?;
    println!(
        "Created forecast workspace for {} at {}",
        ws.market(),
        ws.dir().display()
    );
    println!("  title: {title}");
    println!("\nNext: gather evidence and set base rates, e.g.");
    println!(
        "  deadeye forecast evidence {} --claim \"...\" --stance up --source ...",
        ws.market()
    );
    println!(
        "  deadeye forecast base-rate {} --name \"...\" --rate 0.3 --applicability 0.8",
        ws.market()
    );
    Ok(())
}

fn list() -> Result<()> {
    let markets = ledger::list_markets()?;
    if markets.is_empty() {
        println!("No forecast workspaces yet. Create one with `deadeye forecast new <market>`.");
        return Ok(());
    }
    println!("Forecast workspaces:");
    for m in markets {
        let ws = Workspace::resolve(&m)?;
        let title = ws.load_question().map(|q| q.title).unwrap_or_default();
        let snap = ws.load_snapshot().ok().flatten();
        let head = snap.map_or_else(
            || "no snapshot".to_owned(),
            |s| format!("μ={:.4} σ={:.4}", s.mean, s.sd),
        );
        println!("  {m}  {head}  {title}");
    }
    Ok(())
}

fn show(market: &str) -> Result<()> {
    let ws = Workspace::resolve(market)?;
    if !ws.exists() {
        bail!(
            "no forecast workspace for {market}; create one with `deadeye forecast new {market}`"
        );
    }
    let q = ws.load_question()?;
    println!("# Forecast — {}", q.title);
    println!("market: {}", q.market);
    if !q.resolution_criteria.is_empty() {
        println!("resolves: {}", q.resolution_criteria);
    }
    if let (Some(lo), Some(hi)) = (q.lower_bound, q.upper_bound) {
        println!("range: [{lo}, {hi}]");
    }

    let evidence = ws.load_evidence()?;
    println!("\n## Evidence ({})", evidence.len());
    for e in &evidence {
        let src = e.source.as_deref().unwrap_or("-");
        println!("  [{}] ({:?}) {}  <{}>", e.id, e.stance, e.claim, src);
    }

    let classes = ws.load_base_rates()?;
    println!("\n## Reference classes ({})", classes.len());
    for c in &classes {
        println!(
            "  {} — rate {:.4}, applicability {:.2}, ±{:.3}",
            c.name, c.base_rate, c.applicability, c.uncertainty
        );
    }
    if !classes.is_empty() {
        let blend = blend(&classes);
        println!(
            "  → blended prior (mean ± sd): {:.4} ± {:.4}",
            blend.mean, blend.sd
        );
    }

    match ws.load_snapshot()? {
        Some(s) => {
            println!("\n## Snapshot (curated forecast)");
            println!("  mean (μ)   : {:.6}", s.mean);
            println!("  sd (σ)     : {:.6}", s.sd);
            println!("  variance   : {:.6}", s.variance);
            println!("  method     : {}", s.method);
            if !s.rationale.is_empty() {
                println!("  rationale  : {}", s.rationale);
            }
            print_list("reasons up", &s.reasons_up);
            print_list("reasons down", &s.reasons_down);
            print_list("change my mind", &s.change_my_mind);
            println!("\n## Trade it");
            println!(
                "  deadeye trade quote {} --mean {} --variance {}",
                q.market, s.mean, s.variance
            );
        },
        None => {
            println!(
                "\n## Snapshot\n  (none yet — commit one with `deadeye forecast snapshot {market} --mean M --sd S`)"
            );
        },
    }
    Ok(())
}

fn print_list(label: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    println!("  {label}:");
    for i in items {
        println!("    - {i}");
    }
}

fn evidence_add(args: ForecastEvidenceAddArgs) -> Result<()> {
    let ws = require_ws(&args.market)?;
    let id = ws.add_evidence(EvidenceItem {
        id: String::new(),
        captured_at: now_unix(),
        claim: args.claim,
        source: args.source,
        url: args.url,
        stance: args.stance.into_ledger(),
        reliability: args.reliability,
        relevance: args.relevance,
    })?;
    println!("Recorded evidence {id} for {}", ws.market());
    Ok(())
}

fn base_rate_add(args: ForecastBaseRateAddArgs) -> Result<()> {
    let ws = require_ws(&args.market)?;
    let mut classes = ws.load_base_rates()?;
    classes.push(ReferenceClass {
        name: args.name.clone(),
        base_rate: args.rate,
        applicability: args.applicability,
        uncertainty: args.uncertainty,
    });
    ws.save_base_rates(&classes)?;
    let blend = blend(&classes);
    println!(
        "Added reference class \"{}\" for {}",
        args.name,
        ws.market()
    );
    println!(
        "Blended prior over {} classes (mean ± sd): {:.4} ± {:.4}",
        classes.len(),
        blend.mean,
        blend.sd
    );
    Ok(())
}

fn blend_base_rates(market: &str) -> Result<()> {
    let ws = require_ws(market)?;
    let classes = ws.load_base_rates()?;
    if classes.is_empty() {
        bail!(
            "no reference classes recorded; add some with `deadeye forecast base-rate {market} ...`"
        );
    }
    let blend = blend(&classes);
    println!("{}", json!({ "mean": blend.mean, "sd": blend.sd }));
    Ok(())
}

fn snapshot(args: ForecastSnapshotArgs) -> Result<()> {
    let ws = require_ws(&args.market)?;
    let variance = args.sd * args.sd;
    ws.save_snapshot(&Snapshot {
        mean: args.mean,
        sd: args.sd,
        variance,
        method: args.method,
        rationale: args.rationale,
        reasons_up: args.reason_up,
        reasons_down: args.reason_down,
        change_my_mind: args.change_my_mind,
        created_at: now_unix(),
    })?;
    println!(
        "Committed snapshot for {}: μ={} σ={} (variance {variance})",
        ws.market(),
        args.mean,
        args.sd
    );
    println!(
        "Trade it (reads this snapshot):\n  deadeye forecast quote {0}\n  \
         deadeye forecast quote {0} --budget <XP>   # EV-max sizing",
        ws.market(),
    );
    Ok(())
}

fn require_ws(market: &str) -> Result<Workspace> {
    let ws = Workspace::resolve(market)?;
    if !ws.exists() {
        bail!(
            "no forecast workspace for {market}; create one with `deadeye forecast new {market}`"
        );
    }
    Ok(ws)
}

/// Continuous markets: blend numeric anchors in value space.
fn blend(classes: &[ReferenceClass]) -> bayes::NumericBlend {
    bayes::blend_numeric(&map_classes(classes))
}

fn map_classes(classes: &[ReferenceClass]) -> Vec<bayes::BaseRateClass> {
    classes
        .iter()
        .map(|c| bayes::BaseRateClass {
            base_rate: c.base_rate,
            applicability: c.applicability,
            uncertainty: c.uncertainty,
        })
        .collect()
}

// ─── `forecast bayes <routine>` ──────────────────────────────────────────

fn bayes_routine(args: ForecastBayesArgs) -> Result<()> {
    let input = read_input(args.input.as_deref())?;
    let v: Value = if input.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&input).context("input is not valid JSON")?
    };

    let (out, rationale) = match args.routine {
        BayesRoutine::AggregateNormal => aggregate_normal(&v)?,
        BayesRoutine::BlendBaseRates => run_blend(&v)?,
        BayesRoutine::Pool => run_pool(&v)?,
        BayesRoutine::EvidenceWeight => run_evidence_weight(&v)?,
        BayesRoutine::LrUpdate => run_lr_update(&v)?,
        BayesRoutine::Devig => run_devig(&v)?,
        BayesRoutine::EffectiveCount => run_effective_count(&v)?,
        BayesRoutine::ProbBelow => run_prob_below(&v)?,
    };

    println!(
        "{}",
        serde_json::to_string_pretty(&out).context("serializing result")?
    );
    if !args.json {
        eprintln!("{rationale}");
    }
    Ok(())
}

fn read_input(flag: Option<&str>) -> Result<String> {
    if let Some(s) = flag {
        return Ok(s.to_owned());
    }
    use std::io::Read as _;
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("reading stdin")?;
    Ok(buf)
}

fn f(v: &Value, key: &str, default: f64) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn req_f(v: &Value, key: &str) -> Result<f64> {
    v.get(key)
        .and_then(Value::as_f64)
        .with_context(|| format!("missing numeric field `{key}`"))
}

fn aggregate_normal(v: &Value) -> Result<(Value, String)> {
    let arr = v
        .get("components")
        .and_then(Value::as_array)
        .context("missing `components` array")?;
    let mut comps = Vec::with_capacity(arr.len());
    for c in arr {
        let side = match c.get("side").and_then(Value::as_str).unwrap_or("long") {
            "short" => bayes::Side::Short,
            _ => bayes::Side::Long,
        };
        comps.push(bayes::NormalComponent {
            mu: req_f(c, "mu")?,
            sigma: f(c, "sigma", 0.0),
            weight: f(c, "weight", 1.0),
            side,
        });
    }
    let rho = f(v, "rho", 0.0);
    let a = bayes::aggregate_normal(&comps, rho);
    let out = json!({
        "mean": a.mean, "sd": a.sd, "variance": a.variance,
        "q05": a.q05, "q50": a.q50, "q95": a.q95,
        "cvar05": a.cvar05, "n_eff": a.n_eff,
    });
    let rationale = format!(
        "Aggregated {} components (rho={rho}): mean {:.4}, sd {:.4}. Feed --mean {:.4} --variance {:.4} to `trade quote`.",
        comps.len(),
        a.mean,
        a.sd,
        a.mean,
        a.variance
    );
    Ok((out, rationale))
}

fn run_blend(v: &Value) -> Result<(Value, String)> {
    let arr = v
        .get("classes")
        .and_then(Value::as_array)
        .context("missing `classes` array")?;
    let classes: Vec<bayes::BaseRateClass> = arr
        .iter()
        .map(|c| bayes::BaseRateClass {
            base_rate: f(c, "base_rate", 0.0),
            applicability: f(c, "applicability", 1.0),
            uncertainty: f(c, "uncertainty", 0.0),
        })
        .collect();
    // Continuous markets blend numeric anchors in value space (default);
    // pass `"space":"probability"` for binary sub-questions.
    if v.get("space").and_then(Value::as_str) == Some("probability") {
        let b = bayes::blend_base_rates(&classes);
        return Ok((
            json!({ "blended": b.blended, "uncertainty_sd": b.uncertainty_sd, "space": "probability" }),
            format!(
                "Blended {} classes (probability): {:.4} ± {:.4}.",
                classes.len(),
                b.blended,
                b.uncertainty_sd
            ),
        ));
    }
    let b = bayes::blend_numeric(&classes);
    Ok((
        json!({ "mean": b.mean, "sd": b.sd, "space": "value" }),
        format!(
            "Blended {} classes (value): {:.4} ± {:.4}.",
            classes.len(),
            b.mean,
            b.sd
        ),
    ))
}

fn run_pool(v: &Value) -> Result<(Value, String)> {
    let arr = v
        .get("items")
        .and_then(Value::as_array)
        .context("missing `items` array")?;
    let items: Vec<(f64, f64)> = arr
        .iter()
        .map(|i| (f(i, "p", 0.5), f(i, "weight", 1.0)))
        .collect();
    let method = v
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("log_odds");
    let pooled = if method == "linear" {
        bayes::linear_pool(&items)
    } else {
        bayes::log_odds_pool(&items)
    };
    // Optional extremization for independent agreeing sources (factor > 1).
    let factor = f(v, "extremize", 1.0);
    let p = bayes::extremize(pooled, factor);
    Ok((
        json!({ "probability": p, "method": method, "extremize": factor }),
        format!(
            "Pooled {} estimates ({method}, extremize={factor}): {:.4}.",
            items.len(),
            p
        ),
    ))
}

fn run_prob_below(v: &Value) -> Result<(Value, String)> {
    let x = req_f(v, "x")?;
    let mean = req_f(v, "mean")?;
    let sd = req_f(v, "sd")?;
    let p = bayes::normal_cdf_at(x, mean, sd);
    Ok((
        json!({ "prob_below": p, "prob_above": 1.0 - p }),
        format!("P(outcome ≤ {x}) under N({mean}, {sd}²) = {p:.4}."),
    ))
}

fn run_evidence_weight(v: &Value) -> Result<(Value, String)> {
    let direction = match v.get("direction").and_then(Value::as_str).unwrap_or("for") {
        "against" => bayes::Direction::Against,
        "neutral" => bayes::Direction::Neutral,
        _ => bayes::Direction::For,
    };
    let strength = match v
        .get("strength")
        .and_then(Value::as_str)
        .unwrap_or("medium")
    {
        "negligible" => bayes::Strength::Negligible,
        "weak" => bayes::Strength::Weak,
        "modest" => bayes::Strength::Modest,
        "strong" => bayes::Strength::Strong,
        "very_strong" => bayes::Strength::VeryStrong,
        "decisive" => bayes::Strength::Decisive,
        _ => bayes::Strength::Medium,
    };
    let ew = bayes::evidence_weight(&bayes::EvidenceInput {
        reliability: f(v, "reliability", 1.0),
        relevance: f(v, "relevance", 1.0),
        independence: f(v, "independence", 1.0),
        recency: f(v, "recency", 1.0),
        bias_risk: f(v, "bias_risk", 0.0),
        direction,
        strength,
    });
    Ok((
        json!({ "likelihood_ratio": ew.likelihood_ratio, "log_odds": ew.log_odds, "quality": ew.quality }),
        format!(
            "Evidence → LR {:.3} (quality {:.2}).",
            ew.likelihood_ratio, ew.quality
        ),
    ))
}

fn run_lr_update(v: &Value) -> Result<(Value, String)> {
    let prior = req_f(v, "prior")?;
    let lrs: Vec<f64> = v
        .get("lrs")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    let posterior = bayes::apply_lrs(prior, &lrs);
    Ok((
        json!({ "prior": prior, "posterior": posterior }),
        format!(
            "Applied {} LRs: {:.4} → {:.4}.",
            lrs.len(),
            prior,
            posterior
        ),
    ))
}

fn run_devig(v: &Value) -> Result<(Value, String)> {
    let yes = req_f(v, "yes")?;
    let no = req_f(v, "no")?;
    let fair = bayes::devig_binary(yes, no);
    Ok((json!({ "fair": fair }), format!("De-vigged: {fair:.4}.")))
}

fn run_effective_count(v: &Value) -> Result<(Value, String)> {
    let n = v
        .get("n")
        .and_then(Value::as_u64)
        .context("missing integer `n`")? as usize;
    let rho = f(v, "rho", 0.0);
    let eff = bayes::effective_independent_count(n, rho);
    Ok((
        json!({ "effective": eff }),
        format!("{n} correlated items (rho={rho}) ≈ {eff:.2} independent."),
    ))
}
