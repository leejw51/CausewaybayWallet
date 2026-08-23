//! Tests against the shared vectors in `../testvectors`.
//!
//! These are the checks that tie this implementation to the outside world:
//! the official BIP-39 and EIP-55 vectors, the worked example from EIP-155, and
//! the mnemonics and keys that Anvil, Hardhat and Ganache print on startup. The
//! Python implementation runs the same files, so a disagreement between the two
//! shows up as a failure here rather than as a surprise on chain.
//!
//! Regenerate the files with `make vectors`.

use std::path::PathBuf;

use alloy_primitives::{keccak256, U256};
use serde_json::Value;

use causewaybay_core::error::Code;
use causewaybay_core::tx::LegacyTransaction;
use causewaybay_core::wallet::Keypair;
use causewaybay_core::{bip39, erc20, units, wallet};

/// Where the shared vectors live, relative to this crate.
///
/// One definition: the directory moved once already when the workspace split,
/// and the two copies that existed then did not move together.
fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("testvectors")
}

/// Load one vector file, failing with a hint rather than a bare panic.
fn load(name: &str) -> Value {
    let path: PathBuf = vectors_dir().join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}\nrun `make vectors` from the repository root",
            path.display()
        )
    });
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()))
}

fn array<'a>(value: &'a Value, key: &str) -> &'a Vec<Value> {
    value[key]
        .as_array()
        .unwrap_or_else(|| panic!("expected `{key}` to be an array"))
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("expected `{key}` to be a string in {value}"))
}

fn number(value: &Value, key: &str) -> u64 {
    value[key]
        .as_u64()
        .unwrap_or_else(|| panic!("expected `{key}` to be a number in {value}"))
}

/// Parse a decimal string into a U256 — the vectors carry big values as strings.
fn big(value: &Value, key: &str) -> U256 {
    U256::from_str_radix(text(value, key), 10)
        .unwrap_or_else(|_| panic!("`{key}` is not a decimal integer in {value}"))
}

// ================================================== coverage of the set

/// Every file in `testvectors/` must be read by this suite.
///
/// Without this, adding a vector file and wiring it into only one implementation
/// would look like success on both sides. The Python suite asserts the same
/// thing, so a new file has to be adopted by both before the build goes green.
#[test]
fn every_vector_file_is_consumed_by_this_suite() {
    const CONSUMED: [&str; 10] = [
        "bip39.json",
        "bip39-invalid.json",
        "derivation.json",
        "keys.json",
        "keys-invalid.json",
        "eip55.json",
        "keccak.json",
        "eip191.json",
        "transactions.json",
        "units.json",
    ];

    let dir = vectors_dir();
    let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot list {}: {e}", dir.display()))
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".json"))
        .collect();
    on_disk.sort();

    let mut expected: Vec<String> = CONSUMED.iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(
        on_disk, expected,
        "the vector directory and this suite have drifted apart"
    );

    // And each one must actually parse and carry its provenance marker.
    for name in CONSUMED {
        let file = load(name);
        assert!(
            file.get("$comment").is_some(),
            "{name} should record how it was generated"
        );
    }
}

// =========================================================== BIP-39

#[test]
fn bip39_matches_the_official_trezor_vectors() {
    let file = load("bip39.json");
    let vectors = array(&file, "vectors");
    assert_eq!(vectors.len(), 25, "the official English set has 25 vectors");

    for vector in vectors {
        let entropy = hex::decode(text(vector, "entropy")).unwrap();
        let phrase = text(vector, "mnemonic");

        assert_eq!(
            bip39::entropy_to_mnemonic(&entropy).unwrap(),
            phrase,
            "entropy {} should encode to this phrase",
            text(vector, "entropy")
        );
        assert_eq!(
            bip39::mnemonic_to_entropy(phrase).unwrap(),
            entropy,
            "phrase should decode back to entropy"
        );
        assert!(bip39::validate(phrase), "official vector must validate");
        assert_eq!(
            phrase.split(' ').count() as u64,
            number(vector, "word_count"),
            "word count"
        );
        assert_eq!(
            hex::encode(bip39::to_seed(phrase, "TREZOR")),
            text(vector, "seed_trezor"),
            "seed with the TREZOR passphrase"
        );
        assert_eq!(
            hex::encode(bip39::to_seed(phrase, "")),
            text(vector, "seed_empty_passphrase"),
            "seed with no passphrase"
        );
    }
}

