# CAUSEWAYBAY BANK — LÖVE

> ⚠️ **Educational software.** Keys are stored unencrypted on disk. Do not use
> with funds you are not prepared to lose. For real value, use a hardware wallet.

The wallet as an 8-bit game: an MSX boots, a Bohemian town appears at dusk, and
the bank opens for business. A window over the same store the CLI drives, built
on the Lua binding in [`../luacli`](../luacli) — LÖVE embeds LuaJIT, which is
the one thing that binding needs, so it loads here unchanged.

There is no cryptography in this directory, no store, and no argument parsing.
`causewaybay.open` and method calls, exactly as the binding's README describes.

```sh
make run       # opens the window
make test      # 54 headless tests, no LÖVE required
make shots     # writes a PNG of each screen, for review from a terminal
```

Needs LÖVE 11 (`brew install love`) and the shared library, which `make run`
builds for you.

## Booting

It does not open a window, it *comes on*. The tube warms up, the memory counts,
the hardware reports in, and `Ok` appears with a cursor blinking under it — then
the title card, and any key drops you into the bank.

```
CAUSEWAYBAY BANK (C) 2026
8-BIT WALLET SYSTEM

MAIN RAM      65536 BYTES OK
VRAM          16384 BYTES OK

FFI ABI           1   OK
VERSION       1.0.1   OK
NETWORK  cronos-testnet
CHAIN           338
WALLETS           2
Ok
```

**Everything on that screen is true.** The ABI number is what `cwb_abi_version`
returned, the version is the library's, the network and the wallet count come
from `info`, and if the library did not load it says so in red and halts rather
than handing off to a UI that cannot work. The only invented figures are the two
memory counts — 65536 and 16384 are an MSX1's RAM and VRAM, which is the whole
reference. A boot screen that reports fake state is a lie printed in a font that
makes it look authoritative, and it is the first thing anyone sees.

## The look

The 8-bit style is not a filter over a modern UI — the UI genuinely is that
size. Everything renders to a **480×270 canvas** which is scaled to the window
by a whole number with nearest-neighbour filtering.

That one decision does most of the work. Text is chunky because the font really
is 8 pixels tall. A one-pixel border really is one pixel, four screen pixels
wide at 4×. Nothing has to *try* to look retro, and nothing can accidentally
render a smooth gradient or a hairline, because there is nowhere to put one.

The scale is an integer on purpose: at 2.5× a one-pixel line lands on half a
screen pixel, the filter smears it, and the whole illusion goes. Letterboxing
the remainder is the price.

Sixteen colours, fixed, all in `ui/theme.lua`. None are written inline anywhere
else.

### The font is baked, not shrunk

LÖVE's built-in face is Vera Sans, a vector font hinted for ordinary sizes. At
the ~10px this UI draws it renders one of two ways and both are wrong:
antialiased, which puts grey pixels on every edge that a 3× nearest upscale then
turns into 3×3 grey blocks; or with `"mono"` hinting, which has no greys but
drops stems, so `WALLET` comes out with holes in it.

So `tools/generate-font.py` bakes Menlo at 11pt with antialiasing off into an
image font. Two things it learned the hard way, both recorded in its comments:
trimming each glyph to its ink and re-padding bottom-aligns the *ink* rather than
the *baseline*, so a `y` sits a pixel low — Menlo is monospace and needed no
trimming at all. And a glyph the generator cannot draw must be a hard failure,
because a skipped one shifts every letter after it and the font renders fluent
nonsense instead of visibly breaking.

Anything outside the baked set becomes `?` rather than a silent gap, since the
wallet's own messages can contain any UTF-8.

### The backdrop

Český Krumlov at dusk — the castle on its crag with the round painted tower, red
rooftops, the Vltava bending around the town — drawn by Grok in the style of a
Konami MSX backdrop and scrimmed back so the interface wins every contrast fight
in front of it. It drifts a few pixels, which is what stops a painting behind a
live UI reading as a still image.

## Motion

Three tools in `ui/anim.lua`, and most of the polish is in picking the right one.

| | for | example |
| --- | --- | --- |
| **Easing** | known start, end and duration | a panel sliding in |
| **Smoothing** | chasing a target that keeps moving | a balance counting up |
| **Springs** | motion that should feel like it has mass | a card landing, a button pressing |

Everything that decays uses the closed form of exponential decay:

```lua
x = target + (x - target) * math.exp(-rate * dt)
```

not the `x += (target - x) * 0.1` everyone writes first — which moves a tenth of
the way *per frame* and so converges twice as fast at 120fps as at 60. The form
above gives the identical curve at any frame rate, including a frame that took
200ms because the window was dragged. It is the reason the animation does not go
strange when the wallet blocks for a moment, and the test suite asserts it:
one 0.5s step and five hundred 1ms steps have to land in the same place.

Springs take a damping **ratio**, not a coefficient:

- `1.0` — critically damped: fastest arrival, no overshoot. The default for
  anything a person reads, because overshooting text is just wobble.
