//! Class declaration helper with on-chain idempotency check.

use std::{collections::HashMap, fs, path::PathBuf, sync::Arc};

use starknet_accounts::{Account, ConnectedAccount};
use starknet_core::types::{BlockId, BlockTag, Felt, StarknetError};
use starknet_providers::{Provider, ProviderError};
use thiserror::Error;

use crate::fixture::artifacts::ContractArtifact;

/// Errors emitted by the declarer.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DeclareError {
    /// Wraps the upstream account error.
    #[error("declare failed: {0}")]
    Account(String),
    /// Wraps the upstream provider error.
    #[error("provider failure: {0}")]
    Provider(#[from] ProviderError),
    /// Failed to extract the chain-expected compiled class hash from the
    /// rejection error.
    #[error("could not recover compiled class hash from error: {0}")]
    HashRecovery(String),
}

/// Path to the persistent compiled-class-hash cache. Survives across test
/// runs; gets blown away when devnet resets so we re-discover hashes
/// against the new chain instance.
fn cache_path() -> PathBuf {
    PathBuf::from("/tmp/deadeye_casm_hashes.json")
}

fn load_cache() -> HashMap<String, String> {
    fs::read_to_string(cache_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_cache(cache: &HashMap<String, String>) {
    if let Ok(s) = serde_json::to_string_pretty(cache) {
        let _ = fs::write(cache_path(), s);
    }
}

/// Extracts the chain's expected compiled class hash from a declare error
/// message of the form
///   "Mismatch compiled class hash for class with hash 0x… . Actual: 0x…, Expected: 0x…".
fn extract_expected_hash(error_text: &str) -> Option<Felt> {
    let marker = "Expected: ";
    let start = error_text.find(marker)? + marker.len();
    let tail = error_text.get(start..)?;
    let end = tail
        .find(|c: char| !c.is_ascii_hexdigit() && c != 'x' && c != 'X')
        .unwrap_or(tail.len());
    let hex_str = tail.get(..end)?;
    Felt::from_hex(hex_str).ok()
}

/// Declare `artifact.flattened_sierra` if not already on chain. Returns the
/// Sierra class hash either way.
///
/// Discovers the chain's expected compiled class hash on the first
/// attempt by submitting our locally-computed value, parsing the
/// rejection error if the chain disagrees, caching the correct hash to
/// `/tmp/deadeye_casm_hashes.json`, and retrying. On subsequent runs the
/// cached hash is used directly.
pub async fn declare_idempotent<A>(
    account: &A,
    artifact: &ContractArtifact,
) -> Result<Felt, DeclareError>
where
    A: Account + ConnectedAccount + Sync,
{
    // Quick check: if the class is already declared on chain, skip.
    let already_declared = match account
        .provider()
        .get_class(BlockId::Tag(BlockTag::PreConfirmed), artifact.class_hash)
        .await
    {
        Ok(_) => true,
        Err(ProviderError::StarknetError(StarknetError::ClassHashNotFound)) => false,
        Err(other) => return Err(DeclareError::Provider(other)),
    };
    if already_declared {
        return Ok(artifact.class_hash);
    }

    let flattened: Arc<starknet_core::types::FlattenedSierraClass> =
        Arc::clone(&artifact.flattened_sierra);
    let cache_key = format!("{:#x}", artifact.class_hash);
    let mut cache = load_cache();

    // Use cached hash if we've seen this Sierra class before.
    let initial_hash = cache
        .get(&cache_key)
        .and_then(|s| Felt::from_hex(s).ok())
        .unwrap_or(artifact.compiled_class_hash);

    match account
        .declare_v3(Arc::clone(&flattened), initial_hash)
        .send()
        .await
    {
        Ok(result) => {
            // Persist on success so future runs skip the discovery dance.
            cache.insert(cache_key, format!("{initial_hash:#x}"));
            save_cache(&cache);
            Ok(result.class_hash)
        },
        Err(e) => {
            let text = format!("{e}");
            if let Some(expected) = extract_expected_hash(&text) {
                // Retry with the chain's expected hash and cache it.
                let result = account
                    .declare_v3(flattened, expected)
                    .send()
                    .await
                    .map_err(|e2| {
                        DeclareError::Account(format!("retry after hash discovery: {e2}"))
                    })?;
                cache.insert(cache_key, format!("{expected:#x}"));
                save_cache(&cache);
                Ok(result.class_hash)
            } else {
                Err(DeclareError::Account(text))
            }
        },
    }
}

/// Declared class hashes for the full Deadeye contract suite.
#[derive(Debug, Clone, Copy)]
pub struct DeclaredHashes {
    /// Distribution factory.
    pub factory: Felt,
    /// Oracle.
    pub oracle: Felt,
    /// Restricted collateral token.
    pub restricted_collateral_token: Felt,
    /// Normal AMM.
    pub normal_amm: Felt,
    /// Normal math runtime.
    pub normal_math_runtime: Felt,
    /// Normal factory plugin.
    pub normal_factory_plugin: Felt,
    /// Lognormal AMM.
    pub lognormal_amm: Felt,
    /// Lognormal math runtime.
    pub lognormal_math_runtime: Felt,
    /// Lognormal factory plugin.
    pub lognormal_factory_plugin: Felt,
    /// Multinoulli AMM.
    pub multinoulli_amm: Felt,
    /// Multinoulli math runtime.
    pub multinoulli_math_runtime: Felt,
    /// Multinoulli factory plugin.
    pub multinoulli_factory_plugin: Felt,
    /// Bivariate AMM.
    pub bivariate_amm: Felt,
    /// Bivariate math runtime.
    pub bivariate_math_runtime: Felt,
    /// Bivariate factory plugin.
    pub bivariate_factory_plugin: Felt,
}

/// Declare the entire Deadeye contract suite (15 contracts) sequentially.
///
/// Sequential because nonce conflicts on devnet are silent and ugly.
pub async fn declare_all<A>(
    account: &A,
    artifacts: &crate::fixture::artifacts::AllArtifacts,
) -> Result<DeclaredHashes, DeclareError>
where
    A: Account + ConnectedAccount + Sync,
{
    Ok(DeclaredHashes {
        factory: declare_idempotent(account, &artifacts.factory).await?,
        oracle: declare_idempotent(account, &artifacts.oracle).await?,
        restricted_collateral_token: declare_idempotent(
            account,
            &artifacts.restricted_collateral_token,
        )
        .await?,
        normal_amm: declare_idempotent(account, &artifacts.normal_amm).await?,
        normal_math_runtime: declare_idempotent(account, &artifacts.normal_math_runtime).await?,
        normal_factory_plugin: declare_idempotent(account, &artifacts.normal_factory_plugin)
            .await?,
        lognormal_amm: declare_idempotent(account, &artifacts.lognormal_amm).await?,
        lognormal_math_runtime: declare_idempotent(account, &artifacts.lognormal_math_runtime)
            .await?,
        lognormal_factory_plugin: declare_idempotent(account, &artifacts.lognormal_factory_plugin)
            .await?,
        multinoulli_amm: declare_idempotent(account, &artifacts.multinoulli_amm).await?,
        multinoulli_math_runtime: declare_idempotent(account, &artifacts.multinoulli_math_runtime)
            .await?,
        multinoulli_factory_plugin: declare_idempotent(
            account,
            &artifacts.multinoulli_factory_plugin,
        )
        .await?,
        bivariate_amm: declare_idempotent(account, &artifacts.bivariate_amm).await?,
        bivariate_math_runtime: declare_idempotent(account, &artifacts.bivariate_math_runtime)
            .await?,
        bivariate_factory_plugin: declare_idempotent(account, &artifacts.bivariate_factory_plugin)
            .await?,
    })
}
