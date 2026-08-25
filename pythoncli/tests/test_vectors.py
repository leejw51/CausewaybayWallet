"""The shared test vectors, driven through the Python binding.

``testvectors/`` is the same set of files the Rust and Lua suites read, so a
disagreement between the three shows up here rather than on chain. What this
suite adds over the Rust one is the *path*: these numbers travel through the C
ABI and ctypes before being compared, which is where a binding's own class of
mistake lives — a truncated string, a number silently turned into a float, an
emoji mangled on the way through.

Regenerate the files with ``make vectors`` from the repository root.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from causewaybay import Wallet
from causewaybay.errors import WalletError

VECTOR_DIR = Path(__file__).resolve().parents[2] / "testvectors"


def load(name: str) -> dict:
    """Load one vector file, failing with a hint rather than a bare error."""
    path = VECTOR_DIR / name
    if not path.exists():
        pytest.fail(f"missing {path}\nrun `make vectors` from the repository root")
    return json.loads(path.read_text(encoding="utf-8"))


BIP39_INVALID = load("bip39-invalid.json")
DERIVATION = load("derivation.json")
KEYS = load("keys.json")
KEYS_INVALID = load("keys-invalid.json")
EIP55 = load("eip55.json")
KECCAK = load("keccak.json")
EIP191 = load("eip191.json")
UNITS = load("units.json")
MULTICHAIN = load("multichain.json")
ECASH = load("ecash.json")

# The files this suite reads. Two are left out on purpose:
#
#   bip39.json        the BIP-39 seed is internal; no command prints it, so a
#                     front end that only speaks the CLI surface cannot assert
#                     on it. The Rust suite calls the library directly.
#   transactions.json signing a transaction needs a nonce and a gas price,
#                     which means a node; the Rust suite mocks one.
CONSUMED = {
    "bip39-invalid.json",
    "derivation.json",
    "eip191.json",
    "eip55.json",
    "keccak.json",
    "keys.json",
    "keys-invalid.json",
    "ecash.json",
    "multichain.json",
    "units.json",
}
NOT_CONSUMED = {"bip39.json", "transactions.json"}


# ================================================================== mnemonics


@pytest.mark.parametrize("vector", BIP39_INVALID["vectors"], ids=lambda v: v["reason"])
def test_every_invalid_phrase_is_rejected(wallet: Wallet, vector):
    checked = wallet.validate_mnemonic(vector["mnemonic"])
    assert checked["valid"] is False, vector["reason"]


@pytest.mark.parametrize("entry", DERIVATION["mnemonics"], ids=lambda m: m["name"])
def test_derivation_matches_well_known_wallets(wallet: Wallet, entry):
    """The addresses MetaMask, Anvil and every other wallet derive."""
    for account in entry["accounts"]:
        derived = wallet.derive(mnemonic=entry["phrase"], index=account["index"])
        assert derived["address"] == account["address"], account["index"]
        assert derived["private_key"] == account["private_key"]
        assert derived["derivation_path"] == account["path"]


@pytest.mark.parametrize("entry", KEYS["keys"], ids=lambda k: k["name"])
def test_known_private_keys_produce_their_published_addresses(wallet: Wallet, entry):
    derived = wallet.derive(private_key=entry["private_key"])
    assert derived["address"] == entry["address"]
    assert derived["public_key"] == entry["public_key"]


@pytest.mark.parametrize("vector", KEYS_INVALID["vectors"], ids=lambda v: v["reason"])
def test_invalid_private_keys_are_rejected(wallet: Wallet, vector):
    with pytest.raises(WalletError) as caught:
        wallet.derive(private_key=vector["private_key"] or "-empty-")
    # An empty key never becomes a request at all: `derive` cannot tell "" from
    # "not given", and refusing it here is the same refusal one step earlier.
    assert caught.value.code in ("invalid_private_key", "usage"), vector["reason"]


# ================================================================== hashing


@pytest.mark.parametrize("vector", KECCAK["hashes"], ids=lambda v: v["text"][:24] or "empty")
def test_keccak_matches_published_digests(wallet: Wallet, vector):
    assert wallet.keccak(vector["text"])["keccak256"] == vector["keccak256"]


def test_the_emoji_vector_survives_the_whole_round_trip(wallet: Wallet):
    """keccak.json stores it as a surrogate pair.

    A decoder that got that wrong would hand the wallet different bytes and
    fail with a hash mismatch that says nothing about why.
    """
    emoji = [v for v in KECCAK["hashes"] if not v["text"].isascii()]
    assert emoji, "keccak.json should carry a non-ASCII vector"
    for vector in emoji:
        assert wallet.keccak(vector["text"])["keccak256"] == vector["keccak256"]


@pytest.mark.parametrize("vector", KECCAK["selectors"], ids=lambda v: v["signature"])
def test_selectors_are_the_first_four_bytes_of_the_signature_hash(wallet: Wallet, vector):
    assert wallet.keccak(vector["signature"])["keccak256"][:10] == vector["selector"]


@pytest.mark.parametrize("vector", EIP55["vectors"], ids=lambda v: v["lowercase"][:12])
def test_eip55_checksums_match_the_reference_addresses(wallet: Wallet, vector):
    assert wallet.checksum(vector["lowercase"])["address"] == vector["checksummed"]
    # Already-checksummed input comes back unchanged; uppercase is re-cased
    # rather than rejected.
    assert wallet.checksum(vector["checksummed"])["address"] == vector["checksummed"]


# ================================================================ signatures


@pytest.mark.parametrize("vector", EIP191["vectors"], ids=lambda v: v["message"][:24] or "empty")
def test_eip191_signatures_match_the_reference_signer(wallet: Wallet, vector):
    # The vector names the signer; the key that signed the whole set is named
    # once at the top of the file.
    signed = wallet.sign_with(EIP191["signing_key"], vector["message"])
    assert signed["signature"] == vector["signature"], vector["message"][:24]
    verified = wallet.verify(vector["message"], vector["signature"], vector["signer"])
    assert verified["valid"] is True


def test_a_tampered_message_does_not_verify(wallet: Wallet):
    vector = EIP191["vectors"][1]
    verified = wallet.verify(vector["message"] + "!", vector["signature"], vector["signer"])
    assert verified["valid"] is False


# ===================================================================== units


@pytest.mark.parametrize("vector", UNITS["valid"], ids=lambda v: v["amount"])
def test_unit_conversions_match_the_vectors(wallet: Wallet, vector):
    decimals = vector.get("decimals")
    assert wallet.to_wei(vector["amount"], decimals)["value"] == vector["value"]


@pytest.mark.parametrize("vector", UNITS["invalid"], ids=lambda v: v["reason"])
def test_invalid_amounts_are_rejected(wallet: Wallet, vector):
    with pytest.raises(WalletError) as caught:
        wallet.to_wei(vector["amount"], vector.get("decimals"))
    assert caught.value.code == "invalid_amount", vector["reason"]


# ================================================================ multichain


@pytest.mark.parametrize("index", range(3))
def test_solana_addresses_match_the_sdk(wallet: Wallet, index):
    expected = MULTICHAIN["solana"]["accounts"][index]
    got = wallet.derive(mnemonic=MULTICHAIN["mnemonic"], index=index, chain="solana")
    assert got["address"] == expected["address"]
    assert got["derivation_path"] == expected["path"]
    # On Solana the address *is* the public key, so both come back base58.
    assert got["public_key"] == expected["address"]
    assert not got["private_key"].startswith("0x"), "a Solana key is not hex"


@pytest.mark.parametrize("index", range(3))
def test_cardano_addresses_match_the_sdk(wallet: Wallet, index):
    expected = MULTICHAIN["cardano"]["accounts"][index]
    got = wallet.derive(mnemonic=MULTICHAIN["mnemonic"], index=index, chain="cardano")
    # The network lives inside a Cardano address, so the wallet carries both:
    # what it shows is testnet, and mainnet rides along beside it.
    assert got["address"] == expected["base_addr_testnet"]
    assert got["address_mainnet"] == expected["base_addr_mainnet"]
    assert got["derivation_path"] == expected["path"]
    assert got["payment_key_hash"] == expected["payment_keyhash_hex"]


@pytest.mark.parametrize("index", range(3))
def test_midnight_addresses_match_the_sdk(wallet: Wallet, index):
    expected = MULTICHAIN["midnight"]["accounts"][index]
    got = wallet.derive(mnemonic=MULTICHAIN["mnemonic"], index=index, chain="midnight")
    assert got["address_mainnet"] == expected["addr_mainnet"]
    assert got["address"].startswith("mn_addr_"), "a testnet address names its network"
    assert got["derivation_path"] == expected["path"]
    assert got["public_key"] == expected["verifying_key_hex"]
    # The stored secret is the night key and the dust seed together, since an
    # account that cannot pay a fee is not a usable account.
    assert got["private_key"].startswith(expected["night_sk_hex"])


@pytest.mark.parametrize("index", range(3))
def test_ecash_addresses_match_the_reference_generator(wallet: Wallet, index):
    expected = ECASH["accounts"][index]
    got = wallet.derive(mnemonic=ECASH["mnemonic"], index=index, chain="ecash")
    # A CashAddr prefix is inside the checksum, so these are two different
    # strings rather than one string with two labels.
    assert got["address"] == expected["address"]
    assert got["address_mainnet"] == expected["address_mainnet"]
    assert got["derivation_path"] == expected["path"]
    assert got["public_key"] == expected["public_key_compressed"]
    assert got["public_key_hash"] == expected["public_key_hash"]
    # Wallet Import Format, which is what eCash's own wallets read and write.
    assert got["wif"] == expected["wif"]
    assert got["wif_mainnet"] == expected["wif_mainnet"]


@pytest.mark.parametrize("form", ["wif", "wif_mainnet", "private_key"])
def test_an_ecash_key_imports_back_from_every_encoding_it_exports(wallet: Wallet, form):
    """A key exported in any of its three spellings must come back as one account."""
    expected = ECASH["accounts"][0]
    got = wallet.derive(private_key=expected[form], chain="ecash")
    assert got["address"] == expected["address"]
    # And whichever went in, hex is what the store holds.
    assert got["private_key"] == expected["private_key"]


def test_one_phrase_every_chain_a_different_address_on_each(wallet: Wallet):
    """The whole claim of the wallet, in one assertion."""
    seen = {}
    for chain in ("evm", "solana", "cardano", "midnight", "ecash"):
        account = wallet.derive(mnemonic=MULTICHAIN["mnemonic"], index=0, chain=chain)
        assert account["address"] not in seen, f"{chain} repeats {seen.get(account['address'])}"
        seen[account["address"]] = chain
    assert seen[MULTICHAIN["solana"]["accounts"][0]["address"]] == "solana"
    assert seen[MULTICHAIN["cardano"]["accounts"][0]["base_addr_testnet"]] == "cardano"


# ================================================================== coverage


def test_every_vector_file_is_accounted_for():
    on_disk = {path.name for path in VECTOR_DIR.glob("*.json")}
    assert on_disk, "no vector files found"
    assert on_disk == CONSUMED | NOT_CONSUMED, (
        "a vector file appeared or vanished; read it here or say why not"
    )
