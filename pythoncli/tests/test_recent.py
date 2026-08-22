"""The recall list: remembering mnemonics and private keys for reuse."""

import json
import os
import stat
import time

import pytest
from constants import TEST_ADDRESS_0, TEST_ADDRESS_1, TEST_MNEMONIC, TEST_PRIVATE_KEY

from causewaybay import cli, errors, store
from causewaybay.store import Store

# ------------------------------------------------------------------ the store


@pytest.fixture
def a_store(home):
    return Store(home)


def test_recall_starts_empty(a_store):
    assert a_store.recent() == []
    with pytest.raises(errors.WalletError) as excinfo:
        a_store.find_recent("1")
    assert excinfo.value.code == errors.NOT_FOUND


def test_remembering_the_same_secret_updates_one_entry(a_store):
    first = a_store.remember_secret("mnemonic", "abandon about", "0xabc", 12)
    assert first.uses == 1

    again = a_store.remember_secret("mnemonic", "abandon about", "0xabc", 12)
    assert again.id == first.id
    assert again.uses == 2
    assert again.first_seen_at == first.first_seen_at, "first sighting is preserved"

    entries = a_store.recent()
    assert len(entries) == 1, "a repeat must not duplicate the entry"
    assert entries[0].uses == 2
    # Both writes are on disk; only the replay folds them.
    assert len(a_store.recent_path.read_text().splitlines()) == 2


def test_the_same_material_under_different_kinds_stays_distinct(a_store):
    a_store.remember_secret("mnemonic", "shared", "0xa")
    a_store.remember_secret("private_key", "shared", "0xa")
    assert len(a_store.recent()) == 2


def test_recall_is_ordered_by_most_recent_use(a_store):
    a_store.remember_secret("mnemonic", "one", "0x1", 12)
    time.sleep(0.002)
    a_store.remember_secret("mnemonic", "two", "0x2", 12)
    assert a_store.recent()[0].secret == "two"

    time.sleep(0.002)
    a_store.remember_secret("mnemonic", "one", "0x1", 12)
    assert a_store.recent()[0].secret == "one"


def test_recall_resolves_by_id_position_and_address(a_store):
    entry = a_store.remember_secret("private_key", "0xdead", TEST_ADDRESS_0)
    for selector in (entry.id, "1", TEST_ADDRESS_0, TEST_ADDRESS_0.lower()):
        assert a_store.find_recent(selector).id == entry.id
    for bad in ("9", "nope", "  "):
        with pytest.raises(errors.WalletError):
            a_store.find_recent(bad)


def test_forgetting_and_clearing(a_store):
    first = a_store.remember_secret("mnemonic", "one", "0x1", 12)
    a_store.remember_secret("mnemonic", "two", "0x2", 12)

    a_store.forget_secret(first.id)
    assert len(a_store.recent()) == 1
    assert a_store.clear_recent() == 1
    assert a_store.recent() == []


def test_forgetting_is_append_only(a_store):
    entry = a_store.remember_secret("mnemonic", "one", "0x1", 12)
    before = a_store.recent_path.read_text()
    a_store.forget_secret(entry.id)
    assert a_store.recent_path.read_text().startswith(before)


def test_secret_ids_are_stable_and_kind_scoped():
    assert store.secret_id("mnemonic", "a b") == store.secret_id("mnemonic", " a b ")
    assert store.secret_id("mnemonic", "a b") != store.secret_id("private_key", "a b")
    assert store.secret_id("mnemonic", "a b") != store.secret_id("mnemonic", "a c")
    generated = store.secret_id("mnemonic", "a")
    assert generated.startswith("sec_")
    assert len(generated) == 4 + 16


def test_previews_identify_without_revealing(a_store):
    phrase = a_store.remember_secret("mnemonic", TEST_MNEMONIC, "0xabc", 12)
    assert phrase.preview == "abandon … about"
    assert TEST_MNEMONIC not in phrase.preview

    key = a_store.remember_secret("private_key", TEST_PRIVATE_KEY, "0xabc")
    assert key.preview.startswith("0x1ab42c")
    assert "b618bdea" not in key.preview


