//! The EVM chain — Cronos, and anything else speaking the same JSON-RPC.
//!
//! This is an adapter rather than a reimplementation. The key handling
//! ([`crate::wallet`]), the RLP transaction ([`crate::tx`]) and the JSON-RPC
//! client ([`crate::rpc`]) predate the multi-chain layer and are still where
//! the work happens; what lives here is the translation between them and the
//! [`Chain`] traits, so EVM is one chain among four rather than the special
//! case everything else is measured against.

pub mod client;

use std::sync::Arc;

use alloy_primitives::U256;
use serde_json::json;

use crate::chain::{
    Amount, Capabilities, Chain, ChainClient, ChainId, ClientConfig, DerivedAccount, Recovered,
    Seed, Signer,
};
use crate::error::{self, Result};
use crate::network::Network;
use crate::wallet::{self, Keypair};

pub struct EvmChain;

impl Chain for EvmChain {
    fn id(&self) -> ChainId {
        ChainId::Evm
    }

    fn name(&self) -> &'static str {
        "Cronos EVM"
    }

    fn networks(&self) -> &'static [Network] {
        use std::sync::OnceLock;
        static ROWS: OnceLock<Vec<Network>> = OnceLock::new();
        ROWS.get_or_init(|| crate::network::for_chain(ChainId::Evm))
    }

    fn units(&self, network: &Network) -> Amount {
        network.units()
    }

    fn derivation_path(&self, index: u32) -> String {
        crate::bip32::ethereum_path(index)
    }

    fn derive(&self, seed: &Seed, index: u32) -> Result<DerivedAccount> {
        let keypair = Keypair::from_mnemonic(seed.phrase(), index, seed.passphrase())?;
        Ok(describe(&keypair, Some(self.derivation_path(index))))
    }

    fn account_from_secret(&self, secret: &str) -> Result<DerivedAccount> {
        Ok(describe(&Keypair::from_hex(secret)?, None))
    }

    fn signer(&self, secret: &str) -> Result<Box<dyn Signer>> {
        Ok(Box::new(Keypair::from_hex(secret)?))
    }

    fn check_address(&self, _network: &Network, address: &str) -> Result<()> {
        // EVM addresses carry no network marker — the same 20 bytes are valid
        // on every chain id — so there is nothing here to cross-check. The
        // replay protection lives in the signature (EIP-155), not the address.
        wallet::parse_address(address)?;
        Ok(())
    }

    fn recover_message(
        &self,
        _network: &Network,
        message: &[u8],
        signature: &[u8],
        address: Option<&str>,
    ) -> Result<Recovered> {
        // secp256k1 signatures carry a recovery id, so this is the one chain
        // here that can name the signer without being told who to expect.
        let recovered = wallet::recover_message(message, signature)?;
        let expected = address.map(wallet::parse_address).transpose()?;
        Ok(Recovered {
            valid: expected.map(|e| e == recovered).unwrap_or(true),
            address: Some(recovered.to_checksum(None)),
        })
    }

    fn client(&self, config: &ClientConfig) -> Result<Arc<dyn ChainClient>> {
        Ok(Arc::new(client::EvmClient::new(config)?))
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            faucet: false,
            tokens: true,
            gas_limit: true,
            recoverable_signatures: true,
        }
    }
}

fn describe(keypair: &Keypair, path: Option<String>) -> DerivedAccount {
    DerivedAccount {
        address: keypair.address().to_checksum(None),
        secret: keypair.private_key_hex(),
        public_key: keypair.public_key_hex(),
        derivation_path: path,
        extra: json!({
            "public_key_compressed": keypair.public_key_compressed_hex(),
        }),
    }
}

impl Signer for Keypair {
    fn address(&self) -> String {
        Keypair::address(self).to_checksum(None)
    }

    fn sign_message(&self, message: &[u8]) -> Result<Vec<u8>> {
        Ok(Keypair::sign_message(self, message)?.to_vec())
    }
}

