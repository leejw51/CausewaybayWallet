"""Rendering the account list as JSONL, CSV, plain text or Markdown.

One renderer feeds both front ends: ``account list --format`` on the CLI and the
"Save list" entries in the TUI produce identical bytes. It also matches the Rust
implementation byte for byte — see ``scripts/parity.sh``.
"""

from __future__ import annotations

import json
from typing import Any

from . import errors, store
from .store import Account
from .wallet import Keypair

JSONL = "jsonl"
CSV = "csv"
TXT = "txt"
MARKDOWN = "md"

ALL_FORMATS = (JSONL, CSV, TXT, MARKDOWN)

_ALIASES = {
    "jsonl": JSONL,
    "ndjson": JSONL,
    "csv": CSV,
    "txt": TXT,
    "text": TXT,
    "plain": TXT,
    "md": MARKDOWN,
    "markdown": MARKDOWN,
}

# The columns every format shares, in order.
#
# ``address_index`` is the BIP-44 index *within one mnemonic*, not a position in
# this list — every freshly generated seed starts again at 0. ``position`` is the
# row number, and ``seed`` groups the rows that share a mnemonic, so the
# repetition is self-explanatory. The two public keys are long, so they sit at
# the end where they do not push the readable columns off the side of a table.
COLUMNS = (
    "position",
    "label",
    "address",
    "source",
    "address_index",
    "seed",
    "derivation_path",
    "created_at",
    "active",
    "public_key_compressed",
    "public_key",
)

# The extra columns appended when secrets are included.
SECRET_COLUMNS = ("private_key", "mnemonic")


def parse_format(name: str) -> str:
    """Accept a format name or one of its aliases."""
    canonical = _ALIASES.get(str(name).strip().lower())
    if canonical is None:
        raise errors.usage(f"unknown format '{name}'; use jsonl, csv, txt or md")
    return canonical


def extension(fmt: str) -> str:
    """The file extension to suggest when saving."""
    return fmt


def seed_id(account: Account) -> str:
    """Which mnemonic an account came from, as the id the recall list uses.

    Two wallets showing the same seed were derived from one phrase; a blank
    means the key was imported on its own.
    """
    if not account.mnemonic:
        return ""
    return store.secret_id("mnemonic", account.mnemonic)


def public_keys(account: Account) -> tuple[str, str]:
    """``(compressed, uncompressed)`` from the account's private key.

    33 bytes with a parity prefix, and 64 bytes of raw X‖Y with the SEC1 ``0x04``
    tag dropped — the form the address is hashed from.
    """
    try:
        keypair = Keypair.from_private_key(account.private_key)
    except errors.WalletError:
        # A record whose key will not parse still gets a row, just a blank one.
        return "", ""
    return keypair.public_key_compressed, keypair.public_key


def headers(secrets: bool) -> list[str]:
    return list(COLUMNS) + (list(SECRET_COLUMNS) if secrets else [])


def row(account: Account, position: int, active_id: str | None, secrets: bool) -> list[str]:
    """One account flattened to strings, in ``COLUMNS`` order."""
    compressed, uncompressed = public_keys(account)
    values = [
        str(position),
        account.label,
        account.address,
        account.source,
        "" if account.index is None else str(account.index),
        seed_id(account),
        account.derivation_path or "",
        account.created_at,
        "yes" if account.id == active_id else "no",
        compressed,
        uncompressed,
    ]
    if secrets:
        values.append(account.private_key)
        values.append(account.mnemonic or "")
    return values


def render(
    accounts: list[Account],
    fmt: str,
    active_id: str | None = None,
    secrets: bool = False,
) -> str:
    """Render the account list. ``active_id`` marks the default account."""
    if fmt == JSONL:
        return _render_jsonl(accounts, active_id, secrets)
    if fmt == CSV:
        return _render_csv(accounts, active_id, secrets)
    if fmt == TXT:
        return _render_txt(accounts, active_id, secrets)
    if fmt == MARKDOWN:
        return _render_markdown(accounts, active_id, secrets)
    raise errors.usage(f"unknown format '{fmt}'")


def _render_jsonl(accounts: list[Account], active_id: str | None, secrets: bool) -> str:
    lines = []
    for offset, account in enumerate(accounts):
        compressed, uncompressed = public_keys(account)
        # Built explicitly rather than from ``public_view``, so all four formats
        # carry the same columns under the same names. The on-disk record in
        # accounts.jsonl keeps its own ``index`` field; this is the export view.
        value: dict[str, Any] = {
            "position": offset + 1,
            "label": account.label,
            "address": account.address,
            "source": account.source,
            "address_index": account.index,
            "seed": seed_id(account),
            "derivation_path": account.derivation_path,
            "created_at": account.created_at,
            "active": account.id == active_id,
            "public_key_compressed": compressed,
            "public_key": uncompressed,
        }
        if secrets:
            value["private_key"] = account.private_key
            value["mnemonic"] = account.mnemonic
        # Sorted and compact, matching what the Rust side emits.
        lines.append(json.dumps(value, separators=(",", ":"), sort_keys=True))
    return "".join(line + "\n" for line in lines)


def csv_escape(value: str) -> str:
    """Quote a CSV field per RFC 4180 when it needs it."""
    if any(c in value for c in ',"\n\r') or value.startswith(" ") or value.endswith(" "):
        return '"' + value.replace('"', '""') + '"'
    return value


def _render_csv(accounts: list[Account], active_id: str | None, secrets: bool) -> str:
    out = [",".join(headers(secrets))]
    for offset, account in enumerate(accounts):
        out.append(",".join(csv_escape(v) for v in row(account, offset + 1, active_id, secrets)))
    return "".join(line + "\n" for line in out)


def _render_txt(accounts: list[Account], active_id: str | None, secrets: bool) -> str:
    names = headers(secrets)
    rows = [row(account, offset + 1, active_id, secrets) for offset, account in enumerate(accounts)]

    # Width of the widest cell in each column, header included.
    widths = [
        max([len(r[column]) for r in rows] + [len(name)]) for column, name in enumerate(names)
    ]

    def line(cells: list[str]) -> str:
        return "  ".join(
            cell.ljust(width) for cell, width in zip(cells, widths, strict=True)
        ).rstrip()

    out = [line(names), line(["-" * width for width in widths])]
    out.extend(line(r) for r in rows)
    return "".join(entry + "\n" for entry in out)


def markdown_escape(value: str) -> str:
    """A pipe or a newline inside a cell would break the table."""
    return value.replace("|", "\\|").replace("\n", " ").replace("\r", " ")


def _render_markdown(accounts: list[Account], active_id: str | None, secrets: bool) -> str:
    names = headers(secrets)
    out = [
        "| " + " | ".join(names) + " |",
        "| " + " | ".join("---" for _ in names) + " |",
    ]
    for offset, account in enumerate(accounts):
        cells = [markdown_escape(v) for v in row(account, offset + 1, active_id, secrets)]
        out.append("| " + " | ".join(cells) + " |")
    return "".join(line + "\n" for line in out)
