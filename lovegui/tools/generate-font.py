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
# 12pt bakes to a 16px strip with a 7px cap. Two sizes were tried and rejected
# before it, in both directions:
#
# 8pt fit the rows the layout already had and is simply not legible — at a 6px
# cap Menlo drops enough of each letterform that "SEND" reads as "SEID".
#
# 11pt looked right and shipped, and was wrong in one specific place: lowercase
# `m` has three stems and about five pixels to put them in, so with
# antialiasing off it fills in and bakes as a solid block. Every "from" in the
# interface read as "fro" followed by a smudge. One pixel more of width is all
# it needed, and 13pt is a whole pixel wider and three taller, which would move
# every row in the layout.
#
# `check` below is the part that matters more than the number: a glyph that
# bakes into a solid rectangle is now a hard failure, so the next face or size
# that cannot draw a letter says so instead of shipping.
FONT = "/System/Library/Fonts/Menlo.ttc"
POINTSIZE = 12

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


def unreadable(path: Path) -> bool:
    """Is this glyph a solid rectangle rather than a letterform?

    The failure mode that shipped. A face asked for more stems than it has
    pixels does not fail, and does not render nothing — it renders a filled
    blob, which looks deliberate at a glance and is unreadable in a word.
    Nothing downstream can tell that from a glyph that is *supposed* to be
    solid, so it has to be caught here, at the one moment the intended
    character is known.

    Measured as ink over its own bounding box. A letterform leaves holes; a
    blob does not. `|`, `.` and `_` are solid by nature and are exempt.
    """
    dump = subprocess.run(["magick", str(path), "txt:-"],
                          check=True, capture_output=True, text=True).stdout
    on = []
    for line in dump.splitlines()[1:]:
        position, rest = line.split(":", 1)
        x, y = (int(n) for n in position.split(","))
        if "none" not in rest.lower() and "00000000" not in rest:
            on.append((x, y))
    if not on:
        return False
    xs = [x for x, _ in on]
    ys = [y for _, y in on]
    area = (max(xs) - min(xs) + 1) * (max(ys) - min(ys) + 1)
    return area >= 9 and len(on) / area > 0.95


#: Characters that really are solid, and must not trip the check above.
SOLID = "|.,_-'`:;!"


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

        if glyph not in SOLID and unreadable(Path(pieces[-1])):
            raise SystemExit(
                f"{glyph!r} (U+{ord(glyph):04X}) baked as a solid block at "
                f"{POINTSIZE}pt: the face has more stems than pixels here. "
                "Raise POINTSIZE until it resolves — every glyph has to be a "
                "letter, and a blob is worse than a missing one because it "
                "looks deliberate."
            )

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
