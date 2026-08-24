--- Tests for the boot sequence.
---
--- `boot.lua` opens with a claim: *everything on this screen is true*. The ABI
--- number is what the library returned, the version is the library's, the
--- network and the wallet count come from `info`, and only the two memory
--- figures are invented.
---
--- That is a claim worth a test. A boot screen is decoration right up until
--- somebody reads it to find out which network they are on, and the failure
--- mode of a decorative one is a lie printed in a font that makes it look
--- authoritative. Nothing else in the program would notice if these lines
--- quietly stopped matching the wallet behind them.
---
--- `boot:draw` needs a window; the sequence, the typing and the handover are
--- all above it and run headlessly.

local t = require("tests.runner")
local support = require("tests.support")
local Boot = require("boot")

--- The whole screen as one string, for asking whether something is on it.
local function text_of(boot)
  local lines = {}
  for _, line in ipairs(boot.lines) do lines[#lines + 1] = line.text end
  return table.concat(lines, "\n")
end

--- Run the sequence to the end, the way a person who waits would see it.
local function play(boot, seconds)
  for _ = 1, math.floor((seconds or 6) / (1 / 60)) do boot:update(1 / 60) end
  return boot
end

t.suite("boot / everything on it is true", function()
  t.case("the numbers are the wallet's own", function()
    local wallet = support.wallet()
    local boot = Boot.new(wallet, nil)
    local screen = text_of(boot)

    -- Read back out of the library rather than hard-coded here: a fixture
    -- would only prove this file and boot.lua agree with each other.
    local described = wallet:describe()
    local info = wallet:info()

    t.contains(screen, tostring(described.abi), "the ABI the library reports")
    t.contains(screen, tostring(wallet:version()), "the version the library reports")
    t.contains(screen, tostring(info.network), "the network the wallet is on")
    t.contains(screen, tostring(info.chain_id), "and its chain id")
  end)

  t.case("the wallet count is the real one", function()
    -- Matched rather than compared against a hard-coded string: the column
    -- widths are layout, and a test that breaks when a column moves is a test
    -- that gets deleted.
    local function counted(wallet)
      return tonumber(text_of(Boot.new(wallet, nil)):match("WALLETS%s+(%d+)"))
    end

    local wallet = support.wallet()
    t.equal(counted(wallet), 0, "an empty store should say zero")

    wallet:new_account({ label = "one" })
    wallet:new_account({ label = "two" })
    t.equal(counted(wallet), 2, "and two wallets should say two")
  end)

  t.case("it ends at the Ok prompt", function()
    local boot = Boot.new(support.wallet(), nil)
    t.equal(boot.lines[#boot.lines].text, "Ok",
      "the last line of an MSX coming up is the prompt")
  end)

  t.case("the invented figures are the two the comment admits to", function()
    -- 65536 and 16384 are an MSX1's RAM and VRAM, and they are the only
    -- numbers on the screen that are not the wallet's. If a third invented
    -- figure ever appears it should be a deliberate act, not a slip.
    local boot = Boot.new(support.wallet(), nil)
    local counted = 0
    for _, line in ipairs(boot.lines) do
      if line.kind then counted = counted + 1 end
    end
    t.equal(counted, 2, "exactly two animated counters")
  end)
end)

t.suite("boot / when the library did not load", function()
  t.case("it says so and halts rather than handing over", function()
    local failure = { code = "io_error", message = "libcausewaybay_ffi.dylib not found" }
    local boot = Boot.new(nil, failure)

    t.equal(boot.halted, true, "a boot with no wallet must halt")
    t.contains(text_of(boot), "NOT FOUND", "and say what went wrong")
    t.contains(text_of(boot), "SYSTEM HALTED")
  end)

  t.case("the failure message reaches the screen", function()
    local boot = Boot.new(nil, { code = "io_error", message = "no such file anywhere" })
    t.contains(text_of(boot), "no such file",
      "the reason has to be readable without opening a log")
  end)

  t.case("a failure with no message still builds a screen", function()
    -- The error comes from a library that failed to load, which is exactly
    -- the moment to not also crash on a nil field.
    local boot = Boot.new(nil, nil)
    t.equal(boot.halted, true)
    t.contains(text_of(boot), "unknown")
  end)

  t.case("a halted screen can never be dismissed", function()
    local boot = Boot.new(nil, { message = "gone" })
    boot:skip()
    boot:skip()
    play(boot, 8)
    t.equal(boot.finished, false, "skipping a halted boot must do nothing")
    t.equal(boot:complete(), false, "and it must never hand over to a dead UI")
  end)
end)

t.suite("boot / the sequence", function()
  t.case("the tube warms up before any text", function()
    local boot = Boot.new(support.wallet(), nil)
    boot:update(0.2)
    t.equal(boot:visible(), 0, "nothing should be on screen during the warm-up")
  end)

  t.case("lines arrive one at a time and then it is ready", function()
    local boot = Boot.new(support.wallet(), nil)
    play(boot, 1.2)
    local part_way = boot:visible()
    t.ok(part_way > 0, "something should have appeared by now")

    play(boot, 6)
    t.equal(boot.done, true, "the sequence should have finished")
    t.equal(boot.ready, true, "and the prompt should be up")
    t.equal(boot:visible(), #boot.lines, "with every line shown")
  end)

  t.case("it does not hand over on its own", function()
    -- Waiting is a state, not a countdown: the machine sits at `Ok` until a
    -- key is pressed, which is the whole reference.
    local boot = Boot.new(support.wallet(), nil)
    play(boot, 20)
    t.equal(boot.finished, false, "no key was pressed, so nothing should have happened")
    t.equal(boot:complete(), false)
  end)
end)

t.suite("boot / replaying it", function()
  -- `0` on the title screen plays the whole thing again from black, so the
  -- intro can be recorded without restarting the process.

  t.case("nothing happens while the black runs", function()
    local boot = Boot.new(support.wallet(), nil, Boot.REPLAY_HOLD)
    play(boot, Boot.REPLAY_HOLD - 0.5)
    t.equal(boot:visible(), 0, "no text during the hold")
    t.equal(boot.done, false)
    t.ok(boot.hold > 0, "and it is still holding")
  end)

  t.case("the sequence runs once the black is over", function()
    local boot = Boot.new(support.wallet(), nil, Boot.REPLAY_HOLD)
    play(boot, Boot.REPLAY_HOLD + 6)
    t.ok(boot.hold <= 0, "the hold is spent")
    t.equal(boot.done, true, "and the sequence played")
    t.equal(boot:visible(), #boot.lines)
  end)

  t.case("a key during the black does not cut the take short", function()
    -- The hold exists to be recorded. A stray press must not skip it, or the
    -- recording starts mid-warm-up.
    local boot = Boot.new(support.wallet(), nil, Boot.REPLAY_HOLD)
    play(boot, 0.5)
    boot:skip()
    boot:skip()
    t.equal(boot.done, false, "skipping during the hold does nothing")
    t.equal(boot.finished, false)
    t.equal(boot:complete(), false, "and it certainly cannot hand over")
  end)

  t.case("it replays the same screen it showed the first time", function()
    local wallet = support.wallet()
    local first = play(Boot.new(wallet, nil), 8)
    local again = play(Boot.new(wallet, nil, Boot.REPLAY_HOLD), Boot.REPLAY_HOLD + 8)
    t.equal(text_of(again), text_of(first), "a replay is the same sequence")
  end)

  t.case("the boot at startup does not hold", function()
    -- Those frames are spent opening the wallet; there is nothing to wait for
    -- and a three second black at every launch would be a bug.
    local boot = Boot.new(support.wallet(), nil)
    t.equal(boot.hold, 0)
    play(boot, 0.6)
    t.ok(boot:visible() > 0, "it should be underway already")
  end)
end)

t.suite("boot / skipping", function()
  t.case("the first key finishes the sequence, the second hands over", function()
    -- Two presses, not one: somebody hammering the keyboard during the
    -- animation should not skip straight past the title card.
    local boot = Boot.new(support.wallet(), nil)
    boot:update(0.6)

    boot:skip()
    t.equal(boot.done, true, "the first skip finishes the text")
    t.equal(boot.ready, true)
    t.equal(boot.finished, false, "but does not hand over")

    boot:skip()
    t.equal(boot.finished, true, "the second does")
  end)

  t.case("the handover takes time and completes", function()
    local boot = Boot.new(support.wallet(), nil)
    boot:update(0.6)
    boot:skip()
    boot:skip()

    t.equal(boot:complete(), false, "not the instant the key is pressed")
    play(boot, 1.5)
    t.equal(boot:complete(), true, "but shortly after")
  end)

  t.case("the tube is not lit through the black or the flash", function()
    -- The window-mode button is drawn over this screen and gated on this: it
    -- must not appear during either, because both are a machine coming up and
    -- a button floating in the flash says they are a picture of one.
    local held = Boot.new(support.wallet(), nil, 1.0)
    t.equal(held:lit(), false, "not during the black hold")
    held:update(1.1)
    t.equal(held:lit(), false, "nor during the power-on flash")
    held:update(0.6)
    t.equal(held:lit(), true, "lit once the screen is a screen")
  end)

  t.case("a skipped boot shows the same lines as a watched one", function()
    -- The shortcut must not be a different screen. Somebody who skips has
    -- still been told which network they are on.
    local wallet = support.wallet()
    local watched = play(Boot.new(wallet, nil), 8)

    local skipped = Boot.new(wallet, nil)
    skipped:update(0.6)
    skipped:skip()

    t.equal(skipped:visible(), watched:visible())
    t.equal(text_of(skipped), text_of(watched))
  end)
end)
