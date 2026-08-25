//! Midnight bech32m addresses.
//!
//! The human-readable part is structured — `mn_<type>[_<network>]` — and the
//! network segment is **omitted entirely on mainnet**. So the same 32 bytes
//! render as `mn_addr1…`, `mn_addr_test1…` or `mn_addr_preview1…` depending on
//! where they are meant to be spent, and the prefix is the only thing standing
//! between a transfer and the wrong network.
//!
//! Types this wallet handles:
//! - `addr` — unshielded (Night) address, 32 bytes
//! - `shield-addr` — shielded address, 32-byte coin key ‖ 32-byte encryption key
//! - `shield-cpk` / `shield-epk` — the two shielded halves on their own

use bech32::{Bech32m, Hrp};

use crate::error::{self, Result};

/// Which Midnight network an address is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkId {
    Mainnet,
    /// Any named network: `preview`, `test`, `devnet`, `undeployed`, …
    Named(String),
}

impl NetworkId {
    /// Parse a network name, folding the aliases people type into the HRP
    /// segments the official SDK emits.
    ///
    /// Anything that could not be an HRP segment — uppercase, an underscore, a
    /// whole URL — is refused rather than silently baked into an address that
    /// nobody can decode.
    pub fn parse(name: &str) -> Result<Self> {
        Ok(match name {
            "mainnet" => NetworkId::Mainnet,
            "devnet" | "dev" => NetworkId::Named("devnet".into()),
            "testnet" | "test" => NetworkId::Named("test".into()),
            other => {
                let usable = !other.is_empty()
                    && other
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
                if !usable {
                    return Err(error::unknown_network(format!(
                        "`{other}` is not a Midnight network name; try mainnet, \
                         devnet, testnet, preview or undeployed"
                    )));
                }
                NetworkId::Named(other.to_string())
            }
        })
    }

    /// Which network a wallet row means.
    pub fn of(network: &crate::network::Network) -> Result<Self> {
        // `midnight-preview` → `preview`, which is the segment the SDK uses.
        NetworkId::parse(network.key.strip_prefix("midnight-").unwrap_or(network.key))
    }

    /// The HRP suffix, empty on mainnet.
    fn suffix(&self) -> String {
        match self {
            NetworkId::Mainnet => String::new(),
            NetworkId::Named(name) => format!("_{name}"),
        }
    }
}

impl std::fmt::Display for NetworkId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkId::Mainnet => f.write_str("mainnet"),
            NetworkId::Named(name) => f.write_str(name),
        }
    }
}

pub const TYPE_UNSHIELDED: &str = "addr";
pub const TYPE_SHIELDED: &str = "shield-addr";
pub const TYPE_COIN_PUBLIC_KEY: &str = "shield-cpk";
pub const TYPE_ENCRYPTION_PUBLIC_KEY: &str = "shield-epk";

/// A parsed Midnight bech32m string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidnightAddress {
    pub kind: String,
    pub network: NetworkId,
    pub payload: Vec<u8>,
}

impl MidnightAddress {
    /// An unshielded (Night) address over a 32-byte user address.
    pub fn unshielded(network: NetworkId, address: [u8; 32]) -> Self {
        MidnightAddress {
            kind: TYPE_UNSHIELDED.into(),
            network,
            payload: address.to_vec(),
        }
    }

    pub fn to_bech32m(&self) -> Result<String> {
        let text = format!("mn_{}{}", self.kind, self.network.suffix());
        let hrp = Hrp::parse(&text)
            .map_err(|e| error::invalid_address(format!("invalid prefix `{text}`: {e}")))?;
        bech32::encode::<Bech32m>(hrp, &self.payload)
            .map_err(|e| error::invalid_address(format!("bech32m encoding failed: {e}")))
    }

