//! Cardano key derivation: the Icarus master key, then BIP32-Ed25519.
//!
//! Cardano does not use the BIP-39 *seed*. The Icarus scheme (CIP-3) runs
//! PBKDF2-HMAC-SHA512 with the passphrase as the **password** and the mnemonic
//! **entropy** as the **salt**, 4096 iterations, to produce 96 bytes: a
//! 64-byte extended ed25519 scalar plus a 32-byte chain code. That argument
//! order reads backwards, and it is nonetheless what every Cardano wallet
//! implements — so it is what interoperates.
//!
//! The scalar is then clamped for ed25519, and bit 5 of byte 31 is forced
//! clear ("force3rd") so the scalar stays small enough that BIP32-Ed25519's
//! additive `8 * Z_L + k_par` step can never overflow.
//!
//! Paths follow CIP-1852: `m/1852'/1815'/account'/role/index`, where role 0 is
//! external (receive), 1 is internal (change) and 2 is staking. A wallet needs
//! two keys to form one address: the payment key at `index`, and the account's
//! single staking key.

use blake2::digest::{Update, VariableOutput};
use ed25519_bip32::{DerivationScheme, XPrv, XPub};
use hmac::Hmac;
use sha2::Sha512;

use crate::error::{self, Result};

use super::address::{Address, CardanoNetwork};

/// Everything at or above this is a hardened index.
const HARDENED: u32 = 0x8000_0000;

/// CIP-1852 purpose.
pub const PURPOSE: u32 = 1852;
/// SLIP-44 coin type for ADA.
pub const COIN_TYPE: u32 = 1815;
/// Role index for external (receive) addresses.
pub const ROLE_EXTERNAL: u32 = 0;
/// Role index for the staking key.
pub const ROLE_STAKING: u32 = 2;

const PBKDF2_ITERATIONS: u32 = 4096;
const XPRV_SIZE: usize = 96;

/// The CIP-1852 path for an address index, in the default account.
pub fn path(index: u32) -> String {
    format!("m/{PURPOSE}'/{COIN_TYPE}'/0'/{ROLE_EXTERNAL}/{index}")
}

/// Derive the Icarus root extended private key from BIP-39 entropy.
pub fn root_from_entropy(entropy: &[u8], passphrase: &[u8]) -> Result<XPrv> {
    let mut out = [0u8; XPRV_SIZE];
    pbkdf2::pbkdf2::<Hmac<Sha512>>(passphrase, entropy, PBKDF2_ITERATIONS, &mut out)
        .map_err(|e| error::internal(format!("PBKDF2 failed: {e}")))?;
    out[0] &= 0b1111_1000;
    out[31] &= 0b0001_1111;
    out[31] |= 0b0100_0000;
    XPrv::from_bytes_verified(out)
        .map_err(|e| error::internal(format!("the Icarus root key is invalid: {e:?}")))
}

/// One Cardano account: the payment key at an index plus the staking key.
pub struct CardanoAccount {
    pub payment: XPrv,
    pub stake: XPrv,
    pub path: Option<String>,
}

/// Redacted on purpose, like every other key type in the wallet.
impl std::fmt::Debug for CardanoAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CardanoAccount")
            .field("payment_key_hash", &hex::encode(self.payment_key_hash()))
            .field("payment", &"<redacted>")
            .field("stake", &"<redacted>")
            .finish()
    }
}

impl CardanoAccount {
    /// Derive `m/1852'/1815'/0'/0/<index>` plus the account's staking key.
    pub fn from_entropy(entropy: &[u8], passphrase: &str, index: u32) -> Result<Self> {
        let root = root_from_entropy(entropy, passphrase.as_bytes())?;
        let account = root
            .derive(DerivationScheme::V2, PURPOSE + HARDENED)
            .derive(DerivationScheme::V2, COIN_TYPE + HARDENED)
            .derive(DerivationScheme::V2, HARDENED);
        Ok(CardanoAccount {
            payment: account
                .derive(DerivationScheme::V2, ROLE_EXTERNAL)
                .derive(DerivationScheme::V2, index),
            stake: account
                .derive(DerivationScheme::V2, ROLE_STAKING)
                .derive(DerivationScheme::V2, 0),
            path: Some(path(index)),
        })
    }