def test_public_view_hides_the_secret(a_store):
    entry = a_store.remember_secret("mnemonic", "one two", "0xabc", 2)
    assert "secret" not in entry.public_view()
    assert entry.secret_view()["secret"] == "one two"


def test_corrupt_recall_lines_are_skipped(a_store):
    a_store.remember_secret("mnemonic", "good", "0x1", 12)
    with a_store.recent_path.open("a") as handle:
        handle.write("not json\n")
        handle.write('{"schema":99,"type":"secret.remember","id":"sec_future"}\n')
    assert len(a_store.recent()) == 1


# -------------------------------------------------------------------- the CLI


@pytest.fixture
def jrun(capsys, home):
    def invoke(*args: str):
        code = cli.main(["--json", "--home", str(home), *args])
        out = capsys.readouterr().out.strip()
        return code, json.loads(out)

    return invoke


@pytest.fixture
def run(capsys, home):
    def invoke(*args: str):
        code = cli.main(["--home", str(home), *args])
        captured = capsys.readouterr()
        return code, captured.out, captured.err

    return invoke


def data(jrun, *args):
    code, envelope = jrun(*args)
    assert envelope["ok"] is True, envelope
    return envelope["data"]


def error_code(jrun, *args):
    code, envelope = jrun(*args)
    assert envelope["ok"] is False, envelope
    return envelope["error"]["code"]


def test_recall_starts_empty_and_says_so(jrun, run):
    assert data(jrun, "recent", "list") == []
    assert "Nothing remembered yet" in run("recent", "list")[1]
    assert error_code(jrun, "recent", "show") == "not_found"


def test_creating_and_importing_fills_the_recall_list(jrun):
    data(jrun, "account", "new", "-l", "generated")
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "imported")
    data(jrun, "account", "import-key", "-k", TEST_PRIVATE_KEY, "-l", "raw")

    entries = data(jrun, "recent", "list")
    assert len(entries) == 3
    kinds = [entry["kind"] for entry in entries]
    assert kinds.count("mnemonic") == 2
    assert kinds.count("private_key") == 1
    assert entries[0]["position"] == 1
    assert data(jrun, "info")["remembered"] == 3


def test_recall_hides_secrets_until_asked(jrun):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")

    listed = data(jrun, "recent", "list")
    assert "secret" not in listed[0]
    assert listed[0]["preview"] == "abandon … about"
    assert "secret" not in data(jrun, "recent", "show", "1")

    revealed = data(jrun, "recent", "show", "1", "--secret")
    assert revealed["secret"] == TEST_MNEMONIC
    assert revealed["word_count"] == 12


def test_human_output_never_prints_a_whole_secret_by_accident(run, jrun):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")
    assert TEST_MNEMONIC not in run("recent", "list")[1]
    assert TEST_MNEMONIC not in run("recent", "show", "1")[1]
    # Only the explicit --secret form reveals it.
    assert TEST_MNEMONIC in run("recent", "show", "1", "--secret")[1]


def test_reusing_material_bumps_the_counter(jrun):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "one")
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-i", "1", "-l", "two")
    entries = data(jrun, "recent", "list")
    assert len(entries) == 1
    assert entries[0]["uses"] == 2


def test_deriving_moves_the_parent_mnemonic_to_the_front(jrun):
    # The first account stays active, so name the parent explicitly.
    data(jrun, "account", "new", "-l", "other")
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")
    data(jrun, "account", "derive", "-i", "1", "--from", "main", "-l", "second")

    entries = data(jrun, "recent", "list")
    assert entries[0]["address"] == TEST_ADDRESS_0
    assert entries[0]["uses"] == 2


