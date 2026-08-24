//! Solana: SLIP-0010 ed25519 keys, legacy transactions, JSON-RPC.

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

use keys::SolanaAccount;

pub struct SolanaChain;

impl Chain for SolanaChain {
    fn id(&self) -> ChainId {
        ChainId::Solana
    }

    fn name(&self) -> &'static str {
        "Solana"
    }

    fn networks(&self) -> &'static [Network] {
        use std::sync::OnceLock;
        static ROWS: OnceLock<Vec<Network>> = OnceLock::new();
        ROWS.get_or_init(|| crate::network::for_chain(ChainId::Solana))
    }

    fn units(&self, network: &Network) -> Amount {
        network.units()
    }

    fn derivation_path(&self, index: u32) -> String {
        keys::path(index)
    }

    fn derive(&self, seed: &Seed, index: u32) -> Result<DerivedAccount> {
        let account = SolanaAccount::from_seed(&seed.bip39_seed(), index)?;
        Ok(describe(account))
    }

    fn account_from_secret(&self, secret: &str) -> Result<DerivedAccount> {
        Ok(describe(SolanaAccount::from_secret(secret)?))
    }

    fn signer(&self, secret: &str) -> Result<Box<dyn Signer>> {
        Ok(Box::new(SolanaAccount::from_secret(secret)?))
    }

    fn check_address(&self, _network: &Network, address: &str) -> Result<()> {
        // Solana addresses carry no network marker at all — the same 32 bytes
        // are a valid address on devnet, testnet and mainnet alike. There is
        // nothing to cross-check, which is worth knowing: this is the one
        // chain here where pasting a mainnet address into a devnet send
        // cannot be caught by looking at it.
        keys::address_to_bytes(address)?;
        Ok(())
    }

    fn recover_message(
        &self,
        message: &[u8],
        signature: &[u8],
        identity: Option<&str>,
    ) -> Result<Recovered> {
        // ed25519 signatures do not reveal who made them, so verification has
        // to be told whose signature to expect.
        let Some(identity) = identity else {
            return Err(error::usage(
                "an ed25519 signature does not reveal who made it; pass --address \
                 to say whose signature this should be",
            ));
        };
        // A Solana address *is* the public key, so an address verifies
        // directly. A stored secret works too — the wallet hands one over for
        // the chains that cannot verify from an address, and Solana should not
        // be the odd one out that rejects it.
        let address = match keys::address_to_bytes(identity) {
            Ok(_) => identity.to_string(),
            Err(address_error) => match SolanaAccount::from_secret(identity) {
                Ok(account) => account.address(),
                // Report the address failure: an address is what a user
                // passing `--address` meant, and the key path is the wallet's
                // own business.
                Err(_) => return Err(address_error),
            },
        };
        let valid = keys::verify(&address, message, signature)?;
        Ok(Recovered {
            address: valid.then_some(address),
            valid,
        })
    }

    fn client(&self, config: &ClientConfig) -> Result<Arc<dyn ChainClient>> {
        Ok(Arc::new(client::SolanaClient::new(config)))
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            // Devnet and testnet both have one; mainnet refuses, which the
            // client reports rather than this table pretending otherwise.
            faucet: true,
            tokens: false,
            gas_limit: false,
            recoverable_signatures: false,
        }
    }
}

/// Render a derived account into the shape the store and the commands want.
fn describe(account: SolanaAccount) -> DerivedAccount {
    let address = account.address();
    DerivedAccount {
        public_key: address.clone(),
        derivation_path: account.path.clone(),
        extra: json!({
            // The byte array `solana-keygen` writes to id.json, so a user can
            // move this account into the official tooling without re-deriving.
            "keypair_json": account.keypair_json(),
        }),
        secret: account.secret_base58(),
        address,
    }
}

impl Signer for SolanaAccount {
    fn address(&self) -> String {
        SolanaAccount::address(self)
    }

