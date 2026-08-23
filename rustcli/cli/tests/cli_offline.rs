//! End-to-end tests for everything that needs no network access.

mod common;

use common::*;
use predicates::prelude::*;
use serde_json::Value;

#[test]
fn a_fresh_wallet_reports_no_accounts() {
    let wallet = Wallet::new();
    let accounts = wallet.json(&["account", "list"]);
    assert_eq!(accounts.as_array().unwrap().len(), 0);
    assert_eq!(wallet.json(&["info"])["accounts"], 0);
    assert_eq!(wallet.json_error(&["account", "show"]), "no_active_account");
}

#[test]
fn creates_the_home_directory_on_first_use() {
    let wallet = Wallet::new();
    let home = wallet.home.path().join("nested");
    wallet
        .cmd(&["--home", home.to_str().unwrap(), "info"])
        .assert()
        .success();
    assert!(home.join("config.jsonl").exists() || home.is_dir());
}

#[test]
fn account_new_generates_a_usable_wallet() {
    let wallet = Wallet::new();
    let account = wallet.json(&["account", "new", "--label", "alpha", "--words", "24"]);
    assert_eq!(account["label"], "alpha");
    assert_eq!(account["source"], "mnemonic");
    assert_eq!(account["derivation_path"], "m/44'/60'/0'/0/0");
    let address = account["address"].as_str().unwrap();
    assert!(address.starts_with("0x") && address.len() == 42);

    // The mnemonic is stored but not printed unless asked for.
    assert!(account.get("mnemonic").is_none());
    let exported = wallet.json(&["account", "export", "alpha"]);
    assert_eq!(
        exported["mnemonic"].as_str().unwrap().split(' ').count(),
        24
    );
}

#[test]
fn account_new_can_reveal_its_mnemonic() {
    let wallet = Wallet::new();
    let account = wallet.json(&["account", "new", "--show-secret"]);
    assert_eq!(account["mnemonic"].as_str().unwrap().split(' ').count(), 12);
    assert!(account["private_key"].as_str().unwrap().starts_with("0x"));
}

#[test]
fn importing_the_reference_mnemonic_derives_the_reference_address() {
    let wallet = Wallet::new();
    let account = wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "--label",
        "main",
    ]);
    assert_eq!(account["address"], TEST_ADDRESS_0);
    assert_eq!(account["index"], 0);
}

#[test]
fn importing_at_an_index_derives_that_address() {
    let wallet = Wallet::new();
    let account = wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "--index",
        "1",
        "--label",
        "second",
    ]);
    assert_eq!(account["address"], TEST_ADDRESS_1);
    assert_eq!(account["index"], 1);
}

#[test]
fn a_passphrase_changes_the_derived_address() {
    let wallet = Wallet::new();
    let plain = wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "plain",
    ]);
    let salted = wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "salted",
        "--passphrase",
        "extra",
    ]);
    assert_ne!(plain["address"], salted["address"]);
}

#[test]
fn importing_a_private_key_yields_the_matching_address() {
    let wallet = Wallet::new();
    let account = wallet.json(&["account", "import-key", "-k", TEST_PRIVATE_KEY, "-l", "raw"]);
    assert_eq!(account["address"], TEST_ADDRESS_0);
    assert_eq!(account["source"], "private_key");
    assert!(account["derivation_path"].is_null());
}

#[test]
fn secrets_can_arrive_on_stdin() {
    let wallet = Wallet::new();
    let output = wallet
        .cmd(&["--json", "account", "import-mnemonic", "-m", "-"])
        .write_stdin(TEST_MNEMONIC)
        .output()
        .unwrap();
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["data"]["address"], TEST_ADDRESS_0);
}

#[test]
fn secrets_can_arrive_through_the_environment() {
    let wallet = Wallet::new();
    let output = wallet
        .cmd(&["--json", "account", "import-key"])
        .env("CAUSEWAYBAY_PRIVATE_KEY", TEST_PRIVATE_KEY)
        .output()
        .unwrap();
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["data"]["address"], TEST_ADDRESS_0);
}

#[test]
fn bad_key_material_is_rejected_with_a_specific_code() {
    let wallet = Wallet::new();
    assert_eq!(
        wallet.json_error(&["account", "import-mnemonic", "-m", "clearly not a mnemonic"]),
        "invalid_mnemonic"
    );
    assert_eq!(
        wallet.json_error(&["account", "import-key", "-k", "0xdeadbeef"]),
        "invalid_private_key"
    );
}

#[test]
fn labels_must_be_unique() {
    let wallet = Wallet::new();
    wallet.json(&["account", "new", "-l", "dup"]);
    assert_eq!(
        wallet.json_error(&["account", "new", "-l", "dup"]),
        "duplicate_label"
    );
    assert_eq!(
        wallet.json_error(&["account", "new", "-l", "DUP"]),
        "duplicate_label"
    );
}

#[test]
fn labels_are_auto_assigned_when_omitted() {
    let wallet = Wallet::new();
    assert_eq!(wallet.json(&["account", "new"])["label"], "account-1");
    assert_eq!(wallet.json(&["account", "new"])["label"], "account-2");
}

#[test]
fn the_first_account_becomes_active_automatically() {
    let wallet = Wallet::new();
    wallet.json(&["account", "new", "-l", "first"]);
    wallet.json(&["account", "new", "-l", "second"]);
    assert_eq!(wallet.json(&["account", "show"])["label"], "first");

    wallet.json(&["account", "use", "second"]);
    assert_eq!(wallet.json(&["account", "show"])["label"], "second");
    assert_eq!(wallet.json(&["info"])["active_account"], "second");
}

#[test]
fn account_list_marks_the_active_account() {
    let wallet = Wallet::new();
    wallet.json(&["account", "new", "-l", "one"]);
    wallet.json(&["account", "new", "-l", "two"]);
    wallet.json(&["account", "use", "two"]);

    let accounts = wallet.json(&["account", "list"]);
    let active: Vec<_> = accounts
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["active"] == true)
        .map(|a| a["label"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(active, ["two"]);
}

#[test]
fn accounts_resolve_by_id_label_and_address() {
    let wallet = Wallet::new();
    let account = wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);
    let id = account["id"].as_str().unwrap();
    for selector in [id, "main", TEST_ADDRESS_0, &TEST_ADDRESS_0.to_lowercase()] {
        assert_eq!(
            wallet.json(&["account", "show", selector])["address"],
            TEST_ADDRESS_0
        );
    }
}

