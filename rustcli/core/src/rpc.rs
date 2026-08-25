//! An async JSON-RPC 2.0 client covering the EVM methods the wallet needs.
//!
//! It shares the one HTTP client every chain uses (see [`crate::chain::http`]),
//! so a four-chain `balance --all` reuses connections instead of standing up a
//! stack per chain.

use std::time::Duration;

use alloy_primitives::{Address, U256};
use serde_json::{json, Value};

use crate::chain::http;
use crate::error::{self, Result};

pub struct RpcClient {
    url: String,
}

impl RpcClient {
    pub fn new(url: impl Into<String>) -> Result<Self> {
        Ok(RpcClient { url: url.into() })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// Issue one JSON-RPC call and unwrap its `result`.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        http::json_rpc(&self.url, method, params).await
    }

    // ------------------------------------------------------------- shortcuts

    pub async fn chain_id(&self) -> Result<u64> {
        let value = self.call("eth_chainId", json!([])).await?;
        parse_quantity_u64(&value, "eth_chainId")
    }

    pub async fn block_number(&self) -> Result<u64> {
        let value = self.call("eth_blockNumber", json!([])).await?;
        parse_quantity_u64(&value, "eth_blockNumber")
    }

    pub async fn get_balance(&self, address: Address) -> Result<U256> {
        let value = self
            .call("eth_getBalance", json!([checksum(address), "latest"]))
            .await?;
        parse_quantity_u256(&value, "eth_getBalance")
    }

    /// Uses the pending block so back-to-back sends do not reuse a nonce.
    pub async fn get_transaction_count(&self, address: Address) -> Result<u64> {
        let value = self
            .call(
                "eth_getTransactionCount",
                json!([checksum(address), "pending"]),
            )
            .await?;
        parse_quantity_u64(&value, "eth_getTransactionCount")
    }

    pub async fn gas_price(&self) -> Result<U256> {
        let value = self.call("eth_gasPrice", json!([])).await?;
        parse_quantity_u256(&value, "eth_gasPrice")
    }

    pub async fn estimate_gas(
        &self,
        from: Address,
        to: Address,
        value: U256,
        data: &[u8],
    ) -> Result<u64> {
        let mut call = json!({
            "from": checksum(from),
            "to": checksum(to),
            "value": to_quantity(value),
        });
        if !data.is_empty() {
            call["data"] = json!(format!("0x{}", hex::encode(data)));
        }
        let result = self
            .call("eth_estimateGas", json!([call, "latest"]))
            .await?;
        parse_quantity_u64(&result, "eth_estimateGas")
    }

    /// Read-only contract call; returns the raw ABI-encoded return data.
    pub async fn eth_call(&self, to: Address, data: &[u8]) -> Result<Vec<u8>> {
        let call = json!({"to": checksum(to), "data": format!("0x{}", hex::encode(data))});
        let result = self.call("eth_call", json!([call, "latest"])).await?;
        let text = result
            .as_str()
            .ok_or_else(|| error::rpc_error("eth_call did not return hex data"))?;
        hex::decode(text.trim_start_matches("0x"))
            .map_err(|e| error::rpc_error(format!("eth_call returned malformed hex: {e}")))
    }

    pub async fn send_raw_transaction(&self, raw: &[u8]) -> Result<String> {
        let payload = format!("0x{}", hex::encode(raw));
        let result = self
            .call("eth_sendRawTransaction", json!([payload]))
            .await?;
        result
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| error::rpc_error("eth_sendRawTransaction did not return a hash"))
    }

    pub async fn get_transaction_by_hash(&self, hash: &str) -> Result<Option<Value>> {
        let result = self.call("eth_getTransactionByHash", json!([hash])).await?;
        Ok(if result.is_null() { None } else { Some(result) })
    }

    pub async fn get_transaction_receipt(&self, hash: &str) -> Result<Option<Value>> {
        let result = self
            .call("eth_getTransactionReceipt", json!([hash]))
            .await?;
        Ok(if result.is_null() { None } else { Some(result) })
    }

    /// Poll for a receipt until it appears or the deadline passes.
    ///
    /// Transient poll failures are retried rather than propagated: by the time
    /// this is called the transaction has already been broadcast, so reporting
    /// a single 502 as failure invites the user to send it a second time. Only
    /// a failure that persists to the deadline is surfaced.
    pub async fn wait_for_receipt(&self, hash: &str, timeout: Duration) -> Result<Option<Value>> {
        let deadline = std::time::Instant::now() + timeout;
        let interval = Duration::from_millis(1500);
        let mut last_error: Option<crate::error::Error>;
        loop {
            match self.get_transaction_receipt(hash).await {
                Ok(Some(receipt)) => return Ok(Some(receipt)),
                Ok(None) => last_error = None,
                Err(e) => last_error = Some(e),
            }
            if std::time::Instant::now() + interval > deadline {
                // Nothing but errors for the whole window: say why, and name the
                // hash so the transaction can still be looked up.
                return match last_error {
                    Some(e) => Err(error::rpc_error(format!(
                        "{} was broadcast, but polling for its receipt kept failing: {}",
                        hash, e.message
                    ))),
                    None => Ok(None),
                };
            }
            tokio::time::sleep(interval).await;
        }
    }
}

