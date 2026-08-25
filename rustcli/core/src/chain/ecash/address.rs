//! CashAddr: the address format eCash inherited from Bitcoin Cash.
//!
//! An address is a human-readable prefix, a colon, and a base32 body:
//!
//! ```text
//!   ecash:qrwzys2q6xq98vwz0kjn6ulu5m6yljr5fyc909kalg
//!   ^^^^^ prefix          ^ 34 chars of payload      ^ 8 chars of checksum
//! ```
//!
//! The payload is one version byte followed by the 20-byte hash, regrouped
//! from 8-bit to 5-bit digits. The version byte packs the address type in
//! bits 6..3 (`0` pay-to-public-key-hash, `1` pay-to-script-hash) and the hash
//! size in bits 2..0 (`0` for 160 bits); bit 7 is reserved and must be clear.
//!
//! The checksum is a 40-bit BCH code over the prefix, the payload and eight
//! zero digits — the same construction bech32 uses, with a different generator
//! and, crucially, **the prefix folded in**. That is what makes a mainnet
//! address fail its own checksum when read as a testnet one rather than
//! quietly decoding to the same 20 bytes: the two prefixes are not
//! interchangeable labels on one string, and sending across that line burns
//! the funds.

use crate::error::{self, Result};

/// The base32 alphabet, chosen so no two similar-looking characters are both
/// in it. The same one bech32 uses.
const CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

/// Which eCash network an address belongs to.
///
/// The prefix is part of the checksummed data, so this is not decoration —
/// it is the one thing separating a mainnet address from a testnet one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EcashNetwork {
    Testnet,
    Mainnet,
}

impl EcashNetwork {
    pub fn prefix(self) -> &'static str {
        match self {
            EcashNetwork::Testnet => "ectest",
            EcashNetwork::Mainnet => "ecash",
        }
    }

    /// Which of the two a wallet network row means.
    pub fn of(network: &crate::network::Network) -> Self {
        if network.testnet {
            EcashNetwork::Testnet
        } else {
            EcashNetwork::Mainnet
        }
    }

    fn from_prefix(prefix: &str) -> Option<Self> {
        match prefix {
            "ecash" => Some(EcashNetwork::Mainnet),
            "ectest" => Some(EcashNetwork::Testnet),
            _ => None,
        }
    }

    /// The base58 version byte a WIF private key carries on this network.
    pub fn wif_version(self) -> u8 {
        match self {
            EcashNetwork::Mainnet => 0x80,
            EcashNetwork::Testnet => 0xef,
        }
    }
}

/// What the 20 bytes of an address hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressKind {
    /// A public key. What this wallet's own accounts are.
    P2pkh,
    /// A redeem script — a multisig or a contract. This wallet can pay one but
    /// cannot hold one, because it has no script to spend it with.
    P2sh,
}

impl AddressKind {
    fn version_byte(self) -> u8 {
        match self {
            // Type nibble in bits 6..3, hash size `0` (160 bits) in bits 2..0.
            AddressKind::P2pkh => 0x00,
            AddressKind::P2sh => 0x08,
        }
    }

    fn from_version_byte(byte: u8) -> Result<Self> {
        if byte & 0x80 != 0 {
            return Err(error::invalid_address(
                "the address's version byte has its reserved bit set",
            ));
        }
        if byte & 0x07 != 0 {
            return Err(error::invalid_address(
                "the address encodes a hash this wallet does not handle; \
                 eCash addresses hold 160 bits",
            ));
        }
        match (byte >> 3) & 0x0f {
            0 => Ok(AddressKind::P2pkh),
            1 => Ok(AddressKind::P2sh),
            other => Err(error::invalid_address(format!(
                "unknown address type {other}"
            ))),
        }
    }
}

/// A CashAddr address, kept as the parts it is built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Address {
    pub kind: AddressKind,
    pub network: EcashNetwork,
    pub hash: [u8; 20],
}

impl Address {
    pub fn p2pkh(network: EcashNetwork, hash: [u8; 20]) -> Self {
        Address {
            kind: AddressKind::P2pkh,
            network,
            hash,
        }
    }

    pub fn p2sh(network: EcashNetwork, hash: [u8; 20]) -> Self {
        Address {
            kind: AddressKind::P2sh,
            network,
            hash,
        }
    }

    /// The same address on the other network — the same key, relabelled and
    /// re-checksummed.
    pub fn on(&self, network: EcashNetwork) -> Self {
        Address { network, ..*self }
    }

