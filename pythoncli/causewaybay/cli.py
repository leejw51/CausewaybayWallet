"""``cwbwallet`` for Python — the command line, over the Rust core's C ABI.

Deliberately thin. It does not parse the wallet's arguments: it collects argv,
adds the two things a library cannot get for itself (piped stdin, and where to
find the shared library), and prints what comes back. The command surface is
defined once, in Rust, so this CLI cannot drift from ``cwbwallet`` the way a
hand-written second parser would.

What is left for it to decide is what a *terminal* needs: which stream each
thing goes to, what the exit status is, and whether the reply is rendered as
text or as the JSON envelope from ``SPEC.md``.
"""

from __future__ import annotations

import json
import os
import sys
from collections.abc import Sequence
from typing import Any, TextIO

from . import errors
from .wallet import Wallet, open_wallet

PROG = "cwbwallet"

# Exit statuses, matching the Rust CLI so scripts can drive either.
EXIT_OK = 0
EXIT_ERROR = 1
EXIT_USAGE = 2

# The commands only a terminal front end can run. The core refuses them, but it
# can only say "not available here"; this can say where to go instead.
ELSEWHERE = {
    "tui": "the terminal UI is only in the Rust CLI — run `cwbwallet tui`, "
    "or `python -m causewaybay interactive` for the menu",
}

# The one command this front end has that the wallet does not. It is a prompt
# loop rather than a wallet operation, so it never becomes a request: the core
# would rightly refuse it.
INTERACTIVE = "interactive"

# The global flags that consume the word after them. Only the globals need
# listing: every other flag comes after the subcommand, so by then the answer
# has already been found. Without this, `--home /tmp/w tui` reads "/tmp/w" as
# the command.
_VALUE_FLAGS = frozenset({"--home", "--network", "-n", "--chain", "-c"})


def has_flag(argv: Sequence[str], flag: str) -> bool:
    """True if ``argv`` contains ``flag`` as a whole word."""
    for word in argv:
        if word == flag:
            return True
        # Everything after `--` is a positional, not a flag.
        if word == "--":
            return False
    return False


def first_command(argv: Sequence[str]) -> str | None:
    """The first non-flag word: the subcommand, when there is one."""
    skip = False
    for word in argv:
        if skip:
            skip = False
        elif word == "--":
            skip = False
        elif word in _VALUE_FLAGS:
            skip = True
        elif not word.startswith("-"):
            return word
    return None


def wants_stdin(argv: Sequence[str]) -> bool:
    """Whether any argument is the lone ``-`` meaning "read it from stdin"."""
    return any(word == "-" for word in argv)


def exit_status(code: str | None) -> int:
    """The exit status for an error code, matching the Rust CLI."""
    return EXIT_USAGE if code == "usage" else EXIT_ERROR


def envelope_line(envelope: dict[str, Any]) -> str:
    """Rebuild the SPEC.md envelope from an FFI reply.

    The FFI adds a ``human`` field that ``cwbwallet --json`` does not print, so
    it is dropped rather than shipped: ``--json`` output has to be identical
    across implementations, and parity is checked byte for byte.
    """
    if envelope.get("ok"):
        return json.dumps({"ok": True, "data": envelope.get("data")}, separators=(",", ":"))
    failure = envelope.get("error") or {}
    return json.dumps(
        {
            "ok": False,
            "error": {
                "code": failure.get("code", "internal"),
                "message": failure.get("message", ""),
            },
        },
        separators=(",", ":"),
    )


def globals_from(argv: Sequence[str]) -> dict[str, Any]:
    """The globals an interactive session should inherit.

    ``interactive`` never reaches the core, so nothing parses its ``--home``
    for us. Rather than growing a second argument parser, the few globals are
    read directly: there are five, they are fixed, and getting ``--home`` wrong
    would mean opening the wrong wallet.
    """
    found: dict[str, Any] = {}
    words = list(argv)
    wanted = {
        "--home": "home",
        "--network": "network",
        "-n": "network",
        "--chain": "chain",
        "-c": "chain",
    }
    index = 0
    while index < len(words):
        word = words[index]
        inline: str | None = None
        if word.startswith("--") and "=" in word:
            word, inline = word.split("=", 1)

        name = wanted.get(word)
        if name is not None:
            if inline is not None:
                found[name] = inline
            else:
                index += 1
                found[name] = words[index] if index < len(words) else None
        index += 1
    return found


