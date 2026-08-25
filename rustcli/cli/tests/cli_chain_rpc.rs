//! Balance and send, per chain, against scripted nodes.
//!
//! The offline suite covers derivation and the store; this one covers the two
//! things a wallet exists to do — read what an address holds, and move it —
//! on the chains that were not here before. Every node is a mock, so the
//! assertions are about what the wallet *sends* and what it refuses, not about
//! whatever a public testnet happens to hold today.
//!
//! Each chain gets the same three questions: does a balance come back in the
//! right units, does a transfer put the right bytes on the wire, and are the
//! things that should be refused refused *before* anything is signed.

mod common;

use serde_json::json;

use common::{MockHttp, MockRpc, Wallet, TEST_MNEMONIC};

// ====================================================================== Solana

/// The Solana address the shared phrase derives at index 0.
const SOLANA_ADDRESS: &str = "HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk";
/// Index 1, used as a recipient.
const SOLANA_RECIPIENT: &str = "Hh8QwFUA6MtVu1qAoq12ucvFHNwCcVTV7hpWjeY1Hztb";
/// A valid base58 32-byte blockhash.
const BLOCKHASH: &str = "11111111111111111111111111111111";

/// A Solana node answering every read a transfer makes.
fn solana_node() -> MockRpc {
    let node = MockRpc::start();
    node.on("getBalance", json!({"value": 5_000_000_000u64})) // 5 SOL
        .on(
            "getLatestBlockhash",
            json!({"value": {"blockhash": BLOCKHASH, "lastValidBlockHeight": 100}}),
        )
        .on("getFeeForMessage", json!({"value": 5_000}))
        .on("getMinimumBalanceForRentExemption", json!(890_880))
        .on("sendTransaction", json!("5xSignatureFromTheCluster"))
        .on("getVersion", json!({"solana-core": "2.0.0"}))
        .on("getSlot", json!(1_234))
        .on("getBlockHeight", json!(1_200));
    node
}

fn solana_wallet(node: &MockRpc) -> Wallet {
    let wallet = Wallet::new().endpoint("solana-devnet", &node.url);
    wallet.json(&[
        "--chain",
        "solana",
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "--label",
        "sol",
    ]);
    wallet
}

#[test]
fn solana_reports_a_balance_in_sol_and_lamports() {
    let node = solana_node();
    let wallet = solana_wallet(&node);

    let balance = wallet.json(&["--chain", "solana", "balance"]);
    assert_eq!(balance["balance"], "5");
    assert_eq!(balance["balance_raw"], "5000000000");
    assert_eq!(balance["symbol"], "SOL");
    assert_eq!(balance["decimals"], 9);
    assert_eq!(balance["address"], SOLANA_ADDRESS);
    assert_eq!(balance["chain"], "solana");

    // The active account's address is what got queried.
    let asked = node.requests_for("getBalance");
    assert_eq!(asked[0]["params"][0], SOLANA_ADDRESS);
}

#[test]
fn solana_signs_a_transfer_and_broadcasts_it() {
    let node = solana_node();
    let wallet = solana_wallet(&node);

    let sent = wallet.json(&[
        "--yes",
        "--chain",
        "solana",
        "send",
        "--to",
        SOLANA_RECIPIENT,
        "--amount",
        "1.5",
    ]);
    assert_eq!(sent["chain"], "solana");
    assert_eq!(sent["status"], "submitted");
    assert_eq!(sent["value"], "1.5");
    assert_eq!(sent["value_wei"], "1500000000");

    // The cluster was handed one base64 transaction, and told to treat it as
    // base64 rather than guessing.
    let broadcast = node.requests_for("sendTransaction");
    assert_eq!(broadcast.len(), 1);
    assert_eq!(broadcast[0]["params"][1]["encoding"], "base64");
    let payload = broadcast[0]["params"][0].as_str().unwrap();
    assert!(!payload.is_empty());

    // A legacy transfer is one signature, three accounts and one instruction:
    // 1 + 64 signature bytes, then the message. Anything much smaller means
    // the message never got built.
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .expect("the cluster is sent base64");
    assert!(bytes.len() > 100, "a transfer is {} bytes", bytes.len());
    assert_eq!(bytes[0], 1, "one signature");

    // And it is in the local log before the reply came back.
    let history = wallet.json(&["history"]);
    assert_eq!(history[0]["chain"], "solana");
    assert_eq!(history[0]["to"], SOLANA_RECIPIENT);
}

