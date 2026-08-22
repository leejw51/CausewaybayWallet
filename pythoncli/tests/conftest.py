"""Shared test scaffolding: an isolated wallet home and a scripted JSON-RPC node."""

from __future__ import annotations

import json
import threading
from collections import defaultdict, deque
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Any

import pytest
from constants import TEST_MNEMONIC

from causewaybay.app import App


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
def app(home):
    """An ``App`` on an empty, isolated wallet."""
    return App(home=home)


_MISSING = object()


class MockRpc:
    """A tiny JSON-RPC server that answers from a scripted response table."""

    def __init__(self) -> None:
        self.responses: dict[str, Any] = {}
        self.queued: dict[str, deque] = defaultdict(deque)
        self.requests: list[dict[str, Any]] = []
        self._server = HTTPServer(("127.0.0.1", 0), self._make_handler())
        self.url = f"http://127.0.0.1:{self._server.server_address[1]}"
        # A short poll interval keeps `close()` from adding half a second per test.
        self._thread = threading.Thread(
            target=self._server.serve_forever, kwargs={"poll_interval": 0.02}, daemon=True
        )
        self._thread.start()

    def _make_handler(self):
        node = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):  # keep the test output quiet
                pass

            def do_POST(self):
                length = int(self.headers.get("content-length", 0))
                body = self.rfile.read(length).decode("utf-8")
                try:
                    payload = json.loads(body)
                except json.JSONDecodeError:
                    payload = {}
                node.requests.append(payload)

                method = payload.get("method", "")
                request_id = payload.get("id", 1)
                # A scripted ``None`` is a valid JSON-RPC null result, so
                # presence is tracked with a sentinel rather than by value.
                scripted = _MISSING
                if node.queued.get(method):
                    scripted = node.queued[method].popleft()
                elif method in node.responses:
                    scripted = node.responses[method]

                if scripted is _MISSING:
                    response = {
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "error": {"code": -32601, "message": f"method {method} not scripted"},
                    }
                elif isinstance(scripted, dict) and "__error" in scripted:
                    response = {"jsonrpc": "2.0", "id": request_id, "error": scripted["__error"]}
                else:
                    response = {"jsonrpc": "2.0", "id": request_id, "result": scripted}

                encoded = json.dumps(response).encode("utf-8")
                self.send_response(200)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(encoded)))
                self.end_headers()
                self.wfile.write(encoded)

        return Handler

    # ------------------------------------------------------------- scripting

    def on(self, method: str, result: Any) -> MockRpc:
        """Script a successful result for a method."""
        self.responses[method] = result
        return self

    def on_sequence(self, method: str, results: list[Any]) -> MockRpc:
        """Script consecutive results, consumed one per call."""
        self.queued[method] = deque(results)
        return self

    def on_error(self, method: str, code: int, message: str) -> MockRpc:
        """Script a JSON-RPC error for a method."""
        self.responses[method] = {"__error": {"code": code, "message": message}}
        return self

    def with_defaults(self) -> MockRpc:
        """Answer every read the wallet normally makes."""
        return (
            self.on("eth_chainId", "0x152")  # 338
            .on("eth_blockNumber", "0x1e240")  # 123456
            .on("eth_gasPrice", "0x12a05f200")  # 5 gwei
            .on("eth_getBalance", "0x8ac7230489e80000")  # 10 ether
            .on("eth_getTransactionCount", "0x3")
            .on("eth_estimateGas", "0xcf08")
        )

    def requests_for(self, method: str) -> list[dict[str, Any]]:
        """Every request body recorded for one method."""
        return [r for r in self.requests if r.get("method") == method]

    def close(self) -> None:
        self._server.shutdown()
        self._server.server_close()
        self._thread.join(timeout=5)


@pytest.fixture
def node(monkeypatch):
    """A scripted RPC node wired into both networks for the duration of a test."""
    server = MockRpc().with_defaults()
    monkeypatch.setenv("CAUSEWAYBAY_RPC_CRONOS_TESTNET", server.url)
    monkeypatch.setenv("CAUSEWAYBAY_RPC_CRONOS_MAINNET", server.url)
    try:
        yield server
    finally:
        server.close()


@pytest.fixture
def funded_app(home, node):
    """An ``App`` holding the reference account, pointed at the mock node."""
    wallet_app = App(home=home)
    wallet_app.account_import_mnemonic(TEST_MNEMONIC, label="main")
    return wallet_app


@pytest.fixture
def anyio_backend():
    """Textual's pilot runs on asyncio; anyio would otherwise also try trio."""
    return "asyncio"
