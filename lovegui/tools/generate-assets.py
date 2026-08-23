#!/usr/bin/env python3
"""Draw the game's sprites with Grok, once, so the game never needs an API key.

    XAI_API_KEY=… python3 lovegui/tools/generate-assets.py [name …]

Every sprite lands in ``lovegui/assets/`` and is committed. Running the game
must not depend on a key, a network, or a paid API being up — regenerating art
is an author's job, not a player's. Naming sprites on the command line redraws
only those; with no names it draws the ones that are missing.

## Why the prompts look the way they do

The model returns an opaque JPEG, so every prompt asks for a pure black
background — which this script then keys out, writing a PNG with a real alpha
channel. Doing it here rather than in the game means the sprites are finished
art by the time they are committed: the loader just loads them.

The rest of each prompt fights the model's instinct to render a *painting* of
pixel art rather than pixel art: "hard pixel edges", "limited palette", "no
anti-aliasing", "no text". Text is the one it cannot resist otherwise, and a
misspelled word baked into a sprite is not something a wallet should ship.

## Turning black into alpha

A hard `-transparent black` would leave a halo, because the glow fades to black
over many pixels rather than stopping. So alpha comes from luminance instead,
levelled so that only the near-black is cut:

    magick in.jpg -filter point -resize 256x256 \
      \( +clone -colorspace Gray -level 4%,14% \) \
      -alpha off -compose CopyOpacity -composite out.png

Below 4% brightness is fully transparent, above 14% fully opaque, and the
narrow band between is the feather that keeps the glow's edge soft. `-filter
point` is nearest-neighbour, which is what keeps a 1024px render of pixel art
looking like pixel art after it is scaled down.
"""

from __future__ import annotations

import base64
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

ASSETS = Path(__file__).resolve().parent.parent / "assets"
ENDPOINT = "https://api.x.ai/v1/images/generations"

# grok-imagine-image rather than -2.0 or -quality: on this art it is
# indistinguishable and roughly ten times faster, and there are eight of them.
MODEL = os.environ.get("CWB_IMAGE_MODEL", "grok-imagine-image")

# Shared across every prompt, so the eight sprites look like one set.
STYLE = (
    "8-bit NES-era pixel art, hard aliased pixel edges, chunky visible pixels, "
    "limited retro palette, strong neon rim glow, centered composition, "
    "pure solid black background, no text, no letters, no words, no watermark, "
    "no border, no frame"
)

# Backdrops, which are whole scenes rather than objects on black. They keep
# their background — there is nothing to key out — and are cropped to the
# 16:9 the canvas is rather than the square the model returns.
SCENES: dict[str, str] = {
    "krumlov": (
        "a side-scrolling arcade game backdrop of Cesky Krumlov in Bohemia: the "
        "medieval castle high on a rocky crag with its tall round painted tower, "
        "steep red clay rooftops of the old town crowded below it, the Vltava "
        "river bending around the town in the foreground, forested hills behind, "
        "deep dusk sky with a few stars. Painted in the style of a Konami MSX "
        "game backdrop — Knightmare, Golden Axe — heavy black outlines, chunky "
        "hard-edged pixels, a limited moody palette of blues purples and warm "
        "roof reds, dramatic silhouettes, no characters, no text"
    ),
}

SPRITES: dict[str, str] = {
    "logo": (
        "a heavy armoured bank vault door with a glowing cyan dial, "
        "gold and steel, radiating light"
    ),
    "wallet": "a fat leather coin pouch tied with cord, gold coins spilling out of the top",
    "coin": "a single round gold coin with a star stamped on it, bright yellow, glinting",
    "rocket": "a small chunky rocket ship flying upward with a blue and orange flame trail",
    "globe": "a wireframe planet globe with glowing cyan latitude lines, floating in space",
    "key": "an ornate golden skeleton key with a glowing green gemstone in its bow",
    "skull": "a small white cartoon skull with glowing red eye sockets",
    "spark": (
        "a single bright four-pointed sparkle star, white core fading to cyan, "
        "on black, tiny and simple"
    ),
}


# What the sprites are scaled to. 1024 is what the model returns and far more
# than a 1280x720 window needs; 256 keeps them crisp at every size the game
# draws them and the whole set under a megabyte.
SIZE = 256

# What a backdrop is scaled to: exactly the canvas in ui/theme.lua. Drawing it
# 1:1 is what keeps its pixels the same size as everything drawn over it — a
# backdrop at any other resolution would be visibly finer or coarser than the
# UI on top and the whole illusion would come apart.
SCENE_WIDTH, SCENE_HEIGHT = 480, 270


def converter() -> list[str]:
    """The ImageMagick entry point on this machine, v7 or v6."""
    for candidate in ("magick", "convert"):
        found = shutil.which(candidate)
        if found:
            return [found]
    raise SystemExit(
        "ImageMagick is needed to key the background out and write a PNG.\n"
        "  macOS:  brew install imagemagick\n"
        "  Debian: sudo apt install imagemagick\n"
        "The committed PNGs in lovegui/assets/ are what the game loads, so this\n"
        "is only needed to redraw them."
    )


