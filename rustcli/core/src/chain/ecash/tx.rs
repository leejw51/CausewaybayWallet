//! Building, signing and serialising an eCash transaction.
//!
//! The format is Bitcoin's, unchanged since 2009: a version, a list of inputs
//! naming outputs of earlier transactions, a list of outputs, and a locktime.
//! There is no fee field — the fee is whatever the inputs are worth minus what
//! the outputs claim, which is why a coin selection bug here does not bounce,
//! it *pays*. Every path below either balances exactly or refuses.
//!
//! What eCash changed is the signature. It inherited Bitcoin Cash's
//! `SIGHASH_FORKID`, so the digest a key signs is the BIP-143 one — a
//! structure of pre-hashed sections that commits to the value of the input
//! being spent. Bitcoin's original algorithm did not, which is how a wallet
//! could be lied to about what it was spending. The fork bit (`0x40`) is also
//! what makes an eCash signature invalid on Bitcoin and the other way round,
//! so a transaction built here can only ever be replayed on the chain it was
//! meant for.

use super::address::{Address, AddressKind};
use super::keys::{sha256d, write_varint, EcashAccount};
use crate::chain;
use crate::error::{self, Result};
use crate::network::Network;

/// `SIGHASH_ALL | SIGHASH_FORKID`: sign every input and every output, on the
/// eCash side of the fork. The only combination this wallet produces.
pub const SIGHASH_ALL_FORKID: u32 = 0x41;

/// Inputs are final; this wallet builds no replaceable transactions.
const SEQUENCE_FINAL: u32 = 0xffff_ffff;

const TX_VERSION: u32 = 2;

/// The smallest output the network will relay, in satoshis.
///
/// Below this an output costs more to spend than it is worth, so nodes refuse
/// to carry it. An output at 545 is not a cheap transfer, it is a transaction
/// that never confirms.
pub const DUST_LIMIT: u64 = 546;

/// Satoshis per byte. eCash's `minRelayTxFee` is 1000 satoshis per kilobyte
/// and has not moved; there is no fee market to estimate against.
pub const FEE_PER_BYTE: u64 = 1;

/// The largest a signature push can be: at most 71 bytes of DER — `s` is
/// forced low, so only `r` can need a leading zero — plus the sighash byte.
const MAX_SIGNATURE_LEN: usize = 72;

/// One unspent output this wallet can spend: an outpoint, and what it is worth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utxo {
    /// Internal byte order, as it goes on the wire — not the reversed form an
    /// explorer prints.
    pub txid: [u8; 32],
    pub index: u32,
    pub sats: u64,
}

/// One output being created.
#[derive(Debug, Clone)]
pub struct TxOutput {
    pub address: Address,
    pub sats: u64,
}

impl TxOutput {
    fn serialize(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.sats.to_le_bytes());
        let script = self.address.script_pubkey();
        write_varint(out, script.len() as u64);
        out.extend_from_slice(&script);
    }

    /// How many bytes this output takes on the wire.
    fn size(&self) -> usize {
        let mut buf = Vec::new();
        self.serialize(&mut buf);
        buf.len()
    }
}

/// A transaction, with the script signatures filled in once it is signed.
#[derive(Debug, Clone)]
pub struct Transaction {
    pub inputs: Vec<Utxo>,
    pub outputs: Vec<TxOutput>,
    pub script_sigs: Vec<Vec<u8>>,
}

