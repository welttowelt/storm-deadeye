//! Devnet account fixtures.
//!
//! `starknet-devnet-rs` exposes a `GET /predeployed_accounts` endpoint that
//! returns the pre-funded accounts seeded at boot (10 by default when
//! launched with `--seed 0 --accounts 10`). We fetch them on demand so
//! tests don't have to hardcode the deterministic key material.

use serde::Deserialize;
use starknet_core::types::Felt;
use thiserror::Error;
use url::Url;

use crate::devnet::DevnetError;

/// A pre-funded devnet account.
#[derive(Debug, Clone, Copy)]
pub struct DevnetAccount {
    /// Account contract address.
    pub address: Felt,
    /// Private signing key.
    pub private_key: Felt,
    /// Public key associated with the private key.
    pub public_key: Felt,
}

/// Errors raised while loading devnet accounts.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AccountError {
    /// HTTP transport error.
    #[error("devnet HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    /// Devnet returned a non-2xx status.
    #[error("devnet returned HTTP {status}")]
    Status {
        /// HTTP status code.
        status: u16,
    },
    /// Failed to parse hex strings into felts.
    #[error("failed to decode felt: {0}")]
    Felt(#[from] starknet_core::types::FromStrError),
    /// Wraps a devnet-level error.
    #[error(transparent)]
    Devnet(#[from] DevnetError),
    /// Index out of range.
    #[error("requested account #{requested} but devnet has only {total}")]
    OutOfRange {
        /// Index the test asked for.
        requested: usize,
        /// Total number of predeployed accounts on the devnet.
        total: usize,
    },
}

#[derive(Debug, Deserialize)]
struct PredeployedAccount {
    address: String,
    private_key: String,
    public_key: String,
}

/// Loads all predeployed accounts via the devnet JSON-RPC method
/// `devnet_getPredeployedAccounts` (starknet-devnet-rs 0.7+).
pub async fn predeployed(url: &Url) -> Result<Vec<DevnetAccount>, AccountError> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        result: Option<Vec<PredeployedAccount>>,
        error: Option<RpcError>,
    }
    #[derive(serde::Deserialize)]
    struct RpcError {
        message: String,
    }
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "devnet_getPredeployedAccounts",
        "params": []
    });
    let response = reqwest::Client::builder()
        .timeout(core::time::Duration::from_secs(5))
        .build()?
        .post(url.as_str())
        .json(&body)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(AccountError::Status {
            status: response.status().as_u16(),
        });
    }
    let envelope: Envelope = response.json().await?;
    if let Some(err) = envelope.error {
        return Err(AccountError::Devnet(DevnetError::Rpc(err.message)));
    }
    let raw = envelope.result.unwrap_or_default();
    raw.into_iter()
        .map(|p| {
            Ok(DevnetAccount {
                address: Felt::from_hex(&p.address)?,
                private_key: Felt::from_hex(&p.private_key)?,
                public_key: Felt::from_hex(&p.public_key)?,
            })
        })
        .collect()
}

/// Loads a single predeployed account by zero-based index.
pub async fn predeployed_one(url: &Url, index: usize) -> Result<DevnetAccount, AccountError> {
    let mut accounts = predeployed(url).await?;
    let total = accounts.len();
    if index >= total {
        return Err(AccountError::OutOfRange {
            requested: index,
            total,
        });
    }
    Ok(accounts.swap_remove(index))
}

// `url::ParseError` → `AccountError::Http` is awkward; map manually.
impl From<url::ParseError> for AccountError {
    fn from(err: url::ParseError) -> Self {
        Self::Devnet(DevnetError::Rpc(format!("invalid URL: {err}")))
    }
}