fn checksum(address: Address) -> String {
    address.to_checksum(None)
}

/// Encode a U256 as a minimal `0x`-prefixed quantity, per the JSON-RPC spec.
pub fn to_quantity(value: U256) -> String {
    format!("0x{value:x}")
}

pub fn parse_quantity_u256(value: &Value, context: &str) -> Result<U256> {
    let text = value.as_str().ok_or_else(|| {
        error::rpc_error(format!("{context} returned {value} instead of a quantity"))
    })?;
    let body = text.trim_start_matches("0x").trim_start_matches("0X");
    if body.is_empty() {
        return Ok(U256::ZERO);
    }
    U256::from_str_radix(body, 16)
        .map_err(|_| error::rpc_error(format!("{context} returned an unparsable quantity {text}")))
}

pub fn parse_quantity_u64(value: &Value, context: &str) -> Result<u64> {
    let big = parse_quantity_u256(value, context)?;
    u64::try_from(big)
        .map_err(|_| error::rpc_error(format!("{context} returned a value too large for u64")))
}

/// Read a hex quantity out of a JSON object field, tolerating absent fields.
pub fn field_u64(value: &Value, field: &str) -> Option<u64> {
    parse_quantity_u64(value.get(field)?, field).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_quantities_minimally() {
        assert_eq!(to_quantity(U256::ZERO), "0x0");
        assert_eq!(to_quantity(U256::from(1)), "0x1");
        assert_eq!(to_quantity(U256::from(255)), "0xff");
        assert_eq!(to_quantity(U256::from(1024)), "0x400");
    }

    #[test]
    fn parses_quantities() {
        assert_eq!(parse_quantity_u256(&json!("0x0"), "t").unwrap(), U256::ZERO);
        assert_eq!(parse_quantity_u256(&json!("0x"), "t").unwrap(), U256::ZERO);
        assert_eq!(
            parse_quantity_u256(&json!("0x152"), "t").unwrap(),
            U256::from(338)
        );
        assert_eq!(parse_quantity_u64(&json!("0x19"), "t").unwrap(), 25);
        assert_eq!(
            parse_quantity_u256(&json!("0xde0b6b3a7640000"), "t").unwrap(),
            U256::from(1_000_000_000_000_000_000u64)
        );
    }

    #[test]
    fn rejects_junk_quantities() {
        assert!(parse_quantity_u256(&json!(42), "t").is_err());
        assert!(parse_quantity_u256(&json!(null), "t").is_err());
        assert!(parse_quantity_u256(&json!("0xnothex"), "t").is_err());
        // 2^64 does not fit in a u64 even though it is a valid U256.
        assert!(parse_quantity_u64(&json!("0x10000000000000000"), "t").is_err());
    }

    #[test]
    fn reads_optional_object_fields() {
        let receipt = json!({"blockNumber": "0x2a", "status": "0x1"});
        assert_eq!(field_u64(&receipt, "blockNumber"), Some(42));
        assert_eq!(field_u64(&receipt, "status"), Some(1));
        assert_eq!(field_u64(&receipt, "missing"), None);
    }

    #[test]
    fn client_keeps_its_url() {
        let client = RpcClient::new("http://localhost:8545").unwrap();
        assert_eq!(client.url(), "http://localhost:8545");
    }
}