impl Transaction {
    /// The bytes that go to a node.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.inputs.len() * 150 + self.outputs.len() * 34 + 10);
        out.extend_from_slice(&TX_VERSION.to_le_bytes());
        write_varint(&mut out, self.inputs.len() as u64);
        for (i, input) in self.inputs.iter().enumerate() {
            out.extend_from_slice(&input.txid);
            out.extend_from_slice(&input.index.to_le_bytes());
            let script = self.script_sigs.get(i).map(Vec::as_slice).unwrap_or(&[]);
            write_varint(&mut out, script.len() as u64);
            out.extend_from_slice(script);
            out.extend_from_slice(&SEQUENCE_FINAL.to_le_bytes());
        }
        write_varint(&mut out, self.outputs.len() as u64);
        for output in &self.outputs {
            output.serialize(&mut out);
        }
        out.extend_from_slice(&0u32.to_le_bytes()); // locktime
        out
    }

    /// The transaction id, in the order explorers and RPCs print it.
    ///
    /// A txid is `sha256d` of the serialized transaction read *backwards*.
    /// The reversal is a forty-year-old accident of Bitcoin's byte order, and
    /// forgetting it is how a wallet hands a user a hash no explorer can find.
    pub fn txid(&self) -> String {
        let mut hash = sha256d(&self.serialize());
        hash.reverse();
        hex::encode(hash)
    }

    /// Inputs minus outputs: the fee this transaction actually pays.
    ///
    /// Summed in `i128` because the input values come from an endpoint, and a
    /// total that overflows `u64` should be refused rather than panic in a
    /// debug build.
    pub fn fee(&self) -> i128 {
        let ins: i128 = self.inputs.iter().map(|i| i128::from(i.sats)).sum();
        let outs: i128 = self.outputs.iter().map(|o| i128::from(o.sats)).sum();
        ins - outs
    }
}

/// The BIP-143 digest for one input of a transaction.
///
/// `prevout_script` is the script the input being spent is locked by, and
/// `prevout_value` is what it holds. Committing to the value is the whole
/// point of the algorithm: an offline signer that is lied to about it signs
/// away the difference as fee, and BIP-143 makes that lie fail verification.
pub fn sighash(
    inputs: &[Utxo],
    outputs: &[TxOutput],
    index: usize,
    prevout_script: &[u8],
    prevout_value: u64,
) -> [u8; 32] {
    let mut prevouts = Vec::with_capacity(inputs.len() * 36);
    let mut sequences = Vec::with_capacity(inputs.len() * 4);
    for input in inputs {
        prevouts.extend_from_slice(&input.txid);
        prevouts.extend_from_slice(&input.index.to_le_bytes());
        sequences.extend_from_slice(&SEQUENCE_FINAL.to_le_bytes());
    }
    let mut serialized_outputs = Vec::new();
    for output in outputs {
        output.serialize(&mut serialized_outputs);
    }

    let mut preimage = Vec::with_capacity(prevout_script.len() + 160);
    preimage.extend_from_slice(&TX_VERSION.to_le_bytes());
    preimage.extend_from_slice(&sha256d(&prevouts));
    preimage.extend_from_slice(&sha256d(&sequences));
    preimage.extend_from_slice(&inputs[index].txid);
    preimage.extend_from_slice(&inputs[index].index.to_le_bytes());
    write_varint(&mut preimage, prevout_script.len() as u64);
    preimage.extend_from_slice(prevout_script);
    preimage.extend_from_slice(&prevout_value.to_le_bytes());
    preimage.extend_from_slice(&SEQUENCE_FINAL.to_le_bytes());
    preimage.extend_from_slice(&sha256d(&serialized_outputs));
    preimage.extend_from_slice(&0u32.to_le_bytes()); // locktime
    preimage.extend_from_slice(&SIGHASH_ALL_FORKID.to_le_bytes());
    sha256d(&preimage)
}

