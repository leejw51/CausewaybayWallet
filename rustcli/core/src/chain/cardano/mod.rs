//! Cardano: Icarus/CIP-3 extended keys, CIP-1852 paths, CIP-19 addresses,
//! Shelley-era CBOR transactions, Koios for chain access.

pub mod address;
pub mod client;
pub mod keys;
pub mod tx;

use std::sync::Arc;

use serde_json::json;

use crate::chain::{
    Amount, Capabilities, Chain, ChainClient, ChainId, ClientConfig, DerivedAccount, Recovered,
    Seed, Signer,
};
use crate::error::{self, Result};
use crate::network::Network;

use address::{Address, CardanoNetwork};
use keys::CardanoAccount;

pub struct CardanoChain;

impl Chain for CardanoChain {
    fn id(&self) -> ChainId {
        ChainId::Cardano
    }

    fn name(&self) -> &'static str {
        "Cardano"
    }

    fn networks(&self) -> &'static [Network] {
        use std::sync::OnceLock;
        static ROWS: OnceLock<Vec<Network>> = OnceLock::new();
        ROWS.get_or_init(|| crate::network::for_chain(ChainId::Cardano))
    }

    fn units(&self, network: &Network) -> Amount {
        network.units()
    }

    fn derivation_path(&self, index: u32) -> String {
        keys::path(index)
    }

    fn derive(&self, seed: &Seed, index: u32) -> Result<DerivedAccount> {
        // The one chain here that hashes the entropy rather than the seed.
        let entropy = seed.entropy()?;
        let account = CardanoAccount::from_entropy(&entropy, seed.passphrase(), index)?;
        Ok(describe(account))
    }

    fn account_from_secret(&self, secret: &str) -> Result<DerivedAccount> {
        Ok(describe(CardanoAccount::from_secret(secret)?))
    }

    fn signer(&self, secret: &str) -> Result<Box<dyn Signer>> {
        Ok(Box::new(CardanoAccount::from_secret(secret)?))
    }

    fn address_on(&self, network: &Network, secret: &str) -> Result<Option<String>> {
        // The network id is a nibble of the address header, so this is a
        // different string per network, not the same one relabelled.
        let account = CardanoAccount::from_secret(secret)?;
        Ok(Some(
            account
                .base_address(CardanoNetwork::of(network))
                .to_bech32(),
        ))
    }

    fn check_address(&self, network: &Network, address: &str) -> Result<()> {
        let parsed = Address::parse(address)?;
        let expected = CardanoNetwork::of(network);
        if parsed.network != expected {
            return Err(error::invalid_address(format!(
                "that is a {:?} address, but this wallet is on {}",
                parsed.network, network.name
            )));
        }
        Ok(())
    }

    fn recover_message(
        &self,
        network: &Network,
        message: &[u8],
        signature: &[u8],
        address: Option<&str>,
    ) -> Result<Recovered> {
        // Cardano addresses hold a *hash* of the public key, so a signature
        // cannot even be checked against an address on its own — the key it
        // hashes to has to come from somewhere. The wallet verifies against a
        // key it holds, which is why this asks for the account rather than an
        // address.
        let Some(secret) = address else {
            return Err(error::usage(
                "a Cardano address holds only a hash of the signing key, so a \
                 signature cannot be checked against an address alone; pass \
                 --account to verify with a key this wallet holds",
            ));
        };
        let account = CardanoAccount::from_secret(secret)?;
        let valid = keys::verify(&account.payment_public_key(), message, signature)?;
        Ok(Recovered {
            // Rendered for the network in play: the header nibble makes the
            // testnet and mainnet strings different addresses.
            address: valid.then(|| {
                account
                    .base_address(CardanoNetwork::of(network))
                    .to_bech32()
            }),
            valid,
        })
    }

    fn client(&self, config: &ClientConfig) -> Result<Arc<dyn ChainClient>> {
        Ok(Arc::new(client::CardanoClient::new(config)))
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            faucet: false,
            // Native tokens exist and the wallet deliberately skips UTxOs
            // carrying them rather than pretending to move them.
            tokens: false,
            gas_limit: false,
            recoverable_signatures: false,
        }
    }
}

/// Render a derived account into the shape the store and the commands want.
///
/// The address shown is the testnet base address; the chain layer re-renders
/// for whichever network is in play, because the key material is the same on
/// both and only the header nibble differs.
fn describe(account: CardanoAccount) -> DerivedAccount {
    DerivedAccount {
        address: account.base_address(CardanoNetwork::Testnet).to_bech32(),
        secret: account.secret_hex(),
        public_key: hex::encode(account.payment_public_key()),
        derivation_path: account.path.clone(),
        extra: json!({
            "address_mainnet": account.base_address(CardanoNetwork::Mainnet).to_bech32(),
            "reward_address": account.reward_address(CardanoNetwork::Testnet).to_bech32(),
            "reward_address_mainnet": account.reward_address(CardanoNetwork::Mainnet).to_bech32(),
            "payment_key_hash": hex::encode(account.payment_key_hash()),
            "stake_key_hash": hex::encode(account.stake_key_hash()),
            "stake_public_key": hex::encode(account.stake_public_key()),
        }),
    }
}

