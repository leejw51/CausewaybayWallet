//! Key material: derivation from mnemonics, address computation, EIP-191 signing.

use alloy_primitives::{keccak256, Address, B256};
use k256::ecdsa::{RecoveryId, Signature as EcdsaSignature, SigningKey, VerifyingKey};
use k256::SecretKey;

use crate::bip32::{ethereum_path, ExtendedPrivateKey};
use crate::bip39;
use crate::error::{self, Result};

/// A private key plus everything derivable from it.
#[derive(Clone)]
pub struct Keypair {
    pub private_key: [u8; 32],
}

/// Redacted on purpose: a `{:?}` of a keypair must never leak the secret.
impl std::fmt::Debug for Keypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keypair")
            .field("address", &self.address())
            .field("private_key", &"<redacted>")
            .finish()
    }
}

impl Keypair {
    pub fn from_bytes(bytes: [u8; 32]) -> Result<Self> {
        SecretKey::from_slice(&bytes).map_err(|_| {
            error::invalid_private_key("private key is not a valid secp256k1 scalar")
        })?;
        Ok(Keypair { private_key: bytes })
    }

    /// Accept a private key with or without the `0x` prefix.
    pub fn from_hex(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        let body = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .unwrap_or(trimmed);
        if body.len() != 64 {
            return Err(error::invalid_private_key(format!(
                "private key must be 64 hex characters, got {}",
                body.len()
            )));
        }
        let bytes = hex::decode(body)
            .map_err(|_| error::invalid_private_key("private key is not valid hexadecimal"))?;
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Keypair::from_bytes(arr)
    }

    /// Derive the account at BIP-44 index `index` from a mnemonic.
    pub fn from_mnemonic(phrase: &str, index: u32, passphrase: &str) -> Result<Self> {
        if !bip39::validate(phrase) {
            // Surface the specific reason (bad word, bad checksum, bad length).
            bip39::mnemonic_to_entropy(phrase)?;
        }
        let seed = bip39::to_seed(phrase, passphrase);
        let master = ExtendedPrivateKey::from_seed(&seed)?;
        let child = master.derive_path(&ethereum_path(index))?;
        Keypair::from_bytes(child.key)
    }

    pub fn signing_key(&self) -> SigningKey {
        SigningKey::from_bytes(&self.private_key.into()).expect("validated at construction")
    }

    pub fn private_key_hex(&self) -> String {
        format!("0x{}", hex::encode(self.private_key))
    }

    /// Uncompressed public key without the 0x04 SEC1 tag, hex encoded (128 chars).
    pub fn public_key_hex(&self) -> String {
        let point = self.signing_key().verifying_key().to_encoded_point(false);
        format!("0x{}", hex::encode(&point.as_bytes()[1..]))
    }

    /// Compressed 33-byte public key, hex encoded.
    pub fn public_key_compressed_hex(&self) -> String {
        let point = self.signing_key().verifying_key().to_encoded_point(true);
        format!("0x{}", hex::encode(point.as_bytes()))
    }

    pub fn address(&self) -> Address {
        address_from_verifying_key(self.signing_key().verifying_key())
    }

    /// Sign a 32-byte digest, returning the 65-byte `r‖s‖v` form with `v` in {27, 28}.
    pub fn sign_hash(&self, hash: &B256) -> Result<[u8; 65]> {
        let (sig, recovery_id) = self
            .signing_key()
            .sign_prehash_recoverable(hash.as_slice())
            .map_err(|e| error::internal(format!("signing failed: {e}")))?;
        let mut out = [0u8; 65];
        out[..64].copy_from_slice(&sig.to_bytes());
        out[64] = 27 + recovery_id.to_byte();
        Ok(out)
    }

    /// Sign a personal message per EIP-191.
    pub fn sign_message(&self, message: &[u8]) -> Result<[u8; 65]> {
        self.sign_hash(&eip191_hash(message))
    }
}