#[test]
fn derive_creates_sibling_addresses_from_one_mnemonic() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);
    let derived = wallet.json(&["account", "derive", "--index", "1", "-l", "second"]);
    assert_eq!(derived["address"], TEST_ADDRESS_1);
    assert_eq!(derived["derivation_path"], "m/44'/60'/0'/0/1");
    assert_eq!(
        wallet.json(&["account", "list"]).as_array().unwrap().len(),
        2
    );
}

#[test]
fn derive_refuses_when_there_is_no_mnemonic() {
    let wallet = Wallet::new();
    wallet.json(&["account", "import-key", "-k", TEST_PRIVATE_KEY, "-l", "raw"]);
    assert_eq!(
        wallet.json_error(&["account", "derive", "--index", "1"]),
        "usage"
    );
}

#[test]
fn rename_and_remove_keep_the_wallet_consistent() {
    let wallet = Wallet::new();
    wallet.json(&["account", "new", "-l", "old"]);
    wallet.json(&["account", "new", "-l", "keeper"]);

    wallet.json(&["account", "rename", "old", "renamed"]);
    assert_eq!(
        wallet.json_error(&["account", "show", "old"]),
        "account_not_found"
    );
    assert_eq!(
        wallet.json(&["account", "show", "renamed"])["label"],
        "renamed"
    );

    wallet.json(&["--yes", "account", "remove", "renamed"]);
    assert_eq!(
        wallet.json(&["account", "list"]).as_array().unwrap().len(),
        1
    );
    assert_eq!(wallet.json(&["account", "show"])["label"], "keeper");
}

#[test]
fn removal_needs_confirmation() {
    let wallet = Wallet::new();
    wallet.json(&["account", "new", "-l", "safe"]);
    // Without --yes there is nobody to prompt in JSON mode, so it must refuse.
    assert_eq!(
        wallet.json_error(&["account", "remove", "safe"]),
        "confirmation_required"
    );
    assert_eq!(
        wallet.json(&["account", "list"]).as_array().unwrap().len(),
        1
    );
}

#[test]
fn show_hides_secrets_unless_asked() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);

    let hidden = wallet.json(&["account", "show", "main"]);
    assert!(hidden.get("private_key").is_none());
    assert!(hidden.get("mnemonic").is_none());
    // The public key is safe to expose and useful for verification.
    assert!(hidden["public_key"].as_str().unwrap().starts_with("0x"));

    let shown = wallet.json(&["account", "show", "main", "--secret"]);
    assert_eq!(shown["private_key"], TEST_PRIVATE_KEY);
    assert_eq!(shown["mnemonic"], TEST_MNEMONIC);
}

#[test]
fn human_output_truncates_the_private_key() {
    let wallet = Wallet::new();
    wallet.json(&["account", "import-key", "-k", TEST_PRIVATE_KEY, "-l", "raw"]);
    wallet
        .cmd(&["account", "show", "raw"])
        .assert()
        .success()
        .stdout(predicate::str::contains(TEST_PRIVATE_KEY).not());
}

#[test]
fn networks_can_be_listed_and_switched() {
    let wallet = Wallet::new();
    let networks = wallet.json(&["network", "list"]);
    let keys: Vec<_> = networks
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["cronos-testnet", "cronos-mainnet"]);

    assert_eq!(wallet.json(&["network", "current"])["chain_id"], 338);
    wallet.json(&["network", "use", "mainnet"]);
    assert_eq!(wallet.json(&["network", "current"])["chain_id"], 25);
    assert_eq!(wallet.json(&["network", "current"])["symbol"], "CRO");
}

#[test]
fn an_unknown_network_is_rejected() {
    let wallet = Wallet::new();
    assert_eq!(
        wallet.json_error(&["network", "use", "ethereum"]),
        "unknown_network"
    );
    assert_eq!(
        wallet.json_error(&["-n", "solana", "info"]),
        "unknown_network"
    );
}

#[test]
fn the_network_flag_overrides_only_the_current_invocation() {
    let wallet = Wallet::new();
    assert_eq!(
        wallet.json(&["-n", "mainnet", "network", "current"])["chain_id"],
        25
    );
    // The stored default is untouched.
    assert_eq!(wallet.json(&["network", "current"])["chain_id"], 338);
}

#[test]
fn rpc_urls_can_be_overridden_and_restored() {
    let wallet = Wallet::new();
    wallet.json(&["network", "set-rpc", "testnet", "http://localhost:8545"]);
    assert_eq!(
        wallet.json(&["network", "current"])["rpc"],
        "http://localhost:8545"
    );
    wallet.json(&["network", "set-rpc", "testnet", ""]);
    assert_eq!(
        wallet.json(&["network", "current"])["rpc"],
        "https://evm-t3.cronos.org"
    );
}

#[test]
fn signing_and_verification_round_trip() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);

    let signed = wallet.json(&["sign", "hello causewaybay"]);
    let signature = signed["signature"].as_str().unwrap();
    assert_eq!(signature.len(), 2 + 130);
    assert_eq!(signed["address"], TEST_ADDRESS_0);

    let verified = wallet.json(&[
        "verify",
        "--message",
        "hello causewaybay",
        "--signature",
        signature,
        "--address",
        TEST_ADDRESS_0,
    ]);
    assert_eq!(verified["valid"], true);
    assert_eq!(verified["recovered"], TEST_ADDRESS_0);
}

#[test]
fn verification_fails_for_the_wrong_signer_or_message() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);
    let signature = wallet.json(&["sign", "original"])["signature"]
        .as_str()
        .unwrap()
        .to_string();

    let wrong_signer = wallet.json(&[
        "verify",
        "--message",
        "original",
        "--signature",
        &signature,
        "--address",
        TEST_ADDRESS_1,
    ]);
    assert_eq!(wrong_signer["valid"], false);

    let tampered = wallet.json(&[
        "verify",
        "--message",
        "tampered",
        "--signature",
        &signature,
        "--address",
        TEST_ADDRESS_0,
    ]);
    assert_eq!(tampered["valid"], false);
}