impl Signer for CardanoAccount {
    fn address(&self) -> String {
        self.base_address(CardanoNetwork::Testnet).to_bech32()
    }

    fn sign_message(&self, message: &[u8]) -> Result<Vec<u8>> {
        Ok(CardanoAccount::sign(self, message).to_vec())
    }
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
    fn a_derived_account_round_trips_through_its_stored_secret() {
        let derived = CardanoChain.derive(&seed(), 0).unwrap();
        let restored = CardanoChain.account_from_secret(&derived.secret).unwrap();
        assert_eq!(restored.address, derived.address);
        // The staking half survived, so the reward address is unchanged too.
        assert_eq!(
            restored.extra["reward_address"],
            derived.extra["reward_address"]
        );
        assert_eq!(
            derived.derivation_path.as_deref(),
            Some("m/1852'/1815'/0'/0/0")
        );
    }

    /// The network is a nibble of the address header, so the same key has two
    /// addresses. An export that wrote one of them under both names would send
    /// mainnet funds to a testnet address.
    #[test]
    fn the_address_differs_between_mainnet_and_testnet() {
        let derived = CardanoChain.derive(&seed(), 0).unwrap();
        let testnet = crate::network::find("cardano-preprod").unwrap();
        let mainnet = crate::network::find("cardano-mainnet").unwrap();

        let on_testnet = CardanoChain
            .address_on(&testnet, &derived.secret)
            .unwrap()
            .expect("cardano renders per network");
        let on_mainnet = CardanoChain
            .address_on(&mainnet, &derived.secret)
            .unwrap()
            .expect("cardano renders per network");

        assert_eq!(
            on_testnet, derived.address,
            "the testnet one is the stored one"
        );
        assert!(on_testnet.starts_with("addr_test1"));
        assert!(on_mainnet.starts_with("addr1"));
        assert_eq!(on_mainnet, derived.extra["address_mainnet"]);
    }

    #[test]
    fn the_derived_address_matches_the_official_sdk() {
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("../../../../../testvectors/multichain.json"))
                .unwrap();
        let derived = CardanoChain.derive(&seed(), 0).unwrap();
        assert_eq!(
            derived.address,
            vectors["cardano"]["accounts"][0]["base_addr_testnet"]
                .as_str()
                .unwrap()
        );
        assert_eq!(
            derived.extra["reward_address"],
            vectors["cardano"]["reward_addr_testnet"]
        );
    }

    #[test]
    fn an_address_from_the_wrong_network_is_refused() {
        let derived = CardanoChain.derive(&seed(), 0).unwrap();
        let testnet = derived.address.as_str();
        let mainnet = derived.extra["address_mainnet"].as_str().unwrap();

        assert!(CardanoChain
            .check_address(&crate::network::CARDANO_PREPROD, testnet)
            .is_ok());
        assert!(CardanoChain
            .check_address(&crate::network::CARDANO_MAINNET, mainnet)
            .is_ok());

        let err = CardanoChain
            .check_address(&crate::network::CARDANO_PREPROD, mainnet)
            .unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAddress);
        assert!(err.message.contains("Mainnet"), "{}", err.message);
    }

    #[test]
    fn preprod_and_preview_share_addresses() {
        // Both are testnet id 0, so an address is portable between them and
        // only the endpoint differs.
        let derived = CardanoChain.derive(&seed(), 0).unwrap();
        for network in [
            crate::network::CARDANO_PREPROD,
            crate::network::CARDANO_PREVIEW,
        ] {
            assert!(CardanoChain
                .check_address(&network, &derived.address)
                .is_ok());
        }
    }

    #[test]
    fn signing_goes_through_the_trait_and_verifies_with_the_account() {
        let derived = CardanoChain.derive(&seed(), 0).unwrap();
        let signer = CardanoChain.signer(&derived.secret).unwrap();
        assert_eq!(signer.address(), derived.address);

        let signature = signer.sign_message(b"hello").unwrap();
        let checked = CardanoChain
            .recover_message(
                &crate::network::CARDANO_PREPROD,
                b"hello",
                &signature,
                Some(&derived.secret),
            )
            .unwrap();
        assert!(checked.valid);

        let tampered = CardanoChain
            .recover_message(
                &crate::network::CARDANO_PREPROD,
                b"different",
                &signature,
                Some(&derived.secret),
            )
            .unwrap();
        assert!(!tampered.valid);
    }

    #[test]
    fn verifying_with_nothing_to_verify_against_explains_why() {
        let err = CardanoChain
            .recover_message(&crate::network::CARDANO_PREPROD, b"hello", &[0u8; 64], None)
            .unwrap_err();
        assert_eq!(err.code, error::Code::Usage);
        assert!(err.message.contains("hash"), "{}", err.message);
    }

    #[test]
    fn every_index_gives_a_distinct_address_but_one_reward_address() {
        let derived: Vec<_> = (0..4)
            .map(|i| CardanoChain.derive(&seed(), i).unwrap())
            .collect();
        let addresses: std::collections::BTreeSet<&str> =
            derived.iter().map(|d| d.address.as_str()).collect();
        assert_eq!(addresses.len(), 4);
        for account in &derived[1..] {
            assert_eq!(
                account.extra["reward_address"],
                derived[0].extra["reward_address"]
            );
        }
    }
}