/// Widen a `U256` the RPC returned into the `u128` the chain layer speaks.
///
/// Every real balance and fee fits comfortably; a value that does not is a
/// node returning nonsense, and saying so beats silently truncating it.
pub(crate) fn to_u128(value: U256, what: &str) -> Result<u128> {
    u128::try_from(value)
        .map_err(|_| error::rpc_error(format!("{what} is too large to be a real value")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHRASE: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn seed() -> Seed {
        Seed::new(PHRASE, "").unwrap()
    }

    /// The addresses every Ethereum tool produces for this phrase. They were
    /// already covered before the chain layer existed; going through the trait
    /// must not change them.
    #[test]
    fn derivation_still_produces_the_well_known_addresses() {
        let expected = [
            "0x9858EfFD232B4033E47d90003D41EC34EcaEda94",
            "0x6Fac4D18c912343BF86fa7049364Dd4E424Ab9C0",
            "0xb6716976A3ebe8D39aCEB04372f22Ff8e6802D7A",
        ];
        for (index, want) in expected.iter().enumerate() {
            let derived = EvmChain.derive(&seed(), index as u32).unwrap();
            assert_eq!(derived.address, *want, "index {index}");
        }
        assert_eq!(
            EvmChain.derive(&seed(), 0).unwrap().secret,
            "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727"
        );
    }

    #[test]
    fn the_path_is_the_bip44_ethereum_one() {
        assert_eq!(EvmChain.derivation_path(0), "m/44'/60'/0'/0/0");
        assert_eq!(
            EvmChain
                .derive(&seed(), 5)
                .unwrap()
                .derivation_path
                .as_deref(),
            Some("m/44'/60'/0'/0/5")
        );
    }

    #[test]
    fn a_derived_account_round_trips_through_its_stored_secret() {
        let derived = EvmChain.derive(&seed(), 1).unwrap();
        let restored = EvmChain.account_from_secret(&derived.secret).unwrap();
        assert_eq!(restored.address, derived.address);
        assert_eq!(restored.public_key, derived.public_key);
        assert!(restored.derivation_path.is_none());
    }

    #[test]
    fn signing_and_recovery_name_the_signer_without_being_told() {
        let derived = EvmChain.derive(&seed(), 0).unwrap();
        let signer = EvmChain.signer(&derived.secret).unwrap();
        assert_eq!(signer.address(), derived.address);

        let signature = signer.sign_message(b"hello causewaybay").unwrap();
        let recovered = EvmChain
            .recover_message(
                &crate::network::CRONOS_TESTNET,
                b"hello causewaybay",
                &signature,
                None,
            )
            .unwrap();
        // No address was supplied and it still knows who signed: this is what
        // `capabilities().recoverable_signatures` claims.
        assert!(recovered.valid);
        assert_eq!(recovered.address.as_deref(), Some(derived.address.as_str()));
        assert!(EvmChain.capabilities().recoverable_signatures);
    }

    #[test]
    fn a_signature_from_someone_else_is_invalid_but_still_names_them() {
        let mine = EvmChain.derive(&seed(), 0).unwrap();
        let theirs = EvmChain.derive(&seed(), 1).unwrap();
        let signature = EvmChain
            .signer(&theirs.secret)
            .unwrap()
            .sign_message(b"hello")
            .unwrap();

        let checked = EvmChain
            .recover_message(
                &crate::network::CRONOS_TESTNET,
                b"hello",
                &signature,
                Some(&mine.address),
            )
            .unwrap();
        assert!(!checked.valid);
        assert_eq!(checked.address.as_deref(), Some(theirs.address.as_str()));
    }

    #[test]
    fn addresses_are_accepted_on_both_networks_and_checksummed_on_the_way_in() {
        let derived = EvmChain.derive(&seed(), 0).unwrap();
        for network in EvmChain.networks() {
            assert!(EvmChain.check_address(network, &derived.address).is_ok());
            assert!(EvmChain
                .check_address(network, &derived.address.to_lowercase())
                .is_ok());
            assert!(EvmChain.check_address(network, "0x123").is_err());
        }
    }

    #[test]
    fn an_oversized_quantity_is_reported_rather_than_truncated() {
        assert_eq!(to_u128(U256::from(42), "balance").unwrap(), 42);
        let err = to_u128(U256::MAX, "balance").unwrap_err();
        assert_eq!(err.code, error::Code::RpcError);
        assert!(err.message.contains("balance"), "{}", err.message);
    }
}
