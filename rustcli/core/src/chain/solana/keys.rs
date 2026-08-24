//! Solana keys: SLIP-0010 over ed25519.
//!
//! Ed25519 cannot derive a child *public* key from a parent public key, so
//! SLIP-0010 defines hardened derivation only. Every index must be hardened,
//! and a path like `m/44'/501'/0'/0` — one apostrophe short — is rejected
//! rather than quietly hardened, because it is a real mistake that otherwise
//! yields a plausible, wrong, unfunded address.
//!
//! The derived 32 bytes are the ed25519 seed directly, and the address is the
//! base58 of the 32-byte public key: no hashing, no version byte, no checksum.
//! The key *is* the address, which is why a typo in a Solana address is caught
//! only by base58's own alphabet and length.

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use hmac::{Hmac, Mac};
use sha2::Sha512;

use crate::error::{self, Result};

/// Everything at or above this is a hardened index.
const HARDENED: u32 = 0x8000_0000;

/// The prefix every Solana account path shares.
pub const PATH_PREFIX: &str = "m/44'/501'";

/// The canonical path for an address index, as the Solana CLI and Phantom use it.
pub fn path(index: u32) -> String {
    format!("{PATH_PREFIX}/{index}'/0'")
}

/// A SLIP-0010 node: 32 bytes of key material plus a 32-byte chain code.
#[derive(Debug)]
struct Node {
    key: [u8; 32],
    chain_code: [u8; 32],
}

impl Node {
    /// The master node: `HMAC-SHA512("ed25519 seed", seed)`, split in half.
    fn master(seed: &[u8]) -> Self {
        let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(b"ed25519 seed")
            .expect("HMAC accepts a key of any length");
        mac.update(seed);
        Node::split(&mac.finalize().into_bytes())
    }

    /// One hardened child: `HMAC-SHA512(c_par, 0x00 || k_par || ser32(i))`.
    fn derive_child(&self, index: u32) -> Result<Self> {
        if index < HARDENED {
            return Err(error::usage(format!(
                "Solana derives over ed25519, which supports hardened indices \
                 only; index {index} is not hardened"
            )));
        }
        let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(&self.chain_code)
            .expect("a 32-byte HMAC key is always valid");
        mac.update(&[0u8]);
        mac.update(&self.key);
        mac.update(&index.to_be_bytes());
        Ok(Node::split(&mac.finalize().into_bytes()))
    }

    fn split(bytes: &[u8]) -> Self {
        let mut key = [0u8; 32];
        let mut chain_code = [0u8; 32];
        key.copy_from_slice(&bytes[..32]);
        chain_code.copy_from_slice(&bytes[32..]);
        Node { key, chain_code }
    }
}

/// A derived Solana account.
pub struct SolanaAccount {
    pub signing_key: SigningKey,
    pub path: Option<String>,
}

/// Redacted on purpose, like every other key type in the wallet.
impl std::fmt::Debug for SolanaAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SolanaAccount")
            .field("address", &self.address())
            .field("signing_key", &"<redacted>")
            .finish()
    }
}

impl SolanaAccount {
    /// Derive `m/44'/501'/<index>'/0'` from a BIP-39 seed.
    pub fn from_seed(seed: &[u8], index: u32) -> Result<Self> {
        let path = path(index);
        let mut node = Node::master(seed);
        for component in crate::bip32::parse_path(&path)? {
            node = node.derive_child(component)?;
        }
        Ok(SolanaAccount {
            signing_key: SigningKey::from_bytes(&node.key),
            path: Some(path),
        })
    }

