"""End-to-end tests for the chain-facing commands, driven by the scripted node."""

import json

import pytest
from constants import TEST_ADDRESS_0, TEST_ADDRESS_1, TEST_MNEMONIC

from causewaybay import cli


@pytest.fixture
def jrun(capsys, home, node):
    """Invoke the CLI in-process against the mock node, returning the envelope."""

    def invoke(*args: str):
        cli.main(["--json", "--home", str(home), *args])
        out = capsys.readouterr().out.strip()
        lines = [line for line in out.splitlines() if line]
        assert len(lines) == 1, f"expected exactly one JSON line, got {out!r}"
        return json.loads(lines[0])

    return invoke


@pytest.fixture
def funded(jrun):
    """A wallet holding the reference account."""
    envelope = jrun("account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")
    assert envelope["ok"]
    return jrun


def data(jrun, *args):
    envelope = jrun(*args)
    assert envelope["ok"] is True, envelope
    return envelope["data"]


def error_code(jrun, *args):
    envelope = jrun(*args)
    assert envelope["ok"] is False, envelope
    return envelope["error"]["code"]


# ------------------------------------------------------------------- reads


def test_balance_is_reported_in_whole_tokens_and_wei(funded, node):
    balance = data(funded, "balance")
    assert balance["balance"] == "10"
    assert balance["balance_wei"] == "10000000000000000000"
    assert balance["symbol"] == "TCRO"
    assert balance["address"] == TEST_ADDRESS_0
    assert node.requests_for("eth_getBalance")[0]["params"] == [TEST_ADDRESS_0, "latest"]


def test_balance_accepts_an_explicit_address(funded):
    balance = data(funded, "balance", "-a", TEST_ADDRESS_1)
    assert balance["address"] == TEST_ADDRESS_1
    assert balance["account"] is None


def test_balance_rejects_a_malformed_address(funded):
    assert error_code(funded, "balance", "-a", "0xnope") == "invalid_address"


def test_the_symbol_follows_the_selected_network(funded):
    assert data(funded, "balance")["symbol"] == "TCRO"
    assert data(funded, "-n", "mainnet", "balance")["symbol"] == "CRO"


def test_nonce_uses_the_pending_block(funded, node):
    assert data(funded, "nonce")["nonce"] == 3
    assert node.requests_for("eth_getTransactionCount")[0]["params"][1] == "pending"


def test_gas_price_is_reported_in_gwei(funded):
    gas = data(funded, "gas-price")
    assert gas["gas_price_gwei"] == "5"
    assert gas["gas_price_wei"] == "5000000000"


def test_chain_info_flags_a_mismatch(funded):
    matching = data(funded, "chain-info")
    assert matching["reported_chain_id"] == 338
    assert matching["chain_id_matches"] is True
    assert matching["block_number"] == 123456

    mismatched = data(funded, "-n", "mainnet", "chain-info")
    assert mismatched["expected_chain_id"] == 25
    assert mismatched["chain_id_matches"] is False


def test_rpc_failures_surface_as_rpc_errors(funded, node):
    node.on_error("eth_getBalance", -32000, "node is having a day")
    assert error_code(funded, "balance") == "rpc_error"


def test_chain_commands_need_an_account_or_an_address(jrun):
    assert error_code(jrun, "balance") == "no_active_account"
    assert error_code(jrun, "nonce") == "no_active_account"
    # These two are account independent.
    assert data(jrun, "gas-price")["gas_price_gwei"] == "5"
    assert data(jrun, "chain-info")["block_number"] == 123456


def test_a_config_pinned_rpc_url_is_used(jrun, node, monkeypatch):
    monkeypatch.delenv("CAUSEWAYBAY_RPC_CRONOS_TESTNET")
    jrun("account", "import-mnemonic", "-m", TEST_MNEMONIC, "-l", "main")
    jrun("network", "set-rpc", "testnet", node.url)
    assert data(jrun, "balance")["balance"] == "10"


# -------------------------------------------------------------------- sends


@pytest.fixture
def sending(node):
    """A node that accepts a broadcast and reports a successful receipt."""
    node.on("eth_sendRawTransaction", "0xabc123").on(
        "eth_getTransactionReceipt",
        {"status": "0x1", "blockNumber": "0x2a", "gasUsed": "0x5208"},
    ).on(
        "eth_getTransactionByHash",
        {"hash": "0xabc123", "value": "0xde0b6b3a7640000", "nonce": "0x3"},
    )
    return node


