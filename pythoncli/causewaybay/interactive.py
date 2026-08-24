"""A menu and a REPL at one prompt, over the binding.

The Rust CLI owns the full-screen terminal UI; this is the smaller thing a
scripting front end wants — a numbered menu for the six things a wallet is used
for, and the same prompt accepting any wallet command typed in full. Nothing
here knows how a command works: it collects answers and hands them to the core.

``run`` takes the streams rather than reading ``sys.stdin`` directly, so a test
can drive a whole session with scripted input and assert on what came out.
"""

from __future__ import annotations

from collections.abc import Callable
from typing import Any, TextIO

from . import errors
from .wallet import Wallet

PROMPT = "cwb> "

EXIT_OK = 0
EXIT_ERROR = 1


class Session:
    """One interactive session: a wallet, and the two streams to talk on."""

    def __init__(
        self, wallet: Wallet, out: TextIO, err: TextIO, read: Callable[[], str | None]
    ) -> None:
        self.wallet = wallet
        self.out = out
        self.err = err
        self.read = read

    def write(self, text: str) -> None:
        self.out.write(text)

    def say(self, message: str) -> None:
        self.write(f"  {message}\n")

    def report(self, failure: errors.WalletError) -> None:
        """Print a wallet failure without ending the session."""
        self.write(f"  error [{failure.code}]: {failure.message}\n")

    def ask(self, question: str) -> str | None:
        """Ask for a line. ``None`` means the input ended."""
        self.write(f"  {question}: ")
        answer = self.read()
        return None if answer is None else answer.strip()

    def choose(self, question: str, items: list[Any], render: Callable[[Any], str]) -> Any | None:
        """Offer a numbered list and return the chosen entry."""
        if not items:
            return None
        self.write("\n")
        for index, item in enumerate(items, start=1):
            self.write(f"  {index}  {render(item)}\n")
        while True:
            answer = self.ask(question)
            if answer is None:
                return None
            if answer.isdigit() and 1 <= int(answer) <= len(items):
                return items[int(answer) - 1]
            self.write("  pick one of the numbers above.\n")


# ----------------------------------------------------------------- the actions


def create_wallet(session: Session) -> None:
    """Make a wallet: one index, derived on every chain at once."""
    label = session.ask("label (blank for an automatic one)")
    if label is None:
        return
    try:
        created = session.wallet.new_account(label=label or None, every_chain=True)
    except errors.WalletError as failure:
        return session.report(failure)
    for account in created if isinstance(created, list) else [created]:
        session.say(f"{account['chain']:<9} {account['label']:<20} {account['address']}")


def list_wallets(session: Session) -> None:
    try:
        accounts = session.wallet.accounts()
    except errors.WalletError as failure:
        return session.report(failure)
    if not accounts:
        return session.say("no wallets yet — create one first.")
    active = session.wallet.info().get("active_account")
    for account in accounts:
        mark = "*" if account["label"] == active else " "
        session.write(
            f"  {mark} {account['label']:<20} {account['chain']:<9} {account['address']}\n"
        )


def select_wallet(session: Session) -> None:
    try:
        accounts = session.wallet.accounts()
    except errors.WalletError as failure:
        return session.report(failure)
    if not accounts:
        return session.say("no wallets yet — create one first.")
    chosen = session.choose(
        "wallet",
        accounts,
        lambda a: f"{a['label']:<20} {a['chain']:<9} {a['address']}",
    )
    if chosen is None:
        return
    try:
        session.wallet.use_account(chosen["address"])
    except errors.WalletError as failure:
        return session.report(failure)
    session.say(f"{chosen['label']} is active")


def show_balance(session: Session) -> None:
    try:
        balance = session.wallet.balance()
    except errors.WalletError as failure:
        return session.report(failure)
    session.say(f"{balance['balance']} {balance['symbol']}")


def switch_chain(session: Session) -> None:
    """Move to another chain, on whichever of its networks was last used.

    The wallet has two axes — the chain and the network within it — and a menu
    that only offered networks made "work on Solana" a matter of knowing which
    keys begin with ``solana-``. This offers the chains themselves, with what
    each one can do beside it.
    """
    try:
        chains = session.wallet.chains()
        info = session.wallet.info()
    except errors.WalletError as failure:
        return session.report(failure)

    here = info.get("chain")

    def render(chain: dict[str, Any]) -> str:
        can = sorted(name for name, allowed in (chain.get("capabilities") or {}).items() if allowed)
        mark = "  (current)" if chain["chain"] == here else ""
        return f"{chain['chain']:<9} {chain['derivation_path']:<22} {', '.join(can)}{mark}"

    chosen = session.choose("chain", chains, render)
    if chosen is None:
        return

    # The chain is settled by the network, so moving to a chain means moving to
    # one of its networks: the one already selected there, or its first.
    target = (chosen.get("networks") or [None])[0]
    for held in info.get("chains") or []:
        if held.get("chain") == chosen["chain"] and held.get("network"):
            target = held["network"]
    if not target:
        return session.say(f"{chosen['chain']} has no networks")
    try:
        session.wallet.use_network(target)
    except errors.WalletError as failure:
        return session.report(failure)
    session.say(f"now on {chosen['name']} · {target}")


