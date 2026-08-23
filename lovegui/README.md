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
make app       # a double-clickable macOS .app with LÖVE inside it
make test      # 174 headless tests, no LÖVE required
make shots     # writes a PNG of each screen, for review from a terminal
make sfx       # re-synthesises the sound effects
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

Pressing **`0`** on the title screen plays the whole thing again, after three
seconds of black. That is for recording it: a capture wants somewhere to cut
and a moment for the recorder to settle before the tube comes on, and an intro
you can only see once per launch is an intro nobody gets a clean take of. A
stray key during the black does not cut it short.

**Everything on that screen is true.** The ABI number is what `cwb_abi_version`
returned, the version is the library's, the network and the wallet count come
from `info`, and if the library did not load it says so in red and halts rather
than handing off to a UI that cannot work. The only invented figures are the two
memory counts — 65536 and 16384 are an MSX1's RAM and VRAM, which is the whole
reference. A boot screen that reports fake state is a lie printed in a font that
makes it look authoritative, and it is the first thing anyone sees.

## Getting in

A mnemonic opens the window, and the screen is honest about what that is:

> a session gate, not encryption
> keys stay unencrypted on disk

It decides *which* wallet you are looking at and keeps the screens behind it
until a phrase is entered. It is not protecting the store — that is plain JSONL
exactly as it was before, and anyone with the disk has the keys whether or not
this screen has been passed. A lock that implies more safety than it provides
is worse than no lock, so it is labelled rather than dressed up.

**The phrase is never drawn.** Not masked-with-a-reveal — never. It is the
whole wallet in twelve words, and a SHOW toggle is one shoulder or one
screenshot away from being the last mistake somebody makes. What replaces
seeing it is the word count, which turns green at twelve or twenty-four, and
one button that changes with the situation:

| | |
| --- | --- |
| **PASTE** | a phrase you already have, brought in from the clipboard |
| **COPY** | a phrase just minted by NEW MNEMONIC, taken out before you use it |

COPY appears only for a phrase this screen generated, because that one exists
nowhere else yet and a wallet whose mnemonic was never saved is a wallet nobody
can recover. Everything is checked before anything is written:
`validate-mnemonic` says whether it is a phrase at all, `derive` turns it into
an address without touching the store, and only then does the model choose
between selecting a wallet it knows and importing one it does not. A typo
produces a message, never a stray account.

**Unlocking makes that wallet the one in use** — both branches, the phrase the
store already knew and the one it did not. Logging in *as* a wallet while the
money moves from a different one is exactly the mismatch this screen exists to
prevent. It used to be true of a known phrase and not of a new one, so NEW
MNEMONIC → COPY → UNLOCK created the wallet, put it on screen, and left the
store spending from whichever wallet was active before.

### A session is one mnemonic

The store is one home directory and can hold wallets from a dozen different
phrases. Showing all of them behind any one of them made this a doorway rather
than a gate: unlocking with a brand new phrase produced a "new wallet" sitting
in a list of somebody else's.

So the list is **scoped to the wallets the phrase controls**. Nothing is hidden
that the phrase can reach, and nothing is shown that it cannot. Nothing is
deleted either — the other wallets are still on disk, and logging out shows the
whole store again.

Which wallets those are is **derived, not read**. The store keeps each account's
mnemonic and `accounts()` deliberately does not hand it out — which is correct —
so the only honest way to ask "does this phrase own that wallet?" is to derive
the addresses and see which ones are there. Login walks the BIP-44 indices with
a gap limit, the same way any wallet scans for accounts: keep going until
several in a row are absent, then stop. A wallet whose accounts are 0 and 3 is
found whole; one with a hundred is not scanned a hundred times on every login.
It costs under a millisecond an index and nothing is remembered between runs.

That makes the two buttons mean different things, which is worth knowing:

| | |
| --- | --- |
| **NEW MNEMONIC** | starts a *separate wallet*, with its own phrase |
| **+ NEW** | adds an account to the wallet you are in, recoverable from the same phrase |

`+ NEW` continues the active account's mnemonic, so an account made inside a
session is the next index of that phrase and comes back the next time it is
unlocked. Nothing made in the window is ever stranded behind a phrase you were
never shown.

**LOGOUT** in the header comes back here, to a screen holding nothing: no
phrase, not minted, offering PASTE rather than COPY. Nothing carries over from
the session that ended, so NEW MNEMONIC after a logout starts a genuinely new
wallet — and shows only that wallet.

The phrase is wiped rather than merely dropped — `login:forget()` runs the
moment a session opens and again on the way out. Releasing the last reference
to a string is not the same as clearing it, and "no mnemonic outlives the
session" should be a property with a test on it rather than a consequence of
who remembers to rebuild the screen.

