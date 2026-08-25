//! The Solana JSON-RPC client.

use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Value};

use crate::chain::http;
use crate::chain::{
    self, Balance, ChainClient, ClientConfig, PreparedTransfer, TransactionStatus, TransferReceipt,
    TransferRequest,
};
use crate::error::{self, Result};
use crate::network::Network;

use super::keys::{address_to_bytes, SolanaAccount};
use super::spl;
use super::tx::{Instruction, Message, Transaction};

pub struct SolanaClient {
    network: Network,
    endpoint: String,
}

impl SolanaClient {
    pub fn new(config: &ClientConfig) -> Self {
        SolanaClient {
            network: config.network,
            endpoint: config.endpoint.trim_end_matches('/').to_string(),
        }
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        http::json_rpc(&self.endpoint, method, params).await
    }

    /// Balance in lamports.
    async fn lamports(&self, address: &str) -> Result<u64> {
        let value = self.call("getBalance", json!([address])).await?;
        value
            .get("value")
            .and_then(Value::as_u64)
            .ok_or_else(|| error::rpc_error(format!("unexpected getBalance reply: {value}")))
    }

    /// The blockhash a new transaction must reference.
    async fn latest_blockhash(&self) -> Result<[u8; 32]> {
        let value = self
            .call("getLatestBlockhash", json!([{"commitment": "finalized"}]))
            .await?;
        let hash = value
            .pointer("/value/blockhash")
            .and_then(Value::as_str)
            .ok_or_else(|| error::rpc_error(format!("unexpected blockhash reply: {value}")))?;
        address_to_bytes(hash)
    }

    /// The fee for a message, in lamports.
    async fn fee_for_message(&self, message: &Message) -> Result<Option<u64>> {
        let encoded = base64::engine::general_purpose::STANDARD.encode(message.serialize());
        let value = self
            .call(
                "getFeeForMessage",
                json!([encoded, {"commitment": "processed"}]),
            )
            .await?;
        Ok(value.get("value").and_then(Value::as_u64))
    }

    /// The balance of one SPL token account, in the token's base units, or
    /// `None` if the account does not exist yet.
    ///
    /// A missing account is not an error and not a zero the caller has to
    /// guess at: it is the ordinary state of an address that has never held
    /// this token, and the difference matters when deciding whether a
    /// transfer has to create one.
    async fn token_account_amount(&self, account: &str) -> Result<Option<u128>> {
        let value = self
            .call(
                "getTokenAccountBalance",
                json!([account, {"commitment": "confirmed"}]),
            )
            .await;
        let value = match value {
            Ok(value) => value,
            // The node's way of saying the account is not there. Anything else
            // is a real failure and is propagated.
            Err(e) if e.message.contains("could not find account") => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(amount) = value.pointer("/value/amount").and_then(Value::as_str) else {
            return Ok(None);
        };
        amount
            .parse::<u128>()
            .map(Some)
            .map_err(|e| error::rpc_error(format!("unparsable token amount `{amount}`: {e}")))
    }

    /// The lamports an account of `bytes` needs to be rent-exempt.
    ///
    /// Since Solana 1.9 an account below this is rejected outright, so a
    /// transfer that would leave either side under it is refused here rather
    /// than by the cluster after signing.
    async fn rent_exempt_minimum(&self, bytes: u64) -> Result<u64> {
        let value = self
            .call("getMinimumBalanceForRentExemption", json!([bytes]))
            .await?;
        value
            .as_u64()
            .ok_or_else(|| error::rpc_error(format!("unexpected rent-exemption reply: {value}")))
    }
}

#[async_trait]
impl ChainClient for SolanaClient {
    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    async fn balance(&self, address: &str) -> Result<Balance> {
        Ok(Balance::native(self.lamports(address).await? as u128))
    }