#[test]
fn bip39_normalises_the_same_phrase_written_differently() {
    let file = load("bip39.json");
    for case in array(&file, "normalization") {
        let input = text(case, "input");
        let canonical = text(case, "canonical");
        assert!(
            bip39::validate(input),
            "{input:?} should validate after normalisation"
        );
        assert_eq!(bip39::normalize(input), canonical, "normalising {input:?}");
        assert_eq!(
            Keypair::from_mnemonic(input, 0, "").unwrap().address(),
            Keypair::from_mnemonic(canonical, 0, "").unwrap().address(),
            "{input:?} should name the same wallet as the canonical phrase"
        );
    }
}

#[test]
fn bip39_rejects_every_invalid_vector() {
    let file = load("bip39-invalid.json");
    for vector in array(&file, "vectors") {
        let phrase = text(vector, "mnemonic");
        let reason = text(vector, "reason");
        assert!(
            !bip39::validate(phrase),
            "{phrase:?} should be rejected ({reason})"
        );
        assert_eq!(
            Keypair::from_mnemonic(phrase, 0, "").unwrap_err().code,
            Code::InvalidMnemonic,
            "{phrase:?} should fail with invalid_mnemonic ({reason})"
        );
    }
}

// ========================================================== BIP-44

#[test]
fn derivation_matches_well_known_wallets() {
    let file = load("derivation.json");
    let mnemonics = array(&file, "mnemonics");
    assert!(
        mnemonics.len() >= 6,
        "expected the full set of known mnemonics"
    );

    for entry in mnemonics {
        let phrase = text(entry, "phrase");
        let name = text(entry, "name");

        for account in array(entry, "accounts") {
            let index = number(account, "index") as u32;
            let keypair = Keypair::from_mnemonic(phrase, index, "").unwrap();
            assert_eq!(
                keypair.address().to_checksum(None),
                text(account, "address"),
                "{name} index {index} address"
            );
            assert_eq!(
                keypair.private_key_hex(),
                text(account, "private_key"),
                "{name} index {index} private key"
            );
            assert_eq!(
                causewaybay_core::bip32::ethereum_path(index),
                text(account, "path"),
                "{name} index {index} derivation path"
            );
        }

        // A BIP-39 passphrase must yield an entirely different wallet.
        let salted = Keypair::from_mnemonic(phrase, 0, "TREZOR").unwrap();
        assert_eq!(
            salted.address().to_checksum(None),
            text(entry, "passphrase_trezor_index_0"),
            "{name} with the TREZOR passphrase"
        );
    }
}

#[test]
fn the_anvil_mnemonic_derives_the_addresses_anvil_prints() {
    let file = load("derivation.json");
    let anvil = array(&file, "mnemonics")
        .iter()
        .find(|entry| text(entry, "name") == "foundry-anvil-default")
        .expect("the Anvil mnemonic should be in the vectors");

    // Spelled out rather than only looped over: these are the addresses a
    // developer sees every time they start a local node.
    let keypair = Keypair::from_mnemonic(text(anvil, "phrase"), 0, "").unwrap();
    assert_eq!(
        keypair.address().to_checksum(None),
        "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
    );
    assert_eq!(
        keypair.private_key_hex(),
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
    );

    let second = Keypair::from_mnemonic(text(anvil, "phrase"), 1, "").unwrap();
    assert_eq!(
        second.address().to_checksum(None),
        "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
    );
}

// ====================================================== private keys

#[test]
fn known_private_keys_produce_their_published_addresses() {
    let file = load("keys.json");
    for entry in array(&file, "keys") {
        let name = text(entry, "name");
        let keypair = Keypair::from_hex(text(entry, "private_key")).unwrap();
        assert_eq!(
            keypair.address().to_checksum(None),
            text(entry, "address"),
            "{name} address"
        );
        assert_eq!(
            keypair.public_key_hex(),
            text(entry, "public_key"),
            "{name} public key"
        );
        assert_eq!(
            keypair.public_key_compressed_hex(),
            text(entry, "public_key_compressed"),
            "{name} compressed public key"
        );
    }
}

#[test]
fn private_keys_parse_with_or_without_the_prefix() {
    let file = load("keys.json");
    for entry in array(&file, "keys") {
        let with_prefix = text(entry, "private_key");
        let without = with_prefix.strip_prefix("0x").unwrap();
        assert_eq!(
            Keypair::from_hex(without).unwrap().address(),
            Keypair::from_hex(with_prefix).unwrap().address()
        );
        assert_eq!(
            Keypair::from_hex(&with_prefix.to_uppercase().replace("0X", "0x"))
                .unwrap()
                .address(),
            Keypair::from_hex(with_prefix).unwrap().address()
        );
    }
}

