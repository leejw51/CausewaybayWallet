#!/usr/bin/env python3
"""Bake a bitmap font, so the text is pixels rather than a shrunken vector font.

    python3 lovegui/tools/generate-font.py

Writes ``assets/font.png`` and prints the glyph string that goes with it. Both
are committed; this only needs running to change the font or its size.

## Why bake one at all

LÖVE's built-in font is Vera Sans, a vector face hinted for ordinary sizes. At
the 10px this UI draws it renders one of two ways, and both are wrong:

* antialiased, which puts grey pixels on every edge — and a nearest-neighbour
  3x upscale turns each grey pixel into a 3x3 grey block, which is the smeared
  look people mean when they call pixel art "low resolution";
* with ``"mono"`` hinting, which has no greys but *drops stems*, because at
  10px there is not room for the letterform. "WALLET" comes out with holes.

A face designed to rasterize at small sizes has neither problem. Menlo with
antialiasing off gives complete, hard-edged glyphs at 11pt, which is what this
bakes into an image font LÖVE can load directly.

## The format

LÖVE's ``newImageFont`` wants one row of glyphs separated by columns of a
marker colour, which it reads from the image's top-left pixel. Each glyph is
rendered on its own so its true width is kept — a proportional bitmap font,
rather than every letter padded to the width of an M.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

ASSETS = Path(__file__).resolve().parent.parent / "assets"
TARGET = ASSETS / "font.png"

# Menlo at 8pt with antialiasing off. Checked by eye against Courier and SF
# Mono; Menlo keeps the most shape at this size, and its zero is slashed, which
# matters for a screen full of addresses.
#
# 11pt bakes to a 15px strip with a 7px cap. 8pt was tried first because it fit
# the rows the layout already had, and it is simply not legible — at a 6px cap
# Menlo drops enough of each letterform that "SEND" reads as "SEID". So the
# layout gives the font room instead: fewer rows on screen, each taller. A
# wallet showing addresses needs more pixels per character than a platformer.
FONT = "/System/Library/Fonts/Menlo.ttc"
POINTSIZE = 11

# The separator, and a colour no glyph can contain.
MARKER = "#FF00FF"

# Printable ASCII, plus the three punctuation marks the UI actually uses. A
# glyph the font lacks falls back to LÖVE's built-in one at run time (see
# `theme.load`), so this does not have to be exhaustive — just complete for
# everything drawn every frame.
GLYPHS = "".join(chr(c) for c in range(32, 127)) + "·…—"


def require(tool: str) -> str:
    found = shutil.which(tool)
    if not found:
        raise SystemExit(
            f"{tool} is needed to bake the font.\n"
            "  macOS:  brew install imagemagick\n"
            "  Debian: sudo apt install imagemagick\n"
            "assets/font.png is committed, so this is only needed to change it."
        )
    return found


def render(magick: str, glyph: str) -> Path:
    """One glyph on a transparent background, at the font's own metrics.

    Deliberately *not* trimmed. Trimming to the ink and re-padding was tried
    first, and it bottom-aligns each glyph's ink rather than its baseline — so a
    `y` sits with its descender on the floor and its body a pixel higher than
    the `a` beside it. Menlo is monospace: every glyph already renders to the
    same 8x15 box with the baseline where the font puts it, including the space.
    Leaving that alone is both simpler and correct.
    """
    out = Path(f"/tmp/cwb-glyph-{ord(glyph)}.png")

    # The glyph goes through a file rather than the command line: `label:`
    # treats a leading `@` as "read this file" and a `\\` as an escape, so
    # rendering those two characters inline silently produces the wrong thing —
    # and a dropped glyph desyncs the image from the glyph string, which shifts
    # every letter after it.
    source = Path(f"/tmp/cwb-glyph-{ord(glyph)}.txt")
    source.write_text(glyph, encoding="utf-8")

    subprocess.run(
        [
            magick,
            "-background", "transparent",
            "-fill", "white",
            "-font", FONT,
            "-pointsize", str(POINTSIZE),
            "+antialias",
            f"label:@{source}",
            str(out),
        ],
        check=True,
        capture_output=True,
    )
    return out


def main() -> int:
    magick = require("magick")
    ASSETS.mkdir(parents=True, exist_ok=True)

    # One reference render, only to size the separator columns to match.
    probe = subprocess.run(
        [magick, "-background", "transparent", "-fill", "white", "-font", FONT,
         "-pointsize", str(POINTSIZE), "+antialias", "label:Ayg", "-format", "%h",
         "info:"],
        check=True, capture_output=True, text=True,
    )
    height = int(probe.stdout.strip())

    pieces: list[str] = []
    for glyph in GLYPHS:
        try:
            pieces.append(str(render(magick, glyph)))
        except subprocess.CalledProcessError as exc:
            # Not tolerated: a missing glyph shifts every one after it, so the
            # font would render fluent nonsense rather than obviously break.
            raise SystemExit(
                f"could not render {glyph!r} (U+{ord(glyph):04X}): "
                f"{exc.stderr.decode(errors='replace')}"
            ) from exc

    # A marker column before each glyph and one after the last, which is the
    # arrangement LÖVE's parser expects.
    separator = f"/tmp/cwb-sep.png"
    subprocess.run(
        [magick, "-size", f"1x{height}", f"xc:{MARKER}", separator], check=True
    )

    strip: list[str] = []
    for piece in pieces:
        strip += [separator, piece]
    strip.append(separator)

    subprocess.run([magick] + strip + ["+append", str(TARGET)], check=True)

    width, _ = subprocess.run(
        [magick, str(TARGET), "-format", "%wx%h", "info:"],
        check=True, capture_output=True, text=True,
    ).stdout.split("x")

    print(f"  wrote {TARGET.relative_to(ASSETS.parent)}  {width}x{height}px, "
          f"{len(pieces)} glyphs")
    print(f"  glyph string is in ui/theme.lua; it must match this order exactly")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