    /// Parse `mn_addr_preview1…` back into its parts.
    pub fn parse(text: &str) -> Result<Self> {
        let (hrp, payload) = bech32::decode(text.trim())
            .map_err(|e| error::invalid_address(format!("not a valid bech32m address: {e}")))?;
        let hrp = hrp.to_lowercase();

        // `mn` ‖ type ‖ optional network. A type may contain a hyphen
        // (`shield-addr`) but never an underscore, so splitting on `_` is safe.
        let mut parts = hrp.split('_');
        if parts.next() != Some("mn") {
            return Err(error::invalid_address(format!(
                "`{hrp}` is not a Midnight address prefix"
            )));
        }
        let kind = parts
            .next()
            .ok_or_else(|| error::invalid_address(format!("`{hrp}` has no type segment")))?
            .to_string();
        let network = match parts.next() {
            Some(name) => NetworkId::Named(name.to_string()),
            None => NetworkId::Mainnet,
        };
        if parts.next().is_some() {
            return Err(error::invalid_address(format!(
                "`{hrp}` has more segments than a Midnight prefix can"
            )));
        }
        let expected = match kind.as_str() {
            TYPE_UNSHIELDED | TYPE_COIN_PUBLIC_KEY | TYPE_ENCRYPTION_PUBLIC_KEY => Some(32),
            TYPE_SHIELDED => Some(64),
            _ => None,
        };
        if let Some(len) = expected {
            if payload.len() != len {
                return Err(error::invalid_address(format!(
                    "an `mn_{kind}` payload is {len} bytes, this one is {}",
                    payload.len()
                )));
            }
        }
        Ok(MidnightAddress {
            kind,
            network,
            payload,
        })
    }

    /// The 32-byte user address, for an unshielded address.
    pub fn unshielded_bytes(&self) -> Result<[u8; 32]> {
        if self.kind != TYPE_UNSHIELDED {
            return Err(error::invalid_address(format!(
                "this wallet moves unshielded NIGHT, and `mn_{}` is not an \
                 unshielded (mn_addr…) address",
                self.kind
            )));
        }
        self.payload
            .clone()
            .try_into()
            .map_err(|_| error::invalid_address("an unshielded address is 32 bytes"))
    }
}

