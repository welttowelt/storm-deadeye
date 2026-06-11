//! Local wallet derivation for `deadeye onboard`.
//!
//! A wallet is a BIP-39 mnemonic plus the Starknet account it controls.
//! Derivation is deliberately simple and **self-consistent** — the same
//! mnemonic always reproduces the same key, address, and account — so an
//! agent can recover its wallet on every run:
//!
//! ```text
//! mnemonic ── to_seed("") ──▶ 64-byte seed
//!          ── SHA-256 ───────▶ 32 bytes
//!          ── Felt::from_bytes_be (reduce into the STARK field) ──▶ secret scalar
//!          ── stark curve ───▶ public key
//!          ── OZ account ────▶ counterfactual address
//! ```
//!
//! ## HD derivation (issue #37) — `deadeye/hd/v1`
//!
//! One mnemonic can back a whole fleet. Account `i` derives as:
//!
//! ```text
//! index 0   : sk = Felt(SHA-256(seed))                                  (legacy — unchanged)
//! index i>0 : sk = Felt(SHA-256(seed ‖ "deadeye/hd/v1/" ‖ decimal(i)))
//! ```
//!
//! where `seed = BIP-39 to_seed(passphrase = "")`, `decimal(i)` is the ASCII
//! base-10 rendering of the index, and `Felt::from_bytes_be` reduces modulo
//! the STARK prime. The scheme is deliberately spelled out here so any
//! implementation can reproduce the same addresses from the same phrase.
//!
//! This is **not** Argent/Braavos HD derivation (no EIP-2645 grinding), so
//! a mnemonic minted elsewhere will derive a *different* deadeye address.
//! Importing a deadeye-generated mnemonic round-trips exactly, which is all
//! the recover-every-run flow needs.

use anyhow::{Context as _, Result, bail};
use bip39::Mnemonic;
use sha2::{Digest as _, Sha256};
use starknet_core::{types::Felt, utils::get_contract_address};
use starknet_signers::SigningKey;

/// OpenZeppelin account class hash used by default when deploying.
///
/// Must be **declared** on the target network or the deploy will fail; the
/// onboard flow verifies this on-chain before spending gas. Override with
/// `--account-class-hash` (or the profile's `account_class_hash`) to deploy
/// a different account implementation (Argent, Braavos, a newer OZ, …).
pub(crate) const DEFAULT_OZ_ACCOUNT_CLASS_HASH: &str =
    "0x061dac032f228abef9c6626f995015233097ae253a7f72d68552db02f2971b8f";

/// A derived wallet: mnemonic + secret/public key + account address.
#[derive(Debug, Clone)]
pub(crate) struct Wallet {
    pub(crate) mnemonic: String,
    pub(crate) private_key: Felt,
    pub(crate) public_key: Felt,
    pub(crate) address: Felt,
    pub(crate) class_hash: Felt,
}

/// Generate a fresh 24-word wallet for `class_hash`.
pub(crate) fn generate(class_hash: Felt) -> Result<Wallet> {
    use rand::RngCore as _;
    let mut entropy = [0_u8; 32]; // 256 bits → 24 words
    rand::rngs::OsRng.fill_bytes(&mut entropy);
    let mnemonic = Mnemonic::from_entropy(&entropy).context("building mnemonic from entropy")?;
    Ok(from_mnemonic(mnemonic, class_hash))
}

/// Recover a wallet from an existing BIP-39 phrase (HD index 0).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "index-0 convenience kept for API symmetry; onboarding now routes through import_at_index"
    )
)]
pub(crate) fn import(phrase: &str, class_hash: Felt) -> Result<Wallet> {
    import_at_index(phrase, class_hash, 0)
}

/// Recover the wallet at HD `index` from an existing BIP-39 phrase — the
/// `deadeye/hd/v1` scheme (module docs): one seed, many independent
/// accounts. Index 0 is the legacy single-account derivation.
pub(crate) fn import_at_index(phrase: &str, class_hash: Felt, index: u32) -> Result<Wallet> {
    let phrase = phrase.split_whitespace().collect::<Vec<_>>().join(" ");
    let word_count = phrase.split(' ').count();
    if !matches!(word_count, 12 | 15 | 18 | 21 | 24) {
        bail!("expected a 12/15/18/21/24-word phrase, got {word_count} words");
    }
    let mnemonic = Mnemonic::parse_normalized(&phrase).context("invalid BIP-39 recovery phrase")?;
    Ok(from_mnemonic_at(mnemonic, class_hash, index))
}

/// Rebuild a wallet from a stored private key (no mnemonic). Used to deploy or
/// operate an already-onboarded account whose key is saved in the profile.
pub(crate) fn from_private_key(private_key: Felt, class_hash: Felt) -> Wallet {
    let signing_key = SigningKey::from_secret_scalar(private_key);
    let public_key = signing_key.verifying_key().scalar();
    let address = oz_account_address(public_key, class_hash);
    Wallet {
        mnemonic: String::new(),
        private_key,
        public_key,
        address,
        class_hash,
    }
}

