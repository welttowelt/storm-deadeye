//! `deadeye markets …` — list / show / info read paths.

use anyhow::{Context as _, Result};
use deadeye_core::{Distribution, Sq128};
use deadeye_sdk::{DeadeyeClient, bulk::Family, starknet::JsonRpcProvider};
use deadeye_starknet::{
    BivariateMarketReader, ContractResult, Felt, LognormalMarketReader, MultinoulliMarketReader,
    NormalMarketReader,
    types::common::{AmmParamsRaw, FeeConfigRaw, LpInfoRaw},
};
use serde_json::json;

use crate::{
    cli::{FamilyArg, MarketsCmd},
    context::{AppContext, parse_address},
    render::{
        MarketFeeConfigView, MarketInfoView, MarketLpInfoView, MarketParamsView, MarketRow,
        MarketShowView, MarketStatusView,
    },
};

pub(crate) async fn run(action: MarketsCmd, ctx: &AppContext) -> Result<()> {
    match action {
        MarketsCmd::List { family, limit } => list(ctx, family, limit).await,
        MarketsCmd::Show { address, family } => show(ctx, &address, family).await,
        MarketsCmd::Info { address } => info(ctx, &address).await,
    }
}

async fn list(ctx: &AppContext, family: Option<FamilyArg>, limit: usize) -> Result<()> {
    let indexer = ctx.indexer_client()?;
    let mut markets = indexer
        .markets()
        .await
        .with_context(|| format!("indexer GET /api/markets ({}) failed", ctx.config.indexer_url))?;
    if let Some(f) = family {
        let slug = f.as_indexer_slug();
        markets.retain(|m| m.market_type == slug);
    }
    markets.truncate(limit);
    let rows: Vec<MarketRow> = markets.iter().map(MarketRow::from_summary).collect();
    if rows.is_empty() {
        ctx.renderer.warning("no markets matched the filter");
    }
    ctx.renderer.print_table(&rows)
}

async fn show(
    ctx: &AppContext,
    address: &str,
    family_override: Option<FamilyArg>,
) -> Result<()> {
    let market = parse_address(address)?;
    let client = ctx.deadeye_client()?;

    let family = if let Some(f) = family_override {
        f.as_sdk()
    } else {
        detect_family(&client, market)
            .await
            .with_context(|| format!("could not auto-detect family for market {address}"))?
    };

    let view = match family {
        Family::Normal => show_normal(&client, market, address).await?,
        Family::Lognormal => show_lognormal(&client, market, address).await?,
        Family::Multinoulli => show_multinoulli(&client, market, address).await?,
        Family::Bivariate => show_bivariate(&client, market, address).await?,
    };
    ctx.renderer.print(&view)
}

async fn info(ctx: &AppContext, address: &str) -> Result<()> {
    let indexer = ctx.indexer_client()?;
    let summary = indexer
        .market(address)
        .await
        .with_context(|| format!("indexer GET /api/markets/{address} failed"))?;
    ctx.renderer.print(&MarketInfoView { summary })
}

/// Probe each family's `params()` call until one succeeds.
pub(crate) async fn detect_family(
    client: &DeadeyeClient<JsonRpcProvider>,
    market: Felt,
) -> Result<Family> {
    // Try in the order most commonly observed in the indexer.
    if NormalMarketReader::new(client.provider(), market)
        .params()
        .await
        .is_ok()
    {
        return Ok(Family::Normal);
    }
    if LognormalMarketReader::new(client.provider(), market)
        .params()
        .await
        .is_ok()
    {
        return Ok(Family::Lognormal);
    }
    if MultinoulliMarketReader::new(client.provider(), market)
        .params()
        .await
        .is_ok()
    {
        return Ok(Family::Multinoulli);
    }
    if BivariateMarketReader::new(client.provider(), market)
        .params()
        .await
        .is_ok()
    {
        return Ok(Family::Bivariate);
    }
    anyhow::bail!(
        "no family responded to `get_params` — is the address a Deadeye AMM contract?"
    )
}

fn params_view(p: AmmParamsRaw) -> MarketParamsView {
    MarketParamsView {
        k: Sq128::from_raw(p.k).to_f64(),
        backing: Sq128::from_raw(p.backing).to_f64(),
        tolerance: Sq128::from_raw(p.tolerance).to_f64(),
        min_trade_collateral: Sq128::from_raw(p.min_trade_collateral).to_f64(),
        payout_amplifier: Sq128::from_raw(p.payout_amplifier).to_f64(),
    }
}

fn lp_view(lp: LpInfoRaw) -> MarketLpInfoView {
    MarketLpInfoView {
        total_shares: Sq128::from_raw(lp.total_shares).to_f64(),
        total_backing_deposited: Sq128::from_raw(lp.total_backing_deposited).to_f64(),
    }
}

fn fee_view(fee: FeeConfigRaw) -> MarketFeeConfigView {
    MarketFeeConfigView {
        lp_fee_bps: fee.lp_fee_bps,
        protocol_fee_bps: fee.protocol_fee_bps,
        settlement_fee_bps: fee.settlement_fee_bps,
        total_bps: fee.total_bps(),
    }
}

