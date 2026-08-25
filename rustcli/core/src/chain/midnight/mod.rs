//! Midnight: BIP-32 secp256k1 keys, BIP-340 Schnorr, bech32m addresses.
//!
//! Midnight has two asset worlds. **Night**, the unshielded native token,
//! works like an ordinary transparent ledger and is what this module
//! implements: amounts, sender and recipient are all public. **Zswap**
//! shielded tokens use a separate key hierarchy whose coin and encryption
//! public keys are derived inside the ledger's zero-knowledge machinery; this
//! wallet derives the shielded *seed* and stops there.
//!
//! So the zero-knowledge proving here — real, local, in process — is for the
//! DUST spend that pays a *fee*, not for private transfers. See [`send`].

pub mod address;
pub mod client;
pub mod dust;
pub mod indexer;
pub mod keys;
pub mod send;

use std::sync::Arc;

use serde_json::json;

use crate::chain::{
    Amount, Capabilities, Chain, ChainClient, ChainId, ClientConfig, DerivedAccount, Recovered,
    Seed, Signer,
};
use crate::error::{self, Result};
use crate::network::Network;

use address::{MidnightAddress, NetworkId};
use keys::MidnightAccount;

pub struct MidnightChain;

impl Chain for MidnightChain {
    fn id(&self) -> ChainId {
        ChainId::Midnight
    }

    fn name(&self) -> &'static str {
        "Midnight"
    }

    fn networks(&self) -> &'static [Network] {
        use std::sync::OnceLock;
        static ROWS: OnceLock<Vec<Network>> = OnceLock::new();
        ROWS.get_or_init(|| crate::network::for_chain(ChainId::Midnight))
    }

    fn units(&self, network: &Network) -> Amount {
        network.units()
    }

    /// A Midnight transfer moves NIGHT and pays its fee in DUST.
    fn fee_units(&self, _network: &Network) -> Amount {
        dust::DUST
    }

    fn derivation_path(&self, index: u32) -> String {
        keys::path(index)
    }

    fn derive(&self, seed: &Seed, index: u32) -> Result<DerivedAccount> {
        describe(MidnightAccount::from_seed(&seed.bip39_seed()[..], index)?)
    }

    fn account_from_secret(&self, secret: &str) -> Result<DerivedAccount> {
        describe(MidnightAccount::from_secret(secret)?)
    }

    fn signer(&self, secret: &str) -> Result<Box<dyn Signer>> {
        Ok(Box::new(MidnightAccount::from_secret(secret)?))
    }

    fn address_on(&self, network: &Network, secret: &str) -> Result<Option<String>> {
        // The network name is part of the bech32m prefix — and absent on
        // mainnet — so the same 32 bytes read as a different address on each.
        let account = MidnightAccount::from_secret(secret)?;
        Ok(Some(account.address(NetworkId::of(network)?).to_bech32m()?))
    }

    fn check_address(&self, network: &Network, address: &str) -> Result<()> {
        let parsed = MidnightAddress::parse(address)?;
        let expected = NetworkId::of(network)?;
        if parsed.network != expected {
            return Err(error::invalid_address(format!(
                "that address is for `{}`, but this wallet is on `{expected}`",
                parsed.network
            )));
        }
        Ok(())
    }

    fn recover_message(
        &self,
        message: &[u8],
        signature: &[u8],
        address: Option<&str>,
    ) -> Result<Recovered> {
        // A Midnight address is SHA-256 of the public key, so — as on Cardano —
        // a signature cannot be checked against an address alone. Verification
        // goes through a key the wallet holds.
        let Some(secret) = address else {
            return Err(error::usage(
                "a Midnight address is a hash of the signing key, so a signature \
                 cannot be checked against an address alone; pass --account to \
                 verify with a key this wallet holds",
            ));
        };
        let account = MidnightAccount::from_secret(secret)?;
        let valid = keys::verify(&account.verifying_key_bytes(), message, signature)?;
        Ok(Recovered {
            address: valid
                .then(|| account.address(NetworkId::Mainnet).to_bech32m().ok())
                .flatten(),
            valid,
        })
    }

    fn client(&self, config: &ClientConfig) -> Result<Arc<dyn ChainClient>> {
        Ok(Arc::new(client::MidnightClient::new(config)?))
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            faucet: false,
            // Unshielded tokens other than NIGHT show up in a balance, but
            // this wallet only moves NIGHT.
            tokens: false,
            gas_limit: false,
            recoverable_signatures: false,
        }
    }
}

fn describe(account: MidnightAccount) -> Result<DerivedAccount> {
    // The address shown is the preview one, since that is the network this
    // wallet ships as Midnight's default; the same bytes render per network.
    let preview = NetworkId::Named("preview".into());
    Ok(DerivedAccount {
        address: account.address(preview).to_bech32m()?,
        secret: account.secret_hex(),
        public_key: hex::encode(account.verifying_key_bytes()),
        derivation_path: account.path.clone(),
        extra: json!({
            "address_devnet": account
                .address(NetworkId::Named("devnet".into()))
                .to_bech32m()?,
            "address_mainnet": account.address(NetworkId::Mainnet).to_bech32m()?,
            "address_bytes": hex::encode(account.address_bytes()),
            // Absent for an account imported as a bare night key, which is
            // exactly the account that cannot pay a fee.
            "dust_seed": account.dust_seed().ok().map(hex::encode),
        }),
    })
}

