"""Command line surface. Kept in step with ``rustcli/src/cli.rs``."""

from __future__ import annotations

import argparse
import os
import sys
from collections.abc import Sequence
from typing import Any

from . import __version__, errors, output, wallet
from .app import App
from .output import CommandOutput

PROG = "cwbwallet"

EXIT_OK = 0
EXIT_ERROR = 1
EXIT_USAGE = 2


class _Parser(argparse.ArgumentParser):
    """Exit with code 2 on a usage error, matching the Rust CLI."""

    def error(self, message: str):  # pragma: no cover - argparse plumbing
        self.print_usage(sys.stderr)
        sys.stderr.write(f"{self.prog}: error: {message}\n")
        raise SystemExit(EXIT_USAGE)


def _word_count(value: str) -> int:
    """Accept only the five BIP-39 word counts."""
    try:
        count = int(value)
    except ValueError:
        raise argparse.ArgumentTypeError(f"'{value}' is not a number") from None
    if count not in wallet.WORD_COUNTS:
        raise argparse.ArgumentTypeError("word count must be 12, 15, 18, 21 or 24")
    return count


def _non_negative(value: str) -> int:
    try:
        number = int(value)
    except ValueError:
        raise argparse.ArgumentTypeError(f"'{value}' is not a number") from None
    if number < 0:
        raise argparse.ArgumentTypeError("value must not be negative")
    return number


