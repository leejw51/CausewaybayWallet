--- Motion: easing curves, exponential smoothing, and damped springs.
---
--- Three different tools, and most of the polish is in picking the right one.
---
--- **Easing** is for motion with a known start, end and duration — a panel
--- sliding in, a flash fading out. You drive `t` from 0 to 1 and the curve
--- decides the shape.
---
--- **Smoothing** is for chasing a target that keeps moving — a selection
--- highlight following the cursor, a value counting up to a balance that just
--- arrived. There is no duration because there is no end.
---
--- **Springs** are for motion that should feel like it has mass — a card
--- landing, a button pressing. A spring overshoots and settles, which reads as
--- physical in a way an ease-out never does.
---
--- ## The thing that makes it frame-rate independent
---
--- The naive smoothing everyone writes first is
---
---     x = x + (target - x) * 0.1
---
--- which is wrong: it moves a tenth of the way *per frame*, so it converges
--- twice as fast at 120fps as at 60. The fix is to make the rate a decay
--- constant per *second* and integrate it properly:
---
---     x = target + (x - target) * math.exp(-rate * dt)
---
--- which is the closed form of exponential decay and gives the identical curve
--- at any frame rate, including a frame that took 200ms because the window was
--- dragged. Everything in this file that decays uses that form. It is the whole
--- reason the animation does not go strange when the wallet blocks for a moment.

local anim = {}

local exp, sin, cos, pi, sqrt = math.exp, math.sin, math.cos, math.pi, math.sqrt

-- ------------------------------------------------------------------- easing
--
-- Each takes `t` in 0..1 and returns the eased position. Named for what they
-- do at the ends: `out` decelerates into the finish, `in` accelerates out of
-- the start, `in_out` does both.

function anim.linear(t) return t end

--- Gentle, symmetric. The default when nothing louder is wanted.
function anim.smoothstep(t) return t * t * (3 - 2 * t) end

--- Sharper than smoothstep, with zero acceleration at both ends too.
function anim.smootherstep(t) return t * t * t * (t * (t * 6 - 15) + 10) end

function anim.quad_out(t) return 1 - (1 - t) * (1 - t) end
function anim.cubic_out(t) local f = 1 - t return 1 - f * f * f end
function anim.cubic_in_out(t)
  if t < 0.5 then return 4 * t * t * t end
  local f = -2 * t + 2
  return 1 - f * f * f / 2
end

--- Very fast, then a long glide. The most useful "arrives with authority"
--- curve, and what most of the UI here opens with.
function anim.expo_out(t)
  if t >= 1 then return 1 end
  return 1 - 2 ^ (-10 * t)
end

function anim.expo_in(t)
  if t <= 0 then return 0 end
  return 2 ^ (10 * (t - 1))
end

function anim.expo_in_out(t)
  if t <= 0 then return 0 end
  if t >= 1 then return 1 end
  if t < 0.5 then return 2 ^ (20 * t - 10) / 2 end
  return (2 - 2 ^ (-20 * t + 10)) / 2
end

--- Pulls back before it goes, and overshoots on arrival. Use sparingly — it is
--- charming once per screen and exhausting on every element.
local BACK = 1.70158

function anim.back_out(t)
  local c = BACK + 1
  local f = t - 1
  return 1 + c * f * f * f + BACK * f * f
end

function anim.back_in(t)
  local c = BACK + 1
  return c * t * t * t - BACK * t * t
end

--- Overshoots and wobbles to rest. For "that worked!" moments only.
function anim.elastic_out(t)
  if t <= 0 then return 0 end
  if t >= 1 then return 1 end
  local period = (2 * pi) / 3
  return 2 ^ (-10 * t) * sin((t * 10 - 0.75) * period) + 1
end

--- Drops and bounces. Reads as playful; a wallet uses it for coins, not
--- for anything a person has to read.
function anim.bounce_out(t)
  local n, d = 7.5625, 2.75
  if t < 1 / d then return n * t * t end
  if t < 2 / d then t = t - 1.5 / d return n * t * t + 0.75 end
  if t < 2.5 / d then t = t - 2.25 / d return n * t * t + 0.9375 end
  t = t - 2.625 / d
  return n * t * t + 0.984375
end

--- Run an easing curve backwards, turning any `_out` into its `_in`.
function anim.reverse(ease)
  return function(t) return 1 - ease(1 - t) end
end

-- -------------------------------------------------------------- smoothing
--
-- No duration, no end. A value that chases another.

--- Move `current` toward `target`, frame-rate independently.
---
--- `rate` is how sharply it converges, in units of e-folds per second: 1 is a
--- lazy drift, 10 is snappy, 30 is nearly instant but still takes the edge off
--- a jump. See the module comment for why this is not `+= diff * 0.1`.
function anim.approach(current, target, rate, dt)
  return target + (current - target) * exp(-rate * dt)
end

--- `approach` for a whole table of numbers, in place. Handy for colours.
function anim.approach_all(current, target, rate, dt)
  local factor = exp(-rate * dt)
  for i = 1, #current do
    current[i] = target[i] + (current[i] - target[i]) * factor
  end
  return current
end

--- Angle-aware smoothing, so a wrap from 359° to 1° takes the short way.
function anim.approach_angle(current, target, rate, dt)
  local difference = (target - current + pi) % (2 * pi) - pi
  return current + difference * (1 - exp(-rate * dt))
end

-- ---------------------------------------------------------------- springs

local Spring = {}
Spring.__index = Spring
anim.Spring = Spring