impl std::fmt::Display for MidnightAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.to_bech32m() {
            Ok(text) => f.write_str(&text),
            Err(_) => f.write_str("<invalid midnight address>"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BYTES: [u8; 32] = [0x11; 32];

    #[test]
    fn mainnet_omits_the_network_segment_and_others_carry_it() {
        assert!(MidnightAddress::unshielded(NetworkId::Mainnet, BYTES)
            .to_bech32m()
            .unwrap()
            .starts_with("mn_addr1"));
        assert!(
            MidnightAddress::unshielded(NetworkId::Named("preview".into()), BYTES)
                .to_bech32m()
                .unwrap()
                .starts_with("mn_addr_preview1")
        );
        assert!(
            MidnightAddress::unshielded(NetworkId::Named("test".into()), BYTES)
                .to_bech32m()
                .unwrap()
                .starts_with("mn_addr_test1")
        );
    }

    #[test]
    fn addresses_round_trip() {
        for network in [
            NetworkId::Mainnet,
            NetworkId::Named("preview".into()),
            NetworkId::Named("devnet".into()),
        ] {
            let address = MidnightAddress::unshielded(network, BYTES);
            let parsed = MidnightAddress::parse(&address.to_bech32m().unwrap()).unwrap();
            assert_eq!(parsed, address);
            assert_eq!(parsed.unshielded_bytes().unwrap(), BYTES);
        }
    }

    /// The whole reason the prefix matters: a preview address and a mainnet
    /// one hold identical bytes and differ only here.
    #[test]
    fn the_same_bytes_render_differently_per_network() {
        let mainnet = MidnightAddress::unshielded(NetworkId::Mainnet, BYTES)
            .to_bech32m()
            .unwrap();
        let preview = MidnightAddress::unshielded(NetworkId::Named("preview".into()), BYTES)
            .to_bech32m()
            .unwrap();
        assert_ne!(mainnet, preview);
        assert_eq!(
            MidnightAddress::parse(&mainnet).unwrap().payload,
            MidnightAddress::parse(&preview).unwrap().payload
        );
        // And they parse back to different networks, which is what a send checks.
        assert_eq!(
            MidnightAddress::parse(&mainnet).unwrap().network,
            NetworkId::Mainnet
        );
        assert_eq!(
            MidnightAddress::parse(&preview).unwrap().network,
            NetworkId::Named("preview".into())
        );
    }

    #[test]
    fn network_names_fold_to_the_segments_the_sdk_emits() {
        assert_eq!(NetworkId::parse("mainnet").unwrap(), NetworkId::Mainnet);
        assert_eq!(
            NetworkId::parse("dev").unwrap(),
            NetworkId::Named("devnet".into())
        );
        assert_eq!(
            NetworkId::parse("testnet").unwrap(),
            NetworkId::Named("test".into())
        );
        assert_eq!(
            NetworkId::parse("preview").unwrap(),
            NetworkId::Named("preview".into())
        );
    }

    #[test]
    fn a_name_that_could_not_be_a_prefix_is_refused() {
        // Baking one of these in would produce an address nothing can decode.
        for bad in [
            "",
            "PREVIEW",
            "my_network",
            "https://indexer.example/graphql",
        ] {
            assert!(NetworkId::parse(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn wallet_network_rows_map_to_their_sdk_segment() {
        assert_eq!(
            NetworkId::of(&crate::network::MIDNIGHT_PREVIEW).unwrap(),
            NetworkId::Named("preview".into())
        );
        assert_eq!(
            NetworkId::of(&crate::network::MIDNIGHT_DEVNET).unwrap(),
            NetworkId::Named("devnet".into())
        );
    }

    #[test]
    fn a_shielded_address_is_not_accepted_where_an_unshielded_one_is_needed() {
        let shielded = MidnightAddress {
            kind: TYPE_SHIELDED.into(),
            network: NetworkId::Named("preview".into()),
            payload: vec![0u8; 64],
        };
        let text = shielded.to_bech32m().unwrap();
        assert!(text.starts_with("mn_shield-addr_preview1"));

        let err = MidnightAddress::parse(&text)
            .unwrap()
            .unshielded_bytes()
            .unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAddress);
        assert!(err.message.contains("unshielded"), "{}", err.message);
    }

    #[test]
    fn payloads_of_the_wrong_length_are_refused() {
        let text =
            bech32::encode::<Bech32m>(Hrp::parse("mn_addr_preview").unwrap(), &[0u8; 20]).unwrap();
        let err = MidnightAddress::parse(&text).unwrap_err();
        assert!(err.message.contains("32 bytes"), "{}", err.message);
    }

    #[test]
    fn junk_and_foreign_prefixes_are_refused() {
        for bad in ["", "not an address", "addr_test1qq8ac7qq"] {
            assert!(
                MidnightAddress::parse(bad).is_err(),
                "{bad} should be refused"
            );
        }
        // Valid bech32m, wrong family.
        let foreign =
            bech32::encode::<Bech32m>(Hrp::parse("xx_addr").unwrap(), &[0u8; 32]).unwrap();
        let err = MidnightAddress::parse(&foreign).unwrap_err();
        assert!(err.message.contains("not a Midnight"), "{}", err.message);
    }

    #[test]
    fn a_corrupted_address_fails_the_bech32m_checksum() {
        let good = MidnightAddress::unshielded(NetworkId::Named("preview".into()), BYTES)
            .to_bech32m()
            .unwrap();
        let mut broken: Vec<char> = good.chars().collect();
        let last = broken.len() - 1;
        broken[last] = if broken[last] == 'q' { 'p' } else { 'q' };
        assert!(MidnightAddress::parse(&broken.into_iter().collect::<String>()).is_err());
    }
}
