//! The Midnight indexer client.
//!
//! Unlike Solana and Cardano, the Midnight indexer has **no query that returns
//! an address balance**. Unshielded (Night) history is exposed only as a
//! GraphQL *subscription*, `unshieldedTransactions(address:)`, which replays
//! every UTxO the address created or spent and then emits a progress marker
//! once it has caught up with the chain tip.
//!
//! So computing a balance means opening a `graphql-transport-ws` connection,
//! replaying that history, and summing the outputs still unspent — which is
//! exactly what the official wallet SDK does behind its nicer API.
//!
//! The subscription never completes on its own, so every read here is bounded
//! by a deadline, and a replay that did not reach the marker is reported as a
//! failure rather than summed. A truncated history produces a confidently
//! wrong balance, which is worse than no balance at all.

use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::chain::http;
use crate::error::{self, Result};

/// Midnight's native unshielded token. Its type is 32 zero bytes.
pub const NIGHT_TOKEN_TYPE: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// How long to let a historical replay run before giving up.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(45);

/// One unspent output as the indexer reports it, carrying everything a spend
/// needs: the (intent hash, output index) pair is the UTxO's identity on
/// chain, and the dust fields decide how its fee can be paid.
#[derive(Debug, Clone)]
pub struct UtxoInfo {
    /// Hex-encoded intent hash of the transaction that created this output.
    pub intent_hash: String,
    pub output_index: u32,
    /// Hex-encoded token type; all zeros for Night.
    pub token_type: String,
    pub value: u128,
    /// Creation time in seconds, where the indexer knows it.
    pub ctime: Option<i64>,
    /// Whether this output is already delegated to a DUST address.
    ///
    /// Undelegated outputs accrue an implicit fee allowance that a send can
    /// spend without any proof; delegated ones need a real DUST spend, which
    /// costs a state sync and a proving run. This flag is what decides which.
    pub registered_for_dust: bool,
}

/// A balance broken down by token type, in each token's base units.
#[derive(Debug, Default, Clone)]
pub struct UnshieldedBalance {
    pub by_token: BTreeMap<String, u128>,
}

impl UnshieldedBalance {
    /// The Night balance, in "stars" (10⁻⁶ NIGHT).
    pub fn night(&self) -> u128 {
        self.by_token.get(NIGHT_TOKEN_TYPE).copied().unwrap_or(0)
    }
}

pub struct Indexer {
    url: String,
    pub timeout: Duration,
}