    /// Rebuild from the stored secret: the two 96-byte extended keys as hex.
    ///
    /// Both are kept because an address needs both, and the staking key is not
    /// derivable from the payment key — without it, a restored account could
    /// only produce enterprise addresses and would report a different address
    /// than the one it was saved as.
    pub fn from_secret(secret: &str) -> Result<Self> {
        let body = secret.trim();
        let body = body.strip_prefix("0x").unwrap_or(body);
        let bytes = hex::decode(body)
            .map_err(|e| error::invalid_private_key(format!("not valid hexadecimal: {e}")))?;
        if bytes.len() != XPRV_SIZE * 2 {
            return Err(error::invalid_private_key(format!(
                "a Cardano secret is {} hex characters (a payment and a staking \
                 extended key), got {}",
                XPRV_SIZE * 4,
                body.len()
            )));
        }
        let load = |slice: &[u8]| -> Result<XPrv> {
            let array: [u8; XPRV_SIZE] = slice.try_into().expect("checked length");
            XPrv::from_bytes_verified(array)
                .map_err(|e| error::invalid_private_key(format!("not a valid extended key: {e:?}")))
        };
        Ok(CardanoAccount {
            payment: load(&bytes[..XPRV_SIZE])?,
            stake: load(&bytes[XPRV_SIZE..])?,
            path: None,
        })
    }

    /// The stored form: payment key then staking key, hex encoded.
    pub fn secret_hex(&self) -> String {
        format!(
            "{}{}",
            hex::encode(self.payment.as_ref()),
            hex::encode(self.stake.as_ref())
        )
    }

    pub fn payment_public_key(&self) -> [u8; 32] {
        raw_public_key(&self.payment.public())
    }

    pub fn stake_public_key(&self) -> [u8; 32] {
        raw_public_key(&self.stake.public())
    }

    pub fn payment_key_hash(&self) -> [u8; 28] {
        key_hash(&self.payment_public_key())
    }

    pub fn stake_key_hash(&self) -> [u8; 28] {
        key_hash(&self.stake_public_key())
    }

    /// The base address (payment plus staking) — what a wallet shows.
    pub fn base_address(&self, network: CardanoNetwork) -> Address {
        Address::base(network, self.payment_key_hash(), self.stake_key_hash())
    }

    /// The reward address, where staking rewards accrue.
    pub fn reward_address(&self, network: CardanoNetwork) -> Address {
        Address::reward(network, self.stake_key_hash())
    }

    /// Sign with the payment key.
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        let signature: ed25519_bip32::Signature<Vec<u8>> = self.payment.sign(message);
        let mut out = [0u8; 64];
        out.copy_from_slice(signature.to_bytes());
        out
    }
}

/// Verify a signature by a raw ed25519 public key.
pub fn verify(public_key: &[u8; 32], message: &[u8], signature: &[u8]) -> Result<bool> {
    let Ok(verifying_key) = ed25519_dalek::VerifyingKey::from_bytes(public_key) else {
        return Ok(false);
    };
    let signature: [u8; 64] = signature.try_into().map_err(|_| {
        error::usage(format!(
            "an ed25519 signature is 64 bytes, got {}",
            signature.len()
        ))
    })?;
    Ok(verifying_key
        .verify_strict(message, &ed25519_dalek::Signature::from_bytes(&signature))
        .is_ok())
}

/// Strip the chain code off an extended public key, leaving the ed25519 key.
pub fn raw_public_key(xpub: &XPub) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(&xpub.public_key_slice()[..32]);
    out
}

/// blake2b-224 — Cardano's key hash.
pub fn key_hash(public_key: &[u8; 32]) -> [u8; 28] {
    let mut hasher = blake2::Blake2bVar::new(28).expect("28 is a valid blake2b length");
    hasher.update(public_key);
    let mut out = [0u8; 28];
    hasher
        .finalize_variable(&mut out)
        .expect("the buffer is 28 bytes");
    out
}