/// Derive every wallet field from a parsed mnemonic (HD index 0).
fn from_mnemonic(mnemonic: Mnemonic, class_hash: Felt) -> Wallet {
    from_mnemonic_at(mnemonic, class_hash, 0)
}

/// Derive the wallet at HD `index` — the `deadeye/hd/v1` scheme.
fn from_mnemonic_at(mnemonic: Mnemonic, class_hash: Felt, index: u32) -> Wallet {
    let seed = mnemonic.to_seed("");
    let digest = if index == 0 {
        // Legacy single-account derivation — existing wallets keep their
        // address.
        Sha256::digest(seed)
    } else {
        let mut hasher = Sha256::new();
        hasher.update(seed);
        hasher.update(b"deadeye/hd/v1/");
        hasher.update(index.to_string().as_bytes());
        hasher.finalize()
    };
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(&digest);
    // Felt::from_bytes_be reduces modulo the STARK field prime, so the
    // result is always a valid secret scalar.
    let private_key = Felt::from_bytes_be(&bytes);
    let signing_key = SigningKey::from_secret_scalar(private_key);
    let public_key = signing_key.verifying_key().scalar();
    let address = oz_account_address(public_key, class_hash);
    Wallet {
        mnemonic: mnemonic.to_string(),
        private_key,
        public_key,
        address,
        class_hash,
    }
}

/// Counterfactual address of an OpenZeppelin account for `public_key`.
///
/// Matches `OpenZeppelinAccountFactory`: `salt = public_key`,
/// `constructor_calldata = [public_key]`, `deployer = 0`.
pub(crate) fn oz_account_address(public_key: Felt, class_hash: Felt) -> Felt {
    get_contract_address(public_key, class_hash, &[public_key], Felt::ZERO)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "tests panic on construction failure")]
mod tests {
    use super::*;

    fn class_hash() -> Felt {
        Felt::from_hex(DEFAULT_OZ_ACCOUNT_CLASS_HASH).unwrap()
    }

    #[test]
    fn generate_is_deterministic_under_reimport() {
        let w = generate(class_hash()).unwrap();
        let again = import(&w.mnemonic, class_hash()).unwrap();
        assert_eq!(w.private_key, again.private_key);
        assert_eq!(w.public_key, again.public_key);
        assert_eq!(w.address, again.address);
    }

    #[test]
    fn known_phrase_round_trips_to_stable_address() {
        // A fixed phrase must always derive the same address (recovery).
        let phrase = "legal winner thank year wave sausage worth useful legal winner thank yellow";
        let a = import(phrase, class_hash()).unwrap();
        let b = import(phrase, class_hash()).unwrap();
        assert_eq!(a.address, b.address);
        assert_ne!(a.private_key, Felt::ZERO);
    }

    #[test]
    fn rejects_bad_phrase() {
        import("not a real mnemonic at all", class_hash()).unwrap_err();
    }

    #[test]
    fn hd_index_zero_is_the_legacy_derivation() {
        let phrase = "legal winner thank year wave sausage worth useful legal winner thank yellow";
        let legacy = import(phrase, class_hash()).unwrap();
        let indexed = import_at_index(phrase, class_hash(), 0).unwrap();
        assert_eq!(legacy.address, indexed.address);
        assert_eq!(legacy.private_key, indexed.private_key);
    }

    #[test]
    fn hd_derivation_is_deterministic_per_index() {
        let phrase = "legal winner thank year wave sausage worth useful legal winner thank yellow";
        for index in [1_u32, 7, 42] {
            let a = import_at_index(phrase, class_hash(), index).unwrap();
            let b = import_at_index(phrase, class_hash(), index).unwrap();
            assert_eq!(a.address, b.address, "index {index} must be stable");
            assert_eq!(a.private_key, b.private_key);
        }
    }

    #[test]
    fn hd_indices_derive_distinct_independent_accounts() {
        let phrase = "legal winner thank year wave sausage worth useful legal winner thank yellow";
        let mut seen = std::collections::HashSet::new();
        for index in 0..20_u32 {
            let w = import_at_index(phrase, class_hash(), index).unwrap();
            assert!(seen.insert(w.address), "index {index} collided");
            assert_ne!(w.private_key, Felt::ZERO);
        }
    }

    #[test]
    fn hd_indices_one_and_ten_do_not_collide_on_ascii_prefix() {
        // decimal(1) = "1" and decimal(10) = "10" hash differently — the
        // separator + full-string hash makes the encoding unambiguous.
        let phrase = "legal winner thank year wave sausage worth useful legal winner thank yellow";
        let one = import_at_index(phrase, class_hash(), 1).unwrap();
        let ten = import_at_index(phrase, class_hash(), 10).unwrap();
        assert_ne!(one.address, ten.address);
    }
}
