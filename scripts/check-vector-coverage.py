#!/usr/bin/env python3
"""Prove both implementations really read the shared vectors.

Two suites that both pass is not evidence they consume the same data — one could
be silently skipping a file. So corrupt one value in each vector file in turn and
require *both* suites to notice. A suite that stays green is not reading it.

The vector files are restored afterwards, including when a run is interrupted.

    pythoncli/.venv/bin/python scripts/check-vector-coverage.py
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VECTORS = ROOT / "testvectors"
PYTEST = ROOT / "pythoncli" / ".venv" / "bin" / "pytest"

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


def main() -> int:
    if not PYTEST.exists():
        print("missing the Python virtualenv — run 'make build-python' first", file=sys.stderr)
        return 1

    on_disk = {path.name for path in VECTORS.glob("*.json")}
    unmutated = on_disk - MUTATIONS.keys()
    if unmutated:
        print(f"  no mutation defined for: {', '.join(sorted(unmutated))}", file=sys.stderr)
        return 1

    backup = Path(tempfile.mkdtemp())
    for path in VECTORS.glob("*.json"):
        shutil.copy(path, backup / path.name)

    failures = 0
    try:
        if not (run_rust() and run_python()):
            print("  baseline is not green; fix the suites before mutating", file=sys.stderr)
            return 1

        for name, mutate in MUTATIONS.items():
            path = VECTORS / name
            data = json.loads(path.read_text(encoding="utf-8"))
            mutate(data)
            path.write_text(
                json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
            )

            rust_caught = not run_rust()
            python_caught = not run_python()
            shutil.copy(backup / name, path)  # restore before the next one

            if rust_caught and python_caught:
                continue
            failures += 1
            missed = []
            if not rust_caught:
                missed.append("rust")
            if not python_caught:
                missed.append("python")
            print(f"  {name}: not checked by {' and '.join(missed)}", file=sys.stderr)
    finally:
        for path in backup.glob("*.json"):
            shutil.copy(path, VECTORS / path.name)
        shutil.rmtree(backup)

    if failures:
        print(f"  {failures} vector file(s) are not read by both implementations", file=sys.stderr)
        return 1
    print(f"  {len(MUTATIONS)} vector files are read by both implementations "
          f"(each corruption was caught twice)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