#[test]
fn a_message_can_be_signed_from_stdin() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);
    let output = wallet
        .cmd(&["--json", "sign", "-"])
        .write_stdin("piped message\n")
        .output()
        .unwrap();
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    // The trailing newline a shell adds is stripped, so the message is exact.
    assert_eq!(envelope["data"]["message"], "piped message");
}

#[test]
fn offline_utilities_produce_known_values() {
    let wallet = Wallet::new();
    assert_eq!(
        wallet.json(&["utils", "keccak", "hello"])["keccak256"],
        "0x1c8aff950685c2ed4bc3174f3472287b56d9517b9c948127319a09a7a36deac8"
    );
    assert_eq!(
        wallet.json(&["utils", "checksum", &TEST_ADDRESS_0.to_lowercase()])["address"],
        TEST_ADDRESS_0
    );
    assert_eq!(
        wallet.json(&["utils", "to-wei", "1.5"])["value"],
        "1500000000000000000"
    );
    assert_eq!(
        wallet.json(&["utils", "from-wei", "1500000000000000000"])["amount"],
        "1.5"
    );
    assert_eq!(
        wallet.json(&["utils", "to-wei", "1.5", "--decimals", "6"])["value"],
        "1500000"
    );
}

#[test]
fn utils_reject_malformed_amounts_and_addresses() {
    let wallet = Wallet::new();
    assert_eq!(
        wallet.json_error(&["utils", "to-wei", "1.2.3"]),
        "invalid_amount"
    );
    assert_eq!(
        wallet.json_error(&["utils", "to-wei", "-5"]),
        "invalid_amount"
    );
    // 19 decimal places do not fit in an 18-decimal token.
    assert_eq!(
        wallet.json_error(&["utils", "to-wei", "0.1234567890123456789"]),
        "invalid_amount"
    );
    assert_eq!(
        wallet.json_error(&["utils", "checksum", "0x123"]),
        "invalid_address"
    );
}

#[test]
fn utils_can_mint_a_mnemonic_without_storing_it() {
    let wallet = Wallet::new();
    let generated = wallet.json(&["utils", "new-mnemonic", "--words", "24"]);
    assert_eq!(
        generated["mnemonic"].as_str().unwrap().split(' ').count(),
        24
    );
    assert_eq!(
        wallet.json(&["account", "list"]).as_array().unwrap().len(),
        0
    );
}

#[test]
fn history_starts_empty() {
    let wallet = Wallet::new();
    assert_eq!(wallet.json(&["history"]).as_array().unwrap().len(), 0);
}

#[test]
fn the_store_is_append_only_and_well_formed() {
    let wallet = Wallet::new();
    let account = wallet.json(&["account", "new", "-l", "one"]);
    let id = account["id"].as_str().unwrap().to_string();
    wallet.json(&["account", "rename", "one", "two"]);
    wallet.json(&["--yes", "account", "remove", "two"]);

    let lines = wallet.read_log("accounts.jsonl");
    let types: Vec<_> = lines.iter().map(|l| l["type"].as_str().unwrap()).collect();
    assert_eq!(
        types,
        ["account.create", "account.rename", "account.delete"]
    );
    for line in &lines {
        assert_eq!(line["schema"], 1);
        assert_eq!(line["id"], id);
    }
}

#[test]
fn a_corrupt_line_does_not_break_the_wallet() {
    use std::io::Write;
    let wallet = Wallet::new();
    wallet.json(&["account", "new", "-l", "good"]);

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(wallet.home.path().join("accounts.jsonl"))
        .unwrap();
    writeln!(file, "{{ this is not json").unwrap();
    drop(file);

    wallet.json(&["account", "new", "-l", "also-good"]);
    let labels: Vec<_> = wallet
        .json(&["account", "list"])
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["label"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(labels, ["good", "also-good"]);
}

#[test]
fn two_homes_stay_independent() {
    let first = Wallet::new();
    let second = Wallet::new();
    first.json(&["account", "new", "-l", "only-here"]);
    assert_eq!(
        second.json(&["account", "list"]).as_array().unwrap().len(),
        0
    );
}

#[test]
fn json_mode_writes_exactly_one_line_to_stdout() {
    let wallet = Wallet::new();
    let output = wallet.cmd(&["--json", "info"]).output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.trim().lines().count(),
        1,
        "stdout must be one JSON line"
    );
    assert!(serde_json::from_str::<Value>(stdout.trim()).is_ok());
}

#[test]
fn human_mode_prints_the_warning_on_stderr_only() {
    let wallet = Wallet::new();
    let output = wallet.cmd(&["info"]).output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Educational wallet"));
    assert!(
        !stdout.contains("Educational wallet"),
        "stdout stays parseable"
    );
}

#[test]
fn exit_codes_distinguish_usage_from_runtime_errors() {
    let wallet = Wallet::new();
    wallet.cmd(&["info"]).assert().success();
    // A missing account is a runtime error: exit 1.
    wallet.cmd(&["account", "show", "ghost"]).assert().code(1);
    // An unparsable command line is a usage error: exit 2.
    wallet.cmd(&["not-a-command"]).assert().code(2);
    wallet
        .cmd(&["send", "--to", TEST_ADDRESS_1])
        .assert()
        .code(2);
}

