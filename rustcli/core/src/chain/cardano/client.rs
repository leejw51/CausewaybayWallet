//! Chain access through Koios, a free key-less Cardano API.
//!
//! Blockfrost is the more common choice but needs an API key; Koios lets this
//! wallet work on preprod out of the box.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::chain::http;
use crate::chain::{
    Balance, ChainClient, ClientConfig, PreparedTransfer, TransactionStatus, TransferReceipt,
    TransferRequest,
};
use crate::error::{self, Result};
use crate::network::Network;

use super::address::{Address, CardanoNetwork};
use super::keys::CardanoAccount;
use super::tx::{ProtocolParams, TxBuilder, TxInput};

/// How far ahead of the tip a transaction stays valid: roughly two hours.
/// A TTL in the past is rejected outright, so the slack is deliberate.
const TTL_SLACK_SLOTS: u64 = 7_200;

pub struct CardanoClient {
    network: Network,
    endpoint: String,
}

impl CardanoClient {
    pub fn new(config: &ClientConfig) -> Self {
        CardanoClient {
            network: config.network,
            endpoint: config.endpoint.trim_end_matches('/').to_string(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.endpoint)
    }

    /// Every unspent output at an address, as spendable inputs.
    ///
    /// Outputs carrying native tokens, datums or reference scripts are skipped:
    /// spending one means preserving the extra value it holds, which this
    /// wallet does not model, and silently dropping a token is not an option.
    async fn utxos(&self, address: &str) -> Result<Vec<TxInput>> {
        // `_extended` must be true or Koios omits the asset and datum fields
        // entirely, and the filtering below would pass token UTxOs through.
        let value = http::post_json(
            &self.url("address_utxos"),
            &json!({"_addresses": [address], "_extended": true}),
        )
        .await?;
        let rows = value
            .as_array()
            .ok_or_else(|| error::rpc_error("expected an array of unspent outputs"))?;

        let mut out = Vec::new();
        for row in rows {
            let has_assets = row
                .get("asset_list")
                .and_then(Value::as_array)
                .map(|a| !a.is_empty())
                .unwrap_or(false);
            let has_attachments = ["datum_hash", "inline_datum", "reference_script"]
                .iter()
                .any(|field| row.get(*field).map(|v| !v.is_null()).unwrap_or(false));
            if has_assets || has_attachments {
                continue;
            }
            let hash = row
                .get("tx_hash")
                .and_then(Value::as_str)
                .ok_or_else(|| error::rpc_error("an unspent output has no tx_hash"))?;
            let mut tx_hash = [0u8; 32];
            hex::decode_to_slice(hash, &mut tx_hash)
                .map_err(|e| error::rpc_error(format!("malformed tx_hash `{hash}`: {e}")))?;
            let lovelace = row
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| error::rpc_error("an unspent output has no value"))?;
            out.push(TxInput {
                tx_hash,
                index: row
                    .get("tx_index")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| error::rpc_error("an unspent output has no tx_index"))?,
                lovelace: lovelace.parse().map_err(|e| {
                    error::rpc_error(format!("unparsable output value `{lovelace}`: {e}"))
                })?,
            });
        }
        Ok(out)
    }

    /// The current slot, used to set a transaction's TTL.
    async fn tip_slot(&self) -> Result<u64> {
        let value = http::get_json(&self.url("tip")).await?;
        value
            .as_array()
            .and_then(|a| a.first())
            .and_then(|t| t.get("abs_slot"))
            .and_then(Value::as_u64)
            .ok_or_else(|| error::rpc_error(format!("unexpected tip reply: {value}")))
    }

    /// Live protocol parameters.
    ///
    /// The fee parameters are load-bearing — an under-fee'd transaction is
    /// rejected after signing — so a missing field is an error rather than a
    /// silent fall back to compile-time defaults that may have since moved.
    async fn protocol_params(&self) -> Result<ProtocolParams> {
        let value = http::get_json(&self.url("epoch_params")).await?;
        let row = value
            .as_array()
            .and_then(|a| a.first())
            .ok_or_else(|| error::rpc_error(format!("unexpected epoch_params reply: {value}")))?;
        let field = |name: &str| -> Result<u64> {
            row.get(name)
                .and_then(|x| {
                    x.as_u64()
                        .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
                })
                .ok_or_else(|| error::rpc_error(format!("epoch_params has no `{name}`")))
        };
        Ok(ProtocolParams {
            min_fee_a: field("min_fee_a")?,
            min_fee_b: field("min_fee_b")?,
            coins_per_utxo_byte: field("coins_per_utxo_size")?,
        })
    }
}

