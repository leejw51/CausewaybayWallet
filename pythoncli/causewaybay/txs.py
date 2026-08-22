"""Legacy (type 0x0) transaction construction and EIP-155 signing.

Cronos accepts legacy transactions on both networks, and they keep the encoding —
and therefore the tests — simple and fully deterministic. Signing is delegated to
``eth_account``; the Rust side builds the same bytes from its own RLP encoder and
both are checked against the same reference vector.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from eth_account import Account

from . import errors


@dataclass(frozen=True)
class SignedTransaction:
    hash: str
    raw: bytes
    v: int
    r: str
    s: str

    @property
    def raw_hex(self) -> str:
        return "0x" + self.raw.hex()


@dataclass(frozen=True)
class LegacyTransaction:
    nonce: int
    gas_price: int
    gas_limit: int
    to: str | None
    value: int
    chain_id: int
    data: bytes = b""

    def as_dict(self) -> dict[str, Any]:
        """The transaction in the shape ``eth_account`` expects."""
        tx: dict[str, Any] = {
            "nonce": self.nonce,
            "gasPrice": self.gas_price,
            "gas": self.gas_limit,
            "value": self.value,
            "chainId": self.chain_id,
            "data": self.data,
        }
        # An absent ``to`` means contract creation.
        if self.to is not None:
            tx["to"] = self.to
        return tx

    def sign(self, private_key: str) -> SignedTransaction:
        """Sign with EIP-155 replay protection."""
        for name, value in (
            ("nonce", self.nonce),
            ("gas price", self.gas_price),
            ("gas limit", self.gas_limit),
            ("value", self.value),
            ("chain id", self.chain_id),
        ):
            if value < 0:
                raise errors.usage(f"{name} must not be negative")
        signed = Account.sign_transaction(self.as_dict(), private_key)
        return SignedTransaction(
            hash="0x" + signed.hash.hex().removeprefix("0x"),
            raw=bytes(signed.raw_transaction),
            v=signed.v,
            r="0x" + format(signed.r, "064x"),
            s="0x" + format(signed.s, "064x"),
        )


def with_headroom(estimate: int) -> int:
    """Add 25 % headroom so a slightly heavier execution still fits."""
    return estimate * 125 // 100
