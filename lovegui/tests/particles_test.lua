--- Tests for the particle simulation.
---
--- Only the simulation: `System:draw` is the one function here that touches
--- LÖVE, and it is left to the screenshots. What these check is that particles
--- die, that the cap holds, and that homing actually arrives — the three ways
--- a particle system quietly becomes a memory leak or a disappointment.

local t = require("tests.runner")
local particles = require("ui.particles")

--- Run a system forward at a fixed rate.
local function simulate(system, seconds, dt)
  dt = dt or 1 / 60
  for _ = 1, math.floor(seconds / dt) do system:update(dt) end
  return system
end

t.suite("particles / lifetime", function()
  t.case("a burst dies out", function()
    -- The one that matters: a system that never empties is a leak with a
    -- pretty face on it.
    local fx = particles.System.new()
    fx:burst(100, 100, { count = 40, life = 0.5 })
    t.ok(fx:count() > 0, "should have emitted")
    simulate(fx, 3)
    t.equal(fx:count(), 0, "everything should have died")
  end)

  t.case("every effect eventually empties", function()
    for _, emit in ipairs({
      function(fx) fx:burst(10, 10, { count = 20 }) end,
      function(fx) fx:embers(10, 10, 20, 10) end,
      function(fx) fx:confetti(10, 10, 30) end,
      function(fx) fx:coins(10, 10, { x = 200, y = 50 }, 10) end,
      function(fx) fx:trail(10, 10, 5, 5) end,
    }) do
      local fx = particles.System.new()
      emit(fx)
      simulate(fx, 6)
      t.equal(fx:count(), 0)
    end
  end)

  t.case("the cap is a cap", function()
    local fx = particles.System.new(50)
    for _ = 1, 20 do fx:burst(0, 0, { count = 30, life = 10 }) end
    t.equal(fx:count(), 50, "600 emitted into a system capped at 50")
  end)

  t.case("clear empties it", function()
    local fx = particles.System.new()
    fx:confetti(0, 0, 20)
    fx:clear()
    t.equal(fx:count(), 0)
  end)
end)

t.suite("particles / motion", function()
  t.case("gravity pulls down", function()
    local fx = particles.System.new()
    local p = fx:add({ x = 0, y = 0, ay = 500, life = 5 })
    simulate(fx, 0.5)
    t.ok(p.y > 40, "fell only " .. p.y)
  end)

  t.case("drag slows things down", function()
    local fx = particles.System.new()
    local fast = fx:add({ x = 0, y = 0, vx = 400, drag = 0, life = 5 })
    local slow = fx:add({ x = 0, y = 0, vx = 400, drag = 6, life = 5 })
    simulate(fx, 0.5)
    t.ok(slow.x < fast.x * 0.6, "drag did little: " .. slow.x .. " vs " .. fast.x)
  end)

  t.case("coins arrive at their target", function()
    -- The effect is only satisfying if they land. A pull too weak and they
    -- drift off screen; the homing ramps with age precisely to prevent it.
    local target = { x = 300, y = 40 }
    local fx = particles.System.new()
    fx:coins(100, 250, target, 8)

    local closest = math.huge
    for _ = 1, 240 do
      fx:update(1 / 60)
      for _, p in ipairs(fx.live) do
        local dx, dy = p.x - target.x, p.y - target.y
        closest = math.min(closest, math.sqrt(dx * dx + dy * dy))
      end
    end
    t.ok(closest < 30, "nearest coin got only within " .. math.floor(closest))
  end)

  t.case("embers go the way they are told", function()
    local fx = particles.System.new()
    local up = fx:add({ x = 0, y = 0, life = 5 })
    fx:clear()

    fx:embers(0, 0, 0, 1, -1)
    local rising = fx.live[1]
    fx:clear()
    fx:embers(0, 0, 0, 1, 1)
    local falling = fx.live[1]

    t.ok(rising.vy < 0, "a rising ember should have negative vy")
    t.ok(falling.vy > 0, "a falling one should have positive vy")
    local _ = up
  end)

  t.case("age runs from 0 to 1", function()
    local fx = particles.System.new()
    local p = fx:add({ x = 0, y = 0, life = 1 })
    t.ok(particles.age(p) < 0.01)
    simulate(fx, 0.5)
    t.ok(math.abs(particles.age(p) - 0.5) < 0.05, "age was " .. particles.age(p))
  end)
end)

t.suite("particles / starfield", function()
  t.case("stars wrap rather than leaving", function()
    local stars = particles.Stars.new(30, 480, 270)
    for _ = 1, 600 do stars:update(1 / 60, 1) end
    for _, star in ipairs(stars.stars) do
      t.ok(star.x >= -4 and star.x <= 484, "a star escaped to " .. star.x)
    end
  end)

  t.case("resizing keeps them in the field", function()
    local stars = particles.Stars.new(20, 480, 270)
    stars:resize(960, 540)
    for _, star in ipairs(stars.stars) do
      t.ok(star.x >= 0 and star.x <= 960 and star.y >= 0 and star.y <= 540)
    end
  end)
end)

return true
