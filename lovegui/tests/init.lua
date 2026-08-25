--- The GUI's test suite: `luajit tests/init.lua [name …]`.
---
--- Runs without LÖVE. That is not a trick — it is the payoff of keeping
--- `model.lua`, `ui/anim.lua` and the simulation half of `ui/particles.lua`
--- free of `love.` calls. What is left untested here is drawing, which a test
--- could only check by comparing pixels, and which `CWB_SHOT` covers instead by
--- letting a frame be reviewed from the terminal.
---
--- The runner and the wallet scaffolding are `luacli`'s, because a second copy
--- of either is a second copy to keep in step.

local here = debug.getinfo(1, "S").source:match("^@(.*)/[^/]*$") or "."
package.path = table.concat({
  here .. "/../?.lua",
  here .. "/../?/init.lua",
  here .. "/../../luacli/?.lua",
  here .. "/../../luacli/?/init.lua",
  package.path,
}, ";")

local t = require("tests.runner")
local support = require("tests.support")

local SUITES = {
  "anim", "particles", "sound", "theme", "layout", "card", "launch", "export", "login",
  "boot", "model",
}

local wanted = {}
for i = 1, #arg do wanted[arg[i]] = true end
if next(wanted) == nil then
  for _, name in ipairs(SUITES) do wanted[name] = true end
end

io.write("\n  Causewaybay Wallet — GUI tests\n\n")

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
support.cleanup()

if ran == 0 then
  io.write("  nothing to run\n")
  os.exit(2)
end
os.exit(status)
