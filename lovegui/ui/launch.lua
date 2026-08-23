--- The rocket's state machine: how long it flies and when it is over.
---
--- Pulled out of `main.lua` because it had a bug that only a test would have
--- caught, and `main.lua` cannot be tested — it needs a window. The bug was
--- that the launch ended on exactly one condition, an outcome arriving:
---
---     if state.time >= FLOOR and state.held then ... end
---
--- So when nothing came back the launch never ended. The screen shook forever
--- — the thrust term re-pinned it every frame, overwriting the decay — the
--- exhaust kept burning at a rocket that had left, and SEND stayed disabled
--- for the rest of the session. A user-interface animation must not depend on
--- a network reply to stop.
---
--- Nothing here touches `love.*` or the model. It takes `dt` and one boolean.
---
--- ## The two clocks
---
--- The **flight** is 1.25 seconds and always the same length. A node can
--- answer in 80ms, and an effect over before the eye finds it may as well not
--- have happened, so the rocket always gets its moment.
---
--- The **wait** is however long the wallet takes, and it is not an animation.
--- Once the flight is done the screen is calm whether or not an answer has
--- arrived. Holding the outcome is all that is left, and holding it costs
--- nothing to look at.

local anim = require("ui.anim")

local launch = {}

--- How long the rocket flies, and the earliest an outcome may be announced.
launch.FLOOR = 1.25

function launch.new()
  return { time = 0, held = nil }
end

--- Keep an outcome back until the rocket has had its moment.
---
--- The transfer already happened; this only decides when it is told.
function launch.hold(state, events)
  if not state or #events == 0 then return false end
  state.held = state.held or {}
  for _, event in ipairs(events) do
    state.held[#state.held + 1] = event
  end
  return true
end

--- How far up, and how hard it is burning.
---
--- `expo_in` is the whole character of it: almost nothing for the first third,
--- then it is gone. A linear rise reads as an elevator; this reads as thrust.
--- `thrust` is the raw progress, which is what the exhaust and the shake scale
--- with, and it caps at 1 — past that the rocket is off the screen.
function launch.flight(state)
  if not state then return 0, 0 end
  local thrust = math.min(1, state.time / launch.FLOOR)
  return anim.expo_in(thrust), thrust
end

--- True while the rocket is still on screen and still burning.
---
--- Everything loud is gated on this rather than on the launch existing at all,
--- which is the other half of the fix: a launch that lingers waiting for a
--- slow node must not go on shaking the screen.
function launch.flying(state)
  return state ~= nil and state.time < launch.FLOOR
end

--- Advance the clock. Returns the events to celebrate now, or nil to carry on.
---
--- An empty table means the launch is over with nothing to announce — which is
--- a real outcome, not an error: the flight finished and the wallet is no
--- longer working on anything, so there is nothing left to wait for. Anything
--- that arrives after this is announced when it arrives.
function launch.step(state, dt, busy)
  state.time = state.time + dt
  if state.time < launch.FLOOR then return nil end

  if state.held then return state.held end
  if not busy then return {} end

  -- Still working. Stay only to hold whatever comes back.
  return nil
end

return launch
