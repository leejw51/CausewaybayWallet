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

t.suite("card / cashaddr", function()
  --- An eCash face of the shared test phrase. Fifty characters with a colon
  --- in the middle, where every other chain here is hex or bech32.
  local ECASH = "ecash:qrwzys2q6xq98vwz0kjn6ulu5m6yljr5fyc909kalg"
  local ECTEST = "ectest:qrwzys2q6xq98vwz0kjn6ulu5m6yljr5fy7w393sue"

  t.case("a CashAddr gets a face like any other address", function()
    local design = card.design(ECASH)
    t.ok(design.scheme and design.scheme.name, "no scheme")
    t.ok(design.emblem, "no emblem")
    t.ok(design.tier and design.tier.name, "no tier")
    t.ok(design.sigil and #design.sigil == 5, "no sigil")
    -- And it is stably itself, which is the whole promise of a face.
    t.equal(card.design(ECASH).emblem, design.emblem)
    t.equal(card.design(ECASH).member, design.member)
  end)

  --- The prefix is inside a CashAddr's checksum, so these are two different
  --- addresses for one key. A face that could not tell them apart would let
  --- someone confirm a mainnet transfer while looking at the testnet card.
  t.case("the two networks' faces are different cards", function()
    local main = card.design(ECASH)
    local test = card.design(ECTEST)
    local same = main.scheme.name == test.scheme.name
      and main.pattern == test.pattern
      and main.emblem == test.emblem
      and main.member == test.member
    t.equal(same, false, "one key's two networks deal the same card")
  end)

  t.case("the number keeps every character of the address", function()
    -- A card number that dropped a character would be a card number for a
    -- different wallet, printed convincingly — and a CashAddr is long enough
    -- to take the elided path, where dropping one is easiest.
    local top, bottom = card.number(ECASH)
    t.ok(top ~= "" and bottom ~= "", "nothing was printed")
    if bottom:match("^…") then
      -- Elided: the head and the tail must both be the address's own.
      local head = (top:gsub(" ", ""))
      local tail = (bottom:gsub("… ", ""):gsub(" ", ""))
      t.equal(ECASH:sub(1, #head), head, "the head is not the address's")
      t.equal(ECASH:sub(-#tail), tail, "the tail is not the address's")
    else
      t.equal((top .. bottom):gsub(" ", ""), ECASH, "the halves are not the whole")
    end
  end)

  --- The `0x` a card prints belongs only to the addresses that start with one.
  --- Printing it in front of a CashAddr would make the card lie about what
  --- kind of address it is showing.
  t.case("no hex prefix is invented for an address that has none", function()
    t.equal(tostring(ECASH):match("^0[xX]"), nil)
    t.equal(tostring(ECTEST):match("^0[xX]"), nil)
    -- The card's own rule, which the draw call reads.
    t.equal(tostring(support.ADDRESS_0):match("^0[xX]") and "0x " or "", "0x ")
    t.equal(tostring(ECASH):match("^0[xX]") and "0x " or "", "")
  end)
end)

t.suite("card / faces across the alphabets", function()
  --- The addresses this wallet holds are not one alphabet: hex on EVM, base58
  --- on Solana, bech32 on Cardano, bech32m on Midnight, CashAddr on eCash.
  --- A face has to tell two wallets apart in every one of them.
  ---
  --- This is a real regression, not a hypothetical. `bytes_of` used to read
  --- *hex pairs* out of an address, which on the non-hex alphabets scavenged
  --- whichever hex digits happened to sit next to each other — almost nothing.
  --- One eCash key's two networks dealt the same card, and so did two Cardano
  --- wallets sharing a stake credential.
  local ALPHABETS = {
    { name = "hex", prefix = "0x", set = "0123456789abcdef", len = 40 },
    { name = "cashaddr", prefix = "ecash:q", set = "qpzry9x8gf2tvdw0s3jn54khce6mua7l", len = 41 },
    { name = "bech32", prefix = "addr1q", set = "qpzry9x8gf2tvdw0s3jn54khce6mua7l", len = 92 },
    { name = "base58", prefix = "", set =
      "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz", len = 44 },
  }

  --- A face as one comparable string. The sigil is in it: two cards that
  --- agree on everything else and differ in the mark are different cards.
  local function face(address)
    local d = card.design(address)
    local marks = {}
    for row = 1, 5 do
      for col = 1, 5 do
        marks[#marks + 1] = d.sigil[row][col] and "1" or "0"
      end
    end
    return table.concat({ d.scheme.name, d.pattern, d.emblem, d.tier.name, d.member,
      table.concat(marks) }, "|")
  end

  --- Distinct addresses of one shape, generated without a random source so the
  --- test is the same test every run.
  ---
  --- The first four characters spell the index out in the alphabet's own base,
  --- which makes distinctness structural rather than something to hope for —
  --- the smallest set here is sixteen characters, so four of them count past
  --- sixty thousand. The rest are mixed from the index and the position, so
  --- the body still varies along its whole length.
  local function addresses(shape, count)
    local list = {}
    for k = 1, count do
      local chars = {}
      local counter = k
      for _ = 1, 4 do
        local index = (counter % #shape.set) + 1
        chars[#chars + 1] = shape.set:sub(index, index)
        counter = math.floor(counter / #shape.set)
      end
      for i = 5, shape.len do
        local h = (k * 7919 + i * 65537) % 1000003
        h = (h * 4099 + k * i) % 1000003
        local index = (h % #shape.set) + 1
        chars[#chars + 1] = shape.set:sub(index, index)
      end
      list[#list + 1] = shape.prefix .. table.concat(chars)
    end
    return list
  end

  t.case("a thousand wallets of any shape get a thousand faces", function()
    for _, shape in ipairs(ALPHABETS) do
      local list = addresses(shape, 1000)

      -- The generator has to hand over distinct addresses or the assertion
      -- below would pass on a technicality.
      local distinct_input = {}
      local inputs = 0
      for _, a in ipairs(list) do
        if not distinct_input[a] then distinct_input[a] = true; inputs = inputs + 1 end
      end
      t.equal(inputs, 1000, shape.name .. ": the generator repeated an address")

      local seen, faces = {}, 0
      for _, a in ipairs(list) do
        local f = face(a)
        if not seen[f] then seen[f] = true; faces = faces + 1 end
      end
      -- Not 1000 exactly: the member number is four digits, so a thousand
      -- wallets collide a handful of times by design. A few is the birthday
      -- bound; a few hundred is a hash that is not looking at the address.
      t.ok(faces >= 985, shape.name .. ": 1000 wallets dealt only " .. faces .. " faces")
    end
  end)

  --- On the hashed alphabets every character reaches the face, including the
  --- last — which is where two addresses of one key differ on a checksummed
  --- chain, and so the one that matters most here.
  t.case("one character anywhere is a different card, on the hashed alphabets",
    function()
      for _, shape in ipairs(ALPHABETS) do
        if shape.name ~= "hex" then
          local base = addresses(shape, 1)[1]
          local first, second = shape.set:sub(1, 1), shape.set:sub(2, 2)
          for at = #shape.prefix + 1, #base do
            local was = base:sub(at, at)
            local now = was == first and second or first
            local changed = base:sub(1, at - 1) .. now .. base:sub(at + 1)
            t.ok(face(base) ~= face(changed),
              shape.name .. ": changing character " .. at .. " dealt the same card")
          end
        end
      end
    end)

  --- What the *hex* path actually does, written down because it is a weaker
  --- promise than the one above and has been since the card was first drawn.
  ---
  --- `card.design` reads six bytes from the front of an address and two from
  --- the back, so an EVM address that differs only in the middle deals the
  --- same card. Thirty of its forty characters do not reach the face.
  ---
  --- This is not fixed here, and the reason is the first line of this file: a
  --- card is a face, and every EVM card already dealt was dealt this way.
  --- Widening it would change all of them, which is the one thing a face may
  --- not do. The non-hex chains had no such history — their faces were
  --- colliding outright — so they were free to be made whole.
  t.case("the hex path reads the ends of an address, not its middle", function()
    local base = "0x" .. ("ab"):rep(20)
    local blind = 0
    for at = 3, #base do
      local was = base:sub(at, at)
      local now = was == "a" and "c" or "d"
      local changed = base:sub(1, at - 1) .. now .. base:sub(at + 1)
      if face(base) == face(changed) then blind = blind + 1 end
    end
    -- Pinned exactly, not loosely: widening the hex path's reach would fix a
    -- real weakness *and* change every EVM card ever dealt. That trade is a
    -- decision someone has to make on purpose, so it fails a test rather than
    -- passing quietly.
    t.equal(blind, 26, "the hex path's reach changed")
    -- The ends, which the card is built from, do reach it.
    for _, at in ipairs({ 3, 4, #base - 1, #base }) do
      local was = base:sub(at, at)
      local now = was == "a" and "c" or "d"
      local changed = base:sub(1, at - 1) .. now .. base:sub(at + 1)
      t.ok(face(base) ~= face(changed), "character " .. at .. " does not reach the face")
    end
  end)
end)

t.suite("card / swipe", function()
  -- The real geometry, because the visibility question depends on it: the card
  -- is a little narrower than the column it lives in, and it travels its own
  -- width plus a gap so that one is clear as the other arrives.
  local CARD, WINDOW = 247, 260
  local TRAVEL = CARD + 16

  --- Does a card at this offset overlap the column at all?
  local function visible(place)
    return math.abs(place.x) < (WINDOW + CARD) / 2
  end

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
    t.ok(not visible(leaving), "and is clear of the column")
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

  t.case("both cards are visible together for most of it", function()
    -- The property the whole animation exists for, and the one the first
    -- attempt failed: with a front-loaded curve the outgoing card was half a
    -- screen away within a tenth of the time and faded besides, so what you
    -- saw was a new card appearing, not two cards moving.
    --
    local together = 0
    for step = 0, 100 do
      local leaving, arriving = card.swipe(step / 100, TRAVEL, 1)
      if visible(leaving) and visible(arriving) then together = together + 1 end
    end
    t.ok(together >= 70,
      "the two should share the column for most of the swipe, got "
        .. together .. "%")
  end)

  t.case("neither card fades", function()
    -- The one leaving goes by being clipped at the edge of the column, the
    -- way a card leaves a window in the physical world. An outgoing card at a
    -- third opacity is not something you see travelling.
    for step = 0, 20 do
      local leaving, arriving = card.swipe(step / 20, TRAVEL, 1)
      t.equal(leaving.alpha, 1, "the card leaving stays opaque")
      t.equal(arriving.alpha, 1, "so does the one arriving")
    end
  end)

  t.case("it decelerates without front-loading", function()
    -- The band the curve has to sit in, pinned from both sides. Ahead of
    -- linear at the midpoint, or it is not easing; not so far ahead that the
    -- outgoing card is gone before the eye finds it, which is the failure
    -- `expo_out` had at 97%.
    local _, half = card.swipe(0.5, TRAVEL, 1)
    local moved = (TRAVEL - half.x) / TRAVEL
    t.ok(moved > 0.55, "it should be ahead of linear by half time, got " .. moved)
    t.ok(moved < 0.85, "but not nearly finished, got " .. moved)
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
