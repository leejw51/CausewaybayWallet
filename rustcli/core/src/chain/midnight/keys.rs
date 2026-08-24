//! Midnight key derivation.
//!
//! Midnight's HD scheme (per `@midnight-ntwrk/wallet-sdk-hd`) is plain BIP-32
//! over secp256k1 from the BIP-39 seed, with a CIP-1852-style role level:
//!
//! ```text
//!   m / 44' / 2400' / account' / role / index
//! ```
//!
//! The last two levels are **not** hardened. The derived 32-byte scalar is
//! used directly as a BIP-340 (x-only Schnorr) signing key with no extra
//! hashing, and the unshielded address is `SHA-256` of the 32-byte x-only
//! public key.

use k256::schnorr::signature::{Signer, Verifier};
use k256::schnorr::{Signature, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::bip32::ExtendedPrivateKey;
use crate::error::{self, Result};

use super::address::{MidnightAddress, NetworkId};

/// BIP-44 purpose.
pub const PURPOSE: u32 = 44;
/// Midnight's SLIP-44 coin type.
pub const COIN_TYPE: u32 = 2400;

/// The role level of a Midnight derivation path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Role {
    /// Night receive addresses.
    NightExternal = 0,
    /// Night change addresses.
    NightInternal = 1,
    /// Dust, the token that pays fees.
    Dust = 2,
    /// Shielded (Zswap) keys.
    Zswap = 3,
    /// Metadata signing keys.
    Metadata = 4,
}

impl Role {
    pub fn index(self) -> u32 {
        self as u32
    }
}

/// The path for a role and index, in the default account.
pub fn path_for(role: Role, index: u32) -> String {
    format!("m/{PURPOSE}'/{COIN_TYPE}'/0'/{}/{index}", role.index())
}

/// The Night receive path for an address index.
pub fn path(index: u32) -> String {
    path_for(Role::NightExternal, index)
}

/// Derive the raw 32-byte key at `m/44'/2400'/0'/<role>/<index>`.
pub fn derive_key(seed: &[u8], role: Role, index: u32) -> Result<[u8; 32]> {
    Ok(ExtendedPrivateKey::from_seed(seed)?
        .derive_path(&path_for(role, index))?
        .key)
}

/// A derived Midnight account on the unshielded (Night) side.
pub struct MidnightAccount {
    pub signing_key: SigningKey,
    /// The scalar exactly as BIP-32 produced it.
    ///
    /// Deliberately kept apart from `signing_key`: BIP-340 requires the secret
    /// whose public point has an even y, so `SigningKey::from_bytes` silently
    /// negates the scalar about half the time. Exporting the negated value
    /// would round-trip through *this* wallet but disagree with what the
    /// Midnight SDK reports for the same path, for roughly half of all
    /// indices — so the derived bytes are what is kept and exported.
    secret: [u8; 32],
    /// The seed of the DUST key at the same index, where it is known.
    ///
    /// Fees are paid in DUST, and the DUST key lives at a *different role* of
    /// the same path — it cannot be derived from the night key. So an account
    /// keeps both, and an account imported as a bare night key keeps `None`
    /// and says so when a send needs one, rather than silently deriving a
    /// stranger's dust address.
    dust_seed: Option<[u8; 32]>,
    pub path: Option<String>,
}

