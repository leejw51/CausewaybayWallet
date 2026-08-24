"""Command implementations.

Every command returns a ``CommandOutput`` so the caller decides between human
text and the JSON envelope. This mirrors ``rustcli/core/src/app.rs`` one-for-one.
"""

from __future__ import annotations

import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from eth_utils import keccak

from . import erc20, errors, export, networks, output, paths, store, units, wallet
from .output import CommandOutput
from .rpc import RpcClient, field_int, parse_quantity
from .store import Store
from .txs import LegacyTransaction, with_headroom

RECEIPT_TIMEOUT = 180.0


@dataclass
class SendPlan:
    """A send that has passed every check and is waiting only on a yes."""

    keypair: wallet.Keypair
    to: str
    value: int
    nonce: int
    gas_price: int
    gas_limit: int
    data: bytes
    amount: str
    prompt: str


def _mnemonic_problem(phrase: str, words: int) -> str | None:
    """Why ``phrase`` is not a valid mnemonic, or None when it is.

    Checked in the order a person would: the length, then the words, then the
    checksum — so the message names the first thing actually wrong rather than
    the last thing tested. The wording is this implementation's own; only
    ``valid`` and ``words`` are part of the shared contract.
    """
    if not phrase.strip():
        return "the mnemonic is empty"
    if words not in (12, 15, 18, 21, 24):
        return f"unsupported word count {words}; use 12, 15, 18, 21 or 24"

    normalized = wallet.normalize_mnemonic(phrase)
    vocabulary = set(wallet.wordlist())
    unknown = [word for word in normalized.split() if word not in vocabulary]
    if unknown:
        return f"'{unknown[0]}' is not a BIP-39 word"
    if not wallet.validate_mnemonic(phrase):
        return "the checksum does not match"
    return None