--- A damped harmonic oscillator: position, velocity, and a target to chase.
---
--- `stiffness` is how hard it pulls (higher is faster and tighter). `damping`
--- is the ratio, not a coefficient:
---
---   * `1.0` — critically damped: the fastest arrival with no overshoot at all.
---     The right default for anything a person reads, because overshooting text
---     is just wobble.
---   * `0.5`–`0.8` — a visible overshoot that settles quickly. What makes a
---     card feel like it has weight.
---   * `< 0.4` — springy and cartoonish. Fun for a coin, wrong for a balance.
---
--- Integrated semi-implicitly, which is stable for the stiffness values a UI
--- uses even when a frame runs long. A frame is also clamped below, because a
--- 400ms hitch integrated in one step is how springs explode.
function Spring.new(value, stiffness, damping)
  return setmetatable({
    value = value or 0,
    target = value or 0,
    velocity = 0,
    stiffness = stiffness or 180,
    damping = damping or 1.0,
  }, Spring)
end

--- The longest step the integrator will take at once. Anything larger is cut
--- into several, so a stall makes the animation slow rather than divergent.
local MAX_STEP = 1 / 120

function Spring:update(dt)
  -- Critical damping is 2*sqrt(k) for unit mass; the ratio scales it.
  local c = 2 * sqrt(self.stiffness) * self.damping
  local remaining = dt
  while remaining > 0 do
    local step = remaining > MAX_STEP and MAX_STEP or remaining
    local acceleration = (self.target - self.value) * self.stiffness - self.velocity * c
    self.velocity = self.velocity + acceleration * step
    self.value = self.value + self.velocity * step
    remaining = remaining - step
  end
  return self.value
end

--- Aim somewhere new without disturbing the motion already underway.
function Spring:to(target)
  self.target = target
  return self
end

--- Jump there, stopping dead. For a screen change, where easing from the old
--- value would look like a mistake rather than a transition.
function Spring:set(value)
  self.value, self.target, self.velocity = value, value, 0
  return self
end

--- Shove it, leaving the target alone. This is how a spring is used as an
--- impulse: nudge the velocity and let the damping do the rest.
function Spring:nudge(velocity)
  self.velocity = self.velocity + velocity
  return self
end

--- True once it has stopped meaningfully moving.
function Spring:at_rest(epsilon)
  epsilon = epsilon or 0.001
  return math.abs(self.value - self.target) < epsilon
    and math.abs(self.velocity) < epsilon
end

-- ----------------------------------------------------------------- tweens

local Tween = {}
Tween.__index = Tween
anim.Tween = Tween

--- A value moving from `from` to `to` over `duration`, along `ease`.
---
--- Unlike a spring this has an end, which is what `done` is for — a caller can
--- fire something once when it arrives.
function Tween.new(from, to, duration, ease)
  return setmetatable({
    from = from,
    to = to,
    duration = duration,
    ease = ease or anim.expo_out,
    elapsed = 0,
    value = from,
    done = false,
  }, Tween)
end

function Tween:update(dt)
  if self.done then return self.value end
  self.elapsed = self.elapsed + dt
  local t = self.duration > 0 and (self.elapsed / self.duration) or 1
  if t >= 1 then
    t, self.done = 1, true
  end
  self.value = self.from + (self.to - self.from) * self.ease(t)
  return self.value
end

function Tween:restart(from, to)
  self.from = from or self.from
  self.to = to or self.to
  self.elapsed, self.done = 0, false
  self.value = self.from
  return self
end

-- -------------------------------------------------------------- shorthands

--- A 0..1 ramp that runs once and stays at 1. For entrances.
function anim.ramp(duration, ease)
  return Tween.new(0, 1, duration, ease)
end

--- A value that decays to zero — the shape of a flash, a shake, a hit.
---
--- Returns a function of `dt` that yields the current amount, so a caller can
--- keep one line of state instead of a table.
function anim.decay(amount, rate)
  local value = amount
  return function(dt)
    value = value * exp(-rate * dt)
    return value
  end
end

--- A sine that oscillates between `low` and `high`, for idle motion.
function anim.pulse(time, period, low, high)
  local phase = (time % period) / period
  local wave = (sin(phase * 2 * pi) + 1) / 2
  return low + (high - low) * wave
end

--- Shake offset that decays, for errors. Uses two frequencies so it reads as a
--- rattle rather than a clean sine.
--- Below this a shake cannot move anything, so it is over.
---
--- The offset is applied in whole pixels, and any amplitude under half a pixel
--- rounds to zero. Saying so here is what makes *settled* mean settled:
--- exponential decay never reaches zero, so `amount > 0` was still true at
--- 1e-39 and the screen was still being asked to move.
local SETTLED = 0.5

function anim.shake(time, amount)
  if not amount or amount < SETTLED then return 0, 0 end
  return sin(time * 47) * amount, cos(time * 31) * amount * 0.6
end

--- The shake in whole pixels, which is how it is actually applied.
---
--- Rounded, not floored, and that is the whole point of this function
--- existing. `math.floor` sends every negative value to at least -1, however
--- small — so a shake decayed to 1e-39, oscillating either side of zero, was
--- translating the entire screen between 0 and -1 pixels forever. It read as a
--- faint permanent tremble, and it outlived every animation that caused it.
---
--- Flooring is wrong for a symmetric offset even while the shake is real: it
--- biases the whole screen half a pixel left and up for the duration.
---
--- Both call sites go through here so there is one place to be right.
function anim.shake_offset(time, amount)
  local x, y = anim.shake(time, amount)
  return math.floor(x + 0.5), math.floor(y + 0.5)
end

return anim
