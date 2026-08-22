"""The error type, carrying a stable machine-readable code.

Every code here is part of the public contract in ``SPEC.md``; the Rust
implementation emits the same strings so callers can branch on them.
"""

from __future__ import annotations

USAGE = "usage"
NOT_FOUND = "not_found"
ACCOUNT_NOT_FOUND = "account_not_found"
DUPLICATE_LABEL = "duplicate_label"
INVALID_MNEMONIC = "invalid_mnemonic"
INVALID_PRIVATE_KEY = "invalid_private_key"
INVALID_ADDRESS = "invalid_address"
INVALID_AMOUNT = "invalid_amount"
NO_ACTIVE_ACCOUNT = "no_active_account"
UNKNOWN_NETWORK = "unknown_network"
RPC_ERROR = "rpc_error"
INSUFFICIENT_FUNDS = "insufficient_funds"
CONFIRMATION_REQUIRED = "confirmation_required"
IO_ERROR = "io_error"
INTERNAL = "internal"

ALL_CODES = (
    USAGE,
    NOT_FOUND,
    ACCOUNT_NOT_FOUND,
    DUPLICATE_LABEL,
    INVALID_MNEMONIC,
    INVALID_PRIVATE_KEY,
    INVALID_ADDRESS,
    INVALID_AMOUNT,
    NO_ACTIVE_ACCOUNT,
    UNKNOWN_NETWORK,
    RPC_ERROR,
    INSUFFICIENT_FUNDS,
    CONFIRMATION_REQUIRED,
    IO_ERROR,
    INTERNAL,
)


class WalletError(Exception):
    """An error the CLI knows how to report, with a stable code."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message

    def __str__(self) -> str:  # pragma: no cover - trivial
        return self.message


def _maker(code: str):
    def make(message: str) -> WalletError:
        return WalletError(code, message)

    return make


usage = _maker(USAGE)
not_found = _maker(NOT_FOUND)
account_not_found = _maker(ACCOUNT_NOT_FOUND)
duplicate_label = _maker(DUPLICATE_LABEL)
invalid_mnemonic = _maker(INVALID_MNEMONIC)
invalid_private_key = _maker(INVALID_PRIVATE_KEY)
invalid_address = _maker(INVALID_ADDRESS)
invalid_amount = _maker(INVALID_AMOUNT)
no_active_account = _maker(NO_ACTIVE_ACCOUNT)
unknown_network = _maker(UNKNOWN_NETWORK)
rpc_error = _maker(RPC_ERROR)
insufficient_funds = _maker(INSUFFICIENT_FUNDS)
confirmation_required = _maker(CONFIRMATION_REQUIRED)
io_error = _maker(IO_ERROR)
internal = _maker(INTERNAL)