/// blake2b-256 — used for transaction ids.
pub fn hash32(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = blake2::Blake2bVar::new(32).expect("32 is a valid blake2b length");
    hasher.update(bytes);
    let mut out = [0u8; 32];
    hasher
        .finalize_variable(&mut out)
        .expect("the buffer is 32 bytes");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::Seed;

    const PHRASE: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn vectors() -> serde_json::Value {
        serde_json::from_str(include_str!("../../../../../testvectors/multichain.json")).unwrap()
    }

    fn account(index: u32) -> CardanoAccount {
        let entropy = Seed::new(PHRASE, "").unwrap().entropy().unwrap();
        CardanoAccount::from_entropy(&entropy, "", index).unwrap()
    }

    /// Against `cardano-serialization-lib`. This is the test that pins the
    /// backwards-looking PBKDF2 argument order: swap password and salt and
    /// every address below changes.
    #[test]
    fn addresses_match_the_official_sdk() {
        let vectors = vectors();
        for (index, expected) in vectors["cardano"]["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            let account = account(index as u32);
            assert_eq!(
                account.path.as_deref().unwrap(),
                expected["path"].as_str().unwrap()
            );
            assert_eq!(
                account.base_address(CardanoNetwork::Testnet).to_bech32(),
                expected["base_addr_testnet"].as_str().unwrap(),
                "testnet base address at index {index}"
            );
            assert_eq!(
                account.base_address(CardanoNetwork::Mainnet).to_bech32(),
                expected["base_addr_mainnet"].as_str().unwrap(),
                "mainnet base address at index {index}"
            );
            assert_eq!(
                hex::encode(account.payment_key_hash()),
                expected["payment_keyhash_hex"].as_str().unwrap()
            );
        }
    }

    #[test]
    fn the_reward_address_matches_the_official_sdk() {
        let vectors = vectors();
        assert_eq!(
            account(0)
                .reward_address(CardanoNetwork::Testnet)
                .to_bech32(),
            vectors["cardano"]["reward_addr_testnet"].as_str().unwrap()
        );
        assert_eq!(
            hex::encode(account(0).stake_key_hash()),
            vectors["cardano"]["stake_keyhash_hex"].as_str().unwrap()
        );
    }

    #[test]
    fn the_root_key_matches_the_official_sdk() {
        let entropy = Seed::new(PHRASE, "").unwrap().entropy().unwrap();
        let root = root_from_entropy(&entropy, b"").unwrap();
        assert_eq!(
            hex::encode(root.as_ref()),
            vectors()["cardano"]["root_xprv_hex"].as_str().unwrap()
        );
    }

    #[test]
    fn every_index_shares_one_staking_key() {
        // A wallet has many payment addresses and one stake key; if this
        // drifted, each address would report a different reward address.
        let first = account(0);
        for index in 1..4 {
            assert_eq!(account(index).stake_key_hash(), first.stake_key_hash());
            assert_ne!(account(index).payment_key_hash(), first.payment_key_hash());
        }
    }

    #[test]
    fn the_stored_secret_round_trips_with_both_keys() {
        let original = account(2);
        let restored = CardanoAccount::from_secret(&original.secret_hex()).unwrap();
        assert_eq!(
            restored.base_address(CardanoNetwork::Testnet),
            original.base_address(CardanoNetwork::Testnet)
        );
        // The staking half survived, so the reward address is still right.
        assert_eq!(
            restored.reward_address(CardanoNetwork::Testnet),
            original.reward_address(CardanoNetwork::Testnet)
        );
        assert!(restored.path.is_none());
    }

    #[test]
    fn a_secret_of_the_wrong_length_says_what_was_expected() {
        let err = CardanoAccount::from_secret(&hex::encode([0u8; 96])).unwrap_err();
        assert_eq!(err.code, error::Code::InvalidPrivateKey);
        assert!(err.message.contains("384"), "{}", err.message);
        assert!(CardanoAccount::from_secret("nonsense").is_err());
        assert!(CardanoAccount::from_secret("").is_err());
    }

    #[test]
    fn a_passphrase_yields_a_different_wallet() {
        let entropy = Seed::new(PHRASE, "").unwrap().entropy().unwrap();
        let plain = CardanoAccount::from_entropy(&entropy, "", 0).unwrap();
        let salted = CardanoAccount::from_entropy(&entropy, "hunter2", 0).unwrap();
        assert_ne!(plain.payment_key_hash(), salted.payment_key_hash());
    }

    #[test]
    fn signing_verifies_against_the_payment_key_and_only_that_message() {
        let account = account(0);
        let signature = account.sign(b"hello causewaybay");
        assert!(verify(
            &account.payment_public_key(),
            b"hello causewaybay",
            &signature
        )
        .unwrap());
        assert!(!verify(&account.payment_public_key(), b"tampered", &signature).unwrap());
        // The staking key did not make this signature.
        assert!(!verify(
            &account.stake_public_key(),
            b"hello causewaybay",
            &signature
        )
        .unwrap());
    }

    #[test]
    fn key_hashes_are_the_advertised_widths() {
        assert_eq!(key_hash(&[0u8; 32]).len(), 28);
        assert_eq!(hash32(b"anything").len(), 32);
    }

    #[test]
    fn an_accounts_debug_rendering_leaks_nothing() {
        let account = account(0);
        let rendered = format!("{account:?}");
        assert!(!rendered.contains(&account.secret_hex()));
        assert!(rendered.contains("redacted"));
    }
}
