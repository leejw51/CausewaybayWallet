"""The Python binding, over a real library and a real store.

What this suite is for is the *path*: these values travel through the C ABI and
ctypes before being compared, which is where a binding's own class of mistake
lives — a truncated string, a pointer freed twice, a flag that never made it
into the request.
"""

from __future__ import annotations

import pytest
from constants import TEST_MNEMONIC, TEST_PRIVATE_KEY

from causewaybay import COMMANDS, Wallet, open_wallet
from causewaybay.errors import WalletError

CHAINS = ("evm", "solana", "cardano", "midnight")


# ------------------------------------------------------------------- opening


def test_reports_what_it_loaded(wallet: Wallet):
    assert wallet.version()[0].isdigit()
    described = wallet.describe()
    assert described["name"] == "causewaybay-wallet"
    assert "unencrypted" in described["warning"]
    assert described["abi"] == 2


def test_the_error_vocabulary_comes_from_the_library(wallet: Wallet):
    codes = wallet.codes()
    for code in ("usage", "account_not_found", "confirmation_required", "internal"):
        assert code in codes
    assert "code_from_a_future_version" not in codes


def test_a_missing_library_is_an_io_error(tmp_path):
    with pytest.raises(WalletError) as caught:
        open_wallet(lib=str(tmp_path / "no-such-library.so"))
    assert caught.value.code == "io_error"
    assert "looked in" in caught.value.message


def test_a_fresh_home_starts_empty(wallet: Wallet):
    assert wallet.accounts() == []
    assert wallet.info()["accounts"] == 0
    with pytest.raises(WalletError) as caught:
        wallet.account()
    assert caught.value.code == "no_active_account"


# ------------------------------------------------------------------ coverage


def test_every_command_the_library_has_is_mapped_to_a_method(wallet: Wallet):
    """Adding a command in Rust and forgetting the Python method fails here."""
    unmapped = [c["name"] for c in wallet.commands() if c["name"] not in COMMANDS]
    assert unmapped == []


def test_every_mapped_method_exists(wallet: Wallet):
    for command, mapped in COMMANDS.items():
        if mapped is None:
            continue
        for name in (mapped,) if isinstance(mapped, str) else mapped:
            assert callable(getattr(Wallet, name, None)), f"{command} -> {name}"


def test_nothing_is_mapped_that_the_library_does_not_have(wallet: Wallet):
    known = {c["name"] for c in wallet.commands()}
    assert [name for name in COMMANDS if name not in known] == []


def test_only_the_terminal_ui_is_left_unexposed():
    assert [name for name, mapped in COMMANDS.items() if mapped is None] == ["tui"]


# ------------------------------------------------------------------ accounts


def test_creates_an_account_and_finds_it_again(wallet: Wallet):
    created = wallet.new_account(label="alpha")
    address = created[0]["address"] if isinstance(created, list) else created["address"]
    assert wallet.account("alpha")["address"] == address
    assert wallet.account()["label"] == "alpha", "the first account becomes active"


def test_one_wallet_is_one_index_on_every_chain(wallet: Wallet):
    """The claim the whole wallet rests on, through the binding."""
    wallet.new_account(every_chain=True)
    accounts = wallet.accounts()
    assert [a["chain"] for a in accounts] == list(CHAINS)
    assert {a["index"] for a in accounts} == {0}
    # One mnemonic, four addresses, each in its chain's own encoding.
    assert len({a["address"] for a in accounts}) == 4
    assert [a["label"] for a in accounts] == [f"account0-{chain}" for chain in CHAINS]


def test_labels_are_auto_assigned_from_the_index_and_chain(wallet: Wallet):
    wallet.new_account()
    wallet.new_account()
    assert [a["label"] for a in wallet.accounts()] == ["account0-evm", "account1-evm"]


def test_a_duplicate_label_is_refused(wallet: Wallet):
    wallet.new_account(label="dup")
    with pytest.raises(WalletError) as caught:
        wallet.new_account(label="dup")
    assert caught.value.code == "duplicate_label"


def test_importing_a_mnemonic_gives_the_known_address(wallet: Wallet):
    imported = wallet.import_mnemonic(TEST_MNEMONIC, label="known")
    account = imported[0] if isinstance(imported, list) else imported
    assert account["address"] == "0x9858EfFD232B4033E47d90003D41EC34EcaEda94"


