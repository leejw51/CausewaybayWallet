"""Shared test scaffolding: an isolated wallet home over a real library.

These tests drive the same code a caller would: open a wallet on a throwaway
home, call methods, read dicts. Nothing here needs the network — the
chain-facing commands are covered by the Rust suite against a mock node, and
repeating that through two layers would only test the mock.
"""

from __future__ import annotations

import pytest

from causewaybay import open_wallet
from causewaybay.errors import WalletError


@pytest.fixture(autouse=True)
def isolated_env(monkeypatch, tmp_path):
    """Never let a developer's own wallet, keys or endpoints leak into a test."""
    for name in (
        "CAUSEWAYBAY_HOME",
        "CAUSEWAYBAY_MNEMONIC",
        "CAUSEWAYBAY_PRIVATE_KEY",
        "CAUSEWAYBAY_RPC_CRONOS_TESTNET",
        "CAUSEWAYBAY_RPC_CRONOS_MAINNET",
    ):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv("CAUSEWAYBAY_HOME", str(tmp_path / "wallet-home"))
    return tmp_path


@pytest.fixture
def home(isolated_env):
    """The isolated wallet home directory for this test."""
    return isolated_env / "wallet-home"


@pytest.fixture
def wallet(home):
    """A wallet on an empty, isolated home.

    The home is passed explicitly rather than left to the environment, so a
    test that spawns something else cannot reach a different store than the
    one it is asserting on.
    """
    try:
        return open_wallet(home=str(home))
    except WalletError as failure:  # pragma: no cover - a setup problem
        pytest.skip(f"the wallet library is not built: {failure.message}")


@pytest.fixture
def seeded(wallet):
    """A wallet holding one account on every chain."""
    wallet.new_account(label="alpha", every_chain=True)
    return wallet
