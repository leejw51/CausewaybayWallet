--- Tests for the rocket's state machine.
---
--- This file exists because of a bug that shipped. The launch ended on exactly
--- one condition — an outcome arriving — so when nothing came back it never
--- ended: the screen shook forever, because the thrust term re-pinned the
--- shake every frame and overwrote its decay; the exhaust went on burning at a
--- rocket that had left; and SEND stayed disabled for the rest of the session.
---
--- It lived in `main.lua`, which needs a window and so is never tested. Moving
--- it into `ui/launch.lua` is most of the fix: the state machine takes `dt` and
--- one boolean and can be driven to any state in a loop.
---
--- The property underneath all of these: **an animation must not depend on a
--- network reply to stop.**

local t = require("tests.runner")
local Launch = require("ui.launch")

--- Run the clock, the way frames would.
local function play(state, seconds, busy)
  local finished
  for _ = 1, math.floor(seconds / (1 / 60)) do
    finished = finished or Launch.step(state, 1 / 60, busy or false)
  end
  return finished
end

t.suite("launch / it always ends", function()
  t.case("it ends when nothing ever comes back", function()
    -- The bug, stated directly. Nothing was ever held and the wallet is not
    -- working on anything, so there is nothing to wait for.
    local state = Launch.new()
    local finished = play(state, 4, false)
    t.ok(finished ~= nil, "it must end on its own")
    t.equal(#finished, 0, "with nothing to announce")
  end)

  t.case("it stops being loud the moment the flight is over", function()
    -- The other half. Even while it lingers waiting for a slow node, the
    -- screen has to be calm: `flying` is what the shake and the exhaust are
    -- gated on, and it follows the flight, not the launch.
    local state = Launch.new()
    play(state, Launch.FLOOR - 0.1, true)
    t.equal(Launch.flying(state), true, "still flying just before the floor")

    play(state, 0.3, true)
    t.equal(Launch.flying(state), false, "and done just after it")

    play(state, 30, true)
    t.equal(Launch.flying(state), false, "still done half a minute later")
  end)

  t.case("thrust never exceeds one, however long it waits", function()
    -- The shake is `thrust² × 5`. If thrust kept climbing the screen would not
    -- merely tremble, it would come apart.
    local state = Launch.new()
    play(state, 60, true)
    local _, thrust = Launch.flight(state)
    t.equal(thrust, 1, "thrust is capped, got " .. thrust)
  end)

  t.case("a launch waiting on the wallet does not end early", function()
    -- The flight being over is not the transfer being over. While the wallet
    -- is still working, the launch stays to catch the outcome.
    local state = Launch.new()
    local finished = play(state, 10, true)
    t.equal(finished, nil, "it should still be holding")
  end)

  t.case("and ends as soon as the wallet stops working", function()
    local state = Launch.new()
    play(state, 5, true)
    local finished = Launch.step(state, 1 / 60, false)
    t.ok(finished ~= nil, "once nothing is pending, it is over")
  end)
end)

t.suite("launch / the floor", function()
  t.case("an outcome that arrives early waits for the rocket", function()
    -- A node can answer in 80ms, and an effect over before the eye finds it
    -- may as well not have happened.
    local state = Launch.new()
    Launch.step(state, 0.08, true)
    Launch.hold(state, { "sent" })

    local finished = play(state, Launch.FLOOR - 0.2, true)
    t.equal(finished, nil, "not announced before the rocket has gone")

    finished = play(state, 0.4, true)
    t.ok(finished ~= nil, "and announced once it has")
    t.equal(finished[1], "sent")
  end)

  t.case("everything held is announced, in order", function()
    local state = Launch.new()
    Launch.hold(state, { "balance" })
    Launch.hold(state, { "sent", "selected" })

    local finished = play(state, Launch.FLOOR + 0.2, true)
    t.equal(table.concat(finished, ","), "balance,sent,selected")
  end)

  t.case("holding nothing is not holding", function()
    -- The caller uses the return value to decide whether to celebrate now, so
    -- an empty drain must not be mistaken for something kept back.
    local state = Launch.new()
    t.equal(Launch.hold(state, {}), false, "an empty list holds nothing")
    t.equal(state.held, nil)
  end)

  t.case("there is nothing to hold when nothing is launching", function()
    t.equal(Launch.hold(nil, { "sent" }), false,
      "with no launch the caller celebrates immediately")
  end)
end)

t.suite("launch / the flight", function()
  t.case("it starts on the pad and leaves", function()
    local state = Launch.new()
    local risen = Launch.flight(state)
    t.ok(math.abs(risen) < 0.001, "nothing has happened yet")

    play(state, Launch.FLOOR + 0.1, false)
    risen = Launch.flight(state)
    t.ok(math.abs(risen - 1) < 0.001, "and by the floor it is gone")
  end)

  t.case("it holds low and then goes", function()
    -- `expo_in`. A linear rise reads as an elevator.
    local state = Launch.new()
    play(state, Launch.FLOOR * 0.5, false)
    local risen = Launch.flight(state)
    t.ok(risen < 0.15,
      "half way through the time it should barely have moved, got " .. risen)
  end)

  t.case("nothing at all is flying when nothing is launching", function()
    local risen, thrust = Launch.flight(nil)
    t.equal(risen, 0)
    t.equal(thrust, 0)
    t.equal(Launch.flying(nil), false)
  end)
end)
