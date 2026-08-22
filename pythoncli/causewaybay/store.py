"""The append-only JSONL store described in ``SPEC.md``.

Nothing is ever rewritten: every mutation appends one line, and state is the fold
of every line in order. That makes the files trivially inspectable, safe against
partial writes, and shareable with the Rust front end.
"""

from __future__ import annotations

import json
import os
import re
from collections.abc import Iterator
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from eth_utils import keccak

from . import errors, networks, paths

SCHEMA = 1

ACCOUNTS_FILE = "accounts.jsonl"
CONFIG_FILE = "config.jsonl"
HISTORY_FILE = "history.jsonl"
RECENT_FILE = "recent.jsonl"

KEY_NETWORK = "network"
KEY_ACTIVE_ACCOUNT = "active_account"

SOURCE_MNEMONIC = "mnemonic"
SOURCE_PRIVATE_KEY = "private_key"

_LABEL_RE = re.compile(r"^[A-Za-z0-9._-]{1,64}$")


def _text(record: dict[str, Any], key: str) -> str:
    """A required string field, rejecting anything of another type."""
    value = record[key]
    if not isinstance(value, str):
        raise TypeError(f"{key} must be a string, got {type(value).__name__}")
    return value


def _optional_text(record: dict[str, Any], key: str) -> str | None:
    value = record.get(key)
    if value is None:
        return None
    if not isinstance(value, str):
        raise TypeError(f"{key} must be a string, got {type(value).__name__}")
    return value


def _optional_int(record: dict[str, Any], key: str) -> int | None:
    value = record.get(key)
    if value is None:
        return None
    if isinstance(value, bool) or not isinstance(value, int):
        raise TypeError(f"{key} must be an integer, got {type(value).__name__}")
    return value


def now_rfc3339() -> str:
    """UTC, millisecond precision, ``Z`` suffix — matching the Rust side."""
    stamp = datetime.now(timezone.utc).isoformat(timespec="milliseconds")
    return stamp.replace("+00:00", "Z")


def secret_id(kind: str, secret: str) -> str:
    """Stable id for key material, so re-using a phrase updates one entry."""
    digest = keccak(text=f"{kind}|{secret.strip()}")
    return "sec_" + digest[:8].hex()


def account_id(address: str, created_at: str, label: str) -> str:
    """Deterministic, collision-resistant account id.

    The label is part of the preimage because labels are unique: without it,
    importing the same address twice inside one millisecond would produce two
    records that share an id and get folded together on replay.
    """
    digest = keccak(text=f"{address.lower()}|{created_at}|{label}")
    return "acc_" + digest[:8].hex()


def validate_label(label: str) -> None:
    """Labels are used as command line selectors, so keep them shell-friendly."""
    if not label or len(label) > 64:
        raise errors.usage("label must be 1..64 characters")
    if not _LABEL_RE.match(label):
        raise errors.usage("label may only contain letters, digits, '.', '_' and '-'")
    if label.startswith("0x"):
        raise errors.usage("label must not look like an address")


@dataclass
class Account:
    """An account as reconstructed by replaying the account log.

    The secret fields are excluded from ``repr`` so a stray print, log line or
    traceback showing an Account never leaks them; ``secret_view()`` is the
    deliberate way to get at them.
    """

    id: str
    label: str
    address: str
    source: str
    private_key: str = field(repr=False)
    created_at: str = ""
    mnemonic: str | None = field(default=None, repr=False)
    derivation_path: str | None = None
    index: int | None = None

    def public_view(self) -> dict[str, Any]:
        """The account without any secret fields — what commands print by default."""
        return {
            "id": self.id,
            "label": self.label,
            "address": self.address,
            "source": self.source,
            "derivation_path": self.derivation_path,
            "index": self.index,
            "created_at": self.created_at,
        }

    def secret_view(self) -> dict[str, Any]:
        """The account including secrets — only for ``export`` / ``--secret``."""
        return {
            **self.public_view(),
            "private_key": self.private_key,
            "mnemonic": self.mnemonic,
        }


