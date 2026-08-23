--- The wallet as a bank card, dealt from its own address.
---
--- ## Why a card
---
--- A list of hex strings is a list of hex strings. Nobody recognises
--- `0xCb2134…cb3581`, nobody can tell it from `0xCb2f34…cb3581` at a glance,
--- and everybody has to read all forty characters to be sure. A card is a
--- *face*: after seeing it twice you know your green one with the rocket, and
--- the moment the wrong card is on screen you know that too, before you have
--- read a single character.
---
--- That is the actual safety argument for this file. Recognition beats
--- verification — an address you check character by character gets checked
--- carefully the first three times and skimmed forever after.
---
--- ## Everything on it comes out of the address
---
--- No randomness, no stored preference, no counter. The same address produces
--- the same card on every machine, in every run, forever — which is the only
--- way a face is worth anything. Change one character of the address and it is
--- a different card, visibly.
---
--- The address is twenty bytes of hash output, so its bytes are already evenly
--- distributed; there is nothing to be gained by hashing them again. Different
--- bytes drive different choices, so two addresses sharing a prefix still
--- differ everywhere else on the card.
---
---   byte 1   the colour scheme
---   byte 2   the background pattern
---   byte 3   the emblem
---   4 – 5    the sigil, fifteen bits of it
---   6        the tier roll
---   19 – 20  the member number
---
--- `design` touches no `love.*` call, which is what lets the tests assert all
--- of the above without a window.

local theme = require("ui.theme")
local anim = require("ui.anim")
local sprite = require("ui.sprite")

local card = {}

--- The six card colourways.
---
--- Named by their ink because that is what you see first. All six are built
--- from the sixteen palette colours in `ui/theme.lua` — a card that mixed its
--- own colours would be the one thing on screen not made of the same sixteen,
--- and it would look it.
card.SCHEMES = {
  { name = "CRYO",  ink = "cyan",    face = "deep",   edge = "cyan_dark" },
  { name = "BUL",   ink = "gold",    face = "void",   edge = "amber" },
  { name = "NEON",  ink = "magenta", face = "deep",   edge = "magenta" },
  { name = "JADE",  ink = "green",   face = "void",   edge = "green" },
  { name = "EMBER", ink = "red",     face = "deep",   edge = "amber" },
  { name = "DUSK",  ink = "amber",   face = "panel",  edge = "gold" },
}

--- The backgrounds. Each is a pattern a 1983 machine could draw, because each
--- is drawn the way one would have: rectangles on a grid, nothing else.
card.PATTERNS = { "stripes", "grid", "circuit", "waves", "stars", "chevron" }

--- The emblem, stamped like a hologram. Drawn from the sprites already in
--- `assets/`, so this adds no art and cannot go missing on its own.
card.EMBLEMS = { "globe", "rocket", "key", "coin", "skull", "wallet", "spark", "logo" }

--- The tiers, and how often each comes up.
---
--- Weighted rather than uniform, and that is the whole joke: a black card is
--- three wallets in a hundred, so somebody who makes a few will eventually get
--- one and it will feel like something. Nothing about a tier means anything —
--- it buys no feature and unlocks nothing — which is stated on the card in the
--- only place it could be missed.
card.TIERS = {
  { name = "STANDARD", weight = 60, ink = "dim" },
  { name = "GOLD",     weight = 25, ink = "gold" },
  { name = "PLATINUM", weight = 12, ink = "cyan" },
  { name = "BLACK",    weight = 3,  ink = "magenta" },
}