impl Signer for MidnightAccount {
    fn address(&self) -> String {
        MidnightAccount::address(self, NetworkId::Named("preview".into()))
            .to_bech32m()
            .unwrap_or_else(|_| "<invalid midnight address>".into())
    }

    fn sign_message(&self, message: &[u8]) -> Result<Vec<u8>> {
        Ok(MidnightAccount::sign(self, message).to_vec())
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

    /// The network name is part of the address prefix — and absent on mainnet
    /// — so one key reads as three different addresses. An export has to say
    /// which one it means.
    #[test]
    fn the_address_differs_between_networks() {
        let derived = MidnightChain.derive(&seed(), 0).unwrap();
        let preview = crate::network::find("midnight-preview").unwrap();
        let devnet = crate::network::find("midnight-devnet").unwrap();

        let on_preview = MidnightChain
            .address_on(&preview, &derived.secret)
            .unwrap()
            .expect("midnight renders per network");
        let on_devnet = MidnightChain
            .address_on(&devnet, &derived.secret)
            .unwrap()
            .expect("midnight renders per network");

        assert_eq!(on_preview, derived.address, "preview is the stored one");
        assert!(on_preview.starts_with("mn_addr_preview1"));
        assert!(on_devnet.starts_with("mn_addr_dev"));
        assert_ne!(on_preview, on_devnet);
    }

    #[test]
    fn a_derived_account_round_trips_through_its_stored_secret() {
        let derived = MidnightChain.derive(&seed(), 0).unwrap();
        let restored = MidnightChain.account_from_secret(&derived.secret).unwrap();
        assert_eq!(restored.address, derived.address);
        assert_eq!(restored.extra["dust_seed"], derived.extra["dust_seed"]);
        assert_eq!(
            derived.derivation_path.as_deref(),
            Some("m/44'/2400'/0'/0/0")
        );
    }

    #[test]
    fn the_derived_address_matches_the_official_sdk() {
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("../../../../../testvectors/multichain.json"))
                .unwrap();
        let derived = MidnightChain.derive(&seed(), 0).unwrap();
        let expected = &vectors["midnight"]["accounts"][0];
        assert_eq!(derived.extra["address_mainnet"], expected["addr_mainnet"]);
        assert_eq!(
            derived.public_key,
            expected["verifying_key_hex"].as_str().unwrap()
        );
        assert_eq!(derived.extra["address_bytes"], expected["address_hex"]);
    }

    #[test]
    fn the_default_rendering_is_the_preview_address() {
        let derived = MidnightChain.derive(&seed(), 0).unwrap();
        assert!(
            derived.address.starts_with("mn_addr_preview1"),
            "{}",
            derived.address
        );
        assert!(derived.extra["address_mainnet"]
            .as_str()
            .unwrap()
            .starts_with("mn_addr1"));
    }

    #[test]
    fn an_address_for_another_midnight_network_is_refused() {
        let derived = MidnightChain.derive(&seed(), 0).unwrap();
        let devnet = derived.extra["address_devnet"].as_str().unwrap();

        assert!(MidnightChain
            .check_address(&crate::network::MIDNIGHT_PREVIEW, &derived.address)
            .is_ok());
        assert!(MidnightChain
            .check_address(&crate::network::MIDNIGHT_DEVNET, devnet)
            .is_ok());

        let err = MidnightChain
            .check_address(&crate::network::MIDNIGHT_PREVIEW, devnet)
            .unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAddress);
        assert!(err.message.contains("devnet"), "{}", err.message);
    }

    #[test]
    fn signing_goes_through_the_trait_and_verifies_with_the_account() {
        let derived = MidnightChain.derive(&seed(), 0).unwrap();
        let signer = MidnightChain.signer(&derived.secret).unwrap();
        assert_eq!(signer.address(), derived.address);

        let signature = signer.sign_message(b"hello").unwrap();
        assert!(
            MidnightChain
                .recover_message(b"hello", &signature, Some(&derived.secret))
                .unwrap()
                .valid
        );
        assert!(
            !MidnightChain
                .recover_message(b"different", &signature, Some(&derived.secret))
                .unwrap()
                .valid
        );
    }

    #[test]
    fn verifying_with_nothing_to_verify_against_explains_why() {
        let err = MidnightChain
            .recover_message(b"hello", &[0u8; 64], None)
            .unwrap_err();
        assert_eq!(err.code, error::Code::Usage);
        assert!(err.message.contains("hash"), "{}", err.message);
    }

    #[test]
    fn every_index_gives_a_distinct_address_and_dust_key() {
        let derived: Vec<_> = (0..4)
            .map(|i| MidnightChain.derive(&seed(), i).unwrap())
            .collect();
        let addresses: std::collections::BTreeSet<&str> =
            derived.iter().map(|d| d.address.as_str()).collect();
        assert_eq!(addresses.len(), 4);
        let dust: std::collections::BTreeSet<&str> = derived
            .iter()
            .map(|d| d.extra["dust_seed"].as_str().unwrap())
            .collect();
        assert_eq!(
            dust.len(),
            4,
            "each index pays its fees from its own DUST key"
        );
    }
}
