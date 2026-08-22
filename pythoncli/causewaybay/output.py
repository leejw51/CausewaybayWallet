"""The two rendering modes: human text, and the machine envelope from ``SPEC.md``."""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any

from .errors import WalletError

WARNING = (
    "⚠️  Educational wallet. Keys are stored unencrypted. Do not use with funds you cannot lose."
)


@dataclass
class CommandOutput:
    """What a command produces: structured data plus its human rendering."""

    data: Any
    human: str

    @classmethod
    def message(cls, human: str) -> CommandOutput:
        """For commands whose result is only worth stating in one line."""
        return cls({"message": human}, human)


def success_envelope(data: Any) -> str:
    """``{"ok":true,"data":…}`` on one line."""
    return json.dumps({"ok": True, "data": data}, separators=(",", ":"), sort_keys=True)


def error_envelope(error: WalletError) -> str:
    """``{"ok":false,"error":{…}}`` on one line."""
    return json.dumps(
        {"ok": False, "error": {"code": error.code, "message": error.message}},
        separators=(",", ":"),
        sort_keys=True,
    )


def table(rows: list[tuple[str, str]]) -> str:
    """Render key/value pairs as an aligned block."""
    if not rows:
        return ""
    width = max(len(key) for key, _ in rows)
    return "\n".join(f"{key:<{width}}  {value}" for key, value in rows)


def truncate_secret(secret: str) -> str:
    """Show only the ends of a secret, for at-a-glance identification."""
    body = secret[2:] if secret[:2].lower() == "0x" else secret
    if len(body) <= 12:
        return "*" * len(body)
    return f"0x{body[:6]}…{body[-4:]}"


def short_address(address: str) -> str:
    """Shorten an address for narrow panes."""
    if len(address) <= 16:
        return address
    return f"{address[:8]}…{address[-6:]}"
