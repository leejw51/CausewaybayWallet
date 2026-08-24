"""The Textual TUI, driven through Textual's test pilot."""

import pytest
from constants import TEST_ADDRESS_0, TEST_ADDRESS_1, TEST_MNEMONIC, TEST_PRIVATE_KEY

from causewaybay import store
from causewaybay.app import App
from causewaybay.tui import WalletTUI

# Every test here drives the Textual pilot, which is async.
pytestmark = pytest.mark.anyio


@pytest.fixture
def tui(home):
    """A TUI bound to an isolated wallet."""
    return WalletTUI(App(home=home))


async def type_into_prompt(pilot, text: str) -> None:
    """Fill the inline prompt and submit it."""
    from textual.widgets import Input

    field = pilot.app.query_one("#prompt", Input)
    field.value = text
    await pilot.pause()
    await field.action_submit()
    await pilot.pause()


async def test_starts_on_an_empty_wallet(tui):
    async with tui.run_test() as pilot:
        assert pilot.app.accounts == []
        assert pilot.app.current_account is None
        assert pilot.app.detail_rows == []


async def test_creating_an_account_updates_the_list_and_the_store(tui):
    async with tui.run_test() as pilot:
        await pilot.press("n")
        await type_into_prompt(pilot, "alpha")
        assert [a.label for a in pilot.app.accounts] == ["alpha"]
        assert pilot.app.wallet.store.accounts()[0].label == "alpha"
        assert "Created" in pilot.app.status_text


async def test_a_blank_label_gets_an_automatic_one(tui):
    async with tui.run_test() as pilot:
        await pilot.press("n")
        await type_into_prompt(pilot, "")
        assert pilot.app.accounts[0].label == "account-1"


async def test_importing_a_mnemonic(tui):
    async with tui.run_test() as pilot:
        await pilot.press("m")
        await type_into_prompt(pilot, TEST_MNEMONIC)
        assert pilot.app.accounts[0].address == TEST_ADDRESS_0


async def test_importing_a_bad_mnemonic_reports_an_error(tui):
    async with tui.run_test() as pilot:
        await pilot.press("m")
        await type_into_prompt(pilot, "not a mnemonic at all")
        assert pilot.app.accounts == []
        assert pilot.app.status_text.startswith("error:")


async def test_importing_a_private_key(tui):
    async with tui.run_test() as pilot:
        await pilot.press("p")
        await type_into_prompt(pilot, TEST_PRIVATE_KEY)
        assert pilot.app.accounts[0].address == TEST_ADDRESS_0
        assert pilot.app.accounts[0].source == store.SOURCE_PRIVATE_KEY


async def test_deriving_a_sibling_address(tui):
    async with tui.run_test() as pilot:
        await pilot.press("m")
        await type_into_prompt(pilot, TEST_MNEMONIC)
        pilot.app.dispatch("derive")
        await pilot.pause()
        await type_into_prompt(pilot, "1")
        assert len(pilot.app.accounts) == 2
        assert pilot.app.accounts[1].index == 1


async def test_deriving_with_a_non_numeric_index_is_reported(tui):
    async with tui.run_test() as pilot:
        await pilot.press("m")
        await type_into_prompt(pilot, TEST_MNEMONIC)
        pilot.app.dispatch("derive")
        await pilot.pause()
        await type_into_prompt(pilot, "twelve")
        assert len(pilot.app.accounts) == 1
        assert "not an address index" in pilot.app.status_text


async def test_secrets_stay_hidden_until_toggled(tui):
    async with tui.run_test() as pilot:
        await pilot.press("p")
        await type_into_prompt(pilot, TEST_PRIVATE_KEY)

        detail = dict(pilot.app.detail_rows)
        assert TEST_PRIVATE_KEY not in detail["Private key"]

        await pilot.press("v")
        detail = dict(pilot.app.detail_rows)
        assert detail["Private key"] == TEST_PRIVATE_KEY

        await pilot.press("v")
        assert dict(pilot.app.detail_rows)["Private key"] != TEST_PRIVATE_KEY


async def test_activating_an_account(tui):
    async with tui.run_test() as pilot:
        await pilot.press("n")
        await type_into_prompt(pilot, "one")
        await pilot.press("n")
        await type_into_prompt(pilot, "two")

        # The highlight follows the newly created account list; select the second.
        pilot.app.query_one("#accounts").index = 1
        await pilot.pause()
        await pilot.press("a")
        active = pilot.app.wallet.store.active_account()
        assert active.label == "two"


async def test_removing_needs_a_typed_confirmation(tui):
    async with tui.run_test() as pilot:
        await pilot.press("n")
        await type_into_prompt(pilot, "doomed")

        await pilot.press("x")
        await type_into_prompt(pilot, "no")
        assert len(pilot.app.accounts) == 1
        assert pilot.app.status_text == "Cancelled"

        await pilot.press("x")
        await type_into_prompt(pilot, "yes")
        assert pilot.app.accounts == []


