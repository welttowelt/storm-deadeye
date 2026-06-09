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

/// Recover a wallet from an existing BIP-39 phrase.
pub(crate) fn import(phrase: &str, class_hash: Felt) -> Result<Wallet> {
    let phrase = phrase.split_whitespace().collect::<Vec<_>>().join(" ");
    let word_count = phrase.split(' ').count();
    if !matches!(word_count, 12 | 15 | 18 | 21 | 24) {
        bail!("expected a 12/15/18/21/24-word phrase, got {word_count} words");
    }
    let mnemonic = Mnemonic::parse_normalized(&phrase).context("invalid BIP-39 recovery phrase")?;
    Ok(from_mnemonic(mnemonic, class_hash))
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

/// Derive every wallet field from a parsed mnemonic.
fn from_mnemonic(mnemonic: Mnemonic, class_hash: Felt) -> Wallet {
    let seed = mnemonic.to_seed("");
    let digest = Sha256::digest(seed);
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
}
