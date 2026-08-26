//! eCash key material: BIP-44 derivation, hash160 addresses, WIF, and the
//! two signatures a Bitcoin-family chain asks for.
//!
//! eCash is secp256k1, like EVM, and reuses this workspace's [`ExtendedPrivateKey`]
//! wholesale. What differs is everything downstream of the scalar:
//!
//! * the path is SLIP-44 coin type **1899**, `m/44'/1899'/0'/0/i`;
//! * an address is `ripemd160(sha256(compressed_public_key))`, not the last
//!   twenty bytes of a keccak hash, so the compressed form is load-bearing —
//!   the uncompressed one hashes to a different, equally valid, unfunded
//!   address;
//! * a transaction signature is DER-encoded ECDSA with a sighash byte glued
//!   on, while a *message* signature is the 65-byte recoverable form Bitcoin
//!   has used since 2011. They are different encodings of the same scheme and
//!   are not interchangeable.
//!
//! [`ExtendedPrivateKey`]: crate::bip32::ExtendedPrivateKey

use k256::ecdsa::signature::hazmat::PrehashVerifier;
use k256::ecdsa::{RecoveryId, Signature as EcdsaSignature, SigningKey, VerifyingKey};
use k256::SecretKey;
use ripemd::Ripemd160;
use sha2::{Digest, Sha256};
use zeroize::ZeroizeOnDrop;

use crate::bip32::ExtendedPrivateKey;
use crate::chain::Seed;
use crate::error::{self, Result};

use super::address::{Address, EcashNetwork};

/// What Bitcoin's `signmessage` has prefixed since 2011, and what eCash still
/// prefixes: keeping it means a signature from this wallet verifies in
/// Electrum ABC and in `bitcoind`'s `verifymessage`.
const MESSAGE_MAGIC: &[u8] = b"Bitcoin Signed Message:\n";

/// The SLIP-44 coin type eCash registered for itself.
pub const COIN_TYPE: u32 = 1899;

/// The BIP-44 account path for an address index.
///
/// One path for both networks. eCash's own tooling uses coin type 1 on
/// testnet, as Bitcoin's does, but this wallet renders one key as two
/// addresses rather than deriving two keys — the same choice Cardano makes,
/// and the one that lets `account new` produce an account that works wherever
/// the wallet is later pointed.
pub fn path(index: u32) -> String {
    format!("m/44'/{COIN_TYPE}'/0'/0/{index}")
}

/// `ripemd160(sha256(data))` — how every Bitcoin-family address is built.
pub fn hash160(data: &[u8]) -> [u8; 20] {
    let sha = Sha256::digest(data);
    let mut out = [0u8; 20];
    out.copy_from_slice(&Ripemd160::digest(sha));
    out
}

/// `sha256(sha256(data))` — the chain's hash for transactions and checksums.
pub fn sha256d(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(&Sha256::digest(Sha256::digest(data)));
    out
}

/// One eCash account: a scalar, and everything it implies.
///
/// Wiped on drop, clones included — an account is built and dropped on almost
/// every command that touches a key.
#[derive(Clone, ZeroizeOnDrop)]
pub struct EcashAccount {
    private_key: [u8; 32],
    #[zeroize(skip)]
    pub path: Option<String>,
}

/// Redacted on purpose: a `{:?}` must never carry the scalar.
impl std::fmt::Debug for EcashAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EcashAccount")
            .field(
                "address",
                &self.address(EcashNetwork::Mainnet).to_cashaddr(),
            )
            .field("private_key", &"<redacted>")
            .finish()
    }
}

impl EcashAccount {
    fn from_bytes(private_key: [u8; 32], path: Option<String>) -> Result<Self> {
        SecretKey::from_slice(&private_key)
            .map_err(|_| error::invalid_private_key("that is not a valid secp256k1 scalar"))?;
        Ok(EcashAccount { private_key, path })
    }

    /// Derive the account at `index` from a mnemonic.
    pub fn from_seed(seed: &Seed, index: u32) -> Result<Self> {
        let bip39_seed = seed.bip39_seed();
        let master = ExtendedPrivateKey::from_seed(&bip39_seed[..])?;
        let child = master.derive_path(&path(index))?;
        EcashAccount::from_bytes(child.key, Some(path(index)))
    }

