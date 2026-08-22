"""End-to-end tests through the real CLI entry point (no network needed)."""

import json
import os
import stat
import subprocess
import sys

import pytest
from constants import TEST_ADDRESS_0, TEST_ADDRESS_1, TEST_MNEMONIC, TEST_PRIVATE_KEY

from causewaybay import cli


@pytest.fixture
def run(capsys, home):
    """Invoke the CLI in-process and return (exit code, stdout, stderr)."""

    def invoke(*args: str, stdin: str | None = None):
        if stdin is not None:
            import io

            sys.stdin = io.StringIO(stdin)
        try:
            code = cli.main(["--home", str(home), *args])
        finally:
            sys.stdin = sys.__stdin__
        captured = capsys.readouterr()
        return code, captured.out, captured.err

    return invoke


@pytest.fixture
def jrun(run):
    """Invoke with --json and return the parsed envelope."""

    def invoke(*args: str, stdin: str | None = None):
        code, out, _ = run("--json", *args, stdin=stdin)
        lines = [line for line in out.strip().splitlines() if line]
        assert len(lines) == 1, f"expected exactly one JSON line, got {out!r}"
        return code, json.loads(lines[0])

    return invoke


def data(jrun, *args, stdin=None):
    """Run a command expected to succeed and return its data."""
    code, envelope = jrun(*args, stdin=stdin)
    assert envelope["ok"] is True, envelope
    assert code == 0
    return envelope["data"]


def error_code(jrun, *args, stdin=None):
    """Run a command expected to fail and return its error code."""
    code, envelope = jrun(*args, stdin=stdin)
    assert envelope["ok"] is False, envelope
    assert code != 0
    return envelope["error"]["code"]


# ------------------------------------------------------------------- accounts


def test_a_fresh_wallet_reports_no_accounts(jrun):
    assert data(jrun, "account", "list") == []
    assert data(jrun, "info")["accounts"] == 0
    assert error_code(jrun, "account", "show") == "no_active_account"


def test_account_new_generates_a_usable_wallet(jrun):
    account = data(jrun, "account", "new", "--label", "alpha", "--words", "24")
    assert account["label"] == "alpha"
    assert account["source"] == "mnemonic"
    assert account["derivation_path"] == "m/44'/60'/0'/0/0"
    assert account["address"].startswith("0x") and len(account["address"]) == 42
    # The mnemonic is stored but not printed unless asked for.
    assert "mnemonic" not in account
    assert len(data(jrun, "account", "export", "alpha")["mnemonic"].split()) == 24


def test_account_new_can_reveal_its_mnemonic(jrun):
    account = data(jrun, "account", "new", "--show-secret")
    assert len(account["mnemonic"].split()) == 12
    assert account["private_key"].startswith("0x")


def test_importing_the_reference_mnemonic(jrun):
    account = data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")
    assert account["address"] == TEST_ADDRESS_0
    assert account["index"] == 0


def test_importing_at_an_index(jrun):
    account = data(
        jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-i", "1", "-l", "second"
    )
    assert account["address"] == TEST_ADDRESS_1


def test_a_passphrase_changes_the_derived_address(jrun):
    plain = data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "plain")
    salted = data(
        jrun,
        "account",
        "import-mnemonic",
        "-m",
        TEST_MNEMONIC,
        "-l",
        "salted",
        "--passphrase",
        "extra",
    )
    assert plain["address"] != salted["address"]


def test_importing_a_private_key(jrun):
    account = data(jrun, "account", "import-key", "-k", TEST_PRIVATE_KEY, "-l", "raw")
    assert account["address"] == TEST_ADDRESS_0
    assert account["source"] == "private_key"
    assert account["derivation_path"] is None


def test_secrets_can_arrive_on_stdin(jrun):
    account = data(jrun, "account", "import-mnemonic", "-m", "-", stdin=TEST_MNEMONIC)
    assert account["address"] == TEST_ADDRESS_0