/// Sign every input, all of which this wallet's one key controls.
pub fn sign(
    inputs: &[Utxo],
    outputs: &[TxOutput],
    signer: &EcashAccount,
    source: &Address,
) -> Result<Transaction> {
    let script_code = source.script_pubkey();
    let public_key = signer.public_key();
    let mut script_sigs = Vec::with_capacity(inputs.len());
    for (index, input) in inputs.iter().enumerate() {
        let digest = sighash(inputs, outputs, index, &script_code, input.sats);
        let mut signature = signer.sign_der(&digest)?;
        signature.push(SIGHASH_ALL_FORKID as u8);

        // `<signature> <public key>`, each behind its own push opcode. Both
        // are short enough that the opcode is just the length.
        let mut script = Vec::with_capacity(signature.len() + public_key.len() + 2);
        script.push(signature.len() as u8);
        script.extend_from_slice(&signature);
        script.push(public_key.len() as u8);
        script.extend_from_slice(&public_key);
        script_sigs.push(script);
    }
    Ok(Transaction {
        inputs: inputs.to_vec(),
        outputs: outputs.to_vec(),
        script_sigs,
    })
}

/// How large a transaction with these counts will be once signed.
///
/// An upper bound, never an estimate: the signature is sized at its longest
/// possible DER encoding, so the real transaction can only come out smaller
/// and the fee derived from this can only come out above the relay minimum.
/// The other way round is a transaction that is signed, valid, and never
/// relayed.
pub fn signed_size(input_count: usize, outputs: &[TxOutput]) -> usize {
    let mut size = 4 + 4; // version, locktime
    let mut counts = Vec::new();
    write_varint(&mut counts, input_count as u64);
    write_varint(&mut counts, outputs.len() as u64);
    size += counts.len();
    // Per input: outpoint, the script's length byte, the script, the sequence.
    // The script is `push(sig) sig push(key) key`.
    size += input_count * (32 + 4 + 1 + (1 + MAX_SIGNATURE_LEN + 1 + 33) + 4);
    size += outputs.iter().map(TxOutput::size).sum::<usize>();
    size
}

/// Coin selection, fee and change, in one pass.
///
/// Largest-input-first, which is the fewest inputs and so the smallest fee.
/// Two attempts: one that keeps a change output, and — when the change would
/// be dust the network will not relay — one that folds the remainder into the
/// fee instead. That second pass is how a wallet sweeps the last of an address
/// rather than reporting a balance it cannot move.
pub struct Builder {
    /// What the network and the user will let the fee reach.
    fee_limit: Option<(Network, u128)>,
}

impl Builder {
    pub fn new() -> Self {
        Builder { fee_limit: None }
    }

    /// Refuse to sign a transaction whose fee exceeds `ceiling` satoshis.
    ///
    /// eCash quotes no fee, so there is no endpoint number to distrust here as
    /// there is on Cardano and EVM. What there is instead is *size*: the fee
    /// is a satoshi a byte and every input costs about 148 of them, so an
    /// address that has been paid in a few thousand dust outputs builds a
    /// perfectly valid transaction whose fee runs past what is being sent. The
    /// ceiling is what stops that being signed rather than discovered
    /// afterwards.
    pub fn limit_fee(mut self, network: Network, ceiling: u128) -> Self {
        self.fee_limit = Some((network, ceiling));
        self
    }

