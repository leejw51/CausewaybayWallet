"""Legacy transaction construction and EIP-155 signing."""

import pytest
from constants import TEST_ADDRESS_0, TEST_PRIVATE_KEY

from causewaybay import errors, wallet
from causewaybay.txs import LegacyTransaction, with_headroom

RECIPIENT = "0x3535353535353535353535353535353535353535"


def sample(**overrides) -> LegacyTransaction:
    fields = {
        "nonce": 9,
        "gas_price": 20_000_000_000,
        "gas_limit": 21_000,
        "to": RECIPIENT,
        "value": 10**18,
        "chain_id": 338,
    }
    fields.update(overrides)
    return LegacyTransaction(**fields)


def test_matches_the_reference_vector():
    """The same bytes the Rust implementation produces from its own RLP encoder."""
    signed = sample().sign(TEST_PRIVATE_KEY)
    assert signed.raw_hex == (
        "0xf86e098504a817c800825208943535353535353535353535353535353535353535"
        "880de0b6b3a7640000808202c7a0153b4b4204abb93d881a58619d36fb62817c1545"
        "f0aa021dc421dcdb04431b2ea03bb09adab62e843b7d1d272e28f24479c2d7fe096e"
        "df4de42d9aacea7a5859ee"
    )
    assert signed.hash == "0x7bdcc151f9fe904d4f4927b7a8537873fd574c1c0eb59fd372fd7fd48399a1da"


def test_v_encodes_the_chain_id():
    for chain_id in (25, 338):
        signed = sample(chain_id=chain_id).sign(TEST_PRIVATE_KEY)
        recovery = signed.v - chain_id * 2 - 35
        assert recovery in (0, 1)


def test_the_signer_is_recoverable():
    signed = sample().sign(TEST_PRIVATE_KEY)
    from eth_account import Account

    recovered = Account.recover_transaction(signed.raw_hex)
    assert recovered == TEST_ADDRESS_0


def test_signing_is_deterministic():
    assert sample().sign(TEST_PRIVATE_KEY).raw == sample().sign(TEST_PRIVATE_KEY).raw


def test_different_chains_produce_different_transactions():
    assert (
        sample(chain_id=338).sign(TEST_PRIVATE_KEY).raw
        != sample(chain_id=25).sign(TEST_PRIVATE_KEY).raw
    )


def test_zero_value_and_zero_nonce_are_accepted():
    signed = sample(nonce=0, value=0).sign(TEST_PRIVATE_KEY)
    assert signed.raw_hex.startswith("0xf8")


def test_contract_creation_omits_the_to_field():
    transaction = sample(to=None, value=0, data=b"\x60\x80", gas_limit=100_000)
    assert "to" not in transaction.as_dict()
    assert transaction.sign(TEST_PRIVATE_KEY).raw


def test_call_data_changes_the_signature():
    with_data = sample(data=bytes.fromhex("a9059cbb"), gas_limit=60_000)
    assert with_data.sign(TEST_PRIVATE_KEY).raw != sample().sign(TEST_PRIVATE_KEY).raw
    assert "a9059cbb" in with_data.sign(TEST_PRIVATE_KEY).raw_hex


def test_negative_fields_are_rejected():
    for field in ("nonce", "gas_price", "gas_limit", "value", "chain_id"):
        with pytest.raises(errors.WalletError) as excinfo:
            sample(**{field: -1}).sign(TEST_PRIVATE_KEY)
        assert excinfo.value.code == errors.USAGE


def test_r_and_s_are_padded_to_32_bytes():
    signed = sample().sign(TEST_PRIVATE_KEY)
    assert len(signed.r) == 2 + 64
    assert len(signed.s) == 2 + 64


def test_a_signature_over_a_different_nonce_recovers_the_same_signer():
    from eth_account import Account

    for nonce in (0, 1, 1000):
        signed = sample(nonce=nonce).sign(TEST_PRIVATE_KEY)
        assert Account.recover_transaction(signed.raw_hex) == TEST_ADDRESS_0


@pytest.mark.parametrize(
    ("estimate", "expected"), [(100_000, 125_000), (21_000, 26_250), (0, 0), (1, 1)]
)
def test_gas_headroom_is_twenty_five_percent(estimate, expected):
    assert with_headroom(estimate) == expected


def test_signing_with_a_bad_key_is_rejected():
    with pytest.raises((ValueError, TypeError, errors.WalletError)):
        sample().sign("0xnotakey")


def test_the_recipient_must_be_checksummed_or_lowercase():
    # eth_account accepts a checksummed address; the app checksums before calling.
    checksummed = wallet.parse_address(RECIPIENT)
    assert sample(to=checksummed).sign(TEST_PRIVATE_KEY).raw
