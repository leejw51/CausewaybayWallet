"""The JSON-RPC client, driven by the scripted mock node."""

import pytest

from causewaybay import errors
from causewaybay.rpc import RpcClient, field_int, parse_quantity, to_quantity


@pytest.mark.parametrize(
    ("value", "expected"), [(0, "0x0"), (1, "0x1"), (255, "0xff"), (1024, "0x400")]
)
def test_quantities_encode_minimally(value, expected):
    assert to_quantity(value) == expected


@pytest.mark.parametrize(
    ("text", "expected"),
    [("0x0", 0), ("0x", 0), ("0x152", 338), ("0x19", 25), ("0xde0b6b3a7640000", 10**18)],
)
def test_quantities_decode(text, expected):
    assert parse_quantity(text) == expected


@pytest.mark.parametrize("bad", [42, None, "0xnothex", {"a": 1}, []])
def test_junk_quantities_are_rejected(bad):
    with pytest.raises(errors.WalletError) as excinfo:
        parse_quantity(bad, "test")
    assert excinfo.value.code == errors.RPC_ERROR


def test_optional_object_fields():
    receipt = {"blockNumber": "0x2a", "status": "0x1", "broken": "0xzz"}
    assert field_int(receipt, "blockNumber") == 42
    assert field_int(receipt, "status") == 1
    assert field_int(receipt, "missing") is None
    assert field_int(receipt, "broken") is None
    assert field_int(None, "anything") is None


def test_reads_answer_from_the_node(node):
    client = RpcClient(node.url)
    assert client.chain_id() == 338
    assert client.block_number() == 123456
    assert client.gas_price() == 5_000_000_000
    assert client.get_balance("0xabc") == 10**19
    assert client.get_transaction_count("0xabc") == 3
    assert client.estimate_gas("0xabc", "0xdef", 0) == 0xCF08


def test_balance_queries_the_latest_block(node):
    RpcClient(node.url).get_balance("0xabc")
    assert node.requests_for("eth_getBalance")[0]["params"] == ["0xabc", "latest"]


def test_nonce_queries_the_pending_block(node):
    RpcClient(node.url).get_transaction_count("0xabc")
    assert node.requests_for("eth_getTransactionCount")[0]["params"][1] == "pending"


def test_estimate_gas_includes_call_data_only_when_present(node):
    client = RpcClient(node.url)
    client.estimate_gas("0xabc", "0xdef", 0)
    client.estimate_gas("0xabc", "0xdef", 0, b"\xa9\x05\x9c\xbb")
    calls = node.requests_for("eth_estimateGas")
    assert "data" not in calls[0]["params"][0]
    assert calls[1]["params"][0]["data"] == "0xa9059cbb"


def test_eth_call_returns_raw_bytes(node):
    node.on("eth_call", "0x" + "00" * 31 + "12")
    assert RpcClient(node.url).eth_call("0xtoken", b"\x31\x3c\xe5\x67")[-1] == 0x12


def test_eth_call_rejects_malformed_hex(node):
    node.on("eth_call", "0xnothex")
    with pytest.raises(errors.WalletError) as excinfo:
        RpcClient(node.url).eth_call("0xtoken", b"")
    assert excinfo.value.code == errors.RPC_ERROR


def test_send_raw_transaction_returns_the_hash(node):
    node.on("eth_sendRawTransaction", "0xabc123")
    assert RpcClient(node.url).send_raw_transaction(b"\x01\x02") == "0xabc123"
    assert node.requests_for("eth_sendRawTransaction")[0]["params"] == ["0x0102"]


def test_a_null_result_becomes_none(node):
    node.on("eth_getTransactionByHash", None).on("eth_getTransactionReceipt", None)
    client = RpcClient(node.url)
    assert client.get_transaction_by_hash("0xdead") is None
    assert client.get_transaction_receipt("0xdead") is None


def test_rpc_errors_are_reported_with_their_message(node):
    node.on_error("eth_getBalance", -32000, "node is having a day")
    with pytest.raises(errors.WalletError) as excinfo:
        RpcClient(node.url).get_balance("0xabc")
    assert excinfo.value.code == errors.RPC_ERROR
    assert "node is having a day" in excinfo.value.message


def test_insufficient_funds_gets_its_own_code(node):
    node.on_error("eth_sendRawTransaction", -32000, "insufficient funds for gas * price + value")
    with pytest.raises(errors.WalletError) as excinfo:
        RpcClient(node.url).send_raw_transaction(b"\x01")
    assert excinfo.value.code == errors.INSUFFICIENT_FUNDS


def test_an_unscripted_method_is_an_rpc_error(node):
    with pytest.raises(errors.WalletError) as excinfo:
        RpcClient(node.url).call("eth_somethingElse")
    assert excinfo.value.code == errors.RPC_ERROR


def test_an_unreachable_node_is_an_rpc_error_not_a_crash():
    client = RpcClient("http://127.0.0.1:1", timeout=2.0)
    with pytest.raises(errors.WalletError) as excinfo:
        client.chain_id()
    assert excinfo.value.code == errors.RPC_ERROR


def test_every_request_is_well_formed_json_rpc(node):
    client = RpcClient(node.url)
    client.chain_id()
    client.gas_price()
    for request in node.requests:
        assert request["jsonrpc"] == "2.0"
        assert isinstance(request["id"], int)
        assert isinstance(request["method"], str)
        assert isinstance(request["params"], list)
    # Ids are not reused within a session.
    ids = [r["id"] for r in node.requests]
    assert len(ids) == len(set(ids))


def test_wait_for_receipt_returns_immediately_when_present(node):
    node.on("eth_getTransactionReceipt", {"status": "0x1"})
    assert RpcClient(node.url).wait_for_receipt("0xabc", timeout=5.0) == {"status": "0x1"}


def test_wait_for_receipt_gives_up(node):
    node.on("eth_getTransactionReceipt", None)
    assert RpcClient(node.url).wait_for_receipt("0xabc", timeout=0.1) is None


@pytest.mark.parametrize("bad", ["0x-1", "0x1_0", "0x 1", "0x+1"])
def test_malformed_quantities_are_rejected_not_silently_parsed(bad):
    """`int(body, 16)` accepts signs and underscores; a JSON-RPC quantity is hex
    digits and nothing else, and Rust rejects the same inputs."""
    with pytest.raises(errors.WalletError) as excinfo:
        parse_quantity(bad, "eth_getBalance")
    assert excinfo.value.code == errors.RPC_ERROR


def test_a_transient_poll_failure_does_not_abandon_the_transaction(node):
    """The transaction is already broadcast by the time we poll, so reporting a
    single 502 as failure invites the user to send it twice."""
    node.on_sequence(
        "eth_getTransactionReceipt",
        [{"__error": {"code": -32000, "message": "bad gateway"}}, {"status": "0x1"}],
    )
    client = RpcClient(node.url)
    assert client.wait_for_receipt("0xabc", timeout=10.0) == {"status": "0x1"}


def test_a_persistent_poll_failure_names_the_transaction(node):
    node.on_error("eth_getTransactionReceipt", -32000, "bad gateway")
    with pytest.raises(errors.WalletError) as excinfo:
        RpcClient(node.url).wait_for_receipt("0xabc", timeout=0.2)
    assert "0xabc" in excinfo.value.message, "the hash has to be recoverable"
