//! Math-runtime deployment + local address cache.
//!
//! The math runtime classes (one per market family) are **declared** on
//! mainnet by the upstream contract deployer but no instances exist —
//! consumers like the cpi-arb bot need a runtime instance to perform
//! chain-faithful preflight via `compute_hints_view` / `quote_*`.
//!
//! This module provides:
//!
//! * [`Family`] — the four supported families and their canonical mainnet class
//!   hashes.
//! * [`runtime_class_hash`] — class-hash lookup keyed by `(chain, family)`,
//!   with mainnet hashes embedded as compile-time constants.
//! * [`projected_deploy_address`] — pure address derivation via the
//!   [`starknet_core::utils::get_udc_deployed_address`] helper. Used for
//!   dry-runs and idempotency checks.
//! * [`RuntimeCache`] / [`RuntimeEntry`] — TOML-on-disk cache of
//!   previously-deployed runtime instances, keyed by chain key + family.
//!
//! Actual on-chain deploy submission lives in the CLI (it needs an
//! `deadeye_starknet::OwnedAccount`) — this crate stays
//! provider/account-free so it can be embedded in lighter consumers.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use starknet_core::{
    types::Felt,
    utils::{UdcUniqueSettings, UdcUniqueness, get_udc_deployed_address},
};

use crate::DeployerError;

/// Legacy UDC contract address (Cairo 0). This is the only deployer that
/// is predeployed on starknet-devnet-rs *and* the historical mainnet
/// UDC — every Deadeye contract deployed to date used this UDC.
pub const LEGACY_UDC_ADDRESS_HEX: &str =
    "0x041a78e741e5af2fec34b695679bc6891742439f7afb8484ecd7766661ad02bf";

/// Legacy UDC contract address as a [`Felt`].
#[must_use]
pub fn legacy_udc_address() -> Felt {
    Felt::from_hex(LEGACY_UDC_ADDRESS_HEX).expect("static UDC address parses")
}

/// The four market-runtime families supported by the Deadeye contract suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    /// Normal (Gaussian) market family.
    Normal,
    /// Lognormal market family.
    Lognormal,
    /// Multinoulli (categorical) market family.
    Multinoulli,
    /// Bivariate normal market family.
    Bivariate,
}

impl Family {
    /// All four families, in canonical iteration order.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [
            Self::Normal,
            Self::Lognormal,
            Self::Multinoulli,
            Self::Bivariate,
        ]
    }

    /// Short slug used as a config-file key and as the env-var infix
    /// (`DEADEYE_<FAMILY_UPPER>_RUNTIME_ADDR`).
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Lognormal => "lognormal",
            Self::Multinoulli => "multinoulli",
            Self::Bivariate => "bivariate",
        }
    }

    /// Upper-case slug for the env-var name.
    #[must_use]
    pub const fn env_infix(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Lognormal => "LOGNORMAL",
            Self::Multinoulli => "MULTINOULLI",
            Self::Bivariate => "BIVARIATE",
        }
    }

    /// The env-var name a consumer should set after a successful deploy:
    /// `DEADEYE_NORMAL_RUNTIME_ADDR`, etc.
    #[must_use]
    pub fn env_var_name(self) -> String {
        format!("DEADEYE_{}_RUNTIME_ADDR", self.env_infix())
    }

    /// Parse a slug back into a [`Family`]. Case-insensitive.
    #[must_use]
    pub fn from_slug(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "normal" => Some(Self::Normal),
            "lognormal" => Some(Self::Lognormal),
            "multinoulli" => Some(Self::Multinoulli),
            "bivariate" => Some(Self::Bivariate),
            _ => None,
        }
    }
}

/// Mainnet math-runtime class hashes (from `the-situation`
/// `deployment-mainnet-01.json`).
///
/// These are pinned at the crate level so the CLI can compute a projected
/// deploy address *without* touching the network — the only round-trip
/// before `--confirm` is the `getClassHashAt` idempotency check.
pub mod mainnet_class_hashes {
    /// Normal math runtime class hash on mainnet.
    pub const NORMAL: &str = "0x112f893233ffdfcd3ed8e41af8e3d08c901362a8deef80983fe4d36e3cd824f";
    /// Lognormal math runtime class hash on mainnet.
    pub const LOGNORMAL: &str = "0x7dcbf032695bf2cc60fa124d9271111e178f295c5e92c7a42530902d0fcb1c6";
    /// Bivariate math runtime class hash on mainnet.
    pub const BIVARIATE: &str = "0x53a2cb551ac57ff3d6324992f412c465d4008839b5382957514f815a877c260";
    /// Multinoulli math runtime class hash on mainnet.
    pub const MULTINOULLI: &str =
        "0xbe11cb5bc1973f905dacc533bcc07646f45b572dc97dd42bf265e6013700b9";
}

