--- Tests for the sound policy.
---
--- Not for the audio: whether a square wave sounds like a square wave is a
--- question for ears, and `tools/generate-sfx.py` is where that is decided.
--- What is testable — and what actually breaks — is the layer above it:
---
---   * the throttle, which is the difference between a hover tick and a buzz;
---   * mute, which has to stop *everything*, including the effects fired from
---     inside widgets that never look at it;
---   * the round-robin, which is why two coins a frame apart are two coins.
---
--- All of it runs with no LÖVE and no audio device, because `sound.allowed`
--- holds the decisions and `sound.clock` is advanced by hand rather than read
--- from a timer. That split exists for exactly this.

local t = require("tests.runner")
local sound = require("ui.sound")

--- Back to a known state: these tests share one module.
local function reset()
  sound.enabled = true
  sound.clock = 0
  sound.last = {}
end

t.suite("sound / throttle", function()
  t.case("the same effect will not fire twice in one frame", function()
    reset()
    t.ok(sound.allowed("blip"), "the first blip should be allowed")
    t.ok(not sound.allowed("blip"), "a second blip in the same frame should not")
  end)

  t.case("it fires again once its gap has passed", function()
    reset()
    sound.allowed("blip")
    sound.update(0.02)
    t.ok(not sound.allowed("blip"), "20ms is inside the blip's 45ms gap")
    sound.update(0.04)
    t.ok(sound.allowed("blip"), "60ms is past it")
  end)

  t.case("different effects do not gate each other", function()
    reset()
    t.ok(sound.allowed("press"), "press")
    t.ok(sound.allowed("coin"), "coin should not be blocked by press")
    t.ok(sound.allowed("sent"), "nor sent")
  end)

  t.case("hover survives a pointer resting on a button", function()
    -- The case this exists for: `widgets.button` evaluates hover every frame,
    -- so at 60fps an ungated hover would fire sixty times a second.
    reset()
    local played = 0
    for _ = 1, 60 do
      if sound.allowed("hover") then played = played + 1 end
      sound.update(1 / 60)
    end
    t.ok(played <= 11, "a second of hovering should tick ~10 times, got " .. played)
    t.ok(played >= 9, "but it should still tick, got " .. played)
  end)

  t.case("a long frame does not release a burst", function()
    -- A hitch advances the clock a long way at once. Exactly one play should
    -- come out of it, not one per gap that elapsed.
    reset()
    sound.update(2.0)
    t.ok(sound.allowed("blip"), "the first after a hitch")
    t.ok(not sound.allowed("blip"), "and only the first")
  end)
end)

t.suite("sound / mute", function()
  t.case("nothing is allowed while muted", function()
    reset()
    sound.enabled = false
    for _, name in ipairs({ "blip", "press", "coin", "sent", "launch", "error" }) do
      t.ok(not sound.allowed(name), name .. " should be silent while muted")
    end
  end)

  t.case("muting does not consume the gap", function()
    -- A muted run must not leave every effect throttled the moment sound is
    -- turned back on.
    reset()
    sound.enabled = false
    sound.allowed("press")
    sound.enabled = true
    t.ok(sound.allowed("press"), "unmuting should not have to wait")
  end)

  t.case("play is a no-op with no audio device", function()
    -- There is no LÖVE here, so no pools were ever loaded. `play` has to
    -- return false rather than index a nil pool — the same path a checkout
    -- with no assets/sfx takes.
    reset()
    t.equal(sound.play("blip"), false, "play with no pool should report false")
  end)
end)

t.suite("sound / voices", function()
  t.case("a pool is taken in turn and wraps", function()
    -- The round-robin is what stops the second of two overlapping coins
    -- cutting the first one off. Simulated here because a real pool needs an
    -- audio device.
    local pool = { index = 0, voices = { "a", "b", "c", "d" } }
    local order = {}
    for _ = 1, 6 do
      pool.index = pool.index % #pool.voices + 1
      order[#order + 1] = pool.voices[pool.index]
    end
    t.equal(table.concat(order), "abcdab", "four voices, then round again")
  end)
end)
