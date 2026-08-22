"""Key derivation, address computation and EIP-191 signing."""

import pytest
from constants import (
    KNOWN_ADDRESSES,
    TEST_ADDRESS_0,
    TEST_ADDRESS_1,
    TEST_MNEMONIC,
    TEST_PRIVATE_KEY,
)

from causewaybay import errors, wallet

# Official Trezor BIP-39 vectors: (entropy hex, phrase).
BIP39_VECTORS = [
    (
        "00000000000000000000000000000000",
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
    ),
    (
        "7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f",
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
    ),
    (
        "80808080808080808080808080808080",
        "letter advice cage absurd amount doctor acoustic avoid letter advice cage above",
    ),
    ("ffffffffffffffffffffffffffffffff", "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong"),
    (
        "9e885d952ad362caeb4efe34a8e91bd2",
        "ozone drill grab fiber curtain grace pudding thank cruise elder eight picnic",
    ),
]


@pytest.mark.parametrize(("entropy_hex", "phrase"), BIP39_VECTORS)
def test_official_bip39_vectors_validate(entropy_hex, phrase):
    assert wallet.validate_mnemonic(phrase)


@pytest.mark.parametrize(("index", "expected"), list(enumerate(KNOWN_ADDRESSES)))
def test_bip44_derivation_matches_the_reference_addresses(index, expected):
    assert wallet.Keypair.from_mnemonic(TEST_MNEMONIC, index).address == expected


def test_index_zero_private_key_is_stable():
    assert wallet.Keypair.from_mnemonic(TEST_MNEMONIC, 0).private_key == TEST_PRIVATE_KEY


def test_derivation_paths_are_bip44():
    assert wallet.ethereum_path(0) == "m/44'/60'/0'/0/0"
    assert wallet.ethereum_path(7) == "m/44'/60'/0'/0/7"
    for bad in [-1, 2**31, True]:
        with pytest.raises(errors.WalletError):
            wallet.ethereum_path(bad)


def test_generate_produces_every_supported_length():
    for words in (12, 15, 18, 21, 24):
        phrase = wallet.generate_mnemonic(words)
        assert len(phrase.split()) == words
        assert wallet.validate_mnemonic(phrase)


def test_generated_phrases_do_not_collide():
    assert wallet.generate_mnemonic(12) != wallet.generate_mnemonic(12)


def test_unsupported_word_counts_are_rejected():
    for words in (0, 11, 13, 25):
        with pytest.raises(errors.WalletError) as excinfo:
            wallet.generate_mnemonic(words)
        assert excinfo.value.code == errors.INVALID_MNEMONIC


@pytest.mark.parametrize(
    "bad",
    [
        "",
        "not a mnemonic",
        # Right length, wrong checksum.
        "abandon " * 11 + "abandon",
        # Right length, unknown word.
        "abandon " * 11 + "notaword",
        "abandon about",
    ],
)
def test_invalid_mnemonics_are_rejected(bad):
    assert not wallet.validate_mnemonic(bad.strip())
    with pytest.raises(errors.WalletError) as excinfo:
        wallet.Keypair.from_mnemonic(bad.strip())
    assert excinfo.value.code == errors.INVALID_MNEMONIC


def test_whitespace_and_case_are_normalised():
    messy = "  ABANDON   abandon\tabandon abandon abandon abandon abandon abandon abandon abandon abandon\nabout "
    assert wallet.validate_mnemonic(messy)
    assert wallet.Keypair.from_mnemonic(messy).address == TEST_ADDRESS_0
    assert wallet.normalize_mnemonic(messy) == TEST_MNEMONIC


def test_a_passphrase_yields_a_different_wallet():
    plain = wallet.Keypair.from_mnemonic(TEST_MNEMONIC, 0)
    salted = wallet.Keypair.from_mnemonic(TEST_MNEMONIC, 0, passphrase="hunter2")
    assert plain.address != salted.address


def test_private_key_round_trips_with_and_without_the_prefix():
    keypair = wallet.Keypair.from_mnemonic(TEST_MNEMONIC, 3)
    assert wallet.Keypair.from_private_key(keypair.private_key).address == keypair.address
    assert wallet.Keypair.from_private_key(keypair.private_key[2:]).address == keypair.address
    assert wallet.Keypair.from_private_key(keypair.private_key.upper()).address == keypair.address