- `0.5`–`0.8` — a visible overshoot that settles. What makes a card feel heavy.
- `< 0.4` — springy and cartoonish. Right for a coin, wrong for a balance.

The integrator clamps its step, so a 400ms hitch makes the animation slow rather
than divergent — there is a test for that too, because that is exactly how a
spring explodes in the wild.

## Particles

Hand-rolled in `ui/particles.lua` rather than `love.graphics.newParticleSystem`,
for one reason that matters: the effects here need per-particle behaviour LÖVE's
system does not express. Coins that **arc toward a target** and land. Exhaust
that knows which way is down. Confetti that tumbles on its own axis.

The nice side effect is that a system is a table of numbers with no LÖVE object
in it, so the tests run a burst for two seconds and assert it died — which is
the difference between a particle system and a memory leak with a pretty face.

| effect | where |
| --- | --- |
| `burst` | button presses, errors, selections |
| `coins` | a balance arriving — they arc out, then home in on the number |
| `embers` | the rocket's exhaust, and a warmer idle while a send is in flight |
| `confetti` | a completed transfer |
| `trail` | behind anything moving |
| `Stars` | the parallax starfield behind every screen |

Everything draws additively, so overlapping particles bloom instead of
flattening into an opaque blob.

## The art

Eight sprites in `assets/`, drawn by **Grok** through
`tools/generate-assets.py` and **committed**. Running the game must not depend on
an API key, a network, or a paid endpoint being up — regenerating art is an
author's job, not a player's. A missing sprite becomes a deliberately ugly
placeholder rather than a crash, and the game says so on screen.

```sh
XAI_API_KEY=… make assets       # draw whatever is missing
XAI_API_KEY=… make assets-all   # redraw everything
```

The model returns an opaque JPEG, so every prompt asks for a pure black
background and the script keys it out into a real alpha channel:

```sh
magick in.jpg -filter point -resize 256x256 \
  \( +clone -colorspace Gray -level 4%,14% \) \
  -alpha off -compose CopyOpacity -composite out.png
```

Alpha comes from luminance rather than `-transparent black`, because the glow
fades to black over many pixels and a hard key would leave a halo. `-filter
point` is nearest-neighbour, which is what keeps a 1024px render of pixel art
looking like pixel art at 256.

## How a frame is put together

```
model.lua      every decision, and not one love.* call
  ↓
main.lua       draws that model; turns clicks into calls on it
  ↓
worker.lua     a second thread, for anything that reaches a node
```

**`model.lua` holds the state and touches no LÖVE call.** A GUI whose logic only
runs inside `love.draw` cannot be tested; this one is driven headlessly by
`tests/model_test.lua` exactly the way the CLI's interactive loop is.

**`worker.lua` keeps the window alive.** `balance` and `send` take a network
round trip, and doing that on the main thread freezes everything — the animation
stops, the particles hang in the air, and a person reasonably concludes it has
crashed. It opens its own wallet over the same home, because the FFI handle
cannot cross a thread boundary and the store is append-only by design.

**The confirmation is the wallet's own.** The GUI does not compose the sentence
it asks you to approve. It sends once *without* `yes`, which makes the wallet
resolve the nonce, the gas price and the gas limit, check the balance covers all
of it, and refuse with `confirmation_required` — carrying the summary it would
have put to a human. That summary is what the dialog shows, so what you approve
is a transaction that is real and already priced. It is the same trick the CLI's
interactive mode uses, for the same reason.

## Controls

| | |
| --- | --- |
| `1` `2` `3` | wallets · send · network |
| `↑` `↓` | move through the wallet list |
| `Enter` | refresh the balance, or send |
| `Tab` | switch fields on the send screen |
| `Ctrl/Cmd+C` | copy the selected wallet's address |
| `Ctrl/Cmd+V` | paste into the focused field |
| `Esc` | cancel a confirmation, or quit |

Everything is clickable too — including **COPY** beside the address and
**PASTE** beside the recipient field. An address is 42 characters of hex that
nobody retypes correctly, so the clipboard is not a convenience here, it is the
only realistic way to move one.

## Tests

```sh
make test                        # all of it
luajit tests/init.lua anim       # one suite
```

| suite | what it covers |
| --- | --- |
| `anim` | easing bounds, frame-rate independence, springs that settle and do not explode |
| `particles` | that effects die, the cap holds, and homing coins actually arrive |
| `model` | wallets, screens, networks, the send flow and the form, against a real store |

They run without LÖVE, which is the payoff of keeping the logic free of `love.`
calls. What is left untested is drawing — a test could only check that by
comparing pixels, so `make shots` covers it instead by making a frame reviewable
from a terminal:

```sh
CWB_SHOT=/tmp/x.png CWB_SHOT_AFTER=2.5 CWB_SHOT_SCREEN=send \
  CWB_SHOT_KEYS="type:0xabc,tab,type:0.5" love lovegui
```

Runs normally for that long — so the entrance has settled and effects have
played — writes a PNG, and quits.

## Not here

The wallet itself. If something is missing, it belongs in `rustcli/core` and in
the Python implementation beside it, not in the GUI.