async def test_signing_a_message_shows_the_signature(tui):
    from causewaybay import wallet

    async with tui.run_test() as pilot:
        await pilot.press("m")
        await type_into_prompt(pilot, TEST_MNEMONIC)
        await pilot.press("g")
        await type_into_prompt(pilot, "hello tui")

        detail = dict(pilot.app.detail_rows)
        assert wallet.recover_message("hello tui", detail["Signature"]) == TEST_ADDRESS_0


async def test_each_network_is_its_own_menu_entry(tui):
    """Flattened, and generated from the table so new chains appear on their own."""
    from causewaybay import networks

    async with tui.run_test() as pilot:
        entries = [c for c in pilot.app.commands if c.action.startswith("network:")]
        assert len(entries) == len(networks.ALL)
        assert [c.label for c in entries] == [n.name for n in networks.ALL]
        assert all(c.key is None for c in entries), "networks are menu-only"

        assert pilot.app.wallet.store.network().key == "cronos-testnet"
        pilot.app.dispatch("network:cronos-mainnet")
        await pilot.pause()
        assert pilot.app.wallet.store.network().key == "cronos-mainnet"
        # Selecting the one already in use says so rather than churning.
        pilot.app.dispatch("network:cronos-mainnet")
        await pilot.pause()
        assert "Already on" in pilot.app.status_text


async def test_reload_picks_up_changes_made_elsewhere(tui, home):
    async with tui.run_test() as pilot:
        # Another process (or the CLI) adds an account behind the TUI's back.
        App(home=home).account_import_mnemonic(TEST_MNEMONIC, label="from-cli")
        assert pilot.app.accounts == []
        await pilot.press("r")
        assert [a.label for a in pilot.app.accounts] == ["from-cli"]


async def test_actions_without_a_selection_are_reported_not_fatal(tui):
    async with tui.run_test() as pilot:
        for key in ("d", "a", "b", "s", "g", "x"):
            await pilot.press(key)
            assert "No wallet selected" in pilot.app.status_text


async def test_balance_reports_an_unreachable_node(tui, monkeypatch):
    monkeypatch.setenv("CAUSEWAYBAY_RPC_CRONOS_TESTNET", "http://127.0.0.1:1")
    async with tui.run_test() as pilot:
        await pilot.press("m")
        await type_into_prompt(pilot, TEST_MNEMONIC)
        await pilot.press("b")
        assert pilot.app.status_text.startswith("error:")


async def test_balance_shows_the_node_answer(tui, node, home):
    tui.wallet = App(home=home)
    async with tui.run_test() as pilot:
        await pilot.press("m")
        await type_into_prompt(pilot, TEST_MNEMONIC)
        await pilot.press("b")
        assert "Balance 10 TCRO" in pilot.app.status_text


async def test_sending_walks_through_recipient_amount_and_confirmation(tui, node, home):
    node.on("eth_sendRawTransaction", "0xtuihash")
    tui.wallet = App(home=home)
    async with tui.run_test() as pilot:
        await pilot.press("m")
        await type_into_prompt(pilot, TEST_MNEMONIC)

        await pilot.press("s")
        await type_into_prompt(pilot, TEST_ADDRESS_1)
        await type_into_prompt(pilot, "0.5")
        await type_into_prompt(pilot, "yes")

        assert pilot.app.status_text.startswith("Submitted 0x")
        history = pilot.app.wallet.store.history()
        assert len(history) == 1
        assert history[0].value == "0.5"


async def test_sending_rejects_a_bad_recipient(tui, node, home):
    tui.wallet = App(home=home)
    async with tui.run_test() as pilot:
        await pilot.press("m")
        await type_into_prompt(pilot, TEST_MNEMONIC)
        await pilot.press("s")
        await type_into_prompt(pilot, "0xnope")
        assert pilot.app.status_text.startswith("error:")
        assert pilot.app.wallet.store.history() == []


async def test_sending_can_be_declined_at_the_confirmation(tui, node, home):
    node.on("eth_sendRawTransaction", "0xtuihash")
    tui.wallet = App(home=home)
    async with tui.run_test() as pilot:
        await pilot.press("m")
        await type_into_prompt(pilot, TEST_MNEMONIC)
        await pilot.press("s")
        await type_into_prompt(pilot, TEST_ADDRESS_1)
        await type_into_prompt(pilot, "0.5")
        await type_into_prompt(pilot, "no")
        assert pilot.app.status_text == "Cancelled"
        assert pilot.app.wallet.store.history() == []


async def test_the_header_names_the_network_and_the_active_wallet(tui):
    async with tui.run_test() as pilot:
        assert "Cronos EVM Testnet" in pilot.app.title
        assert "no wallet yet" in pilot.app.title

        pilot.app.dispatch("network:cronos-mainnet")
        await pilot.pause()
        assert "Cronos EVM Mainnet" in pilot.app.title

        # Once there is a wallet, the header carries the address everything
        # defaults to.
        await pilot.press("m")
        await type_into_prompt(pilot, TEST_MNEMONIC)
        assert TEST_ADDRESS_0 in pilot.app.title


