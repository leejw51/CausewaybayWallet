"""The supported Cronos EVM networks and RPC endpoint resolution."""

from __future__ import annotations

import os
from dataclasses import dataclass

from . import errors


@dataclass(frozen=True)
class Network:
    key: str
    name: str
    chain_id: int
    symbol: str
    default_rpc: str
    explorer: str
    testnet: bool

    @property
    def rpc_env_var(self) -> str:
        """The environment variable that overrides this network's RPC URL."""
        return "CAUSEWAYBAY_RPC_" + self.key.upper().replace("-", "_")

    @property
    def rpc_config_key(self) -> str:
        """The ``config.jsonl`` key that overrides this network's RPC URL."""
        return f"rpc.{self.key}"

    def resolve_rpc(self, configured: str | None = None) -> str:
        """Environment beats stored config, which beats the built-in default."""
        from_env = os.environ.get(self.rpc_env_var, "").strip()
        if from_env:
            return from_env
        if configured and configured.strip():
            return configured.strip()
        return self.default_rpc

    def tx_url(self, tx_hash: str) -> str:
        return f"{self.explorer}/tx/{tx_hash}"

    def address_url(self, address: str) -> str:
        return f"{self.explorer}/address/{address}"


CRONOS_TESTNET = Network(
    key="cronos-testnet",
    name="Cronos EVM Testnet",
    chain_id=338,
    symbol="TCRO",
    default_rpc="https://evm-t3.cronos.org",
    explorer="https://explorer.cronos.org/testnet",
    testnet=True,
)

CRONOS_MAINNET = Network(
    key="cronos-mainnet",
    name="Cronos EVM Mainnet",
    chain_id=25,
    symbol="CRO",
    default_rpc="https://evm.cronos.org",
    explorer="https://explorer.cronos.org",
    testnet=False,
)

ALL = (CRONOS_TESTNET, CRONOS_MAINNET)
DEFAULT_NETWORK = CRONOS_TESTNET.key

_ALIASES = {
    "testnet": "cronos-testnet",
    "cronos-testnet": "cronos-testnet",
    "cronostestnet": "cronos-testnet",
    "t3": "cronos-testnet",
    "mainnet": "cronos-mainnet",
    "cronos-mainnet": "cronos-mainnet",
    "cronosmainnet": "cronos-mainnet",
    "cronos": "cronos-mainnet",
}


def find(key: str) -> Network:
    """Look a network up by key, accepting a few friendly aliases."""
    normalized = str(key).strip().lower().replace("_", "-")
    canonical = _ALIASES.get(normalized, normalized)
    for network in ALL:
        if network.key == canonical:
            return network
    known = ", ".join(n.key for n in ALL)
    raise errors.unknown_network(f"unknown network '{key}'; known networks: {known}")