    async fn prepare_transfer(
        &self,
        signer_secret: &str,
        request: &TransferRequest,
    ) -> Result<PreparedTransfer> {
        let from = SolanaAccount::from_secret(signer_secret)?;
        let sender = from.address();
        let to = address_to_bytes(&request.to)?;
        if to == from.public_key_bytes() {
            return Err(error::usage(format!(
                "the recipient is the sending account ({sender}); a transfer to \
                 itself moves nothing and still pays the fee"
            )));
        }
        let lamports = u64::try_from(request.amount)
            .map_err(|_| error::invalid_amount("that amount does not fit in a u64 of lamports"))?;

        let blockhash = self.latest_blockhash().await?;
        let instruction = Instruction::transfer(from.public_key_bytes(), to, lamports);
        let message = Message::compile(from.public_key_bytes(), &[instruction], blockhash)?;

        // `None` means the node does not recognise the blockhash it just gave
        // us. Broadcasting on a guessed fee would sign something the cluster
        // has effectively pre-rejected, so fail loudly instead.
        let fee = self.fee_for_message(&message).await?.ok_or_else(|| {
            error::rpc_error("the node does not recognise the blockhash it just issued; try again")
        })?;

        // The fee is the cluster's number; question it before asking whether
        // the account happens to be rich enough to pay it.
        let units = self.network.units();
        chain::check_fee(
            &self.network,
            request.fee_ceiling(&self.network),
            fee as u128,
            units,
        )?;

        // Every reason this transfer cannot work, checked before signing.
        let balance = self.lamports(&sender).await?;
        let needed = lamports
            .checked_add(fee)
            .ok_or_else(|| error::invalid_amount("the amount plus its fee overflows"))?;
        if balance < needed {
            return Err(error::insufficient_funds(format!(
                "balance {} cannot cover {} plus {} of fee",
                units.format_with_symbol(balance as u128),
                units.format_with_symbol(lamports as u128),
                units.format_with_symbol(fee as u128),
            )));
        }

        let rent_minimum = self.rent_exempt_minimum(0).await?;
        let destination_after = self
            .lamports(&request.to)
            .await?
            .checked_add(lamports)
            .ok_or_else(|| error::invalid_amount("the destination balance would overflow"))?;
        if destination_after < rent_minimum {
            return Err(error::invalid_amount(format!(
                "the destination would hold {}, below the rent-exempt minimum of \
                 {} — Solana would reject the account; send at least that much",
                units.format_with_symbol(destination_after as u128),
                units.format_with_symbol(rent_minimum as u128),
            )));
        }
        let remainder = balance - needed;
        if remainder > 0 && remainder < rent_minimum {
            return Err(error::invalid_amount(format!(
                "this would leave {} behind, below the rent-exempt minimum of {} \
                 — send everything, or leave at least that much",
                units.format_with_symbol(remainder as u128),
                units.format_with_symbol(rent_minimum as u128),
            )));
        }

        let mut transaction = Transaction::new_unsigned(message);
        transaction.sign(&from)?;

        Ok(PreparedTransfer {
            signed: transaction.serialize(),
            id: transaction.signature_base58(),
            from: sender.clone(),
            to: request.to.clone(),
            amount: request.amount,
            fee: fee as u128,
            fee_unit: None,
            amount_unit: None,
            token: None,
            fee_rate: None,
            nonce: None,
            gas_limit: None,
            network: self.network,
            note: None,
            detail: json!({
                "fee_lamports": fee,
                "rent_exempt_minimum": rent_minimum,
                "blockhash": bs58::encode(blockhash).into_string(),
            }),
        })
    }

    /// What an address holds of one SPL token.
    ///
    /// The address given is the *wallet's*, not a token account's: the token
    /// account is derived here, because knowing that USDC lives at a third
    /// address neither the user nor the sender ever sees is precisely the part
    /// of Solana a wallet exists to hide.
    async fn token_balance(&self, token: &crate::token::Token, address: &str) -> Result<u128> {
        let account = spl::associated_token_address_str(address, token.id)?;
        // An address that has never held this token holds none of it.
        Ok(self.token_account_amount(&account).await?.unwrap_or(0))
    }

    /// An SPL transfer, as one or two instructions in a single transaction.
    ///
    /// The second instruction — creating the recipient's token account — is
    /// added only when the account is missing, and it is not free: it costs
    /// the sender the account's rent, about 0.002 SOL, on top of the fee. That
    /// cost is folded into the prepared transfer's `fee` rather than hidden in
    /// `detail`, because it is money leaving the sender's account and the
    /// confirmation must name every such number. Both instructions ride one
    /// transaction so the pair is atomic: there is no state where the account
    /// was created and the tokens did not arrive.
    async fn prepare_token_transfer(
        &self,
        token: &crate::token::Token,
        signer_secret: &str,
        request: &TransferRequest,
    ) -> Result<PreparedTransfer> {
        let from = SolanaAccount::from_secret(signer_secret)?;
        let sender = from.address();
        let recipient = address_to_bytes(&request.to)?;
        if recipient == from.public_key_bytes() {
            return Err(error::usage(format!(
                "the recipient is the sending account ({sender}); a transfer to \
                 itself moves nothing and still pays the fee"
            )));
        }
        let mint = address_to_bytes(token.id)?;
        let amount = u64::try_from(request.amount).map_err(|_| {
            error::invalid_amount("that amount does not fit in a u64 of base units")
        })?;
        let units = token.units();

        // The mint's own decimals, not the table's. `TransferChecked` signs
        // them in and the cluster rejects a mismatch, so a wrong number here
        // is a refusal — but it is better to refuse with a sentence that says
        // which two numbers disagreed than to let the cluster say nothing.
        let mint_info = self
            .call(
                "getAccountInfo",
                json!([token.id, {"encoding": "jsonParsed"}]),
            )
            .await?;
        let on_chain = mint_info
            .pointer("/value/data/parsed/info/decimals")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                error::rpc_error(format!(
                    "{} is not an SPL mint on {}",
                    token.id, self.network.name
                ))
            })?;
        if on_chain != token.decimals as u64 {
            return Err(error::internal(format!(
                "the mint {} reports {on_chain} decimals but this wallet's table \
                 says {}; refusing to scale an amount by a number the mint denies",
                token.id, token.decimals
            )));
        }