    fn sign_message(&self, message: &[u8]) -> Result<Vec<u8>> {
        Ok(SolanaAccount::sign(self, message).to_vec())
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
        let derived = SolanaChain.derive(&seed(), 0).unwrap();
        let restored = SolanaChain.account_from_secret(&derived.secret).unwrap();
        assert_eq!(restored.address, derived.address);
        // Restored from a raw key, there is no path to report.
        assert!(restored.derivation_path.is_none());
        assert_eq!(derived.derivation_path.as_deref(), Some("m/44'/501'/0'/0'"));
    }

    #[test]
    fn the_public_key_and_the_address_are_the_same_string() {
        let derived = SolanaChain.derive(&seed(), 0).unwrap();
        assert_eq!(derived.public_key, derived.address);
    }

    #[test]
    fn signing_goes_through_the_trait_and_verifies() {
        let derived = SolanaChain.derive(&seed(), 0).unwrap();
        let signer = SolanaChain.signer(&derived.secret).unwrap();
        assert_eq!(signer.address(), derived.address);

        let signature = signer.sign_message(b"hello").unwrap();
        let checked = SolanaChain
            .recover_message(b"hello", &signature, Some(&derived.address))
            .unwrap();
        assert!(checked.valid);
        assert_eq!(checked.address.as_deref(), Some(derived.address.as_str()));
    }

    /// The wallet hands every non-recovering chain the account's secret, so
    /// Solana has to accept one even though an address would also do. Getting
    /// this wrong made `verify` fail on Solana and only Solana.
    #[test]
    fn verifying_accepts_either_an_address_or_a_stored_secret() {
        let derived = SolanaChain.derive(&seed(), 0).unwrap();
        let signer = SolanaChain.signer(&derived.secret).unwrap();
        let signature = signer.sign_message(b"hello").unwrap();

        for identity in [derived.address.as_str(), derived.secret.as_str()] {
            let checked = SolanaChain
                .recover_message(b"hello", &signature, Some(identity))
                .unwrap();
            assert!(checked.valid, "identity {identity} should verify");
            assert_eq!(checked.address.as_deref(), Some(derived.address.as_str()));
        }
    }

    #[test]
    fn an_identity_that_is_neither_an_address_nor_a_key_is_refused() {
        let err = SolanaChain
            .recover_message(b"hello", &[0u8; 64], Some("not an address"))
            .unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAddress);
    }

    #[test]
    fn verifying_without_an_address_says_why_rather_than_guessing() {
        let derived = SolanaChain.derive(&seed(), 0).unwrap();
        let signer = SolanaChain.signer(&derived.secret).unwrap();
        let signature = signer.sign_message(b"hello").unwrap();

        let err = SolanaChain
            .recover_message(b"hello", &signature, None)
            .unwrap_err();
        assert_eq!(err.code, error::Code::Usage);
        assert!(err.message.contains("--address"), "{}", err.message);
    }

    #[test]
    fn a_wrong_signature_is_invalid_rather_than_an_error() {
        let derived = SolanaChain.derive(&seed(), 0).unwrap();
        let signer = SolanaChain.signer(&derived.secret).unwrap();
        let signature = signer.sign_message(b"hello").unwrap();
        let checked = SolanaChain
            .recover_message(b"different", &signature, Some(&derived.address))
            .unwrap();
        assert!(!checked.valid);
        assert!(checked.address.is_none());
    }

    #[test]
    fn addresses_are_checked_the_same_on_every_cluster() {
        let derived = SolanaChain.derive(&seed(), 0).unwrap();
        for network in SolanaChain.networks() {
            assert!(SolanaChain.check_address(network, &derived.address).is_ok());
            assert!(SolanaChain.check_address(network, "not base58 !!").is_err());
        }
    }

    #[test]
    fn every_index_gives_a_distinct_address() {
        let addresses: std::collections::BTreeSet<String> = (0..5)
            .map(|i| SolanaChain.derive(&seed(), i).unwrap().address)
            .collect();
        assert_eq!(addresses.len(), 5);
    }
}
