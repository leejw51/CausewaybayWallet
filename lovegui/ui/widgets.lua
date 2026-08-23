--- Buttons, fields and rows — each with its own springs, so they move.
---
--- Every widget keeps a little physics state between frames: how hovered it is,
--- how pressed, how selected. Those are springs rather than booleans, which is
--- the whole difference between a UI that switches and one that *moves*. A
--- button does not become hovered; it accelerates toward hovered and arrives
--- with a slight overshoot, and the eye reads that as a physical thing.
---
--- State lives in a table the caller owns and passes back each frame, keyed by
--- a name. Immediate-mode drawing with retained motion: the screens stay
--- declarative, and nothing has to be constructed or torn down when a list
--- changes length.

local theme = require("ui.theme")
local anim = require("ui.anim")

local widgets = {}

--- A bag of springs, created on demand.
---
--- A screen makes one of these and hands it to every widget it draws. A widget
--- that is drawn for the first time gets a spring at rest; one that stops being
--- drawn simply stops being updated, which is the correct behaviour for a list
--- that shrank.
local Springs = {}
Springs.__index = Springs
widgets.Springs = Springs

function Springs.new()
  return setmetatable({ items = {} }, Springs)
end

function Springs:get(key, stiffness, damping)
  local spring = self.items[key]
  if not spring then
    spring = anim.Spring.new(0, stiffness or 220, damping or 0.7)
    self.items[key] = spring
  end
  return spring
end

function Springs:update(dt)
  for _, spring in pairs(self.items) do
    spring:update(dt)
  end
end

-- ------------------------------------------------------------------ hit test

function widgets.hit(x, y, box)
  return x >= box.x and x < box.x + box.w and y >= box.y and y < box.y + box.h
end

-- -------------------------------------------------------------------- button

--- A button. Returns true on the frame it is clicked.
---
--- `state` carries `mouse_x`, `mouse_y` and `clicked` — the screen gathers
--- those once and every widget reads them, so nothing here touches `love.mouse`
--- and the whole file stays testable in principle.
function widgets.button(springs, key, box, label, state, options)
  options = options or {}
  local hovered = not options.disabled and widgets.hit(state.mouse_x, state.mouse_y, box)
  local clicked = hovered and state.clicked

  -- Two springs: one for the hover glow, one snappier for the press. Pressing
  -- is instant feedback and wants stiffness; hovering is ambience.
  local lift = springs:get(key .. ".hover", 260, 0.65)
  lift:to(hovered and 1 or 0)
  local press = springs:get(key .. ".press", 900, 0.5)
  press:to(clicked and 1 or 0)

  local accent = options.colour or theme.colour.cyan
  local sink = press.value * 2
  local x, y = box.x, box.y + sink
  local w, h = box.w, box.h

  if options.disabled then
    theme.rect(theme.colour.deep, x, y, w, h)
    theme.outline(theme.colour.faint, x, y, w, h, 0.5)
    theme.text_centred(label, x + w / 2, y + (h - 15) / 2, theme.colour.faint, theme.font.body)
    return false
  end

  -- The glow behind a hovered button, drawn additively so it blooms.
  if lift.value > 0.01 then
    local previous = love.graphics.getBlendMode()
    love.graphics.setBlendMode("add")
    for i = 1, 3 do
      theme.set(accent, lift.value * 0.06 * (4 - i))
      love.graphics.rectangle("fill", x - i, y - i, w + i * 2, h + i * 2)
    end
    love.graphics.setBlendMode(previous)
  end

  local fill = theme.mix(theme.colour.panel, theme.colour.raised, lift.value)
  theme.rect(fill, x, y, w, h)
  -- The top edge lights up with hover; the bottom stays dark. One pixel each,
  -- which at this resolution is all a bevel needs to be.
  theme.rect(theme.mix(theme.colour.raised, accent, lift.value), x, y, w, 1)
  theme.rect(theme.colour.void, x, y + h - 1, w, 1, 0.6)
  theme.outline(theme.mix(theme.colour.raised, accent, lift.value), x, y, w, h)

  local ink = theme.mix(theme.colour.text, accent, lift.value * 0.8)
  theme.text_centred(label, x + w / 2, y + (h - 15) / 2, ink, options.font or theme.font.body)

  return clicked, hovered
end

-- --------------------------------------------------------------------- field

--- A single-line text field. Draws only; the model owns the text.
function widgets.field(springs, key, box, value, label, focused, options)
  options = options or {}
  local glow = springs:get(key .. ".focus", 200, 0.8)
  glow:to(focused and 1 or 0)

  local accent = options.colour or theme.colour.cyan
  theme.rect(theme.colour.void, box.x, box.y, box.w, box.h)
  theme.outline(theme.mix(theme.colour.raised, accent, glow.value), box.x, box.y, box.w, box.h)

  if label then
    theme.text(label, box.x, box.y - 13, theme.mix(theme.colour.dim, accent, glow.value),
      theme.font.small)
  end

  local font = theme.font.small
  local shown = value
  -- Show the tail once it overflows, because that is where the cursor is and
  -- what a person is checking as they type.
  local room = box.w - 8
  while theme.width(shown, font) > room and #shown > 0 do
    shown = shown:sub(2)
  end

  local ink = value == "" and theme.colour.faint or theme.colour.text
  local text = value == "" and (options.placeholder or "") or shown
  theme.text(text, box.x + 4, box.y + (box.h - 15) / 2, ink, font)

  -- A blinking block cursor, because this is 8-bit and a caret would not be.
  if focused then
    local blink = (love.timer.getTime() * 2) % 2 < 1
    if blink then
      local cursor_x = box.x + 4 + theme.width(shown, font)
      theme.rect(accent, math.min(cursor_x, box.x + box.w - 5), box.y + (box.h - 11) / 2, 3, 11)
    end
  end

  return glow.value
