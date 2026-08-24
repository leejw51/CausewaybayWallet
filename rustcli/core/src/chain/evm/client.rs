//! The EVM client: the existing JSON-RPC code behind the chain trait.

use alloy_primitives::U256;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::chain::{
    Balance, ChainClient, ClientConfig, PreparedTransfer, TransactionStatus, TransferReceipt,
    TransferRequest,
};
use crate::error::{self, Result};
use crate::network::Network;
use crate::rpc::RpcClient;
use crate::tx::LegacyTransaction;
use crate::wallet::{self, Keypair};

use super::to_u128;

/// A plain transfer costs exactly this much gas; anything with call data has
/// to be estimated.
const PLAIN_TRANSFER_GAS: u64 = 21_000;

pub struct EvmClient {
    network: Network,
    rpc: RpcClient,
}

impl EvmClient {
    pub fn new(config: &ClientConfig) -> Result<Self> {
        Ok(EvmClient {
            network: config.network,
            rpc: RpcClient::new(config.endpoint.clone())?,
        })
    }

    pub fn rpc(&self) -> &RpcClient {
        &self.rpc
    }
}

/// Add 25 % headroom to an estimate so a slightly heavier execution still fits.
pub fn with_headroom(estimate: u64) -> u64 {
    estimate.saturating_mul(125) / 100
}

#[async_trait]
impl ChainClient for EvmClient {
    fn endpoint(&self) -> &str {
        self.rpc.url()
    }

    async fn balance(&self, address: &str) -> Result<Balance> {
        let address = wallet::parse_address(address)?;
        let wei = self.rpc.get_balance(address).await?;
        Ok(Balance::native(to_u128(wei, "the balance")?))
    }

    async fn prepare_transfer(
        &self,
        signer_secret: &str,
        request: &TransferRequest,
    ) -> Result<PreparedTransfer> {
        let keypair = Keypair::from_hex(signer_secret)?;
        let from = keypair.address();
        let to = wallet::parse_address(&request.to)?;

        // A transfer to the account it leaves from moves nothing and costs the
        // gas anyway. It is almost always a paste into the wrong field — the
        // sender's own address is the one most likely to be on the clipboard —
        // so it is refused here, before a node is asked anything.
        if to == from {
            return Err(error::usage(format!(
                "the recipient is the sending account ({}); a transfer to itself \
                 moves nothing and still pays the gas",
                to.to_checksum(None)
            )));
        }

        let value = U256::from(request.amount);
        let data = request.data.clone();

        // The nonce and the gas price are independent lookups, so they go out
        // together rather than one after the other.
        let (nonce, gas_price) = match (request.nonce_override, request.fee_override) {
            (Some(n), Some(p)) => (n, U256::from(p)),
            (Some(n), None) => (n, self.rpc.gas_price().await?),
            (None, Some(p)) => (self.rpc.get_transaction_count(from).await?, U256::from(p)),
            (None, None) => {
                let (nonce, price) =
                    tokio::try_join!(self.rpc.get_transaction_count(from), self.rpc.gas_price())?;
                (nonce, price)
            }
        };

        let gas_limit = match request.gas_limit {
            Some(limit) => limit,
            None if data.is_empty() => PLAIN_TRANSFER_GAS,
            None => with_headroom(self.rpc.estimate_gas(from, to, value, &data).await?),
        };

        // Fail before signing when the balance obviously cannot cover this.
        let balance = self.rpc.get_balance(from).await?;
        let gas_cost = gas_price * U256::from(gas_limit);
        let max_cost = value + gas_cost;
        let units = self.network.units();
        if balance < max_cost {
            return Err(error::insufficient_funds(format!(
                "balance {} cannot cover {} plus up to {} of gas",
                units.format_with_symbol(to_u128(balance, "the balance")?),
                units.format_with_symbol(request.amount),
                units.format_with_symbol(to_u128(gas_cost, "the gas cost")?),
            )));
        }

        let transaction = LegacyTransaction {
            nonce,
            gas_price,
            gas_limit,
            to: Some(to),
            value,
            data,
            chain_id: self
                .network
                .chain_id
                .ok_or_else(|| error::internal("an EVM network with no chain id"))?,
        };
        let signed = transaction.sign(&keypair)?;

        Ok(PreparedTransfer {
            id: signed.hash_hex(),
            signed: signed.raw,
            from: from.to_checksum(None),
            to: to.to_checksum(None),
            amount: request.amount,
            fee: to_u128(gas_cost, "the gas cost")?,
            fee_unit: None,
            fee_rate: Some(to_u128(gas_price, "the gas price")?),
            nonce: Some(nonce),
            gas_limit: Some(gas_limit),
            prompt: format!(
                "Send {} from {} to {} on {}",
                units.format_with_symbol(request.amount),
                from.to_checksum(None),
                to.to_checksum(None),
                self.network.name
            ),
            detail: json!({
                "gas_limit": gas_limit,
                "gas_price_wei": gas_price.to_string(),
                "gas_price_gwei": crate::units::format_gwei(gas_price),
                "max_cost_wei": max_cost.to_string(),
            }),
        })
    }