impl Indexer {
    pub fn new(url: &str) -> Self {
        Indexer {
            url: url.trim_end_matches('/').to_string(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// The `wss://…/ws` form of the configured HTTP endpoint.
    fn ws_url(&self) -> String {
        let base = self
            .url
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        if base.ends_with("/ws") {
            base
        } else {
            format!("{}/ws", base.trim_end_matches('/'))
        }
    }

    /// The chain tip's block height — a cheap way to check the indexer is alive.
    pub async fn block_height(&self) -> Result<u64> {
        let data = http::graphql(&self.url, "{ block { height } }", json!({})).await?;
        data.pointer("/block/height")
            .and_then(Value::as_u64)
            .ok_or_else(|| error::rpc_error(format!("unexpected block reply: {data}")))
    }

    /// Replay an address's unshielded history and sum its unspent outputs.
    pub async fn balance(&self, address: &str) -> Result<UnshieldedBalance> {
        let mut balance = UnshieldedBalance::default();
        for utxo in self.utxos(address).await? {
            *balance.by_token.entry(utxo.token_type).or_insert(0) += utxo.value;
        }
        Ok(balance)
    }

    /// Replay an address's history and return the outputs still unspent.
    pub async fn utxos(&self, address: &str) -> Result<Vec<UtxoInfo>> {
        const QUERY: &str = r#"
subscription($addr: UnshieldedAddress!) {
  unshieldedTransactions(address: $addr) {
    __typename
    ... on UnshieldedTransactionsProgress { highestTransactionId }
    ... on UnshieldedTransaction {
      transaction { id }
      createdUtxos {
        owner tokenType value intentHash outputIndex ctime initialNonce
        registeredForDustGeneration spentAtTransaction { id }
      }
      spentUtxos { owner tokenType value outputIndex }
    }
  }
}"#;

        // The indexer merges the historical replay with a progress stream that
        // fires immediately and then periodically, so a progress marker does
        // *not* mean the replay is done. It carries the highest transaction id
        // known for the address; we are caught up once we have replayed to it.
        let mut events: Vec<Value> = Vec::new();
        let mut target_id: Option<u64> = None;
        let mut highest_seen: u64 = 0;

        let caught_up = self
            .subscribe(
                QUERY,
                json!({ "addr": address }),
                "unshieldedTransactions",
                self.timeout,
                |item| {
                    if item.get("__typename").and_then(Value::as_str)
                        == Some("UnshieldedTransactionsProgress")
                    {
                        target_id = item
                            .get("highestTransactionId")
                            .and_then(Value::as_u64)
                            .or(Some(0));
                    } else {
                        if let Some(id) = item.pointer("/transaction/id").and_then(Value::as_u64) {
                            highest_seen = highest_seen.max(id);
                        }
                        events.push(item.clone());
                    }
                    Ok(matches!(target_id, Some(target) if highest_seen >= target))
                },
            )
            .await?;

        if !caught_up {
            return Err(error::rpc_error(format!(
                "the indexer did not finish replaying this address's history \
                 within {:?} — refusing to report a possibly partial balance",
                self.timeout
            )));
        }

        let mut utxos = Vec::new();
        for event in &events {
            let Some(created) = event.get("createdUtxos").and_then(Value::as_array) else {
                continue;
            };
            for utxo in created {
                // The subscription reports both sides of every transaction the
                // address took part in, so keep only outputs it owns, and drop
                // any the indexer already marks as spent.
                if utxo.get("owner").and_then(Value::as_str) != Some(address) {
                    continue;
                }
                if utxo
                    .get("spentAtTransaction")
                    .map(|s| !s.is_null())
                    .unwrap_or(false)
                {
                    continue;
                }
                let value = utxo
                    .get("value")
                    .and_then(|v| {
                        v.as_str()
                            .and_then(|s| s.parse::<u128>().ok())
                            .or_else(|| v.as_u64().map(u128::from))
                    })
                    .ok_or_else(|| error::rpc_error(format!("unparsable output value: {utxo}")))?;
                utxos.push(UtxoInfo {
                    intent_hash: string_field(utxo, "intentHash"),
                    output_index: utxo.get("outputIndex").and_then(Value::as_u64).ok_or_else(
                        || error::rpc_error(format!("output without an index: {utxo}")),
                    )? as u32,
                    token_type: utxo
                        .get("tokenType")
                        .and_then(Value::as_str)
                        .unwrap_or(NIGHT_TOKEN_TYPE)
                        .to_string(),
                    value,
                    ctime: utxo.get("ctime").and_then(Value::as_i64),
                    registered_for_dust: utxo
                        .get("registeredForDustGeneration")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                });
            }
        }
        Ok(utxos)
    }

    /// Open a `graphql-transport-ws` subscription and hand every `next`
    /// payload to `on_item` until it returns `true`, or the deadline passes.
    ///
    /// Returns whether `on_item` said it was finished. A subscription that ran
    /// out of time is not an error here — the caller decides whether a partial
    /// read is usable, and for a balance it never is.
    pub async fn subscribe(
        &self,
        query: &str,
        variables: Value,
        field: &str,
        timeout: Duration,
        mut on_item: impl FnMut(&Value) -> Result<bool>,
    ) -> Result<bool> {
        let url = self.ws_url();
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|e| error::rpc_error(format!("bad indexer URL `{url}`: {e}")))?;
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            "graphql-transport-ws"
                .parse()
                .expect("a static header value"),
        );

        let (mut socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| error::rpc_error(format!("cannot reach the Midnight indexer: {e}")))?;

        socket
            .send(Message::Text(
                json!({"type": "connection_init"}).to_string(),
            ))
            .await
            .map_err(|e| error::rpc_error(format!("websocket send failed: {e}")))?;

        let mut done = false;
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let Ok(next) = tokio::time::timeout(remaining, socket.next()).await else {
                break; // the whole exchange ran out of time
            };
            let Some(message) = next else { break };
            let message =
                message.map_err(|e| error::rpc_error(format!("websocket read failed: {e}")))?;

            let text = match message {
                Message::Text(text) => text,
                Message::Ping(payload) => {
                    let _ = socket.send(Message::Pong(payload)).await;
                    continue;
                }
                Message::Close(_) => break,
                _ => continue,
            };
            let value: Value = serde_json::from_str(&text)
                .map_err(|e| error::rpc_error(format!("the indexer sent invalid JSON: {e}")))?;

            match value.get("type").and_then(Value::as_str) {
                Some("connection_ack") => {
                    socket
                        .send(Message::Text(
                            json!({
                                "id": "1",
                                "type": "subscribe",
                                "payload": {"query": query, "variables": variables},
                            })
                            .to_string(),
                        ))
                        .await
                        .map_err(|e| error::rpc_error(format!("websocket send failed: {e}")))?;
                }
                Some("next") => {
                    let payload = value.get("payload").cloned().unwrap_or(Value::Null);
                    if let Some(errors) = payload.get("errors") {
                        if !errors.is_null() {
                            return Err(error::rpc_error(format!(
                                "the indexer rejected the query: {errors}"
                            )));
                        }
                    }
                    if let Some(item) = payload.get("data").and_then(|d| d.get(field)) {
                        if on_item(item)? {
                            done = true;
                            break;
                        }
                    }
                }
                Some("error") => {
                    return Err(error::rpc_error(format!(
                        "indexer error: {}",
                        value.get("payload").unwrap_or(&Value::Null)
                    )))
                }
                Some("complete") => break,
                Some("ping") => {
                    let _ = socket
                        .send(Message::Text(json!({"type": "pong"}).to_string()))
                        .await;
                }
                _ => {}
            }
        }

        let _ = socket
            .send(Message::Text(
                json!({"id": "1", "type": "complete"}).to_string(),
            ))
            .await;
        let _ = socket.close(None).await;
        Ok(done)
    }
}

