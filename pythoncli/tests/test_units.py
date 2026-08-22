"""Decimal <-> integer conversions."""

import pytest

from causewaybay import errors, units


@pytest.mark.parametrize(
    ("amount", "decimals", "expected"),
    [
        ("0", 18, 0),
        ("1", 18, 10**18),
        ("1.5", 18, 1_500_000_000_000_000_000),
        ("0.000000000000000001", 18, 1),
        (".5", 18, 500_000_000_000_000_000),
        ("1.", 18, 10**18),
        ("+2", 18, 2 * 10**18),
        ("  3  ", 18, 3 * 10**18),
        ("1.5", 6, 1_500_000),
        ("1", 0, 1),
        ("1.23456789", 9, 1_234_567_890),
        ("000012", 0, 12),
    ],
)
def test_parse_units(amount, decimals, expected):
    assert units.parse_units(amount, decimals) == expected


@pytest.mark.parametrize(
    ("value", "decimals", "expected"),
    [
        (0, 18, "0"),
        (10**18, 18, "1"),
        (1_500_000_000_000_000_000, 18, "1.5"),
        (1, 18, "0.000000000000000001"),
        (1_500_000, 6, "1.5"),
        (42, 0, "42"),
        (1_000_000_000, 9, "1"),
        (10**18 + 10**17, 18, "1.1"),
    ],
)
def test_format_units(value, decimals, expected):
    assert units.format_units(value, decimals) == expected


@pytest.mark.parametrize(
    "bad",
    ["", "   ", "abc", "1.2.3", "-1", "1e18", "0x10", "1,5", "١٢٣", "1 2"],
)
def test_parse_units_rejects_junk(bad):
    with pytest.raises(errors.WalletError) as excinfo:
        units.parse_units(bad, 18)
    assert excinfo.value.code == errors.INVALID_AMOUNT


def test_parse_units_rejects_excess_precision():
    with pytest.raises(errors.WalletError):
        units.parse_units("0.1234567", 6)
    # Exactly at the limit is fine.
    assert units.parse_units("0.123456", 6) == 123_456


def test_parse_units_rejects_overflow():
    with pytest.raises(errors.WalletError):
        units.parse_units("1" + "0" * 60, 18)


@pytest.mark.parametrize(
    "amount", ["0", "1", "0.5", "123456.789", "0.000000000000000001", "1000000"]
)
def test_round_trip_is_lossless(amount):
    assert units.format_ether(units.parse_ether(amount)) == amount


def test_ether_and_gwei_helpers():
    assert units.parse_ether("1") == 10**18
    assert units.format_ether(10**18) == "1"
    assert units.parse_gwei("5") == 5_000_000_000
    assert units.format_gwei(5_000_000_000) == "5"
    assert units.format_gwei(1) == "0.000000001"


def test_parse_int():
    assert units.parse_int("42") == 42
    assert units.parse_int("  7 ") == 7
    for bad in ["", "-1", "1.0", "0x10", "abc"]:
        with pytest.raises(errors.WalletError):
            units.parse_int(bad)


def test_max_uint256_is_representable():
    largest = units.MAX_UINT256
    assert units.parse_units(units.format_units(largest, 18), 18) == largest
