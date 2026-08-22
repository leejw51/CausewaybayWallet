"""Minimal ABI encoding and decoding for the ERC-20 functions the wallet uses."""

from __future__ import annotations

from eth_utils import to_checksum_address

from . import errors

SELECTOR_TRANSFER = bytes.fromhex("a9059cbb")  # transfer(address,uint256)
SELECTOR_BALANCE_OF = bytes.fromhex("70a08231")  # balanceOf(address)
SELECTOR_DECIMALS = bytes.fromhex("313ce567")  # decimals()
SELECTOR_SYMBOL = bytes.fromhex("95d89b41")  # symbol()
SELECTOR_NAME = bytes.fromhex("06fdde03")  # name()
SELECTOR_TOTAL_SUPPLY = bytes.fromhex("18160ddd")  # totalSupply()
SELECTOR_ALLOWANCE = bytes.fromhex("dd62ed3e")  # allowance(address,address)
SELECTOR_APPROVE = bytes.fromhex("095ea7b3")  # approve(address,uint256)


def _pad_address(address: str) -> bytes:
    raw = bytes.fromhex(to_checksum_address(address)[2:])
    return b"\x00" * 12 + raw


def _pad_uint(value: int) -> bytes:
    if value < 0 or value >= 2**256:
        raise errors.invalid_amount("value does not fit in a uint256")
    return value.to_bytes(32, "big")


def encode_transfer(to: str, amount: int) -> bytes:
    return SELECTOR_TRANSFER + _pad_address(to) + _pad_uint(amount)


def encode_approve(spender: str, amount: int) -> bytes:
    return SELECTOR_APPROVE + _pad_address(spender) + _pad_uint(amount)


def encode_balance_of(owner: str) -> bytes:
    return SELECTOR_BALANCE_OF + _pad_address(owner)


def encode_allowance(owner: str, spender: str) -> bytes:
    return SELECTOR_ALLOWANCE + _pad_address(owner) + _pad_address(spender)


def encode_getter(selector: bytes) -> bytes:
    """A zero-argument getter such as ``decimals()`` or ``symbol()``."""
    return selector


def decode_uint(data: bytes) -> int:
    """Decode a single ``uint256`` return value."""
    if len(data) < 32:
        raise errors.rpc_error(f"expected a 32-byte uint return value, got {len(data)} bytes")
    return int.from_bytes(data[:32], "big")


def decode_u8(data: bytes) -> int:
    """Decode a ``uint8`` such as ``decimals()``."""
    value = decode_uint(data)
    if value > 255:
        raise errors.rpc_error("decimals() returned a value larger than 255")
    return value


def decode_string(data: bytes) -> str:
    """Decode a ``string`` return value.

    Some older tokens return a raw ``bytes32`` instead of a dynamic string, so
    both layouts are accepted.
    """
    if not data:
        return ""
    if len(data) >= 64:
        offset = int.from_bytes(data[:32], "big")
        if offset == 32:
            length = int.from_bytes(data[32:64], "big")
            if 64 + length <= len(data):
                return data[64 : 64 + length].decode("utf-8", errors="replace")
    # bytes32 fallback: trim the zero padding.
    head = data[:32]
    end = head.find(b"\x00")
    if end == -1:
        end = len(head)
    return head[:end].decode("utf-8", errors="replace").strip()
