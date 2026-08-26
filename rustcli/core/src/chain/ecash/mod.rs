//! eCash: BIP-44 secp256k1 keys, CashAddr addresses, Bitcoin-format
//! transactions with BIP-143 signatures, Chronik for chain access.
//!
//! The chain most people here will already know the shape of — it is
//! Bitcoin's, with three differences that matter to a wallet.
//!
//! **XEC has two decimal places, not eight.** eCash redenominated at the 2021
//! rebrand: one XEC is a hundred satoshis, where one BCH was a hundred
//! million. A balance read with Bitcoin's decimals is off by a factor of a
//! million in the direction that makes a large transfer look small.
//!
//! **Its signatures carry `SIGHASH_FORKID`.** That is what keeps a
//! transaction built here off Bitcoin's chain and Bitcoin's off eCash's, and
//! it comes with BIP-143's digest, which commits to the value of the input
//! being spent. See [`tx`].
//!
//! **Its addresses name their network.** A CashAddr prefix is inside the
//! checksum, so `ecash:` and `ectest:` are not two labels on one string —
//! neither decodes as the other. See [`address`].
//!
//! What is *not* here is eTokens. eCash carries SLP-style tokens as
//! annotations on ordinary outputs, so an output holding one is spendable in
//! the plain sense and burns the token when it is spent. This wallet reads
//! and moves XEC only, and refuses to touch those outputs at all rather than
//! quietly destroying what they carry — see
//! [`chronik::ScriptUtxos::spendable`].

pub mod address;
pub mod chronik;
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

use address::{Address, EcashNetwork};
use keys::EcashAccount;

pub struct EcashChain;

impl Chain for EcashChain {
    fn id(&self) -> ChainId {
        ChainId::Ecash
    }

    fn name(&self) -> &'static str {
        "eCash"
    }

    fn networks(&self) -> &'static [Network] {
        use std::sync::OnceLock;
        static ROWS: OnceLock<Vec<Network>> = OnceLock::new();
        ROWS.get_or_init(|| crate::network::for_chain(ChainId::Ecash))
    }

    fn units(&self, network: &Network) -> Amount {
        network.units()
    }

    fn derivation_path(&self, index: u32) -> String {
        keys::path(index)
    }

    fn derive(&self, seed: &Seed, index: u32) -> Result<DerivedAccount> {
        Ok(describe(EcashAccount::from_seed(seed, index)?))
    }

    fn account_from_secret(&self, secret: &str) -> Result<DerivedAccount> {
        Ok(describe(EcashAccount::from_secret(secret)?))
    }

    fn signer(&self, secret: &str) -> Result<Box<dyn Signer>> {
        Ok(Box::new(EcashAccount::from_secret(secret)?))
    }

    fn address_on(&self, network: &Network, secret: &str) -> Result<Option<String>> {
        // The prefix is inside the checksum, so this is a different string per
        // network rather than the same one relabelled.
        let account = EcashAccount::from_secret(secret)?;
        Ok(Some(
            account.address(EcashNetwork::of(network)).to_cashaddr(),
        ))
    }

    fn check_address(&self, network: &Network, address: &str) -> Result<()> {
        let parsed = Address::parse(address)?;
        let expected = EcashNetwork::of(network);
        if parsed.network != expected {
            return Err(error::invalid_address(format!(
                "that is an `{}:` address, but this wallet is on {}",
                parsed.network.prefix(),
                network.name
            )));
        }
        Ok(())
    }

    fn recover_message(
        &self,
        network: &Network,
        message: &[u8],
        signature: &[u8],
        identity: Option<&str>,
    ) -> Result<Recovered> {
        // secp256k1, so the signer is named by the signature itself and an
        // identity is only ever something to compare against.
        let recovered = keys::recover_message(message, signature)?;

        // Which prefix to render the answer with. An address given to compare
        // against settles it; otherwise the network in play does, so a
        // recovered address can be read straight against what the wallet is
        // showing for its accounts on that network.
        let (expected, render_on) = match identity {
            None => (None, EcashNetwork::of(network)),
            Some(given) => match Address::parse(given) {
                Ok(parsed) => (Some(parsed.hash), parsed.network),
                // Not an address: a key this wallet holds is the other thing
                // callers pass, and it names a hash just as well.
                Err(address_error) => match EcashAccount::from_secret(given) {
                    Ok(account) => (Some(account.hash160()), EcashNetwork::of(network)),
                    Err(_) => return Err(address_error),
                },
            },
        };

        Ok(Recovered {
            address: Some(Address::p2pkh(render_on, recovered).to_cashaddr()),
            valid: expected.map(|hash| hash == recovered).unwrap_or(true),
        })
    }

    fn client(&self, config: &ClientConfig) -> Result<Arc<dyn ChainClient>> {
        Ok(Arc::new(client::EcashClient::new(config)))
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            faucet: false,
            // eTokens exist; this wallet reads and moves XEC only, and skips
            // the outputs carrying them rather than burning what it cannot
            // account for.
            tokens: false,
            // The fee is satoshis per byte of a transaction whose size the
            // wallet already knows. There is no unit of work to cap.
            gas_limit: false,
            recoverable_signatures: true,
        }
    }
}