#[test]
fn help_and_version_succeed() {
    let wallet = Wallet::new();
    wallet
        .cmd(&["--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Cronos"));
    wallet
        .cmd(&["--version"])
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
    wallet
        .cmd(&["account", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("import-mnemonic"));
}

#[cfg(unix)]
#[test]
fn stored_files_are_not_world_readable() {
    use std::os::unix::fs::PermissionsExt;
    let wallet = Wallet::new();
    wallet.json(&["account", "new", "-l", "secret-holder"]);
    let mode = std::fs::metadata(wallet.home.path().join("accounts.jsonl"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0, "no group or other access to key material");
}

// ------------------------------------------------------- the recall list

#[test]
fn recall_starts_empty_and_says_so() {
    let wallet = Wallet::new();
    assert_eq!(
        wallet.json(&["recent", "list"]).as_array().unwrap().len(),
        0
    );
    wallet
        .cmd(&["recent", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Nothing remembered yet"));
    assert_eq!(wallet.json_error(&["recent", "show"]), "not_found");
}

#[test]
fn creating_and_importing_accounts_fills_the_recall_list() {
    let wallet = Wallet::new();
    wallet.json(&["account", "new", "-l", "generated"]);
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "imported",
    ]);
    wallet.json(&["account", "import-key", "-k", TEST_PRIVATE_KEY, "-l", "raw"]);

    let entries = wallet.json(&["recent", "list"]);
    let entries = entries.as_array().unwrap();
    assert_eq!(entries.len(), 3);
    let kinds: Vec<_> = entries
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.iter().filter(|k| **k == "mnemonic").count(), 2);
    assert_eq!(kinds.iter().filter(|k| **k == "private_key").count(), 1);
    // Positions are 1-based and match the display order.
    assert_eq!(entries[0]["position"], 1);
    assert_eq!(wallet.json(&["info"])["remembered"], 3);
}

#[test]
fn recall_hides_secrets_until_asked() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);

    let listed = wallet.json(&["recent", "list"]);
    assert!(listed[0].get("secret").is_none());
    assert_eq!(listed[0]["preview"], "abandon … about");

    let shown = wallet.json(&["recent", "show", "1"]);
    assert!(shown.get("secret").is_none());

    let revealed = wallet.json(&["recent", "show", "1", "--secret"]);
    assert_eq!(revealed["secret"], TEST_MNEMONIC);
    assert_eq!(revealed["word_count"], 12);
}

#[test]
fn human_recall_output_never_prints_a_whole_secret_by_accident() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);
    wallet
        .cmd(&["recent", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains(TEST_MNEMONIC).not());
    wallet
        .cmd(&["recent", "show", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains(TEST_MNEMONIC).not());
    // Only the explicit --secret form reveals it.
    wallet
        .cmd(&["recent", "show", "1", "--secret"])
        .assert()
        .success()
        .stdout(predicate::str::contains(TEST_MNEMONIC));
}

#[test]
fn re_using_key_material_bumps_the_counter_instead_of_duplicating() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "one",
    ]);
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-i",
        "1",
        "-l",
        "two",
    ]);

    let entries = wallet.json(&["recent", "list"]);
    assert_eq!(entries.as_array().unwrap().len(), 1);
    assert_eq!(entries[0]["uses"], 2);
}

#[test]
fn deriving_moves_the_parent_mnemonic_to_the_front() {
    let wallet = Wallet::new();
    // The first account stays active, so name the parent explicitly.
    wallet.json(&["account", "new", "-l", "other"]);
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);
    wallet.json(&[
        "account", "derive", "-i", "1", "--from", "main", "-l", "second",
    ]);

    let entries = wallet.json(&["recent", "list"]);
    assert_eq!(entries[0]["address"], TEST_ADDRESS_0);
    assert_eq!(entries[0]["uses"], 2, "the parent mnemonic was used twice");
}

#[test]
fn derive_without_a_parent_uses_the_active_account() {
    let wallet = Wallet::new();
    let first = wallet.json(&["account", "new", "-l", "active-one"]);
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "not-active",
    ]);
    // Importing does not steal activation, so this derives from `active-one`.
    wallet.json(&["account", "derive", "-i", "1", "-l", "child"]);

    let entries = wallet.json(&["recent", "list"]);
    assert_eq!(entries[0]["address"], first["address"]);
    assert_eq!(entries[0]["uses"], 2);
}

#[test]
fn recall_can_be_filtered_and_limited() {
    let wallet = Wallet::new();
    wallet.json(&["account", "new", "-l", "generated"]);
    wallet.json(&["account", "import-key", "-k", TEST_PRIVATE_KEY, "-l", "raw"]);

    assert_eq!(
        wallet
            .json(&["recent", "list", "--kind", "mnemonic"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        wallet
            .json(&["recent", "list", "--kind", "private-key"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        wallet
            .json(&["recent", "list", "--limit", "1"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn import_recent_rebuilds_an_account_without_retyping_anything() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "original",
    ]);
    wallet.json(&["--yes", "account", "remove", "original"]);
    assert_eq!(
        wallet.json(&["account", "list"]).as_array().unwrap().len(),
        0
    );
    // Removing the account leaves the material in the recall list.
    assert_eq!(
        wallet.json(&["recent", "list"]).as_array().unwrap().len(),
        1
    );

    let restored = wallet.json(&["account", "import-recent", "1", "-l", "restored"]);
    assert_eq!(restored["address"], TEST_ADDRESS_0);
    assert_eq!(restored["source"], "mnemonic");
}

#[test]
fn import_recent_defaults_to_the_newest_entry() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "older",
    ]);
    // A separate seed, so it becomes a distinct — and newer — recall entry.
    let newest = wallet.json(&["account", "new", "--new-seed", "-l", "newer"]);
    let restored = wallet.json(&["account", "import-recent", "-l", "copy"]);
    assert_eq!(restored["address"], newest["address"]);
}

#[test]
fn import_recent_can_pick_another_address_index() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);
    let derived = wallet.json(&["account", "import-recent", "1", "-i", "1", "-l", "second"]);
    assert_eq!(derived["address"], TEST_ADDRESS_1);
}

#[test]
fn import_recent_restores_a_private_key_entry() {
    let wallet = Wallet::new();
    wallet.json(&["account", "import-key", "-k", TEST_PRIVATE_KEY, "-l", "raw"]);
    wallet.json(&["--yes", "account", "remove", "raw"]);
    let restored = wallet.json(&["account", "import-recent", "1", "-l", "back"]);
    assert_eq!(restored["address"], TEST_ADDRESS_0);
    assert_eq!(restored["source"], "private_key");
}

#[test]
fn import_recent_reports_an_empty_or_bad_selector() {
    let wallet = Wallet::new();
    assert_eq!(
        wallet.json_error(&["account", "import-recent"]),
        "not_found"
    );
    wallet.json(&["account", "new", "-l", "one"]);
    assert_eq!(
        wallet.json_error(&["account", "import-recent", "9"]),
        "not_found"
    );
    assert_eq!(
        wallet.json_error(&["account", "import-recent", "nope"]),
        "not_found"
    );
}

