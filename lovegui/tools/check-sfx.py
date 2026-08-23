#!/usr/bin/env python3
"""Check the committed sound effects are present and worth playing.

    python3 lovegui/tools/check-sfx.py

Run by `make sfx-check`, which `make check` runs, so a sound that went missing
or came out silent fails a build rather than being noticed months later by
somebody wondering why the coins are quiet.

## Why not just diff against a fresh render

That is the stronger test and the wrong one. The waveforms come out of libm:
`sin` and `pow` are allowed to differ in the last bit between platforms, so a
byte comparison would eventually fail a build on a difference no ear could
detect, and the fix would be to weaken or delete the check. Asserting the
properties that actually matter survives being run anywhere.

What it catches, all of which has a plausible way of happening:

* a file that was never generated, or was deleted;
* one truncated by a bad copy — every effect has a length it should be near;
* one that is silent, which is what a botched envelope produces and which is
  invisible in a directory listing;
* one that clips, which is a mixing mistake and sounds like a crackle;
* the wrong sample rate or channel count, which plays at the wrong pitch.

The expected values are deliberately loose ranges rather than exact numbers:
this is a smoke test, and one that has to be re-tuned every time a sound is
adjusted would be deleted within a month.
"""

from __future__ import annotations

import sys
import wave
from pathlib import Path

SFX = Path(__file__).resolve().parent.parent / "assets" / "sfx"

RATE = 22050

# Every effect `ui/sound.lua` asks for, with roughly how long it should be.
# Kept in step with the WANTED table there — a name in one and not the other is
# the failure this catches first.
EXPECTED = {
    "hover": 0.03, "type": 0.02, "blip": 0.05,
    "press": 0.07, "back": 0.08, "tab": 0.10,
    "coin": 0.41, "created": 0.41, "sent": 0.80,
    "error": 0.22, "deny": 0.35,
    "unlock": 0.61, "ready": 0.37,
    "launch": 1.30, "power": 0.60,
}

# How far a length may drift from the figure above before it is a problem. Wide
# on purpose: this is here to catch a truncated or empty file, not to freeze
# the sound design.
TOLERANCE = 0.35


def check(name: str, wanted: float) -> list[str]:
    path = SFX / f"{name}.wav"
    if not path.exists():
        return [f"{name}: missing — run `make sfx`"]

    problems: list[str] = []
    with wave.open(str(path)) as handle:
        if handle.getnchannels() != 1:
            problems.append(f"{name}: {handle.getnchannels()} channels, want mono")
        if handle.getframerate() != RATE:
            problems.append(f"{name}: {handle.getframerate()} Hz, want {RATE}")
        if handle.getsampwidth() != 1:
            problems.append(f"{name}: {handle.getsampwidth() * 8}-bit, want 8")
        frames = handle.readframes(handle.getnframes())

    seconds = len(frames) / RATE
    if abs(seconds - wanted) > wanted * TOLERANCE:
        problems.append(f"{name}: {seconds:.2f}s, expected about {wanted:.2f}s")

    samples = [(byte - 128) / 127 for byte in frames]
    if not samples:
        return problems + [f"{name}: no audio in it at all"]

    peak = max(abs(v) for v in samples)
    if peak < 0.05:
        problems.append(f"{name}: silent (peak {peak:.3f})")
    if peak > 0.98:
        problems.append(f"{name}: clipping (peak {peak:.3f})")

    # A file of one constant value is a valid WAV and no sound whatsoever.
    if len(set(frames)) < 3:
        problems.append(f"{name}: constant, not a waveform")

    return problems


def main() -> int:
    if not SFX.is_dir():
        print(f"  no {SFX} — run `make sfx`", file=sys.stderr)
        return 1

    problems: list[str] = []
    for name, wanted in sorted(EXPECTED.items()):
        problems += check(name, wanted)

    extra = {p.stem for p in SFX.glob("*.wav")} - set(EXPECTED)
    for name in sorted(extra):
        problems.append(f"{name}: on disk but not in EXPECTED or ui/sound.lua")

    if problems:
        for line in problems:
            print(f"  {line}", file=sys.stderr)
        return 1

    print(f"  {len(EXPECTED)} sounds present and well-formed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