def build_parser() -> argparse.ArgumentParser:
    parser = _Parser(
        prog=PROG,
        description=("Educational Cronos/EVM wallet — CLI and TUI over an append-only JSONL store"),
        epilog=(
            "State lives in ~/.causewaybaywallet as append-only JSONL files. "
            "Pass --json for a machine-readable envelope on every command."
        ),
    )
    parser.add_argument("--version", action="version", version=f"{PROG} {__version__}")

    # Global flags live on the top-level parser with real defaults, and are
    # repeated on every subparser with SUPPRESS so that `--json balance` and
    # `balance --json` both work and the later parse never clobbers the earlier.
    def add_globals(target: argparse.ArgumentParser, *, network: bool, suppress: bool) -> None:
        default = argparse.SUPPRESS if suppress else None
        target.add_argument(
            "--json",
            action="store_true",
            default=argparse.SUPPRESS if suppress else False,
            help="emit a single-line JSON envelope",
        )
        target.add_argument(
            "--home",
            metavar="PATH",
            default=default,
            help="override the wallet home directory",
        )
        if network:
            target.add_argument(
                "-n",
                "--network",
                metavar="NETWORK",
                default=default,
                help="use this network for one invocation",
            )
        target.add_argument(
            "-y",
            "--yes",
            action="store_true",
            default=argparse.SUPPRESS if suppress else False,
            help="skip confirmation prompts",
        )

    add_globals(parser, network=True, suppress=False)

    globals_parser = argparse.ArgumentParser(add_help=False)
    add_globals(globals_parser, network=True, suppress=True)

    # `history --network` is a result filter, so that subcommand takes the
    # global flags without the network override.
    history_globals = argparse.ArgumentParser(add_help=False)
    add_globals(history_globals, network=False, suppress=True)

    subs = parser.add_subparsers(dest="command", metavar="COMMAND")
    base: dict[str, Any] = {"parents": [globals_parser]}

    # ------------------------------------------------------------- account
    account = subs.add_parser("account", help="manage accounts", **base)
    account_subs = account.add_subparsers(dest="account_command", metavar="SUBCOMMAND")

    new = account_subs.add_parser(
        "new",
        help="add the next address of the wallet's mnemonic",
        description=(
            "A wallet holds one mnemonic and many addresses derived from it, so this "
            "continues the sequence: 0, 1, 2, … Use --new-seed to start a separate "
            "mnemonic instead, and --index to pick a specific one."
        ),
        **base,
    )
    new.add_argument("-l", "--label")
    new.add_argument(
        "--new-seed",
        action="store_true",
        help="generate a fresh mnemonic instead of using the wallet's",
    )
    new.add_argument(
        "-w", "--words", type=_word_count, default=12, help="only meaningful with --new-seed"
    )
    new.add_argument("-i", "--index", type=_non_negative, help="defaults to the next free index")
    new.add_argument("--show-secret", action="store_true", help="also print the mnemonic")

    import_mnemonic = account_subs.add_parser(
        "import-mnemonic", help="import an existing BIP-39 mnemonic", **base
    )
    import_mnemonic.add_argument("-m", "--mnemonic", help="the mnemonic; '-' reads stdin")
    import_mnemonic.add_argument("-i", "--index", type=_non_negative, default=0)
    import_mnemonic.add_argument("-l", "--label")
    import_mnemonic.add_argument("--passphrase", default="", help="BIP-39 passphrase")

    import_key = account_subs.add_parser("import-key", help="import a raw private key", **base)
    import_key.add_argument("-k", "--private-key", help="the key; '-' reads stdin")
    import_key.add_argument("-l", "--label")

    account_list = account_subs.add_parser("list", help="list every account", **base)
    account_list.add_argument(
        "--format",
        dest="fmt",
        choices=["jsonl", "csv", "txt", "md"],
        help="render the list as a file format instead of the usual output",
    )
    account_list.add_argument("-o", "--output", help="write to this file instead of stdout")
    account_list.add_argument(
        "--secret",
        dest="list_secret",
        action="store_true",
        help="include private keys and mnemonics in the export",
    )

    show = account_subs.add_parser("show", help="show one account", **base)
    show.add_argument("selector", nargs="?")
    show.add_argument("--secret", action="store_true", help="include private key and mnemonic")

    use = account_subs.add_parser("use", help="make an account the default", **base)
    use.add_argument("selector")

    derive = account_subs.add_parser(
        "derive", help="derive another address from a mnemonic account", **base
    )
    derive.add_argument("-i", "--index", type=_non_negative, required=True)
    derive.add_argument("-l", "--label")
    derive.add_argument("--from", dest="from_selector")

    rename = account_subs.add_parser("rename", help="change an account's label", **base)
    rename.add_argument("selector")
    rename.add_argument("label")

    remove = account_subs.add_parser("remove", help="forget an account", **base)
    remove.add_argument("selector")

    export = account_subs.add_parser("export", help="print an account's secrets", **base)
    export.add_argument("selector", nargs="?")

    import_recent = account_subs.add_parser(
        "import-recent", help="create an account from remembered key material", **base
    )
    import_recent.add_argument(
        "selector", nargs="?", help="recall id, 1-based position, or address"
    )
    import_recent.add_argument("-i", "--index", type=_non_negative, default=0)
    import_recent.add_argument("-l", "--label")
    import_recent.add_argument(
        "--passphrase",
        default="",
        help="the BIP-39 passphrase, when the entry was created with one",
    )

    # ------------------------------------------------------------- recall
    recent = subs.add_parser("recent", help="recall mnemonics and private keys used before", **base)
    recent_subs = recent.add_subparsers(dest="recent_command", metavar="SUBCOMMAND")

    recent_list = recent_subs.add_parser("list", help="list remembered key material", **base)
    recent_list.add_argument("--kind", choices=["mnemonic", "private-key"])
    recent_list.add_argument("--limit", type=_non_negative, default=20)

    recent_show = recent_subs.add_parser("show", help="show one remembered entry", **base)
    recent_show.add_argument("selector", nargs="?")
    recent_show.add_argument(
        "--secret", action="store_true", help="reveal the mnemonic or private key"
    )

    recent_forget = recent_subs.add_parser("forget", help="drop one entry", **base)
    recent_forget.add_argument("selector")

    recent_subs.add_parser("clear", help="drop every remembered entry", **base)

    # ------------------------------------------------------------- network
    network = subs.add_parser("network", help="inspect and switch networks", **base)
    network_subs = network.add_subparsers(dest="network_command", metavar="SUBCOMMAND")
    network_subs.add_parser("list", help="list the supported networks", **base)
    network_subs.add_parser("current", help="show the selected network", **base)
    network_use = network_subs.add_parser("use", help="change the default network", **base)
    network_use.add_argument("network_key", metavar="NETWORK")
    set_rpc = network_subs.add_parser("set-rpc", help="override a network's RPC URL", **base)
    set_rpc.add_argument("network_key", metavar="NETWORK")
    set_rpc.add_argument("url", help="the RPC endpoint; empty restores the default")

    # -------------------------------------------------------- chain queries
    balance = subs.add_parser("balance", help="show the native token balance", **base)
    balance.add_argument("-a", "--address")
    balance.add_argument("--account")

    nonce = subs.add_parser("nonce", help="show the next transaction nonce", **base)
    nonce.add_argument("-a", "--address")
    nonce.add_argument("--account")

    subs.add_parser("gas-price", help="show the current gas price", **base)
    subs.add_parser("chain-info", help="show chain id and latest block", **base)

    # ------------------------------------------------------------------ send
    send = subs.add_parser("send", help="send native CRO/TCRO", **base)
    send.add_argument("--to", required=True)
    send.add_argument("--amount", required=True)
    send.add_argument("--gas-limit", type=_non_negative)
    send.add_argument("--gas-price-gwei")
    send.add_argument("--nonce", type=_non_negative)
    send.add_argument("--data", help="hex call data to attach")
    send.add_argument("--wait", action="store_true", help="wait for the receipt")
    send.add_argument("--account")

    tx = subs.add_parser("tx", help="look a transaction up on chain", **base)
    tx.add_argument("hash")

    history = subs.add_parser(
        "history",
        help="list transactions this wallet sent",
        parents=[history_globals],
    )
    history.add_argument("--limit", type=_non_negative, default=20)
    history.add_argument(
        "--network", dest="network_filter", help="only show this network's transactions"
    )

    # ------------------------------------------------------------ signatures
    sign = subs.add_parser("sign", help="sign a message with EIP-191", **base)
    sign.add_argument("message", help="the message; '-' reads stdin")
    sign.add_argument("--account")

    verify = subs.add_parser("verify", help="verify an EIP-191 signature", **base)
    verify.add_argument("--message", required=True, help="the message; '-' reads stdin")
    verify.add_argument("--signature", required=True)
    verify.add_argument("--address")

    # ----------------------------------------------------------------- erc20
    token = subs.add_parser("erc20", help="read and transfer ERC-20 tokens", **base)
    token_subs = token.add_subparsers(dest="erc20_command", metavar="SUBCOMMAND")

    token_info = token_subs.add_parser("info", help="show token metadata", **base)
    token_info.add_argument("-t", "--token", required=True)

    token_balance = token_subs.add_parser("balance", help="show a token balance", **base)
    token_balance.add_argument("-t", "--token", required=True)
    token_balance.add_argument("-a", "--address")

    token_send = token_subs.add_parser("send", help="transfer tokens", **base)
    token_send.add_argument("-t", "--token", required=True)
    token_send.add_argument("--to", required=True)
    token_send.add_argument("--amount", required=True)
    token_send.add_argument("--wait", action="store_true")
    token_send.add_argument("--account")

    # ----------------------------------------------------------------- utils
    utils = subs.add_parser("utils", help="offline helpers", **base)
    utils_subs = utils.add_subparsers(dest="utils_command", metavar="SUBCOMMAND")

    keccak_cmd = utils_subs.add_parser("keccak", help="keccak256 of a string", **base)
    keccak_cmd.add_argument("input")
    keccak_cmd.add_argument("--hex", action="store_true", help="treat input as hex bytes")

    checksum = utils_subs.add_parser("checksum", help="apply the EIP-55 checksum", **base)
    checksum.add_argument("address")

    to_wei = utils_subs.add_parser("to-wei", help="decimal amount -> smallest unit", **base)
    to_wei.add_argument("amount")
    to_wei.add_argument("-d", "--decimals", type=_non_negative, default=18)

    from_wei = utils_subs.add_parser("from-wei", help="smallest unit -> decimal amount", **base)
    from_wei.add_argument("value")
    from_wei.add_argument("-d", "--decimals", type=_non_negative, default=18)

    new_mnemonic = utils_subs.add_parser(
        "new-mnemonic", help="generate a mnemonic without storing it", **base
    )
    new_mnemonic.add_argument("-w", "--words", type=_word_count, default=12)

    # ------------------------------------------------------------------- misc
    subs.add_parser("tui", help="launch the interactive terminal UI", **base)
    subs.add_parser("info", help="report where state lives and what is configured", **base)

    return parser


