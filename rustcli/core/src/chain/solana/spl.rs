//! SPL tokens: where a balance lives on Solana, and what moves one.
//!
//! Solana does not keep token balances on the account that owns them. A wallet
//! address holds lamports and nothing else; every token sits in a *separate*
//! account, owned by the token program, that names its mint and its owner. So
//! "the USDC balance of `9WzD…`" is really the balance of a third address
//! derived from those two — the **associated token account**, or ATA — and
//! deriving it correctly is most of what this module does.
//!
//! Two consequences shape everything below, and both are places a naive
//! implementation loses money or fails late:
//!
//! 1. **The recipient's ATA may not exist yet.** Sending USDC to an address
//!    that has never held USDC means creating an account first, and paying its
//!    rent — about 0.002 SOL. A transfer that assumed the account was there
//!    fails at the cluster with `invalid account data`, after signing. So the
//!    transfer is built as up to two instructions, and the rent is quoted to
//!    the user as part of the cost before they agree to it.
//! 2. **Decimals are signed into the instruction.** `TransferChecked` carries
//!    the mint's decimals and the cluster rejects the transaction if they
//!    disagree with the mint. This is a feature, and the reason this wallet
//!    uses it over plain `Transfer`: a registry row with the wrong decimals
//!    becomes a refusal rather than a transfer of a thousand times too much.
//!    The decimals are read from the mint on chain, not taken from the row.
//!
//! Addresses here are program-derived (PDAs), which means "hash the seeds and
//! keep bumping until the result is *not* a point on the ed25519 curve" — an
//! address nobody can hold the key to, which is what lets a program own it.

use sha2::{Digest, Sha256};

use crate::error::{self, Result};

use super::keys::address_to_bytes;
use super::tx::{AccountMeta, Instruction};

/// `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA` — the SPL token program.
pub const TOKEN_PROGRAM_ID: [u8; 32] = [
    6, 221, 246, 225, 215, 101, 161, 147, 217, 203, 225, 70, 206, 235, 121, 172, 28, 180, 133, 237,
    95, 91, 55, 145, 58, 140, 245, 133, 126, 255, 0, 169,
];

/// `ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL` — the associated token
/// account program, which is the only thing that may create an ATA.
pub const ASSOCIATED_TOKEN_PROGRAM_ID: [u8; 32] = [
    140, 151, 37, 143, 78, 36, 137, 241, 187, 61, 16, 41, 20, 142, 13, 131, 11, 90, 19, 153, 218,
    255, 16, 132, 4, 142, 123, 216, 219, 233, 248, 89,
];

/// The System program: 32 zero bytes.
pub const SYSTEM_PROGRAM_ID: [u8; 32] = [0u8; 32];

/// An SPL token account is 165 bytes, which is what its rent is quoted for.
pub const TOKEN_ACCOUNT_LEN: u64 = 165;

/// `TransferChecked`, the token program's instruction 12.
const IX_TRANSFER_CHECKED: u8 = 12;

/// `CreateIdempotent`, the ATA program's instruction 1.
///
/// Idempotent rather than plain `Create` because two sends racing for the same
/// new recipient would otherwise have the second fail on an account the first
/// had just made — a failure that looks like a bug and is not.
const IX_CREATE_IDEMPOTENT: u8 = 1;

/// The marker Solana appends before hashing a program-derived address.
const PDA_MARKER: &[u8] = b"ProgramDerivedAddress";

/// Derive a program address from seeds and a bump, or `None` if the result
/// lands on the ed25519 curve.
///
/// A point on the curve is an address someone could hold the private key to,
/// and a program must not be given one — so the bump is decremented until the
/// hash misses the curve. That is the whole of Solana's PDA construction.
fn create_program_address(seeds: &[&[u8]], bump: u8, program_id: &[u8; 32]) -> Option<[u8; 32]> {
    let mut hasher = Sha256::new();
    for seed in seeds {
        hasher.update(seed);
    }
    hasher.update([bump]);
    hasher.update(program_id);
    hasher.update(PDA_MARKER);
    let hash: [u8; 32] = hasher.finalize().into();
    // Decompressing succeeds exactly when the bytes are a curve point, which
    // is the condition a PDA must fail.
    if ed25519_dalek::VerifyingKey::from_bytes(&hash).is_ok() {
        None
    } else {
        Some(hash)
    }
}

