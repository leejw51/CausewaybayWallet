"""An interactive terminal UI over the same ``App`` the CLI uses.

Everything here goes through ``App``, so a TUI session and a sequence of CLI
calls leave the wallet in the same state — and the two front ends cannot drift.

The screen is built around a command list that is always visible: nothing has to
be memorised, the arrow keys move, Enter runs the highlighted command, and every
command also has a single-key shortcut. ``?`` opens a full reference. This
mirrors ``rustcli/src/tui.rs``.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass

from textual.app import App as TextualApp
from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.screen import ModalScreen
from textual.widgets import Footer, Header, Input, ListItem, ListView, Static

from . import clipboard, errors, export, networks, output, store, units, wallet
from .app import App as WalletApp
from .app import SendPlan
from .output import CommandOutput

BANNER = "Causewaybay Wallet — educational, keys stored unencrypted"

# The column a command label is padded into, in both the pane and the help.
LABEL_WIDTH = 19


@dataclass(frozen=True)
class Command:
    """One row of the command pane."""

    action: str
    label: str
    key: str | None
    help: str


def build_commands() -> list[Command]:
    """Build the command pane.

    Networks come from ``networks.ALL`` rather than being written out here, so a
    chain added to the table shows up as an entry without touching this file.
    """
    commands = [
        Command("balance", "Get balance", "b", "Ask the node for the selected wallet's balance"),
        Command("send", "Send amount", "s", "Send the network's native coin to an address"),
        Command("new_address", "New address", "n", "Add the next address of this seed: 0, 1, 2, …"),
        Command("new_seed", "New seed", "N", "Start a separate mnemonic, addresses from 0 again"),
        Command("import_mnemonic", "Import mnemonic", "m", "Import an existing BIP-39 phrase"),
        Command("import_key", "Import priv key", "p", "Import a raw private key"),
        Command("recall", "Recall saved keys", "c", "Reuse a mnemonic or key from the recall list"),
        Command("derive", "Derive address", "d", "Derive another address from this mnemonic"),
        Command("activate", "Set active", "a", "Make the selected wallet the CLI default"),
        Command(
            "copy_address",
            "Copy address",
            "y",
            "Copy the selected wallet's address to the clipboard",
        ),
        Command("sign", "Sign message", "g", "Sign a message with EIP-191"),
        Command("save_jsonl", "Save list .jsonl", "1", "Write the wallet list to a JSONL file"),
        Command("save_csv", "Save list .csv", "2", "Write the wallet list to a CSV file"),
        Command("save_txt", "Save list .txt", "3", "Write the wallet list to an aligned text file"),
        Command("save_md", "Save list .md", "4", "Write the wallet list to a Markdown table"),
        Command(
            "export_wallets", "Export wallets", "e", "JSONL with private keys and both public keys"
        ),
        Command(
            "toggle_secrets", "Show/hide secret", "v", "Reveal or hide the private key and mnemonic"
        ),
    ]

    # One entry per network, flattened rather than hidden behind a toggle.
    for chain in networks.ALL:
        commands.append(
            Command(
                f"network:{chain.key}",
                chain.name,
                None,
                f"Switch to {chain.name} (chain {chain.chain_id})",
            )
        )

    commands += [
        Command("remove", "Remove wallet", "x", "Forget the selected wallet (asks first)"),
        Command("reload", "Reload from disk", "r", "Re-read the store, picking up CLI changes"),
        Command("help", "Help", "?", "Show this reference"),
        Command("quit", "Quit", "q", "Leave the wallet"),
    ]
    return commands


# The formats the four "Save list" commands write.
SAVE_FORMATS = {
    "save_jsonl": export.JSONL,
    "save_csv": export.CSV,
    "save_txt": export.TXT,
    "save_md": export.MARKDOWN,
}

# Prompts whose text must never be echoed to the screen.
SECRET_PROMPTS = {"mnemonic", "private_key"}


def echo(kind: str, text: str) -> str:
    """What the prompt shows for the text typed so far.

    A mnemonic or a private key is never echoed: someone pasting a seed phrase
    should not have it sitting in a terminal that may be shared, scrolled back,
    or recorded. The mask still reports progress, because "did my paste arrive,
    and is it the right length?" is the question the prompt has to answer.
    """
    if kind == "mnemonic":
        words = len(text.split())
        if words == 0:
            return ""
        return f"{'•' * min(words, 32)} ({words} word{'' if words == 1 else 's'})"
    if kind == "private_key":
        body = text.strip()
        if body[:2].lower() == "0x":
            body = body[2:]
        if not body:
            return ""
        return f"{'•' * min(len(body), 32)} ({len(body)} hex char{'' if len(body) == 1 else 's'})"
    # Labels, addresses, amounts and filenames are not secrets, and seeing them
    # is how a typo gets caught.
    return text


@dataclass
class Prompt:
    """An inline question and what to do with the answer."""

    kind: str
    label: str
    handler: Callable[[str], None]
    preset: str = ""


class HelpScreen(ModalScreen[None]):
    """The full reference, drawn over the top of everything else."""

    BINDINGS = [Binding("escape,q,question_mark,space,enter", "dismiss_help", "Close")]

    CSS = """
    HelpScreen { align: center middle; }
    #help-box { width: 84; max-height: 90%; border: round $accent; background: $surface; padding: 1 2; }
    """

    def __init__(self, commands: list[Command]) -> None:
        super().__init__()
        self.commands = commands

    def compose(self) -> ComposeResult:
        lines = [
            "[b]Navigation[/b]",
            "  ↑ ↓        move within the focused pane",
            "  Enter      run the highlighted command",
            "  Esc        cancel a prompt or leave the recall list",
            "",
            "[b]Commands — each also works as a single key press[/b]",
        ]
        for command in self.commands:
            key = command.key or " "
            lines.append(f"  [yellow]{key}[/yellow]  {command.label:<{LABEL_WIDTH}}{command.help}")
        lines += ["", "[dim]Saved files land in the directory you started the TUI from.[/dim]"]
        with Vertical(id="help-box"):
            yield Static("\n".join(lines))

    def action_dismiss_help(self) -> None:
        self.dismiss(None)


class WalletTUI(TextualApp):
    """The wallet screen."""

    CSS = """
    Screen { layout: vertical; }
    #banner { height: 1; color: $text-muted; }
    #body { height: 1fr; }
    #commands { width: 30; border: round $primary; }
    #accounts { width: 40; border: round $primary; }
    #recall { width: 40; border: round $accent; display: none; }
    #recall.visible { display: block; }
    #accounts.hidden { display: none; }
    #detail { width: 1fr; border: round $primary; padding: 0 1; }
    #status { height: 3; border: round $secondary; padding: 0 1; }
    #prompt { display: none; }
    #prompt.visible { display: block; }
    """

    BINDINGS = [
        Binding("q", "run('quit')", "Quit"),
        Binding("b", "run('balance')", "Balance"),
        Binding("s", "run('send')", "Send"),
        Binding("n", "run('new_address')", "New addr"),
        Binding("N", "run('new_seed')", "New seed"),
        Binding("m", "run('import_mnemonic')", "Mnemonic"),
        Binding("p", "run('import_key')", "Priv key"),
        Binding("c", "run('recall')", "Recall"),
        Binding("d", "run('derive')", "Derive"),
        Binding("a", "run('activate')", "Activate"),
        Binding("y", "run('copy_address')", "Copy"),
        Binding("g", "run('sign')", "Sign"),
        Binding("1", "run('save_jsonl')", "JSONL"),
        Binding("2", "run('save_csv')", "CSV"),
        Binding("3", "run('save_txt')", "TXT"),
        Binding("4", "run('save_md')", "MD"),
        Binding("e", "run('export_wallets')", "Export"),
        Binding("v", "run('toggle_secrets')", "Secrets"),
        Binding("x", "run('remove')", "Remove"),
        Binding("r", "run('reload')", "Reload"),
        Binding("question_mark", "run('help')", "Help"),
    ]

    def __init__(self, wallet_app: WalletApp) -> None:
        # Set before Textual initialises: its machinery may read our state.
        self.wallet = wallet_app
        self.commands = build_commands()
        self.accounts: list[store.Account] = []
        self.recall: list[store.RecentSecret] = []
        self.in_recall = False
        self.show_secrets = False
        self.status_text = "↑↓ move · Enter runs a command · ? for help"
        self.detail_rows: list[tuple[str, str]] = []
        self._prompt: Prompt | None = None
        self._pending_to = ""
        self._pending_amount = ""
        self._staged: SendPlan | None = None
        self._confirm: str | None = None
        super().__init__()

    # ------------------------------------------------------------- lifecycle

    def compose(self) -> ComposeResult:
        yield Header()
        yield Static(BANNER, id="banner")
        with Horizontal(id="body"):
            yield ListView(id="commands")
            yield ListView(id="accounts")
            yield ListView(id="recall")
            with Vertical(id="detail"):
                yield Static("", id="detail-text")
        yield Static(self.status_text, id="status")
        # Disabled until a prompt needs it, so key presses reach the bindings
        # instead of being typed into a hidden field.
        yield Input(placeholder="", id="prompt", disabled=True)
        yield Footer()

    def on_mount(self) -> None:
        self.reload_commands()
        self.refresh_title()
        self.reload_accounts()
        try:
            active = self.wallet.store.active_account()
        except errors.WalletError:
            active = None
        if active:
            for position, account in enumerate(self.accounts):
                if account.id == active.id:
                    self.query_one("#accounts", ListView).index = position
                    break
        self.refresh_detail()
        self.query_one("#commands", ListView).focus()

    def refresh_title(self) -> None:
        """The header names the network and the wallet everything defaults to."""
        network = self.wallet.store.network()
        try:
            active = self.wallet.store.active_account()
            who = f"{active.label}: {active.address}"
        except errors.WalletError:
            who = "no wallet yet"
        self.title = f"Causewaybay Wallet · {network.name} (chain {network.chain_id}) · {who}"

    def reload_commands(self) -> None:
        """Redraw the command pane, marking the network in use."""
        current = self.wallet.store.network().key
        listing = self.query_one("#commands", ListView)
        index = listing.index
        listing.clear()
        for command in self.commands:
            selected = command.action == f"network:{current}"
            marker = "●" if selected else " "
            key = command.key or " "
            listing.append(ListItem(Static(f"{marker}{command.label:<{LABEL_WIDTH}}{key}")))
        listing.index = index if index is not None else 0

    # ------------------------------------------------------------ the panes

    def reload_accounts(self) -> None:
        keep = self.current_account.id if self.current_account else None
        self.accounts = self.wallet.store.accounts()
        listing = self.query_one("#accounts", ListView)
        listing.clear()
        for account in self.accounts:
            listing.append(
                ListItem(Static(f"{account.label:<14} {output.short_address(account.address)}"))
            )
        if self.accounts:
            index = 0
            if keep:
                for position, account in enumerate(self.accounts):
                    if account.id == keep:
                        index = position
                        break
            listing.index = index

    @property
    def current_account(self) -> store.Account | None:
        if not self.accounts:
            return None
        listing = self.query_one("#accounts", ListView)
        index = listing.index if listing.index is not None else 0
        return self.accounts[index] if 0 <= index < len(self.accounts) else None

    @property
    def current_command(self) -> Command | None:
        listing = self.query_one("#commands", ListView)
        index = listing.index if listing.index is not None else 0
        return self.commands[index] if 0 <= index < len(self.commands) else None

    def reload_recall(self) -> None:
        self.recall = self.wallet.store.recent()
        listing = self.query_one("#recall", ListView)
        listing.clear()
        for entry in self.recall:
            listing.append(
                ListItem(
                    Static(
                        f"{entry.kind:<12} {output.short_address(entry.address)}  {entry.preview}"
                    )
                )
            )
        if self.recall:
            listing.index = 0

    @property
    def current_recall(self) -> store.RecentSecret | None:
        if not self.recall:
            return None
        listing = self.query_one("#recall", ListView)
        index = listing.index if listing.index is not None else 0
        return self.recall[index] if 0 <= index < len(self.recall) else None

    # ------------------------------------------------------------ rendering

    def refresh_detail(self) -> None:
        if self.in_recall:
            self.refresh_recall_detail()
            return
        account = self.current_account
        if account is None:
            self.detail_rows = []
            self.query_one("#detail-text", Static).update(
                'No wallet yet.\n\nPick "New address" on the left and press Enter,\n'
                "or press n. Press ? for the full reference."
            )
            return
        rows = [
            ("Label", account.label),
            ("Address", account.address),
            ("Source", account.source),
        ]
        if account.derivation_path:
            rows.append(("Path", account.derivation_path))
        rows.append(
            (
                "Private key",
                account.private_key
                if self.show_secrets
                else output.truncate_secret(account.private_key),
            )
        )
        if account.mnemonic:
            rows.append(
                ("Mnemonic", account.mnemonic if self.show_secrets else "<hidden — press v>")
            )
        rows.append(("Explorer", self.wallet.network.address_url(account.address)))
        self.detail_rows = rows
        self.query_one("#detail-text", Static).update(output.table(rows))

    def refresh_recall_detail(self) -> None:
        entry = self.current_recall
        if entry is None:
            self.detail_rows = []
            self.query_one("#detail-text", Static).update("Nothing remembered yet.")
            return
        rows = [
            ("Id", entry.id),
            ("Kind", entry.kind),
            ("Address", entry.address),
            ("Uses", str(entry.uses)),
            ("Last used", entry.last_used_at),
            (
                "Mnemonic" if entry.kind == store.SOURCE_MNEMONIC else "Private key",
                entry.secret if self.show_secrets else entry.preview,
            ),
        ]
        self.detail_rows = rows
        self.query_one("#detail-text", Static).update(output.table(rows))

    def set_status(self, message: str) -> None:
        self.status_text = message
        self.query_one("#status", Static).update(message)

    def report(self, exc: Exception) -> None:
        message = exc.message if isinstance(exc, errors.WalletError) else str(exc)
        self.set_status(f"error: {message}")

    # --------------------------------------------------------------- prompt

    def ask(self, kind: str, label: str, handler: Callable[[str], None], preset: str = "") -> None:
        self._prompt = Prompt(kind, label, handler, preset)
        field = self.query_one("#prompt", Input)
        field.value = preset
        field.placeholder = label
        # Textual masks the field too, so nothing is on screen either way.
        field.password = kind in SECRET_PROMPTS
        field.disabled = False
        field.add_class("visible")
        field.focus()
        self.set_status(f"{label} > {echo(kind, preset)}")

    def close_prompt(self) -> None:
        self._prompt = None
        field = self.query_one("#prompt", Input)
        field.value = ""
        field.remove_class("visible")
        field.disabled = True
        self.query_one("#commands", ListView).focus()

    def on_input_changed(self, event: Input.Changed) -> None:
        """Keep the status line showing masked progress as the text arrives."""
        if self._prompt is not None:
            self.set_status(f"{self._prompt.label} > {echo(self._prompt.kind, event.value)}")

    def on_input_submitted(self, event: Input.Submitted) -> None:
        pending = self._prompt
        value = event.value.strip()
        self.close_prompt()
        if pending is None:
            return
        try:
            pending.handler(value)
        except errors.WalletError as exc:
            self.report(exc)

    def on_list_view_highlighted(self, event: ListView.Highlighted) -> None:
        if event.list_view.id == "recall":
            self.refresh_recall_detail()
        elif event.list_view.id == "accounts":
            self.refresh_detail()

    def on_list_view_selected(self, event: ListView.Selected) -> None:
        """Enter runs the highlighted command, or imports a remembered key."""
        if event.list_view.id == "commands":
            command = self.current_command
            if command:
                self.dispatch(command.action)
        elif event.list_view.id == "recall":
            self.import_selected_recall()
        elif event.list_view.id == "accounts":
            self.dispatch("balance")

    # ------------------------------------------------------------- dispatch

    def action_run(self, action: str) -> None:
        self.dispatch(action)

    def dispatch(self, action: str) -> None:
        """The single place a key press and a menu selection converge."""
        if action.startswith("network:"):
            self.select_network(action.split(":", 1)[1])
            return
        if action in SAVE_FORMATS:
            self.start_export(SAVE_FORMATS[action])
            return
        handler = getattr(self, f"do_{action}", None)
        if handler is None:
            self.set_status(f"unknown command {action}")
            return
        try:
            handler()
        except errors.WalletError as exc:
            self.report(exc)

    # -------------------------------------------------------------- actions

    def do_quit(self) -> None:
        self.exit()

    def do_help(self) -> None:
        self.push_screen(HelpScreen(self.commands))

    def do_reload(self) -> None:
        self.reload_accounts()
        self.refresh_detail()
        self.set_status("Reloaded from disk")

    def do_new_address(self) -> None:
        self.ask("label", "Label for the next address (blank = auto)", self._create(False))

    def do_new_seed(self) -> None:
        self.ask(
            "label",
            "Label for the first address of a new seed (blank = auto)",
            self._create(True),
        )

    def _create(self, new_seed: bool) -> Callable[[str], None]:
        def handler(label: str) -> None:
            result = self.wallet.account_new(label or None, new_seed=new_seed)
            self.reload_accounts()
            self.refresh_title()
            self.refresh_detail()
            self.set_status(f"Created {result.data['address']}")

        return handler

    def do_import_mnemonic(self) -> None:
        self.ask("mnemonic", "Mnemonic to import", self._import_mnemonic)

    def _import_mnemonic(self, phrase: str) -> None:
        result = self.wallet.account_import_mnemonic(phrase)
        self.reload_accounts()
        self.refresh_title()
        self.refresh_detail()
        self.set_status(f"Imported {result.data['address']}")

    def do_import_key(self) -> None:
        self.ask("private_key", "Private key to import", self._import_key)

    def _import_key(self, key: str) -> None:
        result = self.wallet.account_import_key(key)
        self.reload_accounts()
        self.refresh_title()
        self.refresh_detail()
        self.set_status(f"Imported {result.data['address']}")

    def do_derive(self) -> None:
        if self.current_account is None:
            self.set_status("No wallet selected — create one first")
            return
        self.ask("index", "Address index to derive", self._derive)

    def _derive(self, raw_index: str) -> None:
        if not raw_index.isdigit():
            self.set_status(f"'{raw_index}' is not an address index")
            return
        account = self.current_account
        if account is None:
            return
        result = self.wallet.account_derive(int(raw_index), from_selector=account.id)
        self.reload_accounts()
        self.refresh_detail()
        self.set_status(f"Derived {result.data['address']}")

    def do_activate(self) -> None:
        account = self.current_account
        if account is None:
            self.set_status("No wallet selected")
            return
        self.wallet.store.config_set(store.KEY_ACTIVE_ACCOUNT, account.id)
        self.refresh_title()
        self.set_status(f"{account.label} is now active")

    def do_copy_address(self) -> None:
        """Put the selected wallet's address on the system clipboard."""
        account = self.current_account
        if account is None:
            self.set_status("No wallet selected")
            return
        try:
            helper = clipboard.copy(account.address)
        except errors.WalletError as exc:
            self.set_status(f"{exc.message} — the address is {account.address}")
            return
        self.set_status(f"Copied {account.address} to the clipboard ({helper})")

    def do_remove(self) -> None:
        if self.in_recall:
            self.forget_selected_recall()
            return
        account = self.current_account
        if account is None:
            self.set_status("No wallet selected")
            return
        self.ask("confirm", f"Remove {account.label}? type 'yes' to confirm", self._remove)

    def _remove(self, answer: str) -> None:
        account = self.current_account
        if account is None:
            return
        if answer.lower() not in {"y", "yes"}:
            self.set_status("Cancelled")
            return
        self.wallet.store.delete_account(account.id)
        self.reload_accounts()
        self.refresh_title()
        self.refresh_detail()
        self.set_status(f"Removed {account.label}")

    def do_balance(self) -> None:
        account = self.current_account
        if account is None:
            self.set_status("No wallet selected — create one first")
            return
        self.set_status("Querying balance…")
        result = self.wallet.balance(address=account.address)
        self.detail_rows = [r for r in self.detail_rows if r[0] != "Balance"]
        self.detail_rows.append(("Balance", f"{result.data['balance']} {result.data['symbol']}"))
        self.query_one("#detail-text", Static).update(output.table(self.detail_rows))
        self.set_status(f"Balance {result.data['balance']} {result.data['symbol']}")

    def do_sign(self) -> None:
        if self.current_account is None:
            self.set_status("No wallet selected — create one first")
            return
        self.ask("message", "Message to sign", self._sign)

    def _sign(self, message: str) -> None:
        account = self.current_account
        if account is None:
            return
        keypair = wallet.Keypair.from_private_key(account.private_key)
        signature = keypair.sign_message(message)
        self.detail_rows = [("Message", message), ("Signature", signature)]
        self.query_one("#detail-text", Static).update(output.table(self.detail_rows))
        self.set_status("Signed")

    # ----------------------------------------------------------------- send

    def do_send(self) -> None:
        if self.current_account is None:
            self.set_status("No wallet selected — create one first")
            return
        self._pending_to = ""
        self._pending_amount = ""
        self.ask("address", "Recipient address", self._send_to)

    def _send_to(self, address: str) -> None:
        try:
            self._pending_to = wallet.parse_address(address)
        except errors.WalletError as exc:
            self.report(exc)
            return
        self.ask("amount", "Amount to send", self._send_amount)

    def _send_amount(self, amount: str) -> None:
        try:
            units.parse_ether(amount)
        except errors.WalletError as exc:
            self.report(exc)
            return
        self._pending_amount = amount
        self.stage_send()

    def stage_send(self) -> None:
        """Work out what the transfer costs and ask before signing anything.

        The plan comes from ``App``, so the TUI gets the same nonce and gas
        resolution and the same refusal when the balance cannot cover it.
        """
        account = self.current_account
        if account is None:
            return
        self.set_status("Checking the balance…")
        try:
            plan = self.wallet.plan_send(
                to=self._pending_to, amount=self._pending_amount, account=account.id
            )
        except errors.WalletError as exc:
            self.report(exc)
            return
        fee = plan.gas_price * plan.gas_limit
        self.detail_rows = [
            ("To", plan.to),
            ("Amount", f"{plan.amount} {self.wallet.network.symbol}"),
            ("Nonce", str(plan.nonce)),
            ("Gas limit", str(plan.gas_limit)),
            ("Gas price", f"{units.format_gwei(plan.gas_price)} gwei"),
            ("Max fee", f"{units.format_ether(fee)} {self.wallet.network.symbol}"),
        ]
        self.query_one("#detail-text", Static).update(output.table(self.detail_rows))
        self._staged = plan
        self.ask(
            "confirm",
            f"Send {plan.amount} {self.wallet.network.symbol} to {plan.to}? type 'yes'",
            self._send_confirm,
        )

    def _send_confirm(self, answer: str) -> None:
        plan, self._staged = self._staged, None
        if answer.lower() not in {"y", "yes"}:
            self.set_status("Cancelled")
            return
        if plan is None:
            self.set_status("Nothing staged to send")
            return
        try:
            result = self.wallet.execute_send(plan)
        except errors.WalletError as exc:
            self.report(exc)
            return
        self.detail_rows = [
            ("Sent", result.data["hash"]),
            ("Explorer", result.data.get("explorer", "")),
        ]
        self.query_one("#detail-text", Static).update(output.table(self.detail_rows))
        self.set_status(f"Submitted {result.data['hash']}")

    # --------------------------------------------------------------- export

    def start_export(self, fmt: str) -> None:
        self.ask(
            "path",
            f"Save {len(self.accounts)} wallets as {fmt} to (Enter accepts)",
            lambda path: self._export(fmt, path, self.show_secrets),
            preset=f"wallets.{export.extension(fmt)}",
        )

    def do_export_wallets(self) -> None:
        """The full export: private keys included, so the prompt says so."""
        self.ask(
            "path",
            f"Export {len(self.accounts)} wallets WITH PRIVATE KEYS as jsonl to (Enter accepts)",
            lambda path: self._export(export.JSONL, path, True),
            preset="wallets-keys.jsonl",
        )

    def _export(self, fmt: str, path: str, secrets: bool) -> None:
        from pathlib import Path

        from . import paths as path_helpers

        if not path:
            self.set_status("error: no filename given")
            return
        if not self.accounts:
            self.set_status("error: there are no wallets to save")
            return
        try:
            active_id = self.wallet.store.active_account().id
        except errors.WalletError:
            active_id = None
        rendered = export.render(self.accounts, fmt, active_id, secrets)
        target = Path(path)
        try:
            if secrets:
                # Born 0600: the file holds private keys from its first byte.
                path_helpers.write_private(target, rendered)
            else:
                target.write_text(rendered, encoding="utf-8")
        except OSError as exc:
            self.set_status(f"error: cannot write {path}: {exc}")
            return

        self.detail_rows = [
            ("Saved", str(target.resolve())),
            ("Format", fmt),
            ("Wallets", str(len(self.accounts))),
            ("Secrets", "included" if secrets else "excluded"),
        ]
        self.query_one("#detail-text", Static).update(output.table(self.detail_rows))
        self.set_status(f"Saved {len(self.accounts)} wallets to {path}")

    # -------------------------------------------------------------- network

    def select_network(self, key: str) -> None:
        """Switch to a named network, rather than cycling through them."""
        target = networks.find(key)
        if self.wallet.store.network().key == target.key:
            self.set_status(f"Already on {target.name}")
            return
        self.wallet.store.config_set(store.KEY_NETWORK, target.key)
        # `App` resolves its network once at construction, so balances, the
        # symbol and explorer links would otherwise keep using the old chain
        # under the new chain's header. Rebind rather than only repaint.
        self.wallet = WalletApp(
            home=self.wallet.store.home,
            network_override=target.key,
            assume_yes=True,
        )
        self.reload_commands()
        self.refresh_title()
        self.refresh_detail()
        self.set_status(f"Network is now {target.name} (chain {target.chain_id})")

    # --------------------------------------------------------------- recall

    def do_recall(self) -> None:
        self.in_recall = not self.in_recall
        accounts = self.query_one("#accounts", ListView)
        recall = self.query_one("#recall", ListView)
        if self.in_recall:
            self.reload_recall()
            accounts.add_class("hidden")
            recall.add_class("visible")
            recall.focus()
            self.refresh_recall_detail()
            self.set_status(
                "Enter imports · x forgets · c goes back"
                if self.recall
                else "Nothing remembered yet — create or import a wallet first"
            )
        else:
            recall.remove_class("visible")
            accounts.remove_class("hidden")
            self.query_one("#commands", ListView).focus()
            self.refresh_detail()
            self.set_status("Ready")

    def import_selected_recall(self) -> None:
        entry = self.current_recall
        if entry is None:
            self.set_status("Nothing selected")
            return
        try:
            if entry.kind == store.SOURCE_MNEMONIC:
                result = self.wallet.account_import_mnemonic(entry.secret)
            else:
                result = self.wallet.account_import_key(entry.secret)
        except errors.WalletError as exc:
            self.report(exc)
            return
        self.do_recall()
        self.reload_accounts()
        self.refresh_title()
        self.refresh_detail()
        self.set_status(f"Imported {result.data['address']}")

    def forget_selected_recall(self) -> None:
        entry = self.current_recall
        if entry is None:
            self.set_status("Nothing selected")
            return
        self.wallet.store.forget_secret(entry.id)
        self.reload_recall()
        self.refresh_recall_detail()
        self.set_status(f"Forgot {entry.id}")

    def do_toggle_secrets(self) -> None:
        self.show_secrets = not self.show_secrets
        self.refresh_detail()
        self.set_status(
            "Secrets shown — mind your shoulder" if self.show_secrets else "Secrets hidden"
        )


def run_tui(wallet_app: WalletApp) -> CommandOutput:
    """Run the TUI until the user quits."""
    WalletTUI(wallet_app).run()
    return CommandOutput({"tui": "exited"}, "Left the wallet TUI.")
