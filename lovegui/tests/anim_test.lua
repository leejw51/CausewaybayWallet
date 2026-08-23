--- Tests for the motion primitives.
---
--- The property that matters most is frame-rate independence: the same motion
--- integrated in one big step and in a hundred small ones must end up in the
--- same place. That is the whole reason these use `exp` rather than
--- multiplying by a per-frame constant, and it is the thing that silently
--- stops being true if someone "simplifies" it later.

local t = require("tests.runner")
local anim = require("ui.anim")

t.suite("anim / easing", function()
  t.case("every curve starts at 0 and ends at 1", function()
    local curves = {
      "linear", "smoothstep", "smootherstep", "quad_out", "cubic_out",
      "cubic_in_out", "expo_out", "expo_in", "expo_in_out", "back_out",
      "back_in", "elastic_out", "bounce_out",
    }
    for _, name in ipairs(curves) do
      local ease = anim[name]
      t.ok(math.abs(ease(0)) < 0.001, name .. "(0) should be 0, got " .. ease(0))
      t.ok(math.abs(ease(1) - 1) < 0.001, name .. "(1) should be 1, got " .. ease(1))
    end
  end)

  t.case("monotonic curves never go backwards", function()
    -- back_ and elastic_ deliberately overshoot, so they are not in this list.
    for _, name in ipairs({ "linear", "smoothstep", "expo_out", "cubic_in_out" }) do
      local previous = -1
      for i = 0, 100 do
        local value = anim[name](i / 100)
        t.ok(value >= previous - 0.0001, name .. " went backwards at " .. i)
        previous = value
      end
    end
  end)

  t.case("back_out really does overshoot", function()
    -- If it stops overshooting the curve has been flattened and the character
    -- of every entrance in the GUI has quietly changed.
    local peak = 0
    for i = 0, 100 do peak = math.max(peak, anim.back_out(i / 100)) end
    t.ok(peak > 1.02, "expected an overshoot, peaked at " .. peak)
  end)

  t.case("reverse turns an out curve into an in curve", function()
    local reversed = anim.reverse(anim.expo_out)
    t.ok(math.abs(reversed(0.5) - anim.expo_in(0.5)) < 0.02)
  end)
end)

t.suite("anim / smoothing", function()
  t.case("approach converges on the target", function()
    local value = 0
    for _ = 1, 200 do value = anim.approach(value, 10, 8, 1 / 60) end
    t.ok(math.abs(value - 10) < 0.01, "ended at " .. value)
  end)

  t.case("the same motion at any frame rate lands in the same place", function()
    -- The property the whole file exists for. One 0.5s step against 500
    -- 1ms steps: a per-frame multiply would be wildly apart here.
    local coarse = anim.approach(0, 100, 6, 0.5)
    local fine = 0
    for _ = 1, 500 do fine = anim.approach(fine, 100, 6, 0.001) end
    t.ok(math.abs(coarse - fine) < 0.5,
      ("one step gave %.3f, five hundred gave %.3f"):format(coarse, fine))
  end)

  t.case("a zero step changes nothing", function()
    t.equal(anim.approach(3, 99, 10, 0), 3)
  end)

  t.case("angles take the short way round", function()
    -- From just under a full turn to just over zero: the long way is 6.2
    -- radians and would spin the sprite the wrong way.
    local value = anim.approach_angle(6.2, 0.1, 10, 0.1)
    t.ok(value > 6.2 or value < 0.2, "went the long way, to " .. value)
  end)
end)

t.suite("anim / springs", function()
  t.case("a critically damped spring never overshoots", function()
    local spring = anim.Spring.new(0, 200, 1.0)
    spring:to(1)
    local peak = 0
    for _ = 1, 400 do peak = math.max(peak, spring:update(1 / 60)) end
    t.ok(peak <= 1.001, "critical damping overshot to " .. peak)
    t.ok(spring:at_rest(), "it should have settled")
  end)

  t.case("an underdamped one does overshoot, then settles", function()
    local spring = anim.Spring.new(0, 200, 0.35)
    spring:to(1)
    local peak = 0
    for _ = 1, 600 do peak = math.max(peak, spring:update(1 / 60)) end
    t.ok(peak > 1.05, "expected overshoot, peaked at " .. peak)
    t.ok(spring:at_rest(0.01), "it should still have settled")
  end)

  t.case("a long frame does not make it explode", function()
    -- The case that matters in practice: the wallet blocked, or the window
    -- was dragged, and one frame took a third of a second.
    local spring = anim.Spring.new(0, 400, 0.6)
    spring:to(1)
    for _ = 1, 20 do spring:update(0.33) end
    t.ok(spring.value == spring.value, "value went NaN")
    t.ok(math.abs(spring.value) < 100, "diverged to " .. spring.value)
  end)

  t.case("set jumps without motion, nudge moves without retargeting", function()
    local spring = anim.Spring.new(0, 200, 1)
    spring:set(5)
    t.equal(spring.value, 5)
    t.equal(spring.target, 5)
    t.equal(spring.velocity, 0)

    spring:nudge(10)
    t.equal(spring.target, 5, "a nudge must not move the target")
    t.ok(spring.velocity > 0)
  end)
end)

t.suite("anim / tweens", function()
  t.case("runs from start to finish and reports done", function()
    -- Stepped until it finishes rather than for an exact frame count: thirty
    -- steps of 1/60 sum to a hair under 0.5, and asserting on that is testing
    -- the arithmetic of the test rather than the behaviour of the tween.
    local tween = anim.Tween.new(0, 100, 0.5, anim.linear)
    t.equal(tween.done, false)
    local frames = 0
    while not tween.done and frames < 120 do
      tween:update(1 / 60)
      frames = frames + 1
    end
    t.equal(tween.done, true)
    t.equal(tween.value, 100)
    t.ok(frames <= 31, "took " .. frames .. " frames for a half-second tween")
  end)

  t.case("a zero duration finishes at once rather than dividing by zero", function()
    local tween = anim.Tween.new(0, 1, 0, anim.linear)
    t.equal(tween:update(1 / 60), 1)
    t.equal(tween.done, true)
  end)

  t.case("restart puts it back", function()
    local tween = anim.Tween.new(0, 1, 0.2, anim.linear)
    for _ = 1, 30 do tween:update(1 / 60) end
    tween:restart()
    t.equal(tween.done, false)
    t.equal(tween.value, 0)
  end)
end)

t.suite("anim / shorthands", function()
  t.case("decay falls toward zero", function()
    local amount = anim.decay(10, 5)
    local value
    for _ = 1, 120 do value = amount(1 / 60) end
    t.ok(value < 0.2, "still at " .. value)
  end)

  t.case("pulse stays between its bounds", function()
    for i = 0, 200 do
      local value = anim.pulse(i / 20, 1.5, 2, 8)
      t.ok(value >= 1.99 and value <= 8.01, "out of range: " .. value)
    end
  end)

  t.case("shake is nothing when the amount is nothing", function()
    local x, y = anim.shake(1.23, 0)
    t.equal(x, 0)
    t.equal(y, 0)
  end)
end)

return true
