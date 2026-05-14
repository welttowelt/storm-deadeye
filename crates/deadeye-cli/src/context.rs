//! Per-invocation context — resolved config, renderer, lazy SDK clients.

use anyhow::{Context as _, Result};
use deadeye_indexer::IndexerClient;
use deadeye_sdk::{DeadeyeClient, starknet::JsonRpcProvider};
use starknet_core::types::Felt;
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
use url::Url;

use crate::{
    cli::Cli,
    config::{self, ResolutionInputs, ResolvedConfig},
    output::{OutputMode, Renderer},
};

/// Bundle the resolved config + a renderer + lazy SDK constructors.
#[derive(Debug)]
pub(crate) struct AppContext {
    pub(crate) config: ResolvedConfig,
    pub(crate) renderer: Renderer,
}

impl AppContext {
    /// Build the context from the parsed CLI struct.
    pub(crate) fn from_cli(cli: &Cli) -> Result<Self> {
        let cfg_file = config::load()?;
        let inputs = ResolutionInputs {
            rpc_url: cli.rpc_url.clone(),
            indexer_url: cli.indexer_url.clone(),
            address: cli.address.clone(),
            profile: cli.profile.clone(),
        };
        let config = ResolvedConfig::resolve(&cfg_file, inputs)?;
        let mode = OutputMode::detect(
            cli.output.map(super::cli::OutputModeArg::into_mode),
            cli.no_color,
        );
        let renderer = Renderer::new(mode);
        Ok(Self { config, renderer })
    }

    /// Build a fresh `DeadeyeClient` over the configured RPC URL. Each
    /// call constructs a new HTTP client so callers can fork concurrent
    /// tasks without sharing a connection pool footgun.
    pub(crate) fn deadeye_client(&self) -> Result<DeadeyeClient<JsonRpcProvider>> {
        let url = Url::parse(&self.config.rpc_url)
            .with_context(|| format!("rpc_url is not a valid URL: {}", self.config.rpc_url))?;
        let rpc = JsonRpcClient::new(HttpTransport::new(url));
        let provider = JsonRpcProvider::new(rpc);
        Ok(DeadeyeClient::new(provider))
    }

    /// Build a fresh `IndexerClient` over the configured indexer URL.
    pub(crate) fn indexer_client(&self) -> Result<IndexerClient> {
        IndexerClient::new(&self.config.indexer_url)
            .with_context(|| format!("invalid indexer URL: {}", self.config.indexer_url))
    }

    /// Parse the resolved address as a hex felt, surfacing a friendly error.
    pub(crate) fn resolved_address_felt(&self) -> Result<Felt> {
        let raw = self.config.address.as_deref().context(
            "no address resolved — pass --address, set DEADEYE_ADDRESS, or store one in the active profile",
        )?;
        Felt::from_hex(raw).with_context(|| format!("address {raw} is not a valid hex felt"))
    }
}

/// Parse a hex address into a felt with a friendly error.
pub(crate) fn parse_address(s: &str) -> Result<Felt> {
    Felt::from_hex(s).with_context(|| format!("`{s}` is not a valid hex felt"))
}