@dataclass
class RecentSecret:
    """Key material the wallet has seen before.

    Offered back so a returning user can pick a mnemonic or private key from a
    list instead of retyping it.
    """

    id: str
    kind: str
    # Excluded from repr: the whole point of this type is to hold a secret.
    secret: str = field(repr=False)
    address: str = ""
    first_seen_at: str = ""
    last_used_at: str = ""
    uses: int = 1
    word_count: int | None = None
    # True when ``address`` was derived with a BIP-39 passphrase. The passphrase
    # itself is deliberately not stored — keeping it apart from the phrase is
    # the whole point of it. What is stored is that one exists, so a restore can
    # demand it back instead of silently producing a different, unfunded wallet.
    has_passphrase: bool = False

    @property
    def preview(self) -> str:
        """Enough of the secret to recognise it, never enough to use it."""
        if self.kind == SOURCE_MNEMONIC:
            words = self.secret.split()
            if not words:
                return ""
            if len(words) == 1:
                return "…"
            return f"{words[0]} … {words[-1]}"
        from .output import truncate_secret

        return truncate_secret(self.secret)

    def public_view(self) -> dict[str, Any]:
        """Everything except the secret itself, plus an identifying preview."""
        return {
            "id": self.id,
            "kind": self.kind,
            "address": self.address,
            "word_count": self.word_count,
            "has_passphrase": self.has_passphrase,
            "preview": self.preview,
            "first_seen_at": self.first_seen_at,
            "last_used_at": self.last_used_at,
            "uses": self.uses,
        }

    def secret_view(self) -> dict[str, Any]:
        return {**self.public_view(), "secret": self.secret}


@dataclass
class TxRecord:
    """A recorded transaction, as reconstructed by replaying the history log."""

    hash: str
    from_address: str
    to: str
    value: str
    value_wei: str
    network: str
    chain_id: int
    nonce: int
    gas_limit: int
    gas_price_wei: str
    status: str
    created_at: str
    token: str | None = None
    block_number: int | None = None
    gas_used: int | None = None

    def to_json(self) -> dict[str, Any]:
        """Serialise with the wire field names (``from``, not ``from_address``)."""
        data = asdict(self)
        data["from"] = data.pop("from_address")
        return {
            k: v
            for k, v in data.items()
            if v is not None or k in {"token", "block_number", "gas_used"}
        }

    @classmethod
    def from_json(cls, record: dict[str, Any]) -> TxRecord:
        return cls(
            hash=record["hash"],
            from_address=record.get("from", ""),
            to=record.get("to", ""),
            value=record.get("value", "0"),
            value_wei=record.get("value_wei", "0"),
            network=record.get("network", ""),
            chain_id=int(record.get("chain_id", 0)),
            nonce=int(record.get("nonce", 0)),
            gas_limit=int(record.get("gas_limit", 0)),
            gas_price_wei=record.get("gas_price_wei", "0"),
            status=record.get("status", "submitted"),
            created_at=record.get("created_at", ""),
            token=record.get("token"),
            block_number=record.get("block_number"),
            gas_used=record.get("gas_used"),
        )


