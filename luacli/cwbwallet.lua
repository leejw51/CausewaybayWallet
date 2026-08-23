#!/usr/bin/env luajit
--- Entry point for the Lua CLI: `luajit cwbwallet.lua account list`.
---
--- Everything it does is in `causewaybay.cli`; this file only makes the
--- modules next to it findable and turns the returned status into an exit
--- code, so the same logic can be tested without a process boundary.

-- The package path is set from this script's own location rather than from
-- the working directory, so `cwbwallet.lua` works from anywhere.
local here = debug.getinfo(1, "S").source:match("^@(.*)/[^/]*$") or "."
package.path = here .. "/?.lua;" .. here .. "/?/init.lua;" .. package.path

local cli = require("causewaybay.cli")

local argv = {}
for i = 1, #arg do argv[i] = arg[i] end

os.exit(cli.run(argv))