def test_send_signs_locally_and_broadcasts(funded, sending):
    sent = data(funded, "--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "1")
    # The hash is computed from the signed transaction, not taken from the
    # node's reply — that is what lets the record be written before broadcast.
    tx_hash = sent["hash"]
    assert len(tx_hash) == 2 + 64 and tx_hash.startswith("0x")
    assert sent["from"] == TEST_ADDRESS_0
    assert sent["to"] == TEST_ADDRESS_1
    assert sent["value"] == "1"
    assert sent["value_wei"] == "1000000000000000000"
    assert sent["nonce"] == 3
    assert sent["gas_limit"] == 21000
    assert sent["chain_id"] == 338
    assert sent["status"] == "submitted"
    assert sent["explorer"] == f"https://explorer.cronos.org/testnet/tx/{tx_hash}"

    # The private key never leaves the machine: the node only sees signed bytes.
    broadcast = sending.requests_for("eth_sendRawTransaction")
    assert len(broadcast) == 1
    raw = broadcast[0]["params"][0]
    assert raw.startswith("0xf8"), "expected an RLP-encoded legacy transaction"


def test_send_records_the_transaction_in_history(funded, sending, home):
    data(funded, "--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "0.25")
    history = data(funded, "history")
    assert len(history) == 1
    assert history[0]["hash"].startswith("0x")
    assert history[0]["value"] == "0.25"
    assert history[0]["network"] == "cronos-testnet"

    log = [json.loads(line) for line in (home / "history.jsonl").read_text().splitlines()]
    assert log[0]["type"] == "tx.send"
    assert log[0]["schema"] == 1


def test_send_without_confirmation_is_refused(funded, sending):
    assert error_code(funded, "send", "--to", TEST_ADDRESS_1, "--amount", "1") == (
        "confirmation_required"
    )
    assert sending.requests_for("eth_sendRawTransaction") == []
    assert data(funded, "history") == []


def test_send_refuses_when_the_balance_is_too_low(funded, sending):
    sending.on("eth_getBalance", "0x2386f26fc10000")  # 0.01 ether
    assert error_code(funded, "--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "1") == (
        "insufficient_funds"
    )
    assert sending.requests_for("eth_sendRawTransaction") == []


def test_send_accounts_for_gas_when_checking_the_balance(funded, sending):
    # Exactly 1 ether: enough for the transfer, not enough for transfer + gas.
    sending.on("eth_getBalance", "0xde0b6b3a7640000")
    assert error_code(funded, "--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "1") == (
        "insufficient_funds"
    )


def test_send_honours_explicit_overrides(funded, sending):
    sent = data(
        funded,
        "--yes",
        "send",
        "--to",
        TEST_ADDRESS_1,
        "--amount",
        "0.1",
        "--nonce",
        "9",
        "--gas-limit",
        "30000",
        "--gas-price-gwei",
        "2",
    )
    assert sent["nonce"] == 9
    assert sent["gas_limit"] == 30000
    assert sent["gas_price_wei"] == "2000000000"
    # With both overrides supplied the node is not asked for a nonce or a price.
    assert sending.requests_for("eth_getTransactionCount") == []
    assert sending.requests_for("eth_gasPrice") == []


def test_send_waits_for_the_receipt_when_asked(funded, sending, home):
    sent = data(funded, "--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "0.1", "--wait")
    assert sent["status"] == "confirmed"
    assert sent["block_number"] == 42
    assert sent["gas_used"] == 21000
    assert data(funded, "history")[0]["status"] == "confirmed"

    log = [json.loads(line) for line in (home / "history.jsonl").read_text().splitlines()]
    assert log[-1]["type"] == "tx.update"


def test_a_reverted_transaction_is_recorded_as_failed(funded, sending):
    sending.on("eth_getTransactionReceipt", {"status": "0x0", "blockNumber": "0x2a"})
    sent = data(funded, "--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "0.1", "--wait")
    assert sent["status"] == "failed"


def test_send_validates_before_touching_the_node(funded, sending):
    assert error_code(funded, "--yes", "send", "--to", "0xnope", "--amount", "1") == (
        "invalid_address"
    )
    assert error_code(funded, "--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "one") == (
        "invalid_amount"
    )
    assert sending.requests_for("eth_sendRawTransaction") == []


def test_a_rejected_broadcast_surfaces_the_node_message(funded, sending):
    sending.on_error("eth_sendRawTransaction", -32000, "insufficient funds for gas * price + value")
    assert error_code(funded, "--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "1") == (
        "insufficient_funds"
    )


def test_the_chain_id_in_the_signature_follows_the_network(funded, sending):
    sending.on_sequence("eth_sendRawTransaction", ["0xhash1", "0xhash2"])
    data(funded, "--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "0.1")
    data(funded, "--yes", "-n", "mainnet", "send", "--to", TEST_ADDRESS_1, "--amount", "0.1")
    broadcasts = sending.requests_for("eth_sendRawTransaction")
    assert broadcasts[0]["params"][0] != broadcasts[1]["params"][0], (
        "EIP-155 makes a testnet signature unusable on mainnet"
    )


def test_history_can_be_filtered_and_limited(funded, sending):
    sending.on_sequence("eth_sendRawTransaction", ["0xhash1", "0xhash2"])
    data(funded, "--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "0.1")
    data(funded, "--yes", "-n", "mainnet", "send", "--to", TEST_ADDRESS_1, "--amount", "0.2")

    assert len(data(funded, "history")) == 2
    assert len(data(funded, "history", "--network", "mainnet")) == 1
    newest = data(funded, "history", "--limit", "1")
    assert len(newest) == 1
    assert newest[0]["network"] == "cronos-mainnet"


def test_resending_the_same_hash_folds_into_one_entry(funded, sending, home):
    # Two identical transfers at the same nonce sign to the same bytes, so the
    # hash repeats and the log folds them together — a real duplicate send.
    data(funded, "--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "0.1")
    data(funded, "--yes", "send", "--to", TEST_ADDRESS_1, "--amount", "0.1")
    assert len(data(funded, "history")) == 1
    # Both sends were still recorded on disk; only the replay deduplicates.
    # Each send writes two lines: the record before broadcast, then its status.
    assert len((home / "history.jsonl").read_text().strip().splitlines()) == 4


def test_tx_looks_a_transaction_up(funded, sending):
    looked_up = data(funded, "tx", "0xabc123")
    assert looked_up["status"] == "confirmed"
    assert looked_up["value"] == "1"
    assert looked_up["explorer"] == "https://explorer.cronos.org/testnet/tx/0xabc123"


def test_tx_reports_a_pending_transaction(funded, node):
    node.on("eth_getTransactionByHash", {"hash": "0xpending", "value": "0x0"}).on(
        "eth_getTransactionReceipt", None
    )
    assert data(funded, "tx", "0xpending")["status"] == "pending"


def test_tx_reports_an_unknown_hash_as_not_found(funded, node):
    node.on("eth_getTransactionByHash", None).on("eth_getTransactionReceipt", None)
    assert error_code(funded, "tx", "0xdeadbeef") == "not_found"


# ------------------------------------------------------------------- erc-20


def word(value: int) -> str:
    return "0x" + value.to_bytes(32, "big").hex()


def abi_string(text: str) -> str:
    body = text.encode()
    padded = body + b"\x00" * ((32 - len(body) % 32) % 32)
    return "0x" + ((32).to_bytes(32, "big") + len(body).to_bytes(32, "big") + padded).hex()


def test_erc20_info_decodes_metadata(funded, node):
    node.on_sequence(
        "eth_call", [abi_string("VVS Finance"), abi_string("VVS"), word(18), word(1_000_000)]
    )
    info = data(funded, "erc20", "info", "-t", TEST_ADDRESS_1)
    assert info["name"] == "VVS Finance"
    assert info["symbol"] == "VVS"
    assert info["decimals"] == 18
    assert info["total_supply_raw"] == "1000000"


def test_erc20_balance_scales_by_the_token_decimals(funded, node):
    node.on_sequence("eth_call", [word(6), abi_string("USDC"), word(1_500_000)])
    balance = data(funded, "erc20", "balance", "-t", TEST_ADDRESS_1)
    assert balance["decimals"] == 6
    assert balance["symbol"] == "USDC"
    assert balance["balance_raw"] == "1500000"
    assert balance["balance"] == "1.5"
    assert balance["address"] == TEST_ADDRESS_0


def test_erc20_send_encodes_a_transfer(funded, node):
    node.on_sequence("eth_call", [word(18), word(5 * 10**18)]).on(
        "eth_sendRawTransaction", "0xtokenhash"
    )
    sent = data(
        funded,
        "--yes",
        "erc20",
        "send",
        "-t",
        TEST_ADDRESS_1,
        "--to",
        TEST_ADDRESS_0,
        "--amount",
        "1.5",
    )
    assert sent["hash"] == "0xtokenhash"
    assert sent["token"] == TEST_ADDRESS_1
    assert sent["to"] == TEST_ADDRESS_0
    assert sent["value_wei"] == "1500000000000000000"
    # The calldata is a transfer() call carrying the recipient and the amount.
    raw = node.requests_for("eth_sendRawTransaction")[0]["params"][0]
    assert "a9059cbb" in raw
    assert TEST_ADDRESS_0[2:].lower() in raw


def test_erc20_send_refuses_when_the_token_balance_is_short(funded, node):
    node.on_sequence("eth_call", [word(18), word(10**17)])  # holds 0.1
    assert (
        error_code(
            funded,
            "--yes",
            "erc20",
            "send",
            "-t",
            TEST_ADDRESS_1,
            "--to",
            TEST_ADDRESS_0,
            "--amount",
            "1",
        )
        == "insufficient_funds"
    )


def test_erc20_send_needs_confirmation(funded, node):
    node.on_sequence("eth_call", [word(18), word(5 * 10**18)])
    assert (
        error_code(
            funded, "erc20", "send", "-t", TEST_ADDRESS_1, "--to", TEST_ADDRESS_0, "--amount", "1"
        )
        == "confirmation_required"
    )


def test_erc20_rejects_a_malformed_token_address(funded):
    assert error_code(funded, "erc20", "balance", "-t", "0xnope") == "invalid_address"


def test_erc20_reports_absurd_decimals_as_an_rpc_error(funded, node):
    node.on("eth_call", word(10**18))
    assert error_code(funded, "erc20", "balance", "-t", TEST_ADDRESS_1) == "rpc_error"