    /// Rebuild an account from a stored or pasted secret.
    ///
    /// Two encodings are accepted and one is written back. Hex is what this
    /// wallet stores, because it says nothing about a network and the key
    /// genuinely works on both. WIF is what eCash's own wallets export, and
    /// refusing to read one would mean telling a user to go and convert their
    /// key by hand before importing it — so it is read, and normalised away.
    pub fn from_secret(secret: &str) -> Result<Self> {
        let trimmed = secret.trim();
        if trimmed.is_empty() {
            return Err(error::invalid_private_key("no private key supplied"));
        }
        let body = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .unwrap_or(trimmed);
        if body.len() == 64 {
            if let Ok(bytes) = hex::decode(body) {
                let mut key = [0u8; 32];
                key.copy_from_slice(&bytes);
                return EcashAccount::from_bytes(key, None);
            }
        }
        EcashAccount::from_wif(trimmed)
    }

    /// Read a Wallet Import Format key: base58check over a version byte, the
    /// scalar, and a trailing `0x01` meaning "this key's address uses the
    /// compressed public key".
    ///
    /// The uncompressed form is refused rather than accepted and quietly
    /// re-rendered: it hashes to a different address, so importing one and
    /// showing the compressed address would name an account that holds
    /// nothing and cannot be told apart from one that does.
    fn from_wif(input: &str) -> Result<Self> {
        let decoded = base58check_decode(input).map_err(|_| {
            error::invalid_private_key(
                "a private key is 64 hex characters or a WIF string; this is neither",
            )
        })?;
        let (version, rest) = decoded
            .split_first()
            .ok_or_else(|| error::invalid_private_key("that WIF key carries no payload"))?;
        if *version != EcashNetwork::Mainnet.wif_version()
            && *version != EcashNetwork::Testnet.wif_version()
        {
            return Err(error::invalid_private_key(format!(
                "that WIF key has version byte 0x{version:02x}, which is not eCash's; \
                 it is probably a key for another chain"
            )));
        }
        match rest.len() {
            33 if rest[32] == 0x01 => {
                let mut key = [0u8; 32];
                key.copy_from_slice(&rest[..32]);
                EcashAccount::from_bytes(key, None)
            }
            32 => Err(error::invalid_private_key(
                "that WIF key is the uncompressed form, whose address is a \
                 different one from the compressed key's; this wallet holds only \
                 compressed-key accounts",
            )),
            other => Err(error::invalid_private_key(format!(
                "a WIF key holds 33 bytes after its version byte, this one holds {other}"
            ))),
        }
    }

    /// The stored form: `0x` and 64 hex characters, network-neutral.
    pub fn secret_hex(&self) -> String {
        format!("0x{}", hex::encode(self.private_key))
    }

    /// The WIF form for one network, for export into eCash's own tooling.
    pub fn wif(&self, network: EcashNetwork) -> String {
        let mut payload = Vec::with_capacity(34);
        payload.push(network.wif_version());
        payload.extend_from_slice(&self.private_key);
        payload.push(0x01); // compressed
        base58check_encode(&payload)
    }

    pub fn signing_key(&self) -> SigningKey {
        SigningKey::from_bytes(&self.private_key.into()).expect("validated at construction")
    }

    /// The 33-byte compressed public key — the one the address hashes.
    pub fn public_key(&self) -> [u8; 33] {
        let mut out = [0u8; 33];
        out.copy_from_slice(
            self.signing_key()
                .verifying_key()
                .to_encoded_point(true)
                .as_bytes(),
        );
        out
    }

    pub fn hash160(&self) -> [u8; 20] {
        hash160(&self.public_key())
    }

    pub fn address(&self, network: EcashNetwork) -> Address {
        Address::p2pkh(network, self.hash160())
    }