    /// Send `sats` to `to`, funded from `utxos`, change back to `change`.
    pub fn build_transfer(
        &self,
        utxos: &[Utxo],
        to: &Address,
        sats: u64,
        change: &Address,
        signer: &EcashAccount,
    ) -> Result<Transaction> {
        if utxos.is_empty() {
            return Err(error::insufficient_funds(
                "this address holds no unspent outputs to spend",
            ));
        }
        if sats < DUST_LIMIT {
            return Err(error::invalid_amount(format!(
                "{sats} satoshis is below the {DUST_LIMIT} satoshi dust limit; \
                 eCash nodes will not relay an output that small"
            )));
        }
        let destination = TxOutput { address: *to, sats };
        let change_probe = TxOutput {
            address: *change,
            sats: 0,
        };

        let mut sorted = utxos.to_vec();
        // By value, then by outpoint, so the same wallet state always selects
        // the same coins and a retry rebuilds the same transaction.
        sorted.sort_by(|a, b| {
            b.sats
                .cmp(&a.sats)
                .then_with(|| a.txid.cmp(&b.txid))
                .then_with(|| a.index.cmp(&b.index))
        });

        for keep_change in [true, false] {
            let mut selected: Vec<Utxo> = Vec::new();
            let mut total: u64 = 0;
            for utxo in &sorted {
                selected.push(*utxo);
                total = total.checked_add(utxo.sats).ok_or_else(|| {
                    error::rpc_error(
                        "the unspent outputs at this address sum past what eCash can hold",
                    )
                })?;

                let mut outputs = vec![destination.clone()];
                if keep_change {
                    outputs.push(change_probe.clone());
                }
                let size = signed_size(selected.len(), &outputs);
                let fee = FEE_PER_BYTE * size as u64;
                // Before the arithmetic below turns an absurd fee into a plain
                // "insufficient funds", which would name the wrong culprit.
                if let Some((network, ceiling)) = self.fee_limit {
                    chain::check_fee(&network, ceiling, u128::from(fee), network.units())?;
                }

                let Some(remainder) = total.checked_sub(sats).and_then(|r| r.checked_sub(fee))
                else {
                    continue; // not enough yet; add another input
                };

                let outputs = if keep_change {
                    if remainder < DUST_LIMIT {
                        continue; // the change would not relay; try more inputs
                    }
                    vec![
                        destination,
                        TxOutput {
                            address: *change,
                            sats: remainder,
                        },
                    ]
                } else {
                    // No change output: the leftover is fee, which is how the
                    // last of an address gets swept. Held to the ceiling too,
                    // because this is the path where the number can run away.
                    if let Some((network, ceiling)) = self.fee_limit {
                        chain::check_fee(
                            &network,
                            ceiling,
                            u128::from(fee + remainder),
                            network.units(),
                        )?;
                    }
                    vec![destination]
                };

                let signed = sign(&selected, &outputs, signer, change)?;
                debug_assert!(signed.fee() > 0, "a transaction must pay a fee");
                debug_assert!(
                    signed.serialize().len() <= signed_size(selected.len(), &signed.outputs),
                    "the size bound must never be under the real size"
                );
                return Ok(signed);
            }
        }

        let available: u64 = utxos.iter().map(|u| u.sats).sum();
        Err(error::insufficient_funds(format!(
            "{available} satoshis available across {} unspent output{}, which cannot \
             cover {sats} plus the fee",
            utxos.len(),
            if utxos.len() == 1 { "" } else { "s" },
        )))
    }
}

