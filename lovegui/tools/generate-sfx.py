#!/usr/bin/env python3
"""Bake the sound effects, the way a 1983 sound chip would have made them.

    python3 lovegui/tools/generate-sfx.py

Writes ``assets/sfx/*.wav`` and prints what it made. Both the script and its
output are committed; this only needs running to change a sound.

## Why synthesise rather than download

Every sample pack is somebody's licence to honour, somebody's server to stay
up, and a megabyte to carry. A PSG is none of those things — it is two square
waves, a triangle, a noise register and a four-bit volume, which is about a
hundred lines of arithmetic. Generating them means the sounds are *source*: a
blip is edited by changing a number here and re-running, not by opening an
audio editor and hoping.

It also means they genuinely are 8-bit rather than merely retro-flavoured.

## What makes it sound like a chip and not a synthesiser

Three constraints, all deliberate, none of them an approximation of something
better:

* **The volume is four bits.** An AY-3-8910 or a 2A03 had sixteen levels and no
  more, so every envelope here is quantised to sixteen steps. That stair-step
  on a decay tail is a large part of the sound — smooth it out and the result
  is a soft synth pretending.
* **The waveforms are the ones the hardware had.** Square at a handful of duty
  cycles, a stepped triangle, and noise from a linear-feedback shift register.
  No sine anywhere: a PSG could not make one.
* **The noise is a real LFSR**, the same 15-bit one the NES used, clocked by a
  divider. That is why it sounds like *that* — white noise from a random number
  generator has a different, hissier character.

Output is 8-bit unsigned mono at 22050 Hz, which is roughly what these chips
were sampled at and keeps every file a few kilobytes.
"""

from __future__ import annotations

import math
import struct
import sys
import wave
from pathlib import Path

SFX = Path(__file__).resolve().parent.parent / "assets" / "sfx"

RATE = 22050

# Sixteen volume levels, signed: the four-bit envelope the hardware had. The
# stair-step this puts on a decay is audible and wanted.
LEVELS = 15


# ---------------------------------------------------------------- oscillators


def square(phase: float, duty: float = 0.5) -> float:
    """A pulse wave. Duty is the whole character of it.

    0.5 is hollow and flute-like, 0.25 is the classic lead, 0.125 is thin and
    nasal — the three the NES could actually select, and the reason a chiptune
    lead and a chiptune bass sound different when they are the same waveform.
    """
    return 1.0 if (phase % 1.0) < duty else -1.0


def triangle(phase: float) -> float:
    """A stepped triangle, sixteen steps up and sixteen down.

    Not a smooth ramp: the 2A03's triangle channel walked a 4-bit counter, so
    the steps are in the hardware. They are what stops it sounding like a sine
    and give the bass its slight buzz.
    """
    p = phase % 1.0
    value = 4.0 * p - 1.0 if p < 0.5 else 3.0 - 4.0 * p
    return math.floor(value * 8.0 + 0.5) / 8.0


class Noise:
    """The NES's 15-bit linear-feedback shift register.

    Clocked by a divider rather than run at the sample rate, which is what
    gives noise a *pitch* — a high divider is a hiss, a low one is a rumble,
    and sweeping it is how every explosion and every rocket was made.

    The short-mode tap (bit 1 rather than bit 6) shortens the period enough
    that it becomes tonal and metallic. That is the coin-clink sound.
    """

    def __init__(self, short: bool = False):
        self.register = 1
        self.tap = 1 if short else 6
        self.phase = 0.0
        self.value = 1.0

    def sample(self, freq: float) -> float:
        self.phase += freq / RATE
        while self.phase >= 1.0:
            self.phase -= 1.0
            bit = (self.register & 1) ^ ((self.register >> self.tap) & 1)
            self.register = (self.register >> 1) | (bit << 14)
            self.value = -1.0 if (self.register & 1) else 1.0
        return self.value


# ----------------------------------------------------------------- envelopes


def decay(power: float = 3.0):
    """Struck and left to ring. The workhorse: every blip and every hit."""
    return lambda u: max(0.0, 1.0 - u) ** power


def hold(release: float = 0.15):
    """Flat, then off. A sustained note that does not fade while it plays."""
    def shape(u: float) -> float:
        if u >= 1.0:
            return 0.0
        if u > 1.0 - release:
            return (1.0 - u) / release
        return 1.0
    return shape