def read_secret(value: str | None, what: str, env_var: str) -> str:
    """Take the secret from the flag, the environment, or stdin."""
    if value is not None and value != "-":
        if not value.strip():
            raise errors.usage(f"the {what} is empty")
        return value.strip()
    if value is None:
        from_env = os.environ.get(env_var, "").strip()
        if from_env:
            return from_env
    text = sys.stdin.read().strip()
    if not text:
        raise errors.usage(f"no {what} supplied on stdin")
    return text


def read_message(message: str) -> str:
    """``-`` means the message is on stdin; anything else is the message itself."""
    if message != "-":
        return message
    # Strip only the trailing newline a shell adds, not meaningful whitespace.
    text = sys.stdin.read()
    return text[:-1] if text.endswith("\n") else text


def dispatch(app: App, args: argparse.Namespace) -> CommandOutput:
    """Route parsed arguments to the matching ``App`` method."""
    command = args.command

    if command == "account":
        return _dispatch_account(app, args)
    if command == "network":
        return _dispatch_network(app, args)
    if command == "erc20":
        return _dispatch_erc20(app, args)
    if command == "utils":
        return _dispatch_utils(app, args)
    if command == "recent":
        return _dispatch_recent(app, args)

    if command == "balance":
        return app.balance(args.address, args.account)
    if command == "nonce":
        return app.nonce(args.address, args.account)
    if command == "gas-price":
        return app.gas_price()
    if command == "chain-info":
        return app.chain_info()
    if command == "send":
        return app.send(
            to=args.to,
            amount=args.amount,
            gas_limit=args.gas_limit,
            gas_price_gwei=args.gas_price_gwei,
            nonce=args.nonce,
            data=args.data,
            wait=args.wait,
            account=args.account,
        )
    if command == "tx":
        return app.tx(args.hash)
    if command == "history":
        return app.history(args.limit, args.network_filter)
    if command == "sign":
        return app.sign(read_message(args.message), args.account)
    if command == "verify":
        return app.verify(read_message(args.message), args.signature, args.address)
    if command == "info":
        return app.info()
    if command == "tui":
        from .tui import run_tui

        return run_tui(app)

    raise errors.usage(f"unknown command '{command}'")