        let source = spl::associated_token_address(&from.public_key_bytes(), &mint)?;
        let destination = spl::associated_token_address(&recipient, &mint)?;
        let source_str = bs58::encode(source).into_string();
        let destination_str = bs58::encode(destination).into_string();

        let held = self.token_account_amount(&source_str).await?.unwrap_or(0);
        if held < request.amount {
            return Err(error::insufficient_funds(format!(
                "token balance {} is less than {}",
                units.format_with_symbol(held),
                units.format_with_symbol(request.amount),
            )));
        }

        let destination_exists = self.token_account_amount(&destination_str).await?.is_some();
        let rent = if destination_exists {
            0
        } else {
            self.rent_exempt_minimum(spl::TOKEN_ACCOUNT_LEN).await?
        };

        let mut instructions = Vec::with_capacity(2);
        if !destination_exists {
            instructions.push(spl::create_associated_token_account(
                from.public_key_bytes(),
                recipient,
                mint,
                destination,
            ));
        }
        instructions.push(spl::transfer_checked(
            source,
            mint,
            destination,
            from.public_key_bytes(),
            amount,
            token.decimals,
        ));

        let blockhash = self.latest_blockhash().await?;
        let message = Message::compile(from.public_key_bytes(), &instructions, blockhash)?;
        let fee = self.fee_for_message(&message).await?.ok_or_else(|| {
            error::rpc_error("the node does not recognise the blockhash it just issued; try again")
        })?;

        // Everything the sender pays in SOL, checked against the ceiling and
        // then against the balance — the rent included, because it leaves the
        // account exactly as surely as the fee does.
        let sol = self.network.units();
        let cost = (fee as u128)
            .checked_add(rent as u128)
            .ok_or_else(|| error::invalid_amount("the fee plus the rent overflows"))?;
        chain::check_fee(&self.network, request.fee_ceiling(&self.network), cost, sol)?;
        let lamports = self.lamports(&sender).await? as u128;
        if lamports < cost {
            return Err(error::insufficient_funds(format!(
                "balance {} cannot cover {} of fee{}",
                sol.format_with_symbol(lamports),
                sol.format_with_symbol(cost),
                if destination_exists {
                    String::new()
                } else {
                    format!(
                        ", which includes {} of rent for {}'s new {} account",
                        sol.format_with_symbol(rent as u128),
                        request.to,
                        token.symbol
                    )
                },
            )));
        }

        let mut transaction = Transaction::new_unsigned(message);
        transaction.sign(&from)?;