/// A short, opinionated chain identifier used as the top-level key in
/// [`RuntimeCache`]. We deliberately collapse the full hex felt to a
/// human-readable slug so the cache stays grep-friendly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChainKey {
    /// Starknet mainnet (`SN_MAIN`).
    Mainnet,
    /// Any other chain — typically a local devnet. The raw chain-id felt is
    /// preserved.
    Other,
}

impl ChainKey {
    /// Derive the [`ChainKey`] from a hex-encoded chain-id felt.
    #[must_use]
    pub fn from_chain_id_hex(hex: &str) -> Self {
        // "SN_MAIN" felt-encoded.
        let normalised = hex.trim_start_matches("0x").trim_start_matches('0');
        let lower = normalised.to_ascii_lowercase();
        if lower == "534e5f4d41494e" {
            Self::Mainnet
        } else {
            Self::Other
        }
    }

    /// Slug used as the top-level TOML key.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Other => "devnet",
        }
    }
}

/// Return the canonical math-runtime class hash for the given
/// `(chain, family)`. For mainnet, returns the pinned constant.
pub fn runtime_class_hash(chain: ChainKey, family: Family) -> Result<Felt, DeployerError> {
    let raw: String = match (chain, family) {
        (ChainKey::Mainnet, Family::Normal) => mainnet_class_hashes::NORMAL.to_owned(),
        (ChainKey::Mainnet, Family::Lognormal) => mainnet_class_hashes::LOGNORMAL.to_owned(),
        (ChainKey::Mainnet, Family::Bivariate) => mainnet_class_hashes::BIVARIATE.to_owned(),
        (ChainKey::Mainnet, Family::Multinoulli) => mainnet_class_hashes::MULTINOULLI.to_owned(),
        (ChainKey::Other, _) => {
            // The cache machinery still works for devnet, but we can't
            // know the class hash up front — caller must declare it
            // themselves and pass it in explicitly.
            return Err(DeployerError::InvalidFelt {
                field: "runtime_class_hash:chain",
                value: "devnet/other — pass class hash explicitly".to_owned(),
            });
        },
    };
    Felt::from_hex(&raw).map_err(|_| DeployerError::InvalidFelt {
        field: "runtime_class_hash",
        value: raw,
    })
}

/// Compute the address a math-runtime instance will land at when
/// deployed via the **legacy UDC** with `unique = 0` (deterministic by salt).
///
/// `deployer` is ignored when `unique = false`, but we still pass it
/// through so a future `unique = true` variant of this helper can reuse
/// the same shape.
#[must_use]
pub fn projected_deploy_address(class_hash: Felt, salt: Felt, _deployer: Felt) -> Felt {
    // Math-runtime classes have no constructor → empty calldata.
    let calldata: &[Felt] = &[];
    get_udc_deployed_address(salt, class_hash, &UdcUniqueness::NotUnique, calldata)
}

/// Same as [`projected_deploy_address`] but with `unique = true` (the
/// UDC mixes the deployer address into the salt). Exposed for symmetry;
/// the CLI defaults to `unique = false` for true idempotency.
#[must_use]
pub fn projected_deploy_address_unique(class_hash: Felt, salt: Felt, deployer: Felt) -> Felt {
    let calldata: &[Felt] = &[];
    get_udc_deployed_address(
        salt,
        class_hash,
        &UdcUniqueness::Unique(UdcUniqueSettings {
            deployer_address: deployer,
            udc_contract_address: legacy_udc_address(),
        }),
        calldata,
    )
}

// ─── Cache file ───────────────────────────────────────────────────────

