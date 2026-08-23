#!/usr/bin/env python3
"""Prove every suite really reads the shared vectors.

Suites that all pass is not evidence they consume the same data — one could be
silently skipping a file. So corrupt one value in each vector file in turn and
require every suite to notice. A suite that stays green is not reading it.

Three suites are checked: Rust, Python, and Lua. Lua is exempt from a named few
(see LUA_EXEMPT) where the mutated field is one no Lua test can reach; the
exemptions are listed rather than inferred, so adding one is a decision someone
made on purpose.

The vector files are restored afterwards, including when a run is interrupted.

    pythoncli/.venv/bin/python scripts/check-vector-coverage.py
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VECTORS = ROOT / "testvectors"
PYTEST = ROOT / "pythoncli" / ".venv" / "bin" / "pytest"
LUAJIT = os.environ.get("LUAJIT", "luajit")

# Files whose mutation the Lua suite cannot be expected to catch, and why.
#
# Both are values the wallet does not expose through any command, so a front end
# that only speaks the CLI surface has no way to assert on them. The Rust suite
# calls the library directly and does check them; that is what it is for.
LUA_EXEMPT = {
    "bip39.json": "the BIP-39 seed is internal; no command prints it",
    "transactions.json": "signing needs a node, which the Rust suite mocks",
}

VALID_PHRASE = (
    "abandon abandon abandon abandon abandon abandon abandon abandon "
    "abandon abandon abandon about"
)
VALID_KEY = "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727"


def flip(text: str) -> str:
    """Change one character, leaving the value well-formed but wrong."""
    return text[:-1] + ("1" if text[-1] != "1" else "2")


# One targeted corruption per file. Each is a value both suites assert on, so a
# "MISSED" means that implementation never looks at this file.
MUTATIONS = {
    "bip39.json": lambda d: d["vectors"][0].update(
        seed_trezor=flip(d["vectors"][0]["seed_trezor"])
    ),
    # Turn a phrase that must be rejected into a perfectly valid one.
    "bip39-invalid.json": lambda d: d["vectors"][3].update(mnemonic=VALID_PHRASE),
    "derivation.json": lambda d: d["mnemonics"][0]["accounts"][0].update(
        private_key=flip(d["mnemonics"][0]["accounts"][0]["private_key"])
    ),
    "keys.json": lambda d: d["keys"][0].update(address=flip(d["keys"][0]["address"])),
    # Turn a key that must be rejected into a valid one.
    "keys-invalid.json": lambda d: d["vectors"][0].update(private_key=VALID_KEY),
    "eip55.json": lambda d: d["vectors"][0].update(
        checksummed=d["vectors"][0]["checksummed"].lower()
    ),
    "keccak.json": lambda d: d["hashes"][0].update(
        keccak256=flip(d["hashes"][0]["keccak256"])
    ),
    "eip191.json": lambda d: d["vectors"][1].update(
        signature=flip(d["vectors"][1]["signature"])
    ),
    "transactions.json": lambda d: d["vectors"][0].update(
        raw=flip(d["vectors"][0]["raw"])
    ),
    "units.json": lambda d: d["valid"][2].update(value=flip(d["valid"][2]["value"])),
}


def run_rust() -> bool:
    """True when the Rust vector suite passes."""
    result = subprocess.run(
        ["cargo", "test", "--test", "vectors", "-q"],
        cwd=ROOT / "rustcli",
        capture_output=True,
    )
    return result.returncode == 0


def run_python() -> bool:
    """True when the Python vector suite passes."""
    result = subprocess.run(
        [str(PYTEST), "tests/test_vectors.py", "-q", "--no-header"],
        cwd=ROOT / "pythoncli",
        capture_output=True,
    )
    return result.returncode == 0


def lua_available() -> bool:
    """True when LuaJIT is installed and the shared library is built."""
    if shutil.which(LUAJIT) is None:
        return False
    result = subprocess.run(
        [LUAJIT, "tests/init.lua", "json"], cwd=ROOT / "luacli", capture_output=True
    )
    return result.returncode == 0


def run_lua() -> bool:
    """True when the Lua vector suite passes."""
    result = subprocess.run(
        [LUAJIT, "tests/init.lua", "vectors"], cwd=ROOT / "luacli", capture_output=True
    )
    return result.returncode == 0


def main() -> int:
    if not PYTEST.exists():
        print("missing the Python virtualenv — run 'make build-python' first", file=sys.stderr)
        return 1

    on_disk = {path.name for path in VECTORS.glob("*.json")}
    unmutated = on_disk - MUTATIONS.keys()
    if unmutated:
        print(f"  no mutation defined for: {', '.join(sorted(unmutated))}", file=sys.stderr)
        return 1

    unknown_exempt = LUA_EXEMPT.keys() - on_disk
    if unknown_exempt:
        print(f"  LUA_EXEMPT names a file that is gone: {', '.join(sorted(unknown_exempt))}",
              file=sys.stderr)
        return 1

    with_lua = lua_available()
    if not with_lua:
        print("  note: LuaJIT or the shared library is missing — Lua is not checked",
              file=sys.stderr)

    backup = Path(tempfile.mkdtemp())
    for path in VECTORS.glob("*.json"):
        shutil.copy(path, backup / path.name)

    failures = 0
    try:
        if not (run_rust() and run_python() and (not with_lua or run_lua())):
            print("  baseline is not green; fix the suites before mutating", file=sys.stderr)
            return 1

        for name, mutate in MUTATIONS.items():
            path = VECTORS / name
            data = json.loads(path.read_text(encoding="utf-8"))
            mutate(data)
            path.write_text(
                json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
            )

            missed = []
            if run_rust():
                missed.append("rust")
            if run_python():
                missed.append("python")
            # An exempt file is not asked about at all, so a suite cannot be
            # blamed for a value it has no way to see.
            if with_lua and name not in LUA_EXEMPT and run_lua():
                missed.append("lua")
            shutil.copy(backup / name, path)  # restore before the next one

            if not missed:
                continue
            failures += 1
            print(f"  {name}: not checked by {' and '.join(missed)}", file=sys.stderr)
    finally:
        for path in backup.glob("*.json"):
            shutil.copy(path, VECTORS / path.name)
        shutil.rmtree(backup)

    if failures:
        print(f"  {failures} vector file(s) are not read by every suite", file=sys.stderr)
        return 1

    suites = "rust and python" if not with_lua else "rust, python and lua"
    exempt = len(LUA_EXEMPT) if with_lua else 0
    note = f", {exempt} exempt for lua" if exempt else ""
    print(f"  {len(MUTATIONS)} vector files are read by {suites} "
          f"(every corruption was caught{note})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
