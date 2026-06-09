//! Sierra + CASM artifact loader.
//!
//! Reads the JSON artifacts produced by `scarb build` from the
//! `the-situation` workspace. The directory is resolved via the
//! `THE_SITUATION_TARGET_DIR` environment variable, defaulting to
//! `../../the-situation/target/dev` relative to the deadeye-rs workspace
//! root.

use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    sync::Arc,
};

use starknet_core::types::{
    Felt, FlattenedSierraClass,
    contract::{CompiledClass, SierraClass},
};
use thiserror::Error;

/// Errors emitted by the artifact loader.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ArtifactError {
    /// I/O failure reading the file.
    #[error("could not read {path}: {source}")]
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// JSON parse failure.
    #[error("could not parse {path}: {source}")]
    Json {
        /// Path that failed.
        path: PathBuf,
        /// Underlying serde error.
        #[source]
        source: serde_json::Error,
    },
    /// Class-hash computation failed.
    #[error("could not compute class hash for {path}: {message}")]
    ClassHash {
        /// Path that failed.
        path: PathBuf,
        /// Diagnostic message.
        message: String,
    },
}

/// A loaded contract class with both Sierra and CASM views.
#[derive(Debug, Clone)]
pub struct ContractArtifact {
    /// Stable human-readable name for diagnostics.
    pub name: &'static str,
    /// Flattened Sierra class, ready for `Account::declare_v3`.
    pub flattened_sierra: Arc<FlattenedSierraClass>,
    /// Sierra class hash.
    pub class_hash: Felt,
    /// CASM (compiled) class hash.
    pub compiled_class_hash: Felt,
}

/// Resolves the artifacts directory.
#[must_use]
pub fn artifacts_dir() -> PathBuf {
    if let Ok(value) = std::env::var("THE_SITUATION_TARGET_DIR") {
        return PathBuf::from(value);
    }
    // Default: walk up to the workspace parent, then into the-situation/.
    // CARGO_MANIFEST_DIR for this crate is
    //   .../the-situation-stack/deadeye-rs/crates/deadeye-testkit
    // so three .parent() hops land at .../the-situation-stack.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let walked = manifest
        .parent() // crates
        .and_then(|p| p.parent()) // deadeye-rs
        .and_then(|p| p.parent()); // the-situation-stack
    match walked {
        Some(p) => p.join("the-situation").join("target").join("dev"),
        None => PathBuf::from("the-situation/target/dev"),
    }
}

/// Loads a single contract artifact given its base filename (no extension).
///
/// Reads `<name>.contract_class.json` and
/// `<name>.compiled_contract_class.json`.
pub fn load(name: &'static str, base: &str) -> Result<ContractArtifact, ArtifactError> {
    let dir = artifacts_dir();
    let sierra_path = dir.join(format!("{base}.contract_class.json"));
    let casm_path = dir.join(format!("{base}.compiled_contract_class.json"));

    let sierra: SierraClass = read_json(&sierra_path)?;
    let class_hash = sierra.class_hash().map_err(|e| ArtifactError::ClassHash {
        path: sierra_path.clone(),
        message: e.to_string(),
    })?;
    let flattened = sierra.flatten().map_err(|e| ArtifactError::Json {
        path: sierra_path.clone(),
        source: serde_json::Error::custom(e.to_string()),
    })?;

    let casm: CompiledClass = read_json(&casm_path)?;
    let compiled_class_hash = casm.class_hash().map_err(|e| ArtifactError::ClassHash {
        path: casm_path.clone(),
        message: e.to_string(),
    })?;

    Ok(ContractArtifact {
        name,
        flattened_sierra: Arc::new(flattened),
        class_hash,
        compiled_class_hash,
    })
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, ArtifactError> {
    let file = File::open(path).map_err(|e| ArtifactError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let reader = BufReader::new(file);
    serde_json::from_reader(reader).map_err(|e| ArtifactError::Json {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Bundle of all contract artifacts needed for a full devnet bootstrap.
#[derive(Debug, Clone)]
pub struct AllArtifacts {
    /// Distribution factory contract.
    pub factory: ContractArtifact,
    /// Oracle contract.
    pub oracle: ContractArtifact,
    /// Restricted collateral token (used as test collateral).
    pub restricted_collateral_token: ContractArtifact,
    /// Normal AMM contract.
    pub normal_amm: ContractArtifact,
    /// Normal math runtime contract.
    pub normal_math_runtime: ContractArtifact,
    /// Normal factory plugin.
    pub normal_factory_plugin: ContractArtifact,
    /// Lognormal AMM contract.
    pub lognormal_amm: ContractArtifact,
    /// Lognormal math runtime contract.
    pub lognormal_math_runtime: ContractArtifact,
    /// Lognormal factory plugin.
    pub lognormal_factory_plugin: ContractArtifact,
    /// Multinoulli AMM contract.
    pub multinoulli_amm: ContractArtifact,
    /// Multinoulli math runtime contract.
    pub multinoulli_math_runtime: ContractArtifact,
    /// Multinoulli factory plugin.
    pub multinoulli_factory_plugin: ContractArtifact,
    /// Bivariate AMM contract.
    pub bivariate_amm: ContractArtifact,
    /// Bivariate math runtime contract.
    pub bivariate_math_runtime: ContractArtifact,
    /// Bivariate factory plugin.
    pub bivariate_factory_plugin: ContractArtifact,
}

impl AllArtifacts {
    /// Load every contract artifact from disk.
    pub fn load() -> Result<Self, ArtifactError> {
        Ok(Self {
            factory: load("distribution_factory", "factory_distribution_factory")?,
            oracle: load("oracle", "the_situation_oracle")?,
            restricted_collateral_token: load(
                "restricted_collateral_token",
                "restricted_collateral_token_restricted_collateral_token",
            )?,
            normal_amm: load("normal_amm", "factory_normal_amm")?,
            normal_math_runtime: load("normal_math_runtime", "factory_normal_math_runtime")?,
            normal_factory_plugin: load("normal_factory_plugin", "factory_normal_factory_plugin")?,
            lognormal_amm: load("lognormal_amm", "factory_lognormal_amm")?,
            lognormal_math_runtime: load(
                "lognormal_math_runtime",
                "factory_lognormal_math_runtime",
            )?,
            lognormal_factory_plugin: load(
                "lognormal_factory_plugin",
                "factory_lognormal_factory_plugin",
            )?,
            multinoulli_amm: load("multinoulli_amm", "factory_multinoulli_amm")?,
            multinoulli_math_runtime: load(
                "multinoulli_math_runtime",
                "factory_multinoulli_math_runtime",
            )?,
            multinoulli_factory_plugin: load(
                "multinoulli_factory_plugin",
                "factory_multinoulli_factory_plugin",
            )?,
            bivariate_amm: load("bivariate_amm", "factory_bivariate_amm")?,
            bivariate_math_runtime: load(
                "bivariate_math_runtime",
                "factory_bivariate_math_runtime",
            )?,
            bivariate_factory_plugin: load(
                "bivariate_factory_plugin",
                "factory_bivariate_normal_factory_plugin",
            )?,
        })
    }
}

// Local helper trait to convert flatten errors into serde JSON errors
// uniformly.
trait CustomError {
    fn custom(msg: String) -> Self;
}

impl CustomError for serde_json::Error {
    fn custom(msg: String) -> Self {
        <Self as serde::de::Error>::custom(msg)
    }
}
