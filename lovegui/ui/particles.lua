--- Particles: coins, sparks, embers, confetti, and a starfield behind it all.
---
--- Hand-rolled rather than `love.graphics.newParticleSystem`, for one reason
--- that matters and one that is nice. The reason that matters: the effects here
--- need per-particle behaviour LÖVE's system does not express — coins that
--- *arc toward a target* and land, sparks that inherit a burst's direction,
--- confetti that tumbles on its own axis. The nice one: the whole thing is a
--- table of numbers with no LÖVE object in it, so `tests/particles_test.lua`
--- can run a burst for two seconds and assert it died.
---
--- Everything decays exponentially rather than linearly — see `anim.approach`
--- for why that is the frame-rate independent form. A particle that fades with
--- `alpha = alpha - dt` looks fine at 60fps and wrong everywhere else.

local anim = require("ui.anim")

local particles = {}

local random, cos, sin, pi, sqrt = math.random, math.cos, math.sin, math.pi, math.sqrt

local System = {}
System.__index = System
particles.System = System

--- Somewhere to put particles.
---
--- `limit` is a hard cap. A wallet that has been left open for an hour on a
--- screen that emits should not have accumulated ten thousand sparks, and a
--- cap is a more honest fix than hoping the emit rate is right.
function System.new(limit)
  return setmetatable({
    live = {},
    limit = limit or 512,
    time = 0,
  }, System)
end

function System:count()
  return #self.live
end

function System:clear()
  self.live = {}
end