#[test]
fn forgetting_needs_confirmation_and_then_removes_the_entry() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);
    assert_eq!(
        wallet.json_error(&["recent", "forget", "1"]),
        "confirmation_required"
    );
    assert_eq!(
        wallet.json(&["recent", "list"]).as_array().unwrap().len(),
        1
    );

    wallet.json(&["--yes", "recent", "forget", "1"]);
    assert_eq!(
        wallet.json(&["recent", "list"]).as_array().unwrap().len(),
        0
    );
    // The account itself is untouched.
    assert_eq!(
        wallet.json(&["account", "list"]).as_array().unwrap().len(),
        1
    );
}

#[test]
fn clearing_forgets_everything() {
    let wallet = Wallet::new();
    wallet.json(&["account", "new", "-l", "one"]);
    wallet.json(&["account", "import-key", "-k", TEST_PRIVATE_KEY, "-l", "two"]);
    assert_eq!(
        wallet.json_error(&["recent", "clear"]),
        "confirmation_required"
    );

    assert_eq!(wallet.json(&["--yes", "recent", "clear"])["forgotten"], 2);
    assert_eq!(
        wallet.json(&["recent", "list"]).as_array().unwrap().len(),
        0
    );
    // Clearing an empty list is a no-op that needs no confirmation.
    assert_eq!(wallet.json(&["recent", "clear"])["forgotten"], 0);
}

#[test]
fn the_recall_log_is_append_only_and_well_formed() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);
    wallet.json(&["--yes", "recent", "forget", "1"]);

    let lines = wallet.read_log("recent.jsonl");
    let types: Vec<_> = lines.iter().map(|l| l["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["secret.remember", "secret.forget"]);
    for line in &lines {
        assert_eq!(line["schema"], 1);
        assert!(line["id"].as_str().unwrap().starts_with("sec_"));
    }
}

#[cfg(unix)]
#[test]
fn the_recall_log_is_not_world_readable() {
    use std::os::unix::fs::PermissionsExt;
    let wallet = Wallet::new();
    wallet.json(&["account", "new", "-l", "one"]);
    let mode = std::fs::metadata(wallet.home.path().join("recent.jsonl"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o077,
        0,
        "remembered secrets must not be world readable"
    );
}

// ------------------------------------------------ saving the wallet list

#[test]
fn the_wallet_list_renders_in_every_format() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);
    wallet.json(&["account", "derive", "-i", "1", "-l", "second"]);

    for format in ["jsonl", "csv", "txt", "md"] {
        let result = wallet.json(&["account", "list", "--format", format]);
        assert_eq!(result["format"], format);
        assert_eq!(result["count"], 2);
        assert!(result["path"].is_null(), "no --output means stdout");
        let content = result["content"].as_str().unwrap();
        assert!(
            content.contains(TEST_ADDRESS_0),
            "{format} should list the address"
        );
        assert!(
            content.contains(TEST_ADDRESS_1),
            "{format} should list both wallets"
        );
        assert!(
            !content.contains(TEST_PRIVATE_KEY),
            "{format} must not leak secrets"
        );
    }
}

#[test]
fn each_format_has_the_shape_its_readers_expect() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);

    let jsonl = wallet.json(&["account", "list", "--format", "jsonl"]);
    let line = jsonl["content"].as_str().unwrap().lines().next().unwrap();
    let parsed: Value = serde_json::from_str(line).expect("each line is a JSON object");
    assert_eq!(parsed["address"], TEST_ADDRESS_0);
    assert_eq!(parsed["active"], true);

    let csv = wallet.json(&["account", "list", "--format", "csv"]);
    let mut lines = csv["content"].as_str().unwrap().lines();
    assert_eq!(
        lines.next().unwrap(),
        "position,label,address,source,address_index,seed,derivation_path,created_at,\
         active,public_key_compressed,public_key"
            .replace(char::is_whitespace, "")
    );
    assert_eq!(lines.next().unwrap().split(',').count(), 11);

    let markdown = wallet.json(&["account", "list", "--format", "md"]);
    let mut md_lines = markdown["content"].as_str().unwrap().lines();
    assert!(md_lines.next().unwrap().starts_with("| position | label |"));
    assert!(md_lines.next().unwrap().starts_with("| --- |"));

    let txt = wallet.json(&["account", "list", "--format", "txt"]);
    let mut txt_lines = txt["content"].as_str().unwrap().lines();
    assert!(txt_lines.next().unwrap().starts_with("position"));
    assert!(txt_lines.next().unwrap().starts_with("--------"));
}

#[test]
fn the_wallet_list_can_be_written_to_a_file() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);
    let target = wallet.home.path().join("wallets.csv");

    let result = wallet.json(&[
        "account",
        "list",
        "--format",
        "csv",
        "-o",
        target.to_str().unwrap(),
    ]);
    assert_eq!(result["count"], 1);
    assert_eq!(result["path"], target.to_str().unwrap());
    assert!(result.get("content").is_none(), "the file is the output");

    let written = std::fs::read_to_string(&target).unwrap();
    assert!(written.starts_with("position,label,address,"));
    assert!(written.contains(TEST_ADDRESS_0));
}

#[test]
fn saving_omits_secrets_unless_asked() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);

    let plain = wallet.json(&["account", "list", "--format", "csv"]);
    let plain = plain["content"].as_str().unwrap();
    assert!(!plain.contains("private_key"));
    assert!(!plain.contains(TEST_PRIVATE_KEY));
    // Public keys are not secrets, so they are there either way.
    assert!(plain.contains("public_key_compressed"));

    let revealed = wallet.json(&["account", "list", "--format", "csv", "--secret"]);
    let revealed = revealed["content"].as_str().unwrap();
    assert!(revealed
        .lines()
        .next()
        .unwrap()
        .ends_with("public_key_compressed,public_key,private_key,mnemonic"));
    assert!(revealed.contains(TEST_PRIVATE_KEY));
    assert!(revealed.contains(TEST_MNEMONIC));
}

