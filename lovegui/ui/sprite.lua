--- Loading the generated art, with a fallback for when it is not there.
---
--- The sprites in `assets/` are drawn by `tools/generate-assets.py` and
--- committed, so the game needs no API key to run. But a checkout with the
--- assets missing — a sparse clone, a half-finished `git lfs`, someone poking
--- at the repository — should still start and still be usable. So a missing
--- sprite becomes a coloured placeholder rather than a crash, and the game says
--- so once at startup instead of failing at the first draw.

local theme = require("ui.theme")

local sprite = {}

sprite.images = {}
sprite.missing = {}

--- The backdrops: whole scenes, drawn behind everything at canvas size.
---
--- Kept apart from the sprites because nothing about them is the same. They are
--- not keyed, not centred, not scaled to a size the caller picks — they fill
--- the screen at 1:1, which is what keeps their pixels the same size as the UI
--- drawn on top of them.
local SCENES = { krumlov = true }

--- Every sprite the game asks for, with the colour its placeholder uses.
local WANTED = {
  logo   = theme.colour.cyan,
  wallet = theme.colour.gold,
  coin   = theme.colour.gold,
  rocket = theme.colour.cyan,
  globe  = theme.colour.green,
  key    = theme.colour.amber,
  skull  = theme.colour.red,
  spark  = theme.colour.white,
}

--- A stand-in: a bordered square in the sprite's colour, obviously not art.
---
--- Deliberately ugly. A placeholder that looked plausible would be worse — it
--- would ship, and nobody would notice the real sprite never arrived.
local function placeholder(colour)
  local size = 32
  local data = love.image.newImageData(size, size)
  data:mapPixel(function(x, y)
    local edge = x < 2 or y < 2 or x >= size - 2 or y >= size - 2
    local cross = math.abs(x - y) < 2 or math.abs(x + y - size) < 2
    if edge or cross then return colour[1], colour[2], colour[3], 1 end
    return colour[1], colour[2], colour[3], 0.12
  end)
  return love.graphics.newImage(data)
end

function sprite.load()
  for name in pairs(SCENES) do
    local path = "assets/" .. name .. ".png"
    if love.filesystem.getInfo(path) then
      sprite.images[name] = love.graphics.newImage(path)
      sprite.images[name]:setFilter("nearest", "nearest")
    else
      sprite.missing[#sprite.missing + 1] = name
    end
  end

  for name, colour in pairs(WANTED) do
    local path = "assets/" .. name .. ".png"
    if love.filesystem.getInfo(path) then
      sprite.images[name] = love.graphics.newImage(path)
      sprite.images[name]:setFilter("nearest", "nearest")
    else
      sprite.images[name] = placeholder(colour)
      sprite.missing[#sprite.missing + 1] = name
    end
  end
  return sprite.images
end

function sprite.get(name)
  return sprite.images[name]
end

--- Fill the screen with a backdrop.
---
--- Drawn slightly larger than the canvas so it can drift a few pixels without
--- opening a gap at the edge — the drift is what stops a static painting
--- reading as a still image pasted behind a live UI.
function sprite.backdrop(name, options)
  local image = sprite.images[name]
  if not image then return end
  options = options or {}

  local over = 1.04
  local width, height = image:getDimensions()
  local scale_x = (theme.WIDTH / width) * over
  local scale_y = (theme.HEIGHT / height) * over
  local slack_x = theme.WIDTH * (over - 1) / 2
  local slack_y = theme.HEIGHT * (over - 1) / 2

  love.graphics.setColor(1, 1, 1, options.alpha or 1)
  love.graphics.draw(image,
    -slack_x + (options.drift_x or 0), -slack_y + (options.drift_y or 0),
    0, scale_x, scale_y)
  love.graphics.setColor(1, 1, 1, 1)

  -- A scrim, so the town sits behind the interface rather than competing with
  -- it. Without this the red rooftops fight every piece of cyan text on screen.
  if options.scrim and options.scrim > 0 then
    theme.rect(theme.colour.void, 0, 0, theme.WIDTH, theme.HEIGHT, options.scrim)
  end
end

--- Draw a sprite centred on a point, scaled so its widest side is `size`.
---
--- Centring is what every caller wants — sprites here are icons that pulse and
--- spin, and both look wrong around a top-left origin.
function sprite.draw(name, x, y, size, options)
  local image = sprite.images[name]
  if not image then return end
  options = options or {}

  local scale = size / image:getWidth()
  local colour = options.colour or theme.colour.white
  love.graphics.setColor(colour[1], colour[2], colour[3], options.alpha or 1)

  if options.blend then
    local previous = love.graphics.getBlendMode()
    love.graphics.setBlendMode(options.blend)
    love.graphics.draw(image, x, y, options.angle or 0,
      scale * (options.scale_x or 1), scale * (options.scale_y or 1),
      image:getWidth() / 2, image:getHeight() / 2)
    love.graphics.setBlendMode(previous)
  else
    love.graphics.draw(image, x, y, options.angle or 0,
      scale * (options.scale_x or 1), scale * (options.scale_y or 1),
      image:getWidth() / 2, image:getHeight() / 2)
  end

  love.graphics.setColor(1, 1, 1, 1)
end

--- Draw it twice: once large and additive for the bloom, once normally.
---
--- Cheaper than a real bloom pass and, at this resolution, indistinguishable
--- from one. The glow is what stops the sprites looking pasted onto the panels.
function sprite.draw_glowing(name, x, y, size, options)
  options = options or {}
  local strength = options.glow or 0.5
  if strength > 0 then
    sprite.draw(name, x, y, size * 1.35, {
      angle = options.angle,
      colour = options.glow_colour or options.colour,
      alpha = strength * 0.5,
      blend = "add",
      scale_x = options.scale_x,
      scale_y = options.scale_y,
    })
  end
  sprite.draw(name, x, y, size, options)
end

return sprite