def build_up(tail: float = 0.15):
    """Grows all the way through, then stops. Thrust, not a hit.

    The opposite of every other envelope here, and it has to be: a decay says
    "that happened", and a launch is still happening.
    """
    def shape(u: float) -> float:
        if u >= 1.0:
            return 0.0
        if u > 1.0 - tail:
            return (1.0 - u) / tail
        return 0.25 + 0.75 * (u / (1.0 - tail)) ** 1.5
    return shape


def swell(attack: float = 0.25, power: float = 2.0):
    """Rises, then falls. Thrust building, rather than a hit."""
    def shape(u: float) -> float:
        if u < attack:
            return (u / attack) ** 0.6
        return max(0.0, 1.0 - (u - attack) / (1.0 - attack)) ** power
    return shape


# --------------------------------------------------------------------- pitch


NAMES = {"C": 0, "C#": 1, "D": 2, "D#": 3, "E": 4, "F": 5, "F#": 6,
         "G": 7, "G#": 8, "A": 9, "A#": 10, "B": 11}


def note(name: str) -> float:
    """"A4" -> 440.0. Written as notes because intervals are what carry."""
    pitch, octave = name[:-1], int(name[-1])
    semitones = NAMES[pitch] + (octave - 4) * 12 - 9
    return 440.0 * (2.0 ** (semitones / 12.0))


def sweep(start: float, end: float, curve: float = 1.0):
    """A pitch that slides. `curve` above 1 holds low then rushes.

    Deliberately the same shape as the rocket's `expo_in` in `ui/anim.lua`:
    the sound and the sprite accelerate together, which is most of why a
    launch reads as one event rather than a picture with a noise over it.
    """
    return lambda u: start * ((end / start) ** (min(1.0, max(0.0, u)) ** curve))


# --------------------------------------------------------------------- voices


def tone(duration, freq, envelope, wave="square", duty=0.5, gain=1.0,
         vibrato=0.0, vibrato_rate=6.0, delay=0.0):
    """One voice, rendered to a list of floats.

    `freq` is a number or a function of progress, so a slide costs nothing
    extra. Phase is accumulated rather than recomputed from `t`, which is the
    only way a slide stays continuous instead of clicking every sample.
    """
    total = int((delay + duration) * RATE)
    lead = int(delay * RATE)
    out = [0.0] * total
    phase = 0.0
    noise = Noise(short=(wave == "clink"))

    for i in range(lead, total):
        u = (i - lead) / max(1, duration * RATE)
        f = freq(u) if callable(freq) else freq
        if vibrato:
            f *= 1.0 + vibrato * math.sin(2.0 * math.pi * vibrato_rate * u * duration)

        amplitude = envelope(u) * gain
        # Four bits, and the quantising happens *before* the waveform is
        # scaled — an envelope step is a step in level, not a smooth fade.
        amplitude = math.floor(amplitude * LEVELS + 0.5) / LEVELS

        if wave in ("noise", "clink"):
            value = noise.sample(f)
        elif wave == "triangle":
            phase += f / RATE
            value = triangle(phase)
        else:
            phase += f / RATE
            value = square(phase, duty)

        out[i] = value * amplitude

    return out


def mix(*voices):
    """Sum the voices. Deliberately does not clip.

    Clipping here would be irreversible and invisible: a sound would arrive at
    the mixing stage already squared off, and no amount of turning it down
    afterwards brings the waveform back. Levelling happens once, at the end,
    where it can be seen — see `MIX`.
    """
    length = max((len(v) for v in voices), default=0)
    out = [0.0] * length
    for voice in voices:
        for i, value in enumerate(voice):
            out[i] += value
    return out


def arpeggio(names, step, envelope=None, wave="square", duty=0.5, gain=0.8,
             last=None):
    """A run of notes, one after another.

    The chip's way of playing a chord: it had no polyphony to spare, so it
    played the notes fast enough in sequence that the ear hears one sound.
    """
    envelope = envelope or decay(2.0)
    voices = []
    for index, name in enumerate(names):
        length = last if (last and index == len(names) - 1) else step
        voices.append(tone(length, note(name), envelope, wave=wave, duty=duty,
                           gain=gain, delay=index * step))
    return mix(*voices)


# ----------------------------------------------------------------- the sounds


