//! CIP-19 Shelley addresses.
//!
//! A Shelley address is one header byte followed by one or two 28-byte
//! credentials, bech32-encoded. The header packs two nibbles:
//!
//! ```text
//!   bits 7..4 : address type   bits 3..0 : network id (0 testnet, 1 mainnet)
//!   0000 base       (payment key hash + stake key hash)
//!   0110 enterprise (payment key hash, no staking)
//!   1110 reward     (stake key hash)
//! ```
//!
//! There is no checksum byte of its own — bech32's is the only guard — which
//! is why the human-readable part (`addr` versus `addr_test`) carries so much
//! weight, and why decoding here insists the prefix and the header agree.

use bech32::{Bech32, Hrp};

use crate::error::{self, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardanoNetwork {
    Testnet,
    Mainnet,
}

impl CardanoNetwork {
    pub fn id(self) -> u8 {
        match self {
            CardanoNetwork::Testnet => 0,
            CardanoNetwork::Mainnet => 1,
        }
    }

    /// Which of the two a wallet network row means.
    ///
    /// Preprod and preview are separate chains but share the testnet id, so
    /// an address is portable between them and only the endpoint differs.
    pub fn of(network: &crate::network::Network) -> Self {
        if network.testnet {
            CardanoNetwork::Testnet
        } else {
            CardanoNetwork::Mainnet
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressKind {
    /// Payment credential plus staking credential.
    Base,
    /// Payment credential only, with no staking rights.
    Enterprise,
    /// Staking credential only; where rewards accrue.
    Reward,
}

impl AddressKind {
    fn type_nibble(self) -> u8 {
        match self {
            AddressKind::Base => 0b0000,
            AddressKind::Enterprise => 0b0110,
            AddressKind::Reward => 0b1110,
        }
    }

    fn hrp(self, network: CardanoNetwork) -> &'static str {
        match (self, network) {
            (AddressKind::Reward, CardanoNetwork::Mainnet) => "stake",
            (AddressKind::Reward, CardanoNetwork::Testnet) => "stake_test",
            (_, CardanoNetwork::Mainnet) => "addr",
            (_, CardanoNetwork::Testnet) => "addr_test",
        }
    }

    fn expected_len(self) -> usize {
        match self {
            AddressKind::Base => 57,
            AddressKind::Enterprise | AddressKind::Reward => 29,
        }
    }
}

/// A Shelley address, kept as raw bytes plus enough context to re-encode it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub kind: AddressKind,
    pub network: CardanoNetwork,
    pub bytes: Vec<u8>,
}

impl Address {
    pub fn base(network: CardanoNetwork, payment: [u8; 28], stake: [u8; 28]) -> Self {
        let mut bytes = Vec::with_capacity(57);
        bytes.push((AddressKind::Base.type_nibble() << 4) | network.id());
        bytes.extend_from_slice(&payment);
        bytes.extend_from_slice(&stake);
        Address {
            kind: AddressKind::Base,
            network,
            bytes,
        }
    }

    pub fn enterprise(network: CardanoNetwork, payment: [u8; 28]) -> Self {
        let mut bytes = Vec::with_capacity(29);
        bytes.push((AddressKind::Enterprise.type_nibble() << 4) | network.id());
        bytes.extend_from_slice(&payment);
        Address {
            kind: AddressKind::Enterprise,
            network,
            bytes,
        }
    }

    pub fn reward(network: CardanoNetwork, stake: [u8; 28]) -> Self {
        let mut bytes = Vec::with_capacity(29);
        bytes.push((AddressKind::Reward.type_nibble() << 4) | network.id());
        bytes.extend_from_slice(&stake);
        Address {
            kind: AddressKind::Reward,
            network,
            bytes,
        }
    }

    pub fn to_bech32(&self) -> String {
        let hrp = Hrp::parse(self.kind.hrp(self.network))
            .expect("the prefixes here are static and valid");
        bech32::encode::<Bech32>(hrp, &self.bytes)
            .expect("an address payload is far under the length limit")
    }

    /// Decode a bech32 address, insisting the header agrees with the prefix.
    pub fn parse(text: &str) -> Result<Self> {
        let (hrp, bytes) = bech32::decode(text.trim())
            .map_err(|e| error::invalid_address(format!("not a valid bech32 address: {e}")))?;
        let hrp = hrp.to_lowercase();
        let header = *bytes
            .first()
            .ok_or_else(|| error::invalid_address("the address payload is empty"))?;
        let network = match header & 0x0f {
            0 => CardanoNetwork::Testnet,
            1 => CardanoNetwork::Mainnet,
            n => {
                return Err(error::invalid_address(format!(
                    "unknown Cardano network id {n}"
                )))
            }
        };
        let kind = match header >> 4 {
            0b0000 => AddressKind::Base,
            0b0110 => AddressKind::Enterprise,
            0b1110 => AddressKind::Reward,
            t => {
                return Err(error::invalid_address(format!(
                    "address type {t} is not one this wallet can spend to"
                )))
            }
        };
        if bytes.len() != kind.expected_len() {
            return Err(error::invalid_address(format!(
                "a {kind:?} address is {} bytes, this one is {}",
                kind.expected_len(),
                bytes.len()
            )));
        }
        // An `addr_test1…` string carrying a mainnet header is a real hazard:
        // the funds go to a chain the recipient is not watching.
        if hrp != kind.hrp(network) {
            return Err(error::invalid_address(format!(
                "the prefix `{hrp}` disagrees with the address header, which says \
                 `{}` — one of the two is from a different network",
                kind.hrp(network)
            )));
        }
        Ok(Address {
            kind,
            network,
            bytes,
        })
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_bech32())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYMENT: [u8; 28] = [0x11; 28];
    const STAKE: [u8; 28] = [0x22; 28];

    #[test]
    fn the_three_kinds_round_trip() {
        for address in [
            Address::base(CardanoNetwork::Testnet, PAYMENT, STAKE),
            Address::enterprise(CardanoNetwork::Testnet, PAYMENT),
            Address::reward(CardanoNetwork::Mainnet, STAKE),
            Address::base(CardanoNetwork::Mainnet, PAYMENT, STAKE),
        ] {
            let parsed = Address::parse(&address.to_bech32()).unwrap();
            assert_eq!(parsed, address);
        }
    }

    #[test]
    fn prefixes_say_which_network_and_which_kind() {
        assert!(Address::base(CardanoNetwork::Testnet, PAYMENT, STAKE)
            .to_bech32()
            .starts_with("addr_test1"));
        assert!(Address::base(CardanoNetwork::Mainnet, PAYMENT, STAKE)
            .to_bech32()
            .starts_with("addr1"));
        assert!(Address::reward(CardanoNetwork::Testnet, STAKE)
            .to_bech32()
            .starts_with("stake_test1"));
        assert!(Address::reward(CardanoNetwork::Mainnet, STAKE)
            .to_bech32()
            .starts_with("stake1"));
    }

    #[test]
    fn a_base_address_is_57_bytes_and_the_others_29() {
        assert_eq!(
            Address::base(CardanoNetwork::Testnet, PAYMENT, STAKE)
                .bytes
                .len(),
            57
        );
        assert_eq!(
            Address::enterprise(CardanoNetwork::Testnet, PAYMENT)
                .bytes
                .len(),
            29
        );
        assert_eq!(
            Address::reward(CardanoNetwork::Testnet, STAKE).bytes.len(),
            29
        );
    }

    /// The hazard this guards: a mainnet payload wearing a testnet prefix.
    /// Accepting it would send funds to a chain nobody is watching.
    #[test]
    fn a_prefix_that_disagrees_with_the_header_is_refused() {
        let mainnet = Address::base(CardanoNetwork::Mainnet, PAYMENT, STAKE);
        let forged =
            bech32::encode::<Bech32>(Hrp::parse("addr_test").unwrap(), &mainnet.bytes).unwrap();
        let err = Address::parse(&forged).unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAddress);
        assert!(err.message.contains("disagrees"), "{}", err.message);
    }

    #[test]
    fn a_corrupted_address_fails_the_bech32_checksum() {
        let good = Address::base(CardanoNetwork::Testnet, PAYMENT, STAKE).to_bech32();
        let mut broken: Vec<char> = good.chars().collect();
        let last = broken.len() - 1;
        broken[last] = if broken[last] == 'q' { 'p' } else { 'q' };
        assert!(Address::parse(&broken.into_iter().collect::<String>()).is_err());
    }

    #[test]
    fn junk_and_wrong_lengths_are_refused() {
        for bad in ["", "addr_test1", "not an address", "0xdeadbeef"] {
            assert!(Address::parse(bad).is_err(), "{bad} should not parse");
        }
        // A well-formed bech32 string of the wrong length for its type.
        let short =
            bech32::encode::<Bech32>(Hrp::parse("addr_test").unwrap(), &[0x00; 30]).unwrap();
        let err = Address::parse(&short).unwrap_err();
        assert!(err.message.contains("57 bytes"), "{}", err.message);
    }

    #[test]
    fn an_unsupported_address_type_says_so() {
        // Type 0b0011 is a script-hash form this wallet cannot spend to.
        // Type 3 on testnet (network id 0), so the low nibble is zero.
        let mut bytes = vec![0b0011 << 4];
        bytes.extend_from_slice(&[0u8; 56]);
        let text = bech32::encode::<Bech32>(Hrp::parse("addr_test").unwrap(), &bytes).unwrap();
        let err = Address::parse(&text).unwrap_err();
        assert!(err.message.contains("type 3"), "{}", err.message);
    }

    #[test]
    fn the_network_of_a_wallet_row_maps_to_the_header_nibble() {
        assert_eq!(
            CardanoNetwork::of(&crate::network::CARDANO_PREPROD),
            CardanoNetwork::Testnet
        );
        assert_eq!(
            CardanoNetwork::of(&crate::network::CARDANO_PREVIEW),
            CardanoNetwork::Testnet
        );
        assert_eq!(
            CardanoNetwork::of(&crate::network::CARDANO_MAINNET),
            CardanoNetwork::Mainnet
        );
    }
}
