--- The sound chip: playing the effects in `assets/sfx`.
---
--- Those files are square waves, a stepped triangle and a shift register,
--- synthesised by `tools/generate-sfx.py` and committed. There is no sample
--- pack and no licence to honour — a blip is edited by changing a number in
--- that script and running it again.
---
--- ## What this file is actually for
---
--- Playing a sound is one line of LÖVE. Everything here exists because of the
--- three ways doing it naively goes wrong:
---
--- **A Source is one voice.** Calling `play` on a Source that is already
--- playing restarts it, so two coins landing a frame apart become one coin.
--- Each effect keeps a small pool of clones and takes them in turn.
---
--- **The UI fires far more often than an ear wants.** Hover is evaluated every
--- frame for every button; a held arrow key steps the list at the key-repeat
--- rate. Without a throttle the result is not a sound effect, it is a buzz. So
--- every effect has a minimum gap, and the ones that fire most have the
--- longest.
---
--- **The same sample twice in a row sounds like a stuck machine.** A few
--- percent of random detune on the effects that repeat is enough for the ear
--- to hear two separate events rather than one glitch.
---
--- ## It has to survive not being there
---
--- Same rule as the art: a checkout without `assets/sfx`, a machine with no
--- audio device, and the headless test suite all have to work. Missing sounds
--- are recorded and the game says so once at startup; every `play` after that
--- is a no-op. Nothing in here is allowed to be load-bearing.

local sound = {}

--- Every effect, and how loud it is *relative to the others*.
---
--- Absolute levels are baked into the files by the generator's `MIX` table.
--- These are the second, smaller adjustment: the place to make one effect sit
--- back a little without regenerating anything.
local WANTED = {
  -- Interface.
  hover   = 0.7,
  blip    = 0.9,
  press   = 1.0,
  back    = 0.9,
  tab     = 1.0,
  card    = 1.0,
  -- Outcomes.
  coin    = 1.0,
  created = 1.0,
  sent    = 1.0,
  error   = 0.9,
  -- The gate.
  unlock  = 1.0,
  deny    = 1.0,
  -- The rocket.
  launch  = 1.0,
  -- The machine coming up.
  power   = 0.9,
  type    = 0.8,
  ready   = 1.0,
}

--- How many of each sound can overlap. Four is enough for a fistful of coins
--- landing together and small enough that fifteen effects cost nothing.
local VOICES = 4

--- The shortest gap between two plays of the same effect, in seconds.
---
--- Tuned by what triggers it rather than by what it sounds like: `hover` is
--- evaluated every frame over every button, `blip` follows the key-repeat
--- rate, and `type` is emitted per line of a boot sequence that can be
--- skipped. The rest are things a person did deliberately, and those want no
--- more gate than enough to swallow a double-fire in one frame.
local THROTTLE = {
  hover = 0.10,
  blip  = 0.045,
  type  = 0.03,
  card  = 0.07,
  coin  = 0.035,
}
local THROTTLE_DEFAULT = 0.02

--- How much random detune each effect gets, as a fraction of its pitch.
---
--- Only the ones that repeat. `sent` and `unlock` are musical phrases that
--- play once, and detuning those would just make them sound out of tune.
local JITTER = {
  hover = 0.08,
  blip  = 0.06,
  type  = 0.14,
  coin  = 0.10,
  press = 0.03,
  back  = 0.03,
  card  = 0.07,
}

sound.pools = {}
sound.missing = {}
sound.last = {}
sound.enabled = true
sound.volume = 0.75

--- A clock advanced by `update`, not read from `love.timer`.
---
--- The throttle is the one piece of policy in this file worth testing, and a
--- test cannot advance a real clock. Driving it from `dt` means the suite can
--- step it by any amount it likes, and the game's behaviour is identical.
sound.clock = 0

--- Where the mute setting lives between runs.
---
--- A person who turns the sound off wants it to stay off. One byte in the save
--- directory; if it cannot be read or written, the sound is simply on, which
--- is the right way for a preference to fail.
local SETTING = "muted"

local function audio_available()
  return love ~= nil and love.audio ~= nil and love.sound ~= nil
end

function sound.load()
  if not audio_available() then return end

  for name in pairs(WANTED) do
    local path = "assets/sfx/" .. name .. ".wav"
    if love.filesystem.getInfo(path) then
      -- Decoded once into SoundData and shared by every clone, so four voices
      -- cost four cursors rather than four copies of the audio.
      local data = love.sound.newSoundData(path)
      local pool = { index = 0, voices = {} }
      for i = 1, VOICES do
        pool.voices[i] = love.audio.newSource(data, "static")
      end
      sound.pools[name] = pool
    else
      sound.missing[#sound.missing + 1] = name
    end
  end

  table.sort(sound.missing)

  if love.filesystem.getInfo(SETTING) then
    sound.enabled = love.filesystem.read(SETTING) ~= "off"
  end
end

function sound.update(dt)
  sound.clock = sound.clock + dt
end

--- Whether policy lets this effect sound right now, and records that it did.
---
--- Split out from `play` on purpose: this is the part with a decision in it,
--- and it holds no reference to LÖVE, so the throttle can be tested by
--- stepping `sound.clock` with no audio device anywhere.
function sound.allowed(name)
  if not sound.enabled then return false end
  local gate = THROTTLE[name] or THROTTLE_DEFAULT
  local last = sound.last[name]
  if last and sound.clock - last < gate then return false end
  sound.last[name] = sound.clock
  return true
end

--- Play an effect. Returns true if it actually sounded.
---
--- `options.pitch` multiplies the pitch — the caller's way of saying "the same
--- sound, but higher because this is the fifth coin". `options.volume`
--- likewise. Both are on top of the table entries above rather than instead of
--- them.
function sound.play(name, options)
  options = options or {}
  if not sound.allowed(name) then return false end

  local pool = sound.pools[name]
  if not pool then return false end

  -- Round-robin rather than "find one that is not playing": if all four are
  -- busy the oldest is the right one to steal, and that is what taking them in
  -- order does for free.
  pool.index = pool.index % #pool.voices + 1
  local voice = pool.voices[pool.index]

  local spread = JITTER[name] or 0
  local pitch = (options.pitch or 1) * (1 + (math.random() * 2 - 1) * spread)

  voice:stop()
  voice:setPitch(math.max(0.05, pitch))
  voice:setVolume(sound.volume * (WANTED[name] or 1) * (options.volume or 1))
  voice:play()
  return true
end

--- Stop every voice of one effect. For the launch, which can be cut short.
function sound.stop(name)
  local pool = sound.pools[name]
  if not pool then return end
  for _, voice in ipairs(pool.voices) do voice:stop() end
end

function sound.stop_all()
  for name in pairs(sound.pools) do sound.stop(name) end
end

--- Turn the sound on or off, and remember which.
function sound.toggle()
  sound.enabled = not sound.enabled
  if not sound.enabled then sound.stop_all() end
  if audio_available() and love.filesystem then
    pcall(love.filesystem.write, SETTING, sound.enabled and "on" or "off")
  end
  return sound.enabled
end

return sound