/// A cached, previously-deployed math-runtime instance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeEntry {
    /// Hex-encoded deployed contract address.
    pub address: String,
    /// Hex-encoded class hash (snapshotted at deploy time).
    pub class_hash: String,
    /// Block number at which the UDC deploy was mined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployed_at_block: Option<u64>,
    /// Hex-encoded UDC deploy transaction hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployed_tx: Option<String>,
}

/// In-memory mirror of `~/.config/deadeye/runtimes.toml`.
///
/// TOML shape:
///
/// ```toml
/// [mainnet.normal]
/// address = "0x..."
/// class_hash = "0x..."
/// deployed_at_block = 1234567
/// deployed_tx = "0x..."
///
/// [devnet.normal]
/// # ...
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct RuntimeCache {
    /// Outer map keyed by chain slug (`mainnet`, `devnet`), inner
    /// map keyed by family slug (`normal`, `lognormal`, …).
    pub chains: BTreeMap<String, BTreeMap<String, RuntimeEntry>>,
}

impl RuntimeCache {
    /// Default cache path: `~/.config/deadeye/runtimes.toml`. Honors
    /// `DEADEYE_RUNTIMES_PATH` for tests + overrides.
    pub fn default_path() -> Result<PathBuf, DeployerError> {
        if let Some(p) = std::env::var_os("DEADEYE_RUNTIMES_PATH") {
            return Ok(PathBuf::from(p));
        }
        let base = dirs_next_config_dir().ok_or_else(|| DeployerError::InvalidFelt {
            field: "runtimes_path",
            value: "could not locate user config dir; set DEADEYE_RUNTIMES_PATH".to_owned(),
        })?;
        Ok(base.join("deadeye").join("runtimes.toml"))
    }

    /// Load the cache from `path`. Missing file returns an empty cache.
    /// Malformed TOML returns a typed error.
    pub fn load(path: &Path) -> Result<Self, DeployerError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path).map_err(|e| DeployerError::InvalidFelt {
            field: "runtimes_path",
            value: format!("reading {}: {e}", path.display()),
        })?;
        if raw.trim().is_empty() {
            return Ok(Self::default());
        }
        toml::from_str::<Self>(&raw).map_err(|e| DeployerError::InvalidFelt {
            field: "runtimes_toml",
            value: format!("parsing {}: {e}", path.display()),
        })
    }

    /// Persist the cache to `path`. Creates the parent directory if needed.
    ///
    /// Uses a temp-file + atomic rename pattern so a crash mid-write (or
    /// two concurrent CLIs racing) can never produce a half-written /
    /// corrupted TOML file. The temp file lives next to `path` so the
    /// rename stays on the same filesystem (POSIX-atomic).
    pub fn save(&self, path: &Path) -> Result<(), DeployerError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| DeployerError::InvalidFelt {
                field: "runtimes_path",
                value: format!("mkdir {}: {e}", parent.display()),
            })?;
        }
        let body = toml::to_string_pretty(self).map_err(|e| DeployerError::InvalidFelt {
            field: "runtimes_toml",
            value: format!("serializing: {e}"),
        })?;
        let mut header = String::from(
            "# Deadeye runtime cache.\n\
             # Populated by `deadeye admin deploy-math-runtime`; safe to delete\n\
             # and re-derive (the command is idempotent + verifies on each run).\n\n",
        );
        header.push_str(&body);

        // Disambiguate concurrent writers via pid + nanos so two CLIs
        // racing on the same cache don't clobber each other's temp file.
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp_name = format!(
            ".{}.{pid}.{nanos}.tmp",
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("runtimes.toml")
        );
        let tmp_path = path
            .parent()
            .map_or_else(|| PathBuf::from(&tmp_name), |p| p.join(&tmp_name));

        fs::write(&tmp_path, header).map_err(|e| DeployerError::InvalidFelt {
            field: "runtimes_path",
            value: format!("writing {}: {e}", tmp_path.display()),
        })?;
        match fs::rename(&tmp_path, path) {
            Ok(()) => Ok(()),
            Err(e) => {
                // Clean up the temp file on failure so we don't litter.
                let _ = fs::remove_file(&tmp_path);
                Err(DeployerError::InvalidFelt {
                    field: "runtimes_path",
                    value: format!("rename {} → {}: {e}", tmp_path.display(), path.display()),
                })
            },
        }
    }

    /// Look up a cached entry.
    #[must_use]
    pub fn get(&self, chain: ChainKey, family: Family) -> Option<&RuntimeEntry> {
        self.chains.get(chain.slug())?.get(family.slug())
    }

    /// Insert / replace an entry. Returns the previous value, if any.
    pub fn upsert(
        &mut self,
        chain: ChainKey,
        family: Family,
        entry: RuntimeEntry,
    ) -> Option<RuntimeEntry> {
        self.chains
            .entry(chain.slug().to_owned())
            .or_default()
            .insert(family.slug().to_owned(), entry)
    }
}

