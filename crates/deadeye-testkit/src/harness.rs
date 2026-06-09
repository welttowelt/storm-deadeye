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

use crate::devnet;

/// Default hosted RPC for integration runs — ZAN's public mainnet node
/// (JSON-RPC `v0_10`). Override with `DEADEYE_TEST_RPC`.
pub const DEFAULT_HOSTED_RPC: &str = "https://api.zan.top/public/starknet-mainnet/rpc/v0_10";

/// The default hosted mainnet RPC URL, parsed.
#[must_use]
pub fn default_mainnet_rpc() -> Url {
    Url::parse(DEFAULT_HOSTED_RPC).expect("static mainnet URL parses")
}

/// Which environment a [`Harness`] is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessKind {
    /// Local `starknet-devnet`.
    Devnet,
    /// A hosted public RPC (mainnet).
    Hosted,
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

    /// Constructs a harness pointed at a hosted public RPC.
    pub fn hosted(url: Url) -> Self {
        let provider = JsonRpcProvider::new(JsonRpcClient::new(HttpTransport::new(url.clone())));
        Self {
            kind: HarnessKind::Hosted,
            url,
            provider,
        }
    }

    /// Picks a harness based on environment variables.
    ///
    /// * `DEADEYE_TEST_TARGET=devnet` → local devnet (default if unset).
    /// * `DEADEYE_TEST_TARGET=hosted` → a hosted public RPC; the URL comes from
    ///   `DEADEYE_TEST_RPC` (default [`DEFAULT_HOSTED_RPC`], mainnet).
    pub async fn from_env() -> Result<Self, HarnessError> {
        match env::var("DEADEYE_TEST_TARGET").as_deref() {
            Ok("hosted") => {
                let raw =
                    env::var("DEADEYE_TEST_RPC").unwrap_or_else(|_| DEFAULT_HOSTED_RPC.to_owned());
                Ok(Self::hosted(Url::parse(&raw)?))
            },
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
