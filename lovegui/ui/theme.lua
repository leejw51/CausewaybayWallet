--- The palette, the fonts, and the low-resolution canvas everything is drawn to.
---
--- ## Why there is a canvas
---
--- The 8-bit look is not a filter applied to a modern UI — it comes from
--- actually drawing at 8-bit resolution. Everything renders to a 480×270
--- canvas, which is then scaled to the window by a whole number with
--- nearest-neighbour filtering.
---
--- That one decision does most of the work. Text becomes chunky because the
--- font really is 8 pixels tall. A one-pixel border really is one pixel, four
--- screen pixels wide at 4×. A sprite drawn at 32×32 has 32 visible squares
--- across. Nothing has to *try* to look retro, and nothing can accidentally
--- render a smooth gradient or a hairline, because there is nowhere to put one.
---
--- The scale is an integer on purpose. At 2.5× a one-pixel line lands on half a
--- screen pixel and the filter smears it, and the whole illusion goes with it.
--- Letterboxing the remainder is the price, and it is the right price.
---
--- ## The palette
---
--- Sixteen colours, fixed. Not because a modern GPU cannot do more, but because
--- a constrained palette is what makes a set of screens look designed rather
--- than assembled. Every colour in the UI comes from here; none are written
--- inline anywhere else.

local theme = {}

--- The resolution everything is drawn at. 16:9 so it scales cleanly to a
--- modern window, and small enough that 8px text is genuinely blocky.
theme.WIDTH = 480
theme.HEIGHT = 270

-- ------------------------------------------------------------------ palette

theme.colour = {
  -- Ground: three depths of near-black blue, for panels stacked on panels.
  void      = { 0.04, 0.03, 0.09 },
  deep      = { 0.09, 0.07, 0.18 },
  panel     = { 0.15, 0.12, 0.28 },
  raised    = { 0.22, 0.18, 0.38 },

  -- Ink.
  text      = { 0.93, 0.94, 1.00 },
  dim       = { 0.55, 0.55, 0.72 },
  faint     = { 0.34, 0.33, 0.50 },

  -- The accent. Cyan is the wallet's colour, used for anything active.
  cyan      = { 0.30, 0.94, 1.00 },
  cyan_dark = { 0.12, 0.45, 0.58 },

  -- Money is gold, danger is red, success is green, and a warning is amber.
  gold      = { 1.00, 0.82, 0.25 },
  green     = { 0.42, 0.96, 0.48 },
  red       = { 1.00, 0.32, 0.42 },
  amber     = { 1.00, 0.66, 0.20 },
  magenta   = { 0.94, 0.40, 0.90 },
  white     = { 1.00, 1.00, 1.00 },
}

--- A colour with a different alpha, without mutating the palette.
function theme.alpha(colour, a)
  return { colour[1], colour[2], colour[3], a }
end

--- Mix two colours. `t` of 0 is the first, 1 is the second.
function theme.mix(a, b, t)
  return {
    a[1] + (b[1] - a[1]) * t,
    a[2] + (b[2] - a[2]) * t,
    a[3] + (b[3] - a[3]) * t,
  }
end

-- -------------------------------------------------------------------- fonts

--- The glyphs in `assets/font.png`, in the order they appear in it.
---
--- This must match `GLYPHS` in `tools/generate-font.py` exactly: the image is
--- just a row of pictures, and this string is the only thing that says which
--- picture is which. One character out and every letter after it is wrong —
--- which renders as fluent nonsense rather than an obvious break, so the
--- generator refuses to skip a glyph it cannot draw.
theme.GLYPHS = " !\"#$%&'()*+,-./0123456789:;<=>?@"
  .. "ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`"
  .. "abcdefghijklmnopqrstuvwxyz{|}~"
  .. "·…—"

--- Loaded in `theme.load`, because fonts need a graphics context.
---
--- Each entry is `{font, scale}` rather than a bare Font, because the sizes are
--- one bitmap face drawn at whole-number scales. Use `theme.width` and
--- `theme.height` rather than the Font's own methods, which know nothing about
--- the scale.
theme.font = {}