/// The canonical program address for these seeds: the highest bump that works.
///
/// Counting down from 255 is not an optimisation — it is the definition. Two
/// implementations that scanned in different directions would derive two
/// different addresses for one owner and mint, and funds sent to the other one
/// would be unreachable.
pub fn find_program_address(seeds: &[&[u8]], program_id: &[u8; 32]) -> Result<([u8; 32], u8)> {
    for bump in (0..=u8::MAX).rev() {
        if let Some(address) = create_program_address(seeds, bump, program_id) {
            return Ok((address, bump));
        }
    }
    // Astronomically unreachable: every one of 256 hashes would have to be a
    // curve point. Reported rather than panicked on all the same.
    Err(error::internal("no program address exists for those seeds"))
}

/// The associated token account holding `mint` for `owner`.
///
/// Seeds are the owner, the token program and the mint, in that order — the
/// order is part of the derivation, and swapping two of them yields a
/// perfectly valid address that holds nothing.
pub fn associated_token_address(owner: &[u8; 32], mint: &[u8; 32]) -> Result<[u8; 32]> {
    let (address, _bump) = find_program_address(
        &[owner, &TOKEN_PROGRAM_ID, mint],
        &ASSOCIATED_TOKEN_PROGRAM_ID,
    )?;
    Ok(address)
}

/// The same, in the base58 form the RPC speaks.
pub fn associated_token_address_str(owner: &str, mint: &str) -> Result<String> {
    let owner = address_to_bytes(owner)?;
    let mint = address_to_bytes(mint)?;
    Ok(bs58::encode(associated_token_address(&owner, &mint)?).into_string())
}

/// `spl_token::instruction::transfer_checked`.
///
/// The decimals travel with the amount and the cluster checks them against the
/// mint, so a wrong number here is a refused transaction rather than a wrong
/// transfer.
pub fn transfer_checked(
    source: [u8; 32],
    mint: [u8; 32],
    destination: [u8; 32],
    owner: [u8; 32],
    amount: u64,
    decimals: u8,
) -> Instruction {
    let mut data = Vec::with_capacity(10);
    data.push(IX_TRANSFER_CHECKED);
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);
    Instruction {
        program_id: TOKEN_PROGRAM_ID,
        accounts: vec![
            AccountMeta::writable(source),
            AccountMeta::readonly(mint),
            AccountMeta::writable(destination),
            // The owner signs but its lamports do not move, so it is a
            // readonly signer — and it is the fee payer besides, which
            // `Message::compile` merges into one writable-signer slot.
            AccountMeta {
                pubkey: owner,
                is_signer: true,
                is_writable: false,
            },
        ],
        data,
    }
}

