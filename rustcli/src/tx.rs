//! Legacy (type 0x0) transaction construction and EIP-155 signing.
//!
//! Cronos accepts legacy transactions on both networks, and they keep the
//! encoding — and therefore the tests — simple and fully deterministic.

use alloy_primitives::{keccak256, Address, B256, U256};

use crate::error::Result;
use crate::rlp;
use crate::wallet::Keypair;

#[derive(Debug, Clone)]
pub struct LegacyTransaction {
    pub nonce: u64,
    pub gas_price: U256,
    pub gas_limit: u64,
    /// `None` means contract creation.
    pub to: Option<Address>,
    pub value: U256,
    pub data: Vec<u8>,
    pub chain_id: u64,
}

/// A signed transaction, ready to broadcast.
#[derive(Debug, Clone)]
pub struct SignedTransaction {
    pub hash: B256,
    pub raw: Vec<u8>,
    pub v: u64,
    pub r: B256,
    pub s: B256,
}

impl SignedTransaction {
    pub fn raw_hex(&self) -> String {
        format!("0x{}", hex::encode(&self.raw))
    }

    pub fn hash_hex(&self) -> String {
        format!("0x{}", hex::encode(self.hash))
    }
}

impl LegacyTransaction {
    /// RLP of the six transaction fields, shared by both encodings.
    fn encode_body(&self, out: &mut Vec<u8>) {
        rlp::encode_u64(out, self.nonce);
        rlp::encode_uint(out, self.gas_price);
        rlp::encode_u64(out, self.gas_limit);
        match self.to {
            Some(address) => rlp::encode_bytes(out, address.as_slice()),
            None => rlp::encode_bytes(out, &[]),
        }
        rlp::encode_uint(out, self.value);
        rlp::encode_bytes(out, &self.data);
    }

    /// The EIP-155 signing payload: body followed by `chainId, 0, 0`.
    pub fn signing_payload(&self) -> Vec<u8> {
        let mut payload = Vec::new();
        self.encode_body(&mut payload);
        rlp::encode_u64(&mut payload, self.chain_id);
        rlp::encode_bytes(&mut payload, &[]);
        rlp::encode_bytes(&mut payload, &[]);
        rlp::encode_list(&payload)
    }

    pub fn signing_hash(&self) -> B256 {
        keccak256(self.signing_payload())
    }