#[cfg(unix)]
#[test]
fn a_saved_file_holding_secrets_is_not_world_readable() {
    use std::os::unix::fs::PermissionsExt;
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);
    let target = wallet.home.path().join("secrets.csv");
    wallet.json(&[
        "account",
        "list",
        "--format",
        "csv",
        "--secret",
        "-o",
        target.to_str().unwrap(),
    ]);
    let mode = std::fs::metadata(&target).unwrap().permissions().mode();
    assert_eq!(
        mode & 0o077,
        0,
        "an export holding keys must stay owner-only"
    );
}

#[test]
fn an_unknown_format_is_a_usage_error() {
    let wallet = Wallet::new();
    wallet
        .cmd(&["account", "list", "--format", "yaml"])
        .assert()
        .code(2);
}

#[test]
fn saving_an_empty_wallet_still_produces_headers() {
    let wallet = Wallet::new();
    let csv = wallet.json(&["account", "list", "--format", "csv"]);
    assert_eq!(csv["count"], 0);
    assert_eq!(csv["content"].as_str().unwrap().lines().count(), 1);
    // JSONL of nothing is genuinely nothing.
    assert_eq!(
        wallet.json(&["account", "list", "--format", "jsonl"])["content"],
        ""
    );
}

#[test]
fn plain_account_list_is_unchanged_without_a_format() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);
    let listed = wallet.json(&["account", "list"]);
    assert!(
        listed.is_array(),
        "the default output is still the array of accounts"
    );
    assert_eq!(listed[0]["label"], "main");
}

#[test]
fn the_address_index_is_per_mnemonic_and_the_position_is_per_list() {
    let wallet = Wallet::new();
    // Two addresses from one phrase, then a wallet with a phrase of its own.
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "first",
    ]);
    wallet.json(&[
        "account", "derive", "-i", "1", "--from", "first", "-l", "second",
    ]);
    wallet.json(&["account", "new", "--new-seed", "-l", "separate"]);

    let listed = wallet.json(&["account", "list", "--format", "jsonl"]);
    let rows: Vec<Value> = listed["content"]
        .as_str()
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    // The list position counts 1, 2, 3 — what a reader expects of a list.
    assert_eq!(rows[0]["position"], 1);
    assert_eq!(rows[1]["position"], 2);
    assert_eq!(rows[2]["position"], 3);

    // The address index is BIP-44's, scoped to one mnemonic, so it restarts.
    assert_eq!(rows[0]["address_index"], 0);
    assert_eq!(rows[1]["address_index"], 1);
    assert_eq!(
        rows[2]["address_index"], 0,
        "a new phrase starts again at 0"
    );

    // And the seed column shows why: the first two share one, the third does not.
    assert_eq!(rows[0]["seed"], rows[1]["seed"]);
    assert_ne!(rows[0]["seed"], rows[2]["seed"]);
}

#[test]
fn the_export_seed_lines_up_with_the_recall_list() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);

    let listed = wallet.json(&["account", "list", "--format", "jsonl"]);
    let row: Value =
        serde_json::from_str(listed["content"].as_str().unwrap().lines().next().unwrap()).unwrap();
    let remembered = wallet.json(&["recent", "list"]);

    assert_eq!(
        row["seed"], remembered[0]["id"],
        "the seed id is the recall id, so the two views can be matched up"
    );
}

// ------------------------------------------------ one seed, many addresses

#[test]
fn new_addresses_continue_the_sequence_on_one_seed() {
    let wallet = Wallet::new();
    // A wallet holds one mnemonic; each new address walks it: 0, 1, 2, 3, …
    let indexes: Vec<u64> = (0..5)
        .map(|n| {
            let account = wallet.json(&["account", "new", "-l", &format!("addr-{n}")]);
            account["index"].as_u64().unwrap()
        })
        .collect();
    assert_eq!(indexes, [0, 1, 2, 3, 4], "address indexes must increase");

    // And they all came from the same phrase.
    let listed = wallet.json(&["account", "list", "--format", "jsonl"]);
    let seeds: Vec<String> = listed["content"]
        .as_str()
        .unwrap()
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["seed"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        seeds.iter().collect::<std::collections::HashSet<_>>().len(),
        1
    );
    // One mnemonic in the recall list, used five times.
    let remembered = wallet.json(&["recent", "list"]);
    assert_eq!(remembered.as_array().unwrap().len(), 1);
    assert_eq!(remembered[0]["uses"], 5);
}

#[test]
fn the_first_wallet_mints_a_seed_and_the_rest_reuse_it() {
    let wallet = Wallet::new();
    let first = wallet.json(&["account", "new", "-l", "first"]);
    assert_eq!(first["new_seed"], true, "an empty wallet has to mint one");
    assert_eq!(first["index"], 0);

    let second = wallet.json(&["account", "new", "-l", "second"]);
    assert_eq!(second["new_seed"], false, "the seed already exists");
    assert_eq!(second["index"], 1);
}

#[test]
fn new_addresses_continue_an_imported_mnemonic() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "imported",
    ]);

    let next = wallet.json(&["account", "new", "-l", "next"]);
    assert_eq!(next["new_seed"], false);
    assert_eq!(next["index"], 1);
    // Index 1 of the reference phrase is a known address.
    assert_eq!(next["address"], TEST_ADDRESS_1);
}

#[test]
fn new_seed_starts_its_own_sequence() {
    let wallet = Wallet::new();
    wallet.json(&["account", "new", "-l", "a0"]);
    wallet.json(&["account", "new", "-l", "a1"]);

    let fresh = wallet.json(&["account", "new", "--new-seed", "-l", "b0"]);
    assert_eq!(fresh["new_seed"], true);
    assert_eq!(fresh["index"], 0, "a separate seed starts again at 0");

    // The next plain `new` continues whichever seed is active.
    wallet.json(&["account", "use", "b0"]);
    assert_eq!(wallet.json(&["account", "new", "-l", "b1"])["index"], 1);
    wallet.json(&["account", "use", "a0"]);
    assert_eq!(wallet.json(&["account", "new", "-l", "a2"])["index"], 2);
}

#[test]
fn an_explicit_index_overrides_the_sequence() {
    let wallet = Wallet::new();
    wallet.json(&["account", "new", "-l", "zero"]);
    assert_eq!(
        wallet.json(&["account", "new", "-i", "9", "-l", "nine"])["index"],
        9
    );
    // The next free index is now 10, not 1.
    assert_eq!(wallet.json(&["account", "new", "-l", "ten"])["index"], 10);
}

