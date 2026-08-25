//! The wire format Chronik speaks, which is protobuf rather than JSON.
//!
//! Chronik is Bitcoin ABC's own indexer and the only key-less service that
//! serves eCash's mainnet *and* its testnet — but it answers in
//! `application/x-protobuf` and has no JSON mode. Rather than pull in a code
//! generator and a runtime for four small messages, the handful of fields this
//! wallet reads are decoded here by hand.
//!
//! That is a deliberate trade and it has a cost: protobuf field *numbers* are
//! the contract, and nothing in this file would notice if Chronik renumbered
//! one. So the tests below decode responses captured from the live mainnet
//! indexer, byte for byte, and check the numbers they carry against the same
//! transactions read through an unrelated explorer. A renumbering breaks them
//! rather than silently changing a balance.
//!
//! Only the fields the wallet acts on are named. Protobuf is defined so that
//! an unknown field is skipped rather than fatal, which is what lets Chronik
//! add fields — as it has, repeatedly — without this stopping working.

use crate::error::{self, Result};

// ============================================================ the wire format

/// One field of a protobuf message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field<'a> {
    Varint(u32, u64),
    Bytes(u32, &'a [u8]),
    Fixed32(u32, u32),
    Fixed64(u32, u64),
}

impl Field<'_> {
    pub fn number(&self) -> u32 {
        match self {
            Field::Varint(n, _)
            | Field::Bytes(n, _)
            | Field::Fixed32(n, _)
            | Field::Fixed64(n, _) => *n,
        }
    }
}

/// A cursor over a protobuf message.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    fn varint(&mut self) -> Result<u64> {
        let mut value: u64 = 0;
        let mut shift = 0;
        loop {
            let byte = *self
                .data
                .get(self.pos)
                .ok_or_else(|| malformed("a value runs off the end of the message"))?;
            self.pos += 1;
            // Ten groups of seven bits is the most a u64 can hold, and a
            // longer one is a hostile or corrupt message rather than a large
            // number.
            if shift >= 64 {
                return Err(malformed("a number in the reply is longer than 64 bits"));
            }
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
        }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(len)
            .filter(|end| *end <= self.data.len())
            .ok_or_else(|| malformed("a length-delimited field runs off the end"))?;
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    /// The next field, or `None` at the end of the message.
    pub fn next_field(&mut self) -> Result<Option<Field<'a>>> {
        if self.pos >= self.data.len() {
            return Ok(None);
        }
        let tag = self.varint()?;
        let number = u32::try_from(tag >> 3)
            .map_err(|_| malformed("a field number in the reply is out of range"))?;
        match tag & 0x07 {
            0 => Ok(Some(Field::Varint(number, self.varint()?))),
            1 => {
                let bytes = self.take(8)?;
                Ok(Some(Field::Fixed64(
                    number,
                    u64::from_le_bytes(bytes.try_into().expect("8 bytes")),
                )))
            }
            2 => {
                let len = usize::try_from(self.varint()?)
                    .map_err(|_| malformed("a field in the reply is impossibly long"))?;
                Ok(Some(Field::Bytes(number, self.take(len)?)))
            }
            5 => {
                let bytes = self.take(4)?;
                Ok(Some(Field::Fixed32(
                    number,
                    u32::from_le_bytes(bytes.try_into().expect("4 bytes")),
                )))
            }
            // Groups (3 and 4) were removed from protobuf in 2008 and Chronik
            // emits none; 6 and 7 have never been assigned.
            other => Err(malformed(format!(
                "the reply uses wire type {other}, which this wallet does not read"
            ))),
        }
    }
}

/// Encode one length-delimited field. All this wallet ever writes.
pub fn write_bytes_field(out: &mut Vec<u8>, number: u32, value: &[u8]) {
    write_varint(out, u64::from(number) << 3 | 2);
    write_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn malformed(what: impl std::fmt::Display) -> crate::error::Error {
    error::rpc_error(format!("the indexer's reply could not be read: {what}"))
}

// ================================================================ the messages

/// `BlockchainInfo`: where the chain's tip is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockchainInfo {
    pub tip_hash: String,
    pub tip_height: u64,
}