def test_secrets_can_arrive_through_the_environment(jrun, monkeypatch):
    monkeypatch.setenv("CAUSEWAYBAY_PRIVATE_KEY", TEST_PRIVATE_KEY)
    assert data(jrun, "account", "import-key")["address"] == TEST_ADDRESS_0


def test_bad_key_material_is_rejected_with_a_specific_code(jrun):
    assert error_code(jrun, "account", "import-mnemonic", "-m", "clearly not one") == (
        "invalid_mnemonic"
    )
    assert error_code(jrun, "account", "import-key", "-k", "0xdeadbeef") == "invalid_private_key"


def test_labels_must_be_unique(jrun):
    data(jrun, "account", "new", "-l", "dup")
    assert error_code(jrun, "account", "new", "-l", "dup") == "duplicate_label"
    assert error_code(jrun, "account", "new", "-l", "DUP") == "duplicate_label"


def test_labels_are_auto_assigned(jrun):
    assert data(jrun, "account", "new")["label"] == "account-1"
    assert data(jrun, "account", "new")["label"] == "account-2"


def test_the_first_account_becomes_active(jrun):
    data(jrun, "account", "new", "-l", "first")
    data(jrun, "account", "new", "-l", "second")
    assert data(jrun, "account", "show")["label"] == "first"
    data(jrun, "account", "use", "second")
    assert data(jrun, "account", "show")["label"] == "second"
    assert data(jrun, "info")["active_account"] == "second"


def test_account_list_marks_the_active_account(jrun):
    data(jrun, "account", "new", "-l", "one")
    data(jrun, "account", "new", "-l", "two")
    data(jrun, "account", "use", "two")
    active = [a["label"] for a in data(jrun, "account", "list") if a["active"]]
    assert active == ["two"]


def test_accounts_resolve_by_id_label_and_address(jrun):
    account = data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")
    for selector in (account["id"], "main", TEST_ADDRESS_0, TEST_ADDRESS_0.lower()):
        assert data(jrun, "account", "show", selector)["address"] == TEST_ADDRESS_0
    assert error_code(jrun, "account", "show", "ghost") == "account_not_found"


def test_derive_creates_sibling_addresses(jrun):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")
    derived = data(jrun, "account", "derive", "-i", "1", "-l", "second")
    assert derived["address"] == TEST_ADDRESS_1
    assert derived["derivation_path"] == "m/44'/60'/0'/0/1"
    assert len(data(jrun, "account", "list")) == 2


def test_derive_refuses_without_a_mnemonic(jrun):
    data(jrun, "account", "import-key", "-k", TEST_PRIVATE_KEY, "-l", "raw")
    assert error_code(jrun, "account", "derive", "-i", "1") == "usage"


def test_rename_and_remove(jrun):
    data(jrun, "account", "new", "-l", "old")
    data(jrun, "account", "new", "-l", "keeper")
    data(jrun, "account", "rename", "old", "renamed")
    assert error_code(jrun, "account", "show", "old") == "account_not_found"
    data(jrun, "--yes", "account", "remove", "renamed")
    assert len(data(jrun, "account", "list")) == 1
    assert data(jrun, "account", "show")["label"] == "keeper"


def test_removal_needs_confirmation(jrun):
    data(jrun, "account", "new", "-l", "safe")
    assert error_code(jrun, "account", "remove", "safe") == "confirmation_required"
    assert len(data(jrun, "account", "list")) == 1


def test_show_hides_secrets_unless_asked(jrun):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")
    hidden = data(jrun, "account", "show", "main")
    assert "private_key" not in hidden
    assert "mnemonic" not in hidden
    assert hidden["public_key"].startswith("0x")

    shown = data(jrun, "account", "show", "main", "--secret")
    assert shown["private_key"] == TEST_PRIVATE_KEY
    assert shown["mnemonic"] == TEST_MNEMONIC


def test_human_output_truncates_the_private_key(run, jrun):
    data(jrun, "account", "import-key", "-k", TEST_PRIVATE_KEY, "-l", "raw")
    _, out, _ = run("account", "show", "raw")
    assert TEST_PRIVATE_KEY not in out
    assert "0x1ab42c" in out