#[test]
fn invalid_private_keys_are_rejected() {
    let file = load("keys-invalid.json");
    for vector in array(&file, "vectors") {
        let key = text(vector, "private_key");
        let reason = text(vector, "reason");
        assert_eq!(
            Keypair::from_hex(key).unwrap_err().code,
            Code::InvalidPrivateKey,
            "{key} should be rejected ({reason})"
        );
    }
}

// ============================================================ EIP-55

#[test]
fn eip55_checksums_match_the_reference_addresses() {
    let file = load("eip55.json");
    let vectors = array(&file, "vectors");
    assert!(vectors.len() >= 9);

    for vector in vectors {
        let lower = text(vector, "lowercase");
        let checksummed = text(vector, "checksummed");
        assert_eq!(
            wallet::parse_address(lower).unwrap().to_checksum(None),
            checksummed,
            "checksum of {lower}"
        );
        // The checksummed form must survive a round trip unchanged.
        assert_eq!(
            wallet::parse_address(checksummed)
                .unwrap()
                .to_checksum(None),
            checksummed
        );
    }
}

// ============================================================ keccak

#[test]
fn keccak_matches_published_digests() {
    let file = load("keccak.json");
    for vector in array(&file, "hashes") {
        let input = text(vector, "text");
        assert_eq!(
            format!("0x{}", hex::encode(keccak256(input.as_bytes()))),
            text(vector, "keccak256"),
            "keccak256 of {input:?}"
        );
    }
}

#[test]
fn erc20_selectors_are_the_first_four_bytes_of_the_signature_hash() {
    let file = load("keccak.json");
    let known: Vec<(&str, [u8; 4])> = vec![
        ("transfer(address,uint256)", erc20::SELECTOR_TRANSFER),
        ("balanceOf(address)", erc20::SELECTOR_BALANCE_OF),
        ("decimals()", erc20::SELECTOR_DECIMALS),
        ("symbol()", erc20::SELECTOR_SYMBOL),
        ("name()", erc20::SELECTOR_NAME),
        ("totalSupply()", erc20::SELECTOR_TOTAL_SUPPLY),
        ("allowance(address,address)", erc20::SELECTOR_ALLOWANCE),
        ("approve(address,uint256)", erc20::SELECTOR_APPROVE),
    ];

    for vector in array(&file, "selectors") {
        let signature = text(vector, "signature");
        let expected = text(vector, "selector");
        // Computed from the signature…
        assert_eq!(
            format!("0x{}", hex::encode(&keccak256(signature.as_bytes())[..4])),
            expected,
            "computed selector for {signature}"
        );
        // …and matching the constant this crate ships.
        let (_, constant) = known
            .iter()
            .find(|(name, _)| *name == signature)
            .unwrap_or_else(|| panic!("no constant for {signature}"));
        assert_eq!(
            format!("0x{}", hex::encode(constant)),
            expected,
            "shipped constant for {signature}"
        );
    }
}

// =========================================================== EIP-191

#[test]
fn eip191_signatures_match_the_reference_signer() {
    let file = load("eip191.json");
    let keypair = Keypair::from_hex(text(&file, "signing_key")).unwrap();

    for vector in array(&file, "vectors") {
        let message = text(vector, "message");
        let expected_hash = text(vector, "prefixed_hash");
        let expected_signature = text(vector, "signature");
        let signer = text(vector, "signer");

        assert_eq!(
            format!("0x{}", hex::encode(wallet::eip191_hash(message.as_bytes()))),
            expected_hash,
            "prefixed hash of {message:?}"
        );
        // RFC-6979 makes this deterministic, so the bytes must match exactly.
        assert_eq!(
            format!(
                "0x{}",
                hex::encode(keypair.sign_message(message.as_bytes()).unwrap())
            ),
            expected_signature,
            "signature over {message:?}"
        );
        assert_eq!(
            wallet::recover_message(
                message.as_bytes(),
                &wallet::parse_hex(expected_signature).unwrap()
            )
            .unwrap()
            .to_checksum(None),
            signer,
            "recovered signer for {message:?}"
        );
    }
}

#[test]
fn eip191_recovery_rejects_a_tampered_message() {
    let file = load("eip191.json");
    let vector = &array(&file, "vectors")[1];
    let signature = wallet::parse_hex(text(vector, "signature")).unwrap();
    let recovered = wallet::recover_message(b"a different message", &signature).unwrap();
    assert_ne!(recovered.to_checksum(None), text(vector, "signer"));
}

// ====================================================== transactions