impl Default for Builder {
    fn default() -> Self {
        Builder::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::ecash::address::EcashNetwork;
    use crate::chain::Seed;
    use crate::network::{ECASH_MAINNET, ECASH_TESTNET};

    const PHRASE: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn signer() -> EcashAccount {
        EcashAccount::from_seed(&Seed::new(PHRASE, "").unwrap(), 0).unwrap()
    }

    fn recipient() -> Address {
        EcashAccount::from_seed(&Seed::new(PHRASE, "").unwrap(), 1)
            .unwrap()
            .address(EcashNetwork::Testnet)
    }

    fn utxo(byte: u8, sats: u64) -> Utxo {
        Utxo {
            txid: [byte; 32],
            index: 0,
            sats,
        }
    }

    /// A transaction taken off the live eCash chain, whose signatures this
    /// wallet's digest has to be the one they were made over.
    ///
    /// This is the strongest check available for a sighash algorithm short of
    /// running a node: the two signatures below were accepted by eCash's own
    /// consensus rules in block 963,838, so if the preimage built here differs
    /// from the one their signer used — by a byte, by a field order, by the
    /// sighash type — neither will verify. Nothing about it can be satisfied
    /// by an implementation that is merely self-consistent.
    ///
    /// `f7696215969330f99cdf433e9e2a699b71ee850bbae9ceff91bea87b634f5fde`:
    /// two pay-to-public-key-hash inputs from one key, one output, version 2,
    /// final sequences, locktime 0 — the same shape this wallet builds.
    #[test]
    fn a_real_transaction_from_the_chain_verifies_against_this_digest() {
        fn hash160(hex: &str) -> [u8; 20] {
            let mut out = [0u8; 20];
            hex::decode_to_slice(hex, &mut out).unwrap();
            out
        }
        /// The txid as an explorer prints it, back into wire order.
        fn outpoint(displayed: &str, index: u32, sats: u64) -> Utxo {
            let mut txid = [0u8; 32];
            hex::decode_to_slice(displayed, &mut txid).unwrap();
            txid.reverse();
            Utxo { txid, index, sats }
        }

        let inputs = [
            outpoint(
                "7af7070f1f8dca4c84d4329b30e8ff7b8b1654c4aea1aa038dbb5c3842f94197",
                3,
                179_457_141,
            ),
            outpoint(
                "61ddbdb1986d58d28bf0cf23c1ad79124a11dd1a58485b40bd57872a8c742dff",
                3,
                179_443_366,
            ),
        ];
        let outputs = [TxOutput {
            address: Address::p2pkh(
                EcashNetwork::Mainnet,
                hash160("2fd0a84cd8d80a62681ab940a24ad42bde61ef80"),
            ),
            sats: 358_899_833,
        }];
        // Both inputs are locked to the same key, so one script code serves.
        let script_code = Address::p2pkh(
            EcashNetwork::Mainnet,
            hash160("942a7a7235b5dbd703836ecc5b196e7ea10760c4"),
        )
        .script_pubkey();
        let public_key =
            hex::decode("03f3828a7f00fdab735162ccd1d223cd54ce28b884ef243294a047e22133c35cbb")
                .unwrap();
        // The DER signatures as they appear in the two script sigs, with the
        // trailing sighash byte still on them.
        let signatures = [
            "3045022100db1c8ff6711d45af41f977934435bf35a9719d12710449cea6199de8f797a9aa\
             0220382074cc70bc144b1a79df3a7a6dc2ad57ed9d83b18ee85a5c71aecc2a7e00e041",
            "3045022100ad94e5dce5bfb1b60fb95737a25f585143b497f997b65dced1b70e2dce3e7b24\
             022069f5c237b218571e0bb4536ec78d32dc5193d67f7e121373981b0c7b3c70ee1241",
        ];

        for (index, signature) in signatures.iter().enumerate() {
            let der = hex::decode(signature.replace([' ', '\n'], "")).unwrap();
            let (body, sighash_byte) = der.split_at(der.len() - 1);
            assert_eq!(
                sighash_byte[0], SIGHASH_ALL_FORKID as u8,
                "input {index} was signed with SIGHASH_ALL|SIGHASH_FORKID"
            );
            let digest = sighash(&inputs, &outputs, index, &script_code, inputs[index].sats);
            assert!(
                super::super::keys::verify_der(&public_key, &digest, body).unwrap(),
                "input {index}: the chain accepted a signature over a digest this \
                 wallet does not compute"
            );
        }
    }

    /// The value of the input being spent is inside the digest, which is the
    /// whole reason BIP-143 replaced Bitcoin's original algorithm: an offline
    /// signer lied to about it used to sign the difference away as fee.
    #[test]
    fn the_digest_commits_to_the_value_of_the_input_being_spent() {
        let inputs = vec![utxo(1, 100_000), utxo(2, 200_000)];
        let outputs = vec![TxOutput {
            address: recipient(),
            sats: 250_000,
        }];
        let script = signer().address(EcashNetwork::Testnet).script_pubkey();

        let honest = sighash(&inputs, &outputs, 0, &script, 100_000);
        let lied_to = sighash(&inputs, &outputs, 0, &script, 100_001);
        assert_ne!(honest, lied_to);
        // And each input of one transaction has its own digest.
        assert_ne!(honest, sighash(&inputs, &outputs, 1, &script, 200_000));
    }

    /// The fork bit is what keeps an eCash signature off Bitcoin's chain.
    #[test]
    fn the_sighash_type_carries_the_fork_bit() {
        assert_eq!(SIGHASH_ALL_FORKID, 0x41);
        assert_eq!(SIGHASH_ALL_FORKID & 0x40, 0x40, "SIGHASH_FORKID");
        assert_eq!(SIGHASH_ALL_FORKID & 0x1f, 0x01, "SIGHASH_ALL");
    }

    #[test]
    fn a_signed_transfer_balances_and_pays_about_one_satoshi_a_byte() {
        let utxos = [utxo(1, 100_000), utxo(2, 50_000)];
        let tx = Builder::new()
            .build_transfer(
                &utxos,
                &recipient(),
                60_000,
                &signer().address(EcashNetwork::Testnet),
                &signer(),
            )
            .unwrap();

        assert_eq!(tx.inputs.len(), 1, "the largest input alone covers it");
        assert_eq!(tx.outputs.len(), 2, "destination and change");
        assert_eq!(tx.outputs[0].sats, 60_000);
        let fee = tx.fee();
        assert!(fee > 0);
        let size = tx.serialize().len() as i128;
        assert!(
            fee >= size,
            "fee {fee} is under 1 sat/byte for {size} bytes"
        );
        assert!(
            fee < size + 8,
            "fee {fee} is far above 1 sat/byte for {size} bytes"
        );
        // Inputs minus outputs is the fee; nothing else balances a Bitcoin
        // transaction.
        assert_eq!(
            i128::from(utxos[0].sats) - 60_000 - i128::from(tx.outputs[1].sats),
            fee
        );
    }

    #[test]
    fn the_size_bound_is_never_under_the_real_size() {
        // Over many keys, so the DER encodings vary in length.
        let seed = Seed::new(PHRASE, "").unwrap();
        for index in 0..12u32 {
            let account = EcashAccount::from_seed(&seed, index).unwrap();
            let own = account.address(EcashNetwork::Testnet);
            let tx = Builder::new()
                .build_transfer(&[utxo(1, 500_000)], &recipient(), 100_000, &own, &account)
                .unwrap();
            let bound = signed_size(tx.inputs.len(), &tx.outputs);
            assert!(
                tx.serialize().len() <= bound,
                "index {index}: {} bytes over a {bound} bound",
                tx.serialize().len()
            );
        }
    }

    #[test]
    fn change_that_would_be_dust_becomes_fee_rather_than_an_unrelayable_output() {
        // Enough to pay the amount and the fee, with only a few satoshis over.
        let outputs = vec![
            TxOutput {
                address: recipient(),
                sats: 1,
            },
            TxOutput {
                address: recipient(),
                sats: 1,
            },
        ];
        let with_change = FEE_PER_BYTE * signed_size(1, &outputs) as u64;
        let funding = 10_000 + with_change + 100;
        let tx = Builder::new()
            .build_transfer(
                &[utxo(1, funding)],
                &recipient(),
                10_000,
                &signer().address(EcashNetwork::Testnet),
                &signer(),
            )
            .unwrap();
        assert_eq!(tx.outputs.len(), 1, "no dust change output");
        assert_eq!(tx.fee(), i128::from(funding) - 10_000);
    }

    #[test]
    fn more_inputs_are_taken_until_the_fee_is_covered() {
        let utxos: Vec<Utxo> = (1..=6).map(|b| utxo(b, 20_000)).collect();
        let tx = Builder::new()
            .build_transfer(
                &utxos,
                &recipient(),
                100_000,
                &signer().address(EcashNetwork::Testnet),
                &signer(),
            )
            .unwrap();
        // Five inputs is 100,000 exactly, which leaves nothing for the fee.
        assert_eq!(tx.inputs.len(), 6);
        assert!(tx.fee() > 0);
    }

    #[test]
    fn selection_is_stable_so_a_retry_rebuilds_the_same_transaction() {
        let a: Vec<Utxo> = (1..=4).map(|b| utxo(b, 30_000)).collect();
        let mut b = a.clone();
        b.reverse();
        let build = |set: &[Utxo]| {
            Builder::new()
                .build_transfer(
                    set,
                    &recipient(),
                    50_000,
                    &signer().address(EcashNetwork::Testnet),
                    &signer(),
                )
                .unwrap()
                .txid()
        };
        assert_eq!(build(&a), build(&b));
    }

    #[test]
    fn an_amount_below_the_dust_limit_is_refused_with_the_number() {
        let err = Builder::new()
            .build_transfer(
                &[utxo(1, 100_000)],
                &recipient(),
                DUST_LIMIT - 1,
                &signer().address(EcashNetwork::Testnet),
                &signer(),
            )
            .unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAmount);
        assert!(err.message.contains("546"), "{}", err.message);
    }

