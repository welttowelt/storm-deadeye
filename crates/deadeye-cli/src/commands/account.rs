//! `deadeye account …` — read account / profile state.

use anyhow::{Context as _, Result};
use deadeye_starknet::Provider as _;
use starknet_core::{
    types::{BlockId, BlockTag, Felt, FunctionCall},
    utils::get_selector_from_name,
};

use crate::{cli::AccountCmd, context::AppContext, render::AccountView};

pub(crate) async fn run(action: AccountCmd, ctx: &AppContext) -> Result<()> {
    match action {
        AccountCmd::Show => show(ctx).await,
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