def _session_for(argv: Sequence[str]) -> Wallet:
    """Open the wallet an interactive session should drive."""
    found = globals_from(argv)
    wallet = open_wallet(
        home=found.get("home"),
        network=found.get("network"),
        chain=found.get("chain"),
        # `--yes` is deliberately not inherited. The menu asks its own
        # questions, and a session that silently answered yes to all of them
        # would be the worst of both shapes.
        yes=False,
    )
    # Fail now rather than three questions in, if the home or network is bad.
    wallet.info()
    return wallet


def main(
    argv: Sequence[str] | None = None, out: TextIO | None = None, err: TextIO | None = None
) -> int:
    """Run the CLI, returning an exit status rather than calling ``sys.exit``.

    ``out`` and ``err`` let the tests capture output instead of writing to the
    terminal; they default to the real streams.
    """
    argv = list(sys.argv[1:] if argv is None else argv)
    out = out or sys.stdout
    err = err or sys.stderr

    as_json = has_flag(argv, "--json")
    command = first_command(argv)

    if command in ELSEWHERE:
        err.write(f"error [usage]: {ELSEWHERE[command]}\n")
        return EXIT_USAGE

    if command == INTERACTIVE:
        if as_json:
            # A menu has nobody to read an envelope, and a caller that asked
            # for one is a script that would hang on the first question.
            err.write("error [usage]: `interactive` is a prompt; it has no --json form\n")
            return EXIT_USAGE
        from . import interactive

        try:
            session = _session_for(argv)
        except errors.WalletError as failure:
            err.write(f"error [{failure.code}]: {failure.message}\n")
            return exit_status(failure.code)
        return interactive.run(session, out=out, err=err)

    try:
        wallet = open_wallet(lib=os.environ.get("CAUSEWAYBAY_LIB"))
    except errors.WalletError as failure:
        # A missing library is not a wallet error, and printing it as an
        # envelope would suggest the wallet ran and declined. It did not run.
        err.write(f"error [{failure.code}]: {failure.message}\n")
        return EXIT_ERROR

    try:
        envelope = wallet.envelope(argv, stdin=sys.stdin.read() if wants_stdin(argv) else None)
    except errors.WalletError as failure:
        err.write(f"error [{failure.code}]: {failure.message}\n")
        return EXIT_ERROR

    if as_json:
        # One envelope on stdout, which stays the single machine-readable
        # channel.
        out.write(envelope_line(envelope) + "\n")
        return (
            EXIT_OK
            if envelope.get("ok")
            else exit_status((envelope.get("error") or {}).get("code"))
        )

    if not envelope.get("ok"):
        failure = envelope.get("error") or {}
        err.write(f"error [{failure.get('code', 'internal')}]: {failure.get('message', '')}\n")
        return exit_status(failure.get("code"))

    asked_for_help = has_flag(argv, "--help") or has_flag(argv, "-h")

    # The banner goes to stderr so `cwbwallet account list > file` is clean,
    # and not at all for --help, --version or a bare invocation: the Rust CLI
    # answers those before it has a wallet to warn about, and a warning the
    # other implementation does not print is a parity failure waiting to be
    # noticed.
    ran_a_command = command is not None
    if ran_a_command and not (
        asked_for_help or has_flag(argv, "--version") or has_flag(argv, "-V")
    ):
        warning = wallet.describe().get("warning")
        if warning:
            err.write(warning + "\n")

    out.write((envelope.get("human") or "") + "\n")

    # The help text comes from the core, which does not know about the one
    # command this front end adds.
    if asked_for_help and first_command(argv) is None:
        out.write(
            "\nOnly in this front end:\n  "
            f"{INTERACTIVE}     Create, list, export and send from a menu\n"
        )
    return EXIT_OK


if __name__ == "__main__":  # pragma: no cover - module entry
    raise SystemExit(main())