class Store:
    """Append-only JSONL storage rooted at a wallet home directory."""

    def __init__(self, home: str | Path) -> None:
        self.home = paths.ensure_dir(Path(home))

    # ------------------------------------------------------------------ paths

    @property
    def accounts_path(self) -> Path:
        return self.home / ACCOUNTS_FILE

    @property
    def config_path(self) -> Path:
        return self.home / CONFIG_FILE

    @property
    def history_path(self) -> Path:
        return self.home / HISTORY_FILE

    @property
    def recent_path(self) -> Path:
        return self.home / RECENT_FILE

    # ---------------------------------------------------------------- raw I/O

    def _append(self, path: Path, record: dict[str, Any]) -> None:
        """Append one compact JSON line, owner-readable only.

        The mode is set at creation via ``os.open``, not by a chmod after the
        record has been written, so there is no window in which the first
        private key sits on disk behind the umask.
        """
        line = json.dumps(record, separators=(",", ":"), sort_keys=True) + "\n"
        try:
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, paths.FILE_MODE)
            with os.fdopen(fd, "a", encoding="utf-8") as handle:
                handle.write(line)
            # O_CREAT applies the mode only on creation; tighten a pre-existing
            # file too, in case it was made by something less careful.
            paths.set_private(path, paths.FILE_MODE)
        except OSError as exc:
            raise errors.io_error(f"cannot write to {path}: {exc}") from exc

    def _read(self, path: Path) -> Iterator[dict[str, Any]]:
        """Yield every well-formed record, skipping junk and future schemas."""
        if not path.exists():
            return
        try:
            content = path.read_text(encoding="utf-8")
        except OSError as exc:
            raise errors.io_error(f"cannot read {path}: {exc}") from exc
        for line in content.splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(record, dict):
                continue
            if record.get("schema", 0) > SCHEMA:
                # Written by a newer version; degrade gracefully.
                continue
            yield record

    # -------------------------------------------------------------- accounts

    def accounts(self) -> list[Account]:
        """Replay the account log into the live account list, in creation order."""
        order: list[str] = []
        by_id: dict[str, Account] = {}

        for record in self._read(self.accounts_path):
            kind = record.get("type")
            account_key = record.get("id")
            if not isinstance(account_key, str) or not account_key:
                continue
            if kind == "account.create":
                try:
                    account = Account(
                        id=account_key,
                        label=_text(record, "label"),
                        address=_text(record, "address"),
                        source=_text(record, "source"),
                        private_key=_text(record, "private_key"),
                        created_at=record.get("created_at") or "",
                        mnemonic=_optional_text(record, "mnemonic"),
                        derivation_path=_optional_text(record, "derivation_path"),
                        index=_optional_int(record, "index"),
                    )
                except (KeyError, TypeError, ValueError):
                    # A hand-edited or truncated line is skipped, exactly as the
                    # Rust replay skips it — otherwise a wrong-typed field would
                    # build a malformed Account and crash every later command.
                    continue
                if account_key not in by_id:
                    order.append(account_key)
                by_id[account_key] = account
            elif kind == "account.rename":
                label = record.get("label")
                if account_key in by_id and isinstance(label, str):
                    by_id[account_key].label = label
            elif kind == "account.delete":
                by_id.pop(account_key, None)
                if account_key in order:
                    order.remove(account_key)

        return [by_id[key] for key in order if key in by_id]

    def current_seed(self) -> str | None:
        """The mnemonic new addresses should come from.

        A wallet normally holds one seed and many addresses derived from it, so
        "new address" has to know which phrase to walk. The active account's
        phrase wins; failing that, the first one in the wallet.
        """
        try:
            active = self.active_account()
        except errors.WalletError:
            active = None
        if active is not None and active.mnemonic:
            return active.mnemonic
        for account in self.accounts():
            if account.mnemonic:
                return account.mnemonic
        return None

    def next_address_index(self, mnemonic: str) -> int:
        """The lowest address index not yet taken on ``mnemonic``.

        Indexes run 0, 1, 2, … across the accounts sharing one phrase, so this
        is what makes a fresh address continue the sequence instead of
        colliding with an existing one.
        """
        taken = [
            account.index
            for account in self.accounts()
            if account.mnemonic == mnemonic and account.index is not None
        ]
        return max(taken) + 1 if taken else 0

    def find_account(self, selector: str) -> Account:
        """Resolve an account by id, label or address (case-insensitive)."""
        needle = str(selector or "").strip()
        if not needle:
            raise errors.usage("account selector is empty")
        lowered = needle.lower()
        accounts = self.accounts()
        for account in accounts:
            if account.id == needle or account.label == needle:
                return account
            if account.address.lower() == lowered:
                return account
        for account in accounts:
            if account.label.lower() == lowered:
                return account
        raise errors.account_not_found(f"no account matching '{needle}'")

    def active_account(self) -> Account:
        """The account commands operate on when none is named."""
        accounts = self.accounts()
        if not accounts:
            raise errors.no_active_account(
                "no accounts yet; create one with `cwbwallet account new`"
            )
        active_id = self.config_get(KEY_ACTIVE_ACCOUNT)
        if active_id:
            for account in accounts:
                if account.id == active_id:
                    return account
        # A stale pointer (deleted account) falls back to the first account.
        return accounts[0]

    def create_account(
        self,
        address: str,
        source: str,
        private_key: str,
        label: str | None = None,
        mnemonic: str | None = None,
        derivation_path: str | None = None,
        index: int | None = None,
    ) -> Account:
        """Append an ``account.create`` record. Labels must be unique."""
        existing = self.accounts()
        if label is None:
            label = self._next_free_label(existing)
        else:
            validate_label(label)
            if any(a.label.lower() == label.lower() for a in existing):
                raise errors.duplicate_label(f"an account named '{label}' already exists")

        created_at = now_rfc3339()
        account = Account(
            id=account_id(address, created_at, label),
            label=label,
            address=address,
            source=source,
            private_key=private_key,
            created_at=created_at,
            mnemonic=mnemonic,
            derivation_path=derivation_path,
            index=index,
        )

        record: dict[str, Any] = {
            "schema": SCHEMA,
            "type": "account.create",
            "id": account.id,
            "label": account.label,
            "address": account.address,
            "source": account.source,
            "private_key": account.private_key,
            "created_at": account.created_at,
        }
        if mnemonic is not None:
            record["mnemonic"] = mnemonic
        if derivation_path is not None:
            record["derivation_path"] = derivation_path
        if index is not None:
            record["index"] = index
        self._append(self.accounts_path, record)
        return account

    @staticmethod
    def _next_free_label(existing: list[Account]) -> str:
        """Pick ``account-N`` for the lowest free N."""
        taken = {a.label.lower() for a in existing}
        n = 1
        while f"account-{n}" in taken:
            n += 1
        return f"account-{n}"

    def rename_account(self, account_key: str, label: str) -> None:
        validate_label(label)
        for account in self.accounts():
            if account.id != account_key and account.label.lower() == label.lower():
                raise errors.duplicate_label(f"an account named '{label}' already exists")
        self._append(
            self.accounts_path,
            {
                "schema": SCHEMA,
                "type": "account.rename",
                "id": account_key,
                "label": label,
                "updated_at": now_rfc3339(),
            },
        )

    def delete_account(self, account_key: str) -> None:
        self._append(
            self.accounts_path,
            {
                "schema": SCHEMA,
                "type": "account.delete",
                "id": account_key,
                "deleted_at": now_rfc3339(),
            },
        )
        # Drop a dangling active-account pointer so later reads stay consistent.
        if self.config_get(KEY_ACTIVE_ACCOUNT) == account_key:
            remaining = self.accounts()
            self.config_set(KEY_ACTIVE_ACCOUNT, remaining[0].id if remaining else "")

    # ---------------------------------------------------------------- config

    def config(self) -> dict[str, str]:
        """Replay the config log; later writes win, an empty value clears."""
        result: dict[str, str] = {}
        for record in self._read(self.config_path):
            if record.get("type") != "config.set":
                continue
            key, value = record.get("key"), record.get("value")
            if not isinstance(key, str) or not isinstance(value, str):
                continue
            if value == "":
                result.pop(key, None)
            else:
                result[key] = value
        return result

    def config_get(self, key: str) -> str | None:
        return self.config().get(key)

    def config_set(self, key: str, value: str) -> None:
        self._append(
            self.config_path,
            {
                "schema": SCHEMA,
                "type": "config.set",
                "key": key,
                "value": value,
                "updated_at": now_rfc3339(),
            },
        )

    def network(self) -> networks.Network:
        """The selected network, defaulting to the testnet."""
        key = self.config_get(KEY_NETWORK)
        if not key:
            return networks.find(networks.DEFAULT_NETWORK)
        try:
            return networks.find(key)
        except errors.WalletError:
            # A garbage value must not lock the user out.
            return networks.find(networks.DEFAULT_NETWORK)

    # ---------------------------------------------------------------- recent

    def recent(self) -> list[RecentSecret]:
        """Replay the recall log, most recently used first."""
        order: list[str] = []
        by_id: dict[str, RecentSecret] = {}

        for record in self._read(self.recent_path):
            kind = record.get("type")
            entry_id = record.get("id")
            if not isinstance(entry_id, str) or not entry_id:
                continue
            if kind == "secret.remember":
                try:
                    entry = RecentSecret(
                        id=entry_id,
                        kind=record["kind"],
                        secret=record["secret"],
                        address=record["address"],
                        first_seen_at=record.get("first_seen_at", ""),
                        last_used_at=record.get("last_used_at", ""),
                        uses=int(record.get("uses", 1)),
                        word_count=record.get("word_count"),
                        has_passphrase=bool(record.get("has_passphrase", False)),
                    )
                except (KeyError, TypeError, ValueError):
                    continue
                if entry_id not in by_id:
                    order.append(entry_id)
                by_id[entry_id] = entry
            elif kind == "secret.forget":
                by_id.pop(entry_id, None)
                if entry_id in order:
                    order.remove(entry_id)

        entries = [by_id[key] for key in order if key in by_id]
        # Newest use first; the log order breaks ties deterministically.
        entries.reverse()
        entries.sort(key=lambda entry: entry.last_used_at, reverse=True)
        return entries

    def remember_secret(
        self,
        kind: str,
        secret: str,
        address: str,
        word_count: int | None = None,
        has_passphrase: bool = False,
    ) -> RecentSecret:
        """Record that this key material was used, refreshing an existing entry."""
        entry_id = secret_id(kind, secret)
        now = now_rfc3339()
        existing = next((e for e in self.recent() if e.id == entry_id), None)
        entry = RecentSecret(
            id=entry_id,
            kind=kind,
            secret=secret,
            address=address,
            first_seen_at=existing.first_seen_at if existing else now,
            last_used_at=now,
            uses=(existing.uses if existing else 0) + 1,
            word_count=word_count,
            has_passphrase=has_passphrase,
        )
        record: dict[str, Any] = {
            "schema": SCHEMA,
            "type": "secret.remember",
            "id": entry.id,
            "kind": entry.kind,
            "secret": entry.secret,
            "address": entry.address,
            "first_seen_at": entry.first_seen_at,
            "last_used_at": entry.last_used_at,
            "uses": entry.uses,
        }
        if word_count is not None:
            record["word_count"] = word_count
        record["has_passphrase"] = has_passphrase
        self._append(self.recent_path, record)
        return entry

    def find_recent(self, selector: str) -> RecentSecret:
        """Resolve a remembered entry by id, 1-based position, or address."""
        needle = str(selector or "").strip()
        if not needle:
            raise errors.usage("recall selector is empty")
        entries = self.recent()
        if not entries:
            raise errors.not_found("nothing has been remembered yet")
        if needle.isdigit():
            position = int(needle)
            if 1 <= position <= len(entries):
                return entries[position - 1]
            raise errors.not_found(f"no remembered entry at position {position}")
        lowered = needle.lower()
        for entry in entries:
            if entry.id == needle or entry.address.lower() == lowered:
                return entry
        raise errors.not_found(f"no remembered entry matching '{needle}'")

    def forget_secret(self, entry_id: str) -> None:
        self._append(
            self.recent_path,
            {
                "schema": SCHEMA,
                "type": "secret.forget",
                "id": entry_id,
                "deleted_at": now_rfc3339(),
            },
        )

    def clear_recent(self) -> int:
        """Forget everything, one record per entry so the log stays append-only."""
        entries = self.recent()
        for entry in entries:
            self.forget_secret(entry.id)
        return len(entries)

    # --------------------------------------------------------------- history

    def history(self) -> list[TxRecord]:
        """Replay the transaction log, oldest first."""
        order: list[str] = []
        by_hash: dict[str, TxRecord] = {}

        for record in self._read(self.history_path):
            kind = record.get("type")
            tx_hash = str(record.get("hash", "")).lower()
            if not tx_hash:
                continue
            if kind == "tx.send":
                try:
                    tx = TxRecord.from_json(record)
                except (KeyError, TypeError, ValueError):
                    continue
                if tx_hash not in by_hash:
                    order.append(tx_hash)
                by_hash[tx_hash] = tx
            elif kind == "tx.update" and tx_hash in by_hash:
                tx = by_hash[tx_hash]
                if isinstance(record.get("status"), str):
                    tx.status = record["status"]
                if isinstance(record.get("block_number"), int):
                    tx.block_number = record["block_number"]
                if isinstance(record.get("gas_used"), int):
                    tx.gas_used = record["gas_used"]

        return [by_hash[key] for key in order]

    def record_tx(self, tx: TxRecord) -> None:
        self._append(
            self.history_path,
            {"schema": SCHEMA, "type": "tx.send", **tx.to_json()},
        )

    def update_tx(
        self,
        tx_hash: str,
        status: str,
        block_number: int | None = None,
        gas_used: int | None = None,
    ) -> None:
        self._append(
            self.history_path,
            {
                "schema": SCHEMA,
                "type": "tx.update",
                "hash": tx_hash.lower(),
                "status": status,
                "block_number": block_number,
                "gas_used": gas_used,
                "updated_at": now_rfc3339(),
            },
        )
