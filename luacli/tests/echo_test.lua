--- Tests for turning terminal echo off.
---
--- These run without a terminal, which is the case that has to be safe: every
--- function must report honestly that it did nothing rather than pretend it
--- hid something. The masked-prompt behaviour itself is covered in
--- `interactive_test.lua`, where the reader can be substituted.

local t = require("tests.runner")
local echo = require("causewaybay.echo")

t.suite("echo", function()
  t.case("knows whether there is a terminal", function()
    -- Under `make test` stdin is whatever the runner had; either answer is
    -- correct, but it must be a boolean and it must not raise.
    t.equal(type(echo.is_tty()), "boolean")
    t.equal(echo.available(), echo.is_tty())
  end)

  t.case("does nothing, and says so, with no terminal", function()
    if echo.is_tty() then
      return t.skip("echo / no-terminal behaviour", "this run has a terminal")
    end
    t.equal(echo.off(), false)
    t.equal(echo.on(), false)
  end)

  t.case("still returns the line when it cannot hide it", function()
    local line = echo.read_hidden(function() return "abandon about" end)
    t.equal(line, "abandon about")
  end)

  t.case("passes end of input through", function()
    t.equal(echo.read_hidden(function() return nil end), nil)
  end)

  t.case("restores echo even when the read fails", function()
    -- The failure that matters: an error thrown while echo is off would leave
    -- the shell swallowing everything typed after the program exits.
    local restored = false
    local real_on = echo.on
    echo.on = function() restored = true; return real_on() end

    local ok = pcall(echo.read_hidden, function() error("reader blew up", 0) end)
    echo.on = real_on

    t.equal(ok, false, "the error should propagate")
    -- With no terminal, `off` was a no-op, so `on` is correctly skipped; with
    -- one, it must have been called.
    if echo.is_tty() then
      t.ok(restored, "echo must be restored before the error escapes")
    end
  end)
end)

return true
