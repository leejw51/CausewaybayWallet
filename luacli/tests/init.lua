--- The test entry point: `luajit tests/init.lua [name …]`.
---
--- Runs every suite in one process, so the shared library is loaded once and
--- the temp homes are cleaned up together. Naming files on the command line
--- runs only those — `luajit tests/init.lua json ffi`.

local here = debug.getinfo(1, "S").source:match("^@(.*)/[^/]*$") or "."
package.path = here .. "/../?.lua;" .. here .. "/../?/init.lua;" .. package.path

local t = require("tests.runner")
local support = require("tests.support")

-- Order matters only in that the cheapest, most fundamental things run first:
-- if the JSON codec is broken, every later failure is a symptom of it.
local SUITES = { "json", "echo", "ffi", "wallet", "cli", "interactive", "vectors" }

local wanted = {}
for i = 1, #arg do wanted[arg[i]] = true end
if next(wanted) == nil then
  for _, name in ipairs(SUITES) do wanted[name] = true end
end

io.write("\n  Causewaybay Wallet — Lua tests\n\n")

local ran = 0
for _, name in ipairs(SUITES) do
  if wanted[name] then
    ran = ran + 1
    require("tests." .. name .. "_test")
    wanted[name] = nil
  end
end

for name in pairs(wanted) do
  io.write("  no suite named '" .. name .. "'\n")
  os.exit(2)
end

local status = t.report()

-- Homes hold private keys, even throwaway ones. They go before the exit.
support.cleanup()

if ran == 0 then
  io.write("  nothing to run\n")
  os.exit(2)
end
os.exit(status)