function theme.load()
  -- Nearest filtering globally: every image and font in the game is meant to
  -- have visible pixels, and the default (linear) would soften all of them.
  love.graphics.setDefaultFilter("nearest", "nearest")

  -- A baked bitmap font (see tools/generate-font.py), not a vector one.
  --
  -- LÖVE's built-in face is Vera Sans, hinted for ordinary sizes. At the ~10px
  -- this UI draws, it renders either antialiased — grey edge pixels, each of
  -- which a 3x nearest upscale turns into a 3x3 grey block — or with "mono"
  -- hinting, which has no greys but drops stems, so "WALLET" comes out with
  -- holes in it. Neither is a pixel font. This is one.
  --
  theme.font.pixel = love.graphics.newImageFont("assets/font.png", theme.GLYPHS)
  theme.font.pixel:setFilter("nearest", "nearest")
  -- No fallback: LÖVE refuses to mix an image font with a vector one, and it
  -- is right to — they have no common baseline or size. Unknown characters are
  -- handled by `theme.printable` instead, which turns them into something
  -- visible rather than leaving a hole in the middle of a message.

  -- One face at several sizes, drawn at whole-number scales. Scaling a bitmap
  -- font by 2 or 3 with a nearest filter keeps every edge exactly on a pixel;
  -- a second, larger raster would not match it.
  theme.font.small = { font = theme.font.pixel, scale = 1 }
  theme.font.body = { font = theme.font.pixel, scale = 1 }
  theme.font.big = { font = theme.font.pixel, scale = 2 }
  theme.font.huge = { font = theme.font.pixel, scale = 3 }

  theme.canvas = love.graphics.newCanvas(theme.WIDTH, theme.HEIGHT)
  theme.canvas:setFilter("nearest", "nearest")
end

-- ------------------------------------------------------------------ scaling

--- How the canvas maps onto the window: whole-number scale, centred.
function theme.transform()
  local window_width, window_height = love.graphics.getDimensions()
  local scale = math.min(window_width / theme.WIDTH, window_height / theme.HEIGHT)
  -- Never below 1, or a small window would scale to zero and draw nothing.
  scale = math.max(1, math.floor(scale))
  local width, height = theme.WIDTH * scale, theme.HEIGHT * scale
  return scale, math.floor((window_width - width) / 2), math.floor((window_height - height) / 2)
end

--- Where a window-space point lands on the canvas.
---
--- Mouse coordinates arrive in window space, and every hit test in the game is
--- written in canvas space, so this is the one place the two meet.
function theme.to_canvas(x, y)
  local scale, offset_x, offset_y = theme.transform()
  return (x - offset_x) / scale, (y - offset_y) / scale
end

--- Draw everything in `body` to the canvas, then blit it to the window.
function theme.frame(body)
  love.graphics.setCanvas(theme.canvas)
  love.graphics.clear(theme.colour.void)
  body()
  love.graphics.setCanvas()

  local scale, offset_x, offset_y = theme.transform()
  love.graphics.setColor(1, 1, 1, 1)
  love.graphics.draw(theme.canvas, offset_x, offset_y, 0, scale, scale)
end

--- Draw `body` with everything outside `box` clipped away.
---
--- For anything that slides in from off its own edge — the card carousel is
--- the reason this exists. Rectangles can be clamped by hand; text cannot, and
--- a card number sliding out of its column and across the wallet list is not a
--- transition, it is a bug with an easing curve on it.
---
--- `setScissor` takes canvas pixels and ignores the current transform, so the
--- box is pushed through `transformPoint` first. Without that the clip would
--- stand still while the screen shake moved everything under it — visible as a
--- one-pixel sliver of card appearing past the edge on exactly the frames a
--- transfer lands.
function theme.clip(box, body)
  local x1, y1 = love.graphics.transformPoint(box.x, box.y)
  local x2, y2 = love.graphics.transformPoint(box.x + box.w, box.y + box.h)
  local previous = { love.graphics.getScissor() }
  love.graphics.setScissor(math.floor(x1), math.floor(y1),
    math.ceil(x2 - x1), math.ceil(y2 - y1))
  body()
  if previous[1] then
    love.graphics.setScissor(previous[1], previous[2], previous[3], previous[4])
  else
    love.graphics.setScissor()
  end
end

-- ------------------------------------------------------------------ drawing
--
-- Small helpers, so no screen writes a raw setColor/rectangle pair.

function theme.set(colour, a)
  love.graphics.setColor(colour[1], colour[2], colour[3], a or 1)
end

function theme.rect(colour, x, y, w, h, a)
  theme.set(colour, a)
  love.graphics.rectangle("fill", math.floor(x), math.floor(y), math.floor(w), math.floor(h))
end