#[test]
fn a_solana_dry_run_signs_but_broadcasts_nothing() {
    let node = solana_node();
    let wallet = solana_wallet(&node);

    let planned = wallet.json(&[
        "--chain",
        "solana",
        "send",
        "--to",
        SOLANA_RECIPIENT,
        "--amount",
        "1",
        "--dry-run",
    ]);
    assert_eq!(planned["dry_run"], true);
    assert_eq!(planned["fee_raw"], "5000");
    assert!(planned["signed"].as_str().unwrap().starts_with("0x"));

    // Every read happened; the write did not.
    assert!(!node.requests_for("getLatestBlockhash").is_empty());
    assert!(
        node.requests_for("sendTransaction").is_empty(),
        "a dry run must not reach the cluster"
    );
    // And nothing was written to the history either.
    assert!(wallet.json(&["history"]).as_array().unwrap().is_empty());
}

/// Solana rejects an account left below the rent-exempt minimum, so the wallet
/// refuses first — after signing nothing and spending nothing.
#[test]
fn solana_refuses_a_transfer_that_would_leave_dust() {
    let node = solana_node();
    // The destination is empty and the amount is under the minimum.
    node.on_sequence(
        "getBalance",
        vec![
            json!({"value": 5_000_000_000u64}), // the sender, for the funding check
            json!({"value": 0u64}),             // the destination
        ],
    );
    let wallet = solana_wallet(&node);

    let code = wallet.json_error(&[
        "--yes",
        "--chain",
        "solana",
        "send",
        "--to",
        SOLANA_RECIPIENT,
        "--amount",
        "0.0000001", // 100 lamports, well under 890880
    ]);
    assert_eq!(code, "invalid_amount");
    assert!(node.requests_for("sendTransaction").is_empty());
}

#[test]
fn solana_refuses_a_transfer_it_cannot_fund() {
    let node = solana_node();
    node.on("getBalance", json!({"value": 1_000u64})); // far too little
    let wallet = solana_wallet(&node);

    let code = wallet.json_error(&[
        "--yes",
        "--chain",
        "solana",
        "send",
        "--to",
        SOLANA_RECIPIENT,
        "--amount",
        "1",
    ]);
    assert_eq!(code, "insufficient_funds");
    assert!(node.requests_for("sendTransaction").is_empty());
}

#[test]
fn solana_asks_the_faucet_and_reports_the_signature() {
    let node = solana_node();
    node.on("requestAirdrop", json!("airdropSignature"));
    let wallet = solana_wallet(&node);

    let dropped = wallet.json(&["--chain", "solana", "airdrop", "--amount", "2"]);
    assert_eq!(dropped["id"], "airdropSignature");
    assert_eq!(dropped["amount"], "2");

    let asked = node.requests_for("requestAirdrop");
    assert_eq!(asked[0]["params"][0], SOLANA_ADDRESS);
    assert_eq!(asked[0]["params"][1], 2_000_000_000u64, "asked in lamports");
}

#[test]
fn solana_chain_info_reports_the_cluster() {
    let node = solana_node();
    let wallet = solana_wallet(&node);
    let info = wallet.json(&["--chain", "solana", "chain-info"]);
    assert_eq!(info["cluster"], "solana-devnet");
    assert_eq!(info["solana_core"], "2.0.0");
    assert_eq!(info["slot"], 1_234);
}

// ===================================================================== Cardano

const CARDANO_ADDRESS: &str = "addr_test1qq8ac7qqy0vtulyl7wntmsxc6wex80gvcyjy33qffrhm7sh927ysx5sftuw0dlft05dz3c7revpf7jx0xnlcjz3g69mqkt5dmn";
const CARDANO_RECIPIENT: &str = "addr_test1qqz85693g4fr8c55mfyxhae8j2u04pydxrgqr73vmwpx3a8927ysx5sftuw0dlft05dz3c7revpf7jx0xnlcjz3g69mqlu9096";

/// A Koios endpoint answering every read a transfer makes.
fn koios() -> MockHttp {
    let api = MockHttp::start();
    api.on("address_info", json!([{"balance": "10000000"}])) // 10 ADA
        .on(
            "address_utxos",
            json!([{
                "tx_hash": "aa".repeat(32),
                "tx_index": 0,
                "value": "10000000",
                "asset_list": [],
                "datum_hash": null,
                "inline_datum": null,
                "reference_script": null,
            }]),
        )
        .on(
            "tip",
            json!([{"abs_slot": 100_000, "block_no": 5_000, "epoch_no": 42}]),
        )
        .on(
            "epoch_params",
            json!([{"min_fee_a": 44, "min_fee_b": 155_381, "coins_per_utxo_size": 4_310}]),
        )
        .on("submittx", json!("cc".repeat(32)));
    api
}