Pasting goes through one path whether it came from the button or `Ctrl+V`.
They were separate, and the key left `minted` alone — so pasting over a freshly
minted phrase left the screen still offering to COPY it while holding a
different one.

### What still touches a mnemonic

Three things do, and all three are deliberate. Naming them is the point — an
unlisted one is the one that surprises somebody.

**COPY puts the phrase on the system clipboard.** That is the whole feature: a
minted phrase exists nowhere else and has to get somewhere safe. But the
clipboard is shared with every application on the machine and is not cleared
here, so a phrase copied out stays there until something else replaces it.
Paste it where it is going, then copy something harmless.

**The store holds it in plain text**, as `mnemonic` and `private_key` fields in
`accounts.jsonl`. That predates this window and is what the warning across the
bottom of every screen is about. `accounts.jsonl` and its siblings are ignored
by name in `.gitignore`, at the repository root and again here.

**`CWB_SHOT_KEYS` carries whatever it replays**, and an environment variable is
visible to `ps` for the life of the process. `make shots` uses BIP-39's
published all-zeros vector, which is a secret to nobody; if you point the shot
harness at a real phrase, that is where it goes.

What does *not* touch it: nothing is written to a log, no error message quotes
it back (`Model.without_phrase`), and it is never drawn — see the tests in
`tests/login_test.lua`, which fail if a reveal toggle is ever added.

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

## The card

A wallet is shown as a bank card, dealt from its own address.

A list of hex strings is a list of hex strings. Nobody recognises
`0x9858Ef…Eda94`, nobody can tell it from `0x9858Ff…Eda94` at a glance, and
everybody has to read all forty characters to be sure. A card is a **face**:
after seeing it twice you know your green one with the rocket, and the moment
the wrong card is on screen you know that too, before reading a character.

That is the real argument for it. Recognition beats verification — an address
checked character by character gets checked carefully the first three times and
skimmed forever after.

### Everything on it comes out of the address

No randomness, no stored preference, no counter. The same address deals the
same card on every machine, in every run, forever, which is the only way a face
is worth anything. The address is twenty bytes of hash output and already
evenly distributed, so there is nothing to gain by hashing it again — different
bytes simply drive different choices:

| byte | decides |
| --- | --- |
| 1 | the colourway — six, all built from the same sixteen palette colours |
| 2 | the background pattern — stripes, grid, circuit, waves, stars, chevron |
| 3 | the emblem, stamped like a hologram from the sprites already in `assets/` |
| 4–5 | the sigil: a 5×5 identicon, mirrored, because a symmetric shape reads as a mark and an asymmetric one reads as noise |
| 6 | the tier — weighted, so a BLACK card is three wallets in a hundred |
| 19–20 | the member number |

The tier means nothing. It buys no feature and unlocks nothing; it is there
because getting one should feel like something.

The card number is the whole address in groups of four — nothing hidden,
nothing abbreviated, and a shape every person alive already knows how to read.
The balance is printed on the card, and **only on the card that is active**: an
inactive card says so instead, because a card with somebody else's money
printed on it is the one mistake this whole design exists to prevent.

### Choosing another one swipes it away

The card you had slides out while the one you asked for slides in, **both on
screen at once**. That overlap is the whole difference between a swipe and a
cut: for a moment you can see them travel together, which is what makes it read
as a stack of cards being moved through rather than a panel whose contents were
replaced.

Moving *down* the list scrolls the card **left** — the way the eye expects a
list to move under a cursor going down — and moving up reverses it. Neither
card fades: the one leaving is opaque until it is gone, and it goes by being
clipped at the edge of the column, the way a card leaves a window in the
physical world.

The curve was chosen by measuring, not by taste. The requirement is that both
cards share the column for most of the swipe, and how long that lasts is a
property of the curve:

| | both visible | |
| --- | --- | --- |
| `linear` | 93% | no easing at all |
| **`quad_out`** | **79%** | 19% moved by a tenth of the time, 75% by half |
| `smoothstep` | 77% | but only 3% moved by a tenth — a mechanical slide |
| `cubic_out` | 65% | |
| `expo_out` | 47% | half the distance in the first tenth |
| `expo_in_out` | 37% | still for a quarter, then a snap |

`expo_out` was the first attempt, on the reasoning that a swipe should feel
thrown. It does — and it puts half the distance in the first thirty
milliseconds, so the outgoing card is gone before the eye finds it and what you
see is a new card *appearing* from the right. That is a cut with a slide on the
end of it. `quad_out` keeps the deceleration that makes an arrival feel like an
arrival and still leaves both cards on screen together for four fifths of it.