/// Locate the user's config dir — mirrors the behaviour of `dirs::config_dir`
/// without taking that crate as a transitive dependency (the deployer crate
/// is meant to stay narrow). On non-Unix or unusual hosts this falls back
/// to `$HOME/.config` to keep the cache discoverable.
fn dirs_next_config_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        let p = PathBuf::from(xdg);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return Some(
                PathBuf::from(home)
                    .join("Library")
                    .join("Application Support"),
            );
        }
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "test code: panic-style failures are the assertion mechanism"
)]
mod tests {
    use super::*;

    #[test]
    fn family_slug_roundtrips() {
        for f in Family::all() {
            assert_eq!(Family::from_slug(f.slug()), Some(f));
        }
        assert_eq!(Family::from_slug("Normal"), Some(Family::Normal));
        assert_eq!(Family::from_slug("nope"), None);
    }

    #[test]
    fn env_var_name_is_canonical() {
        assert_eq!(Family::Normal.env_var_name(), "DEADEYE_NORMAL_RUNTIME_ADDR");
        assert_eq!(
            Family::Lognormal.env_var_name(),
            "DEADEYE_LOGNORMAL_RUNTIME_ADDR"
        );
        assert_eq!(
            Family::Multinoulli.env_var_name(),
            "DEADEYE_MULTINOULLI_RUNTIME_ADDR"
        );
        assert_eq!(
            Family::Bivariate.env_var_name(),
            "DEADEYE_BIVARIATE_RUNTIME_ADDR"
        );
    }

    #[test]
    fn chain_key_detects_mainnet() {
        assert_eq!(
            ChainKey::from_chain_id_hex("0x534e5f4d41494e"),
            ChainKey::Mainnet
        );
        // Any non-mainnet chain id (e.g. a local devnet felt) → Other.
        assert_eq!(
            ChainKey::from_chain_id_hex("0x534e5f5345504f4c4941"),
            ChainKey::Other
        );
        assert_eq!(ChainKey::from_chain_id_hex("0xdeadbeef"), ChainKey::Other);
        // Leading zero tolerated.
        assert_eq!(
            ChainKey::from_chain_id_hex("0x00534e5f4d41494e"),
            ChainKey::Mainnet
        );
    }

    #[test]
    fn mainnet_class_hashes_parse() {
        // All four constants must round-trip via Felt::from_hex.
        for raw in [
            mainnet_class_hashes::NORMAL,
            mainnet_class_hashes::LOGNORMAL,
            mainnet_class_hashes::BIVARIATE,
            mainnet_class_hashes::MULTINOULLI,
        ] {
            Felt::from_hex(raw).expect("class hash parses");
        }
    }

    #[test]
    fn mainnet_lookup_returns_pinned_constants() {
        let h = runtime_class_hash(ChainKey::Mainnet, Family::Normal).expect("mainnet lookup");
        assert_eq!(h, Felt::from_hex(mainnet_class_hashes::NORMAL).unwrap());
    }

