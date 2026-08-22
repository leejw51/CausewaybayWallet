"""Causewaybay Wallet — an educational Cronos/EVM wallet.

⚠️  EDUCATIONAL SOFTWARE. Keys are stored unencrypted on disk. Do not use with
funds you are not prepared to lose. For real value use a hardware wallet.

The on-disk format and the command surface are shared with the Rust
implementation in ``rustcli/``; see ``SPEC.md`` at the repository root.
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