def _dispatch_account(app: App, args: argparse.Namespace) -> CommandOutput:
    sub = args.account_command
    if sub == "new":
        return app.account_new(args.label, args.new_seed, args.words, args.index, args.show_secret)
    if sub == "import-mnemonic":
        phrase = read_secret(args.mnemonic, "mnemonic", "CAUSEWAYBAY_MNEMONIC")
        return app.account_import_mnemonic(phrase, args.index, args.label, args.passphrase)
    if sub == "import-key":
        key = read_secret(args.private_key, "private key", "CAUSEWAYBAY_PRIVATE_KEY")
        return app.account_import_key(key, args.label)
    if sub == "list":
        return app.account_list(args.fmt, args.output, args.list_secret)
    if sub == "show":
        return app.account_show(args.selector, args.secret)
    if sub == "use":
        return app.account_use(args.selector)
    if sub == "derive":
        return app.account_derive(args.index, args.label, args.from_selector)
    if sub == "rename":
        return app.account_rename(args.selector, args.label)
    if sub == "remove":
        return app.account_remove(args.selector)
    if sub == "export":
        return app.account_export(args.selector)
    if sub == "import-recent":
        return app.account_import_recent(args.selector, args.index, args.label, args.passphrase)
    raise errors.usage("account: pick a subcommand (see `account --help`)")


def _dispatch_recent(app: App, args: argparse.Namespace) -> CommandOutput:
    sub = args.recent_command
    if sub == "list":
        return app.recent_list(args.kind, args.limit)
    if sub == "show":
        return app.recent_show(args.selector, args.secret)
    if sub == "forget":
        return app.recent_forget(args.selector)
    if sub == "clear":
        return app.recent_clear()
    raise errors.usage("recent: pick a subcommand (see `recent --help`)")


def _dispatch_network(app: App, args: argparse.Namespace) -> CommandOutput:
    sub = args.network_command
    if sub == "list":
        return app.network_list()
    if sub == "current":
        return app.network_current()
    if sub == "use":
        return app.network_use(args.network_key)
    if sub == "set-rpc":
        return app.network_set_rpc(args.network_key, args.url)
    raise errors.usage("network: pick a subcommand (see `network --help`)")


def _dispatch_erc20(app: App, args: argparse.Namespace) -> CommandOutput:
    sub = args.erc20_command
    if sub == "info":
        return app.erc20_info(args.token)
    if sub == "balance":
        return app.erc20_balance(args.token, args.address)
    if sub == "send":
        return app.erc20_send(args.token, args.to, args.amount, args.wait, args.account)
    raise errors.usage("erc20: pick a subcommand (see `erc20 --help`)")


def _dispatch_utils(app: App, args: argparse.Namespace) -> CommandOutput:
    sub = args.utils_command
    if sub == "keccak":
        return app.utils_keccak(args.input, args.hex)
    if sub == "checksum":
        return app.utils_checksum(args.address)
    if sub == "to-wei":
        return app.utils_to_wei(args.amount, args.decimals)
    if sub == "from-wei":
        return app.utils_from_wei(args.value, args.decimals)
    if sub == "new-mnemonic":
        return app.utils_new_mnemonic(args.words)
    raise errors.usage("utils: pick a subcommand (see `utils --help`)")


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    if not args.command:
        parser.print_help()
        return EXIT_OK

    try:
        app = App(
            home=args.home,
            network_override=args.network,
            json_mode=args.json,
            assume_yes=args.yes,
        )
        result = dispatch(app, args)
    except errors.WalletError as exc:
        if args.json:
            # Machine callers read one envelope; stdout stays the single channel.
            print(output.error_envelope(exc))
        else:
            print(f"error [{exc.code}]: {exc.message}", file=sys.stderr)
        return EXIT_USAGE if exc.code == errors.USAGE else EXIT_ERROR
    except KeyboardInterrupt:  # pragma: no cover - interactive only
        print("interrupted", file=sys.stderr)
        return EXIT_ERROR

    if args.json:
        print(output.success_envelope(result.data))
    else:
        if args.command != "tui":
            # The TUI has drawn its own farewell; do not repeat the banner.
            print(output.WARNING, file=sys.stderr)
        print(result.human)
    return EXIT_OK


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(main())