/// Redacted on purpose, like every other key type in the wallet.
impl std::fmt::Debug for MidnightAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidnightAccount")
            .field("address", &hex::encode(self.address_bytes()))
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl MidnightAccount {
    /// Derive a Night receive account, and the DUST key that pays its fees.
    pub fn from_seed(seed: &[u8], index: u32) -> Result<Self> {
        let key = derive_key(seed, Role::NightExternal, index)?;
        Ok(MidnightAccount {
            signing_key: signing_key(&key)?,
            secret: key,
            dust_seed: Some(derive_key(seed, Role::Dust, index)?),
            path: Some(path(index)),
        })
    }

    /// Rebuild from a stored secret: the night key, then the DUST seed.
    ///
    /// A bare 32-byte night key is accepted too, because that is what someone
    /// pasting a key out of another tool will have. Such an account can hold
    /// and receive NIGHT and sign messages; it cannot pay a fee, and
    /// [`Self::dust_seed`] says so rather than inventing one.
    pub fn from_secret(secret: &str) -> Result<Self> {
        let body = secret.trim();
        let body = body.strip_prefix("0x").unwrap_or(body);
        let bytes = hex::decode(body)
            .map_err(|e| error::invalid_private_key(format!("not valid hexadecimal: {e}")))?;
        let (key, dust_seed) = match bytes.len() {
            32 => (bytes[..32].try_into().expect("checked length"), None),
            64 => (
                bytes[..32].try_into().expect("checked length"),
                Some(bytes[32..].try_into().expect("checked length")),
            ),
            n => {
                return Err(error::invalid_private_key(format!(
                    "a Midnight secret is 32 bytes (a night key) or 64 (a night                      key and its DUST seed), got {n}"
                )))
            }
        };
        let key: [u8; 32] = key;
        Ok(MidnightAccount {
            signing_key: signing_key(&key)?,
            secret: key,
            dust_seed,
            path: None,
        })
    }

    /// The secret exactly as BIP-32 derived it — see the `secret` field.
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.secret
    }

    /// The stored form: the night key, followed by the DUST seed when known.
    pub fn secret_hex(&self) -> String {
        match self.dust_seed {
            Some(dust) => format!("{}{}", hex::encode(self.secret), hex::encode(dust)),
            None => hex::encode(self.secret),
        }
    }

    /// The DUST key seed, or an explanation of why there is none.
    pub fn dust_seed(&self) -> Result<[u8; 32]> {
        self.dust_seed.ok_or_else(|| {
            error::usage(
                "this account was imported as a bare night key, so the wallet does                  not have the DUST key that pays Midnight fees. Re-import it from                  its mnemonic to send; it can still receive and sign as it is",
            )
        })
    }

    /// The 32-byte x-only public key. BIP-340 drops the y coordinate entirely.
    pub fn verifying_key_bytes(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes().into()
    }

    /// The raw 32-byte user address: `SHA-256(x-only public key)`.
    pub fn address_bytes(&self) -> [u8; 32] {
        address_from_verifying_key(&self.verifying_key_bytes())
    }

    /// The bech32m address for a network.
    pub fn address(&self, network: NetworkId) -> MidnightAddress {
        MidnightAddress::unshielded(network, self.address_bytes())
    }

    /// A BIP-340 Schnorr signature.
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        let signature: Signature = self.signing_key.sign(message);
        signature.to_bytes()
    }
}

fn signing_key(bytes: &[u8; 32]) -> Result<SigningKey> {
    SigningKey::from_bytes(bytes)
        .map_err(|e| error::invalid_private_key(format!("not a valid BIP-340 key: {e}")))
}

/// `UserAddress = persistent_hash(verifying_key)`, and Midnight's
/// `persistent_hash` is SHA-256.
pub fn address_from_verifying_key(key: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(key);
    hasher.finalize().into()
}

