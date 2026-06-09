//! Deployment manifests + helpers for the Deadeye contract suite.
//!
//! Today this crate provides a *typed view* over the JSON deployment
//! manifests that the upstream `the-situation` deployer emits:
//!
//! * Class hashes for every market family (AMM + math runtime + factory plugin)
//!   plus the distribution factory, oracle, insurance, etc.
//! * Deployed contract addresses (factory, oracle, plugins, collateral token).
//!
//! Embedded manifests are sourced from
//! [`deadeye_artifacts::MAINNET_DEPLOYMENT_BYTES`].
//!
//! ## Roadmap
//!
//! Sierra/CASM artifact handling (declare-on-fresh-network) is intentionally
//! out of scope for v0.1. The artifacts are ~30 MB total; embedding them
//! would bloat every consumer. The current path is:
//!
//! 1. Use [`Deployment::mainnet()`] to read class hashes pinned by the upstream
//!    deployer.
//! 2. If a fresh network needs declaration, fetch Sierra/CASM from the GitHub
//!    release assets (see `deadeye_artifacts::RELEASE_COMMIT`) and invoke
//!    `starknet_accounts::Account::declare_v3` directly.
//! 3. Use the [`Deployment::factory_address`](Deployment) field to bind a
//!    [`deadeye_starknet::FactoryReader`](::deadeye_starknet::FactoryReader)
//!    once the factory is deployed.

#![doc(html_no_source)]

pub mod runtime;

use serde::{Deserialize, Serialize};
use starknet_core::types::Felt;
use thiserror::Error;

/// Errors emitted by this crate.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DeployerError {
    /// Failed to parse the embedded manifest JSON.
    #[error("manifest JSON parse failed: {0}")]
    Json(#[from] serde_json::Error),
    /// A felt-typed field could not be parsed.
    #[error("invalid felt in field `{field}`: {value}")]
    InvalidFelt {
        /// Field name in the manifest.
        field: &'static str,
        /// Hex string that failed to parse.
        value: String,
    },
}

/// Class hashes for every contract in a Deadeye deployment.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ClassHashes {
    /// Normal AMM contract class hash.
    #[serde(rename = "normalAmmClassHash")]
    pub normal_amm: String,
    /// Normal math runtime class hash.
    #[serde(rename = "normalMathRuntimeClassHash")]
    pub normal_math_runtime: String,
    /// Normal factory plugin class hash.
    #[serde(rename = "normalFactoryPluginClassHash")]
    pub normal_factory_plugin: String,
    /// Lognormal AMM class hash.
    #[serde(rename = "lognormalAmmClassHash")]
    pub lognormal_amm: String,
    /// Lognormal math runtime class hash.
    #[serde(rename = "lognormalMathRuntimeClassHash")]
    pub lognormal_math_runtime: String,
    /// Lognormal factory plugin class hash.
    #[serde(rename = "lognormalFactoryPluginClassHash")]
    pub lognormal_factory_plugin: String,
    /// Bivariate AMM class hash.
    #[serde(rename = "bivariateAmmClassHash")]
    pub bivariate_amm: String,
    /// Bivariate math runtime class hash.
    #[serde(rename = "bivariateMathRuntimeClassHash")]
    pub bivariate_math_runtime: String,
    /// Bivariate factory plugin class hash.
    #[serde(rename = "bivariateFactoryPluginClassHash")]
    pub bivariate_factory_plugin: String,
    /// Multinoulli AMM class hash.
    #[serde(rename = "multinoulliAmmClassHash")]
    pub multinoulli_amm: String,
    /// Multinoulli math runtime class hash.
    #[serde(rename = "multinoulliMathRuntimeClassHash")]
    pub multinoulli_math_runtime: String,
    /// Multinoulli factory plugin class hash.
    #[serde(rename = "multinoulliFactoryPluginClassHash")]
    pub multinoulli_factory_plugin: String,
    /// Distribution factory contract class hash.
    #[serde(rename = "distributionFactoryClassHash")]
    pub distribution_factory: String,
    /// Oracle extension class hash.
    #[serde(rename = "oracleClassHash")]
    pub oracle: String,
    /// Insurance contract class hash (absent in early mainnet deployments).
    #[serde(rename = "insuranceClassHash", default)]
    pub insurance: String,
}

/// Collateral token info.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CollateralToken {
    /// Token contract address.
    pub address: String,
    /// Token symbol.
    pub symbol: String,
    /// Decimals.
    pub decimals: u8,
}

