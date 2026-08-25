--- The login screen: a mnemonic gets you in.
---
--- ## What this is, and what it is not
---
--- It is a session gate. It decides which wallet the window is showing and
--- keeps the screens behind it until a phrase is entered.
---
--- It is **not** encryption, and the screen says so in as many words. The store
--- on disk is plain JSONL exactly as it was before, and anyone with the disk has
--- the keys whether or not this screen has been passed. A lock that implies more
--- safety than it provides is worse than no lock at all, so it is labelled
--- honestly rather than dressed up.
---
--- ## The phrase is checked before anything is written
---
--- `validate-mnemonic` says whether it is a phrase at all and why not; `derive`
--- turns it into an address without touching the store. Only once there is an
--- address does the model decide between selecting a wallet it already knows and
--- importing a new one — so a typo produces a message, never a stray account.
--- Those two commands exist for exactly this.

local theme = require("ui.theme")
local anim = require("ui.anim")
local sprite = require("ui.sprite")
local widgets = require("ui.widgets")
local sound = require("ui.sound")

local login = {}
login.__index = login

--- A mnemonic is a password, and it is never drawn.
---
--- Not maskable-with-a-reveal-button: *never*. It is the whole wallet in twelve
--- words, and a "show" toggle is one shoulder or one screenshot away from being
--- the last mistake somebody makes. The two things a person legitimately needs
--- are covered without ever putting it on screen: PASTE to bring one in, COPY
--- to take a freshly minted one out.
---
--- Word count is the feedback that replaces seeing it — twelve or twenty-four
--- is the thing being counted, and knowing which you are at is enough.
local MASK = "*"

--- Clean up a phrase that arrived from a clipboard or a paste.
function login.tidy(text)
  return (text:gsub("^%s+", ""):gsub("%s+$", ""):gsub("%s+", " "))
end

function login.new()
  return setmetatable({
    phrase = "",
    time = 0,
    entrance = anim.Tween.new(0, 1, 0.7, anim.expo_out),
    minted = false,   -- this phrase was just generated, so it can be copied out
    copied = false,   -- and has been taken somewhere safe
    shake = 0,
  }, login)
end

function login:update(dt)
  self.time = self.time + dt
  self.entrance:update(dt)
  self.shake = self.shake * math.exp(-9 * dt)
end

function login:type_into(text)
  -- A tick per keystroke. The field shows asterisks and nothing else moves,
  -- so without it the only confirmation that a key registered is counting the
  -- stars — and this is the one screen where a person is typing blind.
  sound.play("type")
  -- A pasted mnemonic often arrives with newlines in it.
  self.minted = false
  self.phrase = self.phrase .. text:gsub("[\n\r\t]", " ")
end

function login:backspace()
  self.phrase = self.phrase:sub(1, -2)
end

--- Delete the last whole word, which is what backspacing a mnemonic wants.
function login:backword()
  self.phrase = self.phrase:gsub("%s*%S*%s*$", "")
end

function login:words()
  local count = 0
  for _ in self.phrase:gmatch("%S+") do count = count + 1 end
  return count
end

--- What to draw in the field: never the words.
function login:shown()
  return (self.phrase:gsub("%S", MASK))
end

--- Take a phrase from the clipboard. Returns whether there was one.
---
--- The button and Ctrl+V both come here. They used to be separate: the button
--- tidied the text and cleared `minted`, the key assigned the phrase raw and
--- left `minted` alone — so pasting over a freshly minted phrase left the
--- screen still offering to COPY it, and copying would have handed back the
--- pasted one under a banner describing the minted one.
function login:paste(text)
  if not text or text:gsub("%s", "") == "" then return false end
  self.phrase = login.tidy(text)
  self.minted = false
  self.copied = false
  return true
end