#[test]
fn signed_transactions_match_the_reference_signer() {
    let file = load("transactions.json");
    let vectors = array(&file, "vectors");
    assert!(vectors.len() >= 5);

    for vector in vectors {
        let name = text(vector, "name");
        let keypair = Keypair::from_hex(text(vector, "private_key")).unwrap();
        assert_eq!(
            keypair.address().to_checksum(None),
            text(vector, "signer"),
            "{name} signer"
        );

        let transaction = LegacyTransaction {
            nonce: number(vector, "nonce"),
            gas_price: big(vector, "gas_price"),
            gas_limit: number(vector, "gas_limit"),
            to: Some(wallet::parse_address(text(vector, "to")).unwrap()),
            value: big(vector, "value"),
            data: wallet::parse_hex(text(vector, "data")).unwrap(),
            chain_id: number(vector, "chain_id"),
        };
        let signed = transaction.sign(&keypair).unwrap();

        assert_eq!(
            signed.raw_hex(),
            text(vector, "raw"),
            "{name} raw transaction"
        );
        assert_eq!(
            signed.hash_hex(),
            text(vector, "hash"),
            "{name} transaction hash"
        );
        assert_eq!(signed.v, number(vector, "v"), "{name} v");
        assert_eq!(
            format!("0x{}", hex::encode(signed.r)),
            text(vector, "r"),
            "{name} r"
        );
        assert_eq!(
            format!("0x{}", hex::encode(signed.s)),
            text(vector, "s"),
            "{name} s"
        );
    }
}

#[test]
fn the_eip155_worked_example_is_reproduced_exactly() {
    let file = load("transactions.json");
    let example = array(&file, "vectors")
        .iter()
        .find(|vector| text(vector, "name") == "eip155-official-example")
        .expect("the EIP-155 example should be in the vectors");

    // Spelled out, because this is the one vector the EIP itself publishes.
    let keypair =
        Keypair::from_hex("0x4646464646464646464646464646464646464646464646464646464646464646")
            .unwrap();
    let transaction = LegacyTransaction {
        nonce: 9,
        gas_price: U256::from(20_000_000_000u64),
        gas_limit: 21_000,
        to: Some(wallet::parse_address("0x3535353535353535353535353535353535353535").unwrap()),
        value: U256::from(1_000_000_000_000_000_000u64),
        data: Vec::new(),
        chain_id: 1,
    };
    let signed = transaction.sign(&keypair).unwrap();

    assert_eq!(
        signed.raw_hex(),
        "0xf86c098504a817c800825208943535353535353535353535353535353535353535880de0\
         b6b3a76400008025a028ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620\
         aa636276a067cbe9d8997f761aecb703304b3800ccf555c9f3dc64214b297fb1966a3b6d83"
            .replace(char::is_whitespace, "")
    );
    assert_eq!(signed.v, 37, "v = recovery_id + chain_id * 2 + 35");
    assert_eq!(
        signed.raw_hex(),
        text(example, "raw"),
        "the vector file agrees"
    );
}

#[test]
fn the_chain_id_is_bound_into_every_signature() {
    let file = load("transactions.json");
    let by_chain: Vec<(u64, &str)> = array(&file, "vectors")
        .iter()
        .map(|vector| (number(vector, "chain_id"), text(vector, "raw")))
        .collect();

    // Two vectors share every field but the chain id; their bytes must differ.
    let testnet = by_chain.iter().find(|(chain, _)| *chain == 338).unwrap();
    let mainnet_example = by_chain.iter().find(|(chain, _)| *chain == 1).unwrap();
    assert_ne!(testnet.1, mainnet_example.1);

    for vector in array(&file, "vectors") {
        let chain_id = number(vector, "chain_id");
        let v = number(vector, "v");
        let recovery = v - chain_id * 2 - 35;
        assert!(
            recovery <= 1,
            "recovery id out of range for chain {chain_id}"
        );
    }
}

// ============================================================= units

#[test]
fn unit_conversions_match_the_vectors() {
    let file = load("units.json");
    for vector in array(&file, "valid") {
        let amount = text(vector, "amount");
        let decimals = number(vector, "decimals") as u8;
        let expected = text(vector, "value");

        let parsed = units::parse_units(amount, decimals)
            .unwrap_or_else(|e| panic!("parsing {amount} with {decimals} decimals: {e}"));
        assert_eq!(parsed.to_string(), expected, "{amount} -> smallest unit");
        assert_eq!(
            units::format_units(parsed, decimals),
            amount,
            "{expected} -> decimal string"
        );
    }
}

#[test]
fn invalid_amounts_are_rejected() {
    let file = load("units.json");
    for vector in array(&file, "invalid") {
        let amount = text(vector, "amount");
        let decimals = number(vector, "decimals") as u8;
        let reason = text(vector, "reason");
        assert_eq!(
            units::parse_units(amount, decimals).unwrap_err().code,
            Code::InvalidAmount,
            "{amount:?} should be rejected ({reason})"
        );
    }
}