#[async_trait]
impl ChainClient for CardanoClient {
    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    async fn balance(&self, address: &str) -> Result<Balance> {
        let value =
            http::post_json(&self.url("address_info"), &json!({"_addresses": [address]})).await?;
        // An address the chain has never seen comes back as an empty array,
        // which means zero rather than an error.
        let Some(first) = value.as_array().and_then(|a| a.first()) else {
            return Ok(Balance::native(0));
        };
        let raw = first
            .get("balance")
            .and_then(Value::as_str)
            .ok_or_else(|| error::rpc_error(format!("unexpected address_info reply: {first}")))?;
        let lovelace: u128 = raw
            .parse()
            .map_err(|e| error::rpc_error(format!("unparsable balance `{raw}`: {e}")))?;
        Ok(Balance::native(lovelace))
    }

    async fn prepare_transfer(
        &self,
        signer_secret: &str,
        request: &TransferRequest,
    ) -> Result<PreparedTransfer> {
        let account = CardanoAccount::from_secret(signer_secret)?;
        let cardano_network = CardanoNetwork::of(&self.network);
        let source = account.base_address(cardano_network);
        let destination = Address::parse(&request.to)?;

        if destination.network != cardano_network {
            return Err(error::invalid_address(format!(
                "that address is a {:?} address but this wallet is on {}; the \
                 funds would go to a chain nobody is watching",
                destination.network, self.network.name
            )));
        }
        if destination == source {
            return Err(error::usage(format!(
                "the recipient is the sending account ({source}); a transfer to \
                 itself moves nothing and still pays the fee"
            )));
        }
        let lovelace = u64::try_from(request.amount)
            .map_err(|_| error::invalid_amount("that amount does not fit in a u64 of lovelace"))?;

        // Three independent reads, and none depends on the others — so they
        // go out together rather than one after another.
        let source_bech32 = source.to_bech32();
        let (utxos, params, tip) = tokio::try_join!(
            self.utxos(&source_bech32),
            self.protocol_params(),
            self.tip_slot(),
        )?;

        let signed = TxBuilder::new(params, tip + TTL_SLACK_SLOTS)
            .limit_fee(self.network, request.fee_ceiling(&self.network))
            .build_transfer(&utxos, &destination, lovelace, &source, &account)?;

        Ok(PreparedTransfer {
            id: hex::encode(signed.tx_id()),
            signed: signed.to_cbor(),
            from: source_bech32.clone(),
            to: request.to.clone(),
            amount: request.amount,
            fee: signed.body.fee as u128,
            fee_unit: None,
            amount_unit: None,
            token: None,
            fee_rate: None,
            nonce: None,
            gas_limit: None,
            network: self.network,
            note: None,
            detail: json!({
                "fee_lovelace": signed.body.fee,
                "inputs": signed.body.inputs.len(),
                "outputs": signed.body.outputs.len(),
                "ttl_slot": signed.body.ttl,
                "change_lovelace": signed.body.outputs.get(1).map(|o| o.lovelace),
            }),
        })
    }

    /// What an address holds of one native asset.
    ///
    /// Read from Koios `address_assets`, which reports every asset at an
    /// address as a policy id, an asset name and a quantity — the same pair
    /// the registry's `id` concatenates. The quantity is already in the
    /// asset's base units; the decimals only decide how it is printed.
    ///
    /// Reading is all this wallet does with native assets. Moving one is
    /// refused before it gets this far — see [`crate::token::Standard`].
    async fn token_balance(&self, token: &crate::token::Token, address: &str) -> Result<u128> {
        // 28 bytes of policy id, hex-encoded, then the asset name.
        let (policy, name) = token.id.split_at(56);
        let value = http::post_json(
            &self.url("address_assets"),
            &json!({"_addresses": [address]}),
        )
        .await?;
        let rows = value
            .as_array()
            .ok_or_else(|| error::rpc_error("expected an array of assets"))?;

        // Summed rather than taken from the first match: Koios reports per
        // address and an address can appear once per asset, but a total that
        // silently used one row of several would understate a balance.
        let mut total: u128 = 0;
        for row in rows {
            let matches = row.get("policy_id").and_then(Value::as_str) == Some(policy)
                && row.get("asset_name").and_then(Value::as_str) == Some(name);
            if !matches {
                continue;
            }
            let quantity = row
                .get("quantity")
                .and_then(Value::as_str)
                .ok_or_else(|| error::rpc_error("an asset row has no quantity"))?;
            let held: u128 = quantity.parse().map_err(|e| {
                error::rpc_error(format!("unparsable asset quantity `{quantity}`: {e}"))
            })?;
            total = total
                .checked_add(held)
                .ok_or_else(|| error::rpc_error("an asset balance overflows"))?;
        }
        Ok(total)
    }

    async fn submit(&self, prepared: &PreparedTransfer) -> Result<TransferReceipt> {
        let reply = http::post_bytes(
            &self.url("submittx"),
            "application/cbor",
            prepared.signed.clone(),
        )
        .await?;
        // The locally computed id is authoritative, as it is on EVM: it is the
        // blake2b-256 of the body that was signed, and Koios cannot change
        // what that hashes to. Returning its answer instead meant a
        // misbehaving endpoint could send `--wait` to follow a transaction
        // that does not exist while the real one confirmed unwatched.
        let returned = reply.trim().trim_matches('"').to_string();
        Ok(TransferReceipt {
            id: prepared.id.clone(),
            secondary_id: (returned != prepared.id && !returned.is_empty()).then_some(returned),
        })
    }