--- A one-pixel outline. Always one pixel, because at this resolution it is.
function theme.outline(colour, x, y, w, h, a)
  theme.set(colour, a)
  love.graphics.setLineWidth(1)
  -- The half-pixel offset is what stops a 1px line straddling two rows.
  love.graphics.rectangle("line", math.floor(x) + 0.5, math.floor(y) + 0.5,
    math.floor(w) - 1, math.floor(h) - 1)
end

--- A panel: filled, outlined, with a lit top edge so it reads as raised.
function theme.panel(x, y, w, h, options)
  options = options or {}
  local fill = options.fill or theme.colour.panel
  local edge = options.edge or theme.colour.raised
  theme.rect(fill, x, y, w, h, options.alpha)
  theme.rect(edge, x, y, w, 1, options.alpha)
  theme.outline(edge, x, y, w, h, (options.alpha or 1) * 0.7)
end

--- The characters the baked font can draw, built once from `theme.GLYPHS`.
local drawable = nil

--- Replace anything the font cannot draw with `?`.
---
--- The wallet's messages are its own and can contain any UTF-8 — a node's
--- error, a label somebody typed. An image font renders an unknown character
--- as nothing at all, which reads as a corrupted string rather than a missing
--- glyph, so this makes the gap visible instead.
function theme.printable(text)
  if not drawable then
    drawable = {}
    for glyph in theme.GLYPHS:gmatch("[%z\1-\127\194-\244][\128-\191]*") do
      drawable[glyph] = true
    end
  end
  -- The common case is a string that is already fine, and rebuilding one
  -- character at a time every frame would be wasteful, so scan first.
  local safe = true
  for glyph in text:gmatch("[%z\1-\127\194-\244][\128-\191]*") do
    if not drawable[glyph] then
      safe = false
      break
    end
  end
  if safe then return text end

  local out = {}
  for glyph in text:gmatch("[%z\1-\127\194-\244][\128-\191]*") do
    out[#out + 1] = drawable[glyph] and glyph or "?"
  end
  return table.concat(out)
end

--- How wide a string is, at that size. Use this, not `font:getWidth`.
function theme.width(string, font)
  font = font or theme.font.body
  return font.font:getWidth(theme.printable(string)) * font.scale
end

--- How tall a line is, at that size.
function theme.height(font)
  font = font or theme.font.body
  return font.font:getHeight() * font.scale
end

--- Text, snapped to whole pixels. Sub-pixel text on a pixel canvas shimmers.
function theme.text(string, x, y, colour, font, a)
  font = font or theme.font.body
  love.graphics.setFont(font.font)
  theme.set(colour or theme.colour.text, a)
  love.graphics.print(theme.printable(string), math.floor(x), math.floor(y),
    0, font.scale, font.scale)
end

function theme.text_centred(string, cx, y, colour, font, a)
  font = font or theme.font.body
  theme.text(string, cx - theme.width(string, font) / 2, y, colour, font, a)
end

function theme.text_right(string, right, y, colour, font, a)
  font = font or theme.font.body
  theme.text(string, right - theme.width(string, font), y, colour, font, a)
end

--- Shorten in the middle, keeping both ends. An address is recognised by its
--- first and last characters, so cutting the tail off is the wrong trim.
function theme.ellipsis(string, keep_start, keep_end)
  if #string <= keep_start + keep_end + 1 then return string end
  return string:sub(1, keep_start) .. "…" .. string:sub(-keep_end)
end

--- A horizontal rule with a bright centre, fading out at both ends.
function theme.rule(x, y, w, colour, a)
  colour = colour or theme.colour.raised
  local segments = math.floor(w)
  for i = 0, segments - 1 do
    local t = i / math.max(1, segments - 1)
    local fade = 1 - math.abs(t - 0.5) * 2
    theme.rect(colour, x + i, y, 1, 1, (a or 1) * fade)
  end
end

--- A scanline overlay, drawn last. One dark row every two pixels.
---
--- Overdone this is a gimmick; at this alpha it just takes the digital edge
--- off large flat areas, the way a CRT did.
function theme.scanlines(a)
  theme.set({ 0, 0, 0 }, a or 0.10)
  for y = 0, theme.HEIGHT - 1, 2 do
    love.graphics.rectangle("fill", 0, y, theme.WIDTH, 1)
  end
end

--- A vignette: the corners of the tube were always darker.
function theme.vignette(a)
  local steps = 14
  for i = 0, steps - 1 do
    local t = i / steps
    theme.set({ 0, 0, 0 }, (a or 0.35) * (1 - t) * 0.10)
    love.graphics.rectangle("line", i, i, theme.WIDTH - i * 2, theme.HEIGHT - i * 2)
  end
end

return theme
