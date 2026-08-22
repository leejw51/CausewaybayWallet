"""Tests against the shared vectors in ``../testvectors``.

These are the checks that tie this implementation to the outside world: the
official BIP-39 and EIP-55 vectors, the worked example from EIP-155, and the
mnemonics and keys that Anvil, Hardhat and Ganache print on startup. The Rust
implementation runs the same files, so a disagreement between the two shows up
as a failure here rather than as a surprise on chain.

Regenerate the files with ``make vectors``.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from causewaybay import erc20, errors, units, wallet
from causewaybay.txs import LegacyTransaction

VECTOR_DIR = Path(__file__).resolve().parents[2] / "testvectors"


def load(name: str) -> dict:
    """Load one vector file, failing with a hint rather than a bare error."""
    path = VECTOR_DIR / name
    if not path.exists():
        pytest.fail(f"missing {path}\nrun `make vectors` from the repository root")
    return json.loads(path.read_text(encoding="utf-8"))


BIP39 = load("bip39.json")
BIP39_INVALID = load("bip39-invalid.json")
DERIVATION = load("derivation.json")
KEYS = load("keys.json")
KEYS_INVALID = load("keys-invalid.json")
EIP55 = load("eip55.json")
KECCAK = load("keccak.json")
EIP191 = load("eip191.json")
TRANSACTIONS = load("transactions.json")
UNITS = load("units.json")


# ============================================================== BIP-39


def test_the_official_set_is_complete():
    assert len(BIP39["vectors"]) == 25, "the official English set has 25 vectors"


@pytest.mark.parametrize("vector", BIP39["vectors"], ids=lambda v: v["entropy"][:16])
def test_bip39_matches_the_official_trezor_vectors(vector):
    phrase = vector["mnemonic"]
    assert wallet.validate_mnemonic(phrase), "official vector must validate"
    assert len(phrase.split()) == vector["word_count"]

    from mnemonic import Mnemonic

    reference = Mnemonic("english")
    assert reference.to_mnemonic(bytes.fromhex(vector["entropy"])) == phrase
    assert reference.to_seed(phrase, "TREZOR").hex() == vector["seed_trezor"]
    assert reference.to_seed(phrase, "").hex() == vector["seed_empty_passphrase"]


@pytest.mark.parametrize("case", BIP39["normalization"], ids=lambda c: repr(c["input"][:24]))
def test_bip39_normalises_the_same_phrase_written_differently(case):
    messy, canonical = case["input"], case["canonical"]
    assert wallet.validate_mnemonic(messy), "should validate after normalisation"
    assert wallet.normalize_mnemonic(messy) == canonical
    assert (
        wallet.Keypair.from_mnemonic(messy, 0).address
        == wallet.Keypair.from_mnemonic(canonical, 0).address
    ), "should name the same wallet as the canonical phrase"


@pytest.mark.parametrize("vector", BIP39_INVALID["vectors"], ids=lambda v: v["reason"])
def test_bip39_rejects_every_invalid_vector(vector):
    phrase = vector["mnemonic"]
    assert not wallet.validate_mnemonic(phrase), vector["reason"]
    with pytest.raises(errors.WalletError) as excinfo:
        wallet.Keypair.from_mnemonic(phrase)
    assert excinfo.value.code == errors.INVALID_MNEMONIC, vector["reason"]


# ============================================================== BIP-44


def test_the_known_mnemonic_set_is_complete():
    assert len(DERIVATION["mnemonics"]) >= 6
    names = {entry["name"] for entry in DERIVATION["mnemonics"]}
    assert {"bip39-canonical", "foundry-anvil-default", "ganache-default"} <= names


@pytest.mark.parametrize("entry", DERIVATION["mnemonics"], ids=lambda e: e["name"])
def test_derivation_matches_well_known_wallets(entry):
    phrase = entry["phrase"]
    for account in entry["accounts"]:
        index = account["index"]
        keypair = wallet.Keypair.from_mnemonic(phrase, index)
        assert keypair.address == account["address"], f"index {index} address"
        assert keypair.private_key == account["private_key"], f"index {index} private key"
        assert wallet.ethereum_path(index) == account["path"]

    # A BIP-39 passphrase must yield an entirely different wallet.
    salted = wallet.Keypair.from_mnemonic(phrase, 0, passphrase="TREZOR")
    assert salted.address == entry["passphrase_trezor_index_0"]
    assert salted.address != entry["accounts"][0]["address"]


def test_the_anvil_mnemonic_derives_the_addresses_anvil_prints():
    """Spelled out: these are the addresses a developer sees on every local node."""
    anvil = next(e for e in DERIVATION["mnemonics"] if e["name"] == "foundry-anvil-default")
    first = wallet.Keypair.from_mnemonic(anvil["phrase"], 0)
    assert first.address == "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
    assert first.private_key == (
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
    )
    second = wallet.Keypair.from_mnemonic(anvil["phrase"], 1)
    assert second.address == "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"


# ========================================================= private keys


@pytest.mark.parametrize("entry", KEYS["keys"], ids=lambda e: e["name"])
def test_known_private_keys_produce_their_published_addresses(entry):
    keypair = wallet.Keypair.from_private_key(entry["private_key"])
    assert keypair.address == entry["address"]
    assert keypair.public_key == entry["public_key"]
    assert keypair.public_key_compressed == entry["public_key_compressed"]


@pytest.mark.parametrize("entry", KEYS["keys"], ids=lambda e: e["name"])
def test_private_keys_parse_with_or_without_the_prefix(entry):
    canonical = entry["private_key"]
    assert wallet.Keypair.from_private_key(canonical[2:]).address == entry["address"]
    assert wallet.Keypair.from_private_key(canonical.upper()).address == entry["address"]
    assert wallet.Keypair.from_private_key(f"  {canonical}  ").address == entry["address"]


@pytest.mark.parametrize("vector", KEYS_INVALID["vectors"], ids=lambda v: v["reason"])
def test_invalid_private_keys_are_rejected(vector):
    with pytest.raises(errors.WalletError) as excinfo:
        wallet.Keypair.from_private_key(vector["private_key"])
    assert excinfo.value.code == errors.INVALID_PRIVATE_KEY, vector["reason"]


# =============================================================== EIP-55


@pytest.mark.parametrize("vector", EIP55["vectors"], ids=lambda v: v["checksummed"][:12])
def test_eip55_checksums_match_the_reference_addresses(vector):
    assert wallet.parse_address(vector["lowercase"]) == vector["checksummed"]
    # The checksummed form must survive a round trip unchanged.
    assert wallet.parse_address(vector["checksummed"]) == vector["checksummed"]


# =============================================================== keccak


@pytest.mark.parametrize("vector", KECCAK["hashes"], ids=lambda v: repr(v["text"][:20]))
def test_keccak_matches_published_digests(vector):
    from eth_utils import keccak

    assert "0x" + keccak(text=vector["text"]).hex() == vector["keccak256"]


SHIPPED_SELECTORS = {
    "transfer(address,uint256)": erc20.SELECTOR_TRANSFER,
    "balanceOf(address)": erc20.SELECTOR_BALANCE_OF,
    "decimals()": erc20.SELECTOR_DECIMALS,
    "symbol()": erc20.SELECTOR_SYMBOL,
    "name()": erc20.SELECTOR_NAME,
    "totalSupply()": erc20.SELECTOR_TOTAL_SUPPLY,
    "allowance(address,address)": erc20.SELECTOR_ALLOWANCE,
    "approve(address,uint256)": erc20.SELECTOR_APPROVE,
}


@pytest.mark.parametrize("vector", KECCAK["selectors"], ids=lambda v: v["signature"])
def test_erc20_selectors_are_the_first_four_bytes_of_the_signature_hash(vector):
    from eth_utils import keccak

    signature, expected = vector["signature"], vector["selector"]
    # Computed from the signature…
    assert "0x" + keccak(text=signature)[:4].hex() == expected
    # …and matching the constant this package ships.
    assert "0x" + SHIPPED_SELECTORS[signature].hex() == expected


# ============================================================== EIP-191


@pytest.mark.parametrize("vector", EIP191["vectors"], ids=lambda v: repr(v["message"][:20]))
def test_eip191_signatures_match_the_reference_signer(vector):
    keypair = wallet.Keypair.from_private_key(EIP191["signing_key"])
    message = vector["message"]

    assert "0x" + wallet.eip191_hash(message).hex() == vector["prefixed_hash"]
    # RFC-6979 makes this deterministic, so the bytes must match exactly.
    assert keypair.sign_message(message) == vector["signature"]
    assert wallet.recover_message(message, vector["signature"]) == vector["signer"]


def test_eip191_recovery_rejects_a_tampered_message():
    vector = EIP191["vectors"][1]
    recovered = wallet.recover_message("a different message", vector["signature"])
    assert recovered != vector["signer"]


# ========================================================= transactions


def test_the_transaction_set_covers_both_cronos_chains():
    chain_ids = {vector["chain_id"] for vector in TRANSACTIONS["vectors"]}
    assert {1, 25, 338} <= chain_ids


@pytest.mark.parametrize("vector", TRANSACTIONS["vectors"], ids=lambda v: v["name"])
def test_signed_transactions_match_the_reference_signer(vector):
    keypair = wallet.Keypair.from_private_key(vector["private_key"])
    assert keypair.address == vector["signer"]

    signed = LegacyTransaction(
        nonce=vector["nonce"],
        gas_price=int(vector["gas_price"]),
        gas_limit=vector["gas_limit"],
        to=wallet.parse_address(vector["to"]),
        value=int(vector["value"]),
        chain_id=vector["chain_id"],
        data=wallet.parse_hex(vector["data"]),
    ).sign(keypair.private_key)

    assert signed.raw_hex == vector["raw"]
    assert signed.hash == vector["hash"]
    assert signed.v == vector["v"]
    assert signed.r == vector["r"]
    assert signed.s == vector["s"]


def test_the_eip155_worked_example_is_reproduced_exactly():
    """Spelled out, because this is the one vector the EIP itself publishes."""
    signed = LegacyTransaction(
        nonce=9,
        gas_price=20_000_000_000,
        gas_limit=21_000,
        to="0x3535353535353535353535353535353535353535",
        value=1_000_000_000_000_000_000,
        chain_id=1,
    ).sign("0x4646464646464646464646464646464646464646464646464646464646464646")

    assert signed.raw_hex == (
        "0xf86c098504a817c800825208943535353535353535353535353535353535353535880de0"
        "b6b3a76400008025a028ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620"
        "aa636276a067cbe9d8997f761aecb703304b3800ccf555c9f3dc64214b297fb1966a3b6d83"
    )
    assert signed.v == 37, "v = recovery_id + chain_id * 2 + 35"

    example = next(v for v in TRANSACTIONS["vectors"] if v["name"] == "eip155-official-example")
    assert signed.raw_hex == example["raw"], "the vector file agrees"


@pytest.mark.parametrize("vector", TRANSACTIONS["vectors"], ids=lambda v: v["name"])
def test_the_chain_id_is_bound_into_every_signature(vector):
    recovery = vector["v"] - vector["chain_id"] * 2 - 35
    assert recovery in (0, 1), f"recovery id out of range for chain {vector['chain_id']}"


def test_the_same_transaction_on_two_chains_differs():
    by_chain = {v["chain_id"]: v["raw"] for v in TRANSACTIONS["vectors"]}
    assert by_chain[1] != by_chain[338], (
        "EIP-155 makes a signature for one chain unusable on another"
    )


# ================================================================ units


@pytest.mark.parametrize(
    "vector", UNITS["valid"], ids=lambda v: f"{v['amount'][:18]}@{v['decimals']}"
)
def test_unit_conversions_match_the_vectors(vector):
    amount, decimals, expected = vector["amount"], vector["decimals"], vector["value"]
    parsed = units.parse_units(amount, decimals)
    assert str(parsed) == expected
    assert units.format_units(parsed, decimals) == amount


@pytest.mark.parametrize("vector", UNITS["invalid"], ids=lambda v: v["reason"])
def test_invalid_amounts_are_rejected(vector):
    with pytest.raises(errors.WalletError) as excinfo:
        units.parse_units(vector["amount"], vector["decimals"])
    assert excinfo.value.code == errors.INVALID_AMOUNT, vector["reason"]


# ============================================ the two implementations agree


def test_every_vector_file_is_present_and_generated():
    expected = {
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
    }
    present = {path.name for path in VECTOR_DIR.glob("*.json")}
    assert expected <= present, f"missing vector files: {expected - present}"
    for name in expected:
        assert "$comment" in load(name), f"{name} should record how it was generated"