# -------------------------------------------------------------------- network


def test_networks_can_be_listed_and_switched(jrun):
    assert [n["key"] for n in data(jrun, "network", "list")] == [
        "cronos-testnet",
        "cronos-mainnet",
    ]
    assert data(jrun, "network", "current")["chain_id"] == 338
    data(jrun, "network", "use", "mainnet")
    current = data(jrun, "network", "current")
    assert current["chain_id"] == 25
    assert current["symbol"] == "CRO"


def test_unknown_networks_are_rejected(jrun):
    assert error_code(jrun, "network", "use", "ethereum") == "unknown_network"
    assert error_code(jrun, "-n", "solana", "info") == "unknown_network"


def test_the_network_flag_is_per_invocation(jrun):
    assert data(jrun, "-n", "mainnet", "network", "current")["chain_id"] == 25
    assert data(jrun, "network", "current")["chain_id"] == 338


def test_rpc_urls_can_be_overridden_and_restored(jrun):
    data(jrun, "network", "set-rpc", "testnet", "http://localhost:8545")
    assert data(jrun, "network", "current")["rpc"] == "http://localhost:8545"
    data(jrun, "network", "set-rpc", "testnet", "")
    assert data(jrun, "network", "current")["rpc"] == "https://evm-t3.cronos.org"


# ----------------------------------------------------------------- signatures


def test_signing_and_verification_round_trip(jrun):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")
    signed = data(jrun, "sign", "hello causewaybay")
    assert len(signed["signature"]) == 2 + 130
    assert signed["address"] == TEST_ADDRESS_0

    verified = data(
        jrun,
        "verify",
        "--message",
        "hello causewaybay",
        "--signature",
        signed["signature"],
        "--address",
        TEST_ADDRESS_0,
    )
    assert verified["valid"] is True
    assert verified["recovered"] == TEST_ADDRESS_0


def test_verification_fails_for_the_wrong_signer_or_message(jrun):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")
    signature = data(jrun, "sign", "original")["signature"]
    assert not data(
        jrun,
        "verify",
        "--message",
        "original",
        "--signature",
        signature,
        "--address",
        TEST_ADDRESS_1,
    )["valid"]
    assert not data(
        jrun,
        "verify",
        "--message",
        "tampered",
        "--signature",
        signature,
        "--address",
        TEST_ADDRESS_0,
    )["valid"]


def test_a_message_can_be_signed_from_stdin(jrun):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")
    signed = data(jrun, "sign", "-", stdin="piped message\n")
    # The trailing newline a shell adds is stripped, so the message is exact.
    assert signed["message"] == "piped message"


# ---------------------------------------------------------------------- utils


def test_offline_utilities_produce_known_values(jrun):
    assert (
        data(jrun, "utils", "keccak", "hello")["keccak256"]
        == "0x1c8aff950685c2ed4bc3174f3472287b56d9517b9c948127319a09a7a36deac8"
    )
    assert data(jrun, "utils", "checksum", TEST_ADDRESS_0.lower())["address"] == TEST_ADDRESS_0
    assert data(jrun, "utils", "to-wei", "1.5")["value"] == "1500000000000000000"
    assert data(jrun, "utils", "from-wei", "1500000000000000000")["amount"] == "1.5"
    assert data(jrun, "utils", "to-wei", "1.5", "-d", "6")["value"] == "1500000"


def test_utils_reject_malformed_input(jrun):
    assert error_code(jrun, "utils", "to-wei", "1.2.3") == "invalid_amount"
    assert error_code(jrun, "utils", "to-wei", "0.1234567890123456789") == "invalid_amount"
    assert error_code(jrun, "utils", "checksum", "0x123") == "invalid_address"
    assert error_code(jrun, "utils", "from-wei", "not-a-number") == "invalid_amount"