/// Ethereum address: last 20 bytes of the keccak hash of the uncompressed public key.
pub fn address_from_verifying_key(key: &VerifyingKey) -> Address {
    let point = key.to_encoded_point(false);
    let hash = keccak256(&point.as_bytes()[1..]);
    Address::from_slice(&hash[12..])
}

/// `keccak256("\x19Ethereum Signed Message:\n" || len || message)`.
pub fn eip191_hash(message: &[u8]) -> B256 {
    let mut buf = Vec::with_capacity(message.len() + 32);
    buf.extend_from_slice(b"\x19Ethereum Signed Message:\n");
    buf.extend_from_slice(message.len().to_string().as_bytes());
    buf.extend_from_slice(message);
    keccak256(buf)
}

/// Recover the signer of a 32-byte digest from a 65-byte signature.
pub fn recover_hash(hash: &B256, signature: &[u8]) -> Result<Address> {
    if signature.len() != 65 {
        return Err(error::usage(format!(
            "signature must be 65 bytes, got {}",
            signature.len()
        )));
    }
    let v = signature[64];
    // Accept the raw {0,1}, the EIP-191 {27,28} and the legacy EIP-155 encodings.
    let recovery = match v {
        0 | 1 => v,
        27 | 28 => v - 27,
        _ if v >= 35 => ((v as u64 - 35) % 2) as u8,
        _ => return Err(error::usage(format!("unsupported signature v value {v}"))),
    };
    let recovery_id = RecoveryId::from_byte(recovery)
        .ok_or_else(|| error::usage(format!("unsupported signature v value {v}")))?;
    let sig = EcdsaSignature::from_slice(&signature[..64])
        .map_err(|e| error::usage(format!("malformed signature: {e}")))?;
    let key = VerifyingKey::recover_from_prehash(hash.as_slice(), &sig, recovery_id)
        .map_err(|e| error::usage(format!("could not recover a public key: {e}")))?;
    Ok(address_from_verifying_key(&key))
}

/// Recover the signer of an EIP-191 personal message.
pub fn recover_message(message: &[u8], signature: &[u8]) -> Result<Address> {
    recover_hash(&eip191_hash(message), signature)
}

/// Parse `0x…` hex into bytes, tolerating a missing prefix.
pub fn parse_hex(input: &str) -> Result<Vec<u8>> {
    let trimmed = input.trim();
    let body = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    hex::decode(body).map_err(|e| error::usage(format!("invalid hex: {e}")))
}