        Ok(PreparedTransfer {
            signed: transaction.serialize(),
            id: transaction.signature_base58(),
            from: sender.clone(),
            to: request.to.clone(),
            amount: request.amount,
            fee: cost,
            fee_unit: Some(sol),
            amount_unit: Some(units),
            token: Some(*token),
            fee_rate: None,
            nonce: None,
            gas_limit: None,
            network: self.network,
            // The one clause this transfer has to add: the user is about to
            // pay for an account belonging to someone else.
            note: (!destination_exists).then(|| {
                format!(
                    ", of which {} creates the recipient's {} account",
                    sol.format_with_symbol(rent as u128),
                    token.symbol
                )
            }),
            detail: json!({
                "token": token.key,
                "mint": token.id,
                "fee_lamports": fee,
                "rent_lamports": rent,
                "source_token_account": source_str,
                "destination_token_account": destination_str,
                "created_destination": !destination_exists,
                "blockhash": bs58::encode(blockhash).into_string(),
            }),
        })
    }

    async fn submit(&self, prepared: &PreparedTransfer) -> Result<TransferReceipt> {
        let encoded = base64::engine::general_purpose::STANDARD.encode(&prepared.signed);
        let value = self
            .call(
                "sendTransaction",
                json!([encoded, {"encoding": "base64", "preflightCommitment": "confirmed"}]),
            )
            .await?;
        let id = value.as_str().ok_or_else(|| {
            error::rpc_error(format!("unexpected sendTransaction reply: {value}"))
        })?;
        Ok(TransferReceipt {
            id: id.to_string(),
            secondary_id: None,
        })
    }

    async fn transaction(&self, id: &str) -> Result<Option<TransactionStatus>> {
        let value = self
            .call(
                "getTransaction",
                json!([id, {"encoding": "json", "maxSupportedTransactionVersion": 0}]),
            )
            .await?;
        if value.is_null() {
            return Ok(None);
        }
        // `meta.err` is null on success and an object describing the failure
        // otherwise; a transaction that landed and failed still cost its fee.
        let failed = value
            .pointer("/meta/err")
            .map(|e| !e.is_null())
            .unwrap_or(false);
        Ok(Some(TransactionStatus {
            id: id.to_string(),
            status: if failed { "failed" } else { "confirmed" }.into(),
            block: value.get("slot").and_then(Value::as_u64),
            fee: value
                .pointer("/meta/fee")
                .and_then(Value::as_u64)
                .map(u128::from),
            gas_used: None,
            raw: value,
        }))
    }

    async fn fee_quote(&self) -> Result<Option<u128>> {
        // Solana quotes per signature rather than per unit of work, and a
        // simple transfer carries exactly one.
        Ok(Some(5_000))
    }

    async fn chain_info(&self) -> Result<Value> {
        let version = self.call("getVersion", json!([])).await?;
        let slot = self.call("getSlot", json!([])).await.ok();
        let height = self.call("getBlockHeight", json!([])).await.ok();
        Ok(json!({
            "cluster": self.network.key,
            "solana_core": version.get("solana-core").and_then(Value::as_str),
            "slot": slot.as_ref().and_then(Value::as_u64),
            "block_height": height.as_ref().and_then(Value::as_u64),
        }))
    }

    async fn faucet(&self, address: &str, amount: u128) -> Result<String> {
        if !self.network.testnet {
            return Err(error::usage(format!(
                "{} has no faucet; airdrops exist on devnet and testnet only",
                self.network.name
            )));
        }
        let lamports = u64::try_from(amount)
            .map_err(|_| error::invalid_amount("that airdrop does not fit in a u64"))?;
        let value = self
            .call("requestAirdrop", json!([address, lamports]))
            .await?;
        value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| error::rpc_error(format!("unexpected airdrop reply: {value}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{SOLANA_DEVNET, SOLANA_MAINNET};

    #[test]
    fn the_endpoint_is_kept_without_a_trailing_slash() {
        let mut config = ClientConfig::bare(SOLANA_DEVNET);
        config.endpoint = "https://api.devnet.solana.com/".into();
        let client = SolanaClient::new(&config);
        assert_eq!(client.endpoint(), "https://api.devnet.solana.com");
    }

    #[test]
    fn mainnet_refuses_a_faucet_before_asking_the_node() {
        let client = SolanaClient::new(&ClientConfig::bare(SOLANA_MAINNET));
        let err = crate::runtime::block_on(client.faucet("anyone", 1))
            .unwrap()
            .unwrap_err();
        assert_eq!(err.code, error::Code::Usage);
        assert!(err.message.contains("devnet"), "{}", err.message);
    }

    #[test]
    fn sending_to_yourself_is_refused_before_any_node_is_asked() {
        // No network access happens here: the check comes first on purpose,
        // because the sender's own address is the one most likely to be on the
        // clipboard by mistake.
        let account = SolanaAccount::from_seed(
            &crate::chain::Seed::new(
                "abandon abandon abandon abandon abandon abandon abandon abandon \
                 abandon abandon abandon about",
                "",
            )
            .unwrap()
            .bip39_seed()[..],
            0,
        )
        .unwrap();
        let client = SolanaClient::new(&ClientConfig::bare(SOLANA_DEVNET));
        let request = TransferRequest::new(account.address(), 1);
        let err =
            crate::runtime::block_on(client.prepare_transfer(&account.secret_base58(), &request))
                .unwrap()
                .unwrap_err();
        assert_eq!(err.code, error::Code::Usage);
        assert!(err.message.contains("itself"), "{}", err.message);
    }
}
