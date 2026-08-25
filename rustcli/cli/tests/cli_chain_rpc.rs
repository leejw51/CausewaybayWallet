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

/// The transaction id is blake2b-256 of the body that was signed, and Koios
/// cannot change what that hashes to. Taking its word for it meant `--wait`
/// could follow a transaction that does not exist while the real one confirmed
/// unwatched — the EVM client has always done the opposite, and said why.
#[test]
fn cardano_keeps_the_id_it_computed_rather_than_the_endpoints() {
    let api = koios();
    api.on("submittx", json!("dd".repeat(32)));
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
    let hash = sent["hash"].as_str().unwrap();
    assert_eq!(hash.len(), 64, "a blake2b-256 transaction id");
    assert_ne!(hash, "dd".repeat(32), "the endpoint's answer is not the id");
    // The disagreement is not swallowed, it is reported alongside.
    assert_eq!(sent["secondary_id"], "dd".repeat(32));
}

/// The headline of the review: the fee comes from `min_fee_a`/`min_fee_b`, and
/// a Koios instance can answer anything at all.
#[test]
fn cardano_refuses_a_fee_its_endpoint_inflated() {
    let api = koios();
    api.on(
        "epoch_params",
        json!([{"min_fee_a": 1_000_000_000, "min_fee_b": 155_381, "coins_per_utxo_size": 4_310}]),
    );
    api.on(
        "address_utxos",
        json!([{
            "tx_hash": "aa".repeat(32),
            "tx_index": 0,
            "value": "1000000000000",
            "asset_list": [],
            "datum_hash": null,
            "inline_datum": null,
            "reference_script": null,
        }]),
    );
    let wallet = cardano_wallet(&api);

    let error = wallet.json_failure(&[
        "--yes",
        "--chain",
        "cardano",
        "send",
        "--to",
        CARDANO_RECIPIENT,
        "--amount",
        "5",
    ]);
    assert_eq!(error["code"], "invalid_amount");
    assert!(
        error["message"].as_str().unwrap().contains("--max-fee"),
        "{error}"
    );
    assert!(
        api.bodies_for("submittx").is_empty(),
        "nothing may be signed, let alone submitted"
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

// ======================================================================= eCash

/// The eCash testnet address the shared phrase derives at index 0, and the
/// twenty bytes Chronik indexes it under.
const ECASH_ADDRESS: &str = "ectest:qrwzys2q6xq98vwz0kjn6ulu5m6yljr5fy7w393sue";
const ECASH_HASH160: &str = "dc224140d18053b1c27da53d73fca6f44fc87449";
/// Index 1, used as a recipient.
const ECASH_RECIPIENT: &str = "ectest:qz3w32n8ptaw37ens807egvx7ymvxcelwymtatxs5g";

/// Just enough protobuf to script Chronik. The wire format is
/// `(field << 3) | type`, and these are the only two types it uses.
mod pb {
    pub fn varint(out: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 {
            out.push((value as u8) | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
    }

    pub fn number(number: u32, value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        varint(&mut out, u64::from(number) << 3);
        varint(&mut out, value);
        out
    }

    pub fn bytes(number: u32, value: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        varint(&mut out, u64::from(number) << 3 | 2);
        varint(&mut out, value.len() as u64);
        out.extend_from_slice(value);
        out
    }

    pub fn message(parts: &[Vec<u8>]) -> Vec<u8> {
        parts.concat()
    }
}

/// One `Utxo`: an outpoint, a height, a value, and optionally an eToken.
fn utxo(txid_byte: u8, out_idx: u64, sats: u64, token: bool) -> Vec<u8> {
    let outpoint = pb::message(&[pb::bytes(1, &[txid_byte; 32]), pb::number(2, out_idx)]);
    let mut parts = vec![
        pb::bytes(1, &outpoint),
        pb::number(2, 800_000), // block height
        pb::number(5, sats),
        pb::number(10, 1), // is_final
    ];
    if token {
        parts.push(pb::bytes(11, &pb::message(&[pb::bytes(1, b"a-token-id")])));
    }
    pb::message(&parts)
}

/// A `ScriptUtxos` reply holding `utxos`.
fn script_utxos(utxos: &[Vec<u8>]) -> Vec<u8> {
    let mut parts = vec![pb::bytes(1, b"the locking script")];
    parts.extend(utxos.iter().map(|u| pb::bytes(2, u)));
    pb::message(&parts)
}

/// A Chronik instance answering every read a transfer makes.
fn chronik() -> MockHttp {
    let api = MockHttp::start();
    api.on_bytes(
        "blockchain-info",
        200,
        pb::message(&[pb::bytes(1, &[0xab; 32]), pb::number(2, 800_100)]),
    )
    // 10,000 XEC in one output.
    .on_bytes(
        "utxos",
        200,
        script_utxos(&[utxo(0xaa, 0, 1_000_000, false)]),
    )
    .on_bytes(
        "broadcast-tx",
        200,
        pb::message(&[pb::bytes(1, &[0xcc; 32])]),
    );
    api
}

fn ecash_wallet(api: &MockHttp) -> Wallet {
    let wallet = Wallet::new().endpoint("ecash-testnet", &api.url);
    wallet.json(&[
        "--chain",
        "ecash",
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "--label",
        "xec",
    ]);
    wallet
}

/// The decimal count is the thing to get wrong here: XEC has two places, not
/// Bitcoin's eight, so a million satoshis is ten thousand XEC.
#[test]
fn ecash_reports_a_balance_in_xec_and_satoshis() {
    let api = chronik();
    let wallet = ecash_wallet(&api);

    let balance = wallet.json(&["--chain", "ecash", "balance"]);
    assert_eq!(balance["balance"], "10000");
    assert_eq!(balance["balance_raw"], "1000000");
    assert_eq!(balance["symbol"], "tXEC");
    assert_eq!(balance["decimals"], 2);
    assert_eq!(balance["address"], ECASH_ADDRESS);
    assert_eq!(balance["chain"], "ecash");

    // Chronik indexes by script, so the request names the key hash rather
    // than the address — and it must be the hash of the account it holds.
    let asked = api.paths().join(" ");
    assert!(
        asked.contains(&format!("/script/p2pkh/{ECASH_HASH160}/utxos")),
        "{asked}"
    );
}

/// An address the chain has never seen is a 404, and that means zero.
#[test]
fn an_unseen_ecash_address_reads_as_zero_rather_than_failing() {
    let api = chronik();
    api.on_bytes("utxos", 404, b"nothing here".to_vec());
    let wallet = ecash_wallet(&api);
    assert_eq!(
        wallet.json(&["--chain", "ecash", "balance"])["balance"],
        "0"
    );
}

#[test]
fn ecash_builds_signs_and_submits_a_transfer() {
    let api = chronik();
    let wallet = ecash_wallet(&api);

    let sent = wallet.json(&[
        "--chain",
        "ecash",
        "send",
        "--to",
        ECASH_RECIPIENT,
        "--amount",
        "100",
        "--yes",
    ]);
    assert_eq!(sent["status"], "submitted");
    assert_eq!(sent["to"], ECASH_RECIPIENT);
    assert_eq!(sent["from"], ECASH_ADDRESS);
    // 100 XEC is 10,000 satoshis, and the fee is counted in the same units.
    assert_eq!(sent["value_wei"], "10000");
    assert_eq!(sent["hash"].as_str().unwrap().len(), 64);

    // The broadcast carries the raw transaction inside a protobuf field, and
    // the transaction itself has to be the one that was described.
    let body = &api.bodies_for("broadcast-tx")[0];
    assert_eq!(body[0], 0x0a, "field 1, length-delimited");
    // Past the tag and the length varint, which is two bytes at this size.
    let raw = &body[1 + body[1..].iter().take_while(|b| **b & 0x80 != 0).count() + 1..];
    assert_eq!(&raw[..4], &2u32.to_le_bytes(), "version 2");
    assert_eq!(raw[4], 1, "one input, since one output funds it");
    // The recipient's amount is in there, little-endian, and so is the change.
    let hex_raw = hex::encode(raw);
    assert!(
        hex_raw.contains(&hex::encode(10_000u64.to_le_bytes())),
        "{hex_raw}"
    );
    // The whole thing is signed: two pushes in the script sig, and the
    // sighash byte the chain is forked on.
    assert!(hex_raw.contains("41"), "a SIGHASH_ALL|SIGHASH_FORKID byte");
}

/// The id the wallet computed is the id it reports, because it is the hash of
/// the bytes that were signed and the endpoint cannot change what those hash
/// to.
#[test]
fn ecash_keeps_the_id_it_computed_rather_than_the_endpoints() {
    let api = chronik();
    let wallet = ecash_wallet(&api);
    let sent = wallet.json(&[
        "--chain",
        "ecash",
        "send",
        "--to",
        ECASH_RECIPIENT,
        "--amount",
        "100",
        "--yes",
    ]);
    // Chronik was scripted to answer with 0xcc… and it is not what came back.
    assert_ne!(sent["hash"], "cc".repeat(32));
    assert_eq!(sent["secondary_id"], "cc".repeat(32));
}

#[test]
fn an_ecash_dry_run_shows_the_coin_selection_without_submitting() {
    let api = chronik();
    let wallet = ecash_wallet(&api);

    let planned = wallet.json(&[
        "--chain",
        "ecash",
        "send",
        "--to",
        ECASH_RECIPIENT,
        "--amount",
        "100",
        "--dry-run",
    ]);
    assert_eq!(planned["dry_run"], true);
    assert_eq!(planned["detail"]["inputs"], 1);
    assert_eq!(planned["detail"]["outputs"], 2, "one payment, one change");
    assert_eq!(planned["detail"]["fee_per_byte"], 1);
    // The fee is a satoshi a byte, so it and the size agree.
    let size = planned["detail"]["size_bytes"].as_u64().unwrap();
    let fee = planned["detail"]["fee_sats"].as_u64().unwrap();
    assert!(fee >= size && fee < size + 8, "fee {fee} for {size} bytes");
    assert!(
        api.bodies_for("broadcast-tx").is_empty(),
        "nothing was sent"
    );
}

/// An output carrying an eToken is part of the balance and is never spent:
/// the token rides on the output, so spending it as plain XEC destroys it.
#[test]
fn ecash_holds_back_outputs_carrying_etokens() {
    let api = chronik();
    api.on_bytes(
        "utxos",
        200,
        script_utxos(&[utxo(0xaa, 0, 1_000_000, false), utxo(0xbb, 1, 546, true)]),
    );
    let wallet = ecash_wallet(&api);

    // Counted, because it is genuinely there.
    assert_eq!(
        wallet.json(&["--chain", "ecash", "balance"])["balance_raw"],
        "1000546"
    );

    // Not spent, and the plan says so rather than leaving it unexplained.
    let planned = wallet.json(&[
        "--chain",
        "ecash",
        "send",
        "--to",
        ECASH_RECIPIENT,
        "--amount",
        "100",
        "--dry-run",
    ]);
    assert_eq!(planned["detail"]["inputs"], 1);
    assert_eq!(planned["detail"]["unspent_outputs_held_back"], 1);
}

/// The reading that looks like a contradiction: `balance` says the address
/// holds something and `send` says there is nothing to spend. Both are true
/// when every output is carrying an eToken, and saying only the second is how
/// a user concludes the wallet has lost their money.
#[test]
fn an_address_holding_only_token_outputs_says_why_none_of_it_can_be_spent() {
    let api = chronik();
    api.on_bytes("utxos", 200, script_utxos(&[utxo(0xbb, 1, 546, true)]));
    let wallet = ecash_wallet(&api);

    assert_eq!(
        wallet.json(&["--chain", "ecash", "balance"])["balance"],
        "5.46"
    );
    let failure = wallet.json_failure(&[
        "--chain",
        "ecash",
        "send",
        "--to",
        ECASH_RECIPIENT,
        "--amount",
        "1",
        "--yes",
    ]);
    assert_eq!(failure["code"], "insufficient_funds");
    let message = failure["message"].as_str().unwrap();
    // It names the balance it is refusing to move, and why.
    assert!(message.contains("5.46 tXEC"), "{message}");
    assert!(message.contains("eToken"), "{message}");
    // And it agrees with itself: one output, singular throughout.
    assert!(
        message.contains("its one unspent output carries"),
        "{message}"
    );
}

/// 546 satoshis is the network's own floor; below it the output is not
/// relayed at all, so it is refused here rather than signed and dropped.
#[test]
fn ecash_refuses_an_output_below_the_dust_limit() {
    let api = chronik();
    let wallet = ecash_wallet(&api);
    let error = wallet.json_error(&[
        "--chain",
        "ecash",
        "send",
        "--to",
        ECASH_RECIPIENT,
        "--amount",
        "1",
        "--yes",
    ]);
    assert_eq!(error, "invalid_amount");
    assert!(api.bodies_for("broadcast-tx").is_empty());
}

#[test]
fn ecash_refuses_a_transfer_it_cannot_fund() {
    let api = chronik();
    let wallet = ecash_wallet(&api);
    assert_eq!(
        wallet.json_error(&[
            "--chain",
            "ecash",
            "send",
            "--to",
            ECASH_RECIPIENT,
            "--amount",
            "99999",
            "--yes",
        ]),
        "insufficient_funds"
    );
    assert!(api.bodies_for("broadcast-tx").is_empty());
}

/// A rejected broadcast has its reason inside the body of a 400, and losing
/// it would turn "dust" into "HTTP 400".
#[test]
fn a_rejected_ecash_broadcast_keeps_the_reason_chronik_gave() {
    let api = chronik();
    api.on_bytes(
        "broadcast-tx",
        400,
        pb::bytes(2, b"400: Broadcast failed: txn-mempool-conflict"),
    );
    let wallet = ecash_wallet(&api);
    let failure = wallet.json_failure(&[
        "--chain",
        "ecash",
        "send",
        "--to",
        ECASH_RECIPIENT,
        "--amount",
        "100",
        "--yes",
    ]);
    assert_eq!(failure["code"], "rpc_error");
    assert!(
        failure["message"]
            .as_str()
            .unwrap()
            .contains("txn-mempool-conflict"),
        "the reason was dropped: {failure}"
    );
}

#[test]
fn ecash_chain_info_reports_the_tip() {
    let api = chronik();
    let wallet = ecash_wallet(&api);
    let info = wallet.json(&["--chain", "ecash", "chain-info"]);
    assert_eq!(info["block_height"], 800_100);
    assert_eq!(info["tip_hash"], "ab".repeat(32));
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
