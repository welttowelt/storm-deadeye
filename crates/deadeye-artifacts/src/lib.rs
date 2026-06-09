//! Compile-time-embedded Starknet ABIs and release manifest for Deadeye.
//!
//! All ABIs are bundled via [`include_bytes!`], so this crate has zero
//! runtime I/O and is safe to use from `no_std` contexts (with the
//! `parsed` feature disabled).
//!
//! # Layout
//!
//! Each public constant points at one of the JSON ABI blobs sourced from
//! the upstream `the-situation` contract releases. The release manifest is
//! exposed alongside so downstream tooling can verify which contract
//! commit a binary was built against.
//!
//! # Versioning
//!
//! The bundled artifacts are pinned per-release. Use [`MANIFEST_BYTES`] to
//! retrieve the JSON manifest and pair it with [`RELEASE_VERSION`] /
//! [`RELEASE_COMMIT`] for sanity-checks at startup.

#![cfg_attr(not(feature = "parsed"), no_std)]

/// Bundled ABI for a single contract.
#[derive(Debug, Clone, Copy)]
pub struct Abi {
    /// Stable, human-readable name (matches the manifest entry).
    pub name: &'static str,
    /// JSON-encoded ABI bytes.
    pub bytes: &'static [u8],
}

macro_rules! abi {
    ($name:literal, $path:literal) => {
        Abi {
            name: $name,
            bytes: include_bytes!(concat!("../abis/", $path)),
        }
    };
}

/// Normal AMM ABI (Gaussian distribution markets).
pub const NORMAL_AMM: Abi = abi!("normal_amm", "normal_amm.abi.json");
/// Lognormal AMM ABI.
pub const LOGNORMAL_AMM: Abi = abi!("lognormal_amm", "lognormal_amm.abi.json");
/// Multinoulli (categorical) AMM ABI.
pub const MULTINOULLI_AMM: Abi = abi!("multinoulli_amm", "multinoulli_amm.abi.json");
/// Bivariate normal AMM ABI.
pub const BIVARIATE_AMM: Abi = abi!("bivariate_amm", "bivariate_amm.abi.json");
/// Distribution factory ABI.
pub const FACTORY: Abi = abi!("factory", "factory.abi.json");
/// Oracle ABI.
pub const ORACLE: Abi = abi!("oracle", "oracle.abi.json");
/// Normal math runtime ABI.
pub const NORMAL_MATH_RUNTIME: Abi = abi!("normal_math_runtime", "normal_math_runtime.abi.json");
/// Lognormal math runtime ABI.
pub const LOGNORMAL_MATH_RUNTIME: Abi =
    abi!("lognormal_math_runtime", "lognormal_math_runtime.abi.json");
/// Multinoulli math runtime ABI.
pub const MULTINOULLI_MATH_RUNTIME: Abi = abi!(
    "multinoulli_math_runtime",
    "multinoulli_math_runtime.abi.json"
);
/// Bivariate math runtime ABI.
pub const BIVARIATE_MATH_RUNTIME: Abi =
    abi!("bivariate_math_runtime", "bivariate_math_runtime.abi.json");
/// Restricted collateral token ABI.
pub const RESTRICTED_COLLATERAL_TOKEN: Abi = abi!(
    "restricted_collateral_token",
    "restricted_collateral_token.abi.json"
);

/// Every bundled ABI, in a stable order.
pub const ALL: &[Abi] = &[
    NORMAL_AMM,
    LOGNORMAL_AMM,
    MULTINOULLI_AMM,
    BIVARIATE_AMM,
    FACTORY,
    ORACLE,
    NORMAL_MATH_RUNTIME,
    LOGNORMAL_MATH_RUNTIME,
    MULTINOULLI_MATH_RUNTIME,
    BIVARIATE_MATH_RUNTIME,
    RESTRICTED_COLLATERAL_TOKEN,
];

/// Raw JSON bytes of the release manifest.
pub const MANIFEST_BYTES: &[u8] = include_bytes!("../abis/release-manifest.json");

/// Raw JSON bytes of the deployed-class manifest for mainnet.
pub const MAINNET_DEPLOYMENT_BYTES: &[u8] = include_bytes!("../abis/deployment-mainnet.json");

/// Pinned contract release version, parsed once at compile time.
pub const RELEASE_VERSION: &str = release_field(MANIFEST_BYTES, b"\"version\"");
/// Pinned contract release commit hash.
pub const RELEASE_COMMIT: &str = release_field(MANIFEST_BYTES, b"\"commit\"");