impl BlockchainInfo {
    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut info = BlockchainInfo::default();
        let mut reader = Reader::new(data);
        while let Some(field) = reader.next_field()? {
            match field {
                Field::Bytes(1, hash) => info.tip_hash = reversed_hex(hash),
                Field::Varint(2, height) => info.tip_height = height,
                _ => {}
            }
        }
        Ok(info)
    }
}

/// One entry of `ScriptUtxos`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Utxo {
    /// Wire order, as a transaction spends it — not the reversed display form.
    pub txid: [u8; 32],
    pub out_idx: u32,
    pub sats: u64,
    pub block_height: i64,
    pub is_coinbase: bool,
    /// Whether this output carries an eToken.
    ///
    /// The single most important field on this message for a wallet that does
    /// not handle them: an output with a token on it is an ordinary spendable
    /// output as far as the script goes, and spending it destroys the token it
    /// was carrying. See [`spendable`].
    pub has_token: bool,
}

impl Utxo {
    fn decode(data: &[u8]) -> Result<Self> {
        let mut utxo = Utxo::default();
        let mut reader = Reader::new(data);
        while let Some(field) = reader.next_field()? {
            match field {
                Field::Bytes(1, outpoint) => {
                    let mut inner = Reader::new(outpoint);
                    while let Some(field) = inner.next_field()? {
                        match field {
                            Field::Bytes(1, txid) if txid.len() == 32 => {
                                utxo.txid.copy_from_slice(txid)
                            }
                            Field::Bytes(1, txid) => {
                                return Err(malformed(format!(
                                    "an outpoint names a {}-byte transaction id",
                                    txid.len()
                                )))
                            }
                            Field::Varint(2, index) => {
                                utxo.out_idx = u32::try_from(index).map_err(|_| {
                                    malformed("an outpoint's output index is out of range")
                                })?
                            }
                            _ => {}
                        }
                    }
                }
                Field::Varint(2, height) => utxo.block_height = height as i64,
                Field::Varint(3, flag) => utxo.is_coinbase = flag != 0,
                Field::Varint(5, sats) => utxo.sats = sats,
                Field::Bytes(11, _) => utxo.has_token = true,
                _ => {}
            }
        }
        Ok(utxo)
    }
}

/// `ScriptUtxos`: everything unspent at one locking script.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptUtxos {
    pub utxos: Vec<Utxo>,
}

impl ScriptUtxos {
    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut out = ScriptUtxos::default();
        let mut reader = Reader::new(data);
        while let Some(field) = reader.next_field()? {
            if let Field::Bytes(2, utxo) = field {
                out.utxos.push(Utxo::decode(utxo)?);
            }
        }
        Ok(out)
    }

    /// The total held at this script, tokens and all.
    pub fn total(&self) -> u128 {
        self.utxos.iter().map(|u| u128::from(u.sats)).sum()
    }

    /// The outputs this wallet is willing to spend, given the chain's tip.
    ///
    /// Two kinds are held back, and for opposite reasons:
    ///
    /// * an output carrying an **eToken** is spendable and must not be spent,
    ///   because the token rides on the output and spending it as plain XEC
    ///   burns it. This wallet does not move tokens, so it does not touch
    ///   their outputs at all;
    /// * a **coinbase** output is not spendable until a hundred blocks have
    ///   buried it. Including one builds a transaction that is rejected, and
    ///   the reason a node gives for that is not a helpful one.
    pub fn spendable(&self, tip_height: u64) -> Vec<Utxo> {
        const COINBASE_MATURITY: i64 = 100;
        self.utxos
            .iter()
            .filter(|utxo| !utxo.has_token)
            .filter(|utxo| {
                if !utxo.is_coinbase {
                    return true;
                }
                // A height of 0 or below means it is still in the mempool,
                // which no coinbase output ever is.
                utxo.block_height > 0
                    && tip_height as i64 - utxo.block_height + 1 >= COINBASE_MATURITY
            })
            .copied()
            .collect()
    }
}

