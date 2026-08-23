--- A test harness in one file.
---
--- busted is the usual answer, but it means LuaRocks, and a wallet that needs
--- a package manager to run its own tests is a wallet nobody runs the tests
--- for. This is the subset that gets used: grouped cases, a handful of
--- assertions, and a non-zero exit when anything fails.
---
---     local t = require("tests.runner")
---     t.suite("units", function()
---       t.case("converts whole tokens", function()
---         t.equal(wallet:to_wei("1.5").value, "1500000000000000000")
---       end)
---     end)
---     os.exit(t.report())

local runner = {}

local state = {
  suite = nil,
  passed = 0,
  failures = {},
  -- Cases that could not run for a reason that is not a bug, e.g. no network.
  skipped = {},
}

local GREEN, RED, YELLOW, DIM, RESET = "\27[32m", "\27[31m", "\27[33m", "\27[2m", "\27[0m"
if os.getenv("NO_COLOR") or not os.getenv("TERM") then
  GREEN, RED, YELLOW, DIM, RESET = "", "", "", "", ""
end

--- Group a set of cases under a name.
function runner.suite(name, body)
  state.suite = name
  io.write(DIM, "  ", name, RESET, "\n")
  body()
  state.suite = nil
end

--- One test. A case that raises has failed; anything else has passed.
function runner.case(name, body)
  local ok, err = xpcall(body, function(e)
    -- The traceback is what turns "assertion failed" into something locatable.
    return tostring(e) .. "\n" .. debug.traceback("", 3)
  end)
  local label = (state.suite and (state.suite .. " / ") or "") .. name
  if ok then
    state.passed = state.passed + 1
    io.write("    ", GREEN, "ok", RESET, "   ", name, "\n")
  else
    state.failures[#state.failures + 1] = { name = label, err = err }
    io.write("    ", RED, "FAIL", RESET, " ", name, "\n")
  end
end

--- Record a case that was deliberately not run, with the reason.
function runner.skip(name, why)
  state.skipped[#state.skipped + 1] = { name = name, why = why }
  io.write("    ", YELLOW, "skip", RESET, " ", name, " ", DIM, "(", why, ")", RESET, "\n")
end

-- ----------------------------------------------------------------- assertions

--- Render a value for a failure message: tables are shown, not addressed.
local function show(value, depth)
  depth = depth or 0
  if type(value) ~= "table" or depth > 2 then return tostring(value) end
  local parts = {}
  local keys = {}
  for k in pairs(value) do keys[#keys + 1] = tostring(k) end
  table.sort(keys)
  for _, k in ipairs(keys) do
    local v = value[k] ~= nil and value[k] or value[tonumber(k)]
    parts[#parts + 1] = k .. "=" .. show(v, depth + 1)
  end
  return "{" .. table.concat(parts, ", ") .. "}"
end

runner.show = show

function runner.ok(value, message)
  if not value then
    error(message or ("expected a truthy value, got " .. show(value)), 2)
  end
end

function runner.equal(actual, expected, message)
  if actual ~= expected then
    error(
      (message and (message .. ": ") or "")
        .. "expected " .. show(expected)
        .. "\n         got      " .. show(actual),
      2
    )
  end
end

function runner.not_equal(actual, unexpected, message)
  if actual == unexpected then
    error((message and (message .. ": ") or "") .. "expected something other than " .. show(unexpected), 2)
  end
end

function runner.contains(haystack, needle, message)
  if type(haystack) ~= "string" or not haystack:find(needle, 1, true) then
    error(
      (message and (message .. ": ") or "")
        .. "expected " .. show(haystack) .. "\n         to contain " .. show(needle),
      2
    )
  end
end

--- Assert a call failed with a particular wallet error code.
---
--- The shape every wallet call has — `value, err` — so this reads the way the
--- call site does and says which of the two came back wrong.
function runner.fails_with(code, value, err)
  if value ~= nil then
    error("expected the call to fail with '" .. code .. "', but it returned " .. show(value), 2)
  end
  if not err then error("expected an error value", 2) end
  if err.code ~= code then
    error("expected code '" .. code .. "', got '" .. tostring(err.code) .. "': " .. tostring(err.message), 2)
  end
end

--- Assert that `body` raises, and that the message mentions `fragment`.
function runner.raises(fragment, body)
  local ok, err = pcall(body)
  if ok then error("expected an error mentioning " .. show(fragment), 2) end
  if fragment and not tostring(err):find(fragment, 1, true) then
    error("expected the error to mention " .. show(fragment) .. ", got: " .. tostring(err), 2)
  end
end

-- --------------------------------------------------------------------- report

--- Print the summary and return the exit status: 0 if everything passed.
function runner.report()
  io.write("\n")
  for _, failure in ipairs(state.failures) do
    io.write(RED, "FAIL", RESET, " ", failure.name, "\n", failure.err, "\n\n")
  end
  local total = state.passed + #state.failures
  if #state.failures == 0 then
    io.write(GREEN, ("  %d passed"):format(total), RESET)
  else
    io.write(RED, ("  %d of %d failed"):format(#state.failures, total), RESET)
  end
  if #state.skipped > 0 then
    io.write(YELLOW, (", %d skipped"):format(#state.skipped), RESET)
  end
  io.write("\n\n")
  return #state.failures == 0 and 0 or 1
end

--- Reset between files when several share a process.
function runner.reset()
  state.passed, state.failures, state.skipped = 0, {}, {}
end

return runner
