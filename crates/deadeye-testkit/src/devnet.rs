//! Devnet lifecycle helpers.
//!
//! Mirrors the `the-situation-sdk`'s `setup/devnet.ts` helpers so existing
//! integration tests have a one-to-one translation target.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

/// Default URL used by `starknet-devnet --seed 0 --port 5050`.
pub const DEFAULT_URL: &str = "http://127.0.0.1:5050";

/// Errors emitted by the devnet helpers.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DevnetError {
    /// HTTP error talking to the devnet.
    #[error("devnet HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    /// JSON-RPC payload was malformed.
    #[error("devnet JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// Devnet returned an error payload.
    #[error("devnet RPC error: {0}")]
    Rpc(String),
    /// Devnet did not become healthy within the retry budget.
    #[error("devnet at {url} did not become healthy after {attempts} attempts")]
    Unhealthy {
        /// URL we probed.
        url: Url,
        /// Number of attempts made.
        attempts: u32,
    },
}

/// Liveness check result.
#[derive(Debug, Clone, Copy)]
pub struct DevnetStatus {
    /// Whether the devnet responded to a `starknet_blockNumber` probe.
    pub alive: bool,
    /// Last reported block number, if any.
    pub block_number: Option<u64>,
}

/// Reads the chain id from the devnet via `starknet_chainId`.
pub async fn chain_id(url: &Url) -> Result<starknet_core::types::Felt, DevnetError> {
    let s: String = devnet_rpc(url, "starknet_chainId", &serde_json::json!([])).await?;
    starknet_core::types::Felt::from_hex(&s)
        .map_err(|e| DevnetError::Rpc(format!("invalid chain id felt: {e}")))
}

/// Health-checks a running devnet via `starknet_blockNumber`.
pub async fn check_health(url: &Url) -> Result<DevnetStatus, DevnetError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "starknet_blockNumber",
        "params": []
    });
    let response: JsonRpcResponse<u64> = client
        .post(url.as_str())
        .json(&body)
        .send()
        .await?
        .json()
        .await?;
    if let Some(err) = response.error {
        return Err(DevnetError::Rpc(err.message));
    }
    Ok(DevnetStatus {
        alive: response.result.is_some(),
        block_number: response.result,
    })
}

/// Waits until the devnet responds healthily.
pub async fn wait_until_ready(
    url: &Url,
    max_attempts: u32,
    retry_delay: Duration,
) -> Result<(), DevnetError> {
    for _ in 0..max_attempts {
        if matches!(check_health(url).await, Ok(s) if s.alive) {
            return Ok(());
        }
        tokio::time::sleep(retry_delay).await;
    }
    Err(DevnetError::Unhealthy {
        url: url.clone(),
        attempts: max_attempts,
    })
}

/// Resets the devnet to genesis.
pub async fn reset(url: &Url) -> Result<(), DevnetError> {
    devnet_rpc::<serde_json::Value, _>(url, "devnet_restart", &serde_json::json!({})).await?;
    Ok(())
}

/// Funded account record returned by `devnet_mint`.
#[derive(Debug, Deserialize)]
pub struct MintResponse {
    /// New balance of the funded address as a decimal string.
    pub new_balance: String,
    /// Transaction hash of the mint.
    pub tx_hash: String,
}

/// Mints ETH or STRK to `address` on the devnet (devnet-only RPC).
pub async fn mint(
    url: &Url,
    address: &str,
    amount: u128,
    unit: MintUnit,
) -> Result<MintResponse, DevnetError> {
    let payload = serde_json::json!({
        "address": address,
        "amount": amount,
        "unit": unit.as_str(),
    });
    devnet_rpc(url, "devnet_mint", &payload).await
}

/// Token unit accepted by `devnet_mint`.
#[derive(Debug, Clone, Copy)]
pub enum MintUnit {
    /// ETH, 1e18 base.
    Wei,
    /// STRK, 1e18 base.
    Fri,
}

impl MintUnit {
    fn as_str(self) -> &'static str {
        match self {
            Self::Wei => "WEI",
            Self::Fri => "FRI",
        }
    }
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse<T> {
    result: Option<T>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    message: String,
}

async fn devnet_rpc<T, P>(url: &Url, method: &str, params: &P) -> Result<T, DevnetError>
where
    T: for<'de> Deserialize<'de>,
    P: Serialize + ?Sized,
{
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    let response: JsonRpcResponse<T> = client
        .post(url.as_str())
        .json(&body)
        .send()
        .await?
        .json()
        .await?;
    if let Some(err) = response.error {
        return Err(DevnetError::Rpc(err.message));
    }
    response
        .result
        .ok_or_else(|| DevnetError::Rpc(format!("missing result for {method}")))
}
