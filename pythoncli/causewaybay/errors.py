"""The error type, carrying the stable code the core returned.

The codes themselves are not listed here. They belong to the wallet, which
reports them through ``cwb_describe`` — a second copy in Python is a copy that
goes stale without anything noticing. Ask a wallet: ``wallet.codes()``.

What is here is the one case that never reaches the core at all: the library
would not load, or its reply was not something this binding could read.
"""

from __future__ import annotations


class WalletError(Exception):
    """An error with a stable machine-readable code from ``SPEC.md``."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message

    def __str__(self) -> str:  # pragma: no cover - trivial
        return self.message

    def __repr__(self) -> str:  # pragma: no cover - trivial
        return f"WalletError({self.code!r}, {self.message!r})"


def usage(message: str) -> WalletError:
    """The one code this front end raises on its own: a bad call into it."""
    return WalletError("usage", message)


def internal(message: str) -> WalletError:
    """A reply this binding could not read. Never a wallet failure."""
    return WalletError("internal", message)


def io_error(message: str) -> WalletError:
    """The library is missing or unreadable — a setup problem, not a wallet one."""
    return WalletError("io_error", message)