#[test]
fn a_private_key_wallet_does_not_block_new_addresses() {
    let wallet = Wallet::new();
    wallet.json(&["account", "import-key", "-k", TEST_PRIVATE_KEY, "-l", "raw"]);
    // The only account has no mnemonic, so a seed has to be minted.
    let first = wallet.json(&["account", "new", "-l", "derived"]);
    assert_eq!(first["new_seed"], true);
    assert_eq!(first["index"], 0);
    // And now the sequence continues normally.
    assert_eq!(wallet.json(&["account", "new", "-l", "next"])["index"], 1);
}

#[test]
fn the_full_export_carries_the_address_and_all_three_keys() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);

    let exported = wallet.json(&["account", "list", "--format", "jsonl", "--secret"]);
    let row: Value = serde_json::from_str(
        exported["content"]
            .as_str()
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();

    assert_eq!(row["address"], TEST_ADDRESS_0);
    assert_eq!(row["private_key"], TEST_PRIVATE_KEY);

    // 33 bytes compressed, 64 bytes uncompressed with the SEC1 tag dropped.
    let compressed = row["public_key_compressed"].as_str().unwrap();
    let uncompressed = row["public_key"].as_str().unwrap();
    assert_eq!(compressed.len(), 2 + 66);
    assert_eq!(uncompressed.len(), 2 + 128);
    assert!(compressed.starts_with("0x02") || compressed.starts_with("0x03"));
    assert_eq!(&compressed[4..], &uncompressed[2..66], "same X coordinate");

    // The same values `account show` reports, so the two agree.
    let shown = wallet.json(&["account", "show", "main"]);
    assert_eq!(shown["public_key"], row["public_key"]);
    assert_eq!(shown["public_key_compressed"], row["public_key_compressed"]);
}

#[test]
fn the_network_table_drives_what_can_be_selected() {
    let wallet = Wallet::new();
    let networks = wallet.json(&["network", "list"]);
    let names: Vec<&str> = networks
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Cronos EVM Testnet", "Cronos EVM Mainnet"]);

    // Every listed network can actually be switched to by key.
    for entry in networks.as_array().unwrap() {
        let key = entry["key"].as_str().unwrap();
        let switched = wallet.json(&["network", "use", key]);
        assert_eq!(switched["key"], key);
        assert_eq!(wallet.json(&["network", "current"])["key"], key);
    }
}

#[test]
fn every_table_format_carries_the_public_keys() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "main",
    ]);

    // The compressed key for the reference wallet, as `account show` reports it.
    let expected = wallet.json(&["account", "show", "main"])["public_key_compressed"]
        .as_str()
        .unwrap()
        .to_string();

    for format in ["jsonl", "csv", "txt", "md"] {
        let rendered = wallet.json(&["account", "list", "--format", format]);
        let content = rendered["content"].as_str().unwrap();
        assert!(
            content.contains("public_key_compressed"),
            "{format} is missing the column"
        );
        assert!(
            content.contains(&expected),
            "{format} is missing the key itself"
        );
        // And still no private key without --secret.
        assert!(
            !content.contains(TEST_PRIVATE_KEY),
            "{format} leaks the private key"
        );
    }
}

// ------------------------------------------- fixes for the review findings

/// A phrase remembered with a passphrase names a wallet the phrase alone
/// cannot reach. Restoring it without the passphrase used to hand back a
/// different, unfunded address without saying so.
#[test]
fn a_passphrase_protected_recall_entry_cannot_be_restored_without_it() {
    let wallet = Wallet::new();
    let salted = wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "salted",
        "--passphrase",
        "hunter2",
    ]);
    let salted_address = salted["address"].as_str().unwrap().to_string();
    assert_ne!(
        salted_address, TEST_ADDRESS_0,
        "a passphrase moves the wallet"
    );

    // The recall entry names the wallet the user actually has.
    let remembered = wallet.json(&["recent", "list"]);
    assert_eq!(remembered[0]["address"], salted_address);
    assert_eq!(remembered[0]["has_passphrase"], true);

    // Restoring without it refuses instead of silently creating another wallet.
    assert_eq!(
        wallet.json_error(&["account", "import-recent", "1", "-l", "restored"]),
        "usage"
    );
    // A wrong passphrase is caught by comparing the derived address.
    assert_eq!(
        wallet.json_error(&[
            "account",
            "import-recent",
            "1",
            "-l",
            "restored",
            "--passphrase",
            "wrong",
        ]),
        "usage"
    );
    // With the right one, the original wallet comes back.
    let restored = wallet.json(&[
        "account",
        "import-recent",
        "1",
        "-l",
        "restored",
        "--passphrase",
        "hunter2",
    ]);
    assert_eq!(restored["address"], salted_address);
}

#[test]
fn a_plain_recall_entry_still_restores_without_a_passphrase() {
    let wallet = Wallet::new();
    wallet.json(&[
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "plain",
    ]);
    assert_eq!(wallet.json(&["recent", "list"])[0]["has_passphrase"], false);
    assert_eq!(
        wallet.json(&["account", "import-recent", "1", "-l", "copy"])["address"],
        TEST_ADDRESS_0
    );
}

/// `recent forget 0` used to delete the newest entry: positions are 1-based, and
/// `saturating_sub(1)` turned the invalid 0 into a valid index.
#[test]
fn recall_positions_are_strictly_one_based() {
    let wallet = Wallet::new();
    wallet.json(&["account", "new", "-l", "one"]);
    // A surrounding space is trimmed, as it is for every other selector; what
    // must not resolve is a position that is not a plain 1-based number.
    for bad in ["0", "+1", "1.0", "01x"] {
        assert_eq!(
            wallet.json_error(&["recent", "show", bad]),
            "not_found",
            "{bad:?} must not resolve to an entry"
        );
    }
    // A leading dash is taken as a flag and rejected before it ever reaches the
    // selector — still refused, just as a usage error rather than not_found.
    wallet.cmd(&["recent", "show", "-1"]).assert().code(2);
    // The entry is still there: nothing was silently acted on.
    assert_eq!(
        wallet.json(&["recent", "list"]).as_array().unwrap().len(),
        1
    );
    assert!(wallet.json(&["recent", "show", "1"]).get("id").is_some());
}