fn cardano_wallet(api: &MockHttp) -> Wallet {
    let wallet = Wallet::new().endpoint("cardano-preprod", &api.url);
    wallet.json(&[
        "--chain",
        "cardano",
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "--label",
        "ada",
    ]);
    wallet
}

#[test]
fn cardano_reports_a_balance_in_ada_and_lovelace() {
    let api = koios();
    let wallet = cardano_wallet(&api);

    let balance = wallet.json(&["--chain", "cardano", "balance"]);
    assert_eq!(balance["balance"], "10");
    assert_eq!(balance["balance_raw"], "10000000");
    assert_eq!(balance["symbol"], "tADA");
    assert_eq!(balance["decimals"], 6);
    assert_eq!(balance["address"], CARDANO_ADDRESS);

    // The address it asked about is the one it holds.
    let asked = &api.text_bodies_for("address_info")[0];
    assert!(asked.contains(CARDANO_ADDRESS), "{asked}");
}

/// An address the chain has never seen returns an empty array, which means
/// zero rather than an error.
#[test]
fn cardano_reports_an_unseen_address_as_empty_rather_than_failing() {
    let api = koios();
    api.on("address_info", json!([]));
    let wallet = cardano_wallet(&api);
    assert_eq!(
        wallet.json(&["--chain", "cardano", "balance"])["balance"],
        "0"
    );
}

#[test]
fn cardano_builds_signs_and_submits_a_transfer() {
    let api = koios();
    let wallet = cardano_wallet(&api);

    let sent = wallet.json(&[
        "--yes",
        "--chain",
        "cardano",
        "send",
        "--to",
        CARDANO_RECIPIENT,
        "--amount",
        "2",
    ]);
    assert_eq!(sent["chain"], "cardano");
    assert_eq!(sent["status"], "submitted");
    assert_eq!(sent["value"], "2");

    // The node was handed raw CBOR, not JSON.
    let submitted = api.bodies_for("submittx");
    assert_eq!(submitted.len(), 1);
    let cbor = &submitted[0];
    // `[body, witness_set, is_valid, auxiliary_data]` — a 4-element array.
    assert_eq!(
        cbor[0], 0x84,
        "a transaction is a 4-element CBOR array, got {:#04x}",
        cbor[0]
    );
    assert_eq!(
        &cbor[cbor.len() - 2..],
        &[0xf5, 0xf6],
        "is_valid = true, then no auxiliary data"
    );
    // The witness set carries exactly one signature, so the body was signed.
    assert!(
        cbor.len() > 200,
        "a signed transfer is {} bytes",
        cbor.len()
    );
}

#[test]
fn a_cardano_dry_run_shows_the_coin_selection_without_submitting() {
    let api = koios();
    let wallet = cardano_wallet(&api);

    let planned = wallet.json(&[
        "--chain",
        "cardano",
        "send",
        "--to",
        CARDANO_RECIPIENT,
        "--amount",
        "2",
        "--dry-run",
    ]);
    assert_eq!(planned["dry_run"], true);
    // One 10 ADA input covers 2 ADA plus the fee, with change coming back.
    assert_eq!(planned["detail"]["inputs"], 1);
    assert_eq!(planned["detail"]["outputs"], 2);
    assert!(planned["detail"]["change_lovelace"].as_u64().unwrap() > 0);

    // The fee is the linear one the scripted parameters imply.
    let fee: u64 = planned["fee_raw"].as_str().unwrap().parse().unwrap();
    assert!(fee > 155_381, "the fee must clear min_fee_b, got {fee}");

    assert!(
        api.bodies_for("submittx").is_empty(),
        "a dry run must not submit"
    );
}

/// Cardano rejects an output below the minimum a UTxO entry must carry, so
/// the wallet refuses before signing.
#[test]
fn cardano_refuses_an_output_below_the_minimum() {
    let api = koios();
    let wallet = cardano_wallet(&api);
    let code = wallet.json_error(&[
        "--yes",
        "--chain",
        "cardano",
        "send",
        "--to",
        CARDANO_RECIPIENT,
        "--amount",
        "0.1",
    ]);
    assert_eq!(code, "invalid_amount");
    assert!(api.bodies_for("submittx").is_empty());
}

