//! `deadeye account …` — read account / profile state.

use anyhow::{Context as _, Result};
use deadeye_starknet::Provider as _;
use starknet_core::{
    types::{BlockId, BlockTag, Felt, FunctionCall},
    utils::get_selector_from_name,
};

use crate::{
    cli::AccountCmd, config, context::AppContext, output::OutputMode, render::AccountView,
};

pub(crate) async fn run(action: AccountCmd, ctx: &AppContext) -> Result<()> {
    match action {
        AccountCmd::Show => show(ctx).await,
        AccountCmd::List => list(ctx),
    }
}

/// List every saved wallet profile so an agent can choose one to trade from.
fn list(ctx: &AppContext) -> Result<()> {
    let cfg = config::load()?;
    if cfg.profiles.is_empty() {
        println!("No wallets yet. Create one with `deadeye onboard` (or `--profile <name>`).");
        return Ok(());
    }
    let default = cfg.default_profile.as_deref();

    if ctx.renderer.mode() == OutputMode::Json {
        let rows: Vec<serde_json::Value> = cfg
            .profiles
            .iter()
            .map(|(name, p)| {
                serde_json::json!({
                    "profile": name,
                    "default": default == Some(name.as_str()),
                    "address": p.address,
                    "chain_id": p.chain_id,
                    "rpc_url": p.rpc_url,
                    "deployed": p.account_deployed,
                    "has_key": p.private_key.is_some(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    println!("Saved wallets ( * = default, used when --profile is omitted ):");
    for (name, p) in &cfg.profiles {
        let marker = if default == Some(name.as_str()) {
            "*"
        } else {
            " "
        };
        let addr = p.address.as_deref().unwrap_or("(no address)");
        let net = chain_label(p.chain_id.as_deref());
        let deployed = if p.account_deployed {
            "deployed"
        } else {
            "not-deployed"
        };
        println!("  {marker} {name:<12} {addr}  [{net}, {deployed}]");
    }
    println!("\nTrade from a specific wallet by passing `--profile <name>` to any command.");
    Ok(())
}

/// Friendly network label for a chain id.
fn chain_label(chain_id: Option<&str>) -> &'static str {
    match chain_id {
        Some(config::MAINNET_CHAIN_ID) => "mainnet",
        Some(config::SEPOLIA_CHAIN_ID) => "sepolia",
        _ => "custom",
    }
}

async fn show(ctx: &AppContext) -> Result<()> {
    let cfg = &ctx.config;
    let address = cfg.address.as_deref().and_then(|s| Felt::from_hex(s).ok());

    let (balance_base, balance_strk) = match address {
        Some(addr) => match read_strk_balance(ctx, addr).await {
            Ok(base) => (Some(base), Some((base as f64) / 1e18_f64)),
            Err(err) => {
                ctx.renderer
                    .warning(&format!("could not read STRK balance: {err:#}"));
                (None, None)
            },
        },
        None => (None, None),
    };

    let view = AccountView {
        profile: cfg.profile_name.clone(),
        address: cfg.address.clone(),
        chain_id: cfg.chain_id.clone(),
        rpc_url: cfg.rpc_url.clone(),
        indexer_url: cfg.indexer_url.clone(),
        strk_balance_base: balance_base,
        strk_balance_strk: balance_strk,
    };
    ctx.renderer.print(&view)
}

/// Issue a `balance_of(holder)` view call against the configured STRK ERC-20.
async fn read_strk_balance(ctx: &AppContext, holder: Felt) -> Result<u128> {
    let token = Felt::from_hex(&ctx.config.strk_token)
        .with_context(|| format!("invalid STRK token address: {}", ctx.config.strk_token))?;
    let client = ctx.deadeye_client()?;
    let result = client
        .provider()
        .call(
            FunctionCall {
                contract_address: token,
                entry_point_selector: get_selector_from_name("balance_of")
                    .context("balance_of selector is a constant")?,
                calldata: vec![holder],
            },
            BlockId::Tag(BlockTag::PreConfirmed),
        )
        .await
        .map_err(|e| anyhow::anyhow!("balance_of provider call failed: {e}"))?;
    if result.len() < 2 {
        anyhow::bail!("balance_of returned {} felts (expected ≥ 2)", result.len());
    }
    let bytes = result[0].to_bytes_be();
    let (high, low) = bytes.split_at(16);
    if high.iter().any(|b| *b != 0) {
        anyhow::bail!("balance overflows u128");
    }
    let mut buf = [0_u8; 16];
    buf.copy_from_slice(low);
    Ok(u128::from_be_bytes(buf))
}