    /// Import from the encodings Solana tooling actually hands out.
    ///
    /// Three of them are in circulation and all are accepted: base58 of the
    /// 64-byte keypair (what `solana-keygen pubkey` prints and wallets export),
    /// base58 of a bare 32-byte seed, and `0x`-prefixed or bare hex of either.
    /// A 64-byte form carries its own public half, and it is checked rather
    /// than trusted — a mismatched pair signs for an address it does not own.
    pub fn from_secret(text: &str) -> Result<Self> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(error::invalid_private_key("the private key is empty"));
        }
        let bytes = decode_secret(trimmed)?;
        let seed: [u8; 32] = match bytes.len() {
            32 => bytes[..32].try_into().expect("checked length"),
            64 => bytes[..32].try_into().expect("checked length"),
            n => {
                return Err(error::invalid_private_key(format!(
                    "a Solana key is 32 bytes (a seed) or 64 (a keypair), got {n}"
                )))
            }
        };
        let signing_key = SigningKey::from_bytes(&seed);
        if bytes.len() == 64 && signing_key.verifying_key().as_bytes() != &bytes[32..] {
            return Err(error::invalid_private_key(
                "this keypair's public half does not match its secret half; it \
                 would sign for an address it does not control",
            ));
        }
        Ok(SolanaAccount {
            signing_key,
            path: None,
        })
    }

    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    /// The base58 address — literally the public key, nothing else.
    pub fn address(&self) -> String {
        bs58::encode(self.public_key_bytes()).into_string()
    }

    /// The 64-byte keypair, base58 encoded: the form wallets import.
    pub fn secret_base58(&self) -> String {
        bs58::encode(self.signing_key.to_keypair_bytes()).into_string()
    }

    /// The 32-byte seed as a JSON byte array, the shape of `id.json`.
    pub fn keypair_json(&self) -> Vec<u8> {
        self.signing_key.to_keypair_bytes().to_vec()
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.signing_key.sign(message).to_bytes()
    }
}

/// Decode a secret written as base58 or as hex.
///
/// Base58 is tried first because it is what Solana tooling emits; hex is
/// accepted because it is what every other chain in this wallet uses and
/// pasting the wrong one should not be a silent failure.
fn decode_secret(text: &str) -> Result<Vec<u8>> {
    if let Some(body) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return hex::decode(body)
            .map_err(|e| error::invalid_private_key(format!("not valid hexadecimal: {e}")));
    }
    if let Ok(bytes) = bs58::decode(text).into_vec() {
        return Ok(bytes);
    }
    hex::decode(text)
        .map_err(|_| error::invalid_private_key("a Solana key must be base58 or hex encoded"))
}

/// Decode a base58 address into its 32 raw bytes.
pub fn address_to_bytes(address: &str) -> Result<[u8; 32]> {
    let decoded = bs58::decode(address.trim())
        .into_vec()
        .map_err(|e| error::invalid_address(format!("not valid base58: {e}")))?;
    decoded.try_into().map_err(|v: Vec<u8>| {
        error::invalid_address(format!(
            "a Solana address is 32 bytes, this one decodes to {}",
            v.len()
        ))
    })
}