def test_recall_can_be_filtered_and_limited(jrun):
    data(jrun, "account", "new", "-l", "generated")
    data(jrun, "account", "import-key", "-k", TEST_PRIVATE_KEY, "-l", "raw")
    assert len(data(jrun, "recent", "list", "--kind", "mnemonic")) == 1
    assert len(data(jrun, "recent", "list", "--kind", "private-key")) == 1
    assert len(data(jrun, "recent", "list", "--limit", "1")) == 1


def test_import_recent_rebuilds_an_account(jrun):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "original")
    data(jrun, "--yes", "account", "remove", "original")
    assert data(jrun, "account", "list") == []
    # Removing the account leaves the material in the recall list.
    assert len(data(jrun, "recent", "list")) == 1

    restored = data(jrun, "account", "import-recent", "1", "-l", "restored")
    assert restored["address"] == TEST_ADDRESS_0
    assert restored["source"] == "mnemonic"


def test_import_recent_defaults_to_the_newest_entry(jrun):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "older")
    # A separate seed, so it becomes a distinct — and newer — recall entry.
    newest = data(jrun, "account", "new", "--new-seed", "-l", "newer")
    restored = data(jrun, "account", "import-recent", "-l", "copy")
    assert restored["address"] == newest["address"]


def test_import_recent_can_pick_another_index(jrun):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")
    derived = data(jrun, "account", "import-recent", "1", "-i", "1", "-l", "second")
    assert derived["address"] == TEST_ADDRESS_1


def test_import_recent_restores_a_private_key_entry(jrun):
    data(jrun, "account", "import-key", "-k", TEST_PRIVATE_KEY, "-l", "raw")
    data(jrun, "--yes", "account", "remove", "raw")
    restored = data(jrun, "account", "import-recent", "1", "-l", "back")
    assert restored["address"] == TEST_ADDRESS_0
    assert restored["source"] == "private_key"


def test_import_recent_reports_a_bad_selector(jrun):
    assert error_code(jrun, "account", "import-recent") == "not_found"
    data(jrun, "account", "new", "-l", "one")
    assert error_code(jrun, "account", "import-recent", "9") == "not_found"
    assert error_code(jrun, "account", "import-recent", "nope") == "not_found"


def test_forgetting_needs_confirmation(jrun):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")
    assert error_code(jrun, "recent", "forget", "1") == "confirmation_required"
    assert len(data(jrun, "recent", "list")) == 1

    data(jrun, "--yes", "recent", "forget", "1")
    assert data(jrun, "recent", "list") == []
    # The account itself is untouched.
    assert len(data(jrun, "account", "list")) == 1


def test_clearing_forgets_everything(jrun):
    data(jrun, "account", "new", "-l", "one")
    data(jrun, "account", "import-key", "-k", TEST_PRIVATE_KEY, "-l", "two")
    assert error_code(jrun, "recent", "clear") == "confirmation_required"
    assert data(jrun, "--yes", "recent", "clear")["forgotten"] == 2
    assert data(jrun, "recent", "list") == []
    # Clearing an empty list is a no-op that needs no confirmation.
    assert data(jrun, "recent", "clear")["forgotten"] == 0


def test_the_recall_log_is_append_only_and_well_formed(jrun, home):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")
    data(jrun, "--yes", "recent", "forget", "1")

    lines = [json.loads(line) for line in (home / "recent.jsonl").read_text().splitlines()]
    assert [line["type"] for line in lines] == ["secret.remember", "secret.forget"]
    assert all(line["schema"] == 1 and line["id"].startswith("sec_") for line in lines)


@pytest.mark.skipif(os.name == "nt", reason="POSIX permissions")
def test_the_recall_log_is_not_world_readable(jrun, home):
    data(jrun, "account", "new", "-l", "one")
    mode = stat.S_IMODE((home / "recent.jsonl").stat().st_mode)
    assert mode & 0o077 == 0, "remembered secrets must not be world readable"


# -------------------------------------------------------------------- the TUI


pytestmark_note = "TUI recall tests live below and use the Textual pilot."


