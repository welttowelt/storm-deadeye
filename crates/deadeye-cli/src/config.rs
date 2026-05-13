//! Configuration file loading + profile resolution.
//!
//! Config lives at `~/.config/deadeye/config.toml` (override path with
//! the `DEADEYE_CONFIG` env var). The file lists named profiles and an
//! optional `default_profile`. Private keys are **never** persisted to
//! disk — they must come from the `DEADEYE_PRIVATE_KEY` env var.

use std::{collections::BTreeMap, fs, io::Write as _, path::PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

/// Default public Sepolia RPC endpoint.
pub(crate) const DEFAULT_SEPOLIA_RPC: &str = "https://starknet-sepolia.drpc.org";

/// Default Sepolia indexer URL.
pub(crate) const DEFAULT_SEPOLIA_INDEXER: &str = "https://situation-indexer.fly.dev";

/// Canonical Sepolia chain id (`SN_SEPOLIA`).
pub(crate) const SEPOLIA_CHAIN_ID: &str = "0x534e5f5345504f4c4941";

/// Canonical STRK ERC-20 address (Sepolia & mainnet share the deployment).
pub(crate) const STRK_TOKEN_ADDRESS: &str =
    "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";

/// Top-level config document.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ConfigFile {
    /// Profile name to use when `--profile` and `DEADEYE_PROFILE` are unset.
    pub(crate) default_profile: Option<String>,
    /// Named profiles.
    pub(crate) profiles: BTreeMap<String, ProfileConfig>,
}

/// One named profile. Maps to a single RPC + indexer + address triple.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ProfileConfig {
    /// Starknet JSON-RPC URL.
    pub(crate) rpc_url: Option<String>,
    /// Indexer base URL.
    pub(crate) indexer_url: Option<String>,
    /// Chain id (hex felt).
    pub(crate) chain_id: Option<String>,
    /// Default trader / account address (hex felt).
    pub(crate) address: Option<String>,
    /// ERC-20 collateral token (defaults to canonical STRK).
    pub(crate) strk_token: Option<String>,
}

/// Path resolver — returns `~/.config/deadeye/config.toml` unless
/// `DEADEYE_CONFIG` overrides it.
pub(crate) fn config_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("DEADEYE_CONFIG") {
        return Ok(PathBuf::from(path));
    }
    let mut dir = dirs::config_dir()
        .context("could not locate user config dir; set DEADEYE_CONFIG to override")?;
    dir.push("deadeye");
    dir.push("config.toml");
    Ok(dir)
}

/// Load the config file. Returns an empty `ConfigFile` if it doesn't exist.
pub(crate) fn load() -> Result<ConfigFile> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(ConfigFile::default());
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("reading config file at {}", path.display()))?;
    let parsed: ConfigFile = toml::from_str(&raw)
        .with_context(|| format!("parsing TOML in {}", path.display()))?;
    Ok(parsed)
}

/// Persist a config file. Creates parent directories as needed.
pub(crate) fn save(cfg: &ConfigFile) -> Result<PathBuf> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating config directory {}", parent.display()))?;
    }
    let body = toml::to_string_pretty(cfg).context("serializing config to TOML")?;
    let mut header = String::from(
        "# Deadeye CLI configuration.\n\
         # Private keys are NEVER stored here.\n\
         # Set DEADEYE_PRIVATE_KEY in your environment instead.\n\
         #\n\
         # See `deadeye config --help` for management commands.\n\n",
    );
    header.push_str(&body);
    let mut file = fs::File::create(&path)
        .with_context(|| format!("opening {} for write", path.display()))?;
    file.write_all(header.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Resolved configuration after merging CLI flags + env + file defaults.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedConfig {
    pub(crate) profile_name: String,
    pub(crate) rpc_url: String,
    pub(crate) indexer_url: String,
    pub(crate) chain_id: String,
    pub(crate) address: Option<String>,
    pub(crate) strk_token: String,
    /// `true` iff a private key is plumbed in via env (write paths only).
    pub(crate) has_private_key: bool,
}

/// Inputs from the CLI that influence resolution.
#[derive(Debug, Default, Clone)]
pub(crate) struct ResolutionInputs {
    pub(crate) rpc_url: Option<String>,
    pub(crate) indexer_url: Option<String>,
    pub(crate) address: Option<String>,
    pub(crate) profile: Option<String>,
}

impl ResolvedConfig {
    /// Resolve config in priority order:
    ///   1. CLI flag (passed in via `inputs`)
    ///   2. Env var (DEADEYE_RPC_URL, DEADEYE_INDEXER_URL, …)
    ///   3. Profile in `cfg`
    ///   4. Built-in defaults (Sepolia public RPC + indexer).
    pub(crate) fn resolve(cfg: &ConfigFile, inputs: ResolutionInputs) -> Result<Self> {
        // Profile name: CLI > env (DEADEYE_PROFILE) > cfg.default_profile > "sepolia".
        let profile_name = inputs
            .profile
            .or_else(|| std::env::var("DEADEYE_PROFILE").ok())
            .or_else(|| cfg.default_profile.clone())
            .unwrap_or_else(|| "sepolia".to_owned());

        let profile = cfg.profiles.get(&profile_name).cloned().unwrap_or_default();

        let rpc_url = inputs
            .rpc_url
            .or_else(|| std::env::var("DEADEYE_RPC_URL").ok())
            .or(profile.rpc_url)
            .unwrap_or_else(|| DEFAULT_SEPOLIA_RPC.to_owned());

        let indexer_url = inputs
            .indexer_url
            .or_else(|| std::env::var("DEADEYE_INDEXER_URL").ok())
            .or(profile.indexer_url)
            .unwrap_or_else(|| DEFAULT_SEPOLIA_INDEXER.to_owned());

        let chain_id = std::env::var("DEADEYE_CHAIN_ID")
            .ok()
            .or(profile.chain_id)
            .unwrap_or_else(|| SEPOLIA_CHAIN_ID.to_owned());

        let address = inputs
            .address
            .or_else(|| std::env::var("DEADEYE_ADDRESS").ok())
            .or(profile.address);

        let strk_token = profile
            .strk_token
            .unwrap_or_else(|| STRK_TOKEN_ADDRESS.to_owned());

        let has_private_key = std::env::var_os("DEADEYE_PRIVATE_KEY").is_some();

        Ok(Self {
            profile_name,
            rpc_url,
            indexer_url,
            chain_id,
            address,
            strk_token,
            has_private_key,
        })
    }
}