/// Render a derived account into the shape the store and the commands want.
///
/// The stored address is the testnet one, because testnet is this chain's
/// default network; the chain layer re-renders for whichever network is in
/// play, since the key material is the same on both and only the prefix and
/// its checksum differ.
fn describe(account: EcashAccount) -> DerivedAccount {
    let path = account.path.clone();
    DerivedAccount {
        address: account.address(EcashNetwork::Testnet).to_cashaddr(),
        secret: account.secret_hex(),
        public_key: hex::encode(account.public_key()),
        derivation_path: path,
        extra: json!({
            "address_mainnet": account.address(EcashNetwork::Mainnet).to_cashaddr(),
            "public_key_hash": hex::encode(account.hash160()),
            // What eCash's own wallets import, so an account made here can be
            // opened in Electrum ABC without a conversion step.
            "wif": account.wif(EcashNetwork::Testnet),
            "wif_mainnet": account.wif(EcashNetwork::Mainnet),
        }),
    }
}

impl Signer for EcashAccount {
    fn address(&self) -> String {
        EcashAccount::address(self, EcashNetwork::Testnet).to_cashaddr()
    }

    fn sign_message(&self, message: &[u8]) -> Result<Vec<u8>> {
        Ok(EcashAccount::sign_message(self, message)?.to_vec())
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
        let derived = EcashChain.derive(&seed(), 0).unwrap();
        let restored = EcashChain.account_from_secret(&derived.secret).unwrap();
        assert_eq!(restored.address, derived.address);
        assert_eq!(restored.public_key, derived.public_key);
        assert!(restored.derivation_path.is_none());
        assert_eq!(
            derived.derivation_path.as_deref(),
            Some("m/44'/1899'/0'/0/0")
        );
    }

    /// The prefix is inside the checksum, so an export that wrote one address
    /// under both names would send mainnet funds to a string nothing on
    /// mainnet answers to.
    #[test]
    fn the_address_differs_between_mainnet_and_testnet() {
        let derived = EcashChain.derive(&seed(), 0).unwrap();
        let testnet = crate::network::ECASH_TESTNET;
        let mainnet = crate::network::ECASH_MAINNET;

        let on_testnet = EcashChain
            .address_on(&testnet, &derived.secret)
            .unwrap()
            .expect("ecash renders per network");
        let on_mainnet = EcashChain
            .address_on(&mainnet, &derived.secret)
            .unwrap()
            .expect("ecash renders per network");

        assert_eq!(
            on_testnet, derived.address,
            "the testnet one is the stored one"
        );
        assert!(on_testnet.starts_with("ectest:"));
        assert!(on_mainnet.starts_with("ecash:"));
        assert_eq!(on_mainnet, derived.extra["address_mainnet"]);
    }