def build() -> dict[str, list[float]]:
    """Every effect, named for the moment it belongs to rather than its shape."""
    sounds: dict[str, list[float]] = {}

    # A cursor moving. Deliberately tiny — this one plays more than any other,
    # and anything with a tail turns a held arrow key into a drone.
    sounds["blip"] = tone(0.045, note("E6"), decay(4.0), duty=0.25, gain=0.5)

    # Hovering. Quieter and higher than the blip, because it is feedback for
    # something the person has not committed to yet.
    sounds["hover"] = tone(0.03, note("B6"), decay(5.0), duty=0.125, gain=0.22)

    # A button going in: two notes, up. The interval does the work — a fifth
    # reads as "yes, that happened" where a single note reads as a tick.
    sounds["press"] = arpeggio(["A5", "E6"], 0.035, decay(3.0), duty=0.25, gain=0.55)

    # Backing out. The same two notes, down. Nothing else needs to change for
    # it to mean the opposite.
    sounds["back"] = arpeggio(["E6", "A5"], 0.04, decay(3.0), duty=0.5, gain=0.45)

    # Changing screen: a quick three-note run, the sound of something sliding.
    sounds["tab"] = arpeggio(["D5", "A5", "D6"], 0.033, decay(3.5), duty=0.25, gain=0.5)

    # A refusal. A tritone, held then dropped — the interval every game has
    # used for "no" since there were games to use it.
    sounds["error"] = mix(
        tone(0.18, sweep(note("A3"), note("D#3")), hold(0.4), duty=0.5, gain=0.7),
        tone(0.22, 1400.0, decay(2.0), wave="noise", gain=0.25),
    )

    # Money arriving. The two-note coin, and a metallic tick over it from the
    # LFSR in short mode, which is where the *clink* comes from.
    sounds["coin"] = mix(
        arpeggio(["B5", "E6"], 0.075, decay(2.2), duty=0.125, gain=0.55, last=0.34),
        tone(0.05, 5200.0, decay(6.0), wave="clink", gain=0.3),
    )

    # The lock opening: a major arpeggio, unhurried enough to be heard as a
    # phrase rather than a chirp.
    sounds["unlock"] = mix(
        arpeggio(["C5", "E5", "G5", "C6"], 0.062, decay(2.2), duty=0.25,
                 gain=0.5, last=0.42),
        tone(0.5, note("C3"), decay(2.5), wave="triangle", gain=0.45),
    )

    # The lock refusing. Down a minor third, with a buzz under it.
    sounds["deny"] = mix(
        arpeggio(["F4", "C#4"], 0.09, hold(0.3), duty=0.5, gain=0.55, last=0.26),
        tone(0.3, 700.0, decay(1.5), wave="noise", gain=0.2),
    )

    # ------------------------------------------------------------- the rocket
    #
    # 1.3 seconds, to cover `LAUNCH_FLOOR` in main.lua with a little over. Three
    # layers, because a rocket is three things at once: the bang of ignition,
    # the roar of the motor, and the pitch climbing away from you.
    ignition = mix(
        tone(0.16, 900.0, decay(2.5), wave="noise", gain=0.85),
        tone(0.2, sweep(note("A2"), note("A1")), decay(2.0), duty=0.5, gain=0.6),
    )
    # The roar holds rather than decaying. A motor that fades over its own burn
    # sounds like it is failing — and the picture has the rocket accelerating
    # for the whole 1.25s, so the sound has to still be there at the end.
    roar = tone(1.3, sweep(7200.0, 700.0, 0.7), swell(0.1, 0.25), wave="noise",
                gain=0.5)
    # The climb uses the same exponential curve as the sprite's flight, so the
    # pitch is still low while the rocket is still on the pad and both leave
    # in the same instant. It grows into the moment it goes, which is where the
    # eye is by then.
    climb = tone(1.3, sweep(note("A2"), note("E6"), 2.4), build_up(0.15),
                 duty=0.25, gain=0.5, vibrato=0.012, vibrato_rate=11.0)
    sounds["launch"] = mix(ignition, roar, climb)

    # Arrival. A fanfare, with the bass moving under it so it lands.
    sounds["sent"] = mix(
        arpeggio(["G5", "C6", "E6", "G6"], 0.075, decay(2.0), duty=0.25,
                 gain=0.5, last=0.5),
        tone(0.8, note("C3"), decay(2.0), wave="triangle", gain=0.5),
        tone(0.5, 6000.0, decay(3.0), wave="clink", gain=0.18, delay=0.22),
    )

    # A wallet being made: a short rising run, brighter than `press` and
    # shorter than `sent`, because creating one is smaller than spending.
    sounds["created"] = mix(
        arpeggio(["E5", "A5", "C#6"], 0.055, decay(2.5), duty=0.25, gain=0.5,
                 last=0.3),
        tone(0.4, note("A2"), decay(2.5), wave="triangle", gain=0.4),
    )

    # ------------------------------------------------------------- the boot
    #
    # The tube coming on. A thump of noise sweeping down, which is a speaker
    # cone moving once, plus the low hum the flyback transformer left behind.
    sounds["power"] = mix(
        tone(0.45, sweep(4000.0, 120.0, 1.6), decay(1.6), wave="noise", gain=0.6),
        tone(0.6, note("C2"), decay(2.2), wave="triangle", gain=0.5),
    )

    # A line of the boot sequence typing. Has to be nearly nothing: it plays
    # once per line and would be unbearable otherwise.
    sounds["type"] = tone(0.022, note("A6"), decay(6.0), duty=0.125, gain=0.2)

    # `Ok`, and the machine is yours. The MSX's own two-note ready chirp.
    sounds["ready"] = arpeggio(["E6", "B6"], 0.07, decay(2.5), duty=0.25,
                               gain=0.45, last=0.3)

    return sounds