@pytest.mark.anyio
async def test_the_tui_can_pick_a_remembered_mnemonic(home):
    from causewaybay.app import App
    from causewaybay.tui import WalletTUI

    wallet_app = App(home=home)
    wallet_app.account_import_mnemonic(TEST_MNEMONIC, label="original")
    wallet_app.store.delete_account(wallet_app.store.accounts()[0].id)

    async with WalletTUI(App(home=home)).run_test() as pilot:
        assert pilot.app.accounts == []
        await pilot.press("c")
        assert pilot.app.in_recall is True
        assert len(pilot.app.recall) == 1
        # Enter on the highlighted entry imports it.
        pilot.app.import_selected_recall()
        await pilot.pause()
        assert [a.address for a in pilot.app.accounts] == [TEST_ADDRESS_0]
        assert pilot.app.in_recall is False


@pytest.mark.anyio
async def test_the_tui_recall_pane_hides_secrets_until_toggled(home):
    from causewaybay.app import App
    from causewaybay.tui import WalletTUI

    App(home=home).account_import_mnemonic(TEST_MNEMONIC, label="main")

    async with WalletTUI(App(home=home)).run_test() as pilot:
        await pilot.press("c")
        assert dict(pilot.app.detail_rows)["Mnemonic"] == "abandon … about"
        await pilot.press("v")
        assert dict(pilot.app.detail_rows)["Mnemonic"] == TEST_MNEMONIC


@pytest.mark.anyio
async def test_the_tui_can_forget_a_remembered_entry(home):
    from causewaybay.app import App
    from causewaybay.tui import WalletTUI

    App(home=home).account_import_mnemonic(TEST_MNEMONIC, label="main")

    async with WalletTUI(App(home=home)).run_test() as pilot:
        await pilot.press("c")
        await pilot.press("x")
        assert pilot.app.recall == []
        assert pilot.app.wallet.store.recent() == []


@pytest.mark.anyio
async def test_the_tui_recall_pane_toggles_back(home):
    from causewaybay.app import App
    from causewaybay.tui import WalletTUI

    async with WalletTUI(App(home=home)).run_test() as pilot:
        await pilot.press("c")
        assert pilot.app.in_recall is True
        await pilot.press("c")
        assert pilot.app.in_recall is False


# --------------------------------------- fixes for the review findings


def test_a_passphrase_protected_entry_cannot_be_restored_without_it(jrun):
    """A phrase remembered with a passphrase names a wallet the phrase alone
    cannot reach; restoring without it used to hand back a different, unfunded
    address without saying so."""
    salted = data(
        jrun,
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "salted",
        "--passphrase",
        "hunter2",
    )
    assert salted["address"] != TEST_ADDRESS_0, "a passphrase moves the wallet"

    remembered = data(jrun, "recent", "list")
    assert remembered[0]["address"] == salted["address"]
    assert remembered[0]["has_passphrase"] is True

    assert error_code(jrun, "account", "import-recent", "1", "-l", "restored") == "usage"
    assert (
        error_code(
            jrun,
            "account",
            "import-recent",
            "1",
            "-l",
            "restored",
            "--passphrase",
            "wrong",
        )
        == "usage"
    )
    restored = data(
        jrun,
        "account",
        "import-recent",
        "1",
        "-l",
        "restored",
        "--passphrase",
        "hunter2",
    )
    assert restored["address"] == salted["address"]


def test_a_plain_entry_still_restores_without_a_passphrase(jrun):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "plain")
    assert data(jrun, "recent", "list")[0]["has_passphrase"] is False
    assert data(jrun, "account", "import-recent", "1", "-l", "copy")["address"] == TEST_ADDRESS_0


@pytest.mark.parametrize("bad", ["0", "+1", "1.0", "01x"])
def test_recall_positions_are_strictly_one_based(jrun, bad):
    """`recent forget 0` must not act on an entry nobody selected."""
    data(jrun, "account", "new", "-l", "one")
    assert error_code(jrun, "recent", "show", bad) == "not_found"
    assert len(data(jrun, "recent", "list")) == 1