async fn show_normal(
    client: &DeadeyeClient<JsonRpcProvider>,
    market: Felt,
    address: &str,
) -> Result<MarketShowView> {
    let reader = NormalMarketReader::new(client.provider(), market);
    let (dist_r, params_r, lp_r, fees_r, status_r) = futures::join!(
        reader.distribution(),
        reader.params(),
        reader.lp_info(),
        reader.fee_config(),
        reader.market_status(),
    );
    let dist: deadeye_core::NormalDistribution = wrap("distribution", dist_r)?;
    let params = wrap("params", params_r)?;
    let lp = wrap("lp_info", lp_r)?;
    let fees = wrap("fee_config", fees_r)?;
    let status = status_r.ok();
    Ok(MarketShowView {
        address: address.to_owned(),
        family: "normal".to_owned(),
        distribution: json!({
            "mu": dist.mean().to_f64(),
            "sigma": dist.sigma().to_f64(),
            "variance": dist.variance().to_f64(),
        }),
        params: params_view(params),
        lp_info: lp_view(lp),
        fee_config: fee_view(fees),
        status: status.map(|s| MarketStatusView {
            is_initialised: s.is_initialised,
            is_paused: s.is_paused,
            is_settled: s.is_settled,
            settlement_value: Sq128::from_raw(s.settlement_value).to_f64(),
        }),
    })
}

async fn show_lognormal(
    client: &DeadeyeClient<JsonRpcProvider>,
    market: Felt,
    address: &str,
) -> Result<MarketShowView> {
    let reader = LognormalMarketReader::new(client.provider(), market);
    let (dist_r, params_r, lp_r, fees_r) = futures::join!(
        reader.distribution(),
        reader.params(),
        reader.lp_info(),
        reader.fee_config(),
    );
    let dist: deadeye_core::LognormalDistribution = wrap("distribution", dist_r)?;
    let params = wrap("params", params_r)?;
    let lp = wrap("lp_info", lp_r)?;
    let fees = wrap("fee_config", fees_r)?;
    let raw = dist.to_raw();
    Ok(MarketShowView {
        address: address.to_owned(),
        family: "lognormal".to_owned(),
        distribution: json!({
            "mu": Sq128::from_raw(raw.mu).to_f64(),
            "variance": Sq128::from_raw(raw.variance).to_f64(),
            "sigma": Sq128::from_raw(raw.sigma).to_f64(),
        }),
        params: params_view(params),
        lp_info: lp_view(lp),
        fee_config: fee_view(fees),
        status: None,
    })
}

async fn show_multinoulli(
    client: &DeadeyeClient<JsonRpcProvider>,
    market: Felt,
    address: &str,
) -> Result<MarketShowView> {
    let reader = MultinoulliMarketReader::new(client.provider(), market);
    let (dist_r, params_r, lp_r, fees_r) = futures::join!(
        reader.distribution(),
        reader.params(),
        reader.lp_info(),
        reader.fee_config(),
    );
    let dist: deadeye_core::CategoricalDistribution = wrap("distribution", dist_r)?;
    let params = wrap("params", params_r)?;
    let lp = wrap("lp_info", lp_r)?;
    let fees = wrap("fee_config", fees_r)?;
    let probs: Vec<f64> = dist.probs().to_vec();
    Ok(MarketShowView {
        address: address.to_owned(),
        family: "multinoulli".to_owned(),
        distribution: json!({
            "outcome_count": probs.len(),
            "probs": probs,
        }),
        params: params_view(params),
        lp_info: lp_view(lp),
        fee_config: fee_view(fees),
        status: None,
    })
}

async fn show_bivariate(
    client: &DeadeyeClient<JsonRpcProvider>,
    market: Felt,
    address: &str,
) -> Result<MarketShowView> {
    let reader = BivariateMarketReader::new(client.provider(), market);
    let (dist_r, params_r, lp_r, fees_r) = futures::join!(
        reader.distribution_raw(),
        reader.params(),
        reader.lp_info(),
        reader.fee_config(),
    );
    let dist: deadeye_core::BivariateNormalDistributionRaw = wrap("distribution", dist_r)?;
    let params = wrap("params", params_r)?;
    let lp = wrap("lp_info", lp_r)?;
    let fees = wrap("fee_config", fees_r)?;
    Ok(MarketShowView {
        address: address.to_owned(),
        family: "bivariate".to_owned(),
        distribution: json!({
            "mu1": Sq128::from_raw(dist.mu1).to_f64(),
            "mu2": Sq128::from_raw(dist.mu2).to_f64(),
            "variance1": Sq128::from_raw(dist.variance1).to_f64(),
            "variance2": Sq128::from_raw(dist.variance2).to_f64(),
            "sigma1": Sq128::from_raw(dist.sigma1).to_f64(),
            "sigma2": Sq128::from_raw(dist.sigma2).to_f64(),
            "rho": Sq128::from_raw(dist.rho).to_f64(),
        }),
        params: params_view(params),
        lp_info: lp_view(lp),
        fee_config: fee_view(fees),
        status: None,
    })
}

fn wrap<T>(name: &str, r: ContractResult<T>) -> Result<T> {
    r.map_err(|e| anyhow::anyhow!("reading {name} failed: {e}"))
}
