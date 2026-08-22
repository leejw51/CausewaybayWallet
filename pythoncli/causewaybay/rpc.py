"""A small blocking JSON-RPC 2.0 client covering the methods the wallet needs."""

from __future__ import annotations

import itertools
import json
import time
from typing import Any

import requests

from . import errors

_REQUEST_IDS = itertools.count(1)

DEFAULT_TIMEOUT = 30.0
RECEIPT_POLL_INTERVAL = 1.5


def to_quantity(value: int) -> str:
    """Encode an integer as a minimal ``0x``-prefixed quantity."""
    return hex(value)


def parse_quantity(value: Any, context: str = "value") -> int:
    """Decode a ``0x``-prefixed quantity."""
    if not isinstance(value, str):
        raise errors.rpc_error(f"{context} returned {value!r} instead of a quantity")
    body = value[2:] if value[:2].lower() == "0x" else value
    if body == "":
        return 0
    # `int(body, 16)` would accept "-1", "1_0" and " 1 ". A JSON-RPC quantity is
    # hex digits and nothing else, so anything looser is a malformed response
    # rather than a number — the Rust side rejects the same inputs.
    if not all(c in "0123456789abcdefABCDEF" for c in body):
        raise errors.rpc_error(f"{context} returned an unparsable quantity {value}")
    return int(body, 16)


def field_int(obj: Any, field: str) -> int | None:
    """Read a hex quantity out of a JSON object field, tolerating absent fields."""
    if not isinstance(obj, dict) or field not in obj:
        return None
    try:
        return parse_quantity(obj[field], field)
    except errors.WalletError:
        return None


class RpcClient:
    def __init__(self, url: str, timeout: float = DEFAULT_TIMEOUT) -> None:
        self.url = url
        self.timeout = timeout
        self._session = requests.Session()
        self._session.headers.update(
            {"content-type": "application/json", "user-agent": "causewaybay-wallet/0.1.0"}
        )

    def call(self, method: str, params: list[Any] | None = None) -> Any:
        """Issue one JSON-RPC call and unwrap its ``result``."""
        payload = {
            "jsonrpc": "2.0",
            "id": next(_REQUEST_IDS),
            "method": method,
            "params": params if params is not None else [],
        }
        try:
            response = self._session.post(self.url, json=payload, timeout=self.timeout)
        except requests.RequestException as exc:
            raise errors.rpc_error(f"{method} request to {self.url} failed: {exc}") from exc

        text = response.text
        if not response.ok:
            raise errors.rpc_error(f"{method} returned HTTP {response.status_code}: {text[:200]}")
        try:
            body = json.loads(text)
        except json.JSONDecodeError:
            raise errors.rpc_error(f"{method} returned a non-JSON response: {text[:200]}") from None

        if isinstance(body, dict) and body.get("error"):
            rpc_err = body["error"]
            message = (
                rpc_err.get("message", "unknown RPC error")
                if isinstance(rpc_err, dict)
                else str(rpc_err)
            )
            code = rpc_err.get("code", 0) if isinstance(rpc_err, dict) else 0
            # Surface the funding case distinctly; it is the most common failure.
            if "insufficient funds" in message.lower():
                raise errors.insufficient_funds(message)
            raise errors.rpc_error(f"{method} failed ({code}): {message}")

        if not isinstance(body, dict) or "result" not in body:
            raise errors.rpc_error(f"{method} response has no result field")
        return body["result"]

    # ------------------------------------------------------------- shortcuts

    def chain_id(self) -> int:
        return parse_quantity(self.call("eth_chainId"), "eth_chainId")

    def block_number(self) -> int:
        return parse_quantity(self.call("eth_blockNumber"), "eth_blockNumber")

    def get_balance(self, address: str) -> int:
        return parse_quantity(self.call("eth_getBalance", [address, "latest"]), "eth_getBalance")

    def get_transaction_count(self, address: str) -> int:
        """Uses the pending block so back-to-back sends do not reuse a nonce."""
        return parse_quantity(
            self.call("eth_getTransactionCount", [address, "pending"]),
            "eth_getTransactionCount",
        )

    def gas_price(self) -> int:
        return parse_quantity(self.call("eth_gasPrice"), "eth_gasPrice")

    def estimate_gas(self, sender: str, to: str, value: int, data: bytes = b"") -> int:
        call: dict[str, Any] = {"from": sender, "to": to, "value": to_quantity(value)}
        if data:
            call["data"] = "0x" + data.hex()
        return parse_quantity(self.call("eth_estimateGas", [call, "latest"]), "eth_estimateGas")

    def eth_call(self, to: str, data: bytes) -> bytes:
        """Read-only contract call; returns the raw ABI-encoded return data."""
        result = self.call("eth_call", [{"to": to, "data": "0x" + data.hex()}, "latest"])
        if not isinstance(result, str):
            raise errors.rpc_error("eth_call did not return hex data")
        try:
            return bytes.fromhex(result[2:] if result[:2].lower() == "0x" else result)
        except ValueError as exc:
            raise errors.rpc_error(f"eth_call returned malformed hex: {exc}") from exc

    def send_raw_transaction(self, raw: bytes) -> str:
        result = self.call("eth_sendRawTransaction", ["0x" + raw.hex()])
        if not isinstance(result, str):
            raise errors.rpc_error("eth_sendRawTransaction did not return a hash")
        return result

    def get_transaction_by_hash(self, tx_hash: str) -> dict[str, Any] | None:
        return self.call("eth_getTransactionByHash", [tx_hash]) or None

    def get_transaction_receipt(self, tx_hash: str) -> dict[str, Any] | None:
        return self.call("eth_getTransactionReceipt", [tx_hash]) or None

    def wait_for_receipt(self, tx_hash: str, timeout: float = 180.0) -> dict[str, Any] | None:
        """Poll for a receipt until it appears or the deadline passes.

        Transient poll failures are retried rather than propagated: by the time
        this is called the transaction has already been broadcast, so reporting
        a single 502 as failure invites the user to send it a second time. Only
        a failure that persists to the deadline is surfaced.
        """
        deadline = time.monotonic() + timeout
        last_error: errors.WalletError | None = None
        while True:
            try:
                receipt = self.get_transaction_receipt(tx_hash)
            except errors.WalletError as exc:
                last_error = exc
            else:
                if receipt:
                    return receipt
                last_error = None
            if time.monotonic() + RECEIPT_POLL_INTERVAL > deadline:
                if last_error is not None:
                    raise errors.rpc_error(
                        f"{tx_hash} was broadcast, but polling for its receipt kept "
                        f"failing: {last_error.message}"
                    )
                return None
            time.sleep(RECEIPT_POLL_INTERVAL)