--- The address as bytes.
---
--- Junk still has to produce a card: this is fed from a store, and a truncated
--- or malformed address must give a face rather than an error — a wallet that
--- cannot be drawn is still a wallet somebody needs to see. Short input is
--- padded from what little there was, deterministically, so even a broken
--- address is stably itself.
local function bytes_of(address)
  local hex = (tostring(address or ""):gsub("^0[xX]", ""))
  local out = {}
  for pair in hex:gmatch("%x%x") do
    out[#out + 1] = tonumber(pair, 16)
  end
  local seed = out[1] or 7
  while #out < 20 do
    out[#out + 1] = (#out * 37 + seed * 11 + 13) % 256
  end
  return out
end

--- One bit out of a number, without `bit` — this has to run wherever Lua does.
local function bit_of(value, index)
  return math.floor(value / 2 ^ index) % 2 == 1
end

--- The 5x5 sigil: an identicon, mirrored down its middle.
---
--- Mirrored because a symmetric shape reads as a *mark* and an asymmetric one
--- reads as noise — the same reason every identicon scheme since the first one
--- has done it. Fifteen bits fill the left three columns and the right two are
--- their reflection, so the mark is always a mark and never a smear.
local function sigil_of(a, b)
  local bits = a * 256 + b
  local grid = {}
  local index = 0
  for row = 1, 5 do
    grid[row] = {}
    for col = 1, 3 do
      grid[row][col] = bit_of(bits, index)
      index = index + 1
    end
    grid[row][4] = grid[row][2]
    grid[row][5] = grid[row][1]
  end
  return grid
end

local function tier_of(roll)
  local total = 0
  for _, tier in ipairs(card.TIERS) do total = total + tier.weight end
  local point = roll % total
  for _, tier in ipairs(card.TIERS) do
    if point < tier.weight then return tier end
    point = point - tier.weight
  end
  return card.TIERS[1]
end

--- Everything about one card, from one address. No `love.*` anywhere below.
function card.design(address)
  local b = bytes_of(address)
  return {
    address = address,
    scheme  = card.SCHEMES[b[1] % #card.SCHEMES + 1],
    pattern = card.PATTERNS[b[2] % #card.PATTERNS + 1],
    emblem  = card.EMBLEMS[b[3] % #card.EMBLEMS + 1],
    sigil   = sigil_of(b[4], b[5]),
    tier    = tier_of(b[6]),
    member  = ("%04d"):format((b[19] * 256 + b[20]) % 10000),
  }
end

--- The address as a card number: groups of four, the way one is printed.
---
--- Grouped because a run of forty characters is unreadable and unspeakable,
--- and because every person alive already knows how to read this shape.
--- Nothing is hidden — both halves are the real address, and together they are
--- all of it.
function card.number(address)
  local hex = (tostring(address or ""):gsub("^0[xX]", ""))
  local groups = {}
  for i = 1, #hex, 4 do
    groups[#groups + 1] = hex:sub(i, i + 3)
  end
  local top, bottom = {}, {}
  for i, group in ipairs(groups) do
    if i <= 5 then top[#top + 1] = group else bottom[#bottom + 1] = group end
  end
  return table.concat(top, " "), table.concat(bottom, " ")
end

-- ------------------------------------------------------------------ drawing

--- A rectangle that cannot escape the card.
---
--- Every pattern below is drawn from arithmetic that runs off the edges, and
--- the usual answer — a scissor — is wrong here: `setScissor` works in canvas
--- coordinates and would ignore the flip transform, so the clip would stand
--- still while the card turned under it. Clamping is two `max` calls and is
--- correct under any transform.
local function clipped(box, colour, x, y, w, h, a)
  local x1 = math.max(box.x, x)
  local y1 = math.max(box.y, y)
  local x2 = math.min(box.x + box.w, x + w)
  local y2 = math.min(box.y + box.h, y + h)
  if x2 > x1 and y2 > y1 then
    theme.rect(colour, x1, y1, x2 - x1, y2 - y1, a)
  end
end

--- The background patterns. Each fills the box and nothing outside it.
local function draw_pattern(kind, box, ink, t)
  local a = 0.10
  if kind == "stripes" then
    -- Diagonals, built a row at a time so they clip. Each row is offset a
    -- little further along, which is what makes a stack of dashes a slope.
    for row = 0, box.h - 1, 2 do
      local shift = (row * 0.7) % 9
      for i = -1, math.ceil(box.w / 9) do
        clipped(box, ink, box.x + i * 9 + shift, box.y + row, 3, 2, a)
      end
    end
  elseif kind == "grid" then
    for x = 0, box.w, 10 do clipped(box, ink, box.x + x, box.y, 1, box.h, a * 0.9) end
    for y = 0, box.h, 10 do clipped(box, ink, box.x, box.y + y, box.w, 1, a * 0.9) end
  elseif kind == "circuit" then
    -- Traces that turn corners, laid out from the box rather than at random,
    -- so the same card draws the same board every frame.
    for i = 0, math.floor(box.h / 9) do
      local y = box.y + 8 + i * 9
      local run = 20 + (i * 13) % (box.w - 40)
      clipped(box, ink, box.x + 6, y, run, 1, a * 1.2)
      clipped(box, ink, box.x + 6 + run, y, 1, 9, a * 1.2)
      clipped(box, ink, box.x + 6 + run - 2, y - 2, 3, 3, a * 1.6)
    end
  elseif kind == "waves" then
    for i = 0, math.ceil(box.h / 22) do
      for x = 0, box.w, 2 do
        local y = box.y + 14 + i * 22 + math.sin((x + i * 20) / 13) * 5
        clipped(box, ink, box.x + x, y, 2, 1, a)
      end
    end
  elseif kind == "stars" then
    for i = 0, 47 do
      -- A cheap deterministic scatter: two odd strides walk the box without
      -- landing twice in the same place, and they are the same points every
      -- frame — a card that twinkled differently each time would be a
      -- different card each time.
      local x = (i * 37) % box.w
      local y = (i * 61) % box.h
      clipped(box, ink, box.x + x, box.y + y, 1, 1, a * (1.4 + (i % 3) * 0.5))
    end
  elseif kind == "chevron" then
    -- The one pattern that moves: chevrons drifting slowly upward. It reads
    -- as a card that is *on* rather than printed, and it is the only motion
    -- allowed to be tied to the clock instead of to the address.
    for i = 0, math.ceil(box.h / 16) + 1 do
      local y = box.y + i * 16 - 8 + (t * 6) % 16
      for x = 0, box.w, 2 do
        local dy = math.abs(((x + i * 8) % 32) - 16) / 2
        clipped(box, ink, box.x + x, y + dy, 2, 1, a)
      end
    end
  end
end

--- The contact pad. Every card has one and it is most of what says "card".
local function draw_chip(x, y)
  theme.rect(theme.colour.gold, x, y, 17, 13, 0.85)
  theme.rect(theme.colour.amber, x, y, 17, 13, 0.35)
  theme.rect(theme.colour.void, x + 6, y, 1, 13, 0.55)
  theme.rect(theme.colour.void, x, y + 4, 17, 1, 0.55)
  theme.rect(theme.colour.void, x, y + 8, 17, 1, 0.55)
  theme.outline(theme.colour.gold, x, y, 17, 13, 0.6)
end

local function draw_sigil(grid, x, y, pixel, colour, alpha)
  for row = 1, 5 do
    for col = 1, 5 do
      if grid[row][col] then
        theme.rect(colour, x + (col - 1) * pixel, y + (row - 1) * pixel,
          pixel - 1, pixel - 1, alpha)
      end
    end
  end
end

--- Draw the card.
---
--- `flip` is -1..1: the cosine of how far through a turn it is, so 0 is exactly
--- edge-on. The caller owns that number because the caller knows when the
--- wallet changed; all this does is honour it.
function card.draw(design, box, options)
  options = options or {}
  local t = options.time or 0
  local flip = options.flip == nil and 1 or options.flip
  local alpha = options.alpha or 1
  local ink = theme.colour[design.scheme.ink]
  local edge = theme.colour[design.scheme.edge]
  local face = theme.colour[design.scheme.face]

  local cx, cy = box.x + box.w / 2, box.y + box.h / 2

  love.graphics.push()
  love.graphics.translate(cx, cy)
  -- The arc. A card that turns over on the spot is a rectangle changing its
  -- width; a card that *travels* while it turns is a card being flicked out of
  -- a wallet and dropped back. The path is a half sine — zero at both ends, so
  -- it leaves and arrives exactly where the layout says, and no clamping is
  -- ever needed at the seams.
  love.graphics.translate(options.swing_x or 0, options.swing_y or 0)
  love.graphics.rotate(options.angle or 0)
  -- The turn itself. Scaling x by the cosine is the whole illusion: a real
  -- card seen edge-on is a line, and a rectangle scaled to zero width is one
  -- too. Held just off zero so it never disappears completely.
  love.graphics.scale(math.max(0.02, math.abs(flip)), 1)
  -- A slight tilt with the lean, so it turns like an object rather than a
  -- window blind.
  love.graphics.shear(0, (1 - math.abs(flip)) * 0.06 * (flip < 0 and -1 or 1))
  love.graphics.translate(-cx, -cy)

  -- The body.
  theme.rect(theme.colour.void, box.x + 2, box.y + 3, box.w, box.h, 0.45 * alpha)
  theme.rect(face, box.x, box.y, box.w, box.h, 0.97 * alpha)
  theme.rect(ink, box.x, box.y, box.w, box.h, 0.07 * alpha)

  draw_pattern(design.pattern, box, ink, t)

  -- The emblem: the card's character, and the thing actually recognised from
  -- across the room. Big, over on the right where the face is otherwise empty,
  -- and above the card number rather than behind it — at 0.14 alpha down in
  -- the corner it was invisible under the digits, which is the same as not
  -- having one.
  --
  -- Drawn twice: once additively for a bloom in the card's own ink, once
  -- normally. It sits *in* the card that way rather than looking pasted on.
  local drift = math.sin(t * 0.9) * 2
  sprite.draw(design.emblem, box.x + box.w - 46, box.y + box.h / 2 - 6 + drift, 74, {
    alpha = 0.16 * alpha,
    colour = ink,
    blend = "add",
  })
  sprite.draw(design.emblem, box.x + box.w - 46, box.y + box.h / 2 - 6 + drift, 62, {
    alpha = 0.34 * alpha,
    colour = ink,
  })

  theme.outline(edge, box.x, box.y, box.w, box.h, 0.8 * alpha)
  -- One bright pixel along the top and one dark along the bottom: the whole
  -- bevel, at this resolution.
  theme.rect(ink, box.x, box.y, box.w, 1, 0.5 * alpha)
  theme.rect(theme.colour.void, box.x, box.y + box.h - 1, box.w, 1, 0.6 * alpha)

  -- ------------------------------------------------------------ the face
  theme.text("CAUSEWAYBAY", box.x + 10, box.y + 8, ink, theme.font.small, alpha)
  theme.text("BANK", box.x + 10, box.y + 20, theme.colour.dim, theme.font.small,
    0.8 * alpha)

  local tier_ink = theme.colour[design.tier.ink]
  theme.text_right(design.tier.name, box.x + box.w - 10, box.y + 8, tier_ink,
    theme.font.small, alpha)
  theme.text_right("NO " .. design.member, box.x + box.w - 10, box.y + 20,
    theme.colour.faint, theme.font.small, 0.9 * alpha)

  draw_chip(box.x + 10, box.y + 40)
  draw_sigil(design.sigil, box.x + 34, box.y + 38, 4, ink, 0.85 * alpha)

  if options.body then options.body(box, ink, alpha) end

  -- The number, in the place a card puts it.
  local top, bottom = card.number(design.address)
  theme.text("0x " .. top, box.x + 10, box.y + box.h - 44, ink, theme.font.small, alpha)
  theme.text("   " .. bottom, box.x + 10, box.y + box.h - 31, ink, theme.font.small, alpha)

  theme.text((options.holder or "WALLET"):upper(), box.x + 10, box.y + box.h - 15,
    theme.colour.text, theme.font.small, alpha)

  -- ---------------------------------------------------------- the sheen
  -- A band of light crossing the face every few seconds. It is the one thing
  -- that stops a static rectangle reading as a picture of a card.
  --
  -- Built row by row rather than as a diagonal polygon behind a scissor,
  -- because `setScissor` works in canvas coordinates and ignores the flip
  -- transform above — the clip would sit still while the card turned under it.
  -- A row is a rectangle, and a rectangle can be clipped with two `max` calls.
  local period = 4.5
  local sweep = ((t % period) / period) * (box.w + box.h * 2) - box.h
  local previous = love.graphics.getBlendMode()
  love.graphics.setBlendMode("add")
  local band = 13
  for row = 0, box.h - 1, 2 do
    local left = box.x + sweep + (box.h - row) * 0.7
    local x = math.max(box.x, left)
    local right = math.min(box.x + box.w, left + band)
    if right > x then
      -- Brightest in the middle of the band, so it is a gleam and not a bar.
      for i = 0, 3 do
        local shine = math.sin((i + 0.5) / 4 * math.pi) * 0.055
        local sx = math.max(box.x, left + i * band / 4)
        local sw = math.min(box.x + box.w, sx + band / 4) - sx
        if sw > 0 then theme.rect(theme.colour.white, sx, box.y + row, sw, 2, shine * alpha) end
      end
    end
  end
  love.graphics.setBlendMode(previous)

  -- Edge-on, the card is a bright line rather than a squashed picture.
  if math.abs(flip) < 0.25 then
    local glare = 1 - math.abs(flip) / 0.25
    theme.rect(theme.colour.white, box.x, box.y, box.w, box.h, glare * 0.35 * alpha)
    theme.rect(ink, box.x, box.y, box.w, box.h, glare * 0.3 * alpha)
  end

  love.graphics.pop()
end

--- Where a card is in its turn, and which design it should be showing.
---
--- One number does both jobs. `turn` runs 0 to 1; the cosine of half a
--- revolution is +1 at the start, 0 half way and -1 at the end, so the card is
--- edge-on at exactly the moment the design should change — which is why the
--- swap is invisible and why it has to be driven from here rather than from a
--- separate timer that could drift out of step with it.
function card.turn(progress)
  local eased = anim.expo_in_out(math.min(1, math.max(0, progress)))
  return math.cos(eased * math.pi), eased >= 0.5
end

--- Where the card is along its arc, and how far it has rolled.
---
--- Returns an offset and an angle, all three zero at both ends of the turn —
--- a half sine, which is why the card leaves and arrives exactly on the
--- layout's mark with nothing to clamp or snap at the seams.
---
--- Driven from the *eased* progress rather than the raw one, so the swing has
--- the same acceleration as the flip. Driving them from different curves is
--- what makes an animation look like two effects playing at once instead of
--- one object moving.
function card.swing(progress, reach)
  local eased = anim.expo_in_out(math.min(1, math.max(0, progress)))
  local along = math.sin(eased * math.pi)
  reach = reach or 1
  return along * 26 * reach, -along * 15 * reach, along * 0.13 * reach
end

return card