def test_utils_can_mint_a_mnemonic_without_storing_it(jrun):
    generated = data(jrun, "utils", "new-mnemonic", "-w", "24")
    assert len(generated["mnemonic"].split()) == 24
    assert data(jrun, "account", "list") == []


def test_keccak_over_hex_bytes_differs_from_text(jrun):
    as_text = data(jrun, "utils", "keccak", "0xdeadbeef")["keccak256"]
    as_bytes = data(jrun, "utils", "keccak", "0xdeadbeef", "--hex")["keccak256"]
    assert as_text != as_bytes


# ----------------------------------------------------------------- behaviours


def test_history_starts_empty(jrun):
    assert data(jrun, "history") == []


def test_the_store_is_append_only_and_well_formed(jrun, home):
    account = data(jrun, "account", "new", "-l", "one")
    data(jrun, "account", "rename", "one", "two")
    data(jrun, "--yes", "account", "remove", "two")

    lines = [json.loads(line) for line in (home / "accounts.jsonl").read_text().splitlines()]
    assert [line["type"] for line in lines] == [
        "account.create",
        "account.rename",
        "account.delete",
    ]
    assert all(line["schema"] == 1 and line["id"] == account["id"] for line in lines)


def test_a_corrupt_line_does_not_break_the_wallet(jrun, home):
    data(jrun, "account", "new", "-l", "good")
    with (home / "accounts.jsonl").open("a") as handle:
        handle.write("{ this is not json\n")
    data(jrun, "account", "new", "-l", "also-good")
    assert [a["label"] for a in data(jrun, "account", "list")] == ["good", "also-good"]


def test_json_mode_writes_exactly_one_line_to_stdout(run):
    _, out, _ = run("--json", "info")
    assert len(out.strip().splitlines()) == 1
    json.loads(out.strip())


def test_human_mode_keeps_the_warning_off_stdout(run):
    _, out, err = run("info")
    assert "Educational wallet" in err
    assert "Educational wallet" not in out


def test_global_flags_work_in_either_position(run):
    before = run("--json", "info")[1]
    after = run("info", "--json")[1]
    assert json.loads(before)["ok"] is True
    assert json.loads(after)["ok"] is True


def test_exit_codes_distinguish_usage_from_runtime_errors(run, home):
    assert run("info")[0] == 0
    assert run("account", "show", "ghost")[0] == 1
    with pytest.raises(SystemExit) as excinfo:
        cli.main(["--home", str(home), "not-a-command"])
    assert excinfo.value.code == 2


def test_no_command_prints_help(run):
    code, out, _ = run()
    assert code == 0
    assert "COMMAND" in out


@pytest.mark.skipif(os.name == "nt", reason="POSIX permissions")
def test_stored_files_are_not_world_readable(jrun, home):
    data(jrun, "account", "new", "-l", "secret-holder")
    mode = stat.S_IMODE((home / "accounts.jsonl").stat().st_mode)
    assert mode & 0o077 == 0


# ----------------------------------------------- the installed console script


def test_the_console_script_runs(home):
    """The packaged entry point works, not just the in-process function."""
    result = subprocess.run(
        [sys.executable, "-m", "causewaybay", "--json", "--home", str(home), "info"],
        capture_output=True,
        text=True,
        check=True,
    )
    assert json.loads(result.stdout.strip())["ok"] is True


def test_help_and_version_succeed(home):
    for args in (["--help"], ["--version"], ["account", "--help"]):
        result = subprocess.run(
            [sys.executable, "-m", "causewaybay", *args], capture_output=True, text=True
        )
        assert result.returncode == 0, result.stderr
    listing = subprocess.run(
        [sys.executable, "-m", "causewaybay", "account", "--help"],
        capture_output=True,
        text=True,
    )
    assert "import-mnemonic" in listing.stdout


# ------------------------------ one seed, many addresses; and the exports


def test_new_addresses_continue_the_sequence_on_one_seed(jrun):
    indexes = [data(jrun, "account", "new", "-l", f"addr-{n}")["index"] for n in range(5)]
    assert indexes == [0, 1, 2, 3, 4], "address indexes must increase"
    # One mnemonic in the recall list, used five times.
    remembered = data(jrun, "recent", "list")
    assert len(remembered) == 1
    assert remembered[0]["uses"] == 5