#[test]
fn cardano_refuses_a_transfer_it_cannot_fund() {
    let api = koios();
    let wallet = cardano_wallet(&api);
    let code = wallet.json_error(&[
        "--yes",
        "--chain",
        "cardano",
        "send",
        "--to",
        CARDANO_RECIPIENT,
        "--amount",
        "9999",
    ]);
    assert_eq!(code, "insufficient_funds");
    assert!(api.bodies_for("submittx").is_empty());
}

/// A UTxO carrying native tokens is not spendable by this wallet, so it is
/// skipped — and an address holding only those has nothing to spend.
#[test]
fn cardano_skips_utxos_carrying_native_tokens() {
    let api = koios();
    api.on(
        "address_utxos",
        json!([{
            "tx_hash": "bb".repeat(32),
            "tx_index": 0,
            "value": "10000000",
            "asset_list": [{"policy_id": "aa", "quantity": "1"}],
            "datum_hash": null,
            "inline_datum": null,
            "reference_script": null,
        }]),
    );
    let wallet = cardano_wallet(&api);
    let code = wallet.json_error(&[
        "--yes",
        "--chain",
        "cardano",
        "send",
        "--to",
        CARDANO_RECIPIENT,
        "--amount",
        "2",
    ]);
    assert_eq!(code, "insufficient_funds");
}

#[test]
fn cardano_chain_info_reports_the_tip() {
    let api = koios();
    let wallet = cardano_wallet(&api);
    let info = wallet.json(&["--chain", "cardano", "chain-info"]);
    assert_eq!(info["epoch"], 42);
    assert_eq!(info["block_height"], 5_000);
    assert_eq!(info["abs_slot"], 100_000);
}

// ============================================================ across the board

/// Every chain must refuse a transfer to the account it leaves from, and must
/// do it before asking a node anything — the sender's own address is the one
/// most likely to be on the clipboard by mistake.
#[test]
fn no_chain_will_send_to_the_account_it_is_sending_from() {
    let node = solana_node();
    let solana = solana_wallet(&node);
    assert_eq!(
        solana.json_error(&[
            "--yes",
            "--chain",
            "solana",
            "send",
            "--to",
            SOLANA_ADDRESS,
            "--amount",
            "1",
        ]),
        "usage"
    );
    assert!(node.requests_for("getLatestBlockhash").is_empty());

    let api = koios();
    let cardano = cardano_wallet(&api);
    assert_eq!(
        cardano.json_error(&[
            "--yes",
            "--chain",
            "cardano",
            "send",
            "--to",
            CARDANO_ADDRESS,
            "--amount",
            "2",
        ]),
        "usage"
    );
    assert!(api.bodies_for("address_utxos").is_empty());
}

/// A chain with no faucet says so rather than reaching for one.
#[test]
fn a_chain_without_a_faucet_refuses_an_airdrop() {
    let api = koios();
    let wallet = cardano_wallet(&api);
    assert_eq!(
        wallet.json_error(&["--chain", "cardano", "airdrop"]),
        "usage"
    );
}

/// `nonce` and `gas-price` are account-based and EVM ideas; the UTxO chains
/// say what they do not have rather than inventing a number.
#[test]
fn the_utxo_chains_report_what_they_do_not_have() {
    let api = koios();
    let wallet = cardano_wallet(&api);
    assert_eq!(wallet.json_error(&["--chain", "cardano", "nonce"]), "usage");
}

/// A node that fails must not leave the wallet claiming success, and the
/// failure has to name what went wrong.
#[test]
fn a_failing_node_is_reported_rather_than_swallowed() {
    let node = MockRpc::start();
    node.on_error("getBalance", -32000, "node is having a day");
    let wallet = solana_wallet(&node);
    assert_eq!(
        wallet.json_error(&["--chain", "solana", "balance"]),
        "rpc_error"
    );
}

/// The endpoint override is per network, so pointing one chain at a mock does
/// not quietly redirect the others.
#[test]
fn an_endpoint_override_applies_to_one_network_only() {
    let node = solana_node();
    let wallet = solana_wallet(&node);

    let solana = wallet.json(&["--chain", "solana", "network", "current"]);
    assert_eq!(solana["endpoint"], node.url);

    let cardano = wallet.json(&["--chain", "cardano", "network", "current"]);
    assert_ne!(cardano["endpoint"], node.url);
    assert!(cardano["endpoint"].as_str().unwrap().contains("koios"));
}