    #[test]
    fn an_address_with_nothing_in_it_says_so() {
        let err = Builder::new()
            .build_transfer(
                &[],
                &recipient(),
                10_000,
                &signer().address(EcashNetwork::Testnet),
                &signer(),
            )
            .unwrap_err();
        assert_eq!(err.code, error::Code::InsufficientFunds);
    }

    #[test]
    fn a_transfer_beyond_the_balance_names_what_is_there() {
        let err = Builder::new()
            .build_transfer(
                &[utxo(1, 10_000)],
                &recipient(),
                50_000,
                &signer().address(EcashNetwork::Testnet),
                &signer(),
            )
            .unwrap_err();
        assert_eq!(err.code, error::Code::InsufficientFunds);
        assert!(err.message.contains("10000"), "{}", err.message);
    }

    /// An address paid in dust builds a transaction whose fee is mostly the
    /// cost of the inputs, and the ceiling is what stops it being signed.
    #[test]
    fn a_fee_that_runs_past_the_ceiling_is_refused_before_anything_is_signed() {
        // Sixty outputs of 900 satoshis: 54,000 satoshis held, and about
        // 8,900 of it goes on the fee to move any of it.
        let dust: Vec<Utxo> = (1..=60).map(|b| utxo(b, 900)).collect();
        let own = signer().address(EcashNetwork::Mainnet);

        // With the network's own ceiling this is allowed: it is an expensive
        // transfer, not a runaway one.
        let swept = Builder::new()
            .limit_fee(ECASH_MAINNET, ECASH_MAINNET.max_fee)
            .build_transfer(
                &dust,
                &recipient().on(EcashNetwork::Mainnet),
                40_000,
                &own,
                &signer(),
            )
            .unwrap();
        assert!(swept.inputs.len() > 50, "{} inputs", swept.inputs.len());
        assert!(swept.fee() > 7_000, "fee {}", swept.fee());

        // Held to a thousand satoshis it is refused, in XEC, before a key is
        // asked for anything.
        let err = Builder::new()
            .limit_fee(ECASH_MAINNET, 1_000)
            .build_transfer(
                &dust,
                &recipient().on(EcashNetwork::Mainnet),
                40_000,
                &own,
                &signer(),
            )
            .unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAmount);
        assert!(err.message.contains("XEC"), "{}", err.message);
        assert!(err.message.contains("--max-fee"), "{}", err.message);
    }

    /// The sweep path folds the leftover into the fee, and it too is held to
    /// the ceiling — the leftover is bounded by the dust limit, but the fee it
    /// is added to is not.
    #[test]
    fn the_sweep_path_is_held_to_the_ceiling_as_well() {
        let outputs = vec![TxOutput {
            address: recipient().on(EcashNetwork::Mainnet),
            sats: DUST_LIMIT,
        }];
        let size = signed_size(1, &outputs) as u64;
        // Funded so that keeping change would leave under the dust limit.
        let funding = DUST_LIMIT
            + FEE_PER_BYTE * signed_size(1, &[outputs[0].clone(), outputs[0].clone()]) as u64
            + 1;
        let err = Builder::new()
            .limit_fee(ECASH_MAINNET, u128::from(size / 2))
            .build_transfer(
                &[utxo(1, funding)],
                &recipient().on(EcashNetwork::Mainnet),
                DUST_LIMIT,
                &signer().address(EcashNetwork::Mainnet),
                &signer(),
            )
            .unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAmount);
    }

    /// A txid is the reverse of the hash, and a wallet that forgets that hands
    /// out a hash no explorer can find.
    #[test]
    fn the_transaction_id_is_the_reversed_hash() {
        let tx = Builder::new()
            .build_transfer(
                &[utxo(1, 100_000)],
                &recipient(),
                50_000,
                &signer().address(EcashNetwork::Testnet),
                &signer(),
            )
            .unwrap();
        let mut expected = sha256d(&tx.serialize());
        expected.reverse();
        assert_eq!(tx.txid(), hex::encode(expected));
        assert_eq!(tx.txid().len(), 64);
    }

    /// The serialisation is checked field by field against the format, because
    /// there is no CBOR library or node to catch a misplaced byte here.
    #[test]
    fn the_wire_format_is_laid_out_where_the_protocol_says() {
        let to = recipient();
        let tx = Builder::new()
            .build_transfer(
                &[utxo(0xab, 100_000)],
                &to,
                50_000,
                &signer().address(EcashNetwork::Testnet),
                &signer(),
            )
            .unwrap();
        let bytes = tx.serialize();

        assert_eq!(&bytes[0..4], &2u32.to_le_bytes(), "version 2");
        assert_eq!(bytes[4], 1, "one input");
        assert_eq!(&bytes[5..37], &[0xab; 32], "the outpoint's txid");
        assert_eq!(&bytes[37..41], &0u32.to_le_bytes(), "output index 0");
        let script_len = bytes[41] as usize;
        let after_script = 42 + script_len;
        assert_eq!(
            &bytes[after_script..after_script + 4],
            &SEQUENCE_FINAL.to_le_bytes(),
            "a final sequence"
        );
        assert_eq!(bytes[after_script + 4], 2, "two outputs");
        assert_eq!(
            &bytes[after_script + 5..after_script + 13],
            &50_000u64.to_le_bytes(),
            "the amount, little-endian"
        );
        assert_eq!(&bytes[bytes.len() - 4..], &0u32.to_le_bytes(), "locktime 0");

        // The signature push carries the sighash byte at its end, which is
        // what tells a verifier which digest to rebuild.
        let script_sig = &tx.script_sigs[0];
        let sig_len = script_sig[0] as usize;
        assert_eq!(script_sig[sig_len], SIGHASH_ALL_FORKID as u8);
        assert_eq!(script_sig[sig_len + 1], 33, "the compressed key's push");
    }

    /// Paying a script-hash address is a different locking script and a
    /// different output size, and the fee has to follow it.
    #[test]
    fn a_script_hash_recipient_is_paid_with_its_own_script() {
        let p2sh = Address::parse("ecash:pquc59839pv8fga4h8eayy5fty0s00aj5czp4d547x").unwrap();
        assert_eq!(p2sh.kind, AddressKind::P2sh);
        let tx = Builder::new()
            .build_transfer(
                &[utxo(1, 100_000)],
                &p2sh,
                50_000,
                &signer().address(EcashNetwork::Mainnet),
                &signer(),
            )
            .unwrap();
        assert_eq!(tx.outputs[0].address.script_pubkey().len(), 23);
        assert!(tx.serialize().len() <= signed_size(1, &tx.outputs));
    }
}
