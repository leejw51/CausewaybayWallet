"""ABI encoding and decoding for ERC-20."""

import pytest
from constants import TEST_ADDRESS_0, TEST_ADDRESS_1

from causewaybay import erc20, errors


def test_selectors_match_the_standard():
    """The first four bytes of keccak(signature), as every ERC-20 tool computes."""
    assert erc20.SELECTOR_TRANSFER.hex() == "a9059cbb"
    assert erc20.SELECTOR_BALANCE_OF.hex() == "70a08231"
    assert erc20.SELECTOR_DECIMALS.hex() == "313ce567"
    assert erc20.SELECTOR_SYMBOL.hex() == "95d89b41"
    assert erc20.SELECTOR_NAME.hex() == "06fdde03"
    assert erc20.SELECTOR_TOTAL_SUPPLY.hex() == "18160ddd"
    assert erc20.SELECTOR_ALLOWANCE.hex() == "dd62ed3e"
    assert erc20.SELECTOR_APPROVE.hex() == "095ea7b3"


def test_selectors_agree_with_keccak():
    from eth_utils import keccak

    signatures = {
        "transfer(address,uint256)": erc20.SELECTOR_TRANSFER,
        "balanceOf(address)": erc20.SELECTOR_BALANCE_OF,
        "decimals()": erc20.SELECTOR_DECIMALS,
        "symbol()": erc20.SELECTOR_SYMBOL,
        "name()": erc20.SELECTOR_NAME,
        "totalSupply()": erc20.SELECTOR_TOTAL_SUPPLY,
        "allowance(address,address)": erc20.SELECTOR_ALLOWANCE,
        "approve(address,uint256)": erc20.SELECTOR_APPROVE,
    }
    for signature, selector in signatures.items():
        assert keccak(text=signature)[:4] == selector, signature


def test_encodes_transfer_calldata():
    data = erc20.encode_transfer("0x3535353535353535353535353535353535353535", 1_000_000)
    assert len(data) == 68
    assert data.hex() == (
        "a9059cbb"
        "0000000000000000000000003535353535353535353535353535353535353535"
        "00000000000000000000000000000000000000000000000000000000000f4240"
    )


def test_encodes_balance_of_calldata():
    data = erc20.encode_balance_of(TEST_ADDRESS_0)
    assert len(data) == 36
    assert data.hex() == (
        "70a082310000000000000000000000009858effd232b4033e47d90003d41ec34ecaeda94"
    )


def test_encodes_allowance_and_approve():
    assert len(erc20.encode_allowance(TEST_ADDRESS_0, TEST_ADDRESS_1)) == 68
    approve = erc20.encode_approve(TEST_ADDRESS_1, 2**256 - 1)
    assert len(approve) == 68
    assert approve.hex().endswith("f" * 64)


def test_getters_are_just_the_selector():
    assert erc20.encode_getter(erc20.SELECTOR_DECIMALS) == bytes.fromhex("313ce567")


def test_encoding_rejects_an_out_of_range_amount():
    for bad in (-1, 2**256):
        with pytest.raises(errors.WalletError):
            erc20.encode_transfer(TEST_ADDRESS_0, bad)


def test_decodes_uints():
    word = (18).to_bytes(32, "big")
    assert erc20.decode_uint(word) == 18
    assert erc20.decode_u8(word) == 18
    assert erc20.decode_uint(b"\xff" * 32) == 2**256 - 1
    with pytest.raises(errors.WalletError):
        erc20.decode_u8(b"\xff" * 32)
    for short in (b"", b"\x00" * 16):
        with pytest.raises(errors.WalletError):
            erc20.decode_uint(short)


def abi_string(text: str) -> bytes:
    body = text.encode()
    padded = body + b"\x00" * ((32 - len(body) % 32) % 32)
    return (32).to_bytes(32, "big") + len(body).to_bytes(32, "big") + padded


@pytest.mark.parametrize("text", ["VVS", "", "A Rather Long Token Name That Exceeds One Word"])
def test_decodes_dynamic_strings(text):
    assert erc20.decode_string(abi_string(text)) == text


def test_decodes_bytes32_style_strings():
    word = b"MKR" + b"\x00" * 29
    assert erc20.decode_string(word) == "MKR"


def test_decodes_empty_return_data():
    assert erc20.decode_string(b"") == ""


def test_tolerates_a_truncated_dynamic_string():
    # A length that overruns the buffer must not raise.
    broken = (32).to_bytes(32, "big") + (0xFF).to_bytes(32, "big")
    assert isinstance(erc20.decode_string(broken), str)


def test_decodes_non_utf8_without_raising():
    assert isinstance(erc20.decode_string(b"\xff\xfe" + b"\x00" * 30), str)