At either end of the animation a card sits exactly on the layout's mark, so
nothing needs clamping at the seams and repeated swipes cannot drift.

It is clipped to the column, which is not a detail: a card number sliding
across the wallet list is not a transition, it is a bug with an easing curve on
it. The clip follows the current transform, or it would stand still while the
screen shake moved everything under it.

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

## Sound

Fifteen effects in `assets/sfx`, **synthesised** by `tools/generate-sfx.py`
with nothing but the Python standard library and committed alongside the art.

No sample pack — which means no licence to honour, no server to stay up, and
nothing to download. It also means the sounds are *source*: a blip is edited by
changing a number in that script and running it again, not by opening an audio
editor and hoping.

```sh
make sfx        # re-synthesise everything (no API key, no network)
make sfx-check  # assert the committed WAVs are present and well-formed
```

### It is a chip, not a chip filter

Same principle as the graphics. Three constraints, all deliberate:

* **The volume is four bits.** An AY-3-8910 or a 2A03 had sixteen levels and no
  more, so every envelope is quantised to sixteen steps. That stair-step on a
  decay tail is a large part of the sound.
* **Only the waveforms the hardware had** — pulse at a few duty cycles, a
  stepped triangle, and noise. There is no sine anywhere, because a PSG could
  not make one.
* **The noise is a real LFSR**, the same 15-bit shift register the NES used,
  clocked by a divider so it has a *pitch*. Sweeping that divider is how every
  explosion and every rocket in 1985 was made, and it is how this one is too.

Levels are set in one table (`MIX`) rather than left to whatever the voices
summed to. Doing it per-voice was tried and `error` came out three times the
level of `blip` — which meant a mistake was louder than anything a person did
on purpose. Levelling to peak in one place also means nothing clips.

### Playing them is the part that goes wrong

`ui/sound.lua` exists for three reasons, none of them "play a wav":

* **A Source is one voice.** Calling `play` on a Source that is already playing
  restarts it, so two coins landing a frame apart become one coin. Each effect
  keeps a pool of four and takes them in turn.
* **The UI fires far more often than an ear wants.** Hover is evaluated every
  frame for every button. Ungated, that is not a sound effect, it is a buzz —
  so every effect has a minimum gap, and the ones that fire most have the
  longest.
* **The same sample twice running sounds like a stuck machine.** A few percent
  of random detune on the effects that repeat is enough for the ear to hear two
  events rather than one glitch. Moving through the wallet list also pitches by
  position, so running down twelve wallets is a falling scale.

The throttle and the mute are the testable half and they are tested — with no
LÖVE and no audio device, because the decisions live in `sound.allowed` and the
clock is advanced by `dt` rather than read from a timer.

**SFX** in the header, or `M`, turns it off, and it stays off next time. The
button exists because the key cannot be the only way: every letter is typed
into a field on some screen, so `M` has to be ignored where text is expected,
and a control that stops working on some screens is not one anybody trusts.

A missing sound is a no-op and is named on screen next to any missing art —
a silent effect and a working mute look identical otherwise.

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

## The rocket

A confirmed transfer launches one, and it is the one animation with a
**floor**: `LAUNCH_FLOOR` is 1.25 seconds and the outcome waits behind it.

A node can answer in eighty milliseconds. An effect that is over before the eye
finds it may as well not have happened — the transfer would read as a number
that silently changed. So a result that arrives early is *held* and celebrated
when the rocket has had its moment. Nothing is delayed except the telling; the
transfer already happened.

The flight is `expo_in`, and that is its whole character: almost nothing for
the first third, then it is gone. A linear rise reads as an elevator. Exhaust
thickens with the throttle and is emitted from where the rocket actually is, so
the plume stays attached; the screen shake grows as `t²` rather than thumping
once; the rocket stretches vertically and narrows as it goes. The motor's pitch
climbs on the same exponential curve, so the sound and the sprite leave in the
same instant.

`make shots` captures two frames of it, because the first half and the second
half of an exponential look nothing alike.

## Controls

| | |
| --- | --- |
| `1` `2` `3` | wallets · send · network |
| `↑` `↓` | move through the wallet list |
| `PgUp` `PgDn` · wheel | scroll it |
| `Enter` | refresh the balance, or send |
| `Tab` | switch fields on the send screen |
| `Ctrl/Cmd+C` | copy the selected wallet's address |
| `Ctrl/Cmd+V` | paste into the focused field |
| `M` | sound on or off |
| `F11` · `Alt+Enter` | fullscreen, or the **FULL/WIN** button |
| `0` | on the title screen: replay the intro from black |
| `Esc` | cancel a confirmation, or quit |