def to_png(jpeg: bytes, target: Path, magick: list[str]) -> None:
    """Scale, key the black background to alpha, and write a PNG."""
    with tempfile.NamedTemporaryFile(suffix=".jpg", delete=False) as handle:
        handle.write(jpeg)
        source = Path(handle.name)
    try:
        subprocess.run(
            magick
            + [
                str(source),
                "-filter", "point",
                "-resize", f"{SIZE}x{SIZE}",
                "(", "+clone", "-colorspace", "Gray", "-level", "4%,14%", ")",
                "-alpha", "off",
                "-compose", "CopyOpacity",
                "-composite",
                str(target),
            ],
            check=True,
            capture_output=True,
        )
    except subprocess.CalledProcessError as exc:
        raise SystemExit(
            f"could not convert {target.name}: {exc.stderr.decode(errors='replace')}"
        ) from exc
    finally:
        source.unlink(missing_ok=True)


def to_scene(jpeg: bytes, target: Path, magick: list[str]) -> None:
    """Crop to 16:9 and scale to the canvas. No keying — it is the background."""
    with tempfile.NamedTemporaryFile(suffix=".jpg", delete=False) as handle:
        handle.write(jpeg)
        source = Path(handle.name)
    try:
        subprocess.run(
            magick
            + [
                str(source),
                # The model composes a square and often letterboxes the scene
                # inside it with black bars. Trimming a uniform border first
                # removes those; without it they survive the crop and the game
                # gets a backdrop with black stripes across the top and bottom.
                "-fuzz", "6%", "-trim", "+repage",
                # Then centre-cropped to 16:9 rather than squashed, because the
                # horizon is in the middle and stretching it would show.
                "-gravity", "center",
                "-crop", "%[fx:w]x%[fx:min(h,w*9/16)]+0+0", "+repage",
                "-filter", "point",
                "-resize", f"{SCENE_WIDTH}x{SCENE_HEIGHT}!",
                str(target),
            ],
            check=True,
            capture_output=True,
        )
    except subprocess.CalledProcessError as exc:
        raise SystemExit(
            f"could not convert {target.name}: {exc.stderr.decode(errors='replace')}"
        ) from exc
    finally:
        source.unlink(missing_ok=True)


def draw(name: str, subject: str, key: str) -> bytes:
    """Ask for one sprite. Retries, because image endpoints time out."""
    body = json.dumps(
        {
            "model": MODEL,
            "prompt": f"{subject}. {STYLE}",
            "n": 1,
            "response_format": "b64_json",
        }
    ).encode()

    last: Exception | None = None
    for attempt in range(1, 4):
        request = urllib.request.Request(
            ENDPOINT,
            data=body,
            headers={
                "Authorization": f"Bearer {key}",
                "Content-Type": "application/json",
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=180) as response:
                payload = json.load(response)
            return base64.b64decode(payload["data"][0]["b64_json"])
        except (urllib.error.URLError, KeyError, TimeoutError, OSError) as exc:
            last = exc
            print(f"    attempt {attempt} failed: {exc}", file=sys.stderr)
            time.sleep(2 * attempt)

    raise SystemExit(f"could not draw {name}: {last}")


def main() -> int:
    key = os.environ.get("XAI_API_KEY") or os.environ.get("GROK_API_KEY")
    if not key:
        print(
            "XAI_API_KEY is not set.\n"
            "The committed sprites in lovegui/assets/ are what the game loads, so\n"
            "this is only needed to redraw them.",
            file=sys.stderr,
        )
        return 1

    magick = converter()
    ASSETS.mkdir(parents=True, exist_ok=True)
    everything = {**SPRITES, **SCENES}
    wanted = sys.argv[1:] or list(everything)

    unknown = [name for name in wanted if name not in everything]
    if unknown:
        print(f"no such sprite: {', '.join(unknown)}", file=sys.stderr)
        print(f"known: {', '.join(everything)}", file=sys.stderr)
        return 1

    drawn = 0
    for name in wanted:
        target = ASSETS / f"{name}.png"
        # With no names given this only fills gaps, so a rerun after one
        # failure does not redraw and re-bill the seven that worked.
        if not sys.argv[1:] and target.exists():
            print(f"  {name:8} already drawn")
            continue

        print(f"  {name:8} drawing…", flush=True)
        started = time.time()
        image = draw(name, everything[name], key)
        if name in SCENES:
            to_scene(image, target, magick)
        else:
            to_png(image, target, magick)
        size = target.stat().st_size
        print(f"  {name:8} {size // 1024:>4} KB in {time.time() - started:.0f}s")
        drawn += 1

    print(f"\n  {drawn} drawn, {len(wanted) - drawn} already present, in {ASSETS}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
