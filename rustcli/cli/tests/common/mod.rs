//! Shared test scaffolding: an isolated wallet home and a scripted JSON-RPC node.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use assert_cmd::Command;
use serde_json::{json, Value};
use tempfile::TempDir;

/// A wallet home in a temp directory, plus a builder for `cwbwallet` invocations.
pub struct Wallet {
    pub home: TempDir,
    pub rpc_url: Option<String>,
}

impl Wallet {
    pub fn new() -> Self {
        Wallet {
            home: TempDir::new().unwrap(),
            rpc_url: None,
        }
    }

    /// Point the wallet at a mock node for both networks.
    pub fn with_rpc(url: &str) -> Self {
        Wallet {
            home: TempDir::new().unwrap(),
            rpc_url: Some(url.to_string()),
        }
    }

    pub fn cmd(&self, args: &[&str]) -> Command {
        let mut command = Command::cargo_bin("cwbwallet").unwrap();
        command.env("CAUSEWAYBAY_HOME", self.home.path());
        // Never inherit the developer's own secrets or endpoints.
        command.env_remove("CAUSEWAYBAY_MNEMONIC");
        command.env_remove("CAUSEWAYBAY_PRIVATE_KEY");
        if let Some(url) = &self.rpc_url {
            command.env("CAUSEWAYBAY_RPC_CRONOS_TESTNET", url);
            command.env("CAUSEWAYBAY_RPC_CRONOS_MAINNET", url);
        } else {
            command.env_remove("CAUSEWAYBAY_RPC_CRONOS_TESTNET");
            command.env_remove("CAUSEWAYBAY_RPC_CRONOS_MAINNET");
        }
        command.args(args);
        command
    }

    /// Run a command expected to succeed, returning the `data` object.
    pub fn json(&self, args: &[&str]) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let output = self.cmd(&all).output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let envelope: Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("stdout was not one JSON line ({e}): {stdout}"));
        assert_eq!(
            envelope["ok"], true,
            "expected {args:?} to succeed, got {envelope}"
        );
        envelope["data"].clone()
    }

    /// Run a command expected to fail, returning the error code.
    pub fn json_error(&self, args: &[&str]) -> String {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let output = self.cmd(&all).output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let envelope: Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("stdout was not one JSON line ({e}): {stdout}"));
        assert_eq!(
            envelope["ok"], false,
            "expected {args:?} to fail, got {envelope}"
        );
        envelope["error"]["code"].as_str().unwrap().to_string()
    }

    pub fn read_log(&self, name: &str) -> Vec<Value> {
        let path = self.home.path().join(name);
        if !path.exists() {
            return Vec::new();
        }
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}

/// The canonical BIP-39 test phrase; address index 0 is 0x9858EfFD…
pub const TEST_MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
pub const TEST_ADDRESS_0: &str = "0x9858EfFD232B4033E47d90003D41EC34EcaEda94";
pub const TEST_ADDRESS_1: &str = "0x6Fac4D18c912343BF86fa7049364Dd4E424Ab9C0";
pub const TEST_PRIVATE_KEY: &str =
    "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727";

/// A tiny JSON-RPC server that answers from a scripted response table.
pub struct MockRpc {
    pub url: String,
    responses: Arc<Mutex<HashMap<String, Value>>>,
    /// Responses consumed one per call, taking precedence over `responses`.
    queued: Arc<Mutex<HashMap<String, std::collections::VecDeque<Value>>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    server: Arc<tiny_http::Server>,
    handle: Option<JoinHandle<()>>,
}

impl MockRpc {
    pub fn start() -> Self {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let url = format!("http://{}", server.server_addr());
        let responses: Arc<Mutex<HashMap<String, Value>>> = Arc::new(Mutex::new(HashMap::new()));
        let queued: Arc<Mutex<HashMap<String, std::collections::VecDeque<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let requests: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));

        let worker_server = Arc::clone(&server);
        let worker_queued = Arc::clone(&queued);
        let worker_responses = Arc::clone(&responses);
        let worker_requests = Arc::clone(&requests);
        let handle = std::thread::spawn(move || {
            for mut request in worker_server.incoming_requests() {
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                let method = parsed
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let id = parsed.get("id").cloned().unwrap_or(json!(1));
                worker_requests.lock().unwrap().push(parsed);

                let scripted = worker_queued
                    .lock()
                    .unwrap()
                    .get_mut(&method)
                    .and_then(|queue| queue.pop_front())
                    .or_else(|| worker_responses.lock().unwrap().get(&method).cloned());
                let payload = match scripted {
                    Some(value) if value.get("__error").is_some() => {
                        json!({"jsonrpc": "2.0", "id": id, "error": value["__error"]})
                    }
                    Some(value) => json!({"jsonrpc": "2.0", "id": id, "result": value}),
                    None => json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": {"code": -32601, "message": format!("method {method} not scripted")}
                    }),
                };
                let body = payload.to_string();
                let response = tiny_http::Response::from_string(body).with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                        .unwrap(),
                );
                let _ = request.respond(response);
            }
        });

        MockRpc {
            url,
            responses,
            queued,
            requests,
            server,
            handle: Some(handle),
        }
    }

    /// Script a successful result for a method.
    pub fn on(&self, method: &str, result: Value) -> &Self {
        self.responses
            .lock()
            .unwrap()
            .insert(method.to_string(), result);
        self
    }

    /// Script consecutive results for a method, consumed one per call.
    pub fn on_sequence(&self, method: &str, results: Vec<Value>) -> &Self {
        self.queued
            .lock()
            .unwrap()
            .insert(method.to_string(), results.into_iter().collect());
        self
    }

    /// Script a JSON-RPC error for a method.
    pub fn on_error(&self, method: &str, code: i64, message: &str) -> &Self {
        self.responses.lock().unwrap().insert(
            method.to_string(),
            json!({"__error": {"code": code, "message": message}}),
        );
        self
    }

    /// A node that answers every read the wallet normally makes.
    pub fn with_defaults(self) -> Self {
        self.on("eth_chainId", json!("0x152")) // 338
            .on("eth_blockNumber", json!("0x1e240")) // 123456
            .on("eth_gasPrice", json!("0x12a05f200")) // 5 gwei
            .on("eth_getBalance", json!("0x8ac7230489e80000")) // 10 ether
            .on("eth_getTransactionCount", json!("0x3"))
            .on("eth_estimateGas", json!("0xcf08"));
        self
    }

    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }

    /// Every request body recorded for one method.
    pub fn requests_for(&self, method: &str) -> Vec<Value> {
        self.requests()
            .into_iter()
            .filter(|r| r.get("method").and_then(Value::as_str) == Some(method))
            .collect()
    }
}

impl Drop for MockRpc {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
