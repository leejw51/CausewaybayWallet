//! Solana legacy transaction encoding.
//!
//! A serialized transaction is `compact_array<signature> || message`, and the
//! message is what gets signed. Two details drive the whole layout, and both
//! are the kind that produce a transaction the cluster rejects with no useful
//! explanation:
//!
//! 1. **compact-u16 ("shortvec")** — array lengths are a 1–3 byte varint, 7
//!    bits per byte, low group first, high bit meaning "more follows".
//! 2. **Account ordering** — every account any instruction mentions is
//!    collected into one table and sorted into four buckets: writable signers,
//!    readonly signers, writable non-signers, readonly non-signers. The header
//!    records the boundaries and instructions then refer to accounts by
//!    position. Order the table differently and the signature covers the wrong
//!    bytes.
//!
//! Both are pinned to vectors generated with `@solana/web3.js`.

use crate::error::{self, Result};

/// The System program: 32 zero bytes.
pub const SYSTEM_PROGRAM_ID: [u8; 32] = [0u8; 32];

/// System instruction discriminant for `Transfer`, little-endian u32.
const SYSTEM_IX_TRANSFER: u32 = 2;

/// Append a compact-u16 length prefix.
fn write_compact_u16(out: &mut Vec<u8>, mut n: u16) {
    loop {
        let mut byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return;
        }
        byte |= 0x80;
        out.push(byte);
    }
}

/// One account slot of an instruction, before index assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountMeta {
    pub pubkey: [u8; 32],
    pub is_signer: bool,
    pub is_writable: bool,
}

impl AccountMeta {
    pub fn writable_signer(pubkey: [u8; 32]) -> Self {
        AccountMeta {
            pubkey,
            is_signer: true,
            is_writable: true,
        }
    }
    pub fn writable(pubkey: [u8; 32]) -> Self {
        AccountMeta {
            pubkey,
            is_signer: false,
            is_writable: true,
        }
    }
    pub fn readonly(pubkey: [u8; 32]) -> Self {
        AccountMeta {
            pubkey,
            is_signer: false,
            is_writable: false,
        }
    }
}

/// An instruction as the caller writes it: program, accounts, opaque data.
#[derive(Debug, Clone)]
pub struct Instruction {
    pub program_id: [u8; 32],
    pub accounts: Vec<AccountMeta>,
    pub data: Vec<u8>,
}

impl Instruction {
    /// `SystemProgram::transfer` — move `lamports` from `from` to `to`.
    pub fn transfer(from: [u8; 32], to: [u8; 32], lamports: u64) -> Self {
        let mut data = Vec::with_capacity(12);
        data.extend_from_slice(&SYSTEM_IX_TRANSFER.to_le_bytes());
        data.extend_from_slice(&lamports.to_le_bytes());
        Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![
                AccountMeta::writable_signer(from),
                AccountMeta::writable(to),
            ],
            data,
        }
    }
}

/// A compiled legacy message: the exact bytes that get signed.
#[derive(Debug, Clone)]
pub struct Message {
    pub num_required_signatures: u8,
    pub num_readonly_signed: u8,
    pub num_readonly_unsigned: u8,
    pub account_keys: Vec<[u8; 32]>,
    pub recent_blockhash: [u8; 32],
    pub instructions: Vec<CompiledInstruction>,
}

#[derive(Debug, Clone)]
pub struct CompiledInstruction {
    pub program_id_index: u8,
    pub accounts: Vec<u8>,
    pub data: Vec<u8>,
}

