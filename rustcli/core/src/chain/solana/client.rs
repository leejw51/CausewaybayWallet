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
