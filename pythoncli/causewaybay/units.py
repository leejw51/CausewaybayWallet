"""Decimal-string <-> integer conversions that never touch floating point."""

from __future__ import annotations

from . import errors

MAX_UINT256 = 2**256 - 1


def parse_units(amount: str, decimals: int = 18) -> int:
    """Parse a human decimal amount ("1.25") into its smallest unit."""
    text = str(amount).strip()
    if not text:
        raise errors.invalid_amount("amount is empty")
    if text.startswith("+"):
        text = text[1:]
    if text.startswith("-"):
        raise errors.invalid_amount("amount must not be negative")

    integer, _, fraction = text.partition(".")
    if not integer and not fraction:
        raise errors.invalid_amount(f"not a decimal number: {amount}")
    for part in (integer, fraction):
        if part and not part.isdigit():
            raise errors.invalid_amount(f"not a decimal number: {amount}")
    # ``isdigit`` accepts non-ASCII digits such as '١'; require plain ASCII.
    if not all(c in "0123456789" for c in integer + fraction):
        raise errors.invalid_amount(f"not a decimal number: {amount}")
    if len(fraction) > decimals:
        raise errors.invalid_amount(f"amount {amount} has more than {decimals} decimal places")

    value = int((integer or "0") + fraction.ljust(decimals, "0") or "0")
    if value > MAX_UINT256:
        raise errors.invalid_amount(f"amount {amount} does not fit in 256 bits")
    return value


def format_units(value: int, decimals: int = 18) -> str:
    """Render a smallest-unit integer as a decimal string, without trailing zeros."""
    # Amounts are unsigned; a negative here means a malformed value reached this
    # far, and rendering it would produce something like "0.00000000000000-1".
    if value < 0:
        raise errors.invalid_amount(f"value must not be negative: {value}")
    if value > MAX_UINT256:
        raise errors.invalid_amount(f"value does not fit in 256 bits: {value}")
    if decimals == 0:
        return str(value)
    digits = str(value).rjust(decimals + 1, "0")
    integer, fraction = digits[:-decimals], digits[-decimals:]
    fraction = fraction.rstrip("0")
    return f"{integer}.{fraction}" if fraction else integer


def parse_ether(amount: str) -> int:
    return parse_units(amount, 18)


def format_ether(value: int) -> str:
    return format_units(value, 18)


def parse_gwei(amount: str) -> int:
    return parse_units(amount, 9)


def format_gwei(value: int) -> str:
    return format_units(value, 9)


def parse_int(value: str) -> int:
    """Parse a plain integer, rejecting anything else."""
    text = str(value).strip()
    if not text or not all(c in "0123456789" for c in text):
        raise errors.invalid_amount(f"not an integer: {value}")
    return int(text)