# --------------------------------------------------------------------- output


# How loud each sound ends up, as a fraction of full scale.
#
# Set here rather than left to whatever the voice gains happened to sum to.
# Doing it per-voice does not work: `error` stacks a square on a noise burst
# and came out three times the level of `blip`, which meant a mistake was
# louder than anything a person did on purpose. This is a mixing desk, and the
# ordering in it is the actual design —
#
#   whispers   things that fire constantly and must not be noticed
#   taps       things a person did, heard once and gone
#   events     something arrived, or was refused
#   moments    the two sounds allowed to take over: the launch and the boot
#
# Levelling to peak also means nothing clips, whatever the voices sum to.
MIX = {
    "hover": 0.20, "type": 0.20,                                    # whispers
    "blip": 0.42, "press": 0.50, "back": 0.45, "tab": 0.50,             # taps
    "ready": 0.55, "created": 0.68, "coin": 0.70, "unlock": 0.75,     # events
    "deny": 0.62, "error": 0.68,
    "sent": 0.85, "launch": 0.92, "power": 0.80,                    # moments
}


def level(samples: list[float], target: float) -> list[float]:
    """Scale so the loudest sample lands exactly on `target`."""
    peak = max((abs(v) for v in samples), default=0.0)
    if peak <= 0.0:
        return samples
    return [v * (target / peak) for v in samples]


def write(path: Path, samples: list[float]) -> int:
    """8-bit unsigned mono, which is the format these chips were sampled to.

    A short fade on the last few milliseconds: a waveform cut mid-cycle ends
    on a step, and a step is a click. Two hundred samples is inaudible as a
    fade and completely removes it.
    """
    tail = min(200, len(samples))
    for i in range(tail):
        samples[len(samples) - tail + i] *= 1.0 - (i / tail)

    frames = bytearray()
    for value in samples:
        clipped = max(-1.0, min(1.0, value))
        frames.append(int(round(clipped * 127.0)) + 128)

    with wave.open(str(path), "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(1)
        out.setframerate(RATE)
        out.writeframes(bytes(frames))

    return len(frames)


def main() -> int:
    SFX.mkdir(parents=True, exist_ok=True)

    only = set(sys.argv[1:])
    sounds = build()

    # Every sound must have a level, and every level must have a sound — a
    # typo in either table would otherwise silently ship one effect at whatever
    # its voices summed to, which is exactly the bug `MIX` exists to prevent.
    if set(sounds) != set(MIX):
        raise SystemExit(
            f"MIX and build() disagree: {sorted(set(sounds) ^ set(MIX))}")
    sounds = {name: level(data, MIX[name]) for name, data in sounds.items()}

    if only:
        unknown = only - set(sounds)
        if unknown:
            raise SystemExit(f"no such sound: {', '.join(sorted(unknown))}")
        sounds = {name: data for name, data in sounds.items() if name in only}

    total = 0
    for name, samples in sorted(sounds.items()):
        written = write(SFX / f"{name}.wav", samples)
        total += written
        print(f"  {name:<9} {written / RATE:5.2f}s  {written / 1024:6.1f} KB")

    print(f"  {len(sounds)} sounds, {total / 1024:.1f} KB total -> "
          f"{SFX.relative_to(SFX.parent.parent)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