    /// The full `prefix:body` form. This is what the wallet stores and shows;
    /// the prefix is never dropped, because a bare body is ambiguous.
    pub fn to_cashaddr(&self) -> String {
        let prefix = self.network.prefix();
        let mut payload = Vec::with_capacity(21);
        payload.push(self.kind.version_byte());
        payload.extend_from_slice(&self.hash);
        let body = to_base32(&payload);

        let mut checksum_input: Vec<u8> = prefix.bytes().map(|c| c & 0x1f).collect();
        checksum_input.push(0);
        checksum_input.extend_from_slice(&body);
        checksum_input.extend_from_slice(&[0u8; 8]);
        let checksum = polymod(&checksum_input);

        let mut out = String::with_capacity(prefix.len() + 1 + body.len() + 8);
        out.push_str(prefix);
        out.push(':');
        for digit in body {
            out.push(CHARSET[digit as usize] as char);
        }
        for i in 0..8 {
            let digit = (checksum >> (5 * (7 - i))) & 0x1f;
            out.push(CHARSET[digit as usize] as char);
        }
        out
    }

    /// The locking script this address stands for.
    ///
    /// P2PKH: `OP_DUP OP_HASH160 <20> OP_EQUALVERIFY OP_CHECKSIG`.
    /// P2SH:  `OP_HASH160 <20> OP_EQUAL`.
    pub fn script_pubkey(&self) -> Vec<u8> {
        match self.kind {
            AddressKind::P2pkh => {
                let mut script = Vec::with_capacity(25);
                script.extend_from_slice(&[0x76, 0xa9, 0x14]);
                script.extend_from_slice(&self.hash);
                script.extend_from_slice(&[0x88, 0xac]);
                script
            }
            AddressKind::P2sh => {
                let mut script = Vec::with_capacity(23);
                script.extend_from_slice(&[0xa9, 0x14]);
                script.extend_from_slice(&self.hash);
                script.push(0x87);
                script
            }
        }
    }

    /// Read an address, requiring the prefix that names its network.
    ///
    /// A body with no prefix is accepted only because eCash tooling prints
    /// them that way, and then only when its checksum picks out exactly one
    /// network — which it always does, the prefix being part of the checksum.
    /// An ambiguous one is refused rather than guessed at.
    pub fn parse(input: &str) -> Result<Address> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(error::invalid_address("an address cannot be empty"));
        }
        if let Some((prefix, body)) = trimmed.rsplit_once(':') {
            let prefix = prefix.to_lowercase();
            let network = EcashNetwork::from_prefix(&prefix).ok_or_else(|| {
                error::invalid_address(format!(
                    "'{prefix}' is not an eCash address prefix; this wallet reads \
                     `ecash:` and `ectest:` addresses"
                ))
            })?;
            return decode_body(network, body);
        }
        // Legacy base58 addresses were retired with the eCash rebrand, and a
        // wallet that silently converted one would be guessing at a network.
        if trimmed.starts_with('1') || trimmed.starts_with('3') {
            return Err(error::invalid_address(
                "that looks like a legacy base58 address; eCash uses CashAddr, \
                 which names its network — convert it and pass the `ecash:` form",
            ));
        }
        let candidates: Vec<Address> = [EcashNetwork::Mainnet, EcashNetwork::Testnet]
            .into_iter()
            .filter_map(|network| decode_body(network, trimmed).ok())
            .collect();
        match candidates.as_slice() {
            [only] => Ok(*only),
            [] => Err(error::invalid_address(format!(
                "'{trimmed}' is not a valid eCash address"
            ))),
            _ => Err(error::invalid_address(format!(
                "'{trimmed}' has no prefix and reads as an address on more than one \
                 network; pass it with its `ecash:` or `ectest:` prefix"
            ))),
        }
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_cashaddr())
    }
}

/// Decode a base32 body against one prefix, checksum and all.
fn decode_body(network: EcashNetwork, body: &str) -> Result<Address> {
    // Mixed case is not merely unusual: the checksum is computed over the
    // low five bits of each character, so `Q` and `q` are the same digit and
    // a mixed-case string would pass a check it should not.
    let lower = body.to_lowercase();
    if body != lower && body != body.to_uppercase() {
        return Err(error::invalid_address(
            "an address is all lower case or all upper case, never a mix",
        ));
    }
    let mut digits = Vec::with_capacity(lower.len());
    for c in lower.bytes() {
        let digit = CHARSET.iter().position(|d| *d == c).ok_or_else(|| {
            error::invalid_address(format!(
                "'{}' is not a character an eCash address is written with",
                c as char
            ))
        })?;
        digits.push(digit as u8);
    }
    if digits.len() < 9 {
        return Err(error::invalid_address("that address is too short"));
    }

    let mut checksum_input: Vec<u8> = network.prefix().bytes().map(|c| c & 0x1f).collect();
    checksum_input.push(0);
    checksum_input.extend_from_slice(&digits);
    if polymod(&checksum_input) != 0 {
        return Err(error::invalid_address(format!(
            "that address fails its checksum under the `{}:` prefix; either a \
             character of it is wrong or it belongs to another network",
            network.prefix()
        )));
    }

    let payload = from_base32(&digits[..digits.len() - 8])?;
    let (version, hash) = payload
        .split_first()
        .ok_or_else(|| error::invalid_address("that address carries no payload"))?;
    if hash.len() != 20 {
        return Err(error::invalid_address(format!(
            "that address carries {} bytes where an eCash address carries 20",
            hash.len()
        )));
    }
    let kind = AddressKind::from_version_byte(*version)?;
    let mut bytes = [0u8; 20];
    bytes.copy_from_slice(hash);
    Ok(Address {
        kind,
        network,
        hash: bytes,
    })
}

