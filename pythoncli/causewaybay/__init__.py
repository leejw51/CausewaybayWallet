"""Causewaybay Wallet for Python — a binding over the Rust core.

⚠️  EDUCATIONAL SOFTWARE. Keys are stored unencrypted on disk. Do not use with
funds you are not prepared to lose. For real value use a hardware wallet.

The wallet itself is in ``rustcli/``: the key derivation for all four chains,
the append-only store, the RPC and the command surface. This package loads that
core through its C ABI and gives Python a wallet object over it, the way
``luacli/`` does for Lua and ``ccli/`` does for C.

    from causewaybay import open_wallet

    wallet = open_wallet()
    for account in wallet.accounts():
        print(account["label"], account["chain"], account["address"])

See ``SPEC.md`` at the repository root for the envelope shape and the error
codes.
"""

from __future__ import annotations

import re
from importlib import metadata
from pathlib import Path

DISTRIBUTION = "causewaybay-wallet"


def _detect_version() -> str:
    """The version of record, never a second copy of it.

    ``pyproject.toml`` holds the number and the release checks compare it with
    ``rustcli/Cargo.toml``; a literal repeated here would be free to drift from
    both and a binary would then report a version nothing verified.
    """
    try:
        return metadata.version(DISTRIBUTION)
    except metadata.PackageNotFoundError:
        pass
    # A source checkout that was never installed — there is no distribution to
    # ask, so read the manifest that sits beside the package.
    manifest = Path(__file__).resolve().parent.parent / "pyproject.toml"
    try:
        found = re.search(r'^version = "(.+)"', manifest.read_text(encoding="utf-8"), re.M)
    except OSError:
        return "0.0.0+unknown"
    return found.group(1) if found else "0.0.0+unknown"


__version__ = _detect_version()

from .errors import WalletError  # noqa: E402  (after __version__, which it does not need)
from .wallet import COMMANDS, Wallet, open_wallet  # noqa: E402

__all__ = ["Wallet", "WalletError", "COMMANDS", "open_wallet", "__version__"]