/// Looks up a top-level string field in the JSON manifest at *compile time*.
///
/// We avoid pulling in a full JSON parser at const-eval; the manifest is
/// machine-generated and the field encoding is stable. If a field is
/// missing the build fails with a clear panic — which is the desired
/// behaviour for a const-evaluated assertion.
#[doc(hidden)]
#[expect(
    clippy::panic,
    clippy::manual_let_else,
    clippy::manual_assert,
    reason = "const evaluation cannot use Result-based control flow"
)]
const fn release_field(bytes: &'static [u8], key: &[u8]) -> &'static str {
    let key_pos = match find(bytes, key, 0) {
        Some(p) => p,
        None => panic!("release manifest missing required field"),
    };
    // Advance past `"key":`.
    let mut cursor = key_pos + key.len();
    while cursor < bytes.len() && (bytes[cursor] == b' ' || bytes[cursor] == b':') {
        cursor += 1;
    }
    if cursor >= bytes.len() || bytes[cursor] != b'"' {
        panic!("release manifest field is not a string");
    }
    cursor += 1;
    let start = cursor;
    while cursor < bytes.len() && bytes[cursor] != b'"' {
        cursor += 1;
    }
    if cursor >= bytes.len() {
        panic!("unterminated string in release manifest");
    }
    let slice = const_slice(bytes, start, cursor);
    match core::str::from_utf8(slice) {
        Ok(s) => s,
        Err(_) => panic!("release manifest field is not UTF-8"),
    }
}

/// `const`-context subslicing helper (no panicking indexing).
const fn const_slice(bytes: &'static [u8], start: usize, end: usize) -> &'static [u8] {
    // bytes[start..end] is OK in const context, but explicit form clarifies
    // the intent and survives future const-eval restrictions.
    bytes.split_at(end).0.split_at(start).1
}

/// Linear search for `needle` in `haystack` starting at `from`.
const fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from);
    }
    let mut i = from;
    while i + needle.len() <= haystack.len() {
        let mut j = 0;
        while j < needle.len() && haystack[i + j] == needle[j] {
            j += 1;
        }
        if j == needle.len() {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(feature = "parsed")]
mod parsed {
    //! JSON-aware views over the embedded artifacts.

    use serde::Deserialize;

    use super::{Abi, MANIFEST_BYTES};

    /// Parsed top-level release manifest.
    #[derive(Debug, Clone, Deserialize)]
    pub struct ReleaseManifest {
        /// Release identifier (semver string).
        pub version: String,
        /// Git commit the artifacts were built from.
        pub commit: String,
        /// ISO-8601 build timestamp.
        pub timestamp: String,
        /// Per-contract entries in stable order.
        pub contracts: Vec<ContractEntry>,
    }

    /// Parsed entry for a single contract within the manifest.
    #[derive(Debug, Clone, Deserialize)]
    pub struct ContractEntry {
        /// Contract logical name.
        pub name: String,
        /// Workspace package the contract was compiled from.
        pub package: String,
        /// Cairo module path of the contract.
        pub module_path: String,
        /// Sierra artifact filename.
        pub sierra: String,
        /// CASM artifact filename.
        pub casm: String,
        /// ABI artifact filename.
        pub abi: String,
    }

    /// Parses the embedded manifest into a typed [`ReleaseManifest`].
    pub fn manifest() -> Result<ReleaseManifest, serde_json::Error> {
        serde_json::from_slice(MANIFEST_BYTES)
    }

    /// Returns the embedded ABI bytes parsed as a generic
    /// [`serde_json::Value`]. Use this when a fully-typed ABI walker is
    /// overkill.
    pub fn abi_value(abi: Abi) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::from_slice(abi.bytes)
    }
}

#[cfg(feature = "parsed")]
pub use parsed::{ContractEntry, ReleaseManifest, abi_value, manifest};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_field_parses() {
        assert!(!RELEASE_VERSION.is_empty(), "version must not be empty");
        // Loose check — semver always starts with a digit.
        let first = RELEASE_VERSION.as_bytes()[0];
        assert!(first.is_ascii_digit(), "version starts with digit");
    }

    #[test]
    fn commit_field_parses() {
        assert!(!RELEASE_COMMIT.is_empty(), "commit must not be empty");
    }

    #[test]
    fn every_abi_is_non_empty_json() {
        for abi in ALL {
            assert!(
                abi.bytes.len() > 2,
                "{} ABI is suspiciously short",
                abi.name
            );
            let trimmed = trim_leading_ws(abi.bytes);
            assert!(
                matches!(trimmed.first(), Some(b'[' | b'{')),
                "{} ABI does not start with a JSON delimiter",
                abi.name
            );
        }
    }

    #[cfg(feature = "parsed")]
    #[test]
    fn manifest_decodes_into_typed_view() {
        let m = manifest().expect("manifest parses");
        assert!(!m.contracts.is_empty(), "at least one contract present");
        assert_eq!(m.version, RELEASE_VERSION);
        assert_eq!(m.commit, RELEASE_COMMIT);
    }

    fn trim_leading_ws(bytes: &[u8]) -> &[u8] {
        let mut i = 0;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        &bytes[i..]
    }
}
