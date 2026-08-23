--- The boot sequence: an MSX waking up.
---
--- A home computer of 1983 did not open a window, it *booted* — a black screen,
--- a bang of colour as the tube came on, a memory count, a few lines about what
--- hardware it had found, and then `Ok` with a blinking cursor waiting for you.
--- This is that, in front of the wallet.
---
--- ## Everything on it is true
---
--- The temptation with a sequence like this is to write the lines as decoration
--- and move on. These are not: the ABI number is what `cwb_abi_version`
--- returned, the store path is where the wallet actually is, the network and the
--- account count come from `info`, and if the library did not load the screen
--- says so in red and stops. A boot screen that reports fake state is a lie
--- printed in a font that makes it look authoritative — and this one is the
--- first thing a person sees, so it is the worst place to start lying.
---
--- The memory figures are the exception, and they are the joke: 65536 and 16384
--- are an MSX1's RAM and VRAM, which is the point of the reference. They count
--- up because that is what the machine did.

local theme = require("ui.theme")
local anim = require("ui.anim")
local sprite = require("ui.sprite")
local sound = require("ui.sound")

local boot = {}
boot.__index = boot

--- How long the tube takes to come on, before any text.
local POWER_ON = 0.55

--- Seconds between lines. Slow enough to read, fast enough that a person who
--- has seen it twice is not held hostage — and any key skips to the end.
local LINE_DELAY = 0.14

--- Characters per second, once a line starts typing.
local TYPE_RATE = 90