/// `spl_associated_token_account::instruction::create_associated_token_account_idempotent`.
///
/// `funder` pays the new account's rent; `owner` will own it and need not sign.
pub fn create_associated_token_account(
    funder: [u8; 32],
    owner: [u8; 32],
    mint: [u8; 32],
    associated: [u8; 32],
) -> Instruction {
    Instruction {
        program_id: ASSOCIATED_TOKEN_PROGRAM_ID,
        accounts: vec![
            AccountMeta::writable_signer(funder),
            AccountMeta::writable(associated),
            AccountMeta::readonly(owner),
            AccountMeta::readonly(mint),
            AccountMeta::readonly(SYSTEM_PROGRAM_ID),
            AccountMeta::readonly(TOKEN_PROGRAM_ID),
        ],
        data: vec![IX_CREATE_IDEMPOTENT],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USDC_MAINNET: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    #[test]
    fn the_program_ids_are_the_addresses_solana_publishes() {
        // Written as bytes above so they cost nothing at runtime; checked here
        // against the base58 every Solana tool prints, because a transposed
        // byte would build instructions for a program that does not exist.
        assert_eq!(
            bs58::encode(TOKEN_PROGRAM_ID).into_string(),
            "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
        );
        assert_eq!(
            bs58::encode(ASSOCIATED_TOKEN_PROGRAM_ID).into_string(),
            "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"
        );
    }

    const USDT_MAINNET: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";

    /// Pinned against mainnet itself, not against another implementation.
    ///
    /// Each address below was derived here and then read back from the cluster
    /// with `getAccountInfo`, which reported the very owner and mint it was
    /// derived from. That is the only check that matters: an ATA this wallet
    /// computes differently from everyone else is an address funds go into and
    /// never come out of, and no amount of internal consistency would catch it.
    #[test]
    fn associated_addresses_are_the_accounts_mainnet_actually_holds() {
        for (owner, mint, expected) in [
            (
                "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
                USDC_MAINNET,
                "FGETo8T8wMcN2wCjav8VK6eh3dLk63evNDPxzLSJra8B",
            ),
            (
                "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
                USDT_MAINNET,
                "TB5FCqbNsnuLQgEjUuPaT9qtVPTT4U1A8rvi7qzEj2M",
            ),
            (
                "5tzFkiKscXHK5ZXCGbXZxdw7gTjjD1mBwuoFbhUvuAi9",
                USDC_MAINNET,
                "FzbcyEZ9m8xjtergWgWDq7mfPoHEbboBF791B6cTpzbq",
            ),
        ] {
            assert_eq!(
                associated_token_address_str(owner, mint).unwrap(),
                expected,
                "ATA for {owner}"
            );
        }
    }

    #[test]
    fn an_associated_address_is_never_a_key_anyone_could_hold() {
        // The property that makes a PDA safe to own funds: it is off the
        // curve, so no private key maps to it.
        let owner = address_to_bytes("9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM").unwrap();
        let mint = address_to_bytes(USDC_MAINNET).unwrap();
        let ata = associated_token_address(&owner, &mint).unwrap();
        assert!(ed25519_dalek::VerifyingKey::from_bytes(&ata).is_err());
    }

    #[test]
    fn the_seed_order_is_load_bearing() {
        // Owner, token program, mint — swapping any two yields a valid-looking
        // address that holds nothing, which is why the order is pinned.
        let owner = address_to_bytes("9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM").unwrap();
        let mint = address_to_bytes(USDC_MAINNET).unwrap();
        let right = associated_token_address(&owner, &mint).unwrap();
        let (wrong, _) = find_program_address(
            &[&mint, &TOKEN_PROGRAM_ID, &owner],
            &ASSOCIATED_TOKEN_PROGRAM_ID,
        )
        .unwrap();
        assert_ne!(right, wrong);
    }

    #[test]
    fn one_owner_holds_a_different_account_per_mint() {
        // And the reason the token registry is flat: USDC and USDT for one
        // wallet are two unrelated addresses.
        let owner = address_to_bytes("9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM").unwrap();
        let usdc = address_to_bytes(USDC_MAINNET).unwrap();
        let usdt = address_to_bytes(USDT_MAINNET).unwrap();
        assert_ne!(
            associated_token_address(&owner, &usdc).unwrap(),
            associated_token_address(&owner, &usdt).unwrap()
        );
    }

    #[test]
    fn transfer_checked_carries_the_amount_and_the_decimals() {
        let ix = transfer_checked([1u8; 32], [2u8; 32], [3u8; 32], [4u8; 32], 1_500_000, 6);
        assert_eq!(ix.program_id, TOKEN_PROGRAM_ID);
        assert_eq!(ix.data[0], IX_TRANSFER_CHECKED);
        assert_eq!(
            u64::from_le_bytes(ix.data[1..9].try_into().unwrap()),
            1_500_000
        );
        assert_eq!(ix.data[9], 6);
        assert_eq!(ix.data.len(), 10);
        // Source and destination move; the mint does not, and the owner signs.
        assert!(ix.accounts[0].is_writable && !ix.accounts[0].is_signer);
        assert!(!ix.accounts[1].is_writable);
        assert!(ix.accounts[2].is_writable);
        assert!(ix.accounts[3].is_signer && !ix.accounts[3].is_writable);
    }

    #[test]
    fn creating_an_account_is_idempotent_and_funded_by_the_signer() {
        let ix = create_associated_token_account([1u8; 32], [2u8; 32], [3u8; 32], [4u8; 32]);
        assert_eq!(ix.program_id, ASSOCIATED_TOKEN_PROGRAM_ID);
        assert_eq!(ix.data, vec![IX_CREATE_IDEMPOTENT]);
        assert!(ix.accounts[0].is_signer && ix.accounts[0].is_writable);
        assert!(ix.accounts[1].is_writable);
        // The recipient does not sign for an account made on their behalf.
        assert!(!ix.accounts[2].is_signer);
    }

    /// The two-instruction shape a transfer to a fresh recipient takes.
    ///
    /// Compiled rather than merely built, because compilation is where the
    /// account table is ordered and every instruction is rewritten to point
    /// into it — and an index off by one signs a transfer between two
    /// different accounts than the one that was agreed to.
    #[test]
    fn creating_an_account_and_transferring_compile_into_one_message() {
        use super::super::tx::Message;

        let sender = address_to_bytes("9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM").unwrap();
        let recipient = address_to_bytes("5tzFkiKscXHK5ZXCGbXZxdw7gTjjD1mBwuoFbhUvuAi9").unwrap();
        let mint = address_to_bytes(USDC_MAINNET).unwrap();
        let source = associated_token_address(&sender, &mint).unwrap();
        let destination = associated_token_address(&recipient, &mint).unwrap();

        let message = Message::compile(
            sender,
            &[
                create_associated_token_account(sender, recipient, mint, destination),
                transfer_checked(source, mint, destination, sender, 1_500_000, 6),
            ],
            [7u8; 32],
        )
        .unwrap();

        // The sender pays the fee and is the only signature required — the
        // recipient signs nothing for an account made on their behalf.
        assert_eq!(message.num_required_signatures, 1);
        assert_eq!(message.account_keys[0], sender);
        assert_eq!(message.instructions.len(), 2);

        let key = |i: u8| message.account_keys[i as usize];
        let create = &message.instructions[0];
        assert_eq!(key(create.program_id_index), ASSOCIATED_TOKEN_PROGRAM_ID);
        assert_eq!(key(create.accounts[1]), destination);
        assert_eq!(key(create.accounts[3]), mint);

        let transfer = &message.instructions[1];
        assert_eq!(key(transfer.program_id_index), TOKEN_PROGRAM_ID);
        assert_eq!(key(transfer.accounts[0]), source);
        assert_eq!(key(transfer.accounts[2]), destination);
        assert_eq!(key(transfer.accounts[3]), sender);

        // Both token accounts are written to, and neither signs.
        let writable = |k: &[u8; 32]| {
            let i = message.account_keys.iter().position(|a| a == k).unwrap();
            let signers = message.num_required_signatures as usize;
            let readonly_unsigned = message.num_readonly_unsigned as usize;
            if i < signers {
                i < signers - message.num_readonly_signed as usize
            } else {
                i < message.account_keys.len() - readonly_unsigned
            }
        };
        assert!(writable(&source));
        assert!(writable(&destination));
        assert!(!writable(&mint));
    }

    /// A recipient who already holds the token needs only one instruction,
    /// and no rent is paid for an account that is already there.
    #[test]
    fn transferring_to_an_existing_account_is_one_instruction() {
        use super::super::tx::Message;

        let sender = address_to_bytes("9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM").unwrap();
        let mint = address_to_bytes(USDC_MAINNET).unwrap();
        let source = associated_token_address(&sender, &mint).unwrap();
        let message = Message::compile(
            sender,
            &[transfer_checked(source, mint, [9u8; 32], sender, 1, 6)],
            [7u8; 32],
        )
        .unwrap();
        assert_eq!(message.instructions.len(), 1);
        assert_eq!(
            message.account_keys[message.instructions[0].program_id_index as usize],
            TOKEN_PROGRAM_ID
        );
    }

    #[test]
    fn the_canonical_bump_is_the_highest_one_that_misses_the_curve() {
        let owner = address_to_bytes("9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM").unwrap();
        let mint = address_to_bytes(USDC_MAINNET).unwrap();
        let seeds: [&[u8]; 3] = [&owner, &TOKEN_PROGRAM_ID, &mint];
        let (_, bump) = find_program_address(&seeds, &ASSOCIATED_TOKEN_PROGRAM_ID).unwrap();
        // Nothing above the chosen bump may work, or the derivation would not
        // be canonical and two wallets could disagree on where funds live.
        for higher in (bump + 1)..=u8::MAX {
            assert!(
                create_program_address(&seeds, higher, &ASSOCIATED_TOKEN_PROGRAM_ID).is_none(),
                "bump {higher} also works, so {bump} is not canonical"
            );
        }
    }
}