/// Parse and checksum-normalise an address.
pub fn parse_address(input: &str) -> Result<Address> {
    let trimmed = input.trim();
    trimmed
        .parse::<Address>()
        .map_err(|_| error::invalid_address(format!("not a valid EVM address: {trimmed}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHRASE: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn derives_the_well_known_test_addresses() {
        // These are the standard addresses every Ethereum tool produces for this phrase.
        let expected = [
            "0x9858EfFD232B4033E47d90003D41EC34EcaEda94",
            "0x6Fac4D18c912343BF86fa7049364Dd4E424Ab9C0",
            "0xb6716976A3ebe8D39aCEB04372f22Ff8e6802D7A",
            "0xF3f50213C1d2e255e4B2bAD430F8A38EEF8D718E",
            "0x51cA8ff9f1C0a99f88E86B8112eA3237F55374cA",
        ];
        for (index, want) in expected.iter().enumerate() {
            let kp = Keypair::from_mnemonic(PHRASE, index as u32, "").unwrap();
            assert_eq!(kp.address().to_checksum(None), *want, "index {index}");
        }
    }

    #[test]
    fn index_zero_private_key_is_stable() {
        let kp = Keypair::from_mnemonic(PHRASE, 0, "").unwrap();
        assert_eq!(
            kp.private_key_hex(),
            "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727"
        );
    }

    #[test]
    fn private_key_round_trips_through_hex() {
        let kp = Keypair::from_mnemonic(PHRASE, 3, "").unwrap();
        let again = Keypair::from_hex(&kp.private_key_hex()).unwrap();
        assert_eq!(kp.address(), again.address());

        let without_prefix = Keypair::from_hex(&hex::encode(kp.private_key)).unwrap();
        assert_eq!(kp.address(), without_prefix.address());
    }

    #[test]
    fn rejects_malformed_private_keys() {
        assert!(Keypair::from_hex("").is_err());
        assert!(Keypair::from_hex("0x1234").is_err());
        assert!(Keypair::from_hex(&"z".repeat(64)).is_err());
        // Zero and values >= the curve order are not valid scalars.
        assert!(Keypair::from_hex(&"0".repeat(64)).is_err());
        assert!(Keypair::from_hex(&"f".repeat(64)).is_err());
    }

    #[test]
    fn rejects_bad_mnemonics() {
        assert_eq!(
            Keypair::from_mnemonic("not a real mnemonic phrase at all here ok", 0, "")
                .unwrap_err()
                .code,
            error::Code::InvalidMnemonic
        );
    }

    #[test]
    fn a_passphrase_yields_a_different_wallet() {
        let plain = Keypair::from_mnemonic(PHRASE, 0, "").unwrap();
        let with_pass = Keypair::from_mnemonic(PHRASE, 0, "hunter2").unwrap();
        assert_ne!(plain.address(), with_pass.address());
    }

    #[test]
    fn public_key_forms_are_consistent() {
        let kp = Keypair::from_mnemonic(PHRASE, 0, "").unwrap();
        let full = kp.public_key_hex();
        let compressed = kp.public_key_compressed_hex();
        assert_eq!(full.len(), 2 + 128);
        assert_eq!(compressed.len(), 2 + 66);
        // The compressed form carries the same X coordinate.
        assert_eq!(&compressed[4..], &full[2..66]);
    }

    #[test]
    fn eip191_hash_matches_reference() {
        // keccak256("\x19Ethereum Signed Message:\n12Hello World")
        let hash = eip191_hash(b"Hello World");
        assert_eq!(
            hex::encode(hash),
            "a1de988600a42c4b4ab089b619297c17d53cffae5d5120d82d8a92d0bb3b78f2"
        );
    }

    #[test]
    fn signs_and_recovers_messages() {
        let kp = Keypair::from_mnemonic(PHRASE, 0, "").unwrap();
        let sig = kp.sign_message(b"hello causewaybay").unwrap();
        assert!(sig[64] == 27 || sig[64] == 28);
        assert_eq!(
            recover_message(b"hello causewaybay", &sig).unwrap(),
            kp.address()
        );
    }

    #[test]
    fn recovery_fails_for_a_different_message() {
        let kp = Keypair::from_mnemonic(PHRASE, 0, "").unwrap();
        let sig = kp.sign_message(b"original").unwrap();
        assert_ne!(recover_message(b"tampered", &sig).unwrap(), kp.address());
    }

    #[test]
    fn signatures_are_deterministic_rfc6979() {
        let kp = Keypair::from_mnemonic(PHRASE, 0, "").unwrap();
        assert_eq!(
            kp.sign_message(b"same").unwrap(),
            kp.sign_message(b"same").unwrap()
        );
    }

    #[test]
    fn signs_empty_and_unicode_messages() {
        let kp = Keypair::from_mnemonic(PHRASE, 0, "").unwrap();
        for message in [b"".as_slice(), "héllo 🌏".as_bytes()] {
            let sig = kp.sign_message(message).unwrap();
            assert_eq!(recover_message(message, &sig).unwrap(), kp.address());
        }
    }

    #[test]
    fn rejects_malformed_signatures() {
        assert!(recover_hash(&eip191_hash(b"x"), &[0u8; 64]).is_err());
        let mut sig = [1u8; 65];
        sig[64] = 5;
        assert!(recover_hash(&eip191_hash(b"x"), &sig).is_err());
    }

    #[test]
    fn parses_addresses_case_insensitively() {
        let want = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94";
        assert_eq!(parse_address(want).unwrap().to_checksum(None), want);
        assert_eq!(
            parse_address(&want.to_lowercase())
                .unwrap()
                .to_checksum(None),
            want
        );
        assert!(parse_address("0x123").is_err());
        assert!(parse_address("nonsense").is_err());
    }
}
