--- Causewaybay Wallet, as a game.
---
--- ⚠️  EDUCATIONAL SOFTWARE. Keys are stored unencrypted on disk.
---
--- The window over the same wallet the CLI drives. There is no cryptography
--- here, no store, no argument parsing — `causewaybay.open` and method calls,
--- exactly as `luacli/README.md` describes. LÖVE embeds LuaJIT, which is the
--- one thing that binding requires, so the modules load unchanged.
---
--- ## How a frame is put together
---
--- `model.lua` holds every decision and touches no LÖVE call, so the tests can
--- drive it headlessly. This file draws that model and turns clicks into calls
--- on it. Anything that reaches a node goes through `worker.lua` on a second
--- thread, so the window keeps animating while a node thinks.
---
--- Everything renders to a 480×270 canvas that is scaled up by a whole number
--- with nearest-neighbour filtering (see `ui/theme.lua`). The 8-bit look is not
--- a filter over a modern UI; the UI genuinely is that size.

local theme = require("ui.theme")
local anim = require("ui.anim")
local sprite = require("ui.sprite")
local widgets = require("ui.widgets")
local particles = require("ui.particles")
local Boot = require("boot")
local Model = require("model")

-- The repository root, so both this and the worker thread can find `luacli`.
-- `love.filesystem` is sandboxed and cannot reach it, so the real path is what
-- gets used, and `package.path` is set before anything is required.
local ROOT = love.filesystem.getSource() .. "/.."
package.path = ROOT .. "/luacli/?.lua;" .. ROOT .. "/luacli/?/init.lua;" .. package.path

local causewaybay = require("causewaybay")
local json = require("causewaybay.json")

local game = {
  time = 0,
  model = nil,
  error = nil, -- fatal: the wallet could not be opened at all
  springs = nil,
  fx = nil,
  stars = nil,
  shake = 0,
  entrance = nil,
  screen_slide = nil,
  balance_shown = 0,
  confirm_t = 0,
  toast = nil,
  boot = nil, -- the MSX-style boot sequence, until it hands over
}

-- Positions the drawing code publishes and the update code reads. Declared
-- here because `love.update` is defined above the screens that set them, and a
-- local declared further down would not be in scope there.
local coin_target = { x = theme.WIDTH / 2, y = 90 }  -- where earned coins fly to
local rocket = { x = theme.WIDTH / 2, y = 120 }      -- what the exhaust comes out of

-- ------------------------------------------------------------------- startup

--- Start the worker and hand back the `jobs` interface the model expects.
local function start_worker(home, network)
  -- Relative: `newThread` reads through love.filesystem, which is rooted at
  -- the game directory and cannot see an absolute path outside it.
  local thread = love.thread.newThread("worker.lua")
  thread:start(ROOT, home or "", network or "")

  local requests = love.thread.getChannel("cwb.requests")
  local answers = love.thread.getChannel("cwb.answers")

  return {
    thread = thread,
    submit = function(request) requests:push(request) end,
    poll = function()
      local answer = answers:pop()
      if not answer then return nil end
      -- The worker sends JSON rather than a table, so `json.null` survives the
      -- crossing as itself instead of an empty table.
      local envelope = answer.envelope or json.decode(answer.json)
      return answer.id, envelope
    end,
  }
end

function love.load()
  theme.load()
  sprite.load()
  love.keyboard.setKeyRepeat(true)

  game.springs = widgets.Springs.new()
  game.fx = particles.System.new(700)
  game.stars = particles.Stars.new(110, theme.WIDTH, theme.HEIGHT)
  game.entrance = anim.Tween.new(0, 1, 0.9, anim.expo_out)
  game.screen_slide = anim.Spring.new(0, 240, 0.62)

  local home = os.getenv("CAUSEWAYBAY_HOME")
  local wallet, err = causewaybay.open({
    home = home,
    -- The window asks its own questions and only sends `yes` on a request it
    -- has already confirmed, so the wallet must not assume one.
    yes = false,
  })

  if not wallet then
    game.error = err
    game.boot = Boot.new(nil, err)
    return
  end

  game.boot = Boot.new(wallet, nil)
  game.model = Model.new(wallet, start_worker(home, nil))
  game.model:say("Ready.")
end

function love.quit()
  -- Let the worker finish rather than leaving a thread blocked on `demand`.
  if game.model and game.model.jobs then
    love.thread.getChannel("cwb.requests"):push("quit")
  end