--- Build the sequence from what the wallet actually reports.
---
--- `wallet` may be nil — that is the case where the library did not load, and
--- the boot screen is then the only thing that will ever be shown, so it has to
--- carry the failure rather than hand off to a UI that cannot work.
function boot.new(wallet, failure)
  local self = setmetatable({
    time = 0,
    lines = {},
    shown = 0,       -- how many lines have started
    typed = 0,       -- characters typed on the current line
    done = false,    -- every line is out
    ready = false,   -- the `Ok` prompt is up and it will accept a key
    finished = false,-- the user has pressed something; hand over to the UI
    failure = failure,
    flash = 1,
    handover = 0,
  }, boot)

  local function line(text, colour, kind)
    self.lines[#self.lines + 1] = { text = text, colour = colour, kind = kind }
  end

  line("CAUSEWAYBAY BANK (C) 2026", theme.colour.cyan)
  line("8-BIT WALLET SYSTEM", theme.colour.dim)
  line("", theme.colour.dim)

  -- The two counters, which animate rather than appearing whole.
  line("MAIN RAM", theme.colour.text, "ram")
  line("VRAM", theme.colour.text, "vram")
  line("", theme.colour.dim)

  if not wallet then
    line("FFI LIBRARY      NOT FOUND", theme.colour.red)
    line("", theme.colour.dim)
    for chunk in tostring(failure and failure.message or "unknown"):gmatch("[^\n]+") do
      line(theme.ellipsis(chunk, 44, 0), theme.colour.faint)
    end
    line("", theme.colour.dim)
    line("SYSTEM HALTED", theme.colour.red)
    self.halted = true
    return self
  end

  local described = wallet:describe() or {}
  local info = wallet:info() or {}
  local accounts = wallet:accounts() or {}

  line(("FFI ABI %10s   OK"):format(described.abi or "?"), theme.colour.text)
  line(("VERSION %10s   OK"):format(wallet:version() or "?"), theme.colour.text)
  line(("NETWORK %10s"):format(info.network or "?"), theme.colour.text)
  line(("CHAIN   %10s"):format(info.chain_id or "?"), theme.colour.text)
  line(("WALLETS %10d"):format(#accounts), theme.colour.text)
  line("", theme.colour.dim)
  line("Ok", theme.colour.green)

  return self
end

--- Every line up to and including the one being typed, for drawing.
function boot:visible()
  return math.min(self.shown, #self.lines)
end

--- Skip to the end. Any key does this; a second one hands over.
function boot:skip()
  if self.halted then return end
  if not self.done then
    self.shown = #self.lines
    self.typed = math.huge
    self.done = true
    self.ready = true
    self.chirped = true
    sound.play("ready")
    return
  end
  if self.ready then self.finished = true end
end

function boot:update(dt)
  -- The speaker thump as the tube comes on. Fired from `update` rather than
  -- from `new`, because `new` runs while the wallet is still being opened and
  -- the sound would play against a black screen a beat before the picture.
  if not self.thumped then
    self.thumped = true
    sound.play("power")
  end

  self.time = self.time + dt
  self.flash = self.flash * math.exp(-6 * dt)

  if self.finished then
    self.handover = math.min(1, self.handover + dt * 2.4)
    return
  end

  if self.done then
    if not self.chirped then
      self.chirped = true
      sound.play("ready")
    end
    self.ready = true
    return
  end

  -- The tube has to warm up before anything is on it.
  if self.time < POWER_ON then return end

  local current = self.lines[self.shown]
  if current then
    self.typed = self.typed + TYPE_RATE * dt
    -- A counter line is "typed" for as long as it takes to count.
    local length = (current.kind and 18) or #current.text
    if self.typed < length then return end
  end

  self.next_line = (self.next_line or 0) + dt
  if self.next_line >= LINE_DELAY or self.shown == 0 then
    self.next_line = 0
    self.shown = self.shown + 1
    self.typed = 0
    -- One tick per line rather than per character: at 90 characters a second
    -- a per-character tick is not a typewriter, it is a buzz.
    local line = self.lines[self.shown]
    if line and line.text ~= "" then
      sound.play("type", { pitch = 0.9 + (self.shown % 5) * 0.06 })
    end
    if self.shown > #self.lines then
      self.shown = #self.lines
      self.done = true
    end
  end
end

--- A counter line, part way through counting.
local function counted(kind, progress)
  local total = kind == "ram" and 65536 or 16384
  local value = math.floor(total * math.min(1, progress))
  -- Snapped to a round step, so it reads as a memory test rather than a
  -- smoothly interpolated number, which no 8-bit machine could have shown.
  value = value - (value % 1024)
  if progress >= 1 then value = total end
  local label = kind == "ram" and "MAIN RAM" or "VRAM"
  return ("%-8s %10d BYTES OK"):format(label, value)
end

function boot:draw()
  local width, height = theme.WIDTH, theme.HEIGHT

  -- The tube coming on: a bright band that opens vertically out of nothing.
  local warm = math.min(1, self.time / POWER_ON)
  if warm < 1 then
    local open = anim.expo_out(warm)
    local h = math.max(1, height * open)
    theme.rect(theme.colour.cyan, 0, (height - h) / 2, width, h, 0.10 + 0.5 * (1 - open))
    theme.rect({ 1, 1, 1 }, 0, height / 2 - 1, width, 2, (1 - open) * 0.9)
    theme.scanlines(0.12)
    return
  end

  -- The handover: everything blows out to white and then the UI takes over.
  local fade = 1 - self.handover

  -- The town rises out of the dark as the machine comes up, so the title card
  -- is a reveal rather than a caption over nothing.
  local revealed = anim.expo_out(math.min(1, math.max(0, (self.time - POWER_ON) / 3)))
  sprite.backdrop("krumlov", {
    alpha = revealed * 0.9 * fade,
    scrim = (0.72 - revealed * 0.22) * fade,
    drift_x = math.sin(self.time * 0.09) * 3,
    drift_y = math.cos(self.time * 0.07) * 2,
  })

  local left, top = 26, 34
  local step = 17

  for i = 1, self:visible() do
    local entry = self.lines[i]
    local y = top + (i - 1) * step
    local text = entry.text

    if entry.kind then
      local progress = (i < self.shown) and 1 or math.min(1, self.typed / 18)
      text = counted(entry.kind, progress)
    elseif i == self.shown and self.typed < #text then
      text = text:sub(1, math.floor(self.typed))
    end

    theme.text(text, left, y, entry.colour, theme.font.small, fade)

    -- The cursor sits at the end of whatever is being typed.
    if i == self.shown and not self.done then
      local blink = (self.time * 8) % 2 < 1.4
      if blink then
        theme.rect(theme.colour.cyan, left + theme.width(text, theme.font.small) + 1,
          y, 7, 13, fade)
      end
    end
  end

  -- `Ok`, and then the title card, and then the machine waits for you.
  if self.ready and not self.halted then
    local y = top + #self.lines * step
    if (self.time * 2) % 2 < 1.2 then
      theme.rect(theme.colour.green, left, y, 7, 13, fade)
    end

    -- Timed from when the prompt appeared rather than from boot, so it lands
    -- the same way whether the sequence was watched or skipped.
    self.titled = (self.titled or 0) + love.timer.getDelta()
    local t = anim.expo_out(math.min(1, self.titled / 0.7))
    local drop = (1 - t) * -16

    local cx = width / 2
    local card = height - 76

    sprite.draw_glowing("logo", cx, card + drop, 46 * t, {
      angle = math.sin(self.time * 0.6) * 0.05,
      glow = 0.45 + 0.3 * math.sin(self.time * 3),
      glow_colour = theme.colour.cyan,
      alpha = fade,
    })

    -- Drawn twice: a dark copy one pixel down, so the name sits on the screen
    -- rather than floating over it.
    theme.text_centred("CAUSEWAYBAY BANK", cx, card + 27 + drop, { 0, 0, 0 },
      theme.font.big, 0.7 * t * fade)
    theme.text_centred("CAUSEWAYBAY BANK", cx, card + 26 + drop, theme.colour.cyan,
      theme.font.big, t * fade)

    local pulse = anim.pulse(self.time, 1.4, 0.30, 0.95)
    theme.text_centred("PRESS ANY KEY", cx, height - 26,
      theme.colour.dim, theme.font.small, pulse * t * fade)
  end

  if self.halted then
    local shake_x = math.sin(self.time * 40) * 0.6
    theme.text_centred("THE WALLET COULD NOT START", width / 2 + shake_x, height - 34,
      theme.colour.red, theme.font.small, fade)
  end

  -- A CRT never showed a perfectly steady picture.
  theme.scanlines(0.13)
  theme.vignette(0.45)

  if self.flash > 0.01 then
    theme.rect({ 1, 1, 1 }, 0, 0, width, height, self.flash * 0.5)
  end
  if self.handover > 0 then
    theme.rect({ 1, 1, 1 }, 0, 0, width, height, (1 - math.abs(self.handover * 2 - 1)) * 0.85)
  end
end

--- True once the UI should take over.
function boot:complete()
  return self.finished and self.handover >= 1
end

return boot
