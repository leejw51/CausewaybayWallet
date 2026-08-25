//! The async HTTP the chain clients share.
//!
//! Four chains, four protocols: Solana speaks JSON-RPC 2.0, Koios speaks plain
//! REST, the Midnight indexer speaks GraphQL over both HTTP and WebSocket, and
//! the EVM nodes speak JSON-RPC too. What they have in common is one reqwest
//! client, one set of timeouts, and one rule for turning a transport failure
//! into an [`Error`] a user can act on — so that lives here and the protocol
//! differences live in the chains.
//!
//! [`Error`]: crate::error::Error

use std::time::Duration;

use serde_json::{json, Value};

use crate::error::{self, Result};

/// The default ceiling on a single request.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// One shared client, so keep-alive connections are reused across calls and
/// across chains. Building one per request is what makes a four-chain
/// `balance --all` slower than it needs to be.
pub fn client() -> Result<&'static reqwest::Client> {
    use std::sync::OnceLock;
    static CLIENT: OnceLock<std::result::Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(DEFAULT_TIMEOUT)
                .connect_timeout(Duration::from_secs(10))
                .user_agent(concat!("causewaybay-wallet/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| error::rpc_error(format!("cannot build the HTTP client: {e}")))
}

/// POST a JSON body and read a JSON reply, surfacing the body of an error status.
///
/// A 4xx or 5xx from a node almost always carries the reason in its body, and
/// dropping it in favour of the status code is what turns a fixable mistake
/// into "rpc error: HTTP 400".
pub async fn post_json(url: &str, body: &Value) -> Result<Value> {
    let response = client()?
        .post(url)
        .json(body)
        .send()
        .await
        .map_err(|e| error::rpc_error(format!("request to {url} failed: {e}")))?;
    read_json(url, response).await
}

/// POST raw bytes with an explicit content type, returning the reply as text.
///
/// Koios takes a submitted transaction as `application/cbor`, not as JSON.
pub async fn post_bytes(url: &str, content_type: &str, bytes: Vec<u8>) -> Result<String> {
    let response = client()?
        .post(url)
        .header("content-type", content_type)
        .body(bytes)
        .send()
        .await
        .map_err(|e| error::rpc_error(format!("request to {url} failed: {e}")))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| error::rpc_error(format!("reply from {url} could not be read: {e}")))?;
    if !status.is_success() {
        return Err(status_error(url, status.as_u16(), &text));
    }
    Ok(text)
}

/// GET a JSON document.
pub async fn get_json(url: &str) -> Result<Value> {
    let response = client()?
        .get(url)
        .send()
        .await
        .map_err(|e| error::rpc_error(format!("request to {url} failed: {e}")))?;
    read_json(url, response).await
}

/// One JSON-RPC 2.0 call, unwrapping `result`.
pub async fn json_rpc(url: &str, method: &str, params: Value) -> Result<Value> {
    let body = json!({"jsonrpc": "2.0", "id": next_id(), "method": method, "params": params});
    let mut value = post_json(url, &body).await?;

    // Some providers send `"error": null` beside a valid result, so only a
    // non-null error object counts as a failure.
    if let Some(rpc_error) = value.get("error") {
        if !rpc_error.is_null() {
            let message = rpc_error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown RPC error");
            let code = rpc_error.get("code").and_then(Value::as_i64).unwrap_or(0);
            // By far the most common failure, and the one with its own code.
            if message.to_lowercase().contains("insufficient funds") {
                return Err(error::insufficient_funds(message));
            }
            return Err(error::rpc_error(format!(
                "{method} failed ({code}): {message}"
            )));
        }
    }
    value
        .get_mut("result")
        .map(Value::take)
        .ok_or_else(|| error::rpc_error(format!("{method} response has no result field")))
}

/// One GraphQL query, unwrapping `data`.
pub async fn graphql(url: &str, query: &str, variables: Value) -> Result<Value> {
    let body = json!({"query": query, "variables": variables});
    let mut value = post_json(url, &body).await?;
    if let Some(errors) = value.get("errors") {
        if !errors.is_null() {
            return Err(error::rpc_error(format!(
                "the query was rejected: {errors}"
            )));
        }
    }
    value
        .get_mut("data")
        .map(Value::take)
        .ok_or_else(|| error::rpc_error("GraphQL response has no data field"))
}

// ------------------------------------------------------------------ internals

async fn read_json(url: &str, response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| error::rpc_error(format!("reply from {url} could not be read: {e}")))?;
    if !status.is_success() {
        return Err(status_error(url, status.as_u16(), &text));
    }
    serde_json::from_str(&text).map_err(|_| {
        error::rpc_error(format!(
            "{url} returned a non-JSON response: {}",
            excerpt(&text)
        ))
    })
}

fn status_error(url: &str, status: u16, body: &str) -> crate::error::Error {
    let body = body.trim();
    if body.to_lowercase().contains("insufficient funds") {
        return error::insufficient_funds(body.to_string());
    }
    error::rpc_error(format!("{url} returned HTTP {status}: {}", excerpt(body)))
}

/// Enough of a reply to diagnose it, never enough to bury the message.
fn excerpt(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= 300 {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(300).collect();
    format!("{head}…")
}

fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static ID: AtomicU64 = AtomicU64::new(1);
    ID.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_advance_so_replies_can_be_matched() {
        let first = next_id();
        assert!(next_id() > first);
    }

    #[test]
    fn a_short_body_is_shown_whole() {
        assert_eq!(excerpt("  nonce too low  "), "nonce too low");
    }

    #[test]
    fn a_long_body_is_cut_with_an_ellipsis() {
        let cut = excerpt(&"x".repeat(1000));
        assert_eq!(cut.chars().count(), 301);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn multibyte_bodies_are_cut_on_character_boundaries() {
        // Slicing by byte here would panic on a UTF-8 boundary, which would
        // turn a node's error reply into a crash.
        let cut = excerpt(&"日".repeat(1000));
        assert_eq!(cut.chars().count(), 301);
    }

    #[test]
    fn a_funding_failure_keeps_its_own_code_wherever_it_appears() {
        // Both the JSON-RPC error object and a bare HTTP body can carry it,
        // and callers branch on the code rather than the wording.
        let from_status = status_error("http://node", 400, "insufficient funds for gas * price");
        assert_eq!(from_status.code, error::Code::InsufficientFunds);

        let other = status_error("http://node", 500, "upstream exploded");
        assert_eq!(other.code, error::Code::RpcError);
        assert!(other.message.contains("HTTP 500"));
    }

    #[test]
    fn the_client_is_built_once_and_shared() {
        let first = client().unwrap();
        let second = client().unwrap();
        assert!(std::ptr::eq(first, second));
    }
}