def test_importing_a_private_key_gives_the_known_address(wallet: Wallet):
    imported = wallet.import_key(TEST_PRIVATE_KEY, label="raw")
    account = imported[0] if isinstance(imported, list) else imported
    assert account["address"] == "0x9858EfFD232B4033E47d90003D41EC34EcaEda94"
    assert account["source"] == "private_key"


def test_removing_an_account_asks_first(seeded: Wallet):
    with pytest.raises(WalletError) as caught:
        seeded.remove_account("alpha-evm")
    assert caught.value.code == "confirmation_required"
    seeded.remove_account("alpha-evm", yes=True)
    assert all(a["label"] != "alpha-evm" for a in seeded.accounts())


# -------------------------------------------------------------------- chains


def test_every_chain_describes_itself(wallet: Wallet):
    by_name = {c["chain"]: c for c in wallet.chains()}
    assert set(by_name) == set(CHAINS)
    assert by_name["solana"]["derivation_path"] == "m/44'/501'/0'/0'"
    assert by_name["cardano"]["derivation_path"] == "m/1852'/1815'/0'/0/0"
    # Capabilities are per chain, and are what a caller should branch on.
    assert by_name["evm"]["capabilities"]["tokens"] is True
    assert by_name["solana"]["capabilities"]["faucet"] is True
    assert by_name["cardano"]["capabilities"]["tokens"] is False


def test_the_command_surface_reports_the_same_chains(wallet: Wallet):
    assert {c["chain"] for c in wallet.chain_list()} == {c["chain"] for c in wallet.chains()}


def test_lists_every_chains_networks(wallet: Wallet):
    networks = wallet.networks()
    by_key = {n["key"]: n for n in networks}
    assert by_key["cronos-testnet"]["chain_id"] == 338
    assert by_key["cronos-mainnet"]["chain_id"] == 25
    # Only EVM has a chain id; the others say null rather than a number that
    # would mean something.
    assert by_key["solana-devnet"]["chain_id"] is None
    for chain in CHAINS:
        assert any(n["chain"] == chain for n in networks), chain


def test_switching_network_settles_the_chain_too(wallet: Wallet):
    wallet.use_network("solana-devnet")
    assert wallet.current_network()["chain"] == "solana"
    assert wallet.info()["chain"] == "solana"
    wallet.use_network("mainnet")
    assert wallet.current_network()["chain_id"] == 25


def test_an_unknown_network_is_named_in_the_error(wallet: Wallet):
    with pytest.raises(WalletError) as caught:
        wallet.use_network("ethereum")
    assert caught.value.code == "unknown_network"
    assert "ethereum" in caught.value.message


def test_a_per_call_chain_does_not_change_the_stored_one(wallet: Wallet):
    derived = wallet.derive(mnemonic=TEST_MNEMONIC, index=0, chain="solana")
    assert derived["chain"] == "solana"
    assert derived["derivation_path"] == "m/44'/501'/0'/0'"
    assert wallet.info()["chain"] == "evm", "the stored chain did not move"


# ------------------------------------------------------------------ requests


def test_argv_must_be_strings(wallet: Wallet):
    with pytest.raises(WalletError) as caught:
        wallet.envelope(["utils", "to-wei", 1.5])  # type: ignore[list-item]
    assert caught.value.code == "usage"
    assert "argv[2]" in caught.value.message


def test_a_failed_command_is_an_envelope_not_a_raise(wallet: Wallet):
    envelope = wallet.envelope(["account", "show", "nobody"])
    assert envelope["ok"] is False
    assert envelope["error"]["code"] == "account_not_found"


def test_text_is_what_the_cli_would_have_printed(seeded: Wallet):
    printed = seeded.text(["account", "list"])
    assert "alpha-evm" in printed
    assert "0x" in printed


# ------------------------------------------------------------------- offline


def test_the_offline_helpers_need_no_store(wallet: Wallet):
    assert wallet.keccak("")["keccak256"].startswith("0xc5d2460186f7")
    assert wallet.to_wei("1.5")["value"] == "1500000000000000000"
    assert wallet.from_wei("1500000000000000000")["amount"] == "1.5"
    assert wallet.validate_mnemonic(TEST_MNEMONIC)["valid"] is True


def test_secrets_are_only_in_the_export(seeded: Wallet):
    listed = seeded.accounts()[0]
    assert "private_key" not in listed
    exported = seeded.export_account("alpha-evm")
    assert exported["private_key"].startswith("0x")
    assert exported["mnemonic"]