--- Add one particle. Everything below is a wrapper around this.
---
--- The fields a particle may carry: position, velocity, acceleration, `life`
--- and `max_life`, `size`, `spin`, `angle`, colour `r,g,b`, `drag`, and
--- optionally `sprite` plus a `target` to home toward.
function System:add(p)
  if #self.live >= self.limit then
    -- Drop the oldest rather than the newest: the newest is the one the
    -- player just caused, and losing that is what looks broken.
    table.remove(self.live, 1)
  end
  p.life = p.life or 1
  p.max_life = p.life
  p.vx, p.vy = p.vx or 0, p.vy or 0
  p.ax, p.ay = p.ax or 0, p.ay or 0
  p.size = p.size or 3
  p.angle = p.angle or 0
  p.spin = p.spin or 0
  p.drag = p.drag or 0
  p.r, p.g, p.b = p.r or 1, p.g or 1, p.b or 1
  self.live[#self.live + 1] = p
  return p
end

function System:update(dt)
  self.time = self.time + dt
  local live = self.live
  local kept = 0

  for i = 1, #live do
    local p = live[i]
    p.life = p.life - dt

    if p.life > 0 then
      if p.target then
        -- Homing: steer toward the target with a spring-like pull, so a coin
        -- curves into the balance rather than sliding at it in a straight
        -- line. The pull strengthens as its life runs out, which guarantees
        -- it actually arrives instead of orbiting forever.
        local urgency = 1 - (p.life / p.max_life)
        local pull = p.pull or 900
        local dx, dy = p.target.x - p.x, p.target.y - p.y
        local distance = sqrt(dx * dx + dy * dy) + 0.0001
        p.vx = p.vx + (dx / distance) * pull * urgency * dt
        p.vy = p.vy + (dy / distance) * pull * urgency * dt
      end

      p.vx = p.vx + p.ax * dt
      p.vy = p.vy + p.ay * dt

      if p.drag > 0 then
        -- Exponential, so drag behaves the same at any frame rate.
        local keep = math.exp(-p.drag * dt)
        p.vx, p.vy = p.vx * keep, p.vy * keep
      end

      p.x = p.x + p.vx * dt
      p.y = p.y + p.vy * dt
      p.angle = p.angle + p.spin * dt

      kept = kept + 1
      live[kept] = p
    end
  end

  -- Compact in place rather than building a new table every frame.
  for i = #live, kept + 1, -1 do
    live[i] = nil
  end
end

--- How far through its life a particle is, 0 at birth and 1 at death.
local function age(p)
  return 1 - (p.life / p.max_life)
end

particles.age = age

--- Draw every particle. `sprites` maps a particle's `sprite` name to an Image.
---
--- Additive blending throughout: these are all light — sparks, glows, coins
--- catching it — and additive is what makes overlapping ones bloom instead of
--- flattening into a single opaque blob.
function System:draw(sprites)
  local previous = love.graphics.getBlendMode()
  love.graphics.setBlendMode("add")

  for i = 1, #self.live do
    local p = self.live[i]
    local t = age(p)
    -- Fade in fast, out slow: a particle that pops into existence at full
    -- brightness flickers, and one that fades linearly dies too abruptly.
    local fade = t < 0.1 and (t / 0.1) or (1 - (t - 0.1) / 0.9) ^ 1.6
    love.graphics.setColor(p.r, p.g, p.b, fade * (p.alpha or 1))

    local image = p.sprite and sprites and sprites[p.sprite]
    if image then
      local scale = (p.size * 2) / image:getWidth()
      love.graphics.draw(
        image, p.x, p.y, p.angle,
        scale * (p.squash or 1), scale,
        image:getWidth() / 2, image:getHeight() / 2
      )
    else
      -- No sprite: a square, because this is 8-bit and a circle would be
      -- the wrong shape for the rest of the screen.
      local s = p.size * (1 - t * 0.4)
      love.graphics.push()
      love.graphics.translate(p.x, p.y)
      love.graphics.rotate(p.angle)
      love.graphics.rectangle("fill", -s / 2, -s / 2, s, s)
      love.graphics.pop()
    end
  end

  love.graphics.setBlendMode(previous)
  love.graphics.setColor(1, 1, 1, 1)
end

-- ------------------------------------------------------------------ effects
--
-- Each returns the system, so calls chain.

--- A ring of sparks. The workhorse: a button press, a card landing, a hit.
function System:burst(x, y, options)
  options = options or {}
  local count = options.count or 18
  local speed = options.speed or 220
  local spread = options.spread or (2 * pi)
  local direction = options.direction or 0
  local colour = options.colour or { 1, 0.85, 0.3 }

  for _ = 1, count do
    local angle = direction + (random() - 0.5) * spread
    local magnitude = speed * (0.4 + random() * 0.8)
    self:add({
      x = x, y = y,
      vx = cos(angle) * magnitude,
      vy = sin(angle) * magnitude,
      ay = options.gravity or 0,
      drag = options.drag or 2.5,
      life = (options.life or 0.7) * (0.6 + random() * 0.8),
      size = options.size or (2 + random() * 3),
      spin = (random() - 0.5) * 12,
      sprite = options.sprite,
      r = colour[1], g = colour[2], b = colour[3],
    })
  end
  return self
end

--- Coins that fly out and home in on a target — the "money arrived" effect.
---
--- They launch upward and outward first, then the homing in `update` takes
--- over, so the path is an arc rather than a beeline. That arc is most of what
--- makes it feel good.
function System:coins(x, y, target, count)
  for i = 1, (count or 12) do
    local angle = -pi / 2 + (random() - 0.5) * 2.4
    local magnitude = 160 + random() * 220
    self:add({
      x = x + (random() - 0.5) * 24,
      y = y + (random() - 0.5) * 16,
      vx = cos(angle) * magnitude,
      vy = sin(angle) * magnitude,
      ay = 420,
      drag = 0.6,
      -- Staggered, so they arrive as a stream rather than all at once.
      life = 0.85 + i * 0.035 + random() * 0.2,
      size = 9 + random() * 5,
      spin = (random() - 0.5) * 9,
      sprite = "coin",
      target = target,
      pull = 1500,
    })
  end
  return self
end

--- A column of embers. Idle decoration that reads as heat.
---
--- `direction` is -1 for a rising column (smoke, a warming engine) and 1 for a
--- falling one (exhaust under something flying up). Getting that sign wrong is
--- the difference between a rocket and a bonfire.
function System:embers(x, y, width, count, direction)
  direction = direction or -1
  for _ = 1, (count or 2) do
    self:add({
      x = x + (random() - 0.5) * (width or 40),
      y = y,
      vx = (random() - 0.5) * 24,
      vy = (30 + random() * 50) * direction,
      -- Accelerating the way it is already going: exhaust is pushed, not
      -- dropped, so gravity would read as wrong here.
      ay = 12 * direction,
      drag = 0.4,
      life = 1.2 + random() * 1.4,
      size = 1.5 + random() * 2.5,
      r = 1, g = 0.55 + random() * 0.35, b = 0.15,
    })
  end
  return self
end

--- Confetti: heavier, tumbling, falls under gravity. For a completed send.
function System:confetti(x, y, count)
  local palette = {
    { 1, 0.30, 0.45 }, { 0.35, 0.95, 1 }, { 1, 0.85, 0.25 },
    { 0.55, 1, 0.45 }, { 0.75, 0.55, 1 },
  }
  for _ = 1, (count or 40) do
    local colour = palette[random(#palette)]
    local angle = -pi / 2 + (random() - 0.5) * 2.0
    local magnitude = 240 + random() * 320
    self:add({
      x = x, y = y,
      vx = cos(angle) * magnitude,
      vy = sin(angle) * magnitude,
      ay = 700,
      drag = 0.8,
      life = 1.4 + random() * 1.2,
      size = 3 + random() * 4,
      spin = (random() - 0.5) * 22,
      -- Squashed on one axis and spinning: reads as a flat scrap of paper
      -- tumbling, where a square would read as another spark.
      squash = 0.35,
      r = colour[1], g = colour[2], b = colour[3],
    })
  end
  return self
end

--- A brief trail behind something moving. Called every frame while it moves.
function System:trail(x, y, vx, vy, colour)
  self:add({
    x = x + (random() - 0.5) * 6,
    y = y + (random() - 0.5) * 6,
    vx = -vx * 0.15 + (random() - 0.5) * 30,
    vy = -vy * 0.15 + (random() - 0.5) * 30,
    drag = 3,
    life = 0.25 + random() * 0.25,
    size = 2 + random() * 2,
    r = colour and colour[1] or 0.4,
    g = colour and colour[2] or 0.9,
    b = colour and colour[3] or 1,
  })
  return self
end

-- --------------------------------------------------------------- starfield
--
-- Not a particle system: stars never die, so recycling them beats emitting.

local Stars = {}
Stars.__index = Stars
particles.Stars = Stars

--- A parallax starfield. Three depths, drifting at different speeds.
---
--- The depth also sets brightness and size, which is the whole trick — the
--- eye reads slow-dim-small as far away without being told.
function Stars.new(count, width, height)
  local self = setmetatable({ stars = {}, width = width, height = height }, Stars)
  for i = 1, (count or 90) do
    local depth = random()
    self.stars[i] = {
      x = random() * width,
      y = random() * height,
      depth = depth,
      speed = 6 + depth * 26,
      size = depth < 0.4 and 1 or (depth < 0.8 and 2 or 3),
      -- A phase per star, so they twinkle out of step with each other.
      phase = random() * 2 * pi,
    }
  end
  return self
end

function Stars:update(dt, drift)
  for i = 1, #self.stars do
    local star = self.stars[i]
    star.x = star.x - star.speed * dt * (drift or 1)
    if star.x < -4 then
      star.x = self.width + 4
      star.y = random() * self.height
    end
  end
end

function Stars:draw(time)
  for i = 1, #self.stars do
    local star = self.stars[i]
    local twinkle = 0.55 + 0.45 * sin(time * 2.2 + star.phase)
    love.graphics.setColor(0.55, 0.75, 1, (0.18 + star.depth * 0.5) * twinkle)
    love.graphics.rectangle("fill", star.x, star.y, star.size, star.size)
  end
  love.graphics.setColor(1, 1, 1, 1)
end

--- Resize with the window, keeping the stars spread across it.
function Stars:resize(width, height)
  for i = 1, #self.stars do
    local star = self.stars[i]
    star.x = star.x / self.width * width
    star.y = star.y / self.height * height
  end
  self.width, self.height = width, height
end

-- A convenience so callers do not each write the same two lines.
particles.decay = anim.decay

return particles
