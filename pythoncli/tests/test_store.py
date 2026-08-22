"""The append-only JSONL store."""

import json
import os
import stat

import pytest
from constants import TEST_ADDRESS_0

from causewaybay import errors, store
from causewaybay.store import Store


@pytest.fixture
def a_store(home):
    return Store(home)


def add(a_store, label=None, address="0x1"):
    return a_store.create_account(
        address=address, source=store.SOURCE_PRIVATE_KEY, private_key="0xkey", label=label
    )


def test_creates_the_home_directory(a_store, home):
    assert home.is_dir()


@pytest.mark.skipif(os.name == "nt", reason="POSIX permissions")
def test_home_and_files_are_owner_only(a_store):
    assert stat.S_IMODE(a_store.home.stat().st_mode) == 0o700
    add(a_store, "a")
    mode = stat.S_IMODE(a_store.accounts_path.stat().st_mode)
    assert mode & 0o077 == 0, "no group or other access to key material"


def test_a_fresh_store_is_empty(a_store):
    assert a_store.accounts() == []
    assert a_store.config() == {}
    assert a_store.history() == []
    with pytest.raises(errors.WalletError) as excinfo:
        a_store.active_account()
    assert excinfo.value.code == errors.NO_ACTIVE_ACCOUNT


def test_accounts_replay_in_creation_order(a_store):
    for position, label in enumerate(("first", "second", "third")):
        add(a_store, label, address=f"0x{position}")
    assert [a.label for a in a_store.accounts()] == ["first", "second", "third"]


def test_auto_labels_fill_the_lowest_gap(a_store):
    assert add(a_store, address="0x1").label == "account-1"
    assert add(a_store, address="0x2").label == "account-2"
    third = add(a_store, address="0x3")
    assert third.label == "account-3"
    a_store.delete_account(third.id)
    assert add(a_store, address="0x4").label == "account-3"


def test_the_same_address_can_be_stored_twice_under_different_labels(a_store):
    first = add(a_store, "copy-one", "0xabc")
    second = add(a_store, "copy-two", "0xabc")
    assert first.id != second.id
    assert len(a_store.accounts()) == 2


def test_duplicate_labels_are_rejected_case_insensitively(a_store):
    add(a_store, "main")
    for clash in ("main", "MAIN", "Main"):
        with pytest.raises(errors.WalletError) as excinfo:
            add(a_store, clash, address="0x2")
        assert excinfo.value.code == errors.DUPLICATE_LABEL


@pytest.mark.parametrize("bad", ["", "has space", "quote'", "0xdeadbeef", "x" * 65, "semi;colon"])
def test_hostile_labels_are_rejected(a_store, bad):
    with pytest.raises(errors.WalletError):
        add(a_store, bad)


def test_accounts_resolve_by_id_label_and_address(a_store):
    account = add(a_store, "main", TEST_ADDRESS_0)
    for selector in (account.id, "main", "MAIN", TEST_ADDRESS_0, TEST_ADDRESS_0.lower()):
        assert a_store.find_account(selector).id == account.id
    with pytest.raises(errors.WalletError) as excinfo:
        a_store.find_account("nope")
    assert excinfo.value.code == errors.ACCOUNT_NOT_FOUND
    with pytest.raises(errors.WalletError):
        a_store.find_account("  ")


def test_rename_appends_rather_than_rewrites(a_store):
    account = add(a_store, "old")
    a_store.rename_account(account.id, "new")
    assert a_store.find_account("new").id == account.id
    with pytest.raises(errors.WalletError):
        a_store.find_account("old")
    assert len(a_store.accounts_path.read_text().splitlines()) == 2


def test_rename_rejects_a_taken_label_but_allows_a_self_rename(a_store):
    first = add(a_store, "a")
    add(a_store, "b", "0x2")
    with pytest.raises(errors.WalletError) as excinfo:
        a_store.rename_account(first.id, "b")
    assert excinfo.value.code == errors.DUPLICATE_LABEL
    a_store.rename_account(first.id, "a")  # no-op, not a conflict