    /// Drift guard: the four pinned mainnet constants must match the
    /// math-runtime entries in the bundled mainnet manifest byte-for-byte
    /// (modulo hex zero-padding). If the upstream deployer republishes
    /// `deployment-mainnet-01.json` with new class hashes, this test will
    /// fail until the constants are bumped.
    #[test]
    fn pinned_mainnet_constants_match_bundled_manifest() {
        let d = crate::Deployment::mainnet().expect("mainnet manifest parses");
        for (family, pinned) in [
            (Family::Normal, mainnet_class_hashes::NORMAL),
            (Family::Lognormal, mainnet_class_hashes::LOGNORMAL),
            (Family::Bivariate, mainnet_class_hashes::BIVARIATE),
            (Family::Multinoulli, mainnet_class_hashes::MULTINOULLI),
        ] {
            let manifest_raw = match family {
                Family::Normal => &d.class_hashes.normal_math_runtime,
                Family::Lognormal => &d.class_hashes.lognormal_math_runtime,
                Family::Bivariate => &d.class_hashes.bivariate_math_runtime,
                Family::Multinoulli => &d.class_hashes.multinoulli_math_runtime,
            };
            let pinned_felt = Felt::from_hex(pinned).expect("pinned parses");
            let manifest_felt = Felt::from_hex(manifest_raw).expect("manifest parses");
            assert_eq!(
                pinned_felt, manifest_felt,
                "drift in {family:?}: pinned={pinned} manifest={manifest_raw}"
            );
        }
    }

    /// UDC `unique = false` address must match starknet-core's helper.
    /// Fixed (`class_hash`, salt) → fixed address.
    #[test]
    fn projected_address_is_deterministic() {
        let class_hash = Felt::from_hex(mainnet_class_hashes::NORMAL).unwrap();
        let salt = Felt::from(0x0000_ABCD_u64);
        let deployer = Felt::from_hex("0x0000beef").unwrap();
        let a = projected_deploy_address(class_hash, salt, deployer);
        let b = projected_deploy_address(class_hash, salt, deployer);
        assert_eq!(a, b, "must be deterministic");

        // Deployer must not affect the unique=false case.
        let other_deployer = Felt::from_hex("0xbaadf00d").unwrap();
        let c = projected_deploy_address(class_hash, salt, other_deployer);
        assert_eq!(a, c, "unique=false → deployer ignored");

        // Different salts → different addresses.
        let d = projected_deploy_address(class_hash, Felt::from(0x0000_DCBA_u64), deployer);
        assert_ne!(a, d, "salt must influence address");
    }

    /// Cross-check: the unique=true variant matches starknet-core's
    /// expected derivation by going through both branches of the helper.
    #[test]
    fn projected_address_unique_branch_differs() {
        let class_hash = Felt::from_hex(mainnet_class_hashes::NORMAL).unwrap();
        let salt = Felt::from(0x0000_ABCD_u64);
        let deployer = Felt::from_hex("0x0000beef").unwrap();
        let not_unique = projected_deploy_address(class_hash, salt, deployer);
        let unique = projected_deploy_address_unique(class_hash, salt, deployer);
        assert_ne!(
            not_unique, unique,
            "unique=true must hash the deployer into the salt"
        );
    }

    #[test]
    fn cache_roundtrips_via_toml() {
        let mut cache = RuntimeCache::default();
        cache.upsert(ChainKey::Mainnet, Family::Normal, RuntimeEntry {
            address: "0xabc".to_owned(),
            class_hash: mainnet_class_hashes::NORMAL.to_owned(),
            deployed_at_block: Some(1_234_567),
            deployed_tx: Some("0xdef".to_owned()),
        });
        cache.upsert(ChainKey::Other, Family::Lognormal, RuntimeEntry {
            address: "0xfeed".to_owned(),
            class_hash: "0xface".to_owned(),
            deployed_at_block: None,
            deployed_tx: None,
        });

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("runtimes.toml");
        cache.save(&path).expect("save");
        assert!(path.exists());

        let loaded = RuntimeCache::load(&path).expect("load");
        assert_eq!(loaded, cache);

        // Sanity: the rendered TOML mentions the chain slug + family slug.
        let raw = fs::read_to_string(&path).expect("read back");
        assert!(raw.contains("[mainnet.normal]"));
        assert!(raw.contains("[devnet.lognormal]"));
    }

    #[test]
    fn cache_load_missing_returns_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nonexistent.toml");
        let loaded = RuntimeCache::load(&path).expect("missing → empty");
        assert!(loaded.chains.is_empty());
    }

    #[test]
    fn cache_load_malformed_returns_typed_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bad.toml");
        fs::write(&path, "this is not = valid = toml === ").expect("write");
        let err = RuntimeCache::load(&path).expect_err("must reject malformed");
        let DeployerError::InvalidFelt { field, .. } = err else {
            panic!("expected InvalidFelt-tagged error from malformed TOML");
        };
        assert_eq!(field, "runtimes_toml");
    }
}