class App:
    """The wallet, bound to one home directory and one network."""

    def __init__(
        self,
        home: str | Path | None = None,
        network_override: str | None = None,
        json_mode: bool = False,
        assume_yes: bool = False,
    ) -> None:
        self.store = Store(paths.resolve_home(home))
        self.network = networks.find(network_override) if network_override else self.store.network()
        self.json_mode = json_mode
        self.assume_yes = assume_yes

    # ------------------------------------------------------------- utilities

    def rpc(self) -> RpcClient:
        """An RPC client pointed at the active network."""
        configured = self.store.config_get(self.network.rpc_config_key)
        return RpcClient(self.network.resolve_rpc(configured))

    def pick_account(self, selector: str | None) -> store.Account:
        """The account a command should act on: ``--account``, else the active one."""
        if selector:
            return self.store.find_account(selector)
        return self.store.active_account()

    def pick_address(
        self, address: str | None, account: str | None
    ) -> tuple[str, store.Account | None]:
        """The address a read-only query should use."""
        if address:
            return wallet.parse_address(address), None
        chosen = self.pick_account(account)
        return wallet.parse_address(chosen.address), chosen

    def confirm(self, prompt: str) -> None:
        """Ask before doing something irreversible.

        In ``--json`` mode there is nobody to ask, so ``--yes`` becomes mandatory —
        which also stops an automated caller from spending funds by accident.
        """
        if self.assume_yes:
            return
        if self.json_mode or not sys.stdin.isatty():
            raise errors.confirmation_required(f"{prompt} — re-run with --yes to confirm")
        answer = input(f"{prompt} [y/N]: ").strip().lower()
        if answer not in {"y", "yes"}:
            raise errors.confirmation_required("cancelled")

    def _activate_if_first(self, account: store.Account) -> None:
        """The very first account becomes active automatically."""
        if len(self.store.accounts()) == 1:
            self.store.config_set(store.KEY_ACTIVE_ACCOUNT, account.id)

    def _remember_mnemonic(self, mnemonic: str, passphrase: str = "") -> None:
        """Add a mnemonic to the recall list, keyed by the address it derives.

        Keyed by the address the passphrase actually produces, so the entry
        identifies the wallet the user has rather than a different one that
        happens to share the phrase.
        """
        normalized = wallet.normalize_mnemonic(mnemonic)
        root = wallet.Keypair.from_mnemonic(normalized, 0, passphrase)
        self.store.remember_secret(
            store.SOURCE_MNEMONIC,
            normalized,
            root.address,
            len(normalized.split()),
            has_passphrase=bool(passphrase),
        )

    def _require_passphrase_match(self, parent: store.Account) -> None:
        """Refuse to derive from an account whose phrase alone does not reproduce it.

        A wallet created with a BIP-39 passphrase stores the phrase but not the
        passphrase, so deriving with the phrase alone would silently produce an
        unrelated address. Better to say so than to hand back a wrong wallet.
        """
        if not parent.mnemonic or parent.index is None:
            return
        plain = wallet.Keypair.from_mnemonic(parent.mnemonic, parent.index)
        if plain.address != parent.address:
            raise errors.usage(
                f"'{parent.label}' was created with a BIP-39 passphrase, so its mnemonic "
                "alone derives a different wallet; re-import it with --passphrase and "
                "derive from that"
            )

    def _newest_recent(self) -> store.RecentSecret:
        entries = self.store.recent()
        if not entries:
            raise errors.not_found("nothing has been remembered yet")
        return entries[0]

    # ================================================================ accounts

    def account_new(
        self,
        label: str | None = None,
        new_seed: bool = False,
        words: int = 12,
        index: int | None = None,
        show_secret: bool = False,
    ) -> CommandOutput:
        """Add the next address of the wallet's mnemonic.

        A wallet holds one mnemonic and many addresses derived from it, so a new
        address continues that sequence. Only an empty wallet — or an explicit
        ``new_seed`` — mints a fresh phrase.
        """
        existing = None if new_seed else self.store.current_seed()
        if existing is None:
            phrase, minted = wallet.generate_mnemonic(words), True
        else:
            phrase, minted = existing, False

        if index is None:
            index = self.store.next_address_index(phrase)

        keypair = wallet.Keypair.from_mnemonic(phrase, index)
        path = wallet.ethereum_path(index)
        account = self.store.create_account(
            address=keypair.address,
            source=store.SOURCE_MNEMONIC,
            private_key=keypair.private_key,
            label=label,
            mnemonic=phrase,
            derivation_path=path,
            index=index,
        )
        self._activate_if_first(account)
        self._remember_mnemonic(phrase)

        data = account.public_view()
        data["new_seed"] = minted

        rows = [
            ("Source", "mnemonic"),
            ("Path", path),
            ("Address index", str(index)),
            (
                "Seed",
                f"new, {words} words" if minted else "the wallet's existing mnemonic",
            ),
        ]
        human = f"Created {account.label} ({account.address})\n" + output.table(rows)
        if show_secret:
            data["mnemonic"] = phrase
            data["private_key"] = keypair.private_key
            human += f"\n\nMnemonic (write it down):\n  {phrase}"
        elif minted:
            human += "\n\nThe mnemonic is stored in the wallet; reveal it with `account export`."
        return CommandOutput(data, human)

    def account_import_mnemonic(
        self,
        mnemonic: str,
        index: int = 0,
        label: str | None = None,
        passphrase: str = "",
    ) -> CommandOutput:
        keypair = wallet.Keypair.from_mnemonic(mnemonic, index, passphrase)
        account = self.store.create_account(
            address=keypair.address,
            source=store.SOURCE_MNEMONIC,
            private_key=keypair.private_key,
            label=label,
            mnemonic=wallet.normalize_mnemonic(mnemonic),
            derivation_path=wallet.ethereum_path(index),
            index=index,
        )
        self._activate_if_first(account)
        self._remember_mnemonic(mnemonic, passphrase)
        return CommandOutput(account.public_view(), f"Imported {account.label} ({account.address})")

    def account_import_key(self, private_key: str, label: str | None = None) -> CommandOutput:
        keypair = wallet.Keypair.from_private_key(private_key)
        account = self.store.create_account(
            address=keypair.address,
            source=store.SOURCE_PRIVATE_KEY,
            private_key=keypair.private_key,
            label=label,
        )
        self._activate_if_first(account)
        self.store.remember_secret(store.SOURCE_PRIVATE_KEY, keypair.private_key, keypair.address)
        return CommandOutput(account.public_view(), f"Imported {account.label} ({account.address})")

    def account_list(
        self,
        fmt: str | None = None,
        output_path: str | None = None,
        secret: bool = False,
    ) -> CommandOutput:
        accounts = self.store.accounts()
        try:
            active_id: str | None = self.store.active_account().id
        except errors.WalletError:
            active_id = None

        # `--format` turns this into an export; without it the command behaves
        # exactly as it always has.
        if fmt is not None:
            chosen = export.parse_format(fmt)
            rendered = export.render(accounts, chosen, active_id, secret)
            if output_path:
                path = Path(output_path)
                try:
                    if secret:
                        # Born 0600: it holds private keys from its first byte.
                        paths.write_private(path, rendered)
                    else:
                        path.write_text(rendered, encoding="utf-8")
                except OSError as exc:
                    raise errors.io_error(f"cannot write {output_path}: {exc}") from exc
                return CommandOutput(
                    {
                        "format": chosen,
                        "path": output_path,
                        "count": len(accounts),
                        "secret": secret,
                    },
                    f"Saved {len(accounts)} wallets to {output_path}",
                )
            return CommandOutput(
                {
                    "format": chosen,
                    "path": None,
                    "count": len(accounts),
                    "secret": secret,
                    "content": rendered,
                },
                # The trailing newline is already in the rendering.
                rendered.rstrip("\n"),
            )

        data = []
        for account in accounts:
            entry = account.public_view()
            entry["active"] = account.id == active_id
            data.append(entry)

        if not accounts:
            human = "No accounts yet. Create one with `cwbwallet account new`."
        else:
            human = "\n".join(
                f"{'*' if a.id == active_id else ' '} {a.label:<16} {a.address}  {a.source}"
                for a in accounts
            )
        return CommandOutput(data, human)

    def account_show(self, selector: str | None = None, secret: bool = False) -> CommandOutput:
        account = self.pick_account(selector)
        keypair = wallet.Keypair.from_private_key(account.private_key)
        data = account.secret_view() if secret else account.public_view()
        data["public_key"] = keypair.public_key
        data["public_key_compressed"] = keypair.public_key_compressed

        rows = [
            ("Label", account.label),
            ("Address", account.address),
            ("Id", account.id),
            ("Source", account.source),
        ]
        if account.derivation_path:
            rows.append(("Path", account.derivation_path))
        rows.append(
            (
                "Private key",
                account.private_key if secret else output.truncate_secret(account.private_key),
            )
        )
        if account.mnemonic:
            rows.append(("Mnemonic", account.mnemonic if secret else "<hidden — use --secret>"))
        return CommandOutput(data, output.table(rows))

    def account_use(self, selector: str) -> CommandOutput:
        account = self.store.find_account(selector)
        self.store.config_set(store.KEY_ACTIVE_ACCOUNT, account.id)
        return CommandOutput(
            account.public_view(),
            f"Active account is now {account.label} ({account.address})",
        )

    def account_derive(
        self, index: int, label: str | None = None, from_selector: str | None = None
    ) -> CommandOutput:
        parent = self.pick_account(from_selector)
        if not parent.mnemonic:
            raise errors.usage(
                f"account '{parent.label}' was imported from a private key, "
                "so it has no mnemonic to derive from"
            )
        self._require_passphrase_match(parent)
        keypair = wallet.Keypair.from_mnemonic(parent.mnemonic, index)
        account = self.store.create_account(
            address=keypair.address,
            source=store.SOURCE_MNEMONIC,
            private_key=keypair.private_key,
            label=label,
            mnemonic=parent.mnemonic,
            derivation_path=wallet.ethereum_path(index),
            index=index,
        )
        self._remember_mnemonic(parent.mnemonic)
        return CommandOutput(
            account.public_view(),
            f"Derived {account.label} ({account.address}) at index {index} from {parent.label}",
        )

    def account_rename(self, selector: str, label: str) -> CommandOutput:
        account = self.store.find_account(selector)
        self.store.rename_account(account.id, label)
        return CommandOutput(
            {"id": account.id, "label": label, "address": account.address},
            f"Renamed {account.label} to {label}",
        )

    def account_remove(self, selector: str) -> CommandOutput:
        account = self.store.find_account(selector)
        self.confirm(
            f"Remove account {account.label} ({account.address})? Its key is only in this wallet"
        )
        self.store.delete_account(account.id)
        return CommandOutput(
            {"id": account.id, "label": account.label, "removed": True},
            f"Removed {account.label}",
        )

    def account_export(self, selector: str | None = None) -> CommandOutput:
        account = self.pick_account(selector)
        rows = [
            ("Label", account.label),
            ("Address", account.address),
            ("Private key", account.private_key),
        ]
        if account.mnemonic:
            rows.append(("Mnemonic", account.mnemonic))
        return CommandOutput(account.secret_view(), f"{output.WARNING}\n\n{output.table(rows)}")

    # ================================================================== recall

    def account_import_recent(
        self,
        selector: str | None = None,
        index: int = 0,
        label: str | None = None,
        passphrase: str = "",
    ) -> CommandOutput:
        """Create an account from key material the wallet already remembers."""
        entry = self.store.find_recent(selector) if selector else self._newest_recent()
        if entry.kind == store.SOURCE_MNEMONIC:
            # A phrase remembered with a passphrase names a wallet the phrase
            # alone cannot reach. Restoring without it would hand back a
            # different, unfunded address and say nothing, so refuse — and check
            # the result before storing it.
            if entry.has_passphrase and not passphrase:
                raise errors.usage(
                    f"the remembered mnemonic for {entry.address} was used with a "
                    "BIP-39 passphrase; pass --passphrase to restore that wallet"
                )
            derived = wallet.Keypair.from_mnemonic(entry.secret, 0, passphrase)
            if derived.address != entry.address:
                raise errors.usage(
                    f"that passphrase derives {derived.address}, not the remembered "
                    f"{entry.address} — restoring it would create a different wallet"
                )
            result = self.account_import_mnemonic(
                entry.secret, index=index, label=label, passphrase=passphrase
            )
        else:
            result = self.account_import_key(entry.secret, label=label)
        return CommandOutput(
            result.data,
            f"{result.human}\n(from remembered {entry.kind} {entry.id})",
        )

    def recent_list(self, kind: str | None = None, limit: int = 20) -> CommandOutput:
        wanted = kind.replace("-", "_") if kind else None
        entries = self.store.recent()
        if wanted:
            entries = [entry for entry in entries if entry.kind == wanted]
        entries = entries[: max(limit, 0)]

        data = []
        for position, entry in enumerate(entries, start=1):
            view = entry.public_view()
            view["position"] = position
            data.append(view)

        if not entries:
            human = "Nothing remembered yet. Create or import an account and it will appear here."
        else:
            human = "\n".join(
                f"{position:>2}. {entry.kind:<12} {entry.address}  {entry.preview:<28} "
                f"used {entry.uses}x  {entry.last_used_at[:19]}"
                for position, entry in enumerate(entries, start=1)
            )
        return CommandOutput(data, human)

    def recent_show(self, selector: str | None = None, secret: bool = False) -> CommandOutput:
        entry = self.store.find_recent(selector) if selector else self._newest_recent()
        rows = [
            ("Id", entry.id),
            ("Kind", entry.kind),
            ("Address", entry.address),
            ("Uses", str(entry.uses)),
            ("Last used", entry.last_used_at),
            (
                "Mnemonic" if entry.kind == store.SOURCE_MNEMONIC else "Private key",
                entry.secret if secret else entry.preview,
            ),
        ]
        human = f"{output.WARNING}\n\n{output.table(rows)}" if secret else output.table(rows)
        return CommandOutput(entry.secret_view() if secret else entry.public_view(), human)

    def recent_forget(self, selector: str) -> CommandOutput:
        entry = self.store.find_recent(selector)
        self.confirm(
            f"Forget the remembered {entry.kind} for {entry.address}? "
            "Any account already created from it is kept"
        )
        self.store.forget_secret(entry.id)
        return CommandOutput(
            {"id": entry.id, "forgotten": True},
            f"Forgot {entry.id} ({entry.address})",
        )

    def recent_clear(self) -> CommandOutput:
        count = len(self.store.recent())
        if count == 0:
            return CommandOutput({"forgotten": 0}, "Nothing to forget.")
        self.confirm(f"Forget all {count} remembered secrets")
        forgotten = self.store.clear_recent()
        return CommandOutput({"forgotten": forgotten}, f"Forgot {forgotten} remembered secrets")

    # ================================================================= network

    def network_list(self) -> CommandOutput:
        current = self.store.network()
        data = [
            {
                "key": n.key,
                "name": n.name,
                "chain_id": n.chain_id,
                "symbol": n.symbol,
                "rpc": n.resolve_rpc(self.store.config_get(n.rpc_config_key)),
                "explorer": n.explorer,
                "testnet": n.testnet,
                "current": n.key == current.key,
            }
            for n in networks.ALL
        ]
        human = "\n".join(
            f"{'*' if n.key == current.key else ' '} {n.key:<16} chain {n.chain_id:<4} {n.name}"
            for n in networks.ALL
        )
        return CommandOutput(data, human)

    def network_current(self) -> CommandOutput:
        n = self.network
        rpc_url = n.resolve_rpc(self.store.config_get(n.rpc_config_key))
        return CommandOutput(
            {
                "key": n.key,
                "name": n.name,
                "chain_id": n.chain_id,
                "symbol": n.symbol,
                "rpc": rpc_url,
                "explorer": n.explorer,
                "testnet": n.testnet,
            },
            output.table(
                [
                    ("Network", n.name),
                    ("Key", n.key),
                    ("Chain id", str(n.chain_id)),
                    ("Symbol", n.symbol),
                    ("RPC", rpc_url),
                    ("Explorer", n.explorer),
                ]
            ),
        )

    def network_use(self, key: str) -> CommandOutput:
        target = networks.find(key)
        self.store.config_set(store.KEY_NETWORK, target.key)
        return CommandOutput(
            {"key": target.key, "name": target.name, "chain_id": target.chain_id},
            f"Network is now {target.name} (chain {target.chain_id})",
        )

    def network_set_rpc(self, key: str, url: str) -> CommandOutput:
        target = networks.find(key)
        self.store.config_set(target.rpc_config_key, url.strip())
        effective = target.resolve_rpc(url.strip() or None)
        return CommandOutput(
            {"key": target.key, "rpc": effective},
            f"RPC for {target.key} is now {effective}",
        )

    # ============================================================ chain reads

    def balance(self, address: str | None = None, account: str | None = None) -> CommandOutput:
        resolved, owner = self.pick_address(address, account)
        wei = self.rpc().get_balance(resolved)
        formatted = units.format_ether(wei)
        return CommandOutput(
            {
                "address": resolved,
                "account": owner.label if owner else None,
                "balance": formatted,
                "balance_wei": str(wei),
                "symbol": self.network.symbol,
                "network": self.network.key,
            },
            f"{formatted} {self.network.symbol} — {resolved}",
        )

    def nonce(self, address: str | None = None, account: str | None = None) -> CommandOutput:
        resolved, _ = self.pick_address(address, account)
        value = self.rpc().get_transaction_count(resolved)
        return CommandOutput(
            {"address": resolved, "nonce": value, "network": self.network.key},
            f"nonce {value} — {resolved}",
        )

    def gas_price(self) -> CommandOutput:
        wei = self.rpc().gas_price()
        gwei = units.format_gwei(wei)
        return CommandOutput(
            {
                "gas_price_wei": str(wei),
                "gas_price_gwei": gwei,
                "network": self.network.key,
            },
            f"{gwei} gwei",
        )

    def chain_info(self) -> CommandOutput:
        client = self.rpc()
        reported = client.chain_id()
        block = client.block_number()
        gas = client.gas_price()
        matches = reported == self.network.chain_id
        return CommandOutput(
            {
                "network": self.network.key,
                "name": self.network.name,
                "expected_chain_id": self.network.chain_id,
                "reported_chain_id": reported,
                "chain_id_matches": matches,
                "block_number": block,
                "gas_price_wei": str(gas),
                "rpc": client.url,
            },
            output.table(
                [
                    ("Network", self.network.name),
                    ("RPC", client.url),
                    ("Chain id", f"{reported}{'' if matches else ' (MISMATCH!)'}"),
                    ("Block", str(block)),
                    ("Gas price", f"{units.format_gwei(gas)} gwei"),
                ]
            ),
        )

    # ================================================================== sends

    def send(
        self,
        to: str,
        amount: str,
        gas_limit: int | None = None,
        gas_price_gwei: str | None = None,
        nonce: int | None = None,
        data: str | None = None,
        wait: bool = False,
        account: str | None = None,
    ) -> CommandOutput:
        plan = self.plan_send(
            to=to,
            amount=amount,
            gas_limit=gas_limit,
            gas_price_gwei=gas_price_gwei,
            nonce=nonce,
            data=data,
            account=account,
        )
        self.confirm(plan.prompt)
        return self.execute_send(plan, wait)

    def plan_send(
        self,
        to: str,
        amount: str,
        gas_limit: int | None = None,
        gas_price_gwei: str | None = None,
        nonce: int | None = None,
        data: str | None = None,
        account: str | None = None,
    ) -> SendPlan:
        """Resolve everything a transfer needs and check it can be paid for.

        Splitting the plan from the execution is what lets the TUI run its own
        confirmation prompt while still going through the same validation, gas
        resolution and funding check the CLI uses.
        """
        sender = self.pick_account(account)
        keypair = wallet.Keypair.from_private_key(sender.private_key)
        recipient = wallet.parse_address(to)
        # A transfer to the account it leaves from moves nothing and costs the
        # gas anyway. It is almost always a paste into the wrong field — the
        # sender's own address is the one most likely to be on the clipboard —
        # so it is refused here, before a node is asked anything.
        if recipient.lower() == keypair.address.lower():
            raise errors.usage(
                f"the recipient is the sending account ({recipient}); a transfer to "
                "itself moves nothing and still pays the gas"
            )
        value = units.parse_ether(amount)
        payload = wallet.parse_hex(data) if data else b""

        client = self.rpc()
        resolved_nonce = (
            nonce if nonce is not None else client.get_transaction_count(keypair.address)
        )
        resolved_gas_price = (
            units.parse_gwei(gas_price_gwei) if gas_price_gwei else client.gas_price()
        )
        if gas_limit is not None:
            resolved_gas_limit = gas_limit
        elif not payload:
            resolved_gas_limit = 21_000
        else:
            resolved_gas_limit = with_headroom(
                client.estimate_gas(keypair.address, recipient, value, payload)
            )

        # Fail before signing when the balance obviously cannot cover the transfer.
        balance = client.get_balance(keypair.address)
        gas_cost = resolved_gas_price * resolved_gas_limit
        if balance < value + gas_cost:
            raise errors.insufficient_funds(
                f"balance {units.format_ether(balance)} {self.network.symbol} cannot cover "
                f"{units.format_ether(value)} {self.network.symbol} plus up to "
                f"{units.format_ether(gas_cost)} {self.network.symbol} of gas"
            )

        return SendPlan(
            keypair=keypair,
            to=recipient,
            value=value,
            nonce=resolved_nonce,
            gas_price=resolved_gas_price,
            gas_limit=resolved_gas_limit,
            data=payload,
            amount=amount,
            prompt=(
                f"Send {amount} {self.network.symbol} from {sender.label} to "
                f"{recipient} on {self.network.name}"
            ),
        )

    def execute_send(self, plan: SendPlan, wait: bool = False) -> CommandOutput:
        """Sign and broadcast a plan the caller has already had confirmed."""
        client = self.rpc()
        signed = LegacyTransaction(
            nonce=plan.nonce,
            gas_price=plan.gas_price,
            gas_limit=plan.gas_limit,
            to=plan.to,
            value=plan.value,
            chain_id=self.network.chain_id,
            data=plan.data,
        ).sign(plan.keypair.private_key)

        # Recorded before broadcasting, using the hash computed locally: if the
        # node accepts the transaction and the response is then lost to a
        # timeout, the history still names it. Losing that record is what leads
        # to a user re-sending a transfer that already went through.
        tx_hash = signed.hash
        record = store.TxRecord(
            hash=tx_hash,
            from_address=plan.keypair.address,
            to=plan.to,
            value=units.format_ether(plan.value),
            value_wei=str(plan.value),
            network=self.network.key,
            chain_id=self.network.chain_id,
            nonce=plan.nonce,
            gas_limit=plan.gas_limit,
            gas_price_wei=str(plan.gas_price),
            status="submitting",
            created_at=store.now_rfc3339(),
        )
        self.store.record_tx(record)

        try:
            client.send_raw_transaction(signed.raw)
        except errors.WalletError as exc:
            # The node may still have accepted it, so the record stays and is
            # marked for what it is rather than deleted.
            self.store.update_tx(tx_hash, "unconfirmed")
            raise errors.WalletError(
                exc.code,
                f"{exc.message} — recorded locally as {tx_hash}; check it with "
                f"`tx {tx_hash}` before sending again",
            ) from exc

        record.status = "submitted"
        self.store.update_tx(tx_hash, "submitted")
        return self._finish_send(client, record, wait)

    def _finish_send(self, client: RpcClient, record: store.TxRecord, wait: bool) -> CommandOutput:
        """Optionally wait for the receipt, then render the result."""
        if wait:
            receipt = client.wait_for_receipt(record.hash, RECEIPT_TIMEOUT)
            if receipt:
                # A status of 0x0 means reverted; `or` would swallow that zero.
                status_code = field_int(receipt, "status")
                ok = True if status_code is None else status_code == 1
                record.status = "confirmed" if ok else "failed"
                record.block_number = field_int(receipt, "blockNumber")
                record.gas_used = field_int(receipt, "gasUsed")
                self.store.update_tx(
                    record.hash, record.status, record.block_number, record.gas_used
                )

        explorer = self.network.tx_url(record.hash)
        rows = [
            ("Hash", record.hash),
            ("From", record.from_address),
            ("To", record.to),
            ("Amount", f"{record.value} {self.network.symbol}"),
            ("Nonce", str(record.nonce)),
            ("Status", record.status),
        ]
        if record.block_number is not None:
            rows.append(("Block", str(record.block_number)))
        rows.append(("Explorer", explorer))

        data = record.to_json()
        data["explorer"] = explorer
        data["symbol"] = self.network.symbol
        return CommandOutput(data, output.table(rows))

    def tx(self, tx_hash: str) -> CommandOutput:
        client = self.rpc()
        transaction = client.get_transaction_by_hash(tx_hash)
        receipt = client.get_transaction_receipt(tx_hash)
        if transaction is None and receipt is None:
            raise errors.not_found(f"no transaction {tx_hash} on {self.network.name}")

        if receipt is None:
            status = "pending"
        else:
            code = field_int(receipt, "status")
            status = "pending" if code is None else ("confirmed" if code == 1 else "failed")
            # Keep the local log in step with what the chain says.
            try:
                self.store.update_tx(
                    tx_hash,
                    status,
                    field_int(receipt, "blockNumber"),
                    field_int(receipt, "gasUsed"),
                )
            except errors.WalletError:  # pragma: no cover - defensive
                pass

        value = None
        if isinstance(transaction, dict) and "value" in transaction:
            try:
                value = units.format_ether(parse_quantity(transaction["value"], "value"))
            except errors.WalletError:
                value = None

        return CommandOutput(
            {
                "hash": tx_hash,
                "status": status,
                "network": self.network.key,
                "explorer": self.network.tx_url(tx_hash),
                "value": value,
                "transaction": transaction,
                "receipt": receipt,
            },
            output.table(
                [
                    ("Hash", tx_hash),
                    ("Status", status),
                    ("Value", f"{value} {self.network.symbol}" if value is not None else "-"),
                    ("Explorer", self.network.tx_url(tx_hash)),
                ]
            ),
        )

    def history(self, limit: int = 20, network_filter: str | None = None) -> CommandOutput:
        entries = self.store.history()
        if network_filter:
            key = networks.find(network_filter).key
            entries = [tx for tx in entries if tx.network == key]
        entries = list(reversed(entries))[: max(limit, 0)]

        if not entries:
            human = "No transactions recorded yet."
        else:
            human = "\n".join(
                f"{tx.created_at[:19]}  {tx.status:<10} {tx.value:>12} -> {tx.to}  {tx.hash}"
                for tx in entries
            )
        return CommandOutput([tx.to_json() for tx in entries], human)

    # ============================================================== signatures

    def sign(self, message: str, account: str | None = None) -> CommandOutput:
        signer = self.pick_account(account)
        keypair = wallet.Keypair.from_private_key(signer.private_key)
        signature = keypair.sign_message(message)
        return CommandOutput(
            {
                "address": signer.address,
                "account": signer.label,
                "message": message,
                "signature": signature,
            },
            output.table([("Signer", signer.address), ("Signature", signature)]),
        )

    def verify(self, message: str, signature: str, address: str | None = None) -> CommandOutput:
        recovered = wallet.recover_message(message, signature)
        expected = wallet.parse_address(address) if address else None
        valid = (expected == recovered) if expected else True

        if expected and valid:
            human = f"Valid — signed by {expected}"
        elif expected:
            human = f"INVALID — signature recovers to {recovered} but {expected} was expected"
        else:
            human = f"Signed by {recovered}"
        return CommandOutput(
            {
                "valid": valid,
                "recovered": recovered,
                "expected": expected,
                "message": message,
            },
            human,
        )

    # =================================================================== erc20

    def erc20_info(self, token: str) -> CommandOutput:
        contract = wallet.parse_address(token)
        client = self.rpc()
        name = erc20.decode_string(
            client.eth_call(contract, erc20.encode_getter(erc20.SELECTOR_NAME))
        )
        symbol = erc20.decode_string(
            client.eth_call(contract, erc20.encode_getter(erc20.SELECTOR_SYMBOL))
        )
        decimals = erc20.decode_u8(
            client.eth_call(contract, erc20.encode_getter(erc20.SELECTOR_DECIMALS))
        )
        supply = erc20.decode_uint(
            client.eth_call(contract, erc20.encode_getter(erc20.SELECTOR_TOTAL_SUPPLY))
        )
        return CommandOutput(
            {
                "token": contract,
                "name": name,
                "symbol": symbol,
                "decimals": decimals,
                "total_supply": units.format_units(supply, decimals),
                "total_supply_raw": str(supply),
                "network": self.network.key,
            },
            output.table(
                [
                    ("Token", contract),
                    ("Name", name),
                    ("Symbol", symbol),
                    ("Decimals", str(decimals)),
                    ("Total supply", units.format_units(supply, decimals)),
                ]
            ),
        )

    def erc20_balance(self, token: str, address: str | None = None) -> CommandOutput:
        contract = wallet.parse_address(token)
        owner = (
            wallet.parse_address(address)
            if address
            else wallet.parse_address(self.store.active_account().address)
        )
        client = self.rpc()
        decimals = erc20.decode_u8(
            client.eth_call(contract, erc20.encode_getter(erc20.SELECTOR_DECIMALS))
        )
        try:
            symbol = erc20.decode_string(
                client.eth_call(contract, erc20.encode_getter(erc20.SELECTOR_SYMBOL))
            )
        except errors.WalletError:
            symbol = ""
        raw = erc20.decode_uint(client.eth_call(contract, erc20.encode_balance_of(owner)))
        formatted = units.format_units(raw, decimals)
        return CommandOutput(
            {
                "token": contract,
                "address": owner,
                "balance": formatted,
                "balance_raw": str(raw),
                "decimals": decimals,
                "symbol": symbol,
                "network": self.network.key,
            },
            f"{formatted} {symbol} — {owner}",
        )

    def erc20_send(
        self,
        token: str,
        to: str,
        amount: str,
        wait: bool = False,
        account: str | None = None,
    ) -> CommandOutput:
        sender = self.pick_account(account)
        keypair = wallet.Keypair.from_private_key(sender.private_key)
        contract = wallet.parse_address(token)
        recipient = wallet.parse_address(to)
        # Same rule as a native transfer: sending a token to the account holding
        # it changes no balance and still burns the gas.
        if recipient.lower() == keypair.address.lower():
            raise errors.usage(
                f"the recipient is the sending account ({recipient}); a transfer to "
                "itself moves nothing and still pays the gas"
            )

        client = self.rpc()
        decimals = erc20.decode_u8(
            client.eth_call(contract, erc20.encode_getter(erc20.SELECTOR_DECIMALS))
        )
        raw_amount = units.parse_units(amount, decimals)
        held = erc20.decode_uint(
            client.eth_call(contract, erc20.encode_balance_of(keypair.address))
        )
        if held < raw_amount:
            raise errors.insufficient_funds(
                f"token balance {units.format_units(held, decimals)} is less than {amount}"
            )

        payload = erc20.encode_transfer(recipient, raw_amount)
        try:
            gas_limit = with_headroom(client.estimate_gas(keypair.address, contract, 0, payload))
        except errors.WalletError:
            gas_limit = 100_000
        gas_price = client.gas_price()
        nonce = client.get_transaction_count(keypair.address)

        self.confirm(
            f"Transfer {amount} of token {contract} from {sender.label} to "
            f"{recipient} on {self.network.name}"
        )

        signed = LegacyTransaction(
            nonce=nonce,
            gas_price=gas_price,
            gas_limit=gas_limit,
            to=contract,
            value=0,
            chain_id=self.network.chain_id,
            data=payload,
        ).sign(keypair.private_key)
        tx_hash = client.send_raw_transaction(signed.raw)

        record = store.TxRecord(
            hash=tx_hash,
            from_address=keypair.address,
            to=recipient,
            value=amount,
            value_wei=str(raw_amount),
            network=self.network.key,
            chain_id=self.network.chain_id,
            nonce=nonce,
            gas_limit=gas_limit,
            gas_price_wei=str(gas_price),
            status="submitted",
            created_at=store.now_rfc3339(),
            token=contract,
        )
        self.store.record_tx(record)
        return self._finish_send(client, record, wait)

    # =================================================================== utils

    def utils_keccak(self, value: str, as_hex: bool = False) -> CommandOutput:
        data = wallet.parse_hex(value) if as_hex else value.encode("utf-8")
        digest = "0x" + keccak(data).hex()
        return CommandOutput({"input": value, "keccak256": digest}, digest)

    def utils_checksum(self, address: str) -> CommandOutput:
        parsed = wallet.parse_address(address)
        return CommandOutput({"address": parsed}, parsed)

    def utils_to_wei(self, amount: str, decimals: int = 18) -> CommandOutput:
        value = units.parse_units(amount, decimals)
        return CommandOutput(
            {"amount": amount, "decimals": decimals, "value": str(value)}, str(value)
        )

    def utils_from_wei(self, value: str, decimals: int = 18) -> CommandOutput:
        raw = units.parse_int(value)
        amount = units.format_units(raw, decimals)
        return CommandOutput({"value": str(raw), "decimals": decimals, "amount": amount}, amount)

    def utils_new_mnemonic(self, words: int = 12) -> CommandOutput:
        phrase = wallet.generate_mnemonic(words)
        return CommandOutput({"mnemonic": phrase, "words": words}, phrase)

    def utils_derive_mnemonic(
        self, phrase: str, index: int = 0, passphrase: str = ""
    ) -> CommandOutput:
        """Derive from a mnemonic and show the result, storing nothing.

        The same derivation ``account import-mnemonic`` does, with the wallet
        left out of it — for a caller that wants an address from a phrase
        without acquiring an account and a recall entry as side effects.
        """
        keypair = wallet.Keypair.from_mnemonic(phrase, index, passphrase)
        return self._derived(
            keypair,
            {
                "source": "mnemonic",
                "derivation_path": wallet.ethereum_path(index),
                "index": index,
            },
        )

    def utils_derive_key(self, private_key: str) -> CommandOutput:
        """Derive from a private key and show the result, storing nothing."""
        return self._derived(
            wallet.Keypair.from_private_key(private_key), {"source": "private_key"}
        )

    def _derived(self, keypair: wallet.Keypair, extra: dict) -> CommandOutput:
        """The shared shape of a ``utils derive`` result, whatever it came from."""
        data = {
            "address": keypair.address,
            "private_key": keypair.private_key,
            "public_key": keypair.public_key,
            "public_key_compressed": keypair.public_key_compressed,
            **extra,
        }
        rows = [("Address", keypair.address), ("Source", data["source"])]
        if "derivation_path" in data:
            rows.append(("Path", data["derivation_path"]))
            rows.append(("Address index", str(data["index"])))
        rows.append(("Public key", keypair.public_key_compressed))
        rows.append(("Private key", keypair.private_key))
        return CommandOutput(data, output.table(rows))

    def utils_sign(self, private_key: str, message: str) -> CommandOutput:
        """Sign a message with a key the wallet does not hold."""
        keypair = wallet.Keypair.from_private_key(private_key)
        signature = keypair.sign_message(message)
        return CommandOutput(
            {"address": keypair.address, "message": message, "signature": signature},
            signature,
        )

    def utils_validate_mnemonic(self, phrase: str) -> CommandOutput:
        """Report whether a phrase is a valid mnemonic, and why not when it is not.

        An invalid phrase is the answer here, not a failure — which is the
        whole difference from ``account import-mnemonic``.
        """
        words = len(phrase.split())
        reason = _mnemonic_problem(phrase, words)
        valid = reason is None
        human = f"Valid — {words} words" if valid else f"Not a valid mnemonic: {reason}"
        return CommandOutput({"valid": valid, "words": words, "reason": reason}, human)

    def info(self) -> CommandOutput:
        from . import __version__

        accounts = self.store.accounts()
        try:
            active: store.Account | None = self.store.active_account()
        except errors.WalletError:
            active = None
        rpc_url = self.network.resolve_rpc(self.store.config_get(self.network.rpc_config_key))
        data: dict[str, Any] = {
            "version": __version__,
            "home": str(self.store.home),
            "files": {
                "accounts": str(self.store.accounts_path),
                "config": str(self.store.config_path),
                "history": str(self.store.history_path),
                "recent": str(self.store.recent_path),
            },
            "accounts": len(accounts),
            "remembered": len(self.store.recent()),
            "active_account": active.label if active else None,
            "active_address": active.address if active else None,
            "network": self.network.key,
            "chain_id": self.network.chain_id,
            "rpc": rpc_url,
        }
        return CommandOutput(
            data,
            output.table(
                [
                    ("Version", __version__),
                    ("Home", str(self.store.home)),
                    ("Accounts", str(len(accounts))),
                    ("Remembered", str(len(self.store.recent()))),
                    (
                        "Active",
                        f"{active.label} ({active.address})" if active else "-",
                    ),
                    ("Network", f"{self.network.name} (chain {self.network.chain_id})"),
                    ("RPC", rpc_url),
                ]
            ),
        )