    #[test]
    fn an_address_from_the_wrong_network_is_refused() {
        let derived = EcashChain.derive(&seed(), 0).unwrap();
        let testnet = derived.address.as_str();
        let mainnet = derived.extra["address_mainnet"].as_str().unwrap();

        assert!(EcashChain
            .check_address(&crate::network::ECASH_TESTNET, testnet)
            .is_ok());
        assert!(EcashChain
            .check_address(&crate::network::ECASH_MAINNET, mainnet)
            .is_ok());

        let err = EcashChain
            .check_address(&crate::network::ECASH_TESTNET, mainnet)
            .unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAddress);
        assert!(err.message.contains("ecash"), "{}", err.message);
    }

    #[test]
    fn signing_names_the_signer_without_being_told_who_to_expect() {
        let derived = EcashChain.derive(&seed(), 0).unwrap();
        let signer = EcashChain.signer(&derived.secret).unwrap();
        assert_eq!(signer.address(), derived.address);

        let signature = signer.sign_message(b"hello causewaybay").unwrap();
        let recovered = EcashChain
            .recover_message(
                &crate::network::ECASH_TESTNET,
                b"hello causewaybay",
                &signature,
                None,
            )
            .unwrap();
        assert!(recovered.valid);
        assert_eq!(recovered.address.as_deref(), Some(derived.address.as_str()));
        assert!(EcashChain.capabilities().recoverable_signatures);
    }

    /// The comparison is over the key hash, not the string, so an account
    /// checked against its mainnet address is still its own signature.
    #[test]
    fn a_signature_checks_against_either_of_its_two_addresses() {
        let derived = EcashChain.derive(&seed(), 0).unwrap();
        let signature = EcashChain
            .signer(&derived.secret)
            .unwrap()
            .sign_message(b"hello")
            .unwrap();
        let mainnet = derived.extra["address_mainnet"].as_str().unwrap();

        for identity in [derived.address.as_str(), mainnet, derived.secret.as_str()] {
            let checked = EcashChain
                .recover_message(
                    &crate::network::ECASH_TESTNET,
                    b"hello",
                    &signature,
                    Some(identity),
                )
                .unwrap();
            assert!(checked.valid, "{identity}");
        }
        // And the answer is rendered on the network it was asked about.
        let on_mainnet = EcashChain
            .recover_message(
                &crate::network::ECASH_TESTNET,
                b"hello",
                &signature,
                Some(mainnet),
            )
            .unwrap();
        assert_eq!(on_mainnet.address.as_deref(), Some(mainnet));
    }

    #[test]
    fn a_signature_from_someone_else_is_invalid_but_still_names_them() {
        let mine = EcashChain.derive(&seed(), 0).unwrap();
        let theirs = EcashChain.derive(&seed(), 1).unwrap();
        let signature = EcashChain
            .signer(&theirs.secret)
            .unwrap()
            .sign_message(b"hello")
            .unwrap();

        let checked = EcashChain
            .recover_message(
                &crate::network::ECASH_TESTNET,
                b"hello",
                &signature,
                Some(&mine.address),
            )
            .unwrap();
        assert!(!checked.valid);
        assert_eq!(checked.address.as_deref(), Some(theirs.address.as_str()));
    }

    #[test]
    fn an_identity_that_is_neither_an_address_nor_a_key_says_so() {
        let derived = EcashChain.derive(&seed(), 0).unwrap();
        let signature = EcashChain
            .signer(&derived.secret)
            .unwrap()
            .sign_message(b"hello")
            .unwrap();
        let err = EcashChain
            .recover_message(
                &crate::network::ECASH_TESTNET,
                b"hello",
                &signature,
                Some("not-an-address"),
            )
            .unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAddress);
    }

    /// XEC is quoted in two places, not Bitcoin's eight. Reading a balance
    /// with the wrong one is a factor of a million.
    #[test]
    fn the_token_is_counted_in_two_places() {
        for network in EcashChain.networks() {
            let units = EcashChain.units(network);
            assert_eq!(units.decimals, 2, "{}", network.key);
            assert_eq!(units.format(546), "5.46");
            assert_eq!(units.format(100), "1");
        }
    }

    #[test]
    fn every_index_gives_a_distinct_address() {
        let addresses: std::collections::BTreeSet<String> = (0..4)
            .map(|i| EcashChain.derive(&seed(), i).unwrap().address)
            .collect();
        assert_eq!(addresses.len(), 4);
    }
}