def switch_network(session: Session) -> None:
    """Switch the network later commands use, on any chain."""
    try:
        networks = session.wallet.networks()
        current = session.wallet.current_network()
    except errors.WalletError as failure:
        return session.report(failure)

    def render(network: dict[str, Any]) -> str:
        # Only EVM networks have a numeric chain id; the others carry null,
        # which is not a fact about them anyway. What every network has is the
        # chain it belongs to.
        chain_id = network.get("chain_id")
        where = f"{network['chain']} {chain_id}" if isinstance(chain_id, int) else network["chain"]
        mark = "  (current)" if network["key"] == current["key"] else ""
        return f"{network['key']:<17} {where:<11} {network['symbol']:<5}{mark}"

    chosen = session.choose("network", networks, render)
    if chosen is None:
        return
    try:
        session.wallet.use_network(chosen["key"])
    except errors.WalletError as failure:
        return session.report(failure)
    session.say(f"now on {chosen['name']}")


def export_wallets(session: Session) -> None:
    """Write the wallet list to a file, in one of the four formats."""
    fmt = session.choose("format", ["jsonl", "csv", "txt", "md"], str)
    if fmt is None:
        return
    path = session.ask(f"file (blank for wallets.{fmt})")
    if path is None:
        return
    try:
        written = session.wallet.export_accounts(fmt, output=path or f"wallets.{fmt}")
    except errors.WalletError as failure:
        return session.report(failure)
    session.say(f"wrote {written['count']} wallets to {written['path']}")


def reveal_secrets(session: Session) -> None:
    """Print one wallet's private key and mnemonic, after asking."""
    try:
        accounts = session.wallet.accounts()
    except errors.WalletError as failure:
        return session.report(failure)
    if not accounts:
        return session.say("no wallets yet — create one first.")
    chosen = session.choose("wallet", accounts, lambda a: f"{a['label']:<20} {a['address']}")
    if chosen is None:
        return
    answer = session.ask("this prints a private key. type yes to continue")
    if answer != "yes":
        return session.say("nothing was printed.")
    try:
        exported = session.wallet.export_account(chosen["address"])
    except errors.WalletError as failure:
        return session.report(failure)
    session.say(f"address:     {exported['address']}")
    session.say(f"private key: {exported['private_key']}")
    session.say(f"mnemonic:    {exported.get('mnemonic') or ''}")


# The menu. Order is roughly the order a new wallet is used in.
ACTIONS: list[tuple[str, str, Callable[[Session], None]]] = [
    ("1", "create a wallet", create_wallet),
    ("2", "list wallets", list_wallets),
    ("3", "select the active wallet", select_wallet),
    ("4", "balance", show_balance),
    ("5", "export wallets to a file", export_wallets),
    ("6", "reveal a wallet's secrets", reveal_secrets),
    ("7", "switch chain", switch_chain),
    ("8", "switch network", switch_network),
]


def draw_menu(session: Session) -> None:
    session.write("\n")
    for key, label, _ in ACTIONS:
        session.write(f"  {key}  {label}\n")
    session.write("\n")
    session.write("  Pick a number, or type a command — `balance`, `account list`, …\n")
    session.write("  help [command]   menu   quit\n\n")


def run(
    wallet: Wallet, out: TextIO, err: TextIO, read: Callable[[], str | None] | None = None
) -> int:
    """Run the session until the user quits or the input ends."""
    if read is None:

        def read() -> str | None:  # pragma: no cover - real terminal input
            line = out and input()
            return line

    session = Session(wallet, out, err, read)

    try:
        info = wallet.info()
    except errors.WalletError as failure:
        err.write(f"error [{failure.code}]: {failure.message}\n")
        return EXIT_ERROR

    session.write("\n  Causewaybay Wallet — interactive\n")
    session.write(f"  {info['network']} · {info['accounts']} wallets · {info['home']}\n")
    session.write("  Educational software. Keys are stored unencrypted.\n")

    by_key = {key: action for key, _, action in ACTIONS}
    # Drawn once. Redrawing before every prompt is right for a menu and wrong
    # for a REPL, where it would push the last answer off the screen; `menu`
    # brings it back.
    draw_menu(session)

    while True:
        session.write(PROMPT)
        line = session.read()

        # End of input is a quit, not a spin: this has to survive a closed pipe.
        if line is None:
            session.write("\n")
            return EXIT_OK
        line = line.strip()
        if not line:
            continue
        if line in ("q", "quit", "exit"):
            session.say("bye.")
            return EXIT_OK
        if line == "menu":
            draw_menu(session)
            continue
        if line in by_key:
            by_key[line](session)
            continue

        # Anything else is a wallet command, run exactly as the CLI would run
        # it. The two live at one prompt because nothing is ambiguous: menu
        # keys are digits and commands begin with a letter.
        try:
            session.write(wallet.text(line.split()) + "\n")
        except errors.WalletError as failure:
            session.report(failure)