fn string_field(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_websocket_url_is_derived_from_the_http_one() {
        let indexer = Indexer::new("https://indexer.preview.midnight.network/api/v4/graphql");
        assert_eq!(
            indexer.ws_url(),
            "wss://indexer.preview.midnight.network/api/v4/graphql/ws"
        );
        // Plain HTTP downgrades to plain websockets, for a local indexer.
        assert_eq!(
            Indexer::new("http://localhost:8088/api/v4/graphql").ws_url(),
            "ws://localhost:8088/api/v4/graphql/ws"
        );
        // An endpoint that already names /ws is left alone.
        assert_eq!(
            Indexer::new("https://example.test/graphql/ws").ws_url(),
            "wss://example.test/graphql/ws"
        );
    }

    #[test]
    fn a_trailing_slash_is_trimmed_once_on_the_way_in() {
        let indexer = Indexer::new("https://example.test/graphql/");
        assert_eq!(indexer.url(), "https://example.test/graphql");
        assert_eq!(indexer.ws_url(), "wss://example.test/graphql/ws");
    }

    #[test]
    fn night_is_the_all_zero_token_type() {
        assert_eq!(NIGHT_TOKEN_TYPE.len(), 64);
        assert!(NIGHT_TOKEN_TYPE.chars().all(|c| c == '0'));
    }

    #[test]
    fn a_balance_sums_per_token_and_defaults_night_to_zero() {
        let mut balance = UnshieldedBalance::default();
        assert_eq!(
            balance.night(),
            0,
            "an address with no history holds nothing"
        );

        balance.by_token.insert(NIGHT_TOKEN_TYPE.into(), 1_500_000);
        balance.by_token.insert("aa".repeat(32), 42);
        assert_eq!(balance.night(), 1_500_000);
        // Another token's balance never leaks into the native one.
        assert_eq!(balance.by_token.len(), 2);
    }

    #[test]
    fn a_missing_string_field_reads_as_empty_rather_than_panicking() {
        let value = json!({"intentHash": "abcd"});
        assert_eq!(string_field(&value, "intentHash"), "abcd");
        assert_eq!(string_field(&value, "absent"), "");
    }
}
