--- Tests for the card design.
---
--- The property everything rests on is that a card is a *face*: the same
--- address must produce the same card on every machine, in every run, forever.
--- A face that changed would be worse than no face, because a person would
--- have learned to trust it first.
---
--- Drawing is not tested here — `make shots` covers that by making a frame
--- reviewable — but `card.design` and `card.number` touch no `love.*` call and
--- are pure functions of a string, which is exactly the half worth asserting.

local t = require("tests.runner")
local card = require("ui.card")

--- Twenty bytes of hex, from a number. Enough distinct addresses to say
--- something about the distribution without shipping a fixture file.
local function address(n)
  local hex = {}
  for i = 0, 19 do
    hex[#hex + 1] = ("%02x"):format((n * 61 + i * 37 + i * i * 13) % 256)
  end
  return "0x" .. table.concat(hex)
end

local REAL = "0xCb2134f9F5e0f1B0d0F0e5b8dA2b3c4d5E6f7081"

t.suite("card / determinism", function()
  t.case("the same address always deals the same card", function()
    local a = card.design(REAL)
    local b = card.design(REAL)
    t.equal(a.scheme.name, b.scheme.name, "scheme")
    t.equal(a.pattern, b.pattern, "pattern")
    t.equal(a.emblem, b.emblem, "emblem")
    t.equal(a.tier.name, b.tier.name, "tier")
    t.equal(a.member, b.member, "member number")
    for row = 1, 5 do
      for col = 1, 5 do
        t.equal(a.sigil[row][col], b.sigil[row][col], "sigil " .. row .. "," .. col)
      end
    end
  end)

  t.case("case in the address does not change the card", function()
    -- Addresses arrive checksummed from one place and lower-case from
    -- another. The same wallet must not have two faces.
    local mixed = card.design(REAL)
    local lower = card.design(REAL:lower())
    t.equal(mixed.scheme.name, lower.scheme.name, "scheme")
    t.equal(mixed.emblem, lower.emblem, "emblem")
    t.equal(mixed.member, lower.member, "member number")
  end)

  t.case("the 0x prefix is optional", function()
    local with = card.design(REAL)
    local without = card.design(REAL:sub(3))
    t.equal(with.scheme.name, without.scheme.name, "scheme")
    t.equal(with.pattern, without.pattern, "pattern")
  end)

  t.case("one changed character is a different card", function()
    -- The whole point. Two addresses that differ late must not look alike,
    -- because a person compares the picture and not the string.
    local a = card.design("0x" .. ("ab"):rep(20))
    local b = card.design("0x" .. ("ab"):rep(19) .. "ac")
    t.not_equal(a.member, b.member, "the member number reads the last bytes")
  end)
end)

t.suite("card / spread", function()
  t.case("a hundred addresses use every scheme and every pattern", function()
    -- A derivation that quietly collapsed onto one value would still be
    -- deterministic and completely useless.
    local schemes, patterns, emblems = {}, {}, {}
    for i = 1, 100 do
      local design = card.design(address(i))
      schemes[design.scheme.name] = true
      patterns[design.pattern] = true
      emblems[design.emblem] = true
    end
    local function count(set)
      local n = 0
      for _ in pairs(set) do n = n + 1 end
      return n
    end
    t.equal(count(schemes), #card.SCHEMES, "every scheme should appear")
    t.equal(count(patterns), #card.PATTERNS, "every pattern should appear")
    t.equal(count(emblems), #card.EMBLEMS, "every emblem should appear")
  end)

  t.case("the rare tier is rare and the common one is common", function()
    local seen = {}
    for i = 1, 400 do
      local name = card.design(address(i)).tier.name
      seen[name] = (seen[name] or 0) + 1
    end
    t.ok((seen.STANDARD or 0) > (seen.BLACK or 0),
      "STANDARD should outnumber BLACK")
    t.ok((seen.BLACK or 0) > 0, "BLACK should still turn up in 400 wallets")
    t.ok((seen.BLACK or 0) < 100, "but it should not be a quarter of them")
  end)
end)

t.suite("card / sigil", function()
  t.case("it is symmetric down the middle", function()
    -- Mirrored is what makes it read as a mark rather than as noise, and it
    -- is the one property of the sigil that is not a matter of taste.
    for i = 1, 30 do
      local sigil = card.design(address(i)).sigil
      for row = 1, 5 do
        t.equal(sigil[row][1], sigil[row][5], "row " .. row .. " outer columns")
        t.equal(sigil[row][2], sigil[row][4], "row " .. row .. " inner columns")
      end
    end
  end)

  t.case("it is not blank and not solid", function()
    local lit, total = 0, 0
    for i = 1, 40 do
      local sigil = card.design(address(i)).sigil
      for row = 1, 5 do
        for col = 1, 5 do
          total = total + 1
          if sigil[row][col] then lit = lit + 1 end
        end
      end
    end
    t.ok(lit > total * 0.2, "sigils should not be mostly empty")
    t.ok(lit < total * 0.8, "nor mostly filled")
  end)
end)

t.suite("card / robustness", function()
  t.case("junk still gets a face", function()
    -- Fed from a store. A truncated or malformed address has to draw
    -- something: a wallet that cannot be rendered is still a wallet somebody
    -- needs to see, and an error here would take the whole screen down.
    for _, junk in ipairs({ "", "0x", "0xzz", "not an address", "0xAb" }) do
      local design = card.design(junk)
      t.ok(design.scheme ~= nil, "scheme for " .. ("%q"):format(junk))
      t.ok(design.emblem ~= nil, "emblem for " .. ("%q"):format(junk))
      t.ok(#design.member == 4, "member number for " .. ("%q"):format(junk))
    end
  end)

  t.case("nil gets a face too", function()
    local design = card.design(nil)
    t.ok(design.scheme ~= nil, "a scheme")
    t.ok(design.tier ~= nil, "a tier")
  end)

  t.case("junk is still stably itself", function()
    t.equal(card.design("0xAb").member, card.design("0xAb").member,
      "padding must be derived, not random")
  end)
end)

t.suite("card / number", function()
  t.case("the address is printed in groups of four", function()
    local top, bottom = card.number(REAL)
    t.equal(top, "Cb21 34f9 F5e0 f1B0 d0F0", "the first five groups")
    t.equal(bottom, "e5b8 dA2b 3c4d 5E6f 7081", "and the last five")
  end)

  t.case("nothing is lost or invented", function()
    -- A card number that dropped a character would be a card number for a
    -- different wallet, printed convincingly.
    local top, bottom = card.number(REAL)
    local rebuilt = (top .. bottom):gsub(" ", "")
    t.equal("0x" .. rebuilt, REAL, "the two halves are the whole address")
  end)
end)

t.suite("card / turn", function()
  t.case("it starts face-on, goes edge-on, and ends face-on", function()
    local start = card.turn(0)
    local middle = card.turn(0.5)
    local finish = card.turn(1)
    t.ok(math.abs(start - 1) < 0.001, "0 should be face-on, got " .. start)
    t.ok(math.abs(middle) < 0.001, "0.5 should be edge-on, got " .. middle)
    t.ok(math.abs(finish + 1) < 0.001, "1 should be face-on again, got " .. finish)
  end)

  t.case("the swap happens exactly at the edge", function()
    -- If these two ever disagree the card changes its face in view, which is
    -- the one thing the animation exists to prevent.
    local _, before = card.turn(0.49)
    local _, after = card.turn(0.51)
    t.equal(before, false, "not yet swapped just before half way")
    t.equal(after, true, "swapped just after")
  end)

  t.case("the swing starts and ends exactly on the mark", function()
    -- A path that did not return to zero would leave the card a few pixels
    -- from where the layout put it, and every turn would walk it further.
    for _, progress in ipairs({ 0, 1 }) do
      local dx, dy, angle = card.swing(progress)
      t.ok(math.abs(dx) < 0.001, "x at " .. progress .. " should be 0, got " .. dx)
      t.ok(math.abs(dy) < 0.001, "y at " .. progress .. " should be 0, got " .. dy)
      t.ok(math.abs(angle) < 0.001, "angle at " .. progress .. " should be 0")
    end
  end)

  t.case("the swing is furthest out in the middle", function()
    local mid = select(1, card.swing(0.5))
    local early = select(1, card.swing(0.15))
    t.ok(mid > early, "the arc should peak at the turn, not at the start")
    t.ok(mid > 10, "and it should be a visible distance, got " .. mid)
  end)

  t.case("progress outside 0..1 is clamped", function()
    t.ok(math.abs(card.turn(-3) - 1) < 0.001, "before the start is face-on")
    t.ok(math.abs(card.turn(9) + 1) < 0.001, "after the end is face-on")
    local dx = select(1, card.swing(-3))
    t.ok(math.abs(dx) < 0.001, "and the arc is at rest outside the turn")
  end)
end)
