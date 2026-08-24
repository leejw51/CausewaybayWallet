"""The C ABI, and nothing else.

Everything unsafe about talking to the shared library is confined to this
module: finding it, checking it speaks the ABI this binding was written
against, and turning the ``char *`` it hands back into a Python ``str`` that is
freed exactly once. Everything above this file sees only text.

The library is the Rust core — the same one the Rust CLI, the Lua binding and
the C front end use. There is no second implementation of the wallet here; the
cryptography, the store and the RPC all live over there.
"""

from __future__ import annotations

import ctypes
import os
import sys
from pathlib import Path

from . import errors

# The ABI this binding was written against. A library reporting anything else
# is refused rather than guessed at: the envelope shape is the whole contract.
#
# 2 is the multi-chain contract: requests carry a `chain`, account records carry
# the chain they belong to, and `cwb_chains` exists to be asked what those
# chains are.
ABI_VERSION = 2


def library_name() -> str:
    """The platform's name for a Rust cdylib."""
    if sys.platform == "win32":
        return "causewaybay_ffi.dll"
    if sys.platform == "darwin":
        return "libcausewaybay_ffi.dylib"
    return "libcausewaybay_ffi.so"


def search_paths(root: Path, override: str | None = None) -> list[str]:
    """Where to look, in order, for the shared library.

    ``root`` is the directory holding this file, so everything is relative to
    the checkout rather than to whatever directory the user is standing in.
    ``override`` is ``$CAUSEWAYBAY_LIB`` — passed in rather than read here, so
    this is a pure list and a test can ask what the order *is* without the
    environment it happens to run in changing the answer.
    """
    name = library_name()
    bundle = root.parent  # causewaybay/ -> alongside the entry point
    repo = root.parent.parent  # pythoncli/causewaybay/ -> the checkout
    paths: list[str] = []
    # An exact path beats every guess: it is how a packaged layout, an unusual
    # install and a test run all say "this one".
    if override:
        paths.append(override)
    # A packaged copy carries its library inside the wheel, beside this file,
    # so a bundle that was moved anywhere still finds its own and not a stale
    # build. Then the directory above it, which is where a hand-assembled
    # bundle would put it.
    paths.append(str(root / name))
    paths.append(str(bundle / name))
    # Then a checkout, freshest first. `make build` produces the debug library,
    # so that is the one a working tree has just rebuilt; release and ./dist
    # come only from `make package`. Getting this order backwards means a
    # library from last week's release answering for the code you just changed.
    paths.append(str(repo / "rustcli" / "target" / "debug" / name))
    paths.append(str(repo / "rustcli" / "target" / "release" / name))
    paths.append(str(repo / "dist" / name))
    # Last resort: whatever the system linker can find on its own.
    paths.append(name)
    return paths


class Library:
    """One loaded copy of the shared library.

    Holds the ``CDLL`` and the signatures. Kept as an object rather than module
    state so a test can load a second one from an explicit path without
    disturbing the first.
    """

    def __init__(self, handle: ctypes.CDLL, path: str) -> None:
        self.handle = handle
        self.path = path

    def _text(self, pointer) -> str:
        """Copy a returned string, then free the original.

        The library allocated it, so the library frees it — passing a Rust
        allocation to Python's allocator is how a process ends up with a heap
        it cannot describe.
        """
        if not pointer:
            return ""
        try:
            value = ctypes.cast(pointer, ctypes.c_char_p).value or b""
            return value.decode("utf-8", errors="replace")
        finally:
            self.handle.cwb_string_free(pointer)

    def abi_version(self) -> int:
        return int(self.handle.cwb_abi_version())

    def version(self) -> str:
        """The wallet version the loaded library reports."""
        return self._text(self.handle.cwb_version())

    def describe(self) -> str:
        """The library's self-description, as raw JSON text."""
        return self._text(self.handle.cwb_describe())

    def chains(self) -> str:
        """The chains the library supports, as raw JSON text.

        Added in ABI 2. Calling it on an ABI 1 library would fail at the first
        call rather than at load, which is what the version check is for.
        """
        return self._text(self.handle.cwb_chains())

    def commands(self) -> str:
        """The command tree the library accepts, as raw JSON text."""
        return self._text(self.handle.cwb_commands())

    def execute(self, request_json: str) -> str:
        """Run one request. Text in, envelope text out."""
        return self._text(self.handle.cwb_execute(request_json.encode("utf-8")))


def _bind(handle: ctypes.CDLL) -> None:
    """Declare the signatures.

    ctypes defaults every return to ``int``, which on a 64-bit build truncates
    a pointer to its low half — a crash that looks like memory corruption
    because it is. Every one of these is declared before it is called.
    """
    handle.cwb_abi_version.restype = ctypes.c_int
    handle.cwb_abi_version.argtypes = []
    for name in ("cwb_version", "cwb_describe", "cwb_chains", "cwb_commands"):
        function = getattr(handle, name)
        function.restype = ctypes.POINTER(ctypes.c_char)
        function.argtypes = []
    handle.cwb_execute.restype = ctypes.POINTER(ctypes.c_char)
    handle.cwb_execute.argtypes = [ctypes.c_char_p]
    handle.cwb_string_free.restype = None
    handle.cwb_string_free.argtypes = [ctypes.POINTER(ctypes.c_char)]


_loaded: Library | None = None


def load(explicit: str | None = None) -> Library:
    """Load the shared library, once per process.

    Raises ``io_error`` listing everywhere it looked — a missing library is the
    single most likely thing to go wrong here, so the message has to be enough
    to fix it without reading this file.
    """
    global _loaded
    if _loaded is not None and explicit is None:
        return _loaded

    override = explicit or os.environ.get("CAUSEWAYBAY_LIB") or None
    # A path passed in is exclusive, not merely first: a caller that named a
    # library and got a different one would have no way to tell.
    candidates = [explicit] if explicit else search_paths(Path(__file__).resolve().parent, override)
    tried: list[str] = []
    for candidate in candidates:
        tried.append(candidate)
        try:
            handle = ctypes.CDLL(candidate)
        except OSError:
            continue

        _bind(handle)
        reported = handle.cwb_abi_version()
        if reported != ABI_VERSION:
            raise errors.WalletError(
                "internal",
                f"{candidate} speaks ABI {reported}, this binding expects "
                f"{ABI_VERSION} — rebuild both",
            )
        library = Library(handle, candidate)
        if explicit is None:
            _loaded = library
        return library

    raise errors.WalletError(
        "io_error",
        "cannot find the wallet library; looked in:\n  "
        + "\n  ".join(tried)
        + "\nBuild it with `make -C rustcli build`, or set CAUSEWAYBAY_LIB.",
    )