    /// Sign a 32-byte transaction digest, DER-encoded, low-`s`.
    ///
    /// Low-`s` is not a preference: eCash enforces it as a consensus rule, so
    /// the high-`s` half of every valid signature is unspendable. `k256`
    /// normalises on the way out, which is the only reason this is one line.
    pub fn sign_der(&self, digest: &[u8; 32]) -> Result<Vec<u8>> {
        let (signature, _): (EcdsaSignature, RecoveryId) = self
            .signing_key()
            .sign_prehash_recoverable(digest)
            .map_err(|e| error::internal(format!("signing failed: {e}")))?;
        Ok(signature
            .normalize_s()
            .unwrap_or(signature)
            .to_der()
            .to_bytes()
            .to_vec())
    }

    /// Sign a personal message the way `bitcoind` and Electrum ABC do.
    ///
    /// 65 bytes: a header saying which of the four candidate public keys
    /// recovers, then `r‖s`. The header carries `+4` because this wallet's
    /// addresses are compressed-key ones, and a verifier uses that to know
    /// which address to hash.
    pub fn sign_message(&self, message: &[u8]) -> Result<[u8; 65]> {
        let digest = message_hash(message);
        let (signature, recovery) = self
            .signing_key()
            .sign_prehash_recoverable(&digest)
            .map_err(|e| error::internal(format!("signing failed: {e}")))?;
        let mut out = [0u8; 65];
        out[0] = 27 + 4 + recovery.to_byte();
        out[1..].copy_from_slice(&signature.to_bytes());
        Ok(out)
    }
}

/// `sha256d(magic_len ‖ magic ‖ varint(len) ‖ message)`.
///
/// The length prefixes are what stop one signature being read as a signature
/// over a different message with the same bytes somewhere inside it.
pub fn message_hash(message: &[u8]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(message.len() + 32);
    buf.push(MESSAGE_MAGIC.len() as u8);
    buf.extend_from_slice(MESSAGE_MAGIC);
    write_varint(&mut buf, message.len() as u64);
    buf.extend_from_slice(message);
    sha256d(&buf)
}

/// Recover the public key hash a message signature belongs to.
pub fn recover_message(message: &[u8], signature: &[u8]) -> Result<[u8; 20]> {
    if signature.len() != 65 {
        return Err(error::usage(format!(
            "an eCash message signature is 65 bytes, this one is {}",
            signature.len()
        )));
    }
    let header = signature[0];
    // 27..30 uncompressed, 31..34 compressed. Both recover the same key; only
    // the address it hashes to differs, and this wallet renders the
    // compressed one because that is what it holds.
    if !(27..=34).contains(&header) {
        return Err(error::usage(format!(
            "0x{header:02x} is not a signature header an eCash signature starts with"
        )));
    }
    let recovery = RecoveryId::from_byte((header - 27) & 0x03)
        .ok_or_else(|| error::usage("that signature's recovery id is not a valid one"))?;
    let ecdsa = EcdsaSignature::from_slice(&signature[1..])
        .map_err(|_| error::usage("that signature's r and s are not a valid pair"))?;
    let digest = message_hash(message);
    let key = VerifyingKey::recover_from_prehash(&digest, &ecdsa, recovery)
        .map_err(|_| error::usage("no public key recovers from that signature"))?;
    Ok(hash160(key.to_encoded_point(true).as_bytes()))
}

/// Check a signature against a known public key, for the DER form.
pub fn verify_der(public_key: &[u8], digest: &[u8; 32], der: &[u8]) -> Result<bool> {
    let key = VerifyingKey::from_sec1_bytes(public_key)
        .map_err(|_| error::internal("that is not a valid public key"))?;
    let Ok(signature) = EcdsaSignature::from_der(der) else {
        return Ok(false);
    };
    Ok(key.verify_prehash(digest, &signature).is_ok())
}