async def test_quitting_exits_cleanly(tui):
    async with tui.run_test() as pilot:
        await pilot.press("q")
        await pilot.pause()
    assert True  # reaching here means the app shut down without hanging


# ------------------------------- the surface brought over from the Rust TUI


async def test_the_command_pane_lists_everything_with_its_shortcut(tui):
    """Nothing has to be memorised: every action is a visible row."""
    async with tui.run_test() as pilot:
        actions = {c.action for c in pilot.app.commands}
        for expected in (
            "balance",
            "send",
            "new_address",
            "new_seed",
            "copy_address",
            "export_wallets",
            "save_jsonl",
            "save_csv",
            "save_txt",
            "save_md",
        ):
            assert expected in actions, expected
        # Shortcuts are unique among the commands that have one.
        keys = [c.key for c in pilot.app.commands if c.key]
        assert len(keys) == len(set(keys))
        # And every label fits the pane it is padded into.
        from causewaybay.tui import LABEL_WIDTH

        assert all(len(c.label) <= LABEL_WIDTH for c in pilot.app.commands)


async def test_new_address_walks_the_seed_and_new_seed_starts_over(tui):
    async with tui.run_test() as pilot:
        for _ in range(3):
            pilot.app.dispatch("new_address")
            await pilot.pause()
            await type_into_prompt(pilot, "")
        assert [a.index for a in pilot.app.accounts] == [0, 1, 2]
        seeds = {a.mnemonic for a in pilot.app.accounts}
        assert len(seeds) == 1, "one seed, three addresses"

        pilot.app.dispatch("new_seed")
        await pilot.pause()
        await type_into_prompt(pilot, "")
        assert pilot.app.accounts[-1].index == 0, "a separate seed starts again at 0"
        assert len({a.mnemonic for a in pilot.app.accounts}) == 2


async def test_copy_address_puts_it_on_the_clipboard(tui, monkeypatch):
    copied = {}
    monkeypatch.setattr(
        "causewaybay.clipboard.copy", lambda text: copied.setdefault("text", text) and "stub"
    )
    async with tui.run_test() as pilot:
        await pilot.press("m")
        await type_into_prompt(pilot, TEST_MNEMONIC)
        pilot.app.dispatch("copy_address")
        await pilot.pause()
        assert copied["text"] == TEST_ADDRESS_0
        assert "Copied" in pilot.app.status_text


async def test_a_pasted_mnemonic_is_never_echoed(tui):
    from causewaybay.tui import echo

    # The status line reports progress without the phrase itself.
    shown = echo("mnemonic", TEST_MNEMONIC)
    assert "abandon" not in shown
    assert "12 words" in shown
    assert echo("private_key", TEST_PRIVATE_KEY).endswith("(64 hex chars)")
    # Non-secret prompts still show what was typed.
    assert echo("label", "savings") == "savings"

    async with tui.run_test() as pilot:
        await pilot.press("m")
        await pilot.pause()
        field = pilot.app.query_one("#prompt")
        assert field.password, "the input widget masks it too"
        field.value = TEST_MNEMONIC
        await pilot.pause()
        assert TEST_MNEMONIC not in pilot.app.status_text


async def test_saving_the_wallet_list_writes_each_format(tui, tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    async with tui.run_test() as pilot:
        await pilot.press("m")
        await type_into_prompt(pilot, TEST_MNEMONIC)
        for action, name in [
            ("save_jsonl", "wallets.jsonl"),
            ("save_csv", "wallets.csv"),
            ("save_txt", "wallets.txt"),
            ("save_md", "wallets.md"),
        ]:
            pilot.app.dispatch(action)
            await pilot.pause()
            await type_into_prompt(pilot, name)
            written = (tmp_path / name).read_text()
            assert TEST_ADDRESS_0 in written, name
            assert TEST_PRIVATE_KEY not in written, f"{name} leaks the private key"


async def test_export_wallets_writes_keys_owner_only(tui, tmp_path, monkeypatch):
    import json
    import stat

    monkeypatch.chdir(tmp_path)
    async with tui.run_test() as pilot:
        await pilot.press("m")
        await type_into_prompt(pilot, TEST_MNEMONIC)
        pilot.app.dispatch("export_wallets")
        await pilot.pause()
        assert "PRIVATE KEYS" in pilot.app.status_text, "the prompt says what it writes"
        await type_into_prompt(pilot, "wallets-keys.jsonl")

    target = tmp_path / "wallets-keys.jsonl"
    row = json.loads(target.read_text().splitlines()[0])
    assert row["address"] == TEST_ADDRESS_0
    assert row["private_key"] == TEST_PRIVATE_KEY
    assert len(row["public_key_compressed"]) == 2 + 66
    assert len(row["public_key"]) == 2 + 128
    assert stat.S_IMODE(target.stat().st_mode) & 0o077 == 0
