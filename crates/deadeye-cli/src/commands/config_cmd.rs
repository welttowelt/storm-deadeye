//! `deadeye config …` — manage the on-disk configuration file.

use anyhow::{Context as _, Result};

use crate::{
    cli::{ConfigCmd, ConfigSetArgs},
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
        ConfigCmd::Set(args) => set(ctx, args),
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
    // Backfill mainnet defaults when the user is creating fresh.
    fill_mainnet_defaults(entry);

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

/// `config set` — update only the fields the caller passed on the active
/// (or `--profile`-named) profile, creating it if absent. This is the
/// natural "change one thing about my setup" verb; `init` is the
/// create-with-defaults sibling.
fn set(ctx: &AppContext, args: ConfigSetArgs) -> Result<()> {
    // Default to the *active* profile — "update what I'm using" with no
    // ceremony. `--profile` overrides and creates on demand.
    let profile_name = args
        .profile
        .clone()
        .unwrap_or_else(|| ctx.config.profile_name.clone());

    let mut cfg = config::load()?;
    let is_new = !cfg.profiles.contains_key(&profile_name);
    let entry = cfg.profiles.entry(profile_name.clone()).or_default();

    let mut changed: Vec<&'static str> = Vec::new();
    if let Some(v) = args.address {
        entry.address = Some(v);
        changed.push("address");
    }
    if let Some(v) = args.rpc_url {
        entry.rpc_url = Some(v);
        changed.push("rpc_url");
    }
    if let Some(v) = args.indexer_url {
        entry.indexer_url = Some(v);
        changed.push("indexer_url");
    }
    if let Some(v) = args.chain_id {
        entry.chain_id = Some(v);
        changed.push("chain_id");
    }
    if let Some(v) = args.strk_token {
        entry.strk_token = Some(v);
        changed.push("strk_token");
    }
    // A brand-new profile gets mainnet defaults for anything left unset, so
    // it's immediately usable even from a single `--address`.
    if is_new {
        fill_mainnet_defaults(entry);
    }

    if changed.is_empty() && !is_new && !args.default {
        ctx.renderer
            .warning("nothing to update — pass a field flag (e.g. --rpc-url <url>) or --default");
        return Ok(());
    }

    if args.default || cfg.default_profile.is_none() {
        cfg.default_profile = Some(profile_name.clone());
    }

    let path = config::save(&cfg)?;
    let verb = if is_new { "created" } else { "updated" };
    let detail = if changed.is_empty() {
        String::new()
    } else {
        format!(" — set {}", changed.join(", "))
    };
    ctx.renderer.success(&format!(
        "{verb} profile `{profile_name}`{detail} ({})",
        path.display()
    ));
    Ok(())
}

/// Backfill the canonical mainnet RPC / indexer / chain id for any field
/// the profile hasn't set yet. Idempotent — never overwrites a value.
fn fill_mainnet_defaults(entry: &mut ProfileConfig) {
    if entry.rpc_url.is_none() {
        entry.rpc_url = Some(config::DEFAULT_MAINNET_RPC.to_owned());
    }
    if entry.indexer_url.is_none() {
        entry.indexer_url = Some(config::DEFAULT_MAINNET_INDEXER.to_owned());
    }
    if entry.chain_id.is_none() {
        entry.chain_id = Some(config::MAINNET_CHAIN_ID.to_owned());
    }
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