def test_delete_removes_the_account_and_repoints_active(a_store):
    first = add(a_store, "a")
    second = add(a_store, "b", "0x2")
    a_store.config_set(store.KEY_ACTIVE_ACCOUNT, first.id)
    a_store.delete_account(first.id)

    assert len(a_store.accounts()) == 1
    assert a_store.active_account().id == second.id


def test_deleting_the_last_account_clears_the_pointer(a_store):
    only = add(a_store, "a")
    a_store.config_set(store.KEY_ACTIVE_ACCOUNT, only.id)
    a_store.delete_account(only.id)
    assert a_store.config_get(store.KEY_ACTIVE_ACCOUNT) is None


def test_a_stale_active_pointer_falls_back_to_the_first_account(a_store):
    first = add(a_store, "a")
    a_store.config_set(store.KEY_ACTIVE_ACCOUNT, "acc_doesnotexist")
    assert a_store.active_account().id == first.id


def test_account_ids_are_stable_and_distinct():
    assert store.account_id("0xABC", "t", "l") == store.account_id("0xabc", "t", "l")
    assert store.account_id("0xabc", "t1", "l") != store.account_id("0xabc", "t2", "l")
    # Same address and instant, different label — must not collide.
    assert store.account_id("0xabc", "t", "one") != store.account_id("0xabc", "t", "two")
    generated = store.account_id("0xabc", "t", "l")
    assert generated.startswith("acc_")
    assert len(generated) == 4 + 16


def test_config_last_write_wins_and_an_empty_value_clears(a_store):
    assert a_store.config_get("k") is None
    a_store.config_set("k", "one")
    a_store.config_set("k", "two")
    assert a_store.config_get("k") == "two"
    a_store.config_set("k", "")
    assert a_store.config_get("k") is None


def test_network_defaults_to_testnet_and_survives_garbage(a_store):
    assert a_store.network().key == "cronos-testnet"
    a_store.config_set(store.KEY_NETWORK, "cronos-mainnet")
    assert a_store.network().key == "cronos-mainnet"
    a_store.config_set(store.KEY_NETWORK, "who-knows")
    assert a_store.network().key == "cronos-testnet"


def sample_tx(tx_hash="0xaa"):
    return store.TxRecord(
        hash=tx_hash,
        from_address="0xfrom",
        to="0xto",
        value="1",
        value_wei="1000000000000000000",
        network="cronos-testnet",
        chain_id=338,
        nonce=0,
        gas_limit=21000,
        gas_price_wei="5000000000",
        status="submitted",
        created_at=store.now_rfc3339(),
    )


def test_history_records_and_updates_transactions(a_store):
    a_store.record_tx(sample_tx("0xAA"))
    a_store.record_tx(sample_tx("0xbb"))
    assert len(a_store.history()) == 2

    # Updates match case-insensitively on the hash.
    a_store.update_tx("0xaa", "confirmed", 42, 21000)
    first, second = a_store.history()
    assert (first.status, first.block_number, first.gas_used) == ("confirmed", 42, 21000)
    assert second.status == "submitted"


def test_an_update_for_an_unknown_hash_is_harmless(a_store):
    a_store.update_tx("0xzz", "confirmed")
    assert a_store.history() == []


def test_a_repeated_hash_folds_into_one_entry(a_store):
    a_store.record_tx(sample_tx("0xaa"))
    a_store.record_tx(sample_tx("0xaa"))
    assert len(a_store.history()) == 1
    # Both writes are still on disk; only the replay deduplicates.
    assert len(a_store.history_path.read_text().splitlines()) == 2


def test_corrupt_lines_are_skipped_not_fatal(a_store):
    add(a_store, "good")
    with a_store.accounts_path.open("a") as handle:
        handle.write("this is not json\n")
        handle.write("[1, 2, 3]\n")
        handle.write('{"type": "account.create"}\n')
        handle.write("\n")
    add(a_store, "also-good", "0x2")
    assert [a.label for a in a_store.accounts()] == ["good", "also-good"]


def test_records_from_a_newer_schema_are_ignored(a_store):
    add(a_store, "known")
    future = {
        "schema": 99,
        "type": "account.create",
        "id": "acc_future",
        "label": "future",
        "address": "0x2",
        "source": "private_key",
        "private_key": "0xk",
        "created_at": "now",
    }
    with a_store.accounts_path.open("a") as handle:
        handle.write(json.dumps(future) + "\n")
    assert len(a_store.accounts()) == 1


