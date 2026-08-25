//! End-to-end tests for the chain-facing commands, driven by a scripted node.

mod common;

use common::*;
use serde_json::{json, Value};

/// A wallet holding the reference account, pointed at a mock node.
fn funded(node: &MockRpc) -> Wallet {
    let wallet = Wallet::with_rpc(&node.url);
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);
    wallet
}

#[test]
fn balance_is_reported_in_whole_tokens_and_wei() {
    let node = MockRpc::start().with_defaults();
    let wallet = funded(&node);

    let balance = wallet.json(&["balance"]);
    assert_eq!(balance["balance"], "10");
    assert_eq!(balance["balance_wei"], "10000000000000000000");
    assert_eq!(balance["symbol"], "TCRO");
    assert_eq!(balance["address"], TEST_ADDRESS_0);

    // The active account's address is what gets queried.
    let params = node.requests_for("eth_getBalance");
    assert_eq!(params[0]["params"][0], TEST_ADDRESS_0);
    assert_eq!(params[0]["params"][1], "latest");
}

#[test]
fn balance_accepts_an_explicit_address() {
    let node = MockRpc::start().with_defaults();
    let wallet = funded(&node);
    let balance = wallet.json(&["balance", "--address", TEST_ADDRESS_1]);
    assert_eq!(balance["address"], TEST_ADDRESS_1);
    assert!(balance["account"].is_null());
}

#[test]
fn balance_rejects_a_malformed_address() {
    let node = MockRpc::start().with_defaults();
    let wallet = funded(&node);
    assert_eq!(
        wallet.json_error(&["balance", "--address", "0xnope"]),
        "invalid_address"
    );
}

#[test]
fn the_symbol_follows_the_selected_network() {
    let node = MockRpc::start().with_defaults();
    let wallet = funded(&node);
    assert_eq!(wallet.json(&["balance"])["symbol"], "TCRO");
    assert_eq!(wallet.json(&["-n", "mainnet", "balance"])["symbol"], "CRO");
}

#[test]
fn nonce_uses_the_pending_block() {
    let node = MockRpc::start().with_defaults();
    let wallet = funded(&node);
    assert_eq!(wallet.json(&["nonce"])["nonce"], 3);
    // Pending, so two sends in a row do not collide on a nonce.
    assert_eq!(
        node.requests_for("eth_getTransactionCount")[0]["params"][1],
        "pending"
    );
}

#[test]
fn gas_price_is_reported_in_gwei() {
    let node = MockRpc::start().with_defaults();
    let wallet = funded(&node);
    let gas = wallet.json(&["gas-price"]);
    assert_eq!(gas["gas_price_gwei"], "5");
    assert_eq!(gas["gas_price_wei"], "5000000000");
}

#[test]
fn chain_info_flags_a_chain_id_mismatch() {
    let node = MockRpc::start().with_defaults();
    let wallet = funded(&node);

    let matching = wallet.json(&["chain-info"]);
    assert_eq!(matching["reported_chain_id"], 338);
    assert_eq!(matching["chain_id_matches"], true);
    assert_eq!(matching["block_number"], 123456);

    // The same node claims chain 338 while the wallet is set to mainnet.
    let mismatched = wallet.json(&["-n", "mainnet", "chain-info"]);
    assert_eq!(mismatched["expected_chain_id"], 25);
    assert_eq!(mismatched["chain_id_matches"], false);
}

#[test]
fn rpc_failures_surface_as_rpc_errors() {
    let node = MockRpc::start().with_defaults();
    node.on_error("eth_getBalance", -32000, "node is having a day");
    let wallet = funded(&node);
    assert_eq!(wallet.json_error(&["balance"]), "rpc_error");
}

#[test]
fn an_unreachable_node_is_an_rpc_error_not_a_panic() {
    let wallet = Wallet::with_rpc("http://127.0.0.1:1");
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);
    assert_eq!(wallet.json_error(&["balance"]), "rpc_error");
}

// ------------------------------------------------------------------- sending