def test_the_first_wallet_mints_a_seed_and_the_rest_reuse_it(jrun):
    first = data(jrun, "account", "new", "-l", "first")
    assert first["new_seed"] is True
    assert first["index"] == 0
    second = data(jrun, "account", "new", "-l", "second")
    assert second["new_seed"] is False
    assert second["index"] == 1


def test_new_addresses_continue_an_imported_mnemonic(jrun):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "imported")
    nxt = data(jrun, "account", "new", "-l", "next")
    assert nxt["new_seed"] is False
    assert nxt["index"] == 1
    assert nxt["address"] == TEST_ADDRESS_1


def test_new_seed_starts_its_own_sequence(jrun):
    data(jrun, "account", "new", "-l", "a0")
    data(jrun, "account", "new", "-l", "a1")
    fresh = data(jrun, "account", "new", "--new-seed", "-l", "b0")
    assert fresh["new_seed"] is True
    assert fresh["index"] == 0

    data(jrun, "account", "use", "b0")
    assert data(jrun, "account", "new", "-l", "b1")["index"] == 1
    data(jrun, "account", "use", "a0")
    assert data(jrun, "account", "new", "-l", "a2")["index"] == 2


def test_an_explicit_index_overrides_the_sequence(jrun):
    data(jrun, "account", "new", "-l", "zero")
    assert data(jrun, "account", "new", "-i", "9", "-l", "nine")["index"] == 9
    assert data(jrun, "account", "new", "-l", "ten")["index"] == 10


def test_the_wallet_list_renders_in_every_format(jrun):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")
    data(jrun, "account", "derive", "-i", "1", "-l", "second")

    for fmt in ("jsonl", "csv", "txt", "md"):
        result = data(jrun, "account", "list", "--format", fmt)
        assert result["format"] == fmt
        assert result["count"] == 2
        assert result["path"] is None
        content = result["content"]
        assert TEST_ADDRESS_0 in content and TEST_ADDRESS_1 in content
        assert TEST_PRIVATE_KEY not in content, f"{fmt} leaks the private key"
        # Public keys are not secrets, so they are there either way.
        assert "public_key_compressed" in content


def test_the_full_export_carries_the_address_and_all_three_keys(jrun):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")
    exported = data(jrun, "account", "list", "--format", "jsonl", "--secret")
    row = json.loads(exported["content"].splitlines()[0])

    assert row["address"] == TEST_ADDRESS_0
    assert row["private_key"] == TEST_PRIVATE_KEY
    assert len(row["public_key_compressed"]) == 2 + 66
    assert len(row["public_key"]) == 2 + 128
    assert row["public_key_compressed"][4:] == row["public_key"][2:66]

    # The same values `account show` reports, so the two agree.
    shown = data(jrun, "account", "show", "main")
    assert shown["public_key"] == row["public_key"]


def test_the_wallet_list_can_be_written_to_a_file(jrun, home):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")
    target = home / "wallets.csv"
    result = data(jrun, "account", "list", "--format", "csv", "-o", str(target))
    assert result["count"] == 1
    assert result["path"] == str(target)
    assert "content" not in result
    assert target.read_text().startswith("position,label,address,")


@pytest.mark.skipif(os.name == "nt", reason="POSIX permissions")
def test_a_saved_file_holding_secrets_is_not_world_readable(jrun, home):
    data(jrun, "account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")
    target = home / "secrets.csv"
    data(jrun, "account", "list", "--format", "csv", "--secret", "-o", str(target))
    assert stat.S_IMODE(target.stat().st_mode) & 0o077 == 0


def test_the_networks_are_named_for_the_evm_chains(jrun):
    names = [n["name"] for n in data(jrun, "network", "list")]
    assert names == ["Cronos EVM Testnet", "Cronos EVM Mainnet"]
