//! Shelley/Conway-era transaction building.
//!
//! A Cardano transaction is CBOR: `[body, witness_set, is_valid, auxiliary]`.
//! The body is a map keyed by integers (0 inputs, 1 outputs, 2 fee, 3 ttl) and
//! the transaction id is `blake2b-256` of the body's CBOR — so the body bytes
//! must be produced deterministically and signed exactly as they will be
//! submitted. Conway wraps sets in CBOR tag 258.
//!
//! The CBOR is written by hand rather than through a generic serializer: the
//! encoding has to be byte-identical to what a node expects, and hand-writing
//! makes that explicit and testable against `cardano-serialization-lib`.

use super::address::Address;
use super::keys::{hash32, CardanoAccount};
use crate::error::{self, Result};

/// CBOR tag 258 — "this array is a set", required by the Conway-era CDDL.
const TAG_SET: u64 = 258;

// ---------------------------------------------------------------- CBOR writer

/// Write a CBOR head: a 3-bit major type plus a minimal-length argument.
fn head(out: &mut Vec<u8>, major: u8, arg: u64) {
    let m = major << 5;
    match arg {
        0..=23 => out.push(m | arg as u8),
        24..=0xff => {
            out.push(m | 24);
            out.push(arg as u8);
        }
        0x100..=0xffff => {
            out.push(m | 25);
            out.extend_from_slice(&(arg as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(m | 26);
            out.extend_from_slice(&(arg as u32).to_be_bytes());
        }
        _ => {
            out.push(m | 27);
            out.extend_from_slice(&arg.to_be_bytes());
        }
    }
}

fn uint(out: &mut Vec<u8>, n: u64) {
    head(out, 0, n);
}
fn bytes(out: &mut Vec<u8>, b: &[u8]) {
    head(out, 2, b.len() as u64);
    out.extend_from_slice(b);
}
fn array(out: &mut Vec<u8>, n: u64) {
    head(out, 4, n);
}
fn map(out: &mut Vec<u8>, n: u64) {
    head(out, 5, n);
}
fn set(out: &mut Vec<u8>, n: u64) {
    head(out, 6, TAG_SET);
    array(out, n);
}

// ------------------------------------------------------------------ tx pieces

/// One unspent output being consumed.
#[derive(Debug, Clone)]
pub struct TxInput {
    pub tx_hash: [u8; 32],
    pub index: u64,
    /// What this input is worth; needed to balance the transaction.
    pub lovelace: u64,
}

/// One output being created.
#[derive(Debug, Clone)]
pub struct TxOutput {
    pub address: Address,
    pub lovelace: u64,
}

/// The unsigned body.
#[derive(Debug, Clone)]
pub struct TxBody {
    pub inputs: Vec<TxInput>,
    pub outputs: Vec<TxOutput>,
    pub fee: u64,
    pub ttl: u64,
}

impl TxBody {
    /// Deterministic CBOR for the body — the exact bytes that get hashed.
    pub fn to_cbor(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(256);
        map(&mut out, 4); // keys 0..3, ascending

        uint(&mut out, 0);
        set(&mut out, self.inputs.len() as u64);
        for input in &self.inputs {
            array(&mut out, 2);
            bytes(&mut out, &input.tx_hash);
            uint(&mut out, input.index);
        }

        uint(&mut out, 1);
        array(&mut out, self.outputs.len() as u64);
        for output in &self.outputs {
            // The legacy `[address, coin]` form: still valid in Conway, and
            // what every wallet emits for a plain ADA-only output.
            array(&mut out, 2);
            bytes(&mut out, &output.address.bytes);
            uint(&mut out, output.lovelace);
        }

        uint(&mut out, 2);
        uint(&mut out, self.fee);

        uint(&mut out, 3);
        uint(&mut out, self.ttl);

        out
    }

    /// The transaction id: blake2b-256 of the body CBOR.
    pub fn tx_id(&self) -> [u8; 32] {
        hash32(&self.to_cbor())
    }

    /// Inputs minus outputs minus fee. A node rejects anything but zero.
    pub fn imbalance(&self) -> i128 {
        let ins: u64 = self.inputs.iter().map(|i| i.lovelace).sum();
        let outs: u64 = self.outputs.iter().map(|o| o.lovelace).sum();
        ins as i128 - outs as i128 - self.fee as i128
    }
}

/// A signature over the transaction id by one key.
#[derive(Debug, Clone)]
pub struct VkeyWitness {
    pub public_key: [u8; 32],
    pub signature: [u8; 64],
}

/// A body plus its witnesses, ready to submit.
#[derive(Debug, Clone)]
pub struct SignedTx {
    pub body: TxBody,
    pub witnesses: Vec<VkeyWitness>,
}

impl SignedTx {
    pub fn sign(body: TxBody, signer: &CardanoAccount) -> Self {
        let id = body.tx_id();
        let witnesses = vec![VkeyWitness {
            public_key: signer.payment_public_key(),
            signature: signer.sign(&id),
        }];
        SignedTx { body, witnesses }
    }

    fn witness_set_cbor(&self) -> Vec<u8> {
        let mut out = Vec::new();
        map(&mut out, 1);
        uint(&mut out, 0); // key 0 = vkey witnesses
        set(&mut out, self.witnesses.len() as u64);
        for witness in &self.witnesses {
            array(&mut out, 2);
            bytes(&mut out, &witness.public_key);
            bytes(&mut out, &witness.signature);
        }
        out
    }

    /// The full transaction CBOR: `[body, witnesses, true, null]`.
    pub fn to_cbor(&self) -> Vec<u8> {
        let mut out = Vec::new();
        array(&mut out, 4);
        out.extend_from_slice(&self.body.to_cbor());
        out.extend_from_slice(&self.witness_set_cbor());
        out.push(0xf5); // is_valid = true
        out.push(0xf6); // auxiliary_data = null
        out
    }

    pub fn tx_id(&self) -> [u8; 32] {
        self.body.tx_id()
    }
}

// -------------------------------------------------------------- fee and coins

/// The protocol parameters this wallet needs.
#[derive(Debug, Clone, Copy)]
pub struct ProtocolParams {
    /// Fee per byte of transaction.
    pub min_fee_a: u64,
    /// Flat fee constant.
    pub min_fee_b: u64,
    /// Lovelace charged per byte of a UTxO entry.
    pub coins_per_utxo_byte: u64,
}

impl Default for ProtocolParams {
    fn default() -> Self {
        ProtocolParams {
            min_fee_a: 44,
            min_fee_b: 155_381,
            coins_per_utxo_byte: 4_310,
        }
    }
}

impl ProtocolParams {
    pub fn min_fee(&self, tx_size: usize) -> u64 {
        self.min_fee_a * tx_size as u64 + self.min_fee_b
    }

    /// The minimum lovelace an output must carry, given its serialized size.
    /// The `+ 160` accounts for the UTxO entry's own overhead.
    pub fn min_ada_for_output(&self, output: &TxOutput) -> u64 {
        let mut buf = Vec::new();
        array(&mut buf, 2);
        bytes(&mut buf, &output.address.bytes);
        uint(&mut buf, output.lovelace.max(1_000_000));
        (buf.len() as u64 + 160) * self.coins_per_utxo_byte
    }
}

/// Coin selection, fee calculation and change, in one pass.
///
/// Cardano's fee depends on transaction size, and size depends on the fee
/// field's own encoding and on whether a change output exists. So the
/// transaction is built with a deliberately over-wide fee, measured, then
/// settled: the CBOR head bytes can then only shrink, never grow, which means
/// the measured size is always an upper bound and the transaction can never
/// come out under-fee'd.
pub struct TxBuilder {
    pub params: ProtocolParams,
    pub ttl: u64,
}

impl TxBuilder {
    pub fn new(params: ProtocolParams, ttl: u64) -> Self {
        TxBuilder { params, ttl }
    }

    /// Send `amount` to `to`, funding it from `utxos`, change to `change_address`.
    pub fn build_transfer(
        &self,
        utxos: &[TxInput],
        to: &Address,
        amount: u64,
        change_address: &Address,
        signer: &CardanoAccount,
    ) -> Result<SignedTx> {
        if utxos.is_empty() {
            return Err(error::insufficient_funds(
                "this address holds no unspent outputs to spend",
            ));
        }
        let destination = TxOutput {
            address: to.clone(),
            lovelace: amount,
        };
        let minimum = self.params.min_ada_for_output(&destination);
        if amount < minimum {
            return Err(error::invalid_amount(format!(
                "an output of {amount} lovelace is below the {minimum} lovelace \
                 minimum for this address; Cardano would reject it"
            )));
        }

        // Largest first: the fewest inputs, so the smallest fee.
        let mut sorted = utxos.to_vec();
        sorted.sort_by_key(|u| std::cmp::Reverse(u.lovelace));

        // Two attempts: with a change output, and — if change would be dust —
        // without one, folding the remainder into the fee.
        for keep_change in [true, false] {
            let mut selected: Vec<TxInput> = Vec::new();
            let mut total: u64 = 0;
            for utxo in &sorted {
                selected.push(utxo.clone());
                total += utxo.lovelace;

                let change_probe = TxOutput {
                    address: change_address.clone(),
                    lovelace: 0,
                };
                let min_change = self.params.min_ada_for_output(&change_probe);

                let mut outputs = vec![destination.clone()];
                if keep_change {
                    outputs.push(change_probe);
                }
                let probe = TxBody {
                    inputs: selected.clone(),
                    outputs: outputs
                        .iter()
                        .map(|o| TxOutput {
                            address: o.address.clone(),
                            lovelace: u64::MAX,
                        })
                        .collect(),
                    fee: u64::MAX,
                    ttl: self.ttl,
                };
                let probe_signed = SignedTx {
                    body: probe,
                    witnesses: vec![VkeyWitness {
                        public_key: [0; 32],
                        signature: [0; 64],
                    }],
                };
                let fee = self.params.min_fee(probe_signed.to_cbor().len());

                let Some(remainder) = total.checked_sub(amount).and_then(|r| r.checked_sub(fee))
                else {
                    continue; // not enough yet; add another input
                };

                let body = if keep_change {
                    if remainder < min_change {
                        continue; // the change would be dust; try more inputs
                    }
                    TxBody {
                        inputs: selected,
                        outputs: vec![
                            destination,
                            TxOutput {
                                address: change_address.clone(),
                                lovelace: remainder,
                            },
                        ],
                        fee,
                        ttl: self.ttl,
                    }
                } else {
                    // No change output: the leftover becomes extra fee, which
                    // is legal and is how wallets sweep dust.
                    TxBody {
                        inputs: selected,
                        outputs: vec![destination],
                        fee: fee + remainder,
                        ttl: self.ttl,
                    }
                };
                debug_assert_eq!(body.imbalance(), 0, "a transaction must balance exactly");
                return Ok(SignedTx::sign(body, signer));
            }
        }

        let available: u64 = utxos.iter().map(|u| u.lovelace).sum();
        Err(error::insufficient_funds(format!(
            "{available} lovelace available, which cannot cover {amount} plus fees"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::cardano::address::CardanoNetwork;
    use crate::chain::Seed;

    const PHRASE: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn signer() -> CardanoAccount {
        let entropy = Seed::new(PHRASE, "").unwrap().entropy().unwrap();
        CardanoAccount::from_entropy(&entropy, "", 0).unwrap()
    }

    fn utxo(byte: u8, lovelace: u64) -> TxInput {
        TxInput {
            tx_hash: [byte; 32],
            index: 0,
            lovelace,
        }
    }

    #[test]
    fn cbor_heads_are_minimal_length() {
        let cases: &[(u64, &[u8])] = &[
            (0, &[0x00]),
            (23, &[0x17]),
            (24, &[0x18, 0x18]),
            (1000, &[0x19, 0x03, 0xe8]),
            (1_000_000, &[0x1a, 0x00, 0x0f, 0x42, 0x40]),
            (
                1_000_000_000_000,
                &[0x1b, 0, 0, 0, 0xe8, 0xd4, 0xa5, 0x10, 0x00],
            ),
        ];
        for (n, expected) in cases {
            let mut out = Vec::new();
            uint(&mut out, *n);
            assert_eq!(&out[..], *expected, "uint {n}");
        }
        let mut out = Vec::new();
        set(&mut out, 1);
        assert_eq!(out, [0xd9, 0x01, 0x02, 0x81], "a Conway set is tag 258");
    }

    #[test]
    fn a_transfer_balances_exactly_and_keeps_change() {
        let signer = signer();
        let change = signer.base_address(CardanoNetwork::Testnet);
        let to = Address::base(CardanoNetwork::Testnet, [0x33; 28], [0x44; 28]);
        let builder = TxBuilder::new(ProtocolParams::default(), 100_000);

        let signed = builder
            .build_transfer(&[utxo(1, 10_000_000)], &to, 2_000_000, &change, &signer)
            .unwrap();

        assert_eq!(signed.body.imbalance(), 0);
        assert_eq!(signed.body.outputs.len(), 2, "amount plus change");
        assert_eq!(signed.body.outputs[0].lovelace, 2_000_000);
        assert_eq!(signed.body.outputs[1].address, change);
        assert_eq!(signed.witnesses.len(), 1);
    }

    #[test]
    fn the_fee_covers_the_transaction_it_is_actually_in() {
        // The circularity this checks: the fee is computed from a size that
        // includes the fee field. If the settled transaction were larger than
        // the probe, the node would reject it as under-fee'd.
        let signer = signer();
        let change = signer.base_address(CardanoNetwork::Testnet);
        let to = Address::base(CardanoNetwork::Testnet, [0x33; 28], [0x44; 28]);
        let params = ProtocolParams::default();
        let signed = TxBuilder::new(params, 100_000)
            .build_transfer(&[utxo(1, 10_000_000)], &to, 2_000_000, &change, &signer)
            .unwrap();

        let actual_size = signed.to_cbor().len();
        assert!(
            signed.body.fee >= params.min_fee(actual_size),
            "fee {} does not cover the {actual_size}-byte transaction (needs {})",
            signed.body.fee,
            params.min_fee(actual_size)
        );
    }

    #[test]
    fn dust_change_is_folded_into_the_fee_rather_than_created() {
        // An output below the minimum is unspendable, so a wallet must not
        // make one. Sweeping it into the fee is what real wallets do.
        let signer = signer();
        let change = signer.base_address(CardanoNetwork::Testnet);
        let to = Address::base(CardanoNetwork::Testnet, [0x33; 28], [0x44; 28]);
        let params = ProtocolParams::default();
        // Just barely more than amount + fee, so any change would be dust.
        let funds = 2_000_000 + params.min_fee(300) + 1_000;
        let signed = TxBuilder::new(params, 100_000)
            .build_transfer(&[utxo(1, funds)], &to, 2_000_000, &change, &signer)
            .unwrap();

        assert_eq!(signed.body.outputs.len(), 1, "no dust change output");
        assert_eq!(signed.body.imbalance(), 0);
        assert!(signed.body.fee > params.min_fee(signed.to_cbor().len()));
    }

    #[test]
    fn several_inputs_are_selected_largest_first_until_the_amount_is_covered() {
        let signer = signer();
        let change = signer.base_address(CardanoNetwork::Testnet);
        let to = Address::base(CardanoNetwork::Testnet, [0x33; 28], [0x44; 28]);
        let signed = TxBuilder::new(ProtocolParams::default(), 100_000)
            .build_transfer(
                &[utxo(1, 2_000_000), utxo(2, 9_000_000), utxo(3, 1_000_000)],
                &to,
                8_000_000,
                &change,
                &signer,
            )
            .unwrap();

        // The 9 ADA input alone cannot cover 8 ADA plus the fee, so exactly
        // one more is added — and the largest goes first.
        assert_eq!(signed.body.inputs.len(), 2);
        assert_eq!(signed.body.inputs[0].lovelace, 9_000_000);
        assert_eq!(signed.body.imbalance(), 0);
    }

    #[test]
    fn an_output_below_the_minimum_is_refused_with_the_number() {
        let signer = signer();
        let change = signer.base_address(CardanoNetwork::Testnet);
        let to = Address::base(CardanoNetwork::Testnet, [0x33; 28], [0x44; 28]);
        let err = TxBuilder::new(ProtocolParams::default(), 100_000)
            .build_transfer(&[utxo(1, 10_000_000)], &to, 1, &change, &signer)
            .unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAmount);
        assert!(err.message.contains("minimum"), "{}", err.message);
    }

    #[test]
    fn an_underfunded_transfer_reports_what_was_available() {
        let signer = signer();
        let change = signer.base_address(CardanoNetwork::Testnet);
        let to = Address::base(CardanoNetwork::Testnet, [0x33; 28], [0x44; 28]);
        let err = TxBuilder::new(ProtocolParams::default(), 100_000)
            .build_transfer(&[utxo(1, 3_000_000)], &to, 2_900_000, &change, &signer)
            .unwrap_err();
        assert_eq!(err.code, error::Code::InsufficientFunds);
        assert!(err.message.contains("3000000"), "{}", err.message);

        // And an address with nothing in it says so plainly.
        let empty = TxBuilder::new(ProtocolParams::default(), 100_000)
            .build_transfer(&[], &to, 2_000_000, &change, &signer)
            .unwrap_err();
        assert!(
            empty.message.contains("no unspent outputs"),
            "{}",
            empty.message
        );
    }

    #[test]
    fn the_transaction_id_is_the_hash_of_the_body_only() {
        let signer = signer();
        let change = signer.base_address(CardanoNetwork::Testnet);
        let to = Address::base(CardanoNetwork::Testnet, [0x33; 28], [0x44; 28]);
        let signed = TxBuilder::new(ProtocolParams::default(), 100_000)
            .build_transfer(&[utxo(1, 10_000_000)], &to, 2_000_000, &change, &signer)
            .unwrap();
        assert_eq!(signed.tx_id(), hash32(&signed.body.to_cbor()));
        // Adding a witness must not move the id.
        let mut with_extra = signed.clone();
        with_extra.witnesses.push(with_extra.witnesses[0].clone());
        assert_eq!(with_extra.tx_id(), signed.tx_id());
    }

    #[test]
    fn the_full_transaction_ends_with_is_valid_and_no_auxiliary_data() {
        let signer = signer();
        let change = signer.base_address(CardanoNetwork::Testnet);
        let to = Address::base(CardanoNetwork::Testnet, [0x33; 28], [0x44; 28]);
        let cbor = TxBuilder::new(ProtocolParams::default(), 100_000)
            .build_transfer(&[utxo(1, 10_000_000)], &to, 2_000_000, &change, &signer)
            .unwrap()
            .to_cbor();
        assert_eq!(cbor[0], 0x84, "a 4-element array");
        assert_eq!(&cbor[cbor.len() - 2..], &[0xf5, 0xf6]);
    }
}
