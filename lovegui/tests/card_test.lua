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
local support = require("tests.support")

--- Twenty bytes of hex, from a number. Enough distinct addresses to say
--- something about the distribution without shipping a fixture file.
local function address(n)
  local hex = {}
  for i = 0, 19 do
    hex[#hex + 1] = ("%02x"):format((n * 61 + i * 37 + i * i * 13) % 256)
  end
  return "0x" .. table.concat(hex)
end

--- BIP-39's published all-zeros vector, index 0 — the address in every
--- implementation's fixtures. Deliberately not an address invented for this
--- file: an invented one looks exactly like somebody's real wallet, and a
--- documentation example that could be mistaken for one is worth avoiding in
--- a public repository.
local REAL = support.ADDRESS_0

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
    t.equal(top, "9858 EfFD 232B 4033 E47d", "the first five groups")
    t.equal(bottom, "9000 3D41 EC34 EcaE da94", "and the last five")
  end)

  t.case("nothing is lost or invented", function()
    -- A card number that dropped a character would be a card number for a
    -- different wallet, printed convincingly.
    local top, bottom = card.number(REAL)
    local rebuilt = (top .. bottom):gsub(" ", "")
    t.equal("0x" .. rebuilt, REAL, "the two halves are the whole address")
  end)
end)

t.suite("card / swipe", function()
  local TRAVEL = 260

  t.case("the card leaving starts exactly on the mark", function()
    local leaving, arriving = card.swipe(0, TRAVEL, 1)
    t.ok(math.abs(leaving.x) < 0.001, "at rest it is where the layout put it")
    t.ok(math.abs(leaving.scale - 1) < 0.001, "at full size")
    t.ok(math.abs(leaving.alpha - 1) < 0.001, "and fully opaque")
    t.ok(math.abs(arriving.x - TRAVEL) < 0.001,
      "while the one arriving is a full card away, off the clip")
  end)

  t.case("the card arriving ends exactly on the mark", function()
    -- If it did not, every swipe would leave the card a few pixels from where
    -- the layout put it, and the drift would compound.
    local leaving, arriving = card.swipe(1, TRAVEL, 1)
    t.ok(math.abs(arriving.x) < 0.001, "it lands on the mark, got " .. arriving.x)
    t.ok(math.abs(arriving.scale - 1) < 0.001, "at full size")
    t.ok(math.abs(arriving.alpha - 1) < 0.001, "and fully opaque")
    t.ok(math.abs(leaving.x + TRAVEL) < 0.001, "the other has gone a full card")
    t.ok(math.abs(leaving.alpha) < 0.001, "and faded out entirely")
  end)

  t.case("down the list scrolls the card left", function()
    -- The direction is the whole point: a list moving down should move the
    -- card the same way, or the two read as unrelated.
    local leaving, arriving = card.swipe(0.5, TRAVEL, 1)
    t.ok(leaving.x < 0, "the card leaving goes left, got " .. leaving.x)
    t.ok(arriving.x > 0, "and the one arriving comes from the right")
  end)

  t.case("up the list reverses it", function()
    local leaving, arriving = card.swipe(0.5, TRAVEL, -1)
    t.ok(leaving.x > 0, "the card leaving goes right, got " .. leaving.x)
    t.ok(arriving.x < 0, "and the one arriving comes from the left")
  end)

  t.case("the two never sit on top of each other", function()
    -- They are a fixed distance apart for the whole swipe. If that ever
    -- collapsed the cards would overlap in the middle and the transition
    -- would read as one card glitching rather than two moving.
    for step = 0, 20 do
      local leaving, arriving = card.swipe(step / 20, TRAVEL, 1)
      local gap = arriving.x - leaving.x
      t.ok(math.abs(gap - TRAVEL) < 0.001,
        "a card apart at every point, got " .. gap .. " at " .. (step / 20))
    end
  end)

  t.case("it eases rather than sliding at a constant rate", function()
    -- expo_out: most of the distance is covered immediately and the arrival
    -- is a long settle. Linear motion is the thing this is not.
    local _, arriving = card.swipe(0.5, TRAVEL, 1)
    local travelled = TRAVEL - arriving.x
    t.ok(travelled > TRAVEL * 0.8,
      "half way through the time it should be most of the way there, got "
        .. travelled .. " of " .. TRAVEL)
  end)

  t.case("it only ever moves forward", function()
    local previous = math.huge
    for step = 0, 40 do
      local _, arriving = card.swipe(step / 40, TRAVEL, 1)
      t.ok(arriving.x <= previous + 0.001,
        "the arriving card must not go backwards at " .. (step / 40))
      previous = arriving.x
    end
  end)

  t.case("progress outside 0..1 is clamped", function()
    local before = select(2, card.swipe(-3, TRAVEL, 1))
    local after = select(2, card.swipe(9, TRAVEL, 1))
    t.ok(math.abs(before.x - TRAVEL) < 0.001, "before the start it is off screen")
    t.ok(math.abs(after.x) < 0.001, "after the end it is on the mark")
  end)

  t.case("the default direction is left", function()
    local leaving = card.swipe(0.5, TRAVEL)
    t.ok(leaving.x < 0, "omitting the direction should scroll left")
  end)
end)