/// Regroup bytes into 5-bit digits, zero-padding the last one.
fn to_base32(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 8 / 5 + 1);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for byte in data {
        acc = (acc << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(((acc >> bits) & 0x1f) as u8);
        }
    }
    if bits > 0 {
        out.push(((acc << (5 - bits)) & 0x1f) as u8);
    }
    out
}

/// Regroup 5-bit digits back into bytes, refusing a non-zero pad.
///
/// A pad that carries bits is how one payload gets two spellings, and two
/// spellings of an address is one more than a wallet can safely have.
fn from_base32(digits: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(digits.len() * 5 / 8);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for digit in digits {
        acc = (acc << 5) | u32::from(*digit);
        bits += 5;
        while bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    if bits >= 5 || (acc << (8 - bits)) & 0xff != 0 {
        return Err(error::invalid_address(
            "that address has padding bits it should not have",
        ));
    }
    Ok(out)
}

/// The 40-bit BCH checksum CashAddr is guarded by.
///
/// Detects any error of up to four characters, and any burst of up to eight —
/// which is what lets a wrong-network address be caught rather than decoded.
fn polymod(values: &[u8]) -> u64 {
    let mut c: u64 = 1;
    for value in values {
        let c0 = (c >> 35) as u8;
        c = ((c & 0x07_ffff_ffff) << 5) ^ u64::from(*value);
        if c0 & 0x01 != 0 {
            c ^= 0x98_f2bc_8e61;
        }
        if c0 & 0x02 != 0 {
            c ^= 0x79_b76d_99e2;
        }
        if c0 & 0x04 != 0 {
            c ^= 0xf3_3e5f_b3c4;
        }
        if c0 & 0x08 != 0 {
            c ^= 0xae_2eab_e2a8;
        }
        if c0 & 0x10 != 0 {
            c ^= 0x1e_4f43_e470;
        }
    }
    c ^ 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(hex: &str) -> [u8; 20] {
        let mut out = [0u8; 20];
        hex::decode_to_slice(hex, &mut out).unwrap();
        out
    }

    /// The vector published with the CashAddr specification itself, which is
    /// checkable against a document rather than against another program.
    ///
    /// eCash swapped the prefix and nothing else, so a `bitcoincash:` vector
    /// pins down every part of the encoding that matters: the base32
    /// regrouping, the version byte and the BCH checksum over a prefix.
    #[test]
    fn the_published_specification_vector_encodes_byte_for_byte() {
        let payload = hash("f5bf48b397dae70be82b3cca4793f8eb2b6cdac9");
        let mut body = to_base32(&[&[0u8][..], &payload[..]].concat());
        let mut input: Vec<u8> = "bitcoincash".bytes().map(|c| c & 0x1f).collect();
        input.push(0);
        input.extend_from_slice(&body);
        input.extend_from_slice(&[0u8; 8]);
        let checksum = polymod(&input);
        for i in 0..8 {
            body.push(((checksum >> (5 * (7 - i))) & 0x1f) as u8);
        }
        let encoded: String = body.iter().map(|d| CHARSET[*d as usize] as char).collect();
        assert_eq!(encoded, "qr6m7j9njldwwzlg9v7v53unlr4jkmx6eylep8ekg2");
    }

    /// A pay-to-script-hash address read off the live eCash chain, which is
    /// the other half of the encoding and the one no unit test would invent.
    #[test]
    fn a_script_hash_address_from_the_chain_round_trips() {
        let live = "ecash:pquc59839pv8fga4h8eayy5fty0s00aj5czp4d547x";
        let parsed = Address::parse(live).unwrap();
        assert_eq!(parsed.kind, AddressKind::P2sh);
        assert_eq!(parsed.network, EcashNetwork::Mainnet);
        assert_eq!(
            hex::encode(parsed.hash),
            "398a14f1285874a3b5b9f3d21289591f07bfb2a6"
        );
        assert_eq!(parsed.to_cashaddr(), live);
        // And the script it locks to, which is what a transaction pays.
        assert_eq!(
            hex::encode(parsed.script_pubkey()),
            "a914398a14f1285874a3b5b9f3d21289591f07bfb2a687"
        );
    }

    #[test]
    fn a_public_key_hash_address_renders_its_own_script() {
        let address = Address::p2pkh(
            EcashNetwork::Mainnet,
            hash("dc224140d18053b1c27da53d73fca6f44fc87449"),
        );
        assert_eq!(
            address.to_cashaddr(),
            "ecash:qrwzys2q6xq98vwz0kjn6ulu5m6yljr5fyc909kalg"
        );
        assert_eq!(
            hex::encode(address.script_pubkey()),
            "76a914dc224140d18053b1c27da53d73fca6f44fc8744988ac"
        );
    }

    /// The whole reason the prefix is inside the checksum: the same 20 bytes
    /// on the other network is a different string, and neither one decodes as
    /// the other.
    #[test]
    fn the_same_key_is_a_different_address_on_each_network() {
        let payload = hash("dc224140d18053b1c27da53d73fca6f44fc87449");
        let mainnet = Address::p2pkh(EcashNetwork::Mainnet, payload);
        let testnet = mainnet.on(EcashNetwork::Testnet);
        assert_eq!(
            testnet.to_cashaddr(),
            "ectest:qrwzys2q6xq98vwz0kjn6ulu5m6yljr5fy7w393sue"
        );
        assert_ne!(mainnet.to_cashaddr(), testnet.to_cashaddr());
        assert_eq!(mainnet.hash, testnet.hash);

        // Neither body passes the other's checksum, which is what stops a
        // testnet address quietly taking mainnet funds.
        let mainnet_body = mainnet.to_cashaddr();
        let body = mainnet_body.split_once(':').unwrap().1;
        assert!(decode_body(EcashNetwork::Mainnet, body).is_ok());
        let err = decode_body(EcashNetwork::Testnet, body).unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAddress);
        assert!(err.message.contains("checksum"), "{}", err.message);
    }

    #[test]
    fn a_prefix_free_body_resolves_because_only_one_network_checksums_it() {
        let full = "ecash:qrwzys2q6xq98vwz0kjn6ulu5m6yljr5fyc909kalg";
        let bare = full.split_once(':').unwrap().1;
        let parsed = Address::parse(bare).unwrap();
        assert_eq!(parsed.network, EcashNetwork::Mainnet);
        assert_eq!(parsed.to_cashaddr(), full);
    }

    #[test]
    fn one_wrong_character_is_caught() {
        let mut broken: Vec<char> = "ecash:qrwzys2q6xq98vwz0kjn6ulu5m6yljr5fyc909kalg"
            .chars()
            .collect();
        broken[8] = 'p';
        let err = Address::parse(&broken.into_iter().collect::<String>()).unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAddress);
    }

    #[test]
    fn upper_case_is_the_same_address_and_mixed_case_is_not_an_address() {
        let full = "ecash:qrwzys2q6xq98vwz0kjn6ulu5m6yljr5fyc909kalg";
        assert_eq!(
            Address::parse(&full.to_uppercase()).unwrap().to_cashaddr(),
            full
        );
        let mixed = "ecash:QRWZYS2Q6XQ98VWZ0KJN6ULU5M6YLJR5FYc909kalg";
        let err = Address::parse(mixed).unwrap_err();
        assert!(err.message.contains("mix"), "{}", err.message);
    }

    #[test]
    fn a_foreign_prefix_says_what_this_wallet_reads() {
        let err =
            Address::parse("bitcoincash:qr6m7j9njldwwzlg9v7v53unlr4jkmx6eylep8ekg2").unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAddress);
        assert!(err.message.contains("ecash:"), "{}", err.message);
    }

    #[test]
    fn a_legacy_base58_address_is_named_rather_than_called_malformed() {
        let err = Address::parse("1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2").unwrap_err();
        assert!(err.message.contains("CashAddr"), "{}", err.message);
    }

    #[test]
    fn base32_regrouping_round_trips_and_refuses_a_dirty_pad() {
        let payload = [0u8]
            .iter()
            .chain(hash("dc224140d18053b1c27da53d73fca6f44fc87449").iter())
            .copied()
            .collect::<Vec<u8>>();
        let digits = to_base32(&payload);
        assert_eq!(digits.len(), 34);
        assert_eq!(from_base32(&digits).unwrap(), payload);

        // 21 bytes is 168 bits and 34 digits is 170, so the last digit ends
        // in two bits of pad. Setting one gives a second spelling of one
        // address, which is one spelling too many.
        let mut dirty = digits;
        *dirty.last_mut().unwrap() |= 0x01;
        assert!(from_base32(&dirty).is_err());
    }
}