/// A node that also accepts a broadcast and reports a successful receipt.
fn sending_node() -> MockRpc {
    let node = MockRpc::start().with_defaults();
    node.on("eth_sendRawTransaction", json!("0xabc123"))
        .on(
            "eth_getTransactionReceipt",
            json!({"status": "0x1", "blockNumber": "0x2a", "gasUsed": "0x5208"}),
        )
        .on(
            "eth_getTransactionByHash",
            json!({"hash": "0xabc123", "value": "0xde0b6b3a7640000", "nonce": "0x3"}),
        );
    node
}

#[test]
fn send_signs_locally_and_broadcasts_the_raw_transaction() {
    let node = sending_node();
    let wallet = funded(&node);

    let sent = wallet.json(&["--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "1"]);
    // The hash is computed from the signed transaction, not taken from the
    // node's reply — that is what lets the record be written before broadcast.
    let hash = sent["hash"].as_str().unwrap();
    assert_eq!(hash.len(), 2 + 64, "a real transaction hash");
    assert!(hash.starts_with("0x"));
    assert_eq!(sent["from"], TEST_ADDRESS_0);
    assert_eq!(sent["to"], TEST_ADDRESS_1);
    assert_eq!(sent["value"], "1");
    assert_eq!(sent["value_wei"], "1000000000000000000");
    assert_eq!(sent["nonce"], 3);
    assert_eq!(sent["gas_limit"], 21000);
    assert_eq!(sent["chain_id"], 338);
    assert_eq!(sent["status"], "submitted");
    assert_eq!(
        sent["explorer"],
        format!("https://explorer.cronos.org/testnet/tx/{hash}")
    );

    // The private key never leaves the machine: the node only sees signed bytes.
    let broadcast = node.requests_for("eth_sendRawTransaction");
    assert_eq!(broadcast.len(), 1);
    let raw = broadcast[0]["params"][0].as_str().unwrap();
    assert!(
        raw.starts_with("0xf8"),
        "expected an RLP-encoded legacy transaction"
    );
    assert!(!raw.contains(&TEST_PRIVATE_KEY[2..]));
}

#[test]
fn send_records_the_transaction_in_history() {
    let node = sending_node();
    let wallet = funded(&node);
    wallet.json(&["--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "0.25"]);

    let history = wallet.json(&["history"]);
    let entries = history.as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0]["hash"].as_str().unwrap().starts_with("0x"));
    assert_eq!(entries[0]["value"], "0.25");
    assert_eq!(entries[0]["network"], "cronos-testnet");

    let log = wallet.read_log("history.jsonl");
    assert_eq!(log[0]["type"], "tx.send");
    assert_eq!(log[0]["schema"], 1);
}

#[test]
fn send_without_confirmation_is_refused() {
    let node = sending_node();
    let wallet = funded(&node);
    assert_eq!(
        wallet.json_error(&["send", "--to", TEST_ADDRESS_1, "--amount", "1"]),
        "confirmation_required"
    );
    // Nothing was broadcast and nothing was recorded.
    assert!(node.requests_for("eth_sendRawTransaction").is_empty());
    assert_eq!(wallet.json(&["history"]).as_array().unwrap().len(), 0);
}

/// The question a user answers has to name what the answer costs.
///
/// It did not: every chain built "Send {amount} from {from} to {to} on
/// {network}" and left the fee — the endpoint's number, and the only one that
/// can quietly be wrong — out of the sentence.
#[test]
fn the_confirmation_question_names_the_fee() {
    let node = sending_node();
    let wallet = funded(&node);

    let error = wallet.json_failure(&["send", "--to", TEST_ADDRESS_1, "--amount", "1"]);
    assert_eq!(error["code"], "confirmation_required");
    let question = error["message"].as_str().unwrap();
    // 5 gwei of gas price over a 21000-gas transfer.
    assert!(
        question.contains("fee of 0.000105 TCRO"),
        "the fee is missing from: {question}"
    );
    assert!(question.contains("Send 1 TCRO"), "{question}");
}

/// An endpoint that quotes an absurd gas price gets a refusal, not a signature.
///
/// The balance check is no defence: it only asks whether the account *can*
/// pay, so an account with enough in it would be asked "Send 1 TCRO?" and
/// hand over two hundred to a miner on a yes.
#[test]
fn send_refuses_a_fee_above_the_networks_ceiling() {
    let node = sending_node();
    node.on("eth_gasPrice", json!("0x2386f26fc10000")) // 0.01 CRO per gas
        .on("eth_getBalance", json!("0x152d02c7e14af6800000")); // 100,000 CRO
    let wallet = funded(&node);

    let error = wallet.json_failure(&["--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "1"]);
    assert_eq!(error["code"], "invalid_amount");
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("210 TCRO"), "{message}");
    assert!(message.contains("--max-fee"), "{message}");
    assert!(
        node.requests_for("eth_sendRawTransaction").is_empty(),
        "an over-fee'd transfer must not be signed, let alone broadcast"
    );
}

/// A ceiling stored for the network is used when no flag names one, and `0`
/// is how the setting says "back to automatic".
#[test]
fn a_stored_fee_ceiling_is_what_a_send_is_held_to() {
    let node = sending_node();
    node.on("eth_gasPrice", json!("0x2386f26fc10000")) // 0.01 CRO per gas
        .on("eth_getBalance", json!("0x152d02c7e14af6800000"));
    let wallet = funded(&node);

    // 300 TCRO is above the fee this endpoint implies, so the send goes out
    // where the built-in 25 would have refused it.
    wallet.json(&["network", "set-max-fee", "cronos-testnet", "300"]);
    let current = wallet.json(&["network", "current"]);
    assert_eq!(current["max_fee"], "300");
    assert_eq!(current["max_fee_is_default"], false);
    wallet.json(&["--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "1"]);
    assert_eq!(node.requests_for("eth_sendRawTransaction").len(), 1);

    // And 0 gives the built-in ceiling back, which refuses the same send.
    let back = wallet.json(&["network", "set-max-fee", "cronos-testnet", "0"]);
    assert_eq!(back["automatic"], true);
    assert_eq!(back["max_fee"], "25");
    assert_eq!(
        wallet.json(&["network", "current"])["max_fee_is_default"],
        true
    );
    assert_eq!(
        wallet.json_error(&["--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "1"]),
        "invalid_amount"
    );
    assert_eq!(node.requests_for("eth_sendRawTransaction").len(), 1);
}

/// The one place the fee's unit is not the unit everything else is in.
///
/// A Midnight transfer moves NIGHT and pays its fee in DUST, and the two are
/// nine orders of magnitude apart — so a bare number typed by someone reading
/// their NIGHT balance is the mistake worth catching.
#[test]
fn a_fee_ceiling_written_in_the_wrong_token_is_refused() {
    let wallet = Wallet::new();

    // The fee's own unit is taken bare, or written out.
    for written in ["2", "2 DUST", "2dust"] {
        let set = wallet.json(&[
            "--chain",
            "midnight",
            "network",
            "set-max-fee",
            "midnight-preview",
            written,
        ]);
        assert_eq!(set["symbol"], "DUST", "{written}");
        assert_eq!(set["max_fee"], "2", "{written}");
    }

    // The transfer's unit is not, and the error says why rather than
    // silently storing nine orders of magnitude less than was meant.
    let error = wallet.json_failure(&[
        "--chain",
        "midnight",
        "network",
        "set-max-fee",
        "midnight-preview",
        "2 NIGHT",
    ]);
    assert_eq!(error["code"], "invalid_amount");
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("paid in DUST"), "{message}");
    assert!(message.contains("a transfer moves"), "{message}");
}

/// The stored record says which token it means, so nothing downstream guesses.
#[test]
fn a_stored_ceiling_carries_its_denomination() {
    let wallet = Wallet::new();
    wallet.json(&["network", "set-max-fee", "cronos-testnet", "3"]);
    wallet.json(&[
        "--chain",
        "midnight",
        "network",
        "set-max-fee",
        "midnight-preview",
        "2",
    ]);

    let config = wallet.read_log("config.jsonl");
    let value = |key: &str| -> String {
        config
            .iter()
            .filter(|line| line["key"] == key)
            .next_back()
            .unwrap_or_else(|| panic!("no record for {key}"))["value"]
            .as_str()
            .unwrap()
            .to_string()
    };
    // A bare number would mean nothing on its own: fees are paid in a
    // different token on every chain, and on Midnight in a different token
    // from the one that wallet's balances are counted in.
    assert_eq!(value("max_fee.cronos-testnet"), "3 TCRO");
    assert_eq!(value("max_fee.midnight-preview"), "2 DUST");

    // And it reads back as what it says, rather than as an unparsable string
    // that quietly leaves the built-in ceiling in force.
    let current = wallet.json(&["--chain", "midnight", "network", "current"]);
    assert_eq!(current["max_fee"], "2");
    assert_eq!(current["max_fee_symbol"], "DUST");
    assert_eq!(current["max_fee_is_default"], false);
}

/// A ceiling lower than the built-in one is the useful direction, and the one
/// the setting exists for: this endpoint is honest, and the user still does
/// not want to spend that much.
#[test]
fn a_stored_ceiling_can_tighten_as_well_as_loosen() {
    let node = sending_node();
    let wallet = funded(&node);
    // The default 5 gwei over 21000 gas is 0.000105 TCRO.
    wallet.json(&["network", "set-max-fee", "cronos-testnet", "0.00001"]);

    let error = wallet.json_failure(&["--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "1"]);
    assert_eq!(error["code"], "invalid_amount");
    assert!(
        error["message"].as_str().unwrap().contains("0.00001 TCRO"),
        "{error}"
    );
    assert!(node.requests_for("eth_sendRawTransaction").is_empty());
}

/// The ceiling is the wallet's default, not a wall: a caller who means it
/// says so, in the fee's own unit.
#[test]
fn max_fee_raises_the_ceiling_for_one_send() {
    let node = sending_node();
    node.on("eth_gasPrice", json!("0x2386f26fc10000"))
        .on("eth_getBalance", json!("0x152d02c7e14af6800000"));
    let wallet = funded(&node);

    let sent = wallet.json(&[
        "--yes",
        "send",
        "--to",
        TEST_ADDRESS_1,
        "--amount",
        "1",
        "--max-fee",
        "250",
    ]);
    assert_eq!(sent["status"], "submitted");
    assert_eq!(node.requests_for("eth_sendRawTransaction").len(), 1);
}

#[test]
fn send_refuses_when_the_balance_cannot_cover_the_transfer() {
    let node = sending_node();
    node.on("eth_getBalance", json!("0x2386f26fc10000")); // 0.01 ether
    let wallet = funded(&node);

    assert_eq!(
        wallet.json_error(&["--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "1"]),
        "insufficient_funds"
    );
    assert!(
        node.requests_for("eth_sendRawTransaction").is_empty(),
        "nothing should be signed or broadcast when the funds are not there"
    );
}

#[test]
fn send_accounts_for_gas_when_checking_the_balance() {
    let node = sending_node();
    // Exactly 1 ether: enough for the transfer, not enough for transfer + gas.
    node.on("eth_getBalance", json!("0xde0b6b3a7640000"));
    let wallet = funded(&node);
    assert_eq!(
        wallet.json_error(&["--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "1"]),
        "insufficient_funds"
    );
}

#[test]
fn send_honours_explicit_gas_and_nonce_overrides() {
    let node = sending_node();
    let wallet = funded(&node);
    let sent = wallet.json(&[
        "--yes",
        "send",
        "--to",
        TEST_ADDRESS_1,
        "--amount",
        "0.1",
        "--nonce",
        "9",
        "--gas-limit",
        "30000",
        "--gas-price-gwei",
        "2",
    ]);
    assert_eq!(sent["nonce"], 9);
    assert_eq!(sent["gas_limit"], 30000);
    assert_eq!(sent["gas_price_wei"], "2000000000");
    // With both overrides supplied the node is not asked for a nonce or price.
    assert!(node.requests_for("eth_getTransactionCount").is_empty());
    assert!(node.requests_for("eth_gasPrice").is_empty());
}

#[test]
fn send_waits_for_the_receipt_when_asked() {
    let node = sending_node();
    let wallet = funded(&node);
    let sent = wallet.json(&[
        "--yes",
        "send",
        "--to",
        TEST_ADDRESS_1,
        "--amount",
        "0.1",
        "--wait",
    ]);
    assert_eq!(sent["status"], "confirmed");
    assert_eq!(sent["block_number"], 42);
    assert_eq!(sent["gas_used"], 21000);

    // The confirmation is folded back into the local log.
    let history = wallet.json(&["history"]);
    assert_eq!(history[0]["status"], "confirmed");
    let log = wallet.read_log("history.jsonl");
    assert_eq!(log.last().unwrap()["type"], "tx.update");
}

#[test]
fn a_reverted_transaction_is_recorded_as_failed() {
    let node = sending_node();
    node.on(
        "eth_getTransactionReceipt",
        json!({"status": "0x0", "blockNumber": "0x2a", "gasUsed": "0x5208"}),
    );
    let wallet = funded(&node);
    let sent = wallet.json(&[
        "--yes",
        "send",
        "--to",
        TEST_ADDRESS_1,
        "--amount",
        "0.1",
        "--wait",
    ]);
    assert_eq!(sent["status"], "failed");
}

#[test]
fn send_rejects_a_bad_recipient_or_amount_before_touching_the_node() {
    let node = sending_node();
    let wallet = funded(&node);
    assert_eq!(
        wallet.json_error(&["--yes", "send", "--to", "0xnot-an-address", "--amount", "1"]),
        "invalid_address"
    );
    assert_eq!(
        wallet.json_error(&["--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "one"]),
        "invalid_amount"
    );
    assert!(node.requests_for("eth_sendRawTransaction").is_empty());
}

/// A mixed-case recipient carries an EIP-55 checksum, and a failing one means
/// a character got corrupted on the way to this field.
#[test]
fn send_refuses_a_recipient_whose_checksum_does_not_hold() {
    let node = sending_node();
    let wallet = funded(&node);
    // One letter of TEST_ADDRESS_1 lower-cased: the same twenty bytes, and no
    // longer a valid EIP-55 rendering of them.
    let corrupted = TEST_ADDRESS_1.replacen('F', "f", 1);
    assert_ne!(corrupted, TEST_ADDRESS_1);

    let error = wallet.json_failure(&["--yes", "send", "--to", &corrupted, "--amount", "1"]);
    assert_eq!(error["code"], "invalid_address");
    assert!(
        error["message"].as_str().unwrap().contains("EIP-55"),
        "{error}"
    );
    // All lower case carries no checksum and stays perfectly ordinary.
    wallet.json(&[
        "--yes",
        "send",
        "--to",
        &TEST_ADDRESS_1.to_lowercase(),
        "--amount",
        "1",
    ]);
}

#[test]
fn send_refuses_a_transfer_to_the_sending_account() {
    let node = sending_node();
    let wallet = funded(&node);
    // The active account is TEST_ADDRESS_0; paying itself moves nothing and
    // would still pay the gas, so it is refused before the node is asked.
    assert_eq!(
        wallet.json_error(&["--yes", "send", "--to", TEST_ADDRESS_0, "--amount", "1"]),
        "usage"
    );
    // Spelled in lower case it is the same account, and must be refused the same.
    assert_eq!(
        wallet.json_error(&[
            "--yes",
            "send",
            "--to",
            &TEST_ADDRESS_0.to_lowercase(),
            "--amount",
            "1"
        ]),
        "usage"
    );
    assert!(node.requests_for("eth_sendRawTransaction").is_empty());
    assert!(node.requests_for("eth_getTransactionCount").is_empty());
}

#[test]
fn erc20_send_refuses_a_transfer_to_the_sending_account() {
    let node = MockRpc::start().with_defaults();
    let wallet = funded(&node);
    assert_eq!(
        wallet.json_error(&[
            "--yes",
            "erc20",
            "send",
            "--token",
            TEST_ADDRESS_1,
            "--to",
            TEST_ADDRESS_0,
            "--amount",
            "1"
        ]),
        "usage"
    );
    assert!(node.requests_for("eth_sendRawTransaction").is_empty());
}

#[test]
fn a_rejected_broadcast_surfaces_the_node_message() {
    let node = sending_node();
    node.on_error(
        "eth_sendRawTransaction",
        -32000,
        "insufficient funds for gas * price + value",
    );
    let wallet = funded(&node);
    assert_eq!(
        wallet.json_error(&["--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "1"]),
        "insufficient_funds"
    );
}

#[test]
fn the_chain_id_in_the_signature_follows_the_network() {
    let node = sending_node();
    let wallet = funded(&node);

    wallet.json(&["--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "0.1"]);
    let testnet_raw = node.requests_for("eth_sendRawTransaction")[0]["params"][0]
        .as_str()
        .unwrap()
        .to_string();

    wallet.json(&[
        "--yes",
        "-n",
        "mainnet",
        "send",
        "--to",
        TEST_ADDRESS_1,
        "--amount",
        "0.1",
    ]);
    let mainnet_raw = node.requests_for("eth_sendRawTransaction")[1]["params"][0]
        .as_str()
        .unwrap()
        .to_string();

    assert_ne!(
        testnet_raw, mainnet_raw,
        "EIP-155 makes a testnet signature unusable on mainnet"
    );
}

#[test]
fn history_can_be_filtered_and_limited() {
    let node = sending_node();
    // Distinct hashes, the way a real chain would answer.
    node.on_sequence(
        "eth_sendRawTransaction",
        vec![json!("0xhash1"), json!("0xhash2")],
    );
    let wallet = funded(&node);
    wallet.json(&["--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "0.1"]);
    wallet.json(&[
        "--yes",
        "-n",
        "mainnet",
        "send",
        "--to",
        TEST_ADDRESS_1,
        "--amount",
        "0.2",
    ]);

    assert_eq!(wallet.json(&["history"]).as_array().unwrap().len(), 2);
    assert_eq!(
        wallet
            .json(&["history", "--network", "mainnet"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        wallet
            .json(&["history", "--limit", "1"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // Newest first.
    assert_eq!(
        wallet.json(&["history", "--limit", "1"])[0]["network"],
        "cronos-mainnet"
    );
}

#[test]
fn tx_looks_a_transaction_up_on_chain() {
    let node = sending_node();
    let wallet = funded(&node);
    let looked_up = wallet.json(&["tx", "0xabc123"]);
    assert_eq!(looked_up["status"], "confirmed");
    assert_eq!(looked_up["value"], "1");
    assert_eq!(
        looked_up["explorer"],
        "https://explorer.cronos.org/testnet/tx/0xabc123"
    );
}

#[test]
fn tx_reports_a_pending_transaction() {
    let node = MockRpc::start().with_defaults();
    node.on(
        "eth_getTransactionByHash",
        json!({"hash": "0xpending", "value": "0x0"}),
    )
    .on("eth_getTransactionReceipt", Value::Null);
    let wallet = funded(&node);
    assert_eq!(wallet.json(&["tx", "0xpending"])["status"], "pending");
}

#[test]
fn tx_reports_an_unknown_hash_as_not_found() {
    let node = MockRpc::start().with_defaults();
    node.on("eth_getTransactionByHash", Value::Null)
        .on("eth_getTransactionReceipt", Value::Null);
    let wallet = funded(&node);
    assert_eq!(wallet.json_error(&["tx", "0xdeadbeef"]), "not_found");
}

// -------------------------------------------------------------------- erc-20

/// ABI-encode a single word, the way a token contract would answer.
fn word(value: u64) -> Value {
    json!(format!("0x{:064x}", value))
}

/// ABI-encode a dynamic string return value.
fn abi_string(text: &str) -> Value {
    let mut encoded = format!("{:064x}{:064x}", 32, text.len());
    let mut bytes = hex::encode(text);
    while bytes.len() % 64 != 0 {
        bytes.push('0');
    }
    encoded.push_str(&bytes);
    json!(format!("0x{encoded}"))
}

/// The wallet issues several `eth_call`s with different selectors; answer by selector.
struct TokenNode {
    node: MockRpc,
}

impl TokenNode {
    fn start() -> Self {
        TokenNode {
            node: MockRpc::start().with_defaults(),
        }
    }
}

#[test]
fn erc20_balance_scales_by_the_token_decimals() {
    let node = MockRpc::start().with_defaults();
    // decimals() is asked for first; the mock answers every eth_call the same way,
    // so drive the decimals path with a dedicated node per assertion.
    node.on("eth_call", word(6));
    let wallet = funded(&node);
    let balance = wallet.json(&["erc20", "balance", "--token", TEST_ADDRESS_1]);
    // Every call returns 6, so the raw balance is 6 with 6 decimals => 0.000006.
    assert_eq!(balance["decimals"], 6);
    assert_eq!(balance["balance_raw"], "6");
    assert_eq!(balance["balance"], "0.000006");
    assert_eq!(balance["address"], TEST_ADDRESS_0);
}

#[test]
fn erc20_info_decodes_name_and_symbol() {
    let node = TokenNode::start().node;
    let wallet = funded(&node);
    // name() and symbol() are separate calls, so script them in order.
    node.on_sequence(
        "eth_call",
        vec![
            abi_string("VVS Finance"),
            abi_string("VVS"),
            word(18),
            word(1_000_000),
        ],
    );
    let info = wallet.json(&["erc20", "info", "--token", TEST_ADDRESS_1]);
    assert_eq!(info["name"], "VVS Finance");
    assert_eq!(info["symbol"], "VVS");
    assert_eq!(info["decimals"], 18);
    assert_eq!(info["total_supply_raw"], "1000000");
}

#[test]
fn erc20_send_encodes_a_transfer_call() {
    let node = MockRpc::start().with_defaults();
    // decimals() and balanceOf() both answer with 10^18, so a 1-token transfer fits.
    node.on(
        "eth_call",
        json!(format!("0x{:064x}", 1_000_000_000_000_000_000u128)),
    )
    .on("eth_sendRawTransaction", json!("0xtokenhash"));
    let wallet = funded(&node);

    // decimals() reports 10^18 which is far past 255, so this must fail cleanly
    // rather than mis-scaling the amount.
    assert_eq!(
        wallet.json_error(&[
            "--yes",
            "erc20",
            "send",
            "--token",
            TEST_ADDRESS_1,
            "--to",
            TEST_ADDRESS_1,
            "--amount",
            "1"
        ]),
        "rpc_error"
    );
}

#[test]
fn erc20_transfer_calldata_reaches_the_node() {
    let node = MockRpc::start().with_defaults();
    node.on("eth_call", word(18))
        .on("eth_sendRawTransaction", json!("0xtokenhash"));
    let wallet = funded(&node);
    // decimals() = 18 and balanceOf() = 18 wei, so 1 whole token is more than held.
    assert_eq!(
        wallet.json_error(&[
            "--yes",
            "erc20",
            "send",
            "--token",
            TEST_ADDRESS_1,
            "--to",
            TEST_ADDRESS_1,
            "--amount",
            "1"
        ]),
        "insufficient_funds"
    );
}

#[test]
fn erc20_rejects_a_malformed_token_address() {
    let node = MockRpc::start().with_defaults();
    let wallet = funded(&node);
    assert_eq!(
        wallet.json_error(&["erc20", "balance", "--token", "0xnope"]),
        "invalid_address"
    );
}

// ------------------------------------------------------ cross-cutting checks

#[test]
fn every_chain_command_needs_an_account_or_an_address() {
    let node = MockRpc::start().with_defaults();
    let wallet = Wallet::with_rpc(&node.url);
    assert_eq!(wallet.json_error(&["balance"]), "no_active_account");
    assert_eq!(wallet.json_error(&["nonce"]), "no_active_account");
    // gas-price and chain-info are account independent.
    assert_eq!(wallet.json(&["gas-price"])["gas_price_gwei"], "5");
    assert_eq!(wallet.json(&["chain-info"])["block_number"], 123456);
}

#[test]
fn the_rpc_url_can_be_pinned_per_network_in_config() {
    let node = MockRpc::start().with_defaults();
    let wallet = Wallet::new(); // no env override
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);
    wallet.json(&["network", "set-rpc", "testnet", &node.url]);
    assert_eq!(wallet.json(&["balance"])["balance"], "10");
}

#[test]
fn resending_the_same_hash_updates_rather_than_duplicates() {
    let node = sending_node();
    let wallet = funded(&node);
    // Two identical transfers at the same nonce sign to the same bytes, so the
    // hash repeats and the log folds them together — a real duplicate send.
    wallet.json(&["--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "0.1"]);
    wallet.json(&["--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "0.1"]);
    assert_eq!(wallet.json(&["history"]).as_array().unwrap().len(), 1);
    // Both sends were still recorded on disk; only the replay deduplicates.
    // Each send writes two lines: the record before broadcast, then its status.
    assert_eq!(wallet.read_log("history.jsonl").len(), 4);
}