/// The parts of Chronik's `Tx` a wallet asks about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tx {
    pub txid: String,
    pub block_height: Option<u64>,
    pub size: u64,
    pub is_final: bool,
    /// Inputs minus outputs, which is what a Bitcoin-format fee is. Chronik
    /// reports no fee field; there is nothing to report, only arithmetic.
    pub fee: Option<u128>,
}

impl Tx {
    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut tx = Tx::default();
        let mut inputs: i128 = 0;
        let mut outputs: i128 = 0;
        let mut saw_inputs = false;
        let mut reader = Reader::new(data);
        while let Some(field) = reader.next_field()? {
            match field {
                Field::Bytes(1, txid) => tx.txid = reversed_hex(txid),
                Field::Bytes(3, input) => {
                    saw_inputs = true;
                    inputs += i128::from(sats_of(input, 4)?);
                }
                Field::Bytes(4, output) => outputs += i128::from(sats_of(output, 1)?),
                Field::Bytes(8, block) => {
                    let mut inner = Reader::new(block);
                    while let Some(field) = inner.next_field()? {
                        if let Field::Varint(1, height) = field {
                            tx.block_height = Some(height);
                        }
                    }
                }
                Field::Varint(11, size) => tx.size = size,
                Field::Varint(15, flag) => tx.is_final = flag != 0,
                _ => {}
            }
        }
        // A coinbase transaction has an input worth nothing and mints the
        // block reward, so its outputs exceed its inputs and there is no fee
        // to report. Anything else with a negative fee is a reply to distrust.
        tx.fee = (saw_inputs && inputs >= outputs)
            .then(|| u128::try_from(inputs - outputs).ok())
            .flatten();
        Ok(tx)
    }
}

/// The `sats` field of a `TxInput` or a `TxOutput`, which are numbered apart.
fn sats_of(data: &[u8], number: u32) -> Result<u64> {
    let mut reader = Reader::new(data);
    while let Some(field) = reader.next_field()? {
        if let Field::Varint(n, sats) = field {
            if n == number {
                return Ok(sats);
            }
        }
    }
    Ok(0)
}

/// `BroadcastTxRequest`, which is the one message this wallet sends.
pub fn broadcast_request(raw_tx: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw_tx.len() + 8);
    write_bytes_field(&mut out, 1, raw_tx);
    out
}

/// The transaction id out of a `BroadcastTxResponse`.
pub fn broadcast_response(data: &[u8]) -> Result<Option<String>> {
    let mut reader = Reader::new(data);
    while let Some(field) = reader.next_field()? {
        if let Field::Bytes(1, txid) = field {
            return Ok(Some(reversed_hex(txid)));
        }
    }
    Ok(None)
}

/// The message out of Chronik's `Error`, which is what a 4xx body carries.
///
/// Chronik puts the reason a transaction was rejected in here — "txn-mempool-
/// conflict", "dust", "insufficient priority" — and it is the only part of a
/// failed broadcast worth showing.
pub fn error_message(data: &[u8]) -> Option<String> {
    let mut reader = Reader::new(data);
    while let Ok(Some(field)) = reader.next_field() {
        if let Field::Bytes(2, message) = field {
            return std::str::from_utf8(message)
                .ok()
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty());
        }
    }
    None
}