/// Verify a BIP-340 signature against an x-only public key.
pub fn verify(public_key: &[u8; 32], message: &[u8], signature: &[u8]) -> Result<bool> {
    let Ok(verifying_key) = VerifyingKey::from_bytes(public_key) else {
        return Ok(false);
    };
    // `Signature::try_from` panics rather than erring on a wrong-length
    // slice, so the length is checked here first.
    if signature.len() != 64 {
        return Err(error::usage(format!(
            "a BIP-340 signature is 64 bytes, got {}",
            signature.len()
        )));
    }
    let Ok(signature) = Signature::try_from(signature) else {
        return Ok(false);
    };
    Ok(verifying_key.verify(message, &signature).is_ok())
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

    fn seed() -> [u8; 64] {
        Seed::new(PHRASE, "").unwrap().bip39_seed()
    }

    /// Against `@midnight-ntwrk/wallet-sdk-hd`, for every index in the file.
    #[test]
    fn keys_and_addresses_match_the_official_sdk() {
        let vectors = vectors();
        for (index, expected) in vectors["midnight"]["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            let account = MidnightAccount::from_seed(&seed(), index as u32).unwrap();
            assert_eq!(
                account.path.as_deref().unwrap(),
                expected["path"].as_str().unwrap()
            );
            // `secret_hex` is the stored pair (night key ‖ DUST seed); the
            // SDK reports the night key alone.
            assert_eq!(
                hex::encode(account.secret_bytes()),
                expected["night_sk_hex"].as_str().unwrap(),
                "night key at index {index}"
            );
            assert_eq!(
                hex::encode(account.verifying_key_bytes()),
                expected["verifying_key_hex"].as_str().unwrap(),
                "verifying key at index {index}"
            );
            assert_eq!(
                hex::encode(account.address_bytes()),
                expected["address_hex"].as_str().unwrap(),
                "address bytes at index {index}"
            );
            assert_eq!(
                account.address(NetworkId::Mainnet).to_bech32m().unwrap(),
                expected["addr_mainnet"].as_str().unwrap()
            );
            assert_eq!(
                account
                    .address(NetworkId::Named("test".into()))
                    .to_bech32m()
                    .unwrap(),
                expected["addr_testnet"].as_str().unwrap()
            );
        }
    }

    /// The subtle one. BIP-340 negates about half of all secret keys, so a
    /// wallet that exported `SigningKey::to_bytes()` would disagree with the
    /// SDK for roughly half the indices — and only for those.
    #[test]
    fn the_exported_secret_is_the_derived_scalar_not_the_negated_one() {
        let vectors = vectors();
        let mut negated = 0;
        for (index, expected) in vectors["midnight"]["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            let account = MidnightAccount::from_seed(&seed(), index as u32).unwrap();
            // What the wallet derives always matches the SDK …
            assert_eq!(
                hex::encode(account.secret_bytes()),
                expected["night_sk_hex"].as_str().unwrap()
            );
            // … even where BIP-340 has quietly flipped the scalar underneath.
            if <[u8; 32]>::from(account.signing_key.to_bytes()) != account.secret_bytes() {
                negated += 1;
            }
        }
        // Not an assertion about which indices, only that the distinction is
        // real for this seed — otherwise the test above proves nothing.
        assert!(
            negated > 0,
            "no index in the vector file exercises the negation; the guard is untested"
        );
    }

    #[test]
    fn the_dust_role_derives_the_key_the_sdk_reports() {
        let vectors = vectors();
        let dust = &vectors["midnight"]["dust"];
        assert_eq!(path_for(Role::Dust, 0), dust["path"].as_str().unwrap());
        assert_eq!(
            hex::encode(derive_key(&seed(), Role::Dust, 0).unwrap()),
            dust["seed_hex"].as_str().unwrap()
        );
    }

    #[test]
    fn the_last_two_path_levels_are_not_hardened() {
        assert_eq!(path(0), "m/44'/2400'/0'/0/0");
        assert_eq!(path_for(Role::Dust, 3), "m/44'/2400'/0'/2/3");
        // Which means a normal-derivation branch is genuinely exercised; a
        // hardened-only implementation would simply fail here.
        assert!(derive_key(&seed(), Role::NightExternal, 1).is_ok());
    }

    #[test]
    fn the_stored_secret_round_trips_with_both_keys() {
        let original = MidnightAccount::from_seed(&seed(), 2).unwrap();
        let restored = MidnightAccount::from_secret(&original.secret_hex()).unwrap();
        assert_eq!(restored.address_bytes(), original.address_bytes());
        assert_eq!(restored.secret_bytes(), original.secret_bytes());
        // The DUST half survived, so the restored account can still pay a fee.
        assert_eq!(restored.dust_seed().unwrap(), original.dust_seed().unwrap());
        assert!(restored.path.is_none());
        // The `0x` prefix every other chain here uses is accepted too.
        assert_eq!(
            MidnightAccount::from_secret(&format!("0x{}", original.secret_hex()))
                .unwrap()
                .address_bytes(),
            original.address_bytes()
        );
    }

    #[test]
    fn the_stored_secret_carries_the_dust_seed_the_sdk_reports() {
        let account = MidnightAccount::from_seed(&seed(), 0).unwrap();
        assert_eq!(
            hex::encode(account.dust_seed().unwrap()),
            vectors()["midnight"]["dust"]["seed_hex"].as_str().unwrap()
        );
        assert_eq!(account.secret_hex().len(), 128, "night key then DUST seed");
    }

    /// A key pasted out of another tool has no DUST half. That account is
    /// usable for everything but paying a fee, and the refusal has to say so
    /// rather than deriving a dust address belonging to nobody.
    #[test]
    fn a_bare_night_key_is_usable_but_cannot_pay_a_fee() {
        let derived = MidnightAccount::from_seed(&seed(), 0).unwrap();
        let bare = MidnightAccount::from_secret(&hex::encode(derived.secret_bytes())).unwrap();

        assert_eq!(bare.address_bytes(), derived.address_bytes());
        assert!(verify(&bare.verifying_key_bytes(), b"x", &bare.sign(b"x")).unwrap());

        let err = bare.dust_seed().unwrap_err();
        assert_eq!(err.code, error::Code::Usage);
        assert!(err.message.contains("Re-import"), "{}", err.message);
        // And it round-trips as the bare key it is, rather than growing a
        // fabricated dust half.
        assert_eq!(bare.secret_hex().len(), 64);
    }

    #[test]
    fn a_secret_of_the_wrong_length_says_so() {
        let err = MidnightAccount::from_secret(&hex::encode([0u8; 16])).unwrap_err();
        assert_eq!(err.code, error::Code::InvalidPrivateKey);
        assert!(err.message.contains("got 16"), "{}", err.message);
        assert!(MidnightAccount::from_secret("nonsense").is_err());
        assert!(MidnightAccount::from_secret("").is_err());
    }

    #[test]
    fn the_address_is_sha256_of_the_x_only_public_key() {
        let account = MidnightAccount::from_seed(&seed(), 0).unwrap();
        assert_eq!(
            account.address_bytes(),
            address_from_verifying_key(&account.verifying_key_bytes())
        );
        let mut hasher = Sha256::new();
        hasher.update(account.verifying_key_bytes());
        assert_eq!(account.address_bytes(), <[u8; 32]>::from(hasher.finalize()));
    }

    #[test]
    fn signatures_verify_against_the_key_and_only_that_message() {
        let account = MidnightAccount::from_seed(&seed(), 0).unwrap();
        let signature = account.sign(b"hello causewaybay");
        assert!(verify(
            &account.verifying_key_bytes(),
            b"hello causewaybay",
            &signature
        )
        .unwrap());
        assert!(!verify(&account.verifying_key_bytes(), b"tampered", &signature).unwrap());

        let other = MidnightAccount::from_seed(&seed(), 1).unwrap();
        assert!(!verify(
            &other.verifying_key_bytes(),
            b"hello causewaybay",
            &signature
        )
        .unwrap());
    }

    #[test]
    fn a_signature_of_the_wrong_length_is_an_error_not_a_false() {
        let account = MidnightAccount::from_seed(&seed(), 0).unwrap();
        let err = verify(&account.verifying_key_bytes(), b"x", &[0u8; 10]).unwrap_err();
        assert!(err.message.contains("64 bytes"), "{}", err.message);
    }

    #[test]
    fn an_accounts_debug_rendering_leaks_nothing() {
        let account = MidnightAccount::from_seed(&seed(), 0).unwrap();
        let rendered = format!("{account:?}");
        assert!(!rendered.contains(&account.secret_hex()));
        assert!(rendered.contains("redacted"));
    }
}
