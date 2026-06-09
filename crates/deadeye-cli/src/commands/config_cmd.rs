//! `deadeye config …` — manage the on-disk configuration file.

use anyhow::{Context as _, Result};

use crate::{
    cli::ConfigCmd,
    config::{self, ProfileConfig},
    context::AppContext,
    render::{ConfigShowView, ProfileRow},
};

pub(crate) async fn run(action: ConfigCmd, ctx: &AppContext) -> Result<()> {
    match action {
        ConfigCmd::Init {
            profile,
            address,
            rpc_url,
            indexer_url,
            set_default,
        } => init(ctx, profile, address, rpc_url, indexer_url, set_default),
        ConfigCmd::Show => show(ctx),
        ConfigCmd::ProfileList => profile_list(ctx),
        ConfigCmd::ProfileUse { name } => profile_use(ctx, name),
    }
}

fn init(
    ctx: &AppContext,
    profile_name: String,
    address: Option<String>,
    rpc_url: Option<String>,
    indexer_url: Option<String>,
    set_default: bool,
) -> Result<()> {
    let mut cfg = config::load()?;
    let entry = cfg.profiles.entry(profile_name.clone()).or_default();

    if let Some(addr) = address {
        entry.address = Some(addr);
    }
    if let Some(rpc) = rpc_url {
        entry.rpc_url = Some(rpc);
    }
    if let Some(indexer) = indexer_url {
        entry.indexer_url = Some(indexer);
    }
    // Sensible defaults when the user is creating fresh.
    if entry.rpc_url.is_none() {
        entry.rpc_url = Some(config::DEFAULT_SEPOLIA_RPC.to_owned());
    }
    if entry.indexer_url.is_none() {
        entry.indexer_url = Some(config::DEFAULT_SEPOLIA_INDEXER.to_owned());
    }
    if entry.chain_id.is_none() {
        entry.chain_id = Some(config::SEPOLIA_CHAIN_ID.to_owned());
    }

    if set_default || cfg.default_profile.is_none() {
        cfg.default_profile = Some(profile_name.clone());
    }

    let path = config::save(&cfg)?;
    ctx.renderer.success(&format!(
        "updated profile `{profile_name}` in {} (existing wallet key, if any, preserved)",
        path.display()
    ));
    Ok(())
}

fn show(ctx: &AppContext) -> Result<()> {
    let path = config::config_path()?;
    let cfg = config::load()?;
    let private_key = if ctx.config.has_private_key {
        Some("***")
    } else {
        None
    };
    let view = ConfigShowView {
        config_path: path.display().to_string(),
        default_profile: cfg.default_profile.clone(),
        active_profile: ctx.config.profile_name.clone(),
        rpc_url: ctx.config.rpc_url.clone(),
        indexer_url: ctx.config.indexer_url.clone(),
        chain_id: ctx.config.chain_id.clone(),
        address: ctx.config.address.clone(),
        private_key,
    };
    ctx.renderer.print(&view)
}

fn profile_list(ctx: &AppContext) -> Result<()> {
    let cfg = config::load()?;
    let default = cfg.default_profile.clone();
    let mut rows: Vec<ProfileRow> = cfg
        .profiles
        .into_iter()
        .map(|(name, p): (String, ProfileConfig)| ProfileRow {
            name: name.clone(),
            rpc_url: p.rpc_url,
            indexer_url: p.indexer_url,
            address: p.address,
            is_default: default.as_deref() == Some(name.as_str()),
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    if rows.is_empty() {
        ctx.renderer
            .warning("no profiles configured — run `deadeye config init` first");
    }
    ctx.renderer.print_table(&rows)
}

fn profile_use(ctx: &AppContext, name: String) -> Result<()> {
    let mut cfg = config::load()?;
    if !cfg.profiles.contains_key(&name) {
        anyhow::bail!(
            "profile `{name}` not found — known profiles: {}",
            cfg.profiles.keys().cloned().collect::<Vec<_>>().join(", ")
        );
    }
    cfg.default_profile = Some(name.clone());
    let path = config::save(&cfg).context("persisting updated config")?;
    ctx.renderer.success(&format!(
        "default profile is now `{name}` ({})",
        path.display()
    ));
    Ok(())
}