    async fn submit(&self, prepared: &PreparedTransfer) -> Result<TransferReceipt> {
        // A node echoes the hash back, but the locally computed one is
        // authoritative: it is what was signed. A disagreement is the node's
        // problem, not something to propagate.
        let returned = self.rpc.send_raw_transaction(&prepared.signed).await?;
        Ok(TransferReceipt {
            id: prepared.id.clone(),
            secondary_id: (returned != prepared.id).then_some(returned),
        })
    }

    async fn transaction(&self, id: &str) -> Result<Option<TransactionStatus>> {
        let (transaction, receipt) = tokio::try_join!(
            self.rpc.get_transaction_by_hash(id),
            self.rpc.get_transaction_receipt(id),
        )?;
        if transaction.is_none() && receipt.is_none() {
            return Ok(None);
        }
        let status = match &receipt {
            Some(r) => match crate::rpc::field_u64(r, "status") {
                Some(1) => "confirmed",
                Some(_) => "failed",
                None => "pending",
            },
            None => "pending",
        };
        let gas_used = receipt
            .as_ref()
            .and_then(|r| crate::rpc::field_u64(r, "gasUsed"));
        let gas_price = transaction
            .as_ref()
            .and_then(|t| crate::rpc::field_u64(t, "gasPrice"));
        Ok(Some(TransactionStatus {
            id: id.to_string(),
            status: status.to_string(),
            block: receipt
                .as_ref()
                .and_then(|r| crate::rpc::field_u64(r, "blockNumber")),
            fee: match (gas_used, gas_price) {
                (Some(used), Some(price)) => Some(used as u128 * price as u128),
                _ => None,
            },
            gas_used,
            raw: json!({"transaction": transaction, "receipt": receipt}),
        }))
    }

    async fn nonce(&self, address: &str) -> Result<Option<u64>> {
        let address = wallet::parse_address(address)?;
        Ok(Some(self.rpc.get_transaction_count(address).await?))
    }

    async fn fee_quote(&self) -> Result<Option<u128>> {
        Ok(Some(to_u128(self.rpc.gas_price().await?, "the gas price")?))
    }

    async fn chain_info(&self) -> Result<Value> {
        let (reported, block, gas) = tokio::try_join!(
            self.rpc.chain_id(),
            self.rpc.block_number(),
            self.rpc.gas_price(),
        )?;
        let expected = self.network.chain_id;
        Ok(json!({
            "network": self.network.key,
            "name": self.network.name,
            "expected_chain_id": expected,
            "reported_chain_id": reported,
            // A node answering for a different chain than the one whose
            // replay protection we are signing with is worth shouting about.
            "chain_id_matches": expected == Some(reported),
            "block_number": block,
            "gas_price_wei": gas.to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::CRONOS_TESTNET;

    #[test]
    fn gas_headroom_is_twenty_five_percent() {
        assert_eq!(with_headroom(100_000), 125_000);
        assert_eq!(with_headroom(21_000), 26_250);
        assert_eq!(with_headroom(0), 0);
        // Must not overflow on an absurd estimate.
        assert!(with_headroom(u64::MAX) > 0);
    }

    #[test]
    fn the_client_reports_the_endpoint_it_was_given() {
        let mut config = ClientConfig::bare(CRONOS_TESTNET);
        config.endpoint = "http://localhost:8545".into();
        let client = EvmClient::new(&config).unwrap();
        assert_eq!(client.endpoint(), "http://localhost:8545");
    }

    #[test]
    fn sending_to_yourself_is_refused_before_any_node_is_asked() {
        // No network access: the check comes first on purpose.
        let secret = "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727";
        let own = Keypair::from_hex(secret)
            .unwrap()
            .address()
            .to_checksum(None);
        let client = EvmClient::new(&ClientConfig::bare(CRONOS_TESTNET)).unwrap();
        let request = TransferRequest::new(own, 1);

        let err = crate::runtime::block_on(client.prepare_transfer(secret, &request))
            .unwrap()
            .unwrap_err();
        assert_eq!(err.code, error::Code::Usage);
        assert!(err.message.contains("itself"), "{}", err.message);
    }
}