@pytest.mark.parametrize(
    "bad",
    [
        "",
        "0x1234",
        "z" * 64,
        "0" * 64,  # zero is not a valid scalar
        "f" * 64,  # above the curve order
        None,
    ],
)
def test_invalid_private_keys_are_rejected(bad):
    with pytest.raises(errors.WalletError) as excinfo:
        wallet.Keypair.from_private_key(bad)
    assert excinfo.value.code == errors.INVALID_PRIVATE_KEY


def test_public_key_forms_are_consistent():
    keypair = wallet.Keypair.from_mnemonic(TEST_MNEMONIC, 0)
    assert len(keypair.public_key) == 2 + 128
    assert len(keypair.public_key_compressed) == 2 + 66
    # The compressed form carries the same X coordinate.
    assert keypair.public_key_compressed[4:] == keypair.public_key[2:66]


def test_repr_does_not_leak_the_secret():
    keypair = wallet.Keypair.from_mnemonic(TEST_MNEMONIC, 0)
    assert TEST_PRIVATE_KEY not in repr(keypair)
    assert "redacted" in repr(keypair)


def test_eip191_hash_matches_the_reference():
    assert (
        wallet.eip191_hash("Hello World").hex()
        == "a1de988600a42c4b4ab089b619297c17d53cffae5d5120d82d8a92d0bb3b78f2"
    )


def test_signing_and_recovery_round_trip():
    keypair = wallet.Keypair.from_mnemonic(TEST_MNEMONIC, 0)
    signature = keypair.sign_message("hello causewaybay")
    assert len(signature) == 2 + 130
    assert wallet.recover_message("hello causewaybay", signature) == TEST_ADDRESS_0


def test_recovery_of_a_tampered_message_gives_a_different_address():
    keypair = wallet.Keypair.from_mnemonic(TEST_MNEMONIC, 0)
    signature = keypair.sign_message("original")
    assert wallet.recover_message("tampered", signature) != TEST_ADDRESS_0


def test_signatures_are_deterministic():
    keypair = wallet.Keypair.from_mnemonic(TEST_MNEMONIC, 0)
    assert keypair.sign_message("same") == keypair.sign_message("same")


@pytest.mark.parametrize("message", ["", "héllo 🌏", "a" * 5000])
def test_signs_edge_case_messages(message):
    keypair = wallet.Keypair.from_mnemonic(TEST_MNEMONIC, 0)
    assert wallet.recover_message(message, keypair.sign_message(message)) == TEST_ADDRESS_0


def test_malformed_signatures_are_rejected():
    for bad in ["0x", "0x00", "0x" + "11" * 64]:
        with pytest.raises(errors.WalletError):
            wallet.recover_message("x", bad)


def test_address_parsing_and_checksumming():
    assert wallet.parse_address(TEST_ADDRESS_0) == TEST_ADDRESS_0
    assert wallet.parse_address(TEST_ADDRESS_0.lower()) == TEST_ADDRESS_0
    assert wallet.parse_address(f"  {TEST_ADDRESS_1}  ") == TEST_ADDRESS_1
    for bad in ["", "0x123", "nonsense", TEST_ADDRESS_0 + "00"]:
        with pytest.raises(errors.WalletError) as excinfo:
            wallet.parse_address(bad)
        assert excinfo.value.code == errors.INVALID_ADDRESS


def test_hex_parsing():
    assert wallet.parse_hex("0xdeadbeef") == b"\xde\xad\xbe\xef"
    assert wallet.parse_hex("deadbeef") == b"\xde\xad\xbe\xef"
    assert wallet.parse_hex("") == b""
    with pytest.raises(errors.WalletError):
        wallet.parse_hex("0xzz")


def test_mnemonics_are_nfkd_normalised():
    """SPEC.md §3 requires NFKD; without it the two CLIs sharing one store write
    different bytes — and different recall ids — for the same phrase."""
    import unicodedata

    # U+FB01 'ﬁ' decomposes to 'fi' under NFKD.
    composed = "ﬁnd"
    assert wallet.normalize_mnemonic(composed) == unicodedata.normalize("NFKD", composed)
    assert wallet.normalize_mnemonic(composed) == "find"
    # The canonical phrase is unaffected.
    assert wallet.normalize_mnemonic(TEST_MNEMONIC) == TEST_MNEMONIC


# Surrounding whitespace is trimmed, as it is on the Rust side; what must be
# refused is loose hex inside the key itself.
@pytest.mark.parametrize("bad", ["0x" + "a" * 62 + "_1", "0x+" + "a" * 63])
def test_private_keys_reject_loose_hex(bad):
    """`int(body, 16)` accepts underscores and signs; a key is 64 hex digits."""
    with pytest.raises(errors.WalletError):
        wallet.Keypair.from_private_key(bad)
