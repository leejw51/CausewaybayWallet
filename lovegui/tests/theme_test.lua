--- The palette decisions that are data rather than drawing.
---
--- Drawing itself is out of scope here — a test could only check it by
--- comparing pixels, and `CWB_SHOT` covers that instead. What *is* checkable is
--- the mapping a drawing call reads: which colour a chain gets, and that every
--- chain gets one.

local t = require("tests.runner")
local theme = require("ui.theme")

t.suite("theme / chain colours", function()
  t.case("every chain has a colour of its own", function()
    local seen = {}
    for _, chain in ipairs({ "evm", "solana", "cardano", "midnight" }) do
      local colour = theme.chain_colour(chain)
      t.ok(colour, chain .. " has no colour")
      t.equal(#colour, 3, chain .. "'s colour is not an RGB triple")
      local key = table.concat(colour, ",")
      t.equal(seen[key], nil, chain .. " shares a colour with " .. tostring(seen[key]))
      seen[key] = chain
    end
  end)

  t.case("a chain this build does not know still draws", function()
    -- A chain added in Rust reaches the GUI through the library before this
    -- list catches up. It must render in *some* colour rather than indexing
    -- nil into a drawing call.
    local colour = theme.chain_colour("a-chain-from-the-future")
    t.ok(colour and #colour == 3)
    t.equal(colour, theme.chain_colour("evm"), "it falls back to the default")
  end)
end)

return true