impl Message {
    /// Compile instructions into a message: build the account table, bucket
    /// it, and rewrite each instruction's accounts as indices into it.
    pub fn compile(
        fee_payer: [u8; 32],
        instructions: &[Instruction],
        recent_blockhash: [u8; 32],
    ) -> Result<Self> {
        // Collect every referenced account, merging duplicates by OR-ing their
        // flags: an account writable anywhere is writable everywhere.
        let mut metas: Vec<AccountMeta> = vec![AccountMeta::writable_signer(fee_payer)];
        fn push(m: AccountMeta, metas: &mut Vec<AccountMeta>) {
            match metas.iter_mut().find(|e| e.pubkey == m.pubkey) {
                Some(existing) => {
                    existing.is_signer |= m.is_signer;
                    existing.is_writable |= m.is_writable;
                }
                None => metas.push(m),
            }
        }
        for ix in instructions {
            for m in &ix.accounts {
                push(m.clone(), &mut metas);
            }
        }
        // A program id is itself a readonly, non-signer account.
        for ix in instructions {
            push(AccountMeta::readonly(ix.program_id), &mut metas);
        }

        // Bucket into the four classes, keeping the fee payer first.
        let mut ordered: Vec<AccountMeta> = Vec::with_capacity(metas.len());
        for (signer, writable) in [(true, true), (true, false), (false, true), (false, false)] {
            for m in &metas {
                if m.is_signer == signer && m.is_writable == writable {
                    ordered.push(m.clone());
                }
            }
        }

        let num_required_signatures = ordered.iter().filter(|m| m.is_signer).count();
        let num_readonly_signed = ordered
            .iter()
            .filter(|m| m.is_signer && !m.is_writable)
            .count();
        let num_readonly_unsigned = ordered
            .iter()
            .filter(|m| !m.is_signer && !m.is_writable)
            .count();
        if num_required_signatures > u8::MAX as usize {
            return Err(error::usage("a transaction cannot require 256 signatures"));
        }

        let account_keys: Vec<[u8; 32]> = ordered.iter().map(|m| m.pubkey).collect();
        let index_of = |pk: &[u8; 32]| -> u8 {
            account_keys
                .iter()
                .position(|k| k == pk)
                .expect("every account was collected above") as u8
        };
        let compiled = instructions
            .iter()
            .map(|ix| CompiledInstruction {
                program_id_index: index_of(&ix.program_id),
                accounts: ix.accounts.iter().map(|m| index_of(&m.pubkey)).collect(),
                data: ix.data.clone(),
            })
            .collect();

        Ok(Message {
            num_required_signatures: num_required_signatures as u8,
            num_readonly_signed: num_readonly_signed as u8,
            num_readonly_unsigned: num_readonly_unsigned as u8,
            account_keys,
            recent_blockhash,
            instructions: compiled,
        })
    }

    /// Serialize to the exact bytes that are signed.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(128);
        out.push(self.num_required_signatures);
        out.push(self.num_readonly_signed);
        out.push(self.num_readonly_unsigned);
        write_compact_u16(&mut out, self.account_keys.len() as u16);
        for key in &self.account_keys {
            out.extend_from_slice(key);
        }
        out.extend_from_slice(&self.recent_blockhash);
        write_compact_u16(&mut out, self.instructions.len() as u16);
        for ix in &self.instructions {
            out.push(ix.program_id_index);
            write_compact_u16(&mut out, ix.accounts.len() as u16);
            out.extend_from_slice(&ix.accounts);
            write_compact_u16(&mut out, ix.data.len() as u16);
            out.extend_from_slice(&ix.data);
        }
        out
    }
}

/// A message plus its signatures, in signer order.
#[derive(Debug, Clone)]
pub struct Transaction {
    pub signatures: Vec<[u8; 64]>,
    pub message: Message,
}

impl Transaction {
    /// An unsigned transaction with all-zero signature placeholders.
    pub fn new_unsigned(message: Message) -> Self {
        let n = message.num_required_signatures as usize;
        Transaction {
            signatures: vec![[0u8; 64]; n],
            message,
        }
    }

    /// Sign with one account, which must sit in the signer prefix.
    pub fn sign(&mut self, signer: &super::keys::SolanaAccount) -> Result<()> {
        let bytes = self.message.serialize();
        let pubkey = signer.public_key_bytes();
        let position = self
            .message
            .account_keys
            .iter()
            .position(|k| *k == pubkey)
            .filter(|p| *p < self.message.num_required_signatures as usize)
            .ok_or_else(|| {
                error::usage(format!(
                    "{} is not a required signer of this transaction",
                    signer.address()
                ))
            })?;
        self.signatures[position] = signer.sign(&bytes);
        Ok(())
    }