/// A hash the way an explorer prints it: the wire bytes, backwards.
fn reversed_hex(bytes: &[u8]) -> String {
    let mut reversed = bytes.to_vec();
    reversed.reverse();
    hex::encode(reversed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `GET /blockchain-info` from `chronik.e.cash`.
    const BLOCKCHAIN_INFO: &str =
        "0a20b6aa884924cda956712548f44f60914d40d3727f08be5f56000000000000000010fe\
                              e93a";

    /// `GET /script/p2sh/398a…b2a6/utxos` — one output, and it carries an
    /// eToken.
    const TOKEN_UTXOS: &str = "0a17a914398a14f1285874a3b5b9f3d21289591f07bfb2a6871286010a240a202bdb89d0\
                              a2964c5f929cd4b9c9cbf849c7c18ba470f8b925665926110d1163ff100110fee93a28a2\
                              0450015a550a406165643836316133316239363933346238386330323532656465313335\
                              636239373030643736343966363931393132333530383761333033306535353363623112\
                              02080118ffffffffffffffffff0120a09c01";

    /// `GET /script/p2pkh/2fd0…ef80/utxos` — one plain output, spendable.
    const PLAIN_UTXOS: &str = "0a1976a9142fd0a84cd8d80a62681ab940a24ad42bde61ef8088ac12300a220a20de5f4f\
                              637ba8be91ffcee9ba0b85ee719b692a9e3e43df9cf9309396156269f71080ea3a28f9c0\
                              91ab015001";

    fn bytes(hex_text: &str) -> Vec<u8> {
        hex::decode(hex_text.replace([' ', '\n'], "")).unwrap()
    }

    /// The tip, cross-checked against the same block read through an
    /// unrelated explorer: height 963,838, hash `0000…aab6`.
    #[test]
    fn the_tip_decodes_to_the_block_another_explorer_names() {
        let info = BlockchainInfo::decode(&bytes(BLOCKCHAIN_INFO)).unwrap();
        assert_eq!(info.tip_height, 963_838);
        assert_eq!(
            info.tip_hash,
            "0000000000000000565fbe087f72d3404d91604ff448257156a9cd244988aab6"
        );
    }

    /// The number that has to be right or the wallet spends the wrong amount:
    /// 358,899,833 satoshis, which is the value of the output this unspent
    /// entry points at.
    #[test]
    fn an_unspent_output_decodes_to_the_value_the_chain_holds() {
        let script = ScriptUtxos::decode(&bytes(PLAIN_UTXOS)).unwrap();
        assert_eq!(script.utxos.len(), 1);
        let utxo = script.utxos[0];
        assert_eq!(utxo.sats, 358_899_833);
        assert_eq!(utxo.block_height, 963_840);
        assert_eq!(utxo.out_idx, 0);
        assert!(!utxo.has_token);
        assert!(!utxo.is_coinbase);
        // The txid in display order, which is the wire bytes reversed.
        assert_eq!(
            reversed_hex(&utxo.txid),
            "f7696215969330f99cdf433e9e2a699b71ee850bbae9ceff91bea87b634f5fde"
        );
        assert_eq!(script.total(), 358_899_833);
        assert_eq!(script.spendable(963_900).len(), 1);
    }

    /// An output carrying an eToken counts towards the balance and is never
    /// selected to spend, because spending it as plain XEC burns the token.
    #[test]
    fn an_output_carrying_a_token_is_counted_but_never_spent() {
        let script = ScriptUtxos::decode(&bytes(TOKEN_UTXOS)).unwrap();
        assert_eq!(script.utxos.len(), 1);
        assert!(script.utxos[0].has_token);
        assert_eq!(script.utxos[0].sats, 546);
        assert_eq!(script.total(), 546);
        assert!(script.spendable(999_999).is_empty());
    }

    #[test]
    fn an_immature_coinbase_output_is_held_back_until_a_hundred_blocks_bury_it() {
        let mut script = ScriptUtxos::decode(&bytes(PLAIN_UTXOS)).unwrap();
        script.utxos[0].is_coinbase = true;
        let mined_at = script.utxos[0].block_height as u64;
        assert!(script.spendable(mined_at).is_empty(), "in its own block");
        assert!(
            script.spendable(mined_at + 98).is_empty(),
            "99 confirmations is not enough"
        );
        assert_eq!(script.spendable(mined_at + 99).len(), 1, "100 is");
    }

    /// An unknown field is skipped rather than fatal, which is what lets
    /// Chronik keep adding them.
    #[test]
    fn a_field_this_wallet_does_not_know_is_skipped() {
        let mut padded = bytes(BLOCKCHAIN_INFO);
        write_bytes_field(&mut padded, 99, b"something added later");
        padded.extend_from_slice(&[0xd8, 0x01, 0x2a]); // field 27, varint 42
        let info = BlockchainInfo::decode(&padded).unwrap();
        assert_eq!(info.tip_height, 963_838);
    }

    #[test]
    fn a_truncated_reply_is_an_error_rather_than_a_wrong_number() {
        let full = bytes(BLOCKCHAIN_INFO);
        for cut in 1..full.len() {
            // Decoding a prefix must never panic; it either reads the fields
            // that survived whole or reports the reply as unreadable.
            let _ = BlockchainInfo::decode(&full[..cut]);
        }
        // A length that claims more than is there is the case that matters.
        let mut lying = vec![0x0a, 0x40];
        lying.extend_from_slice(&full[2..]);
        let err = BlockchainInfo::decode(&lying).unwrap_err();
        assert_eq!(err.code, error::Code::RpcError);
    }

    #[test]
    fn a_varint_longer_than_a_u64_is_refused_rather_than_wrapped() {
        let runaway = vec![
            0x10, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f,
        ];
        assert!(BlockchainInfo::decode(&runaway).is_err());
    }

    #[test]
    fn a_broadcast_request_wraps_the_raw_transaction_in_field_one() {
        assert_eq!(broadcast_request(&[1, 2, 3]), vec![0x0a, 0x03, 1, 2, 3]);
        // And the reply's txid comes back in display order.
        let mut reply = Vec::new();
        let mut txid = [0u8; 32];
        txid[0] = 0xff;
        write_bytes_field(&mut reply, 1, &txid);
        assert_eq!(
            broadcast_response(&reply).unwrap().unwrap(),
            "00000000000000000000000000000000000000000000000000000000000000ff"
        );
    }

    /// The two error bodies Chronik actually returns, captured from the live
    /// indexer: a missing transaction and a rejected broadcast.
    #[test]
    fn an_error_body_gives_up_the_reason() {
        let mut not_found = Vec::new();
        write_bytes_field(
            &mut not_found,
            2,
            b"404: Transaction 0000 not found in the index",
        );
        assert_eq!(
            error_message(&not_found).as_deref(),
            Some("404: Transaction 0000 not found in the index")
        );

        let mut rejected = Vec::new();
        write_bytes_field(
            &mut rejected,
            2,
            b"400: Broadcast failed: Transaction rejected by mempool: dust",
        );
        assert!(error_message(&rejected).unwrap().contains("dust"));

        assert_eq!(error_message(&[]), None);
    }

    #[test]
    fn a_transaction_decodes_its_height_and_the_fee_its_values_imply() {
        // Inputs 900 and 100, one output of 950: a fee of 50.
        let input = vec![0x20, 0x84, 0x07]; // field 4, varint 900
        let second = vec![0x20, 0x64]; // 100
        let output = vec![0x08, 0xb6, 0x07]; // field 1, 950
        let block = vec![0x08, 0xfe, 0xe9, 0x3a]; // height 963838

        let mut tx = Vec::new();
        write_bytes_field(&mut tx, 1, &[7u8; 32]);
        write_bytes_field(&mut tx, 3, &input);
        write_bytes_field(&mut tx, 3, &second);
        write_bytes_field(&mut tx, 4, &output);
        write_bytes_field(&mut tx, 8, &block);
        tx.extend_from_slice(&[0x58, 0xf0, 0x02]); // size 368
        tx.extend_from_slice(&[0x78, 0x01]); // is_final

        let decoded = Tx::decode(&tx).unwrap();
        assert_eq!(decoded.block_height, Some(963_838));
        assert_eq!(decoded.size, 368);
        assert!(decoded.is_final);
        assert_eq!(decoded.fee, Some(50));
        assert_eq!(decoded.txid, hex::encode([7u8; 32]));
    }

    /// A coinbase mints more than it spends, and reporting that as a negative
    /// fee — or as an enormous positive one — would be worse than reporting
    /// none.
    #[test]
    fn a_transaction_that_mints_reports_no_fee() {
        let mut tx = Vec::new();
        write_bytes_field(&mut tx, 3, &[0x20, 0x00]); // an input worth nothing
        write_bytes_field(&mut tx, 4, &[0x08, 0xb6, 0x07]); // 950 out
        assert_eq!(Tx::decode(&tx).unwrap().fee, None);
    }
}