end

-- ----------------------------------------------------------------------- row

--- A row in the wallet list. Slides right when selected or hovered.
function widgets.row(springs, key, box, state, selected, options)
  options = options or {}
  local hovered = widgets.hit(state.mouse_x, state.mouse_y, box)
  local clicked = hovered and state.clicked

  -- Selection is heavier than hover: more overshoot, slower settle. It is a
  -- bigger event and should feel like one.
  local slide = springs:get(key .. ".slide", 190, 0.55)
  slide:to((selected and 1 or 0) + (hovered and 0.35 or 0))

  local x = box.x + slide.value * 5
  local accent = options.colour or theme.colour.cyan

  if slide.value > 0.01 then
    theme.rect(theme.mix(theme.colour.deep, theme.colour.panel, math.min(1, slide.value)),
      x, box.y, box.w, box.h)
    -- A bar down the left edge, growing with selection.
    theme.rect(accent, x - 2, box.y, 2, box.h, math.min(1, slide.value))
  end

  return clicked, hovered, x, slide.value
end

-- --------------------------------------------------------------------- frame

--- A titled box: a one-pixel border with its label sitting *in* the top edge.
---
--- The single element that makes a screen look designed rather than assembled.
--- The title interrupts the border rather than floating above it, which is the
--- detail that reads as deliberate — and it costs one extra rectangle, drawn in
--- the background colour behind the text to punch the gap.
---
--- Returns the inner rectangle, so callers lay out against the content area
--- rather than doing the border arithmetic at every call site.
function widgets.frame(x, y, w, h, title, options)
  options = options or {}
  local edge = options.edge or theme.colour.raised
  local ink = options.ink or theme.colour.dim

  -- The fill is darkened rather than opaque: the backdrop should still be
  -- visible through the interface, just not competing with it.
  -- Opaque enough to read against: the backdrop is a painting of a town at
  -- dusk, and its rooftops are the same warm red as an error message.
  theme.rect(theme.colour.void, x, y, w, h, options.fill or 0.80)
  theme.outline(edge, x, y, w, h, options.alpha or 0.9)

  if title then
    local width = theme.width(title, theme.font.small)
    -- Punch a gap in the border, then set the title into it.
    theme.rect(theme.colour.void, x + 7, y - 1, width + 6, 3, 1)
    theme.text(title, x + 10, y - 7, ink, theme.font.small, options.alpha)
  end

  return { x = x + 5, y = y + 8, w = w - 10, h = h - 13 }
end

--- A small pill of text — a status chip, a network name, a tag.
function widgets.chip(x, y, label, colour, options)
  options = options or {}
  local width = theme.width(label, theme.font.small) + 10
  local height = 14
  theme.rect(colour, x, y, width, height, 0.18)
  theme.outline(colour, x, y, width, height, 0.55)
  theme.text(label, x + 5, y + 1, colour, theme.font.small, options.alpha)
  return width
end

-- -------------------------------------------------------------------- dialog

--- A modal panel that scales in. Returns the box it drew, for hit testing.
---
--- `t` is 0..1, driven by the caller so the same tween can fade the backdrop.
--- Scaling from 0.9 rather than 0 keeps it from looking like it was fired out
--- of a cannon; the overshoot in `back_out` does the rest.
function widgets.dialog(box, t, title)
  local eased = anim.back_out(math.min(1, t))
  theme.rect(theme.colour.void, 0, 0, theme.WIDTH, theme.HEIGHT, 0.72 * math.min(1, t))

  local scale = 0.9 + 0.1 * eased
  local w, h = box.w * scale, box.h * scale
  local x, y = box.x + (box.w - w) / 2, box.y + (box.h - h) / 2

  theme.panel(x, y, w, h, { fill = theme.colour.deep, edge = theme.colour.cyan, alpha = eased })
  if title then
    theme.text_centred(title, x + w / 2, y + 6, theme.colour.cyan, theme.font.body, eased)
    theme.rule(x + 8, y + 20, w - 16, theme.colour.cyan_dark, eased)
  end
  return { x = x, y = y, w = w, h = h, eased = eased }
end

-- ------------------------------------------------------------------- readout

--- A big number with a label under it, for the balance.
function widgets.readout(x, y, w, value, unit, options)
  options = options or {}
  local font = options.font or theme.font.huge
  local colour = options.colour or theme.colour.gold

  -- Drawn twice, the lower copy dark, so it sits on the panel rather than
  -- floating over it.
  theme.text_centred(value, x + w / 2, y + 1, { 0, 0, 0 }, font, 0.6 * (options.alpha or 1))
  theme.text_centred(value, x + w / 2, y, colour, font, options.alpha)
  if unit then
    theme.text_centred(unit, x + w / 2, y + theme.height(font) - 2,
      theme.colour.dim, theme.font.small, options.alpha)
  end
end

return widgets