Everything is clickable too — **COPY** under the card, **PASTE** beside the
recipient field, **USE CARD** to spend from the one on screen, **SFX**,
**FULL/WIN** and **LOGOUT** in the header. An address is 42 characters of hex
that nobody retypes correctly, so the clipboard is not a convenience here, it
is the only realistic way to move one.

## Tests

```sh
make test                        # all of it
luajit tests/init.lua anim       # one suite
```

| suite | what it covers |
| --- | --- |
| `anim` | easing bounds, frame-rate independence, springs that settle and do not explode |
| `particles` | that effects die, the cap holds, and homing coins actually arrive |
| `sound` | the throttle, mute, and the voice pool — the parts with decisions in them |
| `card` | that a face is deterministic, spread across every scheme, and survives a malformed address |
| `login` | that the phrase is never drawn, the word count, and what submitting does |
| `boot` | that every figure on the boot screen is the wallet's own, and that a missing library halts |
| `model` | wallets, screens, networks, the session, the list window, the send flow and the form, against a real store |

### Whether the tests would actually fail

A green suite says nothing about whether it would catch a regression — a test
that cannot fail is decoration. So `make mutate` breaks the code on purpose:

```sh
make mutate                 # every mutation
make mutate ARGS=login      # just the ones about the login screen
```

Each mutation is a plausible edit — a dropped clamp, a removed guard, an
off-by-one, the kind of thing a refactor does by accident — applied to a copy
of the tree. Twenty-one of them, and the suite has to notice every one. It
found two real holes when it was first run: `reveal` could scroll one row too
far without any test minding, and creating a wallet could stop selecting it.

One mutation is marked **equivalent**: removing the `validate-mnemonic` call
from `Model:login` changes nothing observable, because `derive` rejects exactly
the same phrases with the same code and message. That is recorded with its
reasoning rather than papered over with a test asserting an implementation
detail — which would be worse than the hole it hid.

It is not part of `make check`; it copies the tree and runs the suite twenty-odd
times, which is a thing to do when the tests change.

They run without LÖVE, which is the payoff of keeping the logic free of `love.`
calls. What is left untested is drawing — a test could only check that by
comparing pixels, so `make shots` covers it instead by making a frame reviewable
from a terminal:

```sh
CWB_SHOT=/tmp/x.png CWB_SHOT_AFTER=2.5 CWB_SHOT_SCREEN=send \
  CWB_SHOT_KEYS="type:0xabc,tab,type:0.5" love lovegui
```

Runs normally for that long — so the entrance has settled and effects have
played — writes a PNG, and quits. `CWB_SHOT_KEYS` takes `type:…` for text and a
key name for anything else, plus one step that is not a key at all: `launch`
starts the rocket, because the only other way to reach it is a funded account
and a node, and the flight itself is pure animation.

## Shipping it

Two shapes, for two different people.

```sh
make package   # a .love, its library, and a launcher — needs LÖVE installed
make app       # a macOS .app with LÖVE inside it — needs nothing
```

`make app` downloads LÖVE, embeds the game, builds an icon from the game's own
logo, signs the bundle and zips it. Three details are not optional:

* **The library goes in `Contents/Frameworks`**, where a signed bundle keeps
  its nested binaries — and its path is worked out by `main.lua` and handed to
  `open` explicitly. A checkout finds the library by walking up to
  `rustcli/target`; a bundle has no checkout to walk up to, `love.filesystem`
  is sandboxed and cannot look outside the archive, and the one thing that
  would fix it — `CAUSEWAYBAY_LIB` — cannot be set by a double-click. The
  worker thread is handed the same path, because a bundle that found its
  library on one thread and not the other would start, show nothing, and never
  say why.
* **Three entitlements, all LuaJIT's fault**: `allow-jit` and
  `allow-unsigned-executable-memory`, without which the app dies the moment
  LuaJIT compiles anything; and `disable-library-validation`, without which
  dlopen refuses to load a `.dylib` not signed by whoever signed LÖVE.
* **Signed inside-out** — nested binaries, then the executable, then the
  bundle. Signing the outer bundle first invalidates it the moment anything
  inside is signed afterwards.

It signs with a Developer ID if the machine has one and falls back to ad-hoc,
which runs locally and is not enough to hand to somebody else without them
right-clicking Open.

## Not here

The wallet itself. If something is missing, it belongs in `rustcli/core` and in
the Python implementation beside it, not in the GUI.