--- Wipe the phrase and everything that describes it.
---
--- Called on the way out as well as on the way in. Dropping the screen and
--- building a new one would clear it just as well, but that leaves the rule
--- "no mnemonic outlives the session" as a property of whoever remembers to
--- rebuild the screen. Here it is one call with a test on it.
---
--- `minted` and `copied` go too: they describe a phrase that no longer exists,
--- and a stale `minted` is the difference between the button offering COPY and
--- offering PASTE.
function login:forget()
  self.phrase = ""
  self.minted = false
  self.copied = false
end

--- Try to get in. Returns whatever the model returned.
function login:submit(model)
  local account = model:login(self.phrase)
  if account then
    sound.play("unlock")
    -- The phrase has done its job. Nothing keeps it after this.
    self:forget()
  else
    sound.play("deny")
    self.shake = 5
  end
  return account
end

--- Where the gate's parts sit, for either way up.
---
--- Pure, so the arithmetic is testable headlessly. Landscape is the screen
--- exactly as it shipped. Portrait is not landscape squeezed: the doors that
--- stood side by side stack — at 270 wide they overlapped, and the one drawn
--- second sat on top of the other — the phrase box takes the width it was
--- starving for between two 50-pixel margins, and the parts spread down the
--- column instead of crowding the top half of it.
function login.places(width, height)
  if height > width then
    local inner = { x = 20, w = width - 40 }
    return {
      logo_y = 92,
      logo_size = 54,
      -- The name on two lines: sixteen big-font characters span the whole of
      -- a 270-pixel row, and a title that touches both edges reads as a
      -- rendering accident rather than a name.
      title = { "CAUSEWAYBAY", "BANK" },
      title_y = 134,
      title_line_h = 24,
      frame = { x = inner.x, y = 208, w = inner.w, h = 64 },
      enter = { x = inner.x, y = 288, w = inner.w, h = 21 },
      mint = { x = inner.x, y = 314, w = inner.w, h = 21 },
      note_y = 348,
      note = { x = inner.x + 4, w = inner.w - 8 },
      honesty_y = height - 34,
    }
  end
  return {
    logo_y = 42,
    logo_size = 46,
    title = { "CAUSEWAYBAY BANK" },
    title_y = 68,
    title_line_h = 24,
    frame = { x = 50, y = 104, w = width - 100, h = 64 },
    enter = { x = 50, y = 178, w = 178, h = 21 },
    mint = { x = width - 228, y = 178, w = 178, h = 21 },
    note_y = 210,
    note = { x = 40, w = width - 80 },
    honesty_y = 230,
  }
end

