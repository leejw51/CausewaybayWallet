"""Copying text to the system clipboard.

Done by piping to whatever the platform provides rather than by adding a
clipboard dependency: on Linux those pull in X11/Wayland libraries, which is a
heavy thing to require of a wallet. The trade-off is that a headless machine
with no helper installed gets a clear message instead of silent failure.
"""

from __future__ import annotations

import platform
import subprocess

from . import errors


def candidates() -> list[tuple[str, list[str]]]:
    """The helpers to try, in order: ``(program, arguments)``."""
    system = platform.system()
    if system == "Darwin":
        return [("pbcopy", [])]
    if system == "Windows":
        return [("clip", [])]
    return [
        ("wl-copy", []),
        ("xclip", ["-selection", "clipboard"]),
        ("xsel", ["--clipboard", "--input"]),
    ]


def _write_to(program: str, args: list[str], text: str) -> None:
    """Spawn one helper and feed it the text on stdin."""
    result = subprocess.run(
        [program, *args],
        input=text.encode("utf-8"),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    if result.returncode != 0:
        raise OSError(f"exited with {result.returncode}")


def copy(text: str) -> str:
    """Copy ``text`` to the clipboard, returning the helper that accepted it."""
    tried = []
    for program, args in candidates():
        try:
            _write_to(program, args, text)
            return program
        except (OSError, FileNotFoundError) as exc:
            tried.append(f"{program} ({exc})")
    raise errors.internal("no clipboard helper worked — tried " + ", ".join(tried))


def is_available() -> bool:
    """True when some clipboard helper is on PATH."""
    import shutil

    return any(shutil.which(program) for program, _ in candidates())
