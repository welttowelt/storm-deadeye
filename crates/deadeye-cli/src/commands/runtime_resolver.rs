//! Helpers for resolving common command inputs:
//!
//! * Felt parsing with friendly error messages.
//! * Math-runtime address resolution (CLI flag → family-specific env var).
//! * Market family auto-detect using the factory's `market_type_for_market`
//!   view call when a factory is configured, else by probing each family's
//!   `distribution()` reader.
//! * Owned-account construction from the resolved config + private-key env var.
//!
//! All resolution failures are surfaced as `anyhow::Error` with a hint
//! about which flag / env var would fix it.

use anyhow::{Context as _, Result, bail};
use deadeye_sdk::{
    DeadeyeClient,
    bulk::Family,
    starknet::{
        BivariateMarketReader, JsonRpcProvider, LognormalMarketReader, MultinoulliMarketReader,
        NormalMarketReader,
    },
};
use deadeye_starknet::OwnedAccount;
use starknet_core::types::Felt;
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
use url::Url;

use crate::{cli::FamilyArg, context::AppContext};

/// Parse a hex felt with a clear context message.
pub(crate) fn parse_felt(label: &str, raw: &str) -> Result<Felt> {
    Felt::from_hex(raw).with_context(|| format!("{label} `{raw}` is not a valid hex felt"))
}

/// Convert [`FamilyArg`] → SDK [`Family`].
pub(crate) const fn family_from_arg(arg: FamilyArg) -> Family {
    match arg {
        FamilyArg::Normal => Family::Normal,
        FamilyArg::Lognormal => Family::Lognormal,
        FamilyArg::Multinoulli => Family::Multinoulli,
        FamilyArg::Bivariate => Family::Bivariate,
    }
}

/// Pretty label for a family (used in rendered output).
pub(crate) const fn family_label(family: Family) -> &'static str {
    match family {
        Family::Normal => "normal",
        Family::Lognormal => "lognormal",
        Family::Multinoulli => "multinoulli",
        Family::Bivariate => "bivariate",
    }
}

/// Resolve the math-runtime contract address for `family`.
///
/// Priority: `--runtime` flag → `DEADEYE_<FAMILY>_RUNTIME_ADDR` env var.
pub(crate) fn resolve_runtime(cli_runtime: Option<&str>, family: Family) -> Result<Felt> {
    let env_var = runtime_env_var(family);
    resolve_runtime_opt(cli_runtime, family)?.with_context(|| {
        format!(
            "math runtime address required: pass `--runtime 0x...` or set `{env_var}` \
             (normal-family quotes resolve this automatically — no runtime needed)"
        )
    })
}

/// Resolution precedence for a math runtime: `--runtime` flag, then
/// `DEADEYE_<FAMILY>_RUNTIME_ADDR`. Returns `None` (rather than erroring) when
/// neither is set, so callers with a client-side path can fall back to offline.
pub(crate) fn resolve_runtime_opt(
    cli_runtime: Option<&str>,
    family: Family,
) -> Result<Option<Felt>> {
    if let Some(raw) = cli_runtime {
        return Ok(Some(parse_felt("runtime address", raw)?));
    }
    match std::env::var(runtime_env_var(family)) {
        Ok(s) if !s.trim().is_empty() => Ok(Some(parse_felt("runtime address", s.trim())?)),
        _ => Ok(None),
    }
}

const fn runtime_env_var(family: Family) -> &'static str {
    match family {
        Family::Normal => "DEADEYE_NORMAL_RUNTIME_ADDR",
        Family::Lognormal => "DEADEYE_LOGNORMAL_RUNTIME_ADDR",
        Family::Multinoulli => "DEADEYE_MULTINOULLI_RUNTIME_ADDR",
        Family::Bivariate => "DEADEYE_BIVARIATE_RUNTIME_ADDR",
    }
}

/// Best-effort family auto-detection — try each reader's `distribution()`
/// until one succeeds. Cheap on devnet; one extra round-trip on mainnet.
pub(crate) async fn detect_family<P>(client: &DeadeyeClient<P>, market: Felt) -> Result<Family>
where
    P: deadeye_starknet::Provider,
{
    let provider = client.provider();
    if NormalMarketReader::new(provider, market)
        .distribution()
        .await
        .is_ok()
    {
        return Ok(Family::Normal);
    }
    if LognormalMarketReader::new(provider, market)
        .distribution()
        .await
        .is_ok()
    {
        return Ok(Family::Lognormal);
    }
    if MultinoulliMarketReader::new(provider, market)
        .distribution()
        .await
        .is_ok()
    {
        return Ok(Family::Multinoulli);
    }
    if BivariateMarketReader::new(provider, market)
        .distribution()
        .await
        .is_ok()
    {
        return Ok(Family::Bivariate);
    }
    bail!("could not detect market family for {market:#x} — pass `--family <name>`")
}

/// Decide the family: explicit flag wins, else auto-detect.
pub(crate) async fn resolve_family<P>(
    client: &DeadeyeClient<P>,
    market: Felt,
    cli_family: Option<FamilyArg>,
) -> Result<Family>
where
    P: deadeye_starknet::Provider,
{
    if let Some(f) = cli_family {
        return Ok(family_from_arg(f));
    }
    detect_family(client, market).await
}

/// Build an [`OwnedAccount`] for the caller, sourcing the private key
/// from `DEADEYE_PRIVATE_KEY`.
pub(crate) fn build_owned_account(ctx: &AppContext) -> Result<OwnedAccount> {
    let raw_key = ctx.config.private_key.as_deref().context(
        "this command requires a private key; run `deadeye onboard`, or set DEADEYE_PRIVATE_KEY",
    )?;
    let address = ctx.resolved_address_felt()?;
    let key = parse_felt("private key", raw_key.trim())?;
    let chain_id = parse_felt("chain_id", &ctx.config.chain_id)?;

    let url = Url::parse(&ctx.config.rpc_url)
        .with_context(|| format!("invalid rpc_url: {}", ctx.config.rpc_url))?;
    let rpc = JsonRpcClient::new(HttpTransport::new(url));
    Ok(OwnedAccount::from_signing_key(rpc, address, key, chain_id))
}

/// Build a fresh provider-only client. Each call constructs its own HTTP
/// connection so concurrent code paths don't share state.
pub(crate) fn build_provider(ctx: &AppContext) -> Result<JsonRpcProvider> {
    let url = Url::parse(&ctx.config.rpc_url)
        .with_context(|| format!("invalid rpc_url: {}", ctx.config.rpc_url))?;
    let rpc = JsonRpcClient::new(HttpTransport::new(url));
    Ok(JsonRpcProvider::new(rpc))
}