    async fn transaction(&self, id: &str) -> Result<Option<TransactionStatus>> {
        let value = http::post_json(&self.url("tx_info"), &json!({"_tx_hashes": [id]})).await?;
        let Some(row) = value.as_array().and_then(|a| a.first()) else {
            return Ok(None);
        };
        let block = row.get("block_height").and_then(Value::as_u64);
        Ok(Some(TransactionStatus {
            id: id.to_string(),
            // Koios only returns transactions that are already on chain, so
            // anything it knows about has confirmed.
            status: if block.is_some() {
                "confirmed"
            } else {
                "pending"
            }
            .into(),
            block,
            fee: row.get("fee").and_then(|f| {
                f.as_str()
                    .and_then(|s| s.parse().ok())
                    .or_else(|| f.as_u64().map(u128::from))
            }),
            gas_used: None,
            raw: row.clone(),
        }))
    }

    async fn fee_quote(&self) -> Result<Option<u128>> {
        // Cardano quotes a linear fee rather than a unit price, so the useful
        // number is the flat constant every transaction pays at minimum.
        Ok(Some(self.protocol_params().await?.min_fee_b as u128))
    }

    async fn chain_info(&self) -> Result<Value> {
        let value = http::get_json(&self.url("tip")).await?;
        let tip = value
            .as_array()
            .and_then(|a| a.first())
            .cloned()
            .unwrap_or(Value::Null);
        Ok(json!({
            "network": self.network.key,
            "epoch": tip.get("epoch_no").and_then(Value::as_u64),
            "block_height": tip.get("block_no").and_then(Value::as_u64),
            "abs_slot": tip.get("abs_slot").and_then(Value::as_u64),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::Seed;
    use crate::network::{CARDANO_MAINNET, CARDANO_PREPROD};

    const PHRASE: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn account() -> CardanoAccount {
        let entropy = Seed::new(PHRASE, "").unwrap().entropy().unwrap();
        CardanoAccount::from_entropy(&entropy, "", 0).unwrap()
    }

    #[test]
    fn the_endpoint_is_kept_without_a_trailing_slash() {
        let mut config = ClientConfig::bare(CARDANO_PREPROD);
        config.endpoint = "https://preprod.koios.rest/api/v1/".into();
        let client = CardanoClient::new(&config);
        assert_eq!(client.endpoint(), "https://preprod.koios.rest/api/v1");
        assert_eq!(
            client.url("address_info"),
            "https://preprod.koios.rest/api/v1/address_info"
        );
    }

    /// The costliest mistake this wallet can make on Cardano, refused offline.
    #[test]
    fn a_mainnet_recipient_on_a_testnet_wallet_is_refused_before_any_node_is_asked() {
        let account = account();
        let mainnet_address = Address::base(CardanoNetwork::Mainnet, [0x33; 28], [0x44; 28]);
        let client = CardanoClient::new(&ClientConfig::bare(CARDANO_PREPROD));
        let request = TransferRequest::new(mainnet_address.to_bech32(), 2_000_000);

        let err =
            crate::runtime::block_on(client.prepare_transfer(&account.secret_hex(), &request))
                .unwrap()
                .unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAddress);
        assert!(
            err.message.contains("nobody is watching"),
            "{}",
            err.message
        );
    }

    #[test]
    fn sending_to_yourself_is_refused_before_any_node_is_asked() {
        let account = account();
        let client = CardanoClient::new(&ClientConfig::bare(CARDANO_PREPROD));
        let own = account.base_address(CardanoNetwork::Testnet).to_bech32();
        let request = TransferRequest::new(own, 2_000_000);

        let err =
            crate::runtime::block_on(client.prepare_transfer(&account.secret_hex(), &request))
                .unwrap()
                .unwrap_err();
        assert_eq!(err.code, error::Code::Usage);
        assert!(err.message.contains("itself"), "{}", err.message);
    }

    #[test]
    fn a_testnet_recipient_on_a_mainnet_wallet_is_refused_too() {
        let account = account();
        let testnet_address = Address::base(CardanoNetwork::Testnet, [0x33; 28], [0x44; 28]);
        let client = CardanoClient::new(&ClientConfig::bare(CARDANO_MAINNET));
        let request = TransferRequest::new(testnet_address.to_bech32(), 2_000_000);

        let err =
            crate::runtime::block_on(client.prepare_transfer(&account.secret_hex(), &request))
                .unwrap()
                .unwrap_err();
        assert_eq!(err.code, error::Code::InvalidAddress);
    }
}