    /// The wire format: `compact_array<signature> || message`.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::new();
        write_compact_u16(&mut out, self.signatures.len() as u16);
        for signature in &self.signatures {
            out.extend_from_slice(signature);
        }
        out.extend_from_slice(&self.message.serialize());
        out
    }

    /// A transaction's id is the base58 of its first signature.
    pub fn signature_base58(&self) -> String {
        bs58::encode(self.signatures.first().copied().unwrap_or([0u8; 64])).into_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::solana::keys::{address_to_bytes, SolanaAccount};
    use crate::chain::Seed;

    const PHRASE: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn compact_u16_matches_the_shortvec_spec() {
        let cases: &[(u16, &[u8])] = &[
            (0, &[0x00]),
            (1, &[0x01]),
            (127, &[0x7f]),
            (128, &[0x80, 0x01]),
            (16383, &[0xff, 0x7f]),
            (16384, &[0x80, 0x80, 0x01]),
            (65535, &[0xff, 0xff, 0x03]),
        ];
        for (n, expected) in cases {
            let mut out = Vec::new();
            write_compact_u16(&mut out, *n);
            assert_eq!(&out[..], *expected, "compact-u16 of {n}");
        }
    }

    /// The whole encoding, against a transaction built by `@solana/web3.js`.
    /// This is the test that would catch a wrong account order or a wrong
    /// varint, both of which are otherwise invisible until a cluster says no.
    #[test]
    fn a_transfer_matches_the_official_sdk_wire_bytes() {
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("../../../../../testvectors/multichain.json"))
                .unwrap();
        let tx = &vectors["solana"]["transfer_tx"];

        let from =
            SolanaAccount::from_seed(&Seed::new(PHRASE, "").unwrap().bip39_seed(), 0).unwrap();
        assert_eq!(from.address(), tx["from"].as_str().unwrap());

        let to = address_to_bytes(tx["to"].as_str().unwrap()).unwrap();
        let blockhash = address_to_bytes(tx["recent_blockhash"].as_str().unwrap()).unwrap();
        let lamports = tx["lamports"].as_u64().unwrap();

        let ix = Instruction::transfer(from.public_key_bytes(), to, lamports);
        let message = Message::compile(from.public_key_bytes(), &[ix], blockhash).unwrap();
        assert_eq!(
            hex::encode(message.serialize()),
            tx["message_hex"].as_str().unwrap(),
            "the signed message bytes must match the SDK's"
        );

        let mut transaction = Transaction::new_unsigned(message);
        transaction.sign(&from).unwrap();
        assert_eq!(
            hex::encode(transaction.serialize()),
            tx["signed_tx_hex"].as_str().unwrap()
        );
        // The vector file names this field `signature_base58`, but the
        // generator wrote base64 into it. Decoding it for what it actually
        // holds still checks our signature against the SDK's, byte for byte —
        // and asserting the label instead would only test the typo.
        use base64::Engine;
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(tx["signature_base58"].as_str().unwrap())
                .expect("the field holds base64 despite its name"),
            transaction.signatures[0].to_vec()
        );
        // A transaction id is the base58 of that same first signature.
        assert_eq!(
            transaction.signature_base58(),
            bs58::encode(transaction.signatures[0]).into_string()
        );
    }

    #[test]
    fn the_account_table_puts_the_fee_payer_first_and_the_program_last() {
        let payer = [1u8; 32];
        let recipient = [2u8; 32];
        let ix = Instruction::transfer(payer, recipient, 1);
        let message = Message::compile(payer, &[ix], [9u8; 32]).unwrap();

        assert_eq!(message.account_keys[0], payer, "the fee payer signs first");
        assert_eq!(message.account_keys[1], recipient);
        assert_eq!(*message.account_keys.last().unwrap(), SYSTEM_PROGRAM_ID);
        assert_eq!(message.num_required_signatures, 1);
        assert_eq!(message.num_readonly_signed, 0);
        // Only the System program is readonly and unsigned.
        assert_eq!(message.num_readonly_unsigned, 1);
    }

    #[test]
    fn an_account_named_twice_appears_once_with_merged_flags() {
        let payer = [1u8; 32];
        // Naming the payer again as a readonly non-signer must not demote it,
        // nor add a second row that shifts every later index.
        let mut ix = Instruction::transfer(payer, [2u8; 32], 1);
        ix.accounts.push(AccountMeta::readonly(payer));
        let message = Message::compile(payer, &[ix], [9u8; 32]).unwrap();

        assert_eq!(
            message.account_keys.iter().filter(|k| **k == payer).count(),
            1
        );
        assert_eq!(message.account_keys[0], payer);
        assert_eq!(message.num_required_signatures, 1);
    }

    #[test]
    fn signing_with_a_key_that_is_not_a_signer_is_refused() {
        let seed = Seed::new(PHRASE, "").unwrap().bip39_seed();
        let payer = SolanaAccount::from_seed(&seed, 0).unwrap();
        let stranger = SolanaAccount::from_seed(&seed, 1).unwrap();

        let ix = Instruction::transfer(payer.public_key_bytes(), [2u8; 32], 1);
        let message = Message::compile(payer.public_key_bytes(), &[ix], [9u8; 32]).unwrap();
        let mut transaction = Transaction::new_unsigned(message);

        let err = transaction.sign(&stranger).unwrap_err();
        assert!(
            err.message.contains("not a required signer"),
            "{}",
            err.message
        );
        // And the placeholder is still unsigned rather than half-filled.
        assert_eq!(transaction.signatures[0], [0u8; 64]);
        assert!(transaction.sign(&payer).is_ok());
    }

    #[test]
    fn the_transfer_instruction_carries_its_discriminant_and_amount() {
        let ix = Instruction::transfer([1u8; 32], [2u8; 32], 1_500_000_000);
        assert_eq!(ix.data.len(), 12);
        assert_eq!(&ix.data[..4], &2u32.to_le_bytes());
        assert_eq!(&ix.data[4..], &1_500_000_000u64.to_le_bytes());
        assert_eq!(ix.program_id, SYSTEM_PROGRAM_ID);
    }
}