function login:draw(model, state, springs)
  local t = self.entrance.value
  local width, height = theme.WIDTH, theme.HEIGHT
  local at = login.places(width, height)

  local shake_x = anim.shake_offset(self.time, self.shake)
  love.graphics.push()
  love.graphics.translate(shake_x, 0)

  -- The vault, breathing, above the whole thing.
  local bob = math.sin(self.time * 1.2) * 2
  sprite.draw_glowing("logo", width / 2, at.logo_y + bob - (1 - t) * 20, at.logo_size * t, {
    angle = math.sin(self.time * 0.5) * 0.05,
    glow = 0.45 + 0.3 * math.sin(self.time * 3),
    glow_colour = theme.colour.cyan,
  })

  for i, line in ipairs(at.title) do
    theme.text_centred(line, width / 2, at.title_y + (i - 1) * at.title_line_h,
      theme.colour.cyan, theme.font.big, t)
  end

  -- ------------------------------------------------------------ the phrase
  local box = widgets.frame(at.frame.x, at.frame.y, at.frame.w, at.frame.h,
    "MNEMONIC", { alpha = t })

  local count = self:words()
  -- Drawn by hand rather than through widgets.field: the mask, the word count
  -- and the cursor are particular to this one input.
  local field = { x = box.x, y = box.y + 12, w = box.w, h = 21 }
  theme.rect(theme.colour.void, field.x, field.y, field.w, field.h, 0.9)
  theme.outline(theme.colour.cyan, field.x, field.y, field.w, field.h, 0.7 * t)

  local shown = self:shown()
  local font = theme.font.small
  local room = field.w - 10
  while theme.width(shown, font) > room and #shown > 0 do
    shown = shown:sub(2)
  end
  if self.phrase == "" then
    -- Trimmed like the phrase itself: the hint used to be the one string in
    -- the field allowed to run out of it and off the canvas.
    local hint = "twelve or twenty-four words…"
    while theme.width(hint, font) > room and #hint > 1 do
      hint = hint:sub(1, -2)
    end
    theme.text(hint, field.x + 5, field.y + 4, theme.colour.faint, font, t)
  else
    theme.text(shown, field.x + 5, field.y + 4, theme.colour.text, font, t)
  end
  if (self.time * 2) % 2 < 1.2 then
    theme.rect(theme.colour.cyan,
      math.min(field.x + 5 + theme.width(shown, font), field.x + field.w - 6),
      field.y + 4, 3, 11, t)
  end

  -- The count is the feedback that matters while typing: a phrase is twelve
  -- words or twenty-four, and knowing which one you are at beats knowing
  -- nothing until you press enter.
  local ok_count = count == 12 or count == 15 or count == 18 or count == 21 or count == 24
  theme.text(("%d WORDS"):format(count), box.x, box.y + 40,
    ok_count and theme.colour.green or theme.colour.faint, theme.font.small, t)

  -- The clipboard goes both ways, and which way depends on where this phrase
  -- came from. One you already have is pasted *in*; one just minted has to be
  -- copied *out* before it is used, because it exists nowhere else yet.
  local action = { x = box.x + box.w - 76, y = box.y + 38, w = 76, h = 17 }
  if self.minted then
    if widgets.button(springs, "logincopy", action, "COPY", state,
        { colour = theme.colour.gold }) then
      love.system.setClipboardText(self.phrase)
      self.copied = true
    end
  elseif widgets.button(springs, "loginpaste", action, "PASTE", state,
      { colour = theme.colour.cyan }) then
    self:paste(love.system.getClipboardText())
  end

  -- ------------------------------------------------------------- the doors
  local enter = at.enter
  if widgets.button(springs, "enter", enter, "UNLOCK", state,
      { colour = theme.colour.green, disabled = self.phrase == "" }) then
    self:submit(model)
  end

  local mint = at.mint
  if widgets.button(springs, "mint", mint, "NEW MNEMONIC", state,
      { colour = theme.colour.gold }) then
    local phrase = model:offer_mnemonic(12)
    if phrase then
      self.phrase = phrase
      self.minted = true
      self.copied = false
    end
  end

  -- A freshly minted phrase is shown, not silently adopted: a wallet whose
  -- mnemonic was never read is a wallet nobody can recover.
  if self.minted then
    local colour = self.copied and theme.colour.green or theme.colour.gold
    theme.rect(theme.colour.void, at.note.x, at.note_y - 2, at.note.w, 19, 0.85)
    theme.outline(colour, at.note.x, at.note_y - 2, at.note.w, 19, 0.8)
    theme.text_centred(
      self.copied and "copied - store it safely, then UNLOCK"
        or "a new phrase - COPY it before you unlock",
      width / 2, at.note_y, colour, theme.font.small, t)
  else
    theme.text_centred("ENTER unlocks · CTRL+V pastes", width / 2, at.note_y,
      theme.colour.faint, theme.font.small, t)
  end

  -- The honesty line. Not decoration: this screen looks like a lock, and it is
  -- not one, so it says which.
  -- Two lines, because one was 61 characters and the canvas is 60 wide — it
  -- ran off the right edge, which is a poor look for the sentence whose whole
  -- job is to be read.
  theme.text_centred("a session gate, not encryption", width / 2, at.honesty_y,
    theme.colour.faint, theme.font.small, t * 0.8)
  theme.text_centred("keys stay unencrypted on disk", width / 2, at.honesty_y + 14,
    theme.colour.faint, theme.font.small, t * 0.8)

  love.graphics.pop()
end

return login