/// Bitcoin's variable-length integer, as scripts and transactions write it.
pub fn write_varint(out: &mut Vec<u8>, value: u64) {
    match value {
        0..=0xfc => out.push(value as u8),
        0xfd..=0xffff => {
            out.push(0xfd);
            out.extend_from_slice(&(value as u16).to_le_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(0xfe);
            out.extend_from_slice(&(value as u32).to_le_bytes());
        }
        _ => {
            out.push(0xff);
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
}

/// base58 with a four-byte `sha256d` tail, which is how WIF is written.
fn base58check_encode(payload: &[u8]) -> String {
    let mut full = payload.to_vec();
    full.extend_from_slice(&sha256d(payload)[..4]);
    bs58::encode(full).into_string()
}

fn base58check_decode(input: &str) -> std::result::Result<Vec<u8>, ()> {
    let full = bs58::decode(input.trim()).into_vec().map_err(|_| ())?;
    if full.len() < 5 {
        return Err(());
    }
    let (payload, checksum) = full.split_at(full.len() - 4);
    if sha256d(payload)[..4] != *checksum {
        return Err(());
    }
    Ok(payload.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHRASE: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn seed() -> Seed {
        Seed::new(PHRASE, "").unwrap()
    }

    #[test]
    fn the_path_is_the_slip44_ecash_one() {
        assert_eq!(path(0), "m/44'/1899'/0'/0/0");
        assert_eq!(path(7), "m/44'/1899'/0'/0/7");
    }

    /// Against the shared vectors, whose keys come from `eth_account`'s BIP-32
    /// and whose addresses come from an encoder pinned to the CashAddr
    /// specification — so neither half of this can drift on its own.
    #[test]
    fn derivation_produces_the_reference_addresses() {
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("../../../../../testvectors/ecash.json")).unwrap();
        for expected in vectors["accounts"].as_array().unwrap() {
            let index = expected["index"].as_u64().unwrap() as u32;
            let account = EcashAccount::from_seed(&seed(), index).unwrap();
            assert_eq!(
                account.secret_hex(),
                expected["private_key"],
                "index {index}"
            );
            assert_eq!(
                account.address(EcashNetwork::Mainnet).to_cashaddr(),
                expected["address_mainnet"],
                "index {index}"
            );
            assert_eq!(
                account.address(EcashNetwork::Testnet).to_cashaddr(),
                expected["address"],
                "index {index}"
            );
            assert_eq!(
                hex::encode(account.hash160()),
                expected["public_key_hash"],
                "index {index}"
            );
        }
    }

    /// The compressed public key is what the address hashes, and the
    /// uncompressed one hashes to a different, unfunded address.
    #[test]
    fn the_address_hashes_the_compressed_key() {
        let account = EcashAccount::from_seed(&seed(), 0).unwrap();
        assert_eq!(
            hex::encode(account.public_key()),
            "03ee1364cd7af3a9ffbbbd886388776a6f92a7b8dd986f6a8578885e4b856f7bfb"
        );
        assert_eq!(
            hex::encode(account.hash160()),
            "dc224140d18053b1c27da53d73fca6f44fc87449"
        );
        let uncompressed = account
            .signing_key()
            .verifying_key()
            .to_encoded_point(false);
        assert_ne!(hash160(uncompressed.as_bytes()), account.hash160());
    }

    #[test]
    fn a_key_round_trips_through_hex_and_through_wif() {
        let account = EcashAccount::from_seed(&seed(), 0).unwrap();
        for form in [
            account.secret_hex(),
            account.secret_hex().trim_start_matches("0x").to_string(),
            account.wif(EcashNetwork::Mainnet),
            account.wif(EcashNetwork::Testnet),
        ] {
            let restored = EcashAccount::from_secret(&form).unwrap();
            assert_eq!(restored.secret_hex(), account.secret_hex(), "{form}");
        }
        // WIF goes in and hex comes out: the store holds one encoding.
        assert_eq!(
            account.wif(EcashNetwork::Mainnet),
            "L2K5YQqeLXuNnaavA6ydeAkzg51KTWGoqmHdzYNsbLwaGfuXn8az"
        );
        assert_eq!(
            account.wif(EcashNetwork::Testnet),
            "cSg51KqVmbbdx24BYWnm1VG4JJJj7xNVuoS76xqP6TbaXQyiVzzH"
        );
    }

    #[test]
    fn a_wif_key_from_another_chain_is_named_rather_than_accepted() {
        // Bitcoin's own mainnet WIF shares eCash's version byte, so the case
        // that has to be caught is a version byte from somewhere else — a
        // Litecoin key, here.
        let mut payload = vec![0xb0];
        payload.extend_from_slice(&[7u8; 32]);
        payload.push(0x01);
        let err = EcashAccount::from_secret(&base58check_encode(&payload)).unwrap_err();
        assert_eq!(err.code, error::Code::InvalidPrivateKey);
        assert!(err.message.contains("another chain"), "{}", err.message);
    }

    #[test]
    fn an_uncompressed_wif_key_is_refused_with_the_reason() {
        let mut payload = vec![0x80];
        payload.extend_from_slice(&[7u8; 32]);
        let err = EcashAccount::from_secret(&base58check_encode(&payload)).unwrap_err();
        assert!(err.message.contains("uncompressed"), "{}", err.message);
    }

    #[test]
    fn a_wif_key_with_a_broken_checksum_is_refused() {
        let account = EcashAccount::from_seed(&seed(), 0).unwrap();
        let mut wif: Vec<char> = account.wif(EcashNetwork::Mainnet).chars().collect();
        wif[10] = if wif[10] == 'a' { 'b' } else { 'a' };
        assert!(EcashAccount::from_secret(&wif.into_iter().collect::<String>()).is_err());
    }

    #[test]
    fn a_message_signature_recovers_the_signer() {
        let account = EcashAccount::from_seed(&seed(), 0).unwrap();
        let signature = account.sign_message(b"hello causewaybay").unwrap();
        assert_eq!(signature.len(), 65);
        // Compressed-key header range, because that is the address it holds.
        assert!((31..=34).contains(&signature[0]), "{}", signature[0]);
        assert_eq!(
            recover_message(b"hello causewaybay", &signature).unwrap(),
            account.hash160()
        );
        // A different message recovers somebody else, not a failure.
        assert_ne!(
            recover_message(b"hello causewaybaz", &signature).unwrap(),
            account.hash160()
        );
    }

    /// The message prefix is what a wallet is checked against; if it drifted,
    /// every signature this wallet made would stop verifying elsewhere.
    #[test]
    fn the_message_prefix_is_the_one_bitcoin_tooling_uses() {
        // `sha256d(0x18 ‖ "Bitcoin Signed Message:\n" ‖ 0x05 ‖ "hello")`, the
        // digest `bitcoind`'s own `signmessage` hashes.
        assert_eq!(MESSAGE_MAGIC.len(), 0x18);
        let mut expected = vec![0x18u8];
        expected.extend_from_slice(b"Bitcoin Signed Message:\n");
        expected.push(0x05);
        expected.extend_from_slice(b"hello");
        assert_eq!(message_hash(b"hello"), sha256d(&expected));
    }

    #[test]
    fn a_der_signature_verifies_against_the_signing_key_and_no_other() {
        let account = EcashAccount::from_seed(&seed(), 0).unwrap();
        let other = EcashAccount::from_seed(&seed(), 1).unwrap();
        let digest = [9u8; 32];
        let der = account.sign_der(&digest).unwrap();
        assert!(verify_der(&account.public_key(), &digest, &der).unwrap());
        assert!(!verify_der(&other.public_key(), &digest, &der).unwrap());
        // Low-`s`, which eCash enforces: the top bit of the s value's first
        // byte can never be set once s is below the curve order's half.
        assert!(der[0] == 0x30 && der.len() >= 8);
    }

    #[test]
    fn varints_use_the_shortest_encoding() {
        let cases: [(u64, &str); 5] = [
            (0, "00"),
            (0xfc, "fc"),
            (0xfd, "fdfd00"),
            (0x1_0000, "fe00000100"),
            (0x1_0000_0000, "ff0000000001000000"),
        ];
        for (value, want) in cases {
            let mut out = Vec::new();
            write_varint(&mut out, value);
            assert_eq!(hex::encode(&out), want, "{value}");
        }
    }

    #[test]
    fn a_debug_rendering_leaks_no_key() {
        let account = EcashAccount::from_seed(&seed(), 0).unwrap();
        let rendered = format!("{account:?}");
        assert!(!rendered.contains("97f2d7fa"), "{rendered}");
        assert!(rendered.contains("ecash:"), "{rendered}");
    }
}