def test_files_stay_append_only(a_store):
    account = add(a_store, "a")
    after_create = a_store.accounts_path.read_text()
    a_store.rename_account(account.id, "b")
    a_store.delete_account(account.id)
    after_delete = a_store.accounts_path.read_text()
    assert after_delete.startswith(after_create), "earlier lines must never change"
    assert len(after_delete.splitlines()) == 3


def test_every_line_is_one_compact_json_object(a_store):
    add(a_store, "a")
    a_store.config_set("k", "v")
    a_store.record_tx(sample_tx())
    for path in (a_store.accounts_path, a_store.config_path, a_store.history_path):
        content = path.read_text()
        assert content.endswith("\n")
        for line in content.splitlines():
            record = json.loads(line)
            assert record["schema"] == store.SCHEMA
            assert "type" in record
            assert ", " not in line, "records must be compact"


def test_public_view_hides_what_secret_view_shows(a_store):
    account = a_store.create_account(
        address="0xabc",
        source=store.SOURCE_MNEMONIC,
        private_key="0xdeadbeef",
        label="m",
        mnemonic="word word",
        derivation_path="m/44'/60'/0'/0/0",
        index=0,
    )
    public = account.public_view()
    assert "private_key" not in public
    assert "mnemonic" not in public
    secret = account.secret_view()
    assert secret["private_key"] == "0xdeadbeef"
    assert secret["mnemonic"] == "word word"


def test_two_stores_over_one_home_see_each_other(home):
    first = Store(home)
    second = Store(home)
    add(first, "from-first")
    assert second.accounts()[0].label == "from-first"


def test_tx_record_uses_the_wire_field_name(a_store):
    a_store.record_tx(sample_tx())
    record = json.loads(a_store.history_path.read_text().splitlines()[0])
    assert record["from"] == "0xfrom"
    assert "from_address" not in record


def test_timestamps_are_rfc3339_utc():
    stamp = store.now_rfc3339()
    assert stamp.endswith("Z")
    assert "T" in stamp
    assert len(stamp) == len("2026-01-01T00:00:00.000Z")


def test_repr_never_contains_secrets(a_store):
    """A stray print, log line or traceback must never leak key material."""
    account = a_store.create_account(
        address="0xabc",
        source=store.SOURCE_MNEMONIC,
        private_key="0xdeadbeefdeadbeef",
        label="m",
        mnemonic="correct horse battery staple",
        derivation_path="m/44'/60'/0'/0/0",
        index=0,
    )
    shown = repr(account)
    assert "deadbeef" not in shown
    assert "correct horse" not in shown
    assert "0xabc" in shown, "the address still identifies it"

    entry = a_store.remember_secret("mnemonic", "correct horse battery staple", "0xabc", 4)
    assert "battery staple" not in repr(entry)


@pytest.mark.skipif(os.name == "nt", reason="POSIX permissions")
def test_the_store_file_is_born_owner_only(a_store):
    """No window in which the first private key sits behind the umask."""
    # Pre-create the file world-readable; the store must tighten it.
    a_store.accounts_path.touch()
    a_store.accounts_path.chmod(0o644)
    add(a_store, "a")
    assert stat.S_IMODE(a_store.accounts_path.stat().st_mode) & 0o077 == 0


def test_a_wrong_typed_field_is_skipped_not_fatal(a_store):
    """Rust's replay skips a malformed line; Python used to build a broken
    Account from it and then crash on every later command."""
    add(a_store, "good")
    with a_store.accounts_path.open("a") as handle:
        handle.write(
            json.dumps(
                {
                    "schema": 1,
                    "type": "account.create",
                    "id": "acc_bad",
                    "label": 5,  # a number where a string belongs
                    "address": "0x2",
                    "source": "private_key",
                    "private_key": "0xk",
                    "created_at": "now",
                }
            )
            + "\n"
        )
    add(a_store, "also-good", "0x3")
    assert [a.label for a in a_store.accounts()] == ["good", "also-good"]
    # And the commands that walk labels still work.
    assert a_store.find_account("also-good").label == "also-good"
