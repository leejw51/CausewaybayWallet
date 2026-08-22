"""The network table and RPC endpoint resolution."""

import dataclasses

import pytest

from causewaybay import errors, networks


def test_both_networks_are_present_with_the_right_chain_ids():
    assert networks.find("cronos-testnet").chain_id == 338
    assert networks.find("cronos-mainnet").chain_id == 25
    assert [n.key for n in networks.ALL] == ["cronos-testnet", "cronos-mainnet"]


@pytest.mark.parametrize("alias", ["testnet", "TESTNET", "  cronos_testnet  ", "t3"])
def test_testnet_aliases(alias):
    assert networks.find(alias).key == "cronos-testnet"


@pytest.mark.parametrize("alias", ["mainnet", "Cronos", "cronos_mainnet"])
def test_mainnet_aliases(alias):
    assert networks.find(alias).key == "cronos-mainnet"


def test_unknown_network_lists_the_known_ones():
    with pytest.raises(errors.WalletError) as excinfo:
        networks.find("ethereum")
    assert excinfo.value.code == errors.UNKNOWN_NETWORK
    assert "cronos-testnet" in excinfo.value.message


def test_only_testnet_is_flagged_as_testnet():
    assert networks.CRONOS_TESTNET.testnet is True
    assert networks.CRONOS_MAINNET.testnet is False
    assert networks.CRONOS_TESTNET.symbol == "TCRO"
    assert networks.CRONOS_MAINNET.symbol == "CRO"


def test_override_names_are_stable():
    assert networks.CRONOS_TESTNET.rpc_env_var == "CAUSEWAYBAY_RPC_CRONOS_TESTNET"
    assert networks.CRONOS_MAINNET.rpc_env_var == "CAUSEWAYBAY_RPC_CRONOS_MAINNET"
    assert networks.CRONOS_TESTNET.rpc_config_key == "rpc.cronos-testnet"


def test_rpc_resolution_order(monkeypatch):
    network = networks.CRONOS_TESTNET
    assert network.resolve_rpc(None) == network.default_rpc
    assert network.resolve_rpc("   ") == network.default_rpc
    assert network.resolve_rpc("http://configured") == "http://configured"
    # The environment wins over stored config.
    monkeypatch.setenv(network.rpc_env_var, "http://from-env")
    assert network.resolve_rpc("http://configured") == "http://from-env"


def test_explorer_links():
    assert networks.CRONOS_MAINNET.tx_url("0xabc") == "https://explorer.cronos.org/tx/0xabc"
    assert (
        networks.CRONOS_TESTNET.address_url("0xdef")
        == "https://explorer.cronos.org/testnet/address/0xdef"
    )


def test_networks_are_immutable():
    with pytest.raises(dataclasses.FrozenInstanceError):
        networks.CRONOS_TESTNET.chain_id = 1