end

-- -------------------------------------------------------------------- update

--- React to what the model says happened, with light and noise.
local function celebrate(event)
  local cx, cy = theme.WIDTH / 2, theme.HEIGHT / 2
  if event == "created" then
    game.fx:burst(cx, cy - 20, { count = 30, speed = 200, colour = { 1, 0.85, 0.3 },
      sprite = "spark", size = 4, life = 0.9, gravity = 180 })
    game.toast = { text = "WALLET CREATED", colour = theme.colour.gold, life = 2.2 }
  elseif event == "balance" then
    game.fx:coins(cx, theme.HEIGHT - 60, coin_target, 14)
  elseif event == "sent" then
    game.fx:confetti(cx, cy, 70)
    game.fx:burst(cx, cy, { count = 40, speed = 320, colour = { 0.4, 1, 0.6 }, life = 1.1 })
    game.toast = { text = "SENT", colour = theme.colour.green, life = 2.4 }
  elseif event == "error" then
    game.shake = 6
    game.fx:burst(cx, cy, { count = 22, speed = 260, colour = { 1, 0.3, 0.4 },
      life = 0.6, gravity = 320 })
  elseif event == "selected" or event == "network" then
    game.fx:burst(cx, cy, { count = 12, speed = 140, colour = { 0.3, 0.94, 1 }, life = 0.5 })
  end
end

function love.update(dt)
  -- A frame that ran long — the window was dragged, the wallet blocked — must
  -- not be integrated in one step. Everything here is exponential and stable,
  -- but a 2-second dt would still teleport particles across the screen.
  dt = math.min(dt, 1 / 20)
  game.time = game.time + dt

  if game.boot then
    game.boot:update(dt)
    if game.boot:complete() then
      game.boot = nil
      -- The entrance starts now rather than at load, so the UI animates in
      -- as the boot screen hands over instead of having played behind it.
      game.entrance:restart()
      game.fx:burst(theme.WIDTH / 2, theme.HEIGHT / 2,
        { count = 34, speed = 300, colour = { 0.3, 0.94, 1 }, life = 0.9 })
    end
    return
  end

  game.entrance:update(dt)
  game.springs:update(dt)
  game.screen_slide:update(dt)
  game.stars:update(dt, 1 + (game.model and game.model:busy() and 6 or 0))
  game.fx:update(dt)
  game.shake = game.shake * math.exp(-9 * dt)

  if game.toast then
    game.toast.life = game.toast.life - dt
    if game.toast.life <= 0 then game.toast = nil end
  end

  if not game.model then return end

  game.model:pump()
  for _, event in ipairs(game.model:drain()) do
    celebrate(event)
  end

  -- The confirmation dialog fades in, and snaps out.
  local wanted = game.model.confirm and 1 or 0
  game.confirm_t = anim.approach(game.confirm_t, wanted, wanted == 1 and 14 or 22, dt)

  -- Count the balance up rather than replacing the number, which turns a value
  -- arriving into an event you can watch.
  local target = tonumber(game.model.balance and game.model.balance.balance) or 0
  game.balance_shown = anim.approach(game.balance_shown, target, 7, dt)

  -- Exhaust under the rocket, so the two read as one object. Harder while a
  -- transaction is in flight, which is the screen's only progress indicator
  -- that does not need words.
  if game.model.screen == "send" then
    local rate = game.model:busy() and 3 or 1
    for _ = 1, rate do
      if math.random() < 0.65 then
        game.fx:embers(rocket.x, rocket.y + 18, 12, 1, 1)
      end
    end
  end
end

-- --------------------------------------------------------------------- input

local function mouse_state()
  local mx, my = theme.to_canvas(love.mouse.getPosition())
  return { mouse_x = mx, mouse_y = my, clicked = game.clicked }
end

function love.mousepressed(_, _, button)
  if game.boot then
    game.boot:skip()
    return
  end
  if button == 1 then game.clicked = true end
end

function love.textinput(text)
  if game.boot then return end
  if game.model then game.model:type_into(text) end
end