#[test]
fn a_home_path_starting_with_a_tilde_is_expanded() {
    // Without expansion this creates a directory literally named `~`.
    let wallet = Wallet::new();
    let output = wallet
        .cmd(&[
            "--json",
            "--home",
            "~/definitely-not-a-real-wallet-dir",
            "info",
        ])
        .output()
        .unwrap();
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    let home = envelope["data"]["home"].as_str().unwrap();
    assert!(
        !home.starts_with('~'),
        "the tilde should have been expanded: {home}"
    );
    // Clean up whatever it created.
    let _ = std::fs::remove_dir_all(home);
}

// ============================================================ crypto utilities
//
// `utils derive`, `utils sign` and `utils validate-mnemonic` are the wallet
// used as a calculator: they take key material as an argument and store
// nothing. That last part is the property worth asserting, because it is the
// one that could silently stop being true.

#[test]
fn utils_derive_reproduces_the_reference_addresses() {
    let wallet = Wallet::new();
    let derived = wallet.json(&["utils", "derive", "-m", TEST_MNEMONIC]);
    assert_eq!(derived["address"], TEST_ADDRESS_0);
    assert_eq!(derived["private_key"], TEST_PRIVATE_KEY);
    assert_eq!(derived["derivation_path"], "m/44'/60'/0'/0/0");
    assert_eq!(derived["source"], "mnemonic");
    assert_eq!(derived["index"], 0);

    let second = wallet.json(&["utils", "derive", "-m", TEST_MNEMONIC, "-i", "1"]);
    assert_eq!(second["address"], TEST_ADDRESS_1);
}

#[test]
fn utils_derive_stores_nothing() {
    // The whole point of it: derivation without acquiring an account, and
    // without the phrase turning up in the recall list afterwards.
    let wallet = Wallet::new();
    wallet.json(&["utils", "derive", "-m", TEST_MNEMONIC]);
    wallet.json(&["utils", "derive", "-k", TEST_PRIVATE_KEY]);

    assert_eq!(
        wallet.json(&["account", "list"]).as_array().unwrap().len(),
        0
    );
    assert_eq!(
        wallet.json(&["recent", "list"]).as_array().unwrap().len(),
        0
    );
    assert_eq!(wallet.json(&["info"])["accounts"], 0);
    assert_eq!(wallet.json(&["info"])["remembered"], 0);
}

#[test]
fn utils_derive_from_a_private_key_has_no_path() {
    let wallet = Wallet::new();
    let derived = wallet.json(&["utils", "derive", "-k", TEST_PRIVATE_KEY]);
    assert_eq!(derived["address"], TEST_ADDRESS_0);
    assert_eq!(derived["source"], "private_key");
    assert!(derived.get("derivation_path").is_none());
    // Both public key encodings, and the compressed one shares the X coordinate.
    let full = derived["public_key"].as_str().unwrap();
    let compressed = derived["public_key_compressed"].as_str().unwrap();
    assert_eq!(full.len(), 2 + 128);
    assert_eq!(compressed.len(), 2 + 66);
    assert_eq!(&compressed[4..], &full[2..66]);
}

#[test]
fn utils_derive_needs_exactly_one_source() {
    let wallet = Wallet::new();
    wallet.cmd(&["utils", "derive"]).assert().failure();
    wallet
        .cmd(&[
            "utils",
            "derive",
            "-m",
            TEST_MNEMONIC,
            "-k",
            TEST_PRIVATE_KEY,
        ])
        .assert()
        .failure();
}

#[test]
fn utils_derive_rejects_bad_material_with_the_usual_codes() {
    let wallet = Wallet::new();
    assert_eq!(
        wallet.json_error(&["utils", "derive", "-m", "not a real phrase"]),
        "invalid_mnemonic"
    );
    assert_eq!(
        wallet.json_error(&["utils", "derive", "-k", "0x1234"]),
        "invalid_private_key"
    );
}

#[test]
fn utils_sign_matches_the_reference_signature() {
    // The published EIP-191 vector for this key and message.
    let wallet = Wallet::new();
    let signed = wallet.json(&[
        "utils",
        "sign",
        "-k",
        "0x4646464646464646464646464646464646464646464646464646464646464646",
        "-m",
        "Hello World",
    ]);
    assert_eq!(
        signed["address"],
        "0x9d8A62f656a8d1615C1294fd71e9CFb3E4855A4F"
    );
    assert_eq!(
        signed["signature"],
        "0xf445005436439a4398409aee0e0b13702bdee4e3774b6aa67184f0732d3a270a1ef3802a2455afba1374fb2ad23345e89eb7366c9d567fe0e5338df934434e3b1c"
    );
    assert_eq!(
        wallet.json(&["account", "list"]).as_array().unwrap().len(),
        0
    );
}

#[test]
fn utils_sign_round_trips_through_verify() {
    let wallet = Wallet::new();
    let signed = wallet.json(&["utils", "sign", "-k", TEST_PRIVATE_KEY, "-m", "hello"]);
    let signature = signed["signature"].as_str().unwrap();
    let checked = wallet.json(&[
        "verify",
        "--message",
        "hello",
        "--signature",
        signature,
        "--address",
        TEST_ADDRESS_0,
    ]);
    assert_eq!(checked["valid"], true);
}

#[test]
fn utils_validate_mnemonic_reports_rather_than_refuses() {
    // The difference from `account import-mnemonic`, which is an error path.
    let wallet = Wallet::new();
    let good = wallet.json(&["utils", "validate-mnemonic", TEST_MNEMONIC]);
    assert_eq!(good["valid"], true);
    assert_eq!(good["words"], 12);
    assert!(good["reason"].is_null());

    for (phrase, words) in [
        ("abandon abandon", 2),
        (
            "abandon abandon abandon abandon abandon abandon abandon abandon \
             abandon abandon abandon abandon",
            12,
        ),
    ] {
        let bad = wallet.json(&["utils", "validate-mnemonic", phrase]);
        assert_eq!(bad["valid"], false, "{phrase}");
        assert_eq!(bad["words"], words);
        assert!(!bad["reason"].as_str().unwrap().is_empty());
    }
}
