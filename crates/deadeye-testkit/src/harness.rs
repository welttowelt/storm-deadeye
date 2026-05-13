//! Test-harness façade.
//!
//! Integration tests construct a [`Harness`] which encapsulates "where am I
//! pointed at, and is it alive?". Tests can then ask for a provider, a
//! signer, fixture accounts, etc.

use std::{env, time::Duration};

use deadeye_starknet::JsonRpcProvider;
use starknet_providers::{JsonRpcClient, jsonrpc::HttpTransport};
use thiserror::Error;
use url::Url;

use crate::{cartridge::CartridgeNetwork, devnet};

/// Which environment a [`Harness`] is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessKind {
    /// Local `starknet-devnet`.
    Devnet,
    /// Hosted Cartridge endpoint (Sepolia by default).
    Cartridge(CartridgeNetwork),
}

/// Errors raised by the harness.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HarnessError {
    /// The chosen target environment was not reachable.
    #[error("environment unreachable: {0}")]
    Unreachable(#[from] devnet::DevnetError),
    /// URL parsing failed.
    #[error("invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
}

/// Wraps a configured RPC provider for use by integration tests.
#[derive(Debug)]
pub struct Harness {
    kind: HarnessKind,
    url: Url,
    provider: JsonRpcProvider,
}

impl Harness {
    /// Constructs a harness pointed at the local devnet at the standard
    /// port, asserting it is alive.
    pub async fn devnet() -> Result<Self, HarnessError> {
        let url = Url::parse(devnet::DEFAULT_URL)?;
        devnet::wait_until_ready(&url, 10, Duration::from_secs(1)).await?;
        let provider = JsonRpcProvider::new(JsonRpcClient::new(HttpTransport::new(url.clone())));
        Ok(Self {
            kind: HarnessKind::Devnet,
            url,
            provider,
        })
    }

    /// Constructs a harness pointed at a Cartridge-hosted public RPC.
    pub fn cartridge(network: CartridgeNetwork) -> Self {
        let url = network.url();
        let provider = JsonRpcProvider::new(JsonRpcClient::new(HttpTransport::new(url.clone())));
        Self {
            kind: HarnessKind::Cartridge(network),
            url,
            provider,
        }
    }

    /// Picks a harness based on environment variables.
    ///
    /// * `DEADEYE_TEST_TARGET=devnet` → local devnet (default if unset).
    /// * `DEADEYE_TEST_TARGET=cartridge` → Cartridge, with the network
    ///   picked by [`CartridgeNetwork::from_env`].
    pub async fn from_env() -> Result<Self, HarnessError> {
        match env::var("DEADEYE_TEST_TARGET").as_deref() {
            Ok("cartridge") => Ok(Self::cartridge(CartridgeNetwork::from_env())),
            _ => Self::devnet().await,
        }
    }

    /// What kind of harness this is.
    #[must_use]
    pub fn kind(&self) -> HarnessKind {
        self.kind
    }

    /// The configured RPC URL.
    #[must_use]
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Borrow the underlying [`JsonRpcProvider`].
    #[must_use]
    pub fn provider(&self) -> &JsonRpcProvider {
        &self.provider
    }
}