function love.keypressed(key)
  if game.boot then
    -- Any key: the first skips the sequence, the second hands over. Escape
    -- still quits, because being trapped in a boot screen is not charming.
    if key == "escape" then love.event.quit() end
    game.boot:skip()
    return
  end

  if key == "escape" then
    if game.model and game.model.confirm then
      game.model:cancel_send()
    else
      love.event.quit()
    end
    return
  end

  if not game.model then return end
  local model = game.model

  if model.confirm then
    -- The dialog owns the keyboard while it is up.
    if key == "return" or key == "kpenter" then model:confirm_send() end
    return
  end

  local ctrl = love.keyboard.isDown("lctrl", "rctrl", "lgui", "rgui")
  if ctrl and key == "v" then
    -- LÖVE delivers Cmd/Ctrl+V as a keypress, not as textinput, so a paste
    -- has to be handled here or it silently does nothing.
    paste_from_clipboard(model, model.focus)
    return
  end
  if ctrl and key == "c" then
    local account = model.wallets[model.selected]
    for _, entry in ipairs(model.wallets) do
      if entry.address == model.active then account = entry end
    end
    if account then copy_to_clipboard(model, account.address, "address") end
    return
  end

  if key == "tab" then
    model:next_field()
  elseif key == "backspace" then
    model:backspace()
  elseif key == "return" or key == "kpenter" then
    if model.screen == "send" then
      model:begin_send(model.form.to, model.form.amount)
    elseif model.screen == "wallets" then
      model:fetch_balance()
    end
  elseif key == "1" or key == "2" or key == "3" then
    -- Only when a text field is not the point of the screen.
    if model.screen ~= "send" then
      model:go(Model.SCREENS[tonumber(key)])
      game.screen_slide:set(1):to(0)
    end
  elseif key == "up" or key == "down" then
    if model.screen == "wallets" and #model.wallets > 0 then
      local step = key == "down" and 1 or -1
      model.selected = math.max(1, math.min(#model.wallets, model.selected + step))
    end
  end
end

-- ---------------------------------------------------------------------- draw

--- A big, dim sprite drifting behind the content.
---
--- The screens are top-heavy — a two-wallet list leaves most of the window
--- empty — and an empty half looks unfinished rather than spacious. This is
--- decoration that cannot be mistaken for information: too dim to read, too
--- slow to distract, and behind everything.
local function draw_watermark(name, cy)
  local bob = math.sin(game.time * 0.5) * 4
  sprite.draw(name, theme.WIDTH / 2, cy + bob, 120, {
    angle = math.sin(game.time * 0.22) * 0.10,
    alpha = 0.055 + 0.02 * math.sin(game.time * 0.9),
    blend = "add",
  })
end

local function draw_header(model)
  local t = game.entrance.value
  theme.rect(theme.colour.deep, 0, 0, theme.WIDTH, 34)
  theme.rule(0, 34, theme.WIDTH, theme.colour.cyan_dark, 0.8 * t)

  local bob = math.sin(game.time * 2) * 1.5
  sprite.draw_glowing("logo", 20, 17 + bob, 26, {
    angle = math.sin(game.time * 0.7) * 0.06,
    glow = 0.6 + 0.25 * math.sin(game.time * 3),
    glow_colour = theme.colour.cyan,
  })

  theme.text("CAUSEWAYBAY", 38, 3, theme.colour.cyan, theme.font.body, t)
  theme.text("BANK", 38, 18, theme.colour.dim, theme.font.small, t)

  local network = model and model.info and model.info.network or "…"
  local chain = model and model.info and model.info.chain_id or ""
  theme.text_right(("%s  ·  chain %s"):format(network, chain),
    theme.WIDTH - 8, 11, theme.colour.dim, theme.font.small, t)

  -- A spinner while the node is thinking, so "busy" is never just a word.
  if model and model:busy() then
    local r = 4
    for i = 0, 5 do
      local a = game.time * 6 + i * 0.9
      local fade = 1 - (i / 6)
      theme.set(theme.colour.cyan, fade * 0.9)
      love.graphics.rectangle("fill",
        theme.WIDTH - 14 + math.cos(a) * r, 24 + math.sin(a) * r, 2, 2)
    end
  end
end

local function draw_tabs(model, state)
  local labels = { wallets = "WALLETS", send = "SEND", network = "NETWORK" }
  local w = 74
  for i, name in ipairs(Model.SCREENS) do
    local box = { x = 8 + (i - 1) * (w + 4), y = 38, w = w, h = 19 }
    local active = model.screen == name
    local spring = game.springs:get("tab." .. name, 240, 0.6)
    spring:to(active and 1 or 0)

    local hovered = widgets.hit(state.mouse_x, state.mouse_y, box)
    if hovered and state.clicked and not active then
      model:go(name)
      game.screen_slide:set(1):to(0)
    end

    local fill = theme.mix(theme.colour.deep, theme.colour.panel, spring.value)
    theme.rect(fill, box.x, box.y, box.w, box.h)
    theme.rect(theme.colour.cyan, box.x, box.y + box.h - 1, box.w * spring.value, 1)
    local ink = theme.mix(hovered and theme.colour.dim or theme.colour.faint,
      theme.colour.cyan, spring.value)
    theme.text_centred(labels[name] .. " " .. i, box.x + box.w / 2, box.y + 3, ink,
      theme.font.small)
  end
end

--- Put text on the system clipboard, reporting it in the status line.
---
--- An address is 42 characters of hex that a person cannot retype correctly,
--- so copying it is not a convenience — it is the only realistic way to use
--- one. `love.system` provides this on every platform LÖVE runs on.
local function copy_to_clipboard(model, text, what)
  if not text or text == "" then return false end
  love.system.setClipboardText(text)
  model:say(("%s copied"):format(what))
  game.toast = { text = "COPIED", colour = theme.colour.cyan, life = 1.4 }
  return true
end

--- Take whatever is on the clipboard, if it is worth taking.
local function paste_from_clipboard(model, field)
  local text = love.system.getClipboardText()
  if not text or text:gsub("%s", "") == "" then
    model:fail({ code = "usage", message = "the clipboard is empty" })
    return false
  end
  model:set_field(field, text)
  model:say("pasted from the clipboard")
  return true
end

--- The layout every screen shares, so the columns line up between them.
local L = {
  margin = 8,
  -- 68 leaves room above for a frame's title, which sits *on* its top edge and
  -- would otherwise collide with the tab row ending at 57.
  top = 68,
  -- Frames stop here; below is the action bar (230..248) and then the warning
  -- banner (253..270). Worked out once so nothing overlaps by a pixel.
  bottom = 224,
  bar = 230,
  button_h = 18,
  gutter = 8,
}
L.left = L.margin
L.column = 196                                        -- the list column
L.right = L.left + L.column + L.gutter                 -- the detail column
L.right_w = theme.WIDTH - L.right - L.margin

--- Two lines: a label over a quieter value. The unit of most of this UI.
local function stat(x, y, label, value, colour, font)
  theme.text(label, x, y, theme.colour.faint, theme.font.small)
  theme.text(value, x, y + 13, colour or theme.colour.text, font or theme.font.small)
end

local function draw_wallets(model, state, x)
  local height = L.bottom - L.top

  -- ------------------------------------------------------------ the list
  local list = widgets.frame(x + L.left, L.top, L.column, height,
    ("WALLETS %d"):format(#model.wallets))

  if #model.wallets == 0 then
    theme.text_centred("no wallets yet", x + L.left + L.column / 2, L.top + height / 2 - 16,
      theme.colour.faint, theme.font.small)
    theme.text_centred("press + NEW below", x + L.left + L.column / 2,
      L.top + height / 2 - 2, theme.colour.faint, theme.font.small)
  end

  -- Two lines each — a name is not enough to tell two wallets apart, and an
  -- address alone is unreadable.
  local row_h = 30
  local rows = math.floor(list.h / row_h)
  for i, account in ipairs(model.wallets) do
    if i > rows then break end
    local box = { x = list.x, y = list.y + (i - 1) * row_h, w = list.w, h = row_h - 2 }
    local selected = account.address == model.active
    local clicked, hovered, row_x, slide = widgets.row(game.springs, "row" .. i, box, state,
      selected or model.selected == i)
    if clicked then model:select(i) end

    local ink = selected and theme.colour.cyan
      or (hovered and theme.colour.text or theme.colour.dim)
    sprite.draw_glowing("wallet", row_x + 13, box.y + 14, 17, {
      alpha = 0.45 + slide * 0.55,
      glow = selected and 0.55 or 0,
      glow_colour = theme.colour.gold,
    })
    theme.text(account.label, row_x + 26, box.y + 1, ink, theme.font.small)
    theme.text(theme.ellipsis(account.address, 8, 6), row_x + 26, box.y + 14,
      selected and theme.colour.cyan_dark or theme.colour.faint, theme.font.small)
  end

  if #model.wallets > rows then
    theme.text_right(("+%d more"):format(#model.wallets - rows), list.x + list.w,
      list.y + list.h - 4, theme.colour.faint, theme.font.small)
  elseif #model.wallets > 0 then
    theme.text(("%s / %s to move"):format("UP", "DOWN"), list.x,
      list.y + list.h - 4, theme.colour.faint, theme.font.small)
  end

  -- ---------------------------------------------------------- the detail
  local selected = model.wallets[model.selected]
  for _, account in ipairs(model.wallets) do
    if account.address == model.active then selected = account end
  end

  -- The balance, given the room a headline deserves.
  local balance_h = 58
  local card = widgets.frame(x + L.right, L.top, L.right_w, balance_h, "BALANCE")
  coin_target.x, coin_target.y = x + L.right + L.right_w / 2, L.top + 40

  if not selected then
    theme.text_centred("no wallet selected", card.x + card.w / 2, card.y + 16,
      theme.colour.faint, theme.font.small)
  elseif model.balance then
    widgets.readout(card.x, card.y - 2, card.w,
      (("%.4f"):format(game.balance_shown):gsub("0+$", ""):gsub("%.$", "")),
      model.balance.symbol, { alpha = game.entrance.value })
  else
    -- Not a row of dashes pretending to be a number: say what is missing and
    -- how to get it. An unknown balance is a state, not a value.
    theme.text_centred("- - -", card.x + card.w / 2, card.y + 4,
      theme.colour.faint, theme.font.big)
    theme.text_centred("press REFRESH to ask the node", card.x + card.w / 2,
      card.y + 28, theme.colour.faint, theme.font.small)
  end

  -- Its address, in full — the one place it is not abbreviated, because this
  -- is where a person comes to copy it.
  local detail = widgets.frame(x + L.right, L.top + balance_h + L.gutter, L.right_w,
    height - balance_h - L.gutter, "ACCOUNT")

  if selected then
    stat(detail.x, detail.y, "LABEL", selected.label, theme.colour.text)
    -- Where the key came from, as a tag rather than another labelled row —
    -- it is a property of the wallet, not a field of equal weight.
    widgets.chip(detail.x + detail.w - 74, detail.y,
      (selected.source or "?"):upper():gsub("_", " "), theme.colour.magenta)

    theme.text("ADDRESS", detail.x, detail.y + 30, theme.colour.faint, theme.font.small)
    -- Split across two lines: 42 characters do not fit a 230px column, and an
    -- address broken in the middle is still checkable end to end.
    theme.text(selected.address:sub(1, 21), detail.x, detail.y + 45,
      theme.colour.cyan, theme.font.small)
    theme.text(selected.address:sub(22), detail.x, detail.y + 58,
      theme.colour.cyan, theme.font.small)

    -- Right beside the thing it copies, which is the only place a copy button
    -- is unambiguous.
    local copy = { x = detail.x + detail.w - 58, y = detail.y + 26, w = 58, h = 17 }
    if widgets.button(game.springs, "copy", copy, "COPY", state,
        { colour = theme.colour.cyan }) then
      copy_to_clipboard(model, selected.address, "address")
    end
  end

  -- ------------------------------------------------------------ actions
  local refresh = { x = x + L.right, y = L.bar, w = 84, h = L.button_h }
  if widgets.button(game.springs, "refresh", refresh, "REFRESH", state,
      { disabled = model:busy() or #model.wallets == 0 }) then
    model:fetch_balance()
  end

  local new = { x = x + L.left, y = L.bar, w = 84, h = L.button_h }
  if widgets.button(game.springs, "new", new, "+ NEW", state,
      { colour = theme.colour.green }) then
    model:create("")
  end
end

local function draw_send(model, state, x)
  local height = L.bottom - L.top

  -- The rocket gets its own column, so the exhaust has somewhere to go.
  local pad = widgets.frame(x + L.left, L.top, 96, height, "LAUNCH")
  rocket.x, rocket.y = pad.x + pad.w / 2, pad.y + 42
  local thrust = model:busy() and 1 or 0
  sprite.draw_glowing("rocket", rocket.x, rocket.y + math.sin(game.time * 2) * 2, 56, {
    angle = math.sin(game.time * 1.4) * 0.06,
    glow = 0.35 + 0.25 * math.sin(game.time * 4) + thrust * 0.5,
    glow_colour = theme.colour.cyan,
  })

  local form = widgets.frame(x + L.right, L.top, L.right_w, height, "TRANSFER")

  -- The recipient field gives up room to its own buttons: pasting is how an
  -- address realistically gets in here, and typing 42 hex characters is not.
  local to = { x = form.x, y = form.y + 18, w = form.w - 108, h = 19 }
  widgets.field(game.springs, "to", to, model.form.to, "RECIPIENT",
    model.focus == "to", { placeholder = "0x…" })

  local paste = { x = form.x + form.w - 104, y = form.y + 18, w = 56, h = 19 }
  if widgets.button(game.springs, "paste", paste, "PASTE", state,
      { colour = theme.colour.cyan }) then
    paste_from_clipboard(model, "to")
  end

  local clear = { x = form.x + form.w - 44, y = form.y + 18, w = 44, h = 19 }
  if widgets.button(game.springs, "clear", clear, "CLR", state,
      { colour = theme.colour.red, disabled = model.form.to == "" }) then
    model:clear_field("to")
  end

  local amount = { x = form.x, y = form.y + 60, w = 120, h = 19 }
  widgets.field(game.springs, "amount", amount, model.form.amount, "AMOUNT",
    model.focus == "amount", { placeholder = "0.0" })

  if state.clicked then
    if widgets.hit(state.mouse_x, state.mouse_y, to) then model.focus = "to" end
    if widgets.hit(state.mouse_x, state.mouse_y, amount) then model.focus = "amount" end
  end

  -- What it will be sent as, so the network is never a surprise. Right-aligned
  -- inside the frame rather than at a fixed offset, which is what ran it off
  -- the edge when the network name got longer.
  if model.info then
    local name = model.info.network or "?"
    local width = theme.width(name, theme.font.small) + 10
    local _ = width
    theme.text("SENDING ON", form.x, form.y + 90, theme.colour.faint, theme.font.small)
    widgets.chip(form.x + 88, form.y + 88, name, theme.colour.cyan)
  end

  local send = { x = form.x, y = form.y + height - 46, w = form.w, h = 21 }
  if widgets.button(game.springs, "send", send, "SEND >", state,
      { colour = theme.colour.gold, disabled = model:busy() }) then
    model:begin_send(model.form.to, model.form.amount)
  end

  theme.text_centred("CTRL+V pastes · ENTER sends",
    form.x + form.w / 2, form.y + height - 22, theme.colour.faint, theme.font.small)
end

local function draw_network(model, state, x)
  local height = L.bottom - L.top
  local frame = widgets.frame(x + L.margin, L.top, theme.WIDTH - L.margin * 2, height,
    "NETWORK")

  local row_h = 46
  for i, network in ipairs(model:networks()) do
    local box = { x = frame.x, y = frame.y + (i - 1) * (row_h + 6), w = frame.w, h = row_h }
    local current = model.info and model.info.network == network.key
    local clicked, hovered, row_x, slide = widgets.row(game.springs, "net" .. i, box, state,
      current)
    if clicked and not current then model:switch_network(network.key) end

    theme.rect(theme.colour.void, row_x, box.y, box.w, box.h, current and 0.5 or 0.35)
    theme.outline(current and theme.colour.green or theme.colour.raised,
      row_x, box.y, box.w, box.h, current and 0.8 or 0.4)

    sprite.draw_glowing("globe", row_x + 28, box.y + box.h / 2, 32, {
      angle = game.time * (current and 0.5 or 0.15),
      glow = current and 0.6 or 0.15,
      glow_colour = current and theme.colour.green or theme.colour.faint,
      alpha = 0.5 + slide * 0.5,
    })

    local ink = current and theme.colour.green or (hovered and theme.colour.text
      or theme.colour.dim)
    theme.text(network.name, row_x + 54, box.y + 8, ink, theme.font.body)
    stat(row_x + 54, box.y + 26, "CHAIN " .. network.chain_id, "", theme.colour.faint)
    theme.text(network.symbol, row_x + 54 + 90, box.y + 26, theme.colour.faint,
      theme.font.small)

    if current then
      widgets.chip(row_x + box.w - 62, box.y + 8, "ACTIVE", theme.colour.green)
    end
  end

  theme.text_centred("the store is shared - a wallet works on either",
    frame.x + frame.w / 2, frame.y + frame.h - 6, theme.colour.faint, theme.font.small)
end

local function draw_status(model)
  local status = model.status
  if not status then return end
  local colour = status.kind == "error" and theme.colour.red
    or (status.kind == "busy" and theme.colour.amber or theme.colour.dim)
  local x = status.kind == "error" and 112 or 100
  if status.kind == "error" then
    sprite.draw("skull", 103, L.bar + 9, 13, { alpha = 0.9 })
  end
  theme.text(theme.ellipsis(status.text, 30, 0), x, L.bar + 3, colour, theme.font.small)
end

local function draw_confirm(model, state)
  if game.confirm_t < 0.01 then return end
  local plan = model.confirm
  local box = widgets.dialog({ x = 60, y = 70, w = theme.WIDTH - 120, h = 110 },
    game.confirm_t, "CONFIRM TRANSFER")
  if not plan then return end

  sprite.draw_glowing("key", box.x + box.w / 2, box.y + 40, 26, {
    angle = math.sin(game.time * 3) * 0.12,
    glow = 0.7, glow_colour = theme.colour.amber,
  })

  -- The wallet's own summary of a transaction it has already priced.
  local words, line, y = {}, "", box.y + 58
  for word in plan.summary:gmatch("%S+") do
    local candidate = line == "" and word or (line .. " " .. word)
    if theme.width(candidate, theme.font.small) > box.w - 24 then
      words[#words + 1] = line
      line = word
    else
      line = candidate
    end
  end
  words[#words + 1] = line
  for _, text in ipairs(words) do
    theme.text_centred(text, box.x + box.w / 2, y, theme.colour.text, theme.font.small,
      box.eased)
    y = y + 10
  end

  local no = { x = box.x + 16, y = box.y + box.h - 24, w = 70, h = 16 }
  local yes = { x = box.x + box.w - 86, y = box.y + box.h - 24, w = 70, h = 16 }
  if widgets.button(game.springs, "no", no, "CANCEL", state, { colour = theme.colour.red }) then
    model:cancel_send()
  end
  if widgets.button(game.springs, "yes", yes, "SEND IT", state,
      { colour = theme.colour.green }) then
    model:confirm_send()
  end
end

local function draw_toast()
  if not game.toast then return end
  local toast = game.toast
  -- Rises and fades over its life, easing out so it decelerates as it goes.
  local t = 1 - (toast.life / 2.4)
  local rise = anim.expo_out(math.min(1, t * 2.5)) * 14
  local fade = toast.life > 0.6 and 1 or (toast.life / 0.6)
  local scale = 1 + (1 - math.min(1, t * 4)) * 0.4

  love.graphics.push()
  love.graphics.translate(theme.WIDTH / 2, 66 - rise)
  love.graphics.scale(scale, scale)
  theme.text_centred(toast.text, 0, 0, toast.colour, theme.font.big, fade)
  love.graphics.pop()
end

local function draw_fatal()
  sprite.draw_glowing("skull", theme.WIDTH / 2, 90, 48, {
    glow = 0.6 + 0.3 * math.sin(game.time * 4),
    glow_colour = theme.colour.red,
  })
  theme.text_centred("THE WALLET WOULD NOT OPEN", theme.WIDTH / 2, 130,
    theme.colour.red, theme.font.big)
  theme.text_centred("[" .. (game.error.code or "?") .. "]", theme.WIDTH / 2, 152,
    theme.colour.dim, theme.font.small)

  local y = 168
  for line in tostring(game.error.message):gmatch("[^\n]+") do
    theme.text_centred(theme.ellipsis(line, 70, 0), theme.WIDTH / 2, y,
      theme.colour.faint, theme.font.small)
    y = y + 9
    if y > theme.HEIGHT - 20 then break end
  end
end

function love.draw()
  if game.boot then
    theme.frame(function() game.boot:draw() end)
    game.clicked = false
    return
  end

  theme.frame(function()
    -- Cesky Krumlov at dusk, behind everything. Heavily scrimmed: it is a
    -- backdrop, and a wallet's numbers have to win every contrast fight
    -- against it.
    sprite.backdrop("krumlov", {
      alpha = 0.85,
      scrim = 0.68,
      drift_x = math.sin(game.time * 0.09) * 3,
      drift_y = math.cos(game.time * 0.07) * 2,
    })
    game.stars:draw(game.time)

    -- The whole scene shakes on an error, and slides when the screen changes.
    local shake_x, shake_y = anim.shake(game.time, game.shake)
    love.graphics.push()
    love.graphics.translate(math.floor(shake_x), math.floor(shake_y))

    if game.error then
      draw_fatal()
    else
      local model = game.model
      local state = mouse_state()
      -- The entrance eases everything down from above on the first frames.
      local drop = (1 - game.entrance.value) * -30
      local slide = game.screen_slide.value * 26

      love.graphics.push()
      love.graphics.translate(0, drop)
      draw_header(model)
      if model then
        draw_tabs(model, state)
        if model.screen == "wallets" then
          draw_wallets(model, state, slide)
        elseif model.screen == "send" then
          draw_send(model, state, slide)
        else
          draw_network(model, state, slide)
        end
        draw_status(model)
      end
      love.graphics.pop()

      game.fx:draw(sprite.images)
      draw_toast()
      if model then draw_confirm(model, state) end
    end

    love.graphics.pop()

    -- The warning every front end shows, in the one place it cannot be missed.
    theme.rect(theme.colour.void, 0, L.bar - 4, theme.WIDTH, theme.HEIGHT - L.bar + 4, 0.5)
    theme.rect(theme.colour.void, 0, theme.HEIGHT - 17, theme.WIDTH, 17, 0.9)
    theme.rule(0, theme.HEIGHT - 17, theme.WIDTH, theme.colour.raised, 0.5)
    theme.text_centred("EDUCATIONAL · KEYS ARE STORED UNENCRYPTED", theme.WIDTH / 2,
      theme.HEIGHT - 16, theme.colour.faint, theme.font.small)

    if #sprite.missing > 0 then
      theme.text("assets missing: " .. table.concat(sprite.missing, " "), 4, 48,
        theme.colour.amber, theme.font.small)
    end

    theme.scanlines(0.10)
    theme.vignette(0.4)
  end)

  -- Consumed once per frame, after every widget has had a chance to see it.
  game.clicked = false
end

function love.resize(w, h)
  game.stars:resize(theme.WIDTH, theme.HEIGHT)
  local _ = w, h
end

-- ----------------------------------------------------------------- capturing
--
-- A GUI that can only be checked by looking at it is a GUI nobody checks. This
-- makes a frame reviewable from a terminal:
--
--     CWB_SHOT=/tmp/wallet.png CWB_SHOT_AFTER=2.5 love lovegui
--
-- Runs normally for that many seconds — so the entrance has settled and any
-- effects have played — writes a PNG, and quits. `CWB_SHOT_SCREEN` picks which
-- screen to grab, and `CWB_SHOT_KEYS` replays keypresses first, so a shot of
-- the confirmation dialog does not need a human at the keyboard.

local shot = {
  path = os.getenv("CWB_SHOT"),
  after = tonumber(os.getenv("CWB_SHOT_AFTER") or "2"),
  screen = os.getenv("CWB_SHOT_SCREEN"),
  keys = os.getenv("CWB_SHOT_KEYS"),
  taken = false,
}

--- Drive the game the way a person would, before the shot is taken.
---
--- The boot screen owns the keyboard while it is up, so a replay that started
--- typing straight away had its keystrokes swallowed and produced a shot of an
--- empty form. It is dismissed outright here instead: a screenshot of a screen
--- is not the place to also exercise the boot sequence.
local function replay()
  game.boot = nil
  game.entrance:restart()
  if shot.screen and game.model then
    game.model:go(shot.screen)
    game.screen_slide:set(0)
  end
  if not shot.keys then return end
  for step in shot.keys:gmatch("[^,]+") do
    local text = step:match("^type:(.*)$")
    if text then
      love.textinput(text)
    else
      love.keypressed(step)
    end
  end
end

if shot.path then
  local update, draw = love.update, love.draw
  local elapsed, replayed = 0, false

  love.update = function(dt)
    update(dt)
    elapsed = elapsed + dt
    -- Replayed a beat in, so the model exists and the entrance has begun.
    if not replayed and elapsed > 0.35 then
      replayed = true
      replay()
    end
  end

  love.draw = function()
    draw()
    if elapsed >= shot.after and not shot.taken then
      shot.taken = true
      love.graphics.captureScreenshot(function(data)
        -- love.filesystem is sandboxed, so the PNG is encoded in memory and
        -- written with plain io to wherever the caller asked for.
        local file = io.open(shot.path, "wb")
        if file then
          file:write(data:encode("png"):getString())
          file:close()
        end
        love.event.quit()
      end)
    end
  end
end