/// Verify an ed25519 signature over a message by a base58 address.
pub fn verify(address: &str, message: &[u8], signature: &[u8]) -> Result<bool> {
    let key_bytes = address_to_bytes(address)?;
    let Ok(verifying_key) = VerifyingKey::from_bytes(&key_bytes) else {
        // A valid 32-byte string that is not a curve point: real for Solana,
        // where program-derived addresses are chosen to be exactly that.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::Seed;

    const PHRASE: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn seed() -> Seed {
        Seed::new(PHRASE, "").unwrap()
    }

    /// The addresses `solana-keygen` derives from this phrase. Checked against
    /// the official CLI, and the reason SLIP-0010 is implemented rather than
    /// approximated.
    #[test]
    fn derives_the_addresses_the_official_cli_produces() {
        // testvectors/multichain.json, generated by @solana/web3.js.
        let expected = [
            "HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk",
            "Hh8QwFUA6MtVu1qAoq12ucvFHNwCcVTV7hpWjeY1Hztb",
            "7WktogJEd2wQ9eH2oWusmcoFTgeYi6rS632UviTBJ2jm",
        ];
        let bytes = seed().bip39_seed();
        for (index, want) in expected.iter().enumerate() {
            let account = SolanaAccount::from_seed(&bytes, index as u32).unwrap();
            assert_eq!(account.address(), *want, "index {index}");
        }
    }

    #[test]
    fn the_path_is_four_hardened_levels() {
        assert_eq!(path(0), "m/44'/501'/0'/0'");
        assert_eq!(path(7), "m/44'/501'/7'/0'");
        let account = SolanaAccount::from_seed(&seed().bip39_seed(), 3).unwrap();
        assert_eq!(account.path.as_deref(), Some("m/44'/501'/3'/0'"));
    }

    #[test]
    fn an_unhardened_index_is_refused_rather_than_hardened() {
        // The mistake this catches: one missing apostrophe silently producing
        // a different wallet.
        let node = Node::master(&seed().bip39_seed());
        let err = node.derive_child(44).unwrap_err();
        assert!(err.message.contains("hardened"), "{}", err.message);
    }

    #[test]
    fn the_address_is_the_public_key_itself() {
        let account = SolanaAccount::from_seed(&seed().bip39_seed(), 0).unwrap();
        assert_eq!(
            address_to_bytes(&account.address()).unwrap(),
            account.public_key_bytes()
        );
    }

    #[test]
    fn secrets_round_trip_through_every_encoding_tooling_uses() {
        let account = SolanaAccount::from_seed(&seed().bip39_seed(), 0).unwrap();
        let address = account.address();

        // base58 of the 64-byte keypair, which is what wallets export.
        assert_eq!(
            SolanaAccount::from_secret(&account.secret_base58())
                .unwrap()
                .address(),
            address
        );
        // base58 of a bare 32-byte seed.
        let seed58 = bs58::encode(account.signing_key.to_bytes()).into_string();
        assert_eq!(
            SolanaAccount::from_secret(&seed58).unwrap().address(),
            address
        );
        // hex, with and without the prefix every other chain here uses.
        let as_hex = hex::encode(account.signing_key.to_bytes());
        assert_eq!(
            SolanaAccount::from_secret(&as_hex).unwrap().address(),
            address
        );
        assert_eq!(
            SolanaAccount::from_secret(&format!("0x{as_hex}"))
                .unwrap()
                .address(),
            address
        );
    }

    #[test]
    fn a_keypair_whose_halves_disagree_is_refused() {
        // Signing with this would produce valid signatures for an address the
        // holder does not control, and the wallet would show the wrong one.
        let account = SolanaAccount::from_seed(&seed().bip39_seed(), 0).unwrap();
        let mut bytes = account.signing_key.to_keypair_bytes();
        bytes[32] ^= 0xff;
        let err = SolanaAccount::from_secret(&bs58::encode(bytes).into_string()).unwrap_err();
        assert_eq!(err.code, error::Code::InvalidPrivateKey);
        assert!(err.message.contains("does not match"), "{}", err.message);
    }

    #[test]
    fn malformed_secrets_are_refused_with_their_length() {
        assert!(SolanaAccount::from_secret("").is_err());
        assert!(SolanaAccount::from_secret("   ").is_err());
        assert!(SolanaAccount::from_secret("0xnothex").is_err());
        let short = SolanaAccount::from_secret(&bs58::encode([1u8; 16]).into_string()).unwrap_err();
        assert!(short.message.contains("got 16"), "{}", short.message);
    }

    /// ed25519 is deterministic, so the official SDK's signature over this
    /// message is reproducible byte for byte.
    #[test]
    fn signing_matches_the_official_sdk_byte_for_byte() {
        let account = SolanaAccount::from_seed(&seed().bip39_seed(), 0).unwrap();
        let signature = account.sign(b"midnight-cardano-solana test vector");
        assert_eq!(hex::encode(signature), "52e95ba23743b3219fd434b45e928cb298cf27a59852df2e68efc5538f75769af66805cac25f8601c7397d5413973b68e085174f338855b0dd96c15729cf6601");
    }

    #[test]
    fn signatures_verify_against_the_address_and_only_that_message() {
        let account = SolanaAccount::from_seed(&seed().bip39_seed(), 0).unwrap();
        let signature = account.sign(b"hello causewaybay");
        assert!(verify(&account.address(), b"hello causewaybay", &signature).unwrap());
        assert!(!verify(&account.address(), b"tampered", &signature).unwrap());

        let other = SolanaAccount::from_seed(&seed().bip39_seed(), 1).unwrap();
        assert!(!verify(&other.address(), b"hello causewaybay", &signature).unwrap());
    }

    #[test]
    fn verifying_against_a_non_curve_address_is_false_not_an_error() {
        // Program-derived addresses are deliberately off the curve, and a user
        // may well paste one. That is an answer, not a crash.
        let off_curve = bs58::encode([0u8; 32]).into_string();
        assert!(!verify(&off_curve, b"x", &[0u8; 64]).unwrap());
    }

    #[test]
    fn addresses_are_checked_for_length_not_just_alphabet() {
        assert!(address_to_bytes("not-base58-!!").is_err());
        let short = address_to_bytes(&bs58::encode([1u8; 20]).into_string()).unwrap_err();
        assert_eq!(short.code, error::Code::InvalidAddress);
        assert!(short.message.contains("20"), "{}", short.message);
    }

    #[test]
    fn an_accounts_debug_rendering_leaks_nothing() {
        let account = SolanaAccount::from_seed(&seed().bip39_seed(), 0).unwrap();
        let rendered = format!("{account:?}");
        assert!(rendered.contains(&account.address()));
        assert!(!rendered.contains(&account.secret_base58()));
    }
}
