"""Resolution and creation of the wallet home directory."""

from __future__ import annotations

import os
from pathlib import Path

from . import errors

HOME_ENV = "CAUSEWAYBAY_HOME"
DEFAULT_DIR = ".causewaybaywallet"

DIR_MODE = 0o700
FILE_MODE = 0o600


def resolve_home(explicit: str | os.PathLike[str] | None = None) -> Path:
    """Explicit flag, then ``CAUSEWAYBAY_HOME``, then ``~/.causewaybaywallet``."""
    if explicit:
        return Path(explicit).expanduser()

    from_env = os.environ.get(HOME_ENV, "").strip()
    if from_env:
        return Path(from_env).expanduser()

    home = os.environ.get("HOME") or os.environ.get("USERPROFILE")
    if not home:
        raise errors.internal("cannot determine the user home directory; set CAUSEWAYBAY_HOME")
    return Path(home) / DEFAULT_DIR


def ensure_dir(directory: Path) -> Path:
    """Create the home directory if missing, restricted to the owner."""
    directory.mkdir(parents=True, exist_ok=True)
    set_private(directory, DIR_MODE)
    return directory


def write_private(path: Path, contents: str) -> None:
    """Write a file that is owner-only from the moment it exists.

    Used for exports that carry key material: ``write_text`` + a later chmod
    would leave a window in which the file sits behind the umask, and an export
    lands in an unprotected working directory rather than the 0700 wallet home.
    """
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, FILE_MODE)
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        handle.write(contents)
    # The mode applies only on creation; tighten a pre-existing file too.
    set_private(path, FILE_MODE)


def set_private(path: Path, mode: int) -> None:
    """Tighten permissions. Silently skipped where chmod is not meaningful."""
    try:
        os.chmod(path, mode)
    except (OSError, NotImplementedError):  # pragma: no cover - platform specific
        pass