/// Typed view over a Deadeye deployment manifest.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Deployment {
    /// Network identifier (`"mainnet"`).
    pub network: String,
    /// Contracts release version this manifest was deployed from (e.g.
    /// `"0.13.0"`).
    #[serde(rename = "contractsVersion")]
    pub contracts_version: String,
    /// ISO-8601 timestamp.
    pub timestamp: String,
    /// Address that performed the deployment.
    pub deployer: String,
    /// Class hashes for every contract.
    #[serde(rename = "classHashes")]
    pub class_hashes: ClassHashes,
    /// Factory contract address.
    #[serde(rename = "factoryAddress")]
    pub factory_address: String,
    /// Oracle contract address.
    #[serde(rename = "oracleAddress")]
    pub oracle_address: String,
    /// Multinoulli factory-plugin contract address.
    #[serde(rename = "multinoulliPluginAddress", default)]
    pub multinoulli_plugin_address: String,
    /// Normal factory-plugin contract address.
    #[serde(rename = "normalPluginAddress", default)]
    pub normal_plugin_address: String,
    /// Lognormal factory-plugin contract address.
    #[serde(rename = "lognormalPluginAddress", default)]
    pub lognormal_plugin_address: String,
    /// Bivariate factory-plugin contract address.
    #[serde(rename = "bivariatePluginAddress", default)]
    pub bivariate_plugin_address: String,
    /// Collateral token configuration.
    #[serde(rename = "collateralToken")]
    pub collateral_token: CollateralToken,
}

impl Deployment {
    /// Decode the bundled mainnet deployment manifest.
    pub fn mainnet() -> Result<Self, DeployerError> {
        serde_json::from_slice(deadeye_artifacts::MAINNET_DEPLOYMENT_BYTES).map_err(Into::into)
    }

    /// Decode from arbitrary bytes (e.g. a user-supplied manifest file).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DeployerError> {
        serde_json::from_slice(bytes).map_err(Into::into)
    }

    /// Parse the factory address as a [`Felt`].
    pub fn factory_felt(&self) -> Result<Felt, DeployerError> {
        Felt::from_hex(&self.factory_address).map_err(|_| DeployerError::InvalidFelt {
            field: "factoryAddress",
            value: self.factory_address.clone(),
        })
    }

    /// Parse the oracle address as a [`Felt`].
    pub fn oracle_felt(&self) -> Result<Felt, DeployerError> {
        Felt::from_hex(&self.oracle_address).map_err(|_| DeployerError::InvalidFelt {
            field: "oracleAddress",
            value: self.oracle_address.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mainnet_manifest_parses() {
        let d = Deployment::mainnet().expect("mainnet manifest parses");
        assert_eq!(d.network, "mainnet");
        assert_eq!(d.contracts_version, "0.13.0");
        assert!(!d.factory_address.is_empty());
        assert!(!d.oracle_address.is_empty());
        let factory = d.factory_felt().expect("factory address is a valid felt");
        assert!(factory != Felt::ZERO);
    }

    /// The bundled release manifest must pin the live v0.13.0 contracts.
    #[test]
    fn release_version_is_pinned() {
        assert_eq!(deadeye_artifacts::RELEASE_VERSION, "0.13.0");
    }

    #[test]
    fn class_hashes_are_present() {
        let d = Deployment::mainnet().expect("mainnet manifest parses");
        assert!(!d.class_hashes.normal_amm.is_empty());
        assert!(!d.class_hashes.lognormal_amm.is_empty());
        assert!(!d.class_hashes.multinoulli_amm.is_empty());
        assert!(!d.class_hashes.bivariate_amm.is_empty());
        assert!(!d.class_hashes.distribution_factory.is_empty());
        assert!(!d.class_hashes.oracle.is_empty());
    }

    /// Drift guard: the per-family AMM class hashes must match the live
    /// v0.13.0 mainnet deployment. If the upstream deployer republishes the
    /// manifest with new class hashes, this test fails until the pins below
    /// are bumped.
    #[test]
    fn amm_class_hashes_pinned_to_v0_13() {
        let d = Deployment::mainnet().expect("mainnet manifest parses");
        for (label, manifest_raw, pinned) in [
            (
                "normal_amm",
                &d.class_hashes.normal_amm,
                "0x784ef10f901193cca1a735a122f868c987308b04cdb20c996bb4aaa804dd9d7",
            ),
            (
                "lognormal_amm",
                &d.class_hashes.lognormal_amm,
                "0x30eb01f2444b8eca1faea25607dc0679e691a7d8d363cf427b6c7343d6d5688",
            ),
            (
                "bivariate_amm",
                &d.class_hashes.bivariate_amm,
                "0x73bd6488183a278e46aabb76dfe555df1cc9b493b088a11a0a11dcc5f7e4257",
            ),
            (
                "multinoulli_amm",
                &d.class_hashes.multinoulli_amm,
                "0x32eeb4c9f11bbed4a5ecc24849424754fb2e59e11cf172bc7d794723ea4aae4",
            ),
        ] {
            let manifest_felt = Felt::from_hex(manifest_raw).expect("manifest hash parses");
            let pinned_felt = Felt::from_hex(pinned).expect("pinned hash parses");
            assert_eq!(
                manifest_felt, pinned_felt,
                "drift in {label}: manifest={manifest_raw} pinned={pinned}"
            );
        }
    }
}