    /// Sign with EIP-155 replay protection: `v = recovery + chainId * 2 + 35`.
    pub fn sign(&self, keypair: &Keypair) -> Result<SignedTransaction> {
        let signature = keypair.sign_hash(&self.signing_hash())?;
        let recovery = (signature[64] - 27) as u64;
        let v = recovery + self.chain_id * 2 + 35;

        let mut payload = Vec::new();
        self.encode_body(&mut payload);
        rlp::encode_u64(&mut payload, v);
        // r and s are encoded as minimal-length unsigned integers.
        rlp::encode_uint(&mut payload, U256::from_be_slice(&signature[..32]));
        rlp::encode_uint(&mut payload, U256::from_be_slice(&signature[32..64]));
        let raw = rlp::encode_list(&payload);

        Ok(SignedTransaction {
            hash: keccak256(&raw),
            raw,
            v,
            r: B256::from_slice(&signature[..32]),
            s: B256::from_slice(&signature[32..64]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wallet;

    const KEY: &str = "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727";

    fn sample() -> LegacyTransaction {
        LegacyTransaction {
            nonce: 9,
            gas_price: U256::from(20_000_000_000u64),
            gas_limit: 21000,
            to: Some(
                "0x3535353535353535353535353535353535353535"
                    .parse()
                    .unwrap(),
            ),
            value: U256::from(1_000_000_000_000_000_000u64),
            data: Vec::new(),
            chain_id: 338,
        }
    }

    /// Cross-checked against `eth_account.Account.sign_transaction` for the same input.
    #[test]
    fn matches_the_reference_signer() {
        let keypair = Keypair::from_hex(KEY).unwrap();
        let signed = sample().sign(&keypair).unwrap();
        assert_eq!(
            signed.raw_hex(),
            "0xf86e098504a817c800825208943535353535353535353535353535353535353535880de0b6b3a7640000808202c7a0153b4b4204abb93d881a58619d36fb62817c1545f0aa021dc421dcdb04431b2ea03bb09adab62e843b7d1d272e28f24479c2d7fe096edf4de42d9aacea7a5859ee"
        );
        assert_eq!(
            signed.hash_hex(),
            "0x7bdcc151f9fe904d4f4927b7a8537873fd574c1c0eb59fd372fd7fd48399a1da"
        );
    }

    #[test]
    fn v_encodes_the_chain_id() {
        let keypair = Keypair::from_hex(KEY).unwrap();
        for chain_id in [25u64, 338] {
            let mut tx = sample();
            tx.chain_id = chain_id;
            let v = tx.sign(&keypair).unwrap().v;
            let recovery = v - chain_id * 2 - 35;
            assert!(
                recovery <= 1,
                "recovery id out of range for chain {chain_id}"
            );
        }
    }

    #[test]
    fn the_signer_is_recoverable_from_the_signature() {
        let keypair = Keypair::from_hex(KEY).unwrap();
        let tx = sample();
        let signed = tx.sign(&keypair).unwrap();

        let mut signature = [0u8; 65];
        signature[..32].copy_from_slice(signed.r.as_slice());
        signature[32..64].copy_from_slice(signed.s.as_slice());
        signature[64] = (signed.v - tx.chain_id * 2 - 35) as u8;

        assert_eq!(
            wallet::recover_hash(&tx.signing_hash(), &signature).unwrap(),
            keypair.address()
        );
    }

    #[test]
    fn signing_is_deterministic() {
        let keypair = Keypair::from_hex(KEY).unwrap();
        assert_eq!(
            sample().sign(&keypair).unwrap().raw,
            sample().sign(&keypair).unwrap().raw
        );
    }

    #[test]
    fn different_chains_produce_different_transactions() {
        let keypair = Keypair::from_hex(KEY).unwrap();
        let mut mainnet = sample();
        mainnet.chain_id = 25;
        assert_ne!(
            sample().sign(&keypair).unwrap().raw,
            mainnet.sign(&keypair).unwrap().raw
        );
    }

    #[test]
    fn zero_value_and_zero_nonce_encode_as_empty_strings() {
        let mut tx = sample();
        tx.nonce = 0;
        tx.value = U256::ZERO;
        let payload = tx.signing_payload();
        // After the list header the first two items are 0x80 (empty) and the gas price.
        assert_eq!(payload[1], 0x80, "nonce 0 must RLP-encode as 0x80");
    }

    #[test]
    fn contract_creation_encodes_an_empty_to_field() {
        let mut tx = sample();
        tx.to = None;
        tx.data = vec![0x60, 0x80];
        let keypair = Keypair::from_hex(KEY).unwrap();
        // It must still sign and hash without panicking.
        assert!(!tx.sign(&keypair).unwrap().raw.is_empty());
        assert!(tx.signing_payload().windows(1).any(|w| w == [0x80]));
    }

    #[test]
    fn carries_call_data() {
        let mut tx = sample();
        tx.data = vec![0xa9, 0x05, 0x9c, 0xbb];
        let keypair = Keypair::from_hex(KEY).unwrap();
        assert_ne!(
            tx.sign(&keypair).unwrap().raw,
            sample().sign(&keypair).unwrap().raw
        );
        assert!(hex::encode(tx.signing_payload()).contains("a9059cbb"));
    }

    #[test]
    fn handles_very_large_values() {
        let mut tx = sample();
        tx.value = U256::MAX;
        tx.gas_price = U256::MAX;
        let keypair = Keypair::from_hex(KEY).unwrap();
        assert!(tx.sign(&keypair).is_ok());
    }
}
