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
local sound = require("ui.sound")
local card = require("ui.card")
local Launch = require("ui.launch")
local Boot = require("boot")
local Login = require("login")

-- The repository root, so both this and the worker thread can find `luacli`.
-- `love.filesystem` is sandboxed and cannot reach it, so the real path is what
-- gets used.
--
-- This has to happen before anything that reaches the binding is required, and
-- `model.lua` now does — it loads `export.lua`, which needs the JSON encoder.
-- Requiring it above this line failed at startup with a missing module, and
-- neither the tests nor `make lint` could see it: the tests set the path
-- themselves before requiring anything, and lint byte-compiles without ever
-- running a `require`. It is only reachable by starting the game.
local ROOT = love.filesystem.getSource() .. "/.."
package.path = ROOT .. "/luacli/?.lua;" .. ROOT .. "/luacli/?/init.lua;" .. package.path

local causewaybay = require("causewaybay")
local json = require("causewaybay.json")
local Model = require("model")

--- The layout every screen shares, so the columns line up between them.
---
--- Computed, not written down: `ui/layout.lua` holds the arithmetic for both
--- orientations, and the LAYOUT button swaps which one this is.
local layout = require("ui.layout")
local L = layout.compute(theme.WIDTH, theme.HEIGHT)

--- Remembered across launches, like the session: a wallet someone stood on
--- its side should come back that way.
local LAYOUT_FILE = "layout"

--- What the per-row quick-send button says.
---
--- Four characters, and not for brevity's sake: the wallet list is a 196-pixel
--- column that already holds a name and an address, and the word "QUICKSEND"
--- is 73 pixels of it. Spelling it out would have cost the address half its
--- characters, and an address is the one thing on that row nobody can afford
--- to have shortened — it is how you tell two wallets apart.
---
--- The amount is not on the button because it is on the confirmation, which is
--- where it matters: nothing is signed until that dialog is answered, and it
--- names the amount, the recipient and the wallet being spent from.
local QUICK_LABEL = "SEND"

--- Turn the whole game on its side, or back.
---
--- `remember` is false for a turn nobody asked for — the screenshot harness
--- photographing the tall layout — so taking a picture cannot decide which way
--- up the game opens next time.
local function set_orientation(portrait, remember)
  if portrait == L.portrait then return end
  theme.set_size(theme.HEIGHT, theme.WIDTH)
  L = layout.compute(theme.WIDTH, theme.HEIGHT)
  if remember ~= false then
    pcall(love.filesystem.write, LAYOUT_FILE, portrait and "portrait" or "landscape")
  end
  -- A windowed window turns with the canvas, keeping its scale; fullscreen
  -- stays as it is and the transform letterboxes, as it always has.
  if not love.window.getFullscreen() then
    local _, _, flags = love.window.getMode()
    local scale = math.max(1, math.floor(math.min(
      love.graphics.getWidth() / theme.HEIGHT, love.graphics.getHeight() / theme.WIDTH)))
    flags.minwidth, flags.minheight = theme.WIDTH, theme.HEIGHT
    love.window.setMode(theme.WIDTH * scale, theme.HEIGHT * scale, flags)
  end
end

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
  -- The same tween, for the dialog that asks before a file is written.
  write_t = 0,
  toast = nil,
  boot = nil,  -- the MSX-style boot sequence, until it hands over
  login = nil, -- the mnemonic gate, until a session is open
  list_rows = 6,
  -- The first grid row showing on the NETWORK screen. Whole rows, so a row is
  -- never half on screen; clamped where the frame is drawn, since only the
  -- frame knows how many of them fit.
  net_scroll = 0,
  -- The row the screen last scrolled to on its own, so it does so once per
  -- change rather than fighting the wheel every frame.
  net_revealed = nil,
  -- A confirmed send launches the rocket. See LAUNCH below for why the
  -- animation has a floor and the outcome waits behind it.
  launch = nil,
  -- Which card is on screen, and how far through turning over it is. See
  -- CARD_TURN. `shown` and `next` are the account tables themselves rather
  -- than addresses: two accounts in one store can share an address (an import
  -- of a phrase that was already there), and looking the label back up by
  -- address then finds whichever of them came last.
  face = { shown = nil, next = nil, turn = 1, key = nil, index = nil, dir = 1 },
  -- Seconds left on an armed destructive button. See ARM_TIME.
  armed = {},
  -- A window mode change waiting for the frame to end. See `ask_fullscreen`.
  window = nil,
  -- Where the screenshot harness is pointing, when there is one. See CAPTURING.
  pointer = nil,
}

--- Ask for windowed or fullscreen, to happen between frames rather than now.
---
--- Never from inside `love.draw`. Every screen here is drawn to a canvas —
--- `theme.frame` sets it and unsets it around the whole scene — and changing
--- the window mode in the middle of that pass tears the render target out from
--- under the drawing that has not happened yet, which is a crash and not a
--- resize. The keyboard shortcut always worked because `love.keypressed` runs
--- outside the pass; the button beside it did not, because a button is drawn.
---
--- So both go through here, and `love.update` performs it before it does
--- anything else.
local function ask_fullscreen(wanted)
  game.window = { fullscreen = wanted and true or false }
end

--- Fullscreen as far as the interface is concerned, including a change that
--- has been asked for and not yet applied — so the label under the pointer
--- does not flick back to what it said before the click for one frame.
local function fullscreen_now()
  if game.window then return game.window.fullscreen end
  return love.window.getFullscreen()
end

--- How long a destructive button stays armed after the first press.
---
--- LOGOUT deletes the store, and it should not happen because a pointer was in
--- the wrong place. A button that says what it is about to do, and waits, is
--- the smallest thing that makes a misclick harmless. It disarms itself, so
--- walking away does not leave the wallet one stray click from being wiped.
---
--- SAVE and KEYS used to arm the same way and no longer do: they write files
--- somewhere, and *where* is the part a person needs told. That does not fit
--- on a button, so those two ask in a dialog. See `draw_write`.
local ARM_TIME = 4

--- How long one card takes to swipe out and the next to swipe in.
---
--- Short enough that holding an arrow key is not a queue of animations, long
--- enough that both cards are on screen together for a beat you can actually
--- see. 0.32 with a front-loaded curve was neither: the card being replaced
--- was gone before the eye found it, and the whole thing read as a new card
--- appearing rather than two cards moving.
local CARD_SWIPE = 0.42

-- Positions the drawing code publishes and the update code reads. Declared
-- here because `love.update` is defined above the screens that set them, and a
-- local declared further down would not be in scope there.
local coin_target = { x = theme.WIDTH / 2, y = 90 }  -- where earned coins fly to
local card_target = { x = theme.WIDTH / 2, y = 140 } -- the middle of the card
local rocket = { x = theme.WIDTH / 2, y = 120 }      -- what the exhaust comes out of

-- ----------------------------------------------------------------- session
--
-- Remembering that you are logged in, so the phrase is asked for once rather
-- than at every launch.
--
-- What is written is addresses and a label — see `Model:session_snapshot`.
-- Nothing secret, because nothing secret is needed: the gate decides *which*
-- wallets the window shows, and which wallets they are is public. It goes in
-- LÖVE's save directory rather than the wallet's home, so a wipe of the store
-- and a forgetting of the session stay separate acts.

local SESSION_FILE = "session"

local function forget_session()
  pcall(love.filesystem.remove, SESSION_FILE)
end

local function remember_session()
  local snapshot = game.model and game.model:session_snapshot()
  if not snapshot then return forget_session() end
  pcall(love.filesystem.write, SESSION_FILE, json.encode(snapshot))
end

--- Put the remembered session back. False if there is nothing to put back, or
--- what was remembered no longer fits the store — a wipe, or a different home.
local function recall_session()
  if not game.model then return false end
  if not love.filesystem.getInfo(SESSION_FILE) then return false end

  local body = love.filesystem.read(SESSION_FILE)
  local decoded, snapshot = pcall(json.decode, body)
  if not decoded then
    forget_session()
    return false
  end
  if game.model:restore_session(snapshot) then return true end

  -- It did not fit. Drop it rather than trying again next time.
  forget_session()
  return false
end

-- ------------------------------------------------------------------- startup

--- The shared library's file name on this platform.
local function library_name()
  local system = love.system.getOS()
  if system == "OS X" then return "libcausewaybay_ffi.dylib" end
  if system == "Windows" then return "causewaybay_ffi.dll" end
  return "libcausewaybay_ffi.so"
end

--- Where the library is when the game is an application bundle.
---
--- A checkout finds it on its own: the binding walks up from its own file to
--- `rustcli/target`, which is exactly right for `make run`. A bundle has no
--- checkout to walk up to — the Lua is inside a zip, `love.filesystem` is
--- sandboxed and cannot look outside it, and the one thing that *would* fix it,
--- `CAUSEWAYBAY_LIB`, cannot be set by a double-click.
---
--- So the path is worked out here and handed to `open` explicitly. The
--- candidates are probed with plain `io.open` rather than `love.filesystem`,
--- because these paths are deliberately outside the sandbox.
---
--- Returns nil when nothing is found, which is not an error: a checkout has no
--- bundle layout and is expected to fall through to the binding's own search.
local function bundled_library()
  local base = love.filesystem.getSourceBaseDirectory()
  if not base or base == "" then return nil end
  local name = library_name()

  for _, path in ipairs({
    -- macOS, where `make app` puts it. Frameworks is where a signed bundle's
    -- nested binaries belong, and codesign expects to find them there.
    base .. "/../Frameworks/" .. name,
    base .. "/Frameworks/" .. name,
    -- Beside the .love, which is the shape `make package` produces and the one
    -- a Linux or Windows bundle uses.
    base .. "/" .. name,
    base .. "/../" .. name,
  }) do
    local handle = io.open(path, "rb")
    if handle then
      handle:close()
      return path
    end
  end
  return nil
end

--- Start the worker and hand back the `jobs` interface the model expects.
--- `library` is passed through because the worker opens its own wallet on the
--- other side of a thread boundary, where none of this file's locals exist —
--- and a bundle that found its library here but not there would start, show a
--- balance of nothing, and never say why.
local function start_worker(home, network, library)
  -- Relative: `newThread` reads through love.filesystem, which is rooted at
  -- the game directory and cannot see an absolute path outside it.
  local thread = love.thread.newThread("worker.lua")
  thread:start(ROOT, home or "", network or "", library or "")

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
  -- Which way up: what the harness asked for, or the way it was left — a
  -- wallet stood on its side comes back that way.
  --
  -- `CWB_SHOT_TALL=1` photographs the tall layout — every screen has two of
  -- them and only the wide one was ever pictured, so a portrait screen could
  -- overlap itself for a release without anybody seeing it. A shot states its
  -- orientation either way, rather than inheriting whichever one somebody last
  -- left the game in, and never remembers the turn it made for the picture.
  -- `~= ""` because an empty environment variable is still a string, and a
  -- string is true: `CWB_SHOT_TALL= love lovegui` turned the game on its side.
  local saved = love.filesystem.read(LAYOUT_FILE)
  if os.getenv("CWB_SHOT") then
    set_orientation((os.getenv("CWB_SHOT_TALL") or "") ~= "", false)
  elseif saved == "portrait" and not L.portrait then
    set_orientation(true)
  end
  sprite.load()
  sound.load()
  love.keyboard.setKeyRepeat(true)

  game.springs = widgets.Springs.new()
  game.fx = particles.System.new(700)
  game.stars = particles.Stars.new(110, theme.WIDTH, theme.HEIGHT)
  game.entrance = anim.Tween.new(0, 1, 0.9, anim.expo_out)
  game.screen_slide = anim.Spring.new(0, 240, 0.62)

  local home = os.getenv("CAUSEWAYBAY_HOME")
  local library = bundled_library()
  local wallet, err = causewaybay.open({
    home = home,
    -- nil in a checkout, which lets the binding's own search run.
    lib = library,
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
  game.model = Model.new(wallet, start_worker(home, nil, library))
  -- A remembered session greets you by name; only a cold start says "Ready".
  if not recall_session() then
    game.model:say("Ready.")
  end
end

function love.quit()
  -- Let the worker finish rather than leaving a thread blocked on `demand`.
  if game.model and game.model.jobs then
    love.thread.getChannel("cwb.requests"):push("quit")
  end
end

-- -------------------------------------------------------------------- update

--- Start the rocket. Called the moment a transfer is confirmed.
local function begin_launch()
  game.launch = Launch.new()
  game.shake = 2
  -- 1.3 seconds of motor, generated to cover LAUNCH_FLOOR with a little over,
  -- and its pitch climbs on the same exponential curve as the sprite.
  sound.play("launch")
end

--- Where the rocket is, and how hard it is burning. See `ui/launch.lua`.
local function flight()
  return Launch.flight(game.launch)
end

--- Which row a card belongs to, for deciding whether it is a different card.
---
--- The row index *and* the address, because neither alone is enough. Two rows
--- in one store can share an address — importing a phrase the store already
--- had — so the address does not identify a row; and a refresh rebuilds the
--- list into fresh tables, so identity does not survive one. Together they are
--- stable across a refresh and still tell two identical-looking rows apart.
local function face_key(index, account)
  if not account then return nil end
  return index .. "\0" .. tostring(account.address)
end

--- Keep the card in step with the selection, swiping when they disagree.
---
--- Driven here rather than from the drawing code because *which way* the card
--- should travel is a fact about the selection moving, and the drawing code
--- only ever sees where it ended up.
local function swipe_card(model)
  local index = model.selected
  local wanted = model.wallets[index]
  local face = game.face
  local key = face_key(index, wanted)
  if key == face.key then return end
  face.key = key

  if face.shown == nil then
    -- The first card of the session does not slide in from nowhere.
    face.shown, face.turn, face.index = wanted, 1, index
    return
  end

  -- Down the list scrolls the card left, which is the way the eye expects a
  -- list to move under a cursor going down. Up reverses it.
  face.dir = index >= (face.index or index) and 1 or -1
  face.index = index

  if face.turn < 1 and face.next then
    -- Already moving. Retarget rather than restart: holding an arrow key
    -- would otherwise snap the card back to the middle on every repeat.
    face.next = wanted
    return
  end

  face.next, face.turn = wanted, 0
  sound.play("card")
  -- Thrown off the trailing edge, so the spray comes from where the card
  -- left rather than from the middle of a card that is no longer there.
  game.fx:burst(card_target.x - face.dir * 40, card_target.y,
    { count = 20, speed = 150, colour = { 0.3, 0.94, 1 }, life = 0.45,
      sprite = "spark", size = 3 })
end

--- React to what the model says happened, with light and noise.
---
--- Light and noise are emitted together on purpose. An effect and its sound
--- are one event to a person, and splitting them across two files is how they
--- drift apart until the confetti lands a beat before the fanfare.
local function celebrate(event)
  local cx, cy = theme.WIDTH / 2, theme.HEIGHT / 2
  if event == "created" then
    sound.play("created")
    game.fx:burst(cx, cy - 20, { count = 30, speed = 200, colour = { 1, 0.85, 0.3 },
      sprite = "spark", size = 4, life = 0.9, gravity = 180 })
    game.toast = { text = "WALLET CREATED", colour = theme.colour.gold, life = 2.2 }
  elseif event == "balance" then
    sound.play("coin")
    game.fx:coins(cx, theme.HEIGHT - 60, coin_target, 14)
  elseif event == "sent" then
    -- Fired from where the rocket left, so the celebration comes from the
    -- thing that just flew rather than from the middle of the screen.
    sound.play("sent")
    game.fx:confetti(cx, 40, 90)
    game.fx:burst(cx, 40, { count = 60, speed = 420, colour = { 0.4, 1, 0.6 }, life = 1.3 })
    game.fx:burst(cx, 40, { count = 30, speed = 220, colour = { 1, 0.85, 0.3 },
      sprite = "spark", size = 5, life = 1.0 })
    game.toast = { text = "SENT", colour = theme.colour.green, life = 2.6 }
    -- A tap, not another shake. This lands at the end of a second and a
    -- quarter of the screen shaking with the thrust, and repeating the same
    -- gesture louder makes the arrival read as more of the flight rather than
    -- as the end of it. The confetti says it arrived; the screen goes still.
    game.shake = 1.2
  elseif event == "error" then
    sound.play("error")
    game.shake = 6
    game.fx:burst(cx, cy, { count = 22, speed = 260, colour = { 1, 0.3, 0.4 },
      life = 0.6, gravity = 320 })
  elseif event == "saved" or event == "exported" then
    -- With the path under it, in full. The status line on this screen is a
    -- hundred pixels wide — it sits between + NEW and REFRESH — and every path
    -- is longer than that, so a person who took their eyes off the dialog for
    -- a second would have no way left to find out where their keys went.
    local secret = event == "exported"
    sound.play(secret and "power" or "created")
    game.toast = {
      text = secret and "KEYS WRITTEN" or "ADDRESSES SAVED",
      detail = (game.model.written or {}).where,
      colour = secret and theme.colour.red or theme.colour.green,
      life = 2.6,
    }
  elseif event == "selected" or event == "network" then
    sound.play("blip", { pitch = 1.25 })
    game.fx:burst(cx, cy, { count = 12, speed = 140, colour = { 0.3, 0.94, 1 }, life = 0.5 })
  end
end

function love.update(dt)
  -- Before anything else, and before the boot screen's early return below:
  -- this is the one place in the file allowed to resize the window, because it
  -- is the one place that is not inside a canvas pass.
  if game.window then
    local wanted = game.window.fullscreen
    game.window = nil
    love.window.setFullscreen(wanted, "desktop")
  end

  -- A frame that ran long — the window was dragged, the wallet blocked — must
  -- not be integrated in one step. Everything here is exponential and stable,
  -- but a 2-second dt would still teleport particles across the screen.
  dt = math.min(dt, 1 / 20)
  game.time = game.time + dt
  -- The clamped dt, deliberately: the sound throttle should slow down with
  -- everything else on a long frame rather than letting a hitch fire a burst
  -- of blips the moment the frame lands.
  sound.update(dt)

  for name, left in pairs(game.armed) do
    game.armed[name] = left > dt and (left - dt) or nil
  end

  if game.boot then
    -- The springs move during the boot too, because one widget is drawn over
    -- it: without this the mode button would be clickable but dead — no hover
    -- glow, no press, since both of those are springs and nothing was
    -- integrating them.
    game.springs:update(dt)
    game.boot:update(dt)
    if game.boot:complete() then
      game.boot = nil
      if game.model and not game.model:logged_in() then game.login = Login.new() end
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

  if game.login then
    game.login:update(dt)
    if game.model and game.model:logged_in() then
      -- The session has started, so the screen goes. Wiped first: a phrase
      -- must not outlive the moment it was used, and dropping the reference
      -- is not the same as clearing it.
      game.login:forget()
      game.login = nil
      remember_session()
      game.entrance:restart()
      game.fx:burst(theme.WIDTH / 2, theme.HEIGHT / 2,
        { count = 40, speed = 320, colour = { 0.42, 0.96, 0.48 }, life = 1.0 })
    end
  end

  if not game.model then return end

  game.model:pump()

  swipe_card(game.model)
  if game.face.turn < 1 then
    game.face.turn = math.min(1, game.face.turn + dt / CARD_SWIPE)
    if game.face.turn >= 1 and game.face.next then
      -- Arrived. The card that slid in is simply the card now.
      game.face.shown, game.face.next = game.face.next, nil
    end
  end

  if game.launch then
    local finished = Launch.step(game.launch, dt, game.model:busy())
    local risen, t = flight()

    -- Only while it is actually flying. Everything loud used to be gated on
    -- the launch existing at all, so a launch waiting on a slow node went on
    -- shaking the screen and burning fuel at a rocket that had left.
    if Launch.flying(game.launch) then
      -- Exhaust, thickening as the throttle opens. Emitted from where the
      -- rocket actually is, so the plume stays attached to it.
      local burn = 1 + math.floor(t * 6)
      for _ = 1, burn do
        game.fx:embers(rocket.x + (math.random() - 0.5) * 6,
          rocket.y - risen * 260 + 18, 10, 1, 1)
      end
      game.fx:trail(rocket.x, rocket.y - risen * 260 + 14, 0, -risen * 700,
        { 0.5, 0.85, 1 })

      -- The shake builds with the thrust rather than being a one-off thump.
      game.shake = math.max(game.shake, t * t * 5)
    end

    if finished then
      for _, event in ipairs(finished) do celebrate(event) end
      game.launch = nil
    end
  end

  local events = game.model:drain()
  if not Launch.hold(game.launch, events) then
    for _, event in ipairs(events) do celebrate(event) end
  end

  -- The confirmation dialog fades in, and snaps out.
  local wanted = game.model.confirm and 1 or 0
  game.confirm_t = anim.approach(game.confirm_t, wanted, wanted == 1 and 14 or 22, dt)
  local asked = game.model.write and 1 or 0
  game.write_t = anim.approach(game.write_t, asked, asked == 1 and 14 or 22, dt)

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

--- Clipboard helpers, above the keypress handler *and* the buttons that call
--- them. A `local function` declared later is not in scope earlier — the call
--- would read a nil global and crash — which is exactly how Ctrl+V used to
--- take the whole game down. The globals check in `make lint` now refuses
--- that shape outright.
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


local function mouse_state()
  -- `game.pointer` is the screenshot harness pressing a button — see CAPTURING
  -- at the foot of this file. Nothing else ever sets it, and with it unset this
  -- is the real pointer and nothing else.
  if game.pointer then
    return { mouse_x = game.pointer.x, mouse_y = game.pointer.y, clicked = game.clicked }
  end
  local mx, my = theme.to_canvas(love.mouse.getPosition())
  return { mouse_x = mx, mouse_y = my, clicked = game.clicked }
end

function love.wheelmoved(_, y)
  if game.boot or game.login or not game.model then return end
  -- A dialog owns the wheel too, or the list scrolls under the panel that is
  -- describing what is about to happen to it.
  if game.model:asking() then return end
  if game.model.screen == "wallets" then
    -- Three rows a notch, which is the step that feels like a list rather
    -- than a slingshot.
    game.model:scroll_by(-y * 3, game.list_rows)
  elseif game.model.screen == "network" then
    -- One row a notch here: the grid's rows are two wide, so three notches
    -- would jump six entries and the eye would lose its place. The upper
    -- bound is the frame's, so it is applied where the frame is drawn.
    game.net_scroll = math.max(0, (game.net_scroll or 0) - y)
  end
end

function love.mousepressed(_, _, button)
  -- A click does not continue the boot. The prompt asks for space, and a boot
  -- screen that also took a click would be a boot screen that ends when
  -- somebody moves the window — `boot:skip` is reached from the keyboard only.
  --
  -- The click is still recorded, because one thing on that screen does want
  -- it: the window-mode button. It is the only widget drawn over the boot, so
  -- there is nothing else for the click to fall through to.
  if button == 1 then game.clicked = true end
end

function love.textinput(text)
  if game.boot then return end
  if game.login then return game.login:type_into(text) end
  if not game.model then return end
  -- On the network screen the keyboard has nothing else to do, so it narrows
  -- the list: arriving and typing `usdc` is the whole interaction, with no
  -- click into a field first. Everywhere else the form owns it as before.
  if game.model.screen == "network" and not game.model:asking() then
    game.net_scroll = 0
    return game.model:search_for(game.model:search() .. text)
  end
  game.model:type_into(text)
end

function love.keypressed(key)
  -- Muting works everywhere, including on the boot screen and behind the
  -- login — the one control a person reaches for in a hurry is the one that
  -- must not be gated behind getting into the wallet first.
  if key == "m" and not (game.model and game.model.screen == "send")
      and not game.login then
    sound.toggle()
    sound.play("press")
    return
  end

  if game.boot then
    -- Space, and only space: the first press skips the typing, the second
    -- hands over. It used to be any key, which is friendlier right up until
    -- somebody is recording the sequence — then every stray press ends the
    -- take, and the keys most likely to be pressed by accident are the ones
    -- nobody thinks of as input. The prompt says which key it wants.
    --
    -- Escape still quits, because being trapped in a boot screen is not
    -- charming, and `0` still replays it.
    if key == "escape" then love.event.quit() end
    -- `0` plays the whole intro again from black, for recording it. A capture
    -- wants to be able to go round twice without restarting the process, and
    -- a boot sequence you can only see once per launch is a boot sequence
    -- nobody gets a clean take of.
    if key == "0" then
      game.boot = Boot.new(game.model and game.model.wallet, game.error, Boot.REPLAY_HOLD)
      return
    end
    if key == "space" then game.boot:skip() end
    return
  end

  -- The window is fullscreen by default, so a way back out matters.
  if key == "f11" or (key == "return" and love.keyboard.isDown("lalt", "ralt")) then
    ask_fullscreen(not fullscreen_now())
    return
  end

  if game.login then
    if key == "escape" then
      love.event.quit()
    elseif key == "return" or key == "kpenter" then
      game.login:submit(game.model)
    elseif key == "backspace" then
      if love.keyboard.isDown("lalt", "ralt") then
        game.login:backword()
      else
        game.login:backspace()
      end
    elseif love.keyboard.isDown("lctrl", "rctrl", "lgui", "rgui") and key == "v" then
      game.login:paste(love.system.getClipboardText())
    end
    return
  end

  if key == "escape" then
    if game.model and game.model.confirm then
      sound.play("back")
      game.model:cancel_send()
    elseif game.model and game.model.write then
      sound.play("back")
      game.model:cancel_write()
    -- A search is a thing you are in the middle of, and Escape is how anyone
    -- gets out of one. Quitting the wallet instead — from a screen where the
    -- last thing they did was type — is not a mistake worth allowing.
    elseif game.model and game.model.screen == "network"
        and game.model:search() ~= "" then
      sound.play("back")
      game.net_scroll = 0
      game.model:search_for("")
    else
      love.event.quit()
    end
    return
  end

  if not game.model then return end
  local model = game.model

  if model.confirm then
    -- The dialog owns the keyboard while it is up.
    if key == "return" or key == "kpenter" then
      if model:confirm_send() then begin_launch() end
    end
    return
  end

  if model.write then
    -- So does this one. ENTER writes the files it just named; ESCAPE, handled
    -- above, writes nothing.
    if key == "return" or key == "kpenter" then model:confirm_write() end
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
    sound.play("blip", { pitch = 1.1 })
    model:next_field()
  elseif key == "backspace" then
    if model.screen == "network" then
      game.net_scroll = 0
      model:search_for(model:search():sub(1, -2))
    else
      model:backspace()
    end
  elseif key == "return" or key == "kpenter" then
    if model.screen == "send" then
      model:begin_send(model.form.to, model.form.amount)
    elseif model.screen == "wallets" then
      model:fetch_balance()
    end
  elseif key == "1" or key == "2" or key == "3" then
    -- Only when a text field is not the point of the screen. The network
    -- screen's search box is one: `1` there is a character in `usdt1`, not a
    -- jump to the wallet list, and a digit that did both would type and
    -- navigate at once.
    if model.screen ~= "send" and model.screen ~= "network" then
      sound.play("tab")
      model:go(Model.SCREENS[tonumber(key)])
      game.screen_slide:set(1):to(0)
    end
  elseif key == "up" or key == "down" then
    if model.screen == "network" then
      game.net_scroll = math.max(0, (game.net_scroll or 0) + (key == "down" and 1 or -1))
      sound.play("blip", { pitch = 1.1 })
    elseif model.screen == "wallets" and #model.wallets > 0 then
      local step = key == "down" and 1 or -1
      local was = model.selected
      model.selected = math.max(1, math.min(#model.wallets, model.selected + step))
      model:reveal(model.selected, game.list_rows)
      if model.selected ~= was then
        -- Pitched by position in the list, so running down twelve wallets is
        -- a falling scale rather than the same tick twelve times. Held inside
        -- a fifth: past that it stops reading as the same sound.
        local place = (model.selected - 1) / math.max(1, #model.wallets - 1)
        sound.play("blip", { pitch = 1.2 - place * 0.4 })
      end
    end
  elseif key == "pageup" or key == "pagedown" then
    if model.screen == "wallets" then
      model:scroll_by(key == "pagedown" and game.list_rows or -game.list_rows,
        game.list_rows)
    end
  elseif key == "home" then
    model.selected, model.scroll = 1, 0
  elseif key == "end" then
    model.selected = #model.wallets
    model:reveal(model.selected, game.list_rows)
  end
end

-- ---------------------------------------------------------------------- draw

--- Break text into lines that fit a width.
---
--- At word boundaries where it can and mid-word where it cannot, because the
--- longest single thing this has to lay out is a filesystem path, and a path
--- has no spaces in it. Falling back to a character break is the difference
--- between a path that wraps and a path that runs off the panel.
local function wrap(text, width, font)
  font = font or theme.font.small
  local lines, line = {}, ""

  local function flush()
    if line ~= "" then lines[#lines + 1] = line end
    line = ""
  end

  for word in tostring(text):gmatch("%S+") do
    local candidate = line == "" and word or (line .. " " .. word)
    if theme.width(candidate, font) <= width then
      line = candidate
    else
      flush()
      -- A word that does not fit on a line of its own is cut where it stops
      -- fitting, and the rest carries on below.
      while theme.width(word, font) > width do
        local cut = #word
        while cut > 1 and theme.width(word:sub(1, cut), font) > width do
          cut = cut - 1
        end
        lines[#lines + 1] = word:sub(1, cut)
        word = word:sub(cut + 1)
      end
      line = word
    end
  end
  flush()
  return lines
end

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
  theme.rect(theme.colour.deep, 0, 0, theme.WIDTH, L.header.band_h)
  theme.rule(0, L.header.band_h, theme.WIDTH, theme.colour.cyan_dark, 0.8 * t)

  local bob = math.sin(game.time * 2) * 1.5
  sprite.draw_glowing("logo", 20, 17 + bob, 26, {
    angle = math.sin(game.time * 0.7) * 0.06,
    glow = 0.6 + 0.25 * math.sin(game.time * 3),
    glow_colour = theme.colour.cyan,
  })

  theme.text("CAUSEWAYBAY", 38, 3, theme.colour.cyan, theme.font.body, t)
  theme.text("BANK", 38, 18, theme.colour.dim, theme.font.small, t)

  -- The chain, then the network within it. Not the chain *id*: only EVM has
  -- one, and "chain nil" was what the other three used to render. Portrait
  -- shows the key alone — it carries the chain in its first word, and a
  -- 254-pixel row has no room to say it twice.
  local network = model and model.info and model.info.network or "…"
  local chain = model and model.info and model.info.chain or ""
  local where = L.portrait and network or ("%s · %s"):format(chain, network)
  -- And which asset, when that is not the network's own coin. The header is
  -- the one line on every screen, so it is where "you are spending USDC, not
  -- CRO" has to be legible — a balance and a send form that quietly changed
  -- denomination with nothing above them saying so is the mistake this line
  -- exists to prevent.
  --
  -- The symbol goes *first*, and the chain word is dropped. This line is
  -- clipped from the right when it will not fit, and appending the token put
  -- the one thing worth saying in the first place to be cut: the header read
  -- `evm · cronos-main`, which is the old line with a character missing rather
  -- than the new one. The network key still opens with its chain, so nothing
  -- is lost by dropping the word that repeats it.
  if model and model.token then
    where = ("%s · %s"):format(model.token.symbol or model.token.key, network)
  end
  local spot = L.header.network
  -- Marked when it had to be cut, rather than silently ending early: chopped
  -- bare, `cronos-mainnet` becomes `cronos-mai`, which does not look like a
  -- shortened name so much as a different one.
  if theme.width(where, theme.font.small) > spot.max_w then
    while theme.width(where .. "…", theme.font.small) > spot.max_w and #where > 1 do
      where = where:sub(1, -2)
    end
    where = where .. "…"
  end
  if spot.align == "right" then
    theme.text_right(where, spot.x, spot.y, theme.colour.dim, theme.font.small, t)
  else
    theme.text(where, spot.x, spot.y, theme.colour.dim, theme.font.small, t)
  end

  -- A spinner while the node is thinking, so "busy" is never just a word.
  if model and model:busy() then
    local r = 4
    for i = 0, 5 do
      local a = game.time * 6 + i * 0.9
      local fade = 1 - (i / 6)
      theme.set(theme.colour.cyan, fade * 0.9)
      love.graphics.rectangle("fill",
        L.header.spinner.x + math.cos(a) * r, L.header.spinner.y + math.sin(a) * r, 2, 2)
    end
  end
end

local function draw_tabs(model, state)
  local labels = { wallets = "WALLETS", send = "SEND", network = "NETWORK" }
  local w = 74
  for i, name in ipairs(Model.SCREENS) do
    local box = { x = 8 + (i - 1) * (w + 4), y = L.header.tabs_y, w = w, h = 19 }
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
--- The window-mode button: FULL when windowed, WIN when it is not.
---
--- One function rather than one per screen, because it is on three of them —
--- the boot sequence, the login gate, and the wallet header. A window that
--- opens fullscreen needs a visible way back out from the first frame, not
--- from whenever somebody gets past the mnemonic prompt, and a keyboard
--- shortcut nobody is told about is not a way out.
local function draw_mode_button(box, state)
  local full = fullscreen_now()
  if widgets.button(game.springs, "mode", box, full and "WIN" or "FULL", state,
      { colour = theme.colour.cyan, font = theme.font.small }) then
    ask_fullscreen(not full)
  end
end

--- The orientation button: TALL when wide, WIDE when tall.
---
--- Labelled like FULL/WIN — the button names what pressing it gives you, not
--- what you already have. On the same three screens as the mode button, and
--- for the same reason: a game that can stand on its side needs the way back
--- visible from the first frame, not only from behind the mnemonic prompt.
local function draw_layout_button(box, state)
  if widgets.button(game.springs, "layout", box, L.portrait and "WIDE" or "TALL",
      state, { colour = theme.colour.cyan, font = theme.font.small }) then
    set_orientation(not L.portrait)
  end
end

--- Where those buttons go on a screen that has no header to hang them in.
--- Functions, because the width they hang from changes with the orientation.
local function mode_box()
  return { x = theme.WIDTH - 50, y = 4, w = 44, h = 15 }
end

local function layout_box()
  return { x = theme.WIDTH - 98, y = 4, w = 44, h = 15 }
end

--- The header's two buttons: the window mode, and the way out.
local function draw_header_buttons(model, state)
  -- A button as well as the `M` key. The key cannot be the only way: every
  -- letter is typed into a field somewhere, so `M` has to be ignored on the
  -- screens that take text — and a control that stops working on some screens
  -- is not a control anyone trusts. This one is always here.
  draw_layout_button(L.header.buttons.layout, state)

  local sfx = L.header.buttons.sfx
  if widgets.button(game.springs, "sfx", sfx, sound.enabled and "SFX" or "MUTE",
      state, {
        colour = sound.enabled and theme.colour.green or theme.colour.faint,
        font = theme.font.small,
        -- The one button that makes its own noise. Left to the widget, the
        -- click would play *before* the toggle — so turning the sound on
        -- would be the one press in the game that answers with silence.
        silent = true,
      }) then
    sound.toggle()
    sound.play("press")
  end

  draw_mode_button(L.header.buttons.mode, state)

  -- LOGOUT deletes the store, so it asks. The first press arms it and the
  -- label says what the second one does; it disarms itself after a few
  -- seconds, because walking away should not leave the wallet one stray click
  -- from being wiped.
  local armed = game.armed.logout ~= nil
  local out = L.header.buttons.logout
  if widgets.button(game.springs, "logout", out, armed and "WIPE?" or "LOGOUT", state,
      { colour = armed and theme.colour.gold or theme.colour.red,
        font = theme.font.small }) then
    if armed then
      game.armed.logout = nil
      -- Everything the wallet keeps goes with the session. Anything not
      -- exported first is gone, which is what the arming was for.
      model:logout({ wipe = true })
      forget_session()
      -- A brand new screen, which holds no phrase, is not minted, and offers
      -- PASTE rather than COPY. Nothing is carried over from the session that
      -- just ended — including which wallet it was, so NEW MNEMONIC after this
      -- starts a genuinely new one.
      game.login = Login.new()
    else
      game.armed.logout = ARM_TIME
      model:say("LOGOUT again to wipe every wallet", "error")
    end
  end
end

--- Two lines: a label over a quieter value. The unit of most of this UI.
local function stat(x, y, label, value, colour, font)
  theme.text(label, x, y, theme.colour.faint, theme.font.small)
  theme.text(value, x, y + 13, colour or theme.colour.text, font or theme.font.small)
end

--- One card, at a placement along the swipe.
---
--- Pulled out of `draw_wallets` because during a swipe it is called twice —
--- once for the card leaving and once for the one arriving. `place` is nil
--- when the card is simply sitting there, which is most frames.
local function draw_card(model, entry, face, place)
  card.draw(card.design(entry.address), face, {
    time = game.time,
    offset_x = place and place.x or 0,
    scale = place and place.scale or 1,
    alpha = (place and place.alpha or 1) * game.entrance.value,
    holder = entry.label,
    -- The balance belongs on the card, where a card puts a number. It was a
    -- frame of its own above; a card with somebody else's balance printed
    -- over it would be the one mistake this whole design is here to prevent.
    body = function(box, ink, alpha)
      if model.balance and entry.address == model.active then
        local amount = (("%.4f"):format(game.balance_shown)
          :gsub("0+$", ""):gsub("%.$", ""))
        theme.text(amount, box.x + 10, box.y + 60, theme.colour.text,
          theme.font.big, alpha)
        theme.text(model.balance.symbol or "",
          box.x + 12 + theme.width(amount, theme.font.big), box.y + 68,
          ink, theme.font.small, alpha)
      elseif entry.address == model.active then
        theme.text("- - -", box.x + 10, box.y + 60, theme.colour.faint,
          theme.font.big, alpha)
        theme.text("REFRESH to ask the node", box.x + 10, box.y + 84,
          theme.colour.faint, theme.font.small, alpha * 0.9)
      else
        -- An inactive card must not show the active one's money. Saying
        -- which card the balance belongs to is the only honest option.
        theme.text("USE THIS CARD", box.x + 10, box.y + 62, ink,
          theme.font.body, alpha)
        theme.text("to see its balance", box.x + 10, box.y + 80,
          theme.colour.faint, theme.font.small, alpha * 0.9)
      end

      local tag = (entry.source or "?"):upper():gsub("_", " ")
      theme.text_right(tag, box.x + box.w - 10, box.y + box.h - 15,
        theme.colour.faint, theme.font.small, alpha * 0.8)

      if entry.address == model.active then
        -- The one badge worth the room: which card the wallet is actually
        -- spending from. It pulses, because it is the answer to the question
        -- a person asks most often on this screen.
        --
        -- Up beside the sigil rather than down by the holder — the bottom
        -- right belongs to the second line of the card number, and a badge
        -- printed over an address is a badge that makes the address wrong.
        --
        -- Measured rather than guessed at 56: the chip is its label plus ten,
        -- which is 58, so the badge hung two pixels out through the card's own
        -- edge.
        local pulse = anim.pulse(game.time, 1.6, 0.55, 1.0)
        local chip_w = theme.width("ACTIVE", theme.font.small) + 10
        widgets.chip(box.x + box.w - 10 - chip_w, box.y + 36, "ACTIVE",
          theme.colour.green, { alpha = alpha * pulse })
      end
    end,
  })
end

--- Trim text to a pixel width, saying so with a trailing "…".
---
--- `theme.ellipsis` counts characters from both ends, which is right for an
--- address — the two ends are what identify it — and wrong for a name, whose
--- end carries nothing. A name loses its tail instead.
---
--- Not a bare scissor: clipped text stops mid-stroke and reads as a rendering
--- fault, where "account0-card…" reads as a name that goes on. `keep_end` of
--- zero cannot be passed to `theme.ellipsis` for this — `("abc"):sub(-0)` is
--- the whole string in Lua, so it would return the text it was asked to trim.
local function fit_text(text, width, font)
  if theme.width(text, font) <= width then return text end
  local kept = #text
  while kept > 1 do
    kept = kept - 1
    local candidate = text:sub(1, kept) .. "…"
    if theme.width(candidate, font) <= width then return candidate end
  end
  return "…"
end

local function draw_wallets(model, state, x)
  local height = L.list.h

  -- ------------------------------------------------------------ the list
  --
  -- Two lines per wallet — a name is not enough to tell two apart, and an
  -- address alone is unreadable — so how many fit is known before the frame is
  -- drawn, and the frame's own inner height (13 less than the outer) is what
  -- they have to fit inside.
  local row_h = 27
  local rows = math.floor((height - 13) / row_h)
  -- Published so the wheel and the arrow keys, which are handled far from
  -- here, know how big a page is.
  game.list_rows = rows
  model.scroll = math.max(0, math.min(math.max(0, #model.wallets - rows), model.scroll))

  -- How far down the list you are goes in the frame's top edge rather than
  -- along its bottom. The bottom line was a row of content that could not hold
  -- content: it sat over the last wallet of a full list and hung through the
  -- border into the action bar.
  local range = nil
  if #model.wallets > rows then
    range = ("%d-%d OF %d"):format(model.scroll + 1,
      math.min(#model.wallets, model.scroll + rows), #model.wallets)
  end
  local list = widgets.frame(x + L.list.x, L.list.y, L.list.w, height,
    ("WALLETS %d"):format(#model.wallets), { note = range })

  if #model.wallets == 0 then
    theme.text_centred("no wallets yet", x + L.list.x + L.list.w / 2,
      L.list.y + height / 2 - 16, theme.colour.faint, theme.font.small)
    theme.text_centred("press + NEW below", x + L.list.x + L.list.w / 2,
      L.list.y + height / 2 - 2, theme.colour.faint, theme.font.small)
  end

  -- The per-row pay button, measured rather than assumed at some number that
  -- happens to be right for this label. It is the same width on every row, so
  -- the buttons line up into a column instead of a ragged edge.
  local pay_w = theme.width(QUICK_LABEL, theme.font.small) + 6

  for slot = 1, rows do
    local i = slot + model.scroll
    local account = model.wallets[i]
    if not account then break end
    local box = { x = list.x, y = list.y + (slot - 1) * row_h, w = list.w, h = row_h - 2 }
    local selected = account.address == model.active

    -- One press pays this wallet `QUICK_AMOUNT` from whichever wallet is
    -- active. Never on the active row: a wallet paying itself moves nothing
    -- and still costs the fee, and the wallet refuses it anyway.
    --
    -- Anchored to the frame rather than to `row_x`, so it does not slide with
    -- the selection underneath it. A button that spends money should be where
    -- it was a moment ago, and not two pixels along because the pointer
    -- arrived.
    local pay = nil
    if not selected then
      pay = { x = box.x + box.w - pay_w, y = box.y + 4, w = pay_w, h = 17 }
    end

    local clicked, hovered, row_x, slide = widgets.row(game.springs, "row" .. i, box, state,
      selected or model.selected == i)
    -- The button sits inside the row, and the row is one big click target, so
    -- a press on the button would otherwise also select the row. Paying a
    -- wallet and moving into it are different intentions and this is the one
    -- that must not do both.
    if clicked and not (pay and widgets.hit(state.mouse_x, state.mouse_y, pay)) then
      model:select(i)
    end

    local ink = selected and theme.colour.cyan
      or (hovered and theme.colour.text or theme.colour.dim)
    sprite.draw_glowing("wallet", row_x + 13, box.y + 14, 17, {
      alpha = 0.45 + slide * 0.55,
      glow = selected and 0.55 or 0,
      glow_colour = theme.colour.gold,
    })

    -- What the name and the address have left once the button has taken its
    -- end of the row.
    --
    -- The width is reserved on every row, including the active one that draws
    -- no button: measuring each row against its own leftover space made the
    -- active wallet's address two characters longer than all the others, and a
    -- column of addresses that are not the same length is harder to read down,
    -- which is the only thing it is for.
    --
    -- Measured from the row's resting position plus the five pixels it slides
    -- when selected, so the address keeps its length through the animation
    -- rather than dropping a character as the row moves.
    local text_x = row_x + 26
    local edge = box.x + box.w - pay_w - 3
    local budget = edge - (box.x + 26 + 5)
    local short
    for _, pair in ipairs({ { 8, 6 }, { 8, 5 }, { 7, 5 }, { 6, 4 }, { 5, 4 }, { 4, 4 } }) do
      short = theme.ellipsis(account.address, pair[1], pair[2])
      if theme.width(short, theme.font.small) <= budget then break end
    end
    -- A label is whatever its owner called it, so it loses its tail rather
    -- than being shortened from both ends the way the address is.
    --
    -- The clip is a second line of defence behind that, and its box is
    -- deliberately taller than the row: it is here to hold text back from the
    -- button, which is a question about width only, and a box the row's own
    -- height cut the bottom two pixels off every address — the second line
    -- starts fourteen pixels down and the glyphs are taller than the eleven
    -- that leaves.
    local room = { x = text_x, y = box.y - 2, w = math.max(0, edge - text_x), h = row_h + 4 }
    theme.clip(room, function()
      theme.text(fit_text(account.label, budget, theme.font.small),
        text_x, box.y + 1, ink, theme.font.small)
      theme.text(short, text_x, box.y + 14,
        selected and theme.colour.cyan_dark or theme.colour.faint, theme.font.small)
    end)

    if pay then
      local pressed = widgets.button(game.springs, "pay" .. i, pay, QUICK_LABEL, state, {
        colour = theme.colour.gold,
        font = theme.font.small,
        -- While the wallet is pricing one transfer, a second press would
        -- queue a second — and the two would come back as two dialogs.
        disabled = model:busy(),
      })
      if pressed then model:quick_send(i) end
    end
  end

  -- A scrollbar, so a list longer than the frame says so and shows where in
  -- it you are. "+7 more" told you there was more and nothing else.
  if #model.wallets > rows then
    local track_x = list.x + list.w + 1
    local track_h = rows * row_h - 2
    theme.rect(theme.colour.void, track_x, list.y, 2, track_h, 0.6)
    local thumb = math.max(8, track_h * rows / #model.wallets)
    local travel = (track_h - thumb) * (model.scroll / math.max(1, #model.wallets - rows))
    theme.rect(theme.colour.cyan, track_x, list.y + travel, 2, thumb, 0.7)
  elseif #model.wallets > 0 and #model.wallets < rows then
    -- Under the last wallet, in the empty row below it — never over one. A
    -- list with no room to spare has a scrollbar saying the same thing.
    theme.text("UP / DOWN TO MOVE", list.x, list.y + #model.wallets * row_h + 4,
      theme.colour.faint, theme.font.small)
  end

  -- ------------------------------------------------------------ the card
  -- Whatever the list is highlighting — not whichever account is *active*,
  -- which meant the panel could show one wallet's label above another's
  -- address the moment the two disagreed.
  --
  -- A card rather than a labelled panel, because a list of hex strings is a
  -- list of hex strings: nobody recognises one, and everybody has to read all
  -- forty characters to be sure. A face is recognised before a single
  -- character has been read, and the wrong face is noticed just as fast. See
  -- `ui/card.lua` for how it is dealt from the address.
  local selected = model.wallets[model.selected]

  -- A real card's proportions, centred in its region. The ratio is the reason
  -- it reads as a card at a glance and not as a panel with a picture on it —
  -- so when the region is the wrong shape (portrait's band is wide and short),
  -- the card keeps its ratio and centres, rather than stretching to fill.
  local face_h = math.min(L.detail.h, math.floor(L.detail.w / 1.585))
  local face_w = math.floor(face_h * 1.585)
  local face = {
    x = x + L.detail.x + math.floor((L.detail.w - face_w) / 2),
    y = L.detail.y + math.floor((L.detail.h - face_h) / 2),
    w = face_w,
    h = face_h,
  }
  card_target.x, card_target.y = face.x + face.w / 2, face.y + face.h / 2
  coin_target.x, coin_target.y = face.x + face.w / 2, face.y + 72

  if not selected then
    theme.rect(theme.colour.deep, face.x, face.y, face.w, face.h, 0.6)
    theme.outline(theme.colour.faint, face.x, face.y, face.w, face.h, 0.5)
    theme.text_centred("no wallet selected", face.x + face.w / 2,
      face.y + face.h / 2 - 16, theme.colour.faint, theme.font.small)
    theme.text_centred("press + NEW below", face.x + face.w / 2,
      face.y + face.h / 2 - 2, theme.colour.faint, theme.font.small)
  else
    -- Two cards while it is moving: the one leaving and the one arriving,
    -- both on screen at once. That overlap is the whole difference between a
    -- swipe and a cut — for a moment you can see them travel together, which
    -- is what makes it read as a stack being moved through rather than a
    -- panel whose contents were replaced.
    --
    -- Clipped to the column, because a card number sliding across the wallet
    -- list is not a transition, it is a bug with an easing curve on it.
    local window = { x = x + L.detail.x, y = face.y - 2, w = L.detail.w, h = face.h + 4 }
    theme.clip(window, function()
      if game.face.next then
        local leaving, arriving = card.swipe(game.face.turn, face.w + 16, game.face.dir)
        draw_card(model, game.face.shown, face, leaving)
        draw_card(model, game.face.next, face, arriving)
      else
        draw_card(model, game.face.shown or selected, face, nil)
      end
    end)
  end

  -- ------------------------------------------------------------ actions
  --
  -- Five buttons across the card's column, so they are all narrow and all in
  -- the small font. Widths are hand-fitted to their labels rather than shared,
  -- because REFRESH is seven characters and SAVE is four — and fitted with two
  -- clear pixels either side of the longest label each one ever shows. USE
  -- reads IN USE while that card is the one being spent from, and at the width
  -- that fitted USE the longer label ran into its own border.
  --
  -- 60 + 40 + 56 + 40 + 40 across five buttons and 6 between them is exactly
  -- the 260 the column has, so the row is flush at both ends.
  local small = theme.font.small
  local refresh = widgets.offset(L.actions.refresh, x)
  if widgets.button(game.springs, "refresh", refresh, "REFRESH", state,
      { font = small, disabled = model:busy() or #model.wallets == 0 }) then
    model:fetch_balance()
  end

  local new = widgets.offset(L.actions.new, x)
  if widgets.button(game.springs, "new", new, "+ NEW", state,
      { colour = theme.colour.green }) then
    model:create("")
  end

  -- The card's own verbs. COPY takes the address the card is showing — the one
  -- on screen, not the one the wallet happens to be spending from, because the
  -- card is what a person is looking at.
  local copy = widgets.offset(L.actions.copy, x)
  if widgets.button(game.springs, "copy", copy, "COPY", state,
      { font = small, colour = theme.colour.cyan, disabled = selected == nil }) then
    copy_to_clipboard(model, selected.address, "address")
  end

  local usable = selected ~= nil and selected.address ~= model.active
  local use = widgets.offset(L.actions.use, x)
  if widgets.button(game.springs, "use", use, usable and "USE" or "IN USE",
      state, { font = small, colour = theme.colour.gold, disabled = not usable }) then
    model:select(model.selected)
  end

  -- Addresses, in four formats at once. Public information: which format you
  -- want depends on where it is going, and the file costs nothing to lose.
  --
  -- It still asks, because the question is not *whether* — it is *where*. The
  -- directory these land in is one nobody chose and most people have never
  -- opened, and a file you cannot find is a file you cannot delete.
  local save = widgets.offset(L.actions.save, x)
  if widgets.button(game.springs, "save", save, "SAVE", state,
      { font = small, colour = theme.colour.green, disabled = #model.wallets == 0 }) then
    model:ask_save()
  end

  -- And the keys: every mnemonic and every private key, in one file, in the
  -- clear. The dialog spells out the path and what the file is worth to
  -- whoever reads it, and nothing is written until it is answered.
  local keys = widgets.offset(L.actions.keys, x)
  if widgets.button(game.springs, "keys", keys, "KEYS", state,
      { font = small, colour = theme.colour.red,
        disabled = #model.wallets == 0 }) then
    model:ask_export()
  end
end

local function draw_send(model, state, x)

  -- The rocket gets its own column, so the exhaust has somewhere to go.
  --
  -- As wide as the wallet list on the screen next door, rather than the 96 it
  -- used to be. The columns are supposed to line up between screens, and a
  -- narrow pad left a hundred pixels of nothing between it and the form —
  -- which read as the form having drifted right rather than as space.
  local pad = widgets.frame(x + L.pad.x, L.pad.y, L.pad.w, L.pad.h, "LAUNCH")
  rocket.x, rocket.y = pad.x + pad.w / 2, pad.y + 42
  local risen, t = flight()
  local thrust = model:busy() and 1 or 0

  -- Off the top of the screen by the end, stretched as it goes: a sprite that
  -- keeps its proportions while accelerating reads as a sticker being dragged.
  sprite.draw_glowing("rocket", rocket.x,
    rocket.y + math.sin(game.time * 2) * 2 - risen * 260, 56, {
      angle = math.sin(game.time * 1.4) * 0.06 * (1 - t),
      scale_y = 1 + t * 0.5,
      scale_x = 1 - t * 0.18,
      glow = 0.35 + 0.25 * math.sin(game.time * 4) + thrust * 0.5 + t * 1.2,
      glow_colour = theme.colour.cyan,
    })

  -- The pad flashes white underneath at ignition.
  if game.launch and t < 0.35 then
    theme.rect(theme.colour.white, pad.x, rocket.y + 22, pad.w, 3,
      (1 - t / 0.35) * 0.8)
  end

  local form = widgets.frame(x + L.form.x, L.form.y, L.form.w, L.form.h, "TRANSFER")

  -- Which wallet the money leaves, at the top, before anything about where it
  -- is going. A transfer reads from-then-to, and the wallet being spent from
  -- is the one thing on this screen that is not typed here — it was chosen on
  -- another screen, possibly a while ago, and the send screen used to say
  -- nothing about it at all.
  local from
  for _, account in ipairs(model.wallets) do
    if account.address == model.active then from = account end
  end

  theme.text("FROM", form.x, form.y + 2, theme.colour.faint, theme.font.small)
  if from then
    -- The label and the address share this row, and they used to share it by
    -- printing through each other: a twelve-character label and a shortened
    -- address are 20 pixels wider than the frame, so the address's first
    -- characters landed on the label's last ones. The address is shortened
    -- further where the row is narrow, and the label is clipped to what is
    -- left over — a long label loses its tail rather than the address.
    local short = theme.ellipsis(from.address, 8, 6)
    local address_w = theme.width(short, theme.font.small)
    if 38 + theme.width(from.label, theme.font.small) + 6 + address_w > form.w then
      short = theme.ellipsis(from.address, 4, 4)
      address_w = theme.width(short, theme.font.small)
    end
    local room = { x = form.x + 38, y = form.y, w = form.w - 44 - address_w, h = 16 }
    theme.clip(room, function()
      theme.text(from.label, room.x, form.y + 2, theme.colour.text, theme.font.small)
    end)
    theme.text_right(short, form.x + form.w, form.y + 2,
      theme.colour.cyan, theme.font.small)
  else
    -- No active wallet means the send would be refused anyway. Saying so here
    -- beats letting somebody fill the form in first.
    theme.text("no wallet in use", form.x + 38, form.y + 2,
      theme.colour.red, theme.font.small)
  end
  theme.rule(form.x, form.y + 20, form.w, theme.colour.raised, 0.6)

  -- Everything below is placed against `form`, which is the *inside* of the
  -- frame — `height` is the outside, and measuring the bottom of the form from
  -- it is what pushed the last line of this screen through the border and out
  -- into the action bar. A field's label sits 13px above it, so the first row
  -- starts far enough below the rule for the two not to touch.
  local field_h = 19

  -- The recipient field gives up room to its own buttons: pasting is how an
  -- address realistically gets in here, and typing 42 hex characters is not.
  -- The placeholder shows the shape of an address *here*: `0x…` on a chain
  -- whose addresses are bech32 would teach exactly the wrong thing.
  local shapes = {
    evm = "0x…", solana = "base58…", cardano = "addr…", midnight = "mn_addr…",
    ecash = "ecash:…",
  }
  local to = { x = form.x, y = form.y + 42, w = form.w - 108, h = field_h }
  widgets.field(game.springs, "to", to, model.form.to, "RECIPIENT",
    model.focus == "to", { placeholder = shapes[model:chain()] or "address…", ellipsis = 8 })

  local paste = { x = form.x + form.w - 104, y = to.y, w = 56, h = field_h }
  if widgets.button(game.springs, "paste", paste, "PASTE", state,
      { colour = theme.colour.cyan }) then
    paste_from_clipboard(model, "to")
  end

  local clear = { x = form.x + form.w - 44, y = to.y, w = 44, h = field_h }
  if widgets.button(game.springs, "clear", clear, "CLR", state,
      { colour = theme.colour.red, disabled = model.form.to == "" }) then
    model:clear_field("to")
  end

  -- The label carries the unit, because an amount box with no denomination on
  -- it is the one field in this window whose meaning changes silently.
  local amount = { x = form.x, y = form.y + 85, w = 120, h = field_h }
  local unit = model:symbol()
  widgets.field(game.springs, "amount", amount, model.form.amount,
    unit ~= "" and ("AMOUNT (%s)"):format(unit) or "AMOUNT",
    model.focus == "amount", { placeholder = "0.0" })

  if state.clicked then
    if widgets.hit(state.mouse_x, state.mouse_y, to) then model.focus = "to" end
    if widgets.hit(state.mouse_x, state.mouse_y, amount) then model.focus = "amount" end
  end

  -- What it will be sent as, so the network is never a surprise.
  --
  -- On the amount's own row, in the space to its right: an amount is four
  -- characters wide and the field cannot honestly be as wide as the frame, so
  -- that gap was going to hold something. Right-aligned against the frame
  -- rather than at a fixed offset, which is what ran it off the edge when the
  -- network name got longer.
  if model.info then
    -- Where it is going, and nothing more: the amount field's own label
    -- carries the unit, and a chip that repeated it grew wide enough to be
    -- drawn through the amount box beside it. Gold when the asset is a token,
    -- because the colour is what makes the difference noticeable at a glance
    -- and it costs no width at all.
    local asset = model:asset()
    local name = model.info.network or "?"
    local width = theme.width(name, theme.font.small) + 10
    theme.text_right("SENDING ON", form.x + form.w, amount.y - 13,
      theme.colour.faint, theme.font.small)
    widgets.chip(form.x + form.w - width, amount.y + 3, name,
      asset.is_token and theme.colour.gold or theme.colour.cyan)
  end

  local send = { x = form.x, y = form.y + form.h - 25, w = form.w, h = 21 }
  if widgets.button(game.springs, "send", send,
      game.launch and "LAUNCHING…" or "SEND >", state,
      { colour = theme.colour.gold,
        -- Nothing to send *from* is as good a reason to refuse as being busy —
        -- and so is an asset this wallet reads but cannot move.
        disabled = model:busy() or game.launch ~= nil or from == nil
          or not model:asset().transferable }) then
    model:begin_send(model.form.to, model.form.amount)
  end

  -- The keyboard shortcuts go at the foot of the launch column rather than
  -- under the SEND button, where they used to hang through the bottom of the
  -- frame and into the action bar. On a plate of their own, because the
  -- exhaust falls straight through this spot and faint text under a shower of
  -- embers is text nobody reads.
  local legend = { x = pad.x, y = pad.y + pad.h - 28, w = pad.w, h = 26 }
  theme.rect(theme.colour.void, legend.x, legend.y, legend.w, legend.h, 0.85)
  theme.rule(legend.x, legend.y, legend.w, theme.colour.raised, 0.5)
  theme.text_centred("CTRL+V PASTES", legend.x + legend.w / 2, legend.y + 2,
    theme.colour.faint, theme.font.small)
  theme.text_centred("ENTER SENDS", legend.x + legend.w / 2, legend.y + 13,
    theme.colour.faint, theme.font.small)
end

local function draw_network(model, state, x)

  -- Every network *and* every token, in one flat list, narrowed by the search
  -- box at the top of the frame.
  --
  -- The screen used to fit everything at once, on the principle that a row you
  -- have to scroll to find is a row the wallet has hidden from you. The
  -- principle is intact; what changed is what makes it true. There are twenty
  -- rows now and there will be many more — every stablecoin on every chain —
  -- and fitting them all would mean rows too small to read or to hit, which
  -- hides them far more thoroughly than a scrollbar. So: the list still opens
  -- whole, the search only ever narrows it, and one word brings back anything
  -- that scrolled away. How the frame divides is `layout.net_grid`, so the
  -- sums can be tested.
  local rows = model:rows()
  local total = model:row_total()
  local query = model:search()

  -- The border's right-hand end says how much of the list is on screen. It
  -- goes there because the alternative is a line inside the frame, and a line
  -- inside the frame is a row that cannot be a row. A filtered list that does
  -- not say it is filtered is a list that has quietly lost things.
  local note = query ~= "" and ("%d/%d"):format(#rows, total) or nil
  local frame = widgets.frame(x + L.net.x, L.net.y, L.net.w, L.net.h, "NETWORK",
    { note = note })

  -- The sentence under the grid is wrapped before the grid is measured, so the
  -- rows know how many lines they have to leave room for. On a phone's width
  -- it is two, and it used to be one line 370 pixels wide printed across a
  -- 244-pixel frame — off both edges and over the town behind it.
  -- The fee ceiling belongs on this screen and nowhere else: it is a property
  -- of the network, not of a transfer, and it must never sit beside an amount
  -- field where it would read as the price being chosen rather than the
  -- refusal being set. Shown, not editable — the number's whole value is that
  -- it works untouched, and the only edit anyone reaches for is raising it,
  -- which is what the endpoint quoting an absurd fee was hoping for. Someone
  -- who genuinely means it can say so on the command line.
  --
  -- Always with its denomination. Fees are paid in a different token on every
  -- chain, and on Midnight in a different token from the balance drawn two
  -- frames away: "2" read as NIGHT rather than DUST is out by a billion.
  local ceiling = model.info and model.info.max_fee
  local sentence = "the store is shared - a wallet works on either"
  if ceiling then
    sentence = ("refuses a fee over %s %s - %s"):format(
      ceiling, model.info.max_fee_symbol or "", sentence)
  end
  local footer = wrap(sentence, frame.w - 8)
  local grid = layout.net_grid(frame.w, frame.h, #rows, #footer)

  -- The search box, on the frame's own top line. Focused whenever this screen
  -- is up: the keyboard has nothing else to do here, so typing narrows rather
  -- than requiring a click into a field first. That is the difference between
  -- a search box people use and one they find later.
  local searching = model.screen == "network"
  widgets.field(game.springs, "netsearch", {
    x = frame.x, y = frame.y, w = frame.w, h = grid.search.h,
  }, query, nil, searching, {
    placeholder = "type a tag - usdc, stablecoin, evm, testnet, faucet",
    colour = #rows == 0 and query ~= "" and theme.colour.red or nil,
  })

  -- What scrolled off, if anything. `net_scroll` is in whole grid rows so a
  -- row is never half on screen.
  local hidden_rows = math.max(0, grid.rows - grid.visible_rows)
  game.net_scroll = math.max(0, math.min(hidden_rows, game.net_scroll or 0))

  -- Scroll to whatever the window is aimed at, when that changes.
  --
  -- Twenty rows do not fit, and the marked one is often not among the first
  -- eight — pick USDC on Cronos from a search, clear the search, and the
  -- screen showed no ● anywhere, which reads as "nothing is selected" rather
  -- than "it is further down". Only on a change, so scrolling away afterwards
  -- stays where it was put.
  local marked = model.token and model.token.key
    or (model.info and model.info.network)
  if marked ~= game.net_revealed then
    game.net_revealed = marked
    for i, entry in ipairs(rows) do
      local is_token = entry.kind == "token"
      if is_token == (model.token ~= nil) and entry.key == marked then
        local at = game.net_scroll * grid.columns
        if i <= at or i > at + grid.visible then
          -- The least scrolling that brings it on screen, in whole grid rows.
          game.net_scroll = math.max(0, math.min(hidden_rows,
            math.ceil((i - grid.visible) / grid.columns)))
        end
        break
      end
    end
  end
  local first = game.net_scroll * grid.columns

  -- A chevron at the right-hand end of the search row, pointing at whichever
  -- way there is more. Two characters rather than a scrollbar: the frame has
  -- no width to spare, and what a reader needs to know is only that the list
  -- continues and which way.
  if hidden_rows > 0 then
    local arrows = ""
    if game.net_scroll > 0 then arrows = arrows .. "^" end
    if game.net_scroll < hidden_rows then arrows = arrows .. "v" end
    theme.text_right(arrows, frame.x + frame.w - 5,
      frame.y + math.floor((grid.search.h - 8) / 2), theme.colour.cyan,
      theme.font.small)
  end

  for i = first + 1, math.min(#rows, first + grid.visible) do
    local place = i - first - 1
    local column = math.floor(place / grid.visible_rows)
    local row = place % grid.visible_rows
    local network = rows[i]
    local box = {
      x = frame.x + column * (grid.column_w + grid.column_gap),
      y = frame.y + grid.rows_y + row * (grid.row_h + grid.gap),
      w = grid.column_w,
      h = grid.row_h,
    }
    -- Exactly one row wears the mark, and it is the one the window is aimed
    -- at: the token if a token is selected, the network otherwise. Two marked
    -- rows would be the screen saying it was in two places.
    local current
    if network.kind == "token" then
      current = model.token ~= nil and model.token.key == network.key
    else
      current = model.token == nil
        and model.info and model.info.network == network.key
    end
    local clicked, hovered, row_x, slide = widgets.row(game.springs, "net" .. i, box, state,
      current)
    if clicked then model:pick_row(network) end

    theme.rect(theme.colour.void, row_x, box.y, box.w, box.h, current and 0.5 or 0.35)
    theme.outline(current and theme.colour.green or theme.colour.raised,
      row_x, box.y, box.w, box.h, current and 0.8 or 0.4)

    local tint = theme.chain_colour(network.chain)
    -- A globe for a place you can go, a coin for a thing you can hold. The two
    -- kinds of row do different things when clicked — one moves the wallet,
    -- one reads a balance — so they must not look like one kind of row.
    sprite.draw_glowing(network.kind == "token" and "coin" or "globe",
      row_x + 18, box.y + box.h / 2, math.min(22, box.h - 2), {
      angle = game.time * (current and 0.5 or 0.15),
      glow = current and 0.6 or 0.15,
      glow_colour = current and theme.colour.green or tint,
      alpha = 0.5 + slide * 0.5,
    })

    local ink = current and theme.colour.green or (hovered and theme.colour.text
      or theme.colour.dim)

    -- One thing at the right-hand end, never two. The chip used to be placed
    -- at a fixed offset from that end — which is where the ticker already is —
    -- so the single row the chip marks was the single row whose ticker could
    -- not be read, "NOW" and "TCRO" printed through each other. On the current
    -- network the badge *is* the right-hand end: which network you are on is
    -- what that row is being asked, and a 224-pixel row cannot hold a name, a
    -- ticker and a badge at a size anybody can read.
    -- One thing at the right-hand end, never two: a row this narrow cannot
    -- hold a name, a ticker and a badge at a size anybody can read.
    --
    -- Which one gives depends on the kind of row. For a network the ticker is
    -- derivable from the name and the badge is the answer to the question the
    -- row is being asked, so the badge wins. For a token the ticker *is* the
    -- identity — three rows on this screen say `cronos-mainnet`, and only the
    -- symbol tells them apart — so it stays, and being current is said by the
    -- green it is drawn in and the frame around it.
    local tail = row_x + box.w - 8
    if current and network.kind ~= "token" then
      local chip_w = theme.width("NOW", theme.font.small) + 10
      tail = tail - chip_w
      widgets.chip(tail, box.y + math.floor((box.h - 14) / 2), "NOW",
        theme.colour.green)
    else
      theme.text_right(network.symbol, tail,
        box.y + math.floor(box.h / 2) - 4,
        current and theme.colour.green or tint, theme.font.small)
      tail = tail - theme.width(network.symbol, theme.font.small)
    end

    -- The key, not the name: `solana-devnet` says the chain and the network in
    -- the width a row of this size has, where "Solana Devnet" says one of them
    -- and "Cronos EVM Testnet" runs off the end. Clipped to where the tail
    -- starts, so a key longer than any of today's is cut off rather than
    -- printed through whatever is over there.
    -- A token row prints the network it is on, not its whole key: the ticker
    -- at the right-hand end already says USDC, and `usdc-cronos-mainnet`
    -- alongside it both repeats the symbol and runs out of row — it was
    -- clipped to `usdc-cronos-mainne`, which names no network at all. Read
    -- across, the row still says the flat name it has: USDC · cronos-mainnet.
    local said = network.kind == "token" and network.network or network.key
    local name = { x = row_x + 34, y = box.y, w = tail - 6 - (row_x + 34), h = box.h }
    theme.clip(name, function()
      theme.text(said, name.x, box.y + math.floor(box.h / 2) - 5, ink,
        theme.font.body)
    end)
  end

  -- A miss says so, rather than leaving an empty frame to be read as a bug.
  -- Where the rows would have been, so the eye finds it without being led.
  if #rows == 0 then
    theme.text_centred("nothing matches - backspace to bring it all back",
      frame.x + frame.w / 2, frame.y + grid.rows_y + 12, theme.colour.faint,
      theme.font.small)
  end

  -- The sentence the grid left room for, on the lines it was wrapped into.
  for i, line in ipairs(footer) do
    theme.text_centred(line, frame.x + frame.w / 2,
      frame.y + grid.footer_y + (i - 1) * grid.line_h, theme.colour.faint,
      theme.font.small)
  end
end

local function draw_status(model)
  local status = model.status
  if not status then return end
  local colour = status.kind == "error" and theme.colour.red
    or (status.kind == "busy" and theme.colour.amber or theme.colour.dim)
  -- Bounded by where the next button starts, and trimmed to fit rather than
  -- to a guessed character count: a balance is as long as it is, and it used
  -- to run straight under REFRESH.
  local x = status.kind == "error" and 112 or 100
  if status.kind == "error" then
    sprite.draw("skull", 103, L.bar + 9, 13, { alpha = 0.9 })
  end
  -- As far as the next thing in the bar, which is not the same on every
  -- screen: the wallet screen puts five buttons in the right column and the
  -- others put nothing there at all. Measuring to the wallet screen's buttons
  -- everywhere cut a message that had the width of the window to spread out
  -- in — "the recipient is this…" is not a refusal anybody can act on.
  local edge = model.screen == "wallets" and L.status_edge or (theme.WIDTH - L.margin)
  local room = edge - x
  local text = status.text
  while theme.width(text, theme.font.small) > room and #text > 1 do
    text = text:sub(1, -2)
  end
  if text ~= status.text then text = text:sub(1, -2) .. "…" end
  theme.text(text, x, L.bar + 3, colour, theme.font.small)
end

--- The dialog that stands between a click and a file on disk.
---
--- Both files this window writes go somewhere a person did not choose: the
--- wallet's own home, `~/.causewaybaywallet` unless something said otherwise.
--- One of them holds every private key in the wallet. Somebody who does not
--- know where it landed cannot move it, cannot delete it, and cannot tell
--- whether the copy they later find is the only one — so the path is the
--- loudest thing on this panel, spelled out in full and never abbreviated.
local function draw_write(model, state)
  if game.write_t < 0.01 then return end
  local pending = model.write
  if not pending then return end

  local files = pending.files or {}
  local accent = pending.secret and theme.colour.red or theme.colour.green
  -- Sized to its contents: one line per file, however many the path wraps to,
  -- and however many the warning does. A fixed height would leave the
  -- four-file save gaping and push a long path through the buttons — and a
  -- home is as long as somebody's home directory is.
  -- As wide as the canvas allows, up to the width it has always been. The 68
  -- taken off either side is a wide canvas's margin: on a phone's it left a
  -- 202-pixel dialog with 24 pixels of margin to spare, the title crammed
  -- into the sprite beside it and the warning wrapped to four lines.
  local w = math.min(theme.WIDTH - 24, 412)
  local inner = w - 24
  -- `~/.causewaybaywallet` rather than `/Users/somebody/.causewaybaywallet`:
  -- the same directory, said the way a person says it, and short enough to
  -- read in one line. There is only ever this one directory — the wallet's own
  -- home — and every file this window writes goes into it.
  local where = wrap(Model.tilde(pending.dir), inner)
  local note = wrap(pending.note or "", inner)
  local h = 90 + (#where + #files + #note) * 11
  local box = widgets.dialog({ x = math.floor((theme.WIDTH - w) / 2),
    y = math.floor((theme.HEIGHT - h) / 2), w = w, h = h },
    game.write_t, pending.title)

  local alpha = box.eased
  sprite.draw_glowing(pending.secret and "key" or "wallet",
    box.x + box.w - 26, box.y + 14, 22, {
      angle = math.sin(game.time * 3) * 0.12,
      glow = 0.7, glow_colour = pending.secret and theme.colour.red or theme.colour.green,
    })

  local y = box.y + 26
  theme.text("WRITING TO", box.x + 12, y, theme.colour.faint, theme.font.small, alpha)
  y = y + 13

  -- The directory, in full. This is the answer to the only question this
  -- dialog exists to answer, so it is the one thing drawn in the wallet's own
  -- colour rather than in ink.
  for _, line in ipairs(where) do
    theme.text(line, box.x + 12, y, theme.colour.cyan, theme.font.small, alpha)
    y = y + 11
  end

  y = y + 4
  for _, name in ipairs(files) do
    theme.text("·", box.x + 12, y, theme.colour.faint, theme.font.small, alpha)
    theme.text(name, box.x + 22, y, theme.colour.text, theme.font.small, alpha)
    y = y + 11
  end

  y = y + 4
  theme.rule(box.x + 12, y, box.w - 24, theme.colour.raised, 0.6 * alpha)
  y = y + 6
  -- 11 to the line, like the rows above it. At 10 the descenders of one line
  -- landed inside the caps of the next — the "y" of "keys." read as a stray
  -- mark after "owns" — and this is the paragraph that says who owns the money.
  for _, line in ipairs(note) do
    theme.text(line, box.x + 12, y, accent, theme.font.small, alpha)
    y = y + 11
  end

  theme.text_right(("%d wallets"):format(pending.count or 0),
    box.x + box.w - 12, box.y + 26, theme.colour.faint, theme.font.small, alpha)

  -- Wide enough for the verb it is showing: WRITE KEYS is 82 pixels of text in
  -- a button that was 80 wide, so the one button that writes secrets to disk
  -- had its label crossing its own border at both ends.
  local no = { x = box.x + 16, y = box.y + box.h - 24, w = 70, h = 16 }
  local yes_w = math.max(80, theme.width(pending.verb, theme.font.body) + 16)
  local yes = { x = box.x + box.w - 16 - yes_w, y = box.y + box.h - 24,
    w = yes_w, h = 16 }
  if widgets.button(game.springs, "write_no", no, "CANCEL", state,
      { colour = theme.colour.red }) then
    model:cancel_write()
  end
  if widgets.button(game.springs, "write_yes", yes, pending.verb, state,
      { colour = accent }) then
    model:confirm_write()
  end
end

local function draw_confirm(model, state)
  if game.confirm_t < 0.01 then return end
  local plan = model.confirm
  local network = model.info and model.info.network
  -- 11 to the line: at 10 an address's descenders sat in the next line's
  -- digits, and every line in this dialog is somebody's money.
  local line = 11

  -- Measured before it is drawn, because what it has to say is not the same
  -- width everywhere. An address is 42 characters: on a wide canvas that is
  -- one line, which is what the 184-pixel box was hand-sized around, and on a
  -- phone's it is two — so that box had both addresses running off its edges,
  -- the summary printed through the buttons, and the network key drawn over
  -- the amount. Nothing here is a fixed height any more.
  local w = math.min(theme.WIDTH - 24, 412)
  local inner = w - 24
  -- With its denomination, always. A bare "25" in a dialog whose window may
  -- be aimed at USDC or at CRO is the one number here that could be read as
  -- either, and this dialog is somebody's money.
  local amount = tostring(plan and plan.amount or "?")
  if plan and plan.symbol and plan.symbol ~= "" then
    amount = amount .. " " .. plan.symbol
  end
  local from = wrap(plan and plan.from or "?", inner)
  local to = wrap(plan and plan.to or "?", inner)
  local words = wrap(plan and plan.summary or "", inner)
  -- The amount and the network share a row where both fit and take one each
  -- where they do not.
  local abreast = 76 + theme.width(amount, theme.font.body) + 8
    + theme.width(network or "", theme.font.small) <= inner

  local h = 20                                  -- the title and its rule
    + (#from + 1) * line + 8                    -- FROM: the label, the address
    + 12                                        -- the arrow between the two
    + (#to + 1) * line + 8                      -- TO
    + 6 + line + (abreast and 0 or line)        -- the rule, then what and where
    + 8 + #words * line                         -- the wallet's own words
    + 34                                        -- the buttons, and air above
  h = math.min(h, theme.HEIGHT - 32)

  local box = widgets.dialog({
    x = math.floor((theme.WIDTH - w) / 2),
    y = math.max(16, math.floor((theme.HEIGHT - h) / 2)),
    w = w,
    h = h,
  }, game.confirm_t, "CONFIRM TRANSFER")
  if not plan then return end

  sprite.draw_glowing("key", box.x + box.w - 26, box.y + 14, 22, {
    angle = math.sin(game.time * 3) * 0.12,
    glow = 0.7, glow_colour = theme.colour.amber,
  })

  -- Labelled rows, not a sentence to be read carefully.
  --
  -- This used to be the wallet's summary alone, centred and wrapped: "Send 1
  -- TCRO from account-2 to 0xB32d…" — where the only thing identifying the
  -- payer is a label, and the one full address on screen is the *recipient*,
  -- sitting directly under the word "from". Two wallets with similar labels,
  -- or a glance rather than a read, and it says the opposite of what it means.
  --
  -- So who pays and who is paid are two rows, both spelled out in full, and
  -- neither is inferred from the other.
  local alpha = box.eased
  local y = box.y + 20

  local function party(label, colour, name, address)
    theme.text(label, box.x + 12, y, theme.colour.faint, theme.font.small, alpha)
    if name then
      theme.text(name, box.x + 58, y, colour, theme.font.small, alpha)
    end
    y = y + line
    for _, text in ipairs(address) do
      theme.text(text, box.x + 12, y, colour, theme.font.small, alpha)
      y = y + line
    end
    y = y + 8
  end

  party("FROM", theme.colour.amber, plan.from_label, from)
  -- The direction as a picture rather than a word, on the left where the two
  -- labels line up, so it reads down the column instead of floating.
  theme.text("|", box.x + 20, y - 4, theme.colour.faint, theme.font.small, alpha * 0.7)
  theme.text("v", box.x + 20, y + 2, theme.colour.faint, theme.font.small, alpha * 0.7)
  y = y + 12
  party("TO", theme.colour.cyan, nil, to)

  theme.rule(box.x + 12, y, box.w - 24, theme.colour.raised, 0.6 * alpha)
  y = y + 6
  theme.text("AMOUNT", box.x + 12, y, theme.colour.faint, theme.font.small, alpha)
  theme.text(amount, box.x + 76, y, theme.colour.text, theme.font.body, alpha)
  if network then
    -- Beside the amount, or under it. Right-aligning a fourteen-character
    -- network key into a dialog this narrow printed it through the number.
    if not abreast then y = y + line end
    theme.text_right(network, box.x + box.w - 12, y, theme.colour.dim,
      theme.font.small, alpha)
  end
  y = y + line + 8

  -- The wallet's own words underneath, still. It priced this — the nonce, the
  -- gas, the balance check are all behind that sentence — and the rows above
  -- are a reading of it, not a replacement for it.
  for _, text in ipairs(words) do
    theme.text_centred(text, box.x + box.w / 2, y, theme.colour.faint,
      theme.font.small, alpha * 0.9)
    y = y + line
  end

  local no = { x = box.x + 16, y = box.y + box.h - 24, w = 70, h = 16 }
  local yes = { x = box.x + box.w - 86, y = box.y + box.h - 24, w = 70, h = 16 }
  if widgets.button(game.springs, "no", no, "CANCEL", state, { colour = theme.colour.red }) then
    model:cancel_send()
  end
  if widgets.button(game.springs, "yes", yes, "SEND IT", state,
      { colour = theme.colour.green }) then
    if model:confirm_send() then begin_launch() end
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

  -- The line under it is not scaled with the pop: a path read at 1.4× and
  -- shrinking is a path nobody can read. It sits still, on a plate of its own
  -- — this lands over the wallet list and a card, and faint text over a wall
  -- of hex is not a path anybody can read either — and only fades.
  if toast.detail then
    local width = theme.width(toast.detail, theme.font.small) + 12
    local y = 66 - rise + 26
    theme.rect(theme.colour.void, (theme.WIDTH - width) / 2, y, width, 14, 0.88 * fade)
    theme.outline(theme.colour.raised, (theme.WIDTH - width) / 2, y, width, 14, 0.6 * fade)
    theme.text_centred(toast.detail, theme.WIDTH / 2, y + 1,
      theme.colour.text, theme.font.small, fade)
  end
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
    theme.frame(function()
      game.boot:draw()
      -- The way back out of a fullscreen window, from the very first screen.
      -- Not during the black hold or the power-on flash, which are a machine
      -- coming up and not a screen with controls on it.
      if game.boot:lit() then
        draw_mode_button(mode_box(), mouse_state())
        draw_layout_button(layout_box(), mouse_state())
      end
    end)
    game.clicked = false
    return
  end

  if game.login then
    theme.frame(function()
      sprite.backdrop("krumlov", {
        alpha = 0.85, scrim = 0.66,
        drift_x = math.sin(game.time * 0.09) * 3,
        drift_y = math.cos(game.time * 0.07) * 2,
      })
      game.stars:draw(game.time)
      game.login:draw(game.model, mouse_state(), game.springs)
      draw_mode_button(mode_box(), mouse_state())
      draw_layout_button(layout_box(), mouse_state())
      game.fx:draw(sprite.images)
      theme.scanlines(0.10)
      theme.vignette(0.4)
    end)
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
    local shake_x, shake_y = anim.shake_offset(game.time, game.shake)
    love.graphics.push()
    love.graphics.translate(shake_x, shake_y)

    if game.error then
      draw_fatal()
    else
      local model = game.model
      local state = mouse_state()

      -- A dialog is modal for the mouse as well as for the keyboard.
      --
      -- One `state` used to reach both the screen and the dialog, and the
      -- dialog is drawn last — so every widget underneath saw the same click
      -- first. The screen's own SEND button sits directly under the dialog's
      -- SEND IT, overlapping by a few pixels, which meant confirming a
      -- transfer *also* started a second one: the wallet priced it again and
      -- the confirmation reappeared, on top of a send that had gone through.
      --
      -- The keyboard had this right already — `love.keypressed` gives the
      -- dialog the keys and returns. This is the same rule for clicks.
      local behind = state
      if model and model:asking() then
        behind = { mouse_x = state.mouse_x, mouse_y = state.mouse_y, clicked = false }
      end

      -- The entrance eases everything down from above on the first frames.
      local drop = (1 - game.entrance.value) * -30
      local slide = game.screen_slide.value * 26

      love.graphics.push()
      love.graphics.translate(0, drop)
      draw_header(model)
      if model then
        draw_header_buttons(model, behind)
        draw_tabs(model, behind)
        if model.screen == "wallets" then
          draw_wallets(model, behind, slide)
        elseif model.screen == "send" then
          draw_send(model, behind, slide)
        else
          draw_network(model, behind, slide)
        end
        draw_status(model)
      end
      love.graphics.pop()

      game.fx:draw(sprite.images)
      draw_toast()
      if model then
        draw_confirm(model, state)
        draw_write(model, state)
      end
    end

    love.graphics.pop()

    -- The warning every front end shows, in the one place it cannot be missed.
    theme.rect(theme.colour.void, 0, L.bar - 4, theme.WIDTH, theme.HEIGHT - L.bar + 4, 0.5)
    theme.rect(theme.colour.void, 0, theme.HEIGHT - 17, theme.WIDTH, 17, 0.9)
    theme.rule(0, theme.HEIGHT - 17, theme.WIDTH, theme.colour.raised, 0.5)
    -- The banner is one line tall by design — it is the last 17 rows of every
    -- layout — so the sentence has to be the one that fits rather than the one
    -- that wraps. On a phone's width the long form ran off both ends of the
    -- canvas, which is a warning nobody can finish reading; the short form
    -- still says both things it has to say.
    local warning = "EDUCATIONAL · KEYS ARE STORED UNENCRYPTED"
    if theme.width(warning, theme.font.small) > theme.WIDTH - 8 then
      warning = "EDUCATIONAL · KEYS UNENCRYPTED"
    end
    theme.text_centred(warning, theme.WIDTH / 2,
      theme.HEIGHT - 16, theme.colour.faint, theme.font.small)

    -- Absent art and absent sound both say so, in one line, once. A missing
    -- effect is silent by design, and silence is exactly what a working mute
    -- looks like — so without this the two are indistinguishable.
    local absent = {}
    for _, name in ipairs(sprite.missing) do absent[#absent + 1] = name end
    for _, name in ipairs(sound.missing) do absent[#absent + 1] = name .. ".wav" end
    if #absent > 0 then
      theme.text("assets missing: " .. table.concat(absent, " "), 4, 48,
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
-- screen to grab, `CWB_SHOT_KEYS` replays keypresses first, so a shot of the
-- confirmation dialog does not need a human at the keyboard, and
-- `CWB_SHOT_TALL=1` turns the canvas on its side for the picture (see
-- `love.load`) without remembering the turn.

local shot = {
  path = os.getenv("CWB_SHOT"),
  after = tonumber(os.getenv("CWB_SHOT_AFTER") or "2"),
  screen = os.getenv("CWB_SHOT_SCREEN"),
  keys = os.getenv("CWB_SHOT_KEYS"),
  taken = false,
  queue = {},
  hold = 0,
}

--- Drive the game the way a person would, before the shot is taken.
---
--- The boot screen owns the keyboard while it is up, so a replay that started
--- typing straight away had its keystrokes swallowed and produced a shot of an
--- empty form. It is dismissed outright here instead: a screenshot of a screen
--- is not the place to also exercise the boot sequence.
local function replay()
  -- `CWB_SHOT_SCREEN=boot` photographs the boot sequence itself, which is the
  -- one screen the harness could not reach: it used to dismiss the boot before
  -- doing anything, so there was no way to take a picture of the thing that
  -- happens first.
  if shot.screen == "boot" then
    for step in (shot.keys or ""):gmatch("[^,]+") do
      shot.queue[#shot.queue + 1] = step
    end
    return
  end

  game.boot = nil
  game.entrance:restart()
  -- `CWB_SHOT_SCREEN=login` photographs the gate; anything else goes straight
  -- past it, because a shot of a wallet screen should not have to log in first.
  -- Created, then the keys fall through below — returning here meant a
  -- scripted mnemonic was never typed and the shot showed an empty field.
  if shot.screen == "login" then
    game.login = Login.new()
  else
    game.login = nil
    if shot.screen and game.model then
      game.model:go(shot.screen)
      game.screen_slide:set(0)
    end
  end
  if not shot.keys then return end
  for step in shot.keys:gmatch("[^,]+") do
    shot.queue[#shot.queue + 1] = step
  end
end

--- Perform the replayed steps, pausing wherever one says to.
---
--- Everything used to happen in a single burst at 0.35s, which made half the
--- animations in this file unphotographable: by the time a shot could be taken
--- the card had finished turning, and taking it earlier caught the entrance
--- instead. A `wait:0.4` step lets a shot say *settle first, then press this*,
--- which is the only way to photograph the middle of something.
local function advance_replay(dt)
  if shot.hold > 0 then
    shot.hold = shot.hold - dt
    return
  end
  while shot.queue[1] do
    local step = table.remove(shot.queue, 1)
    local text = step:match("^type:(.*)$")
    local pause = step:match("^wait:(.*)$")
    if text then
      love.textinput(text)
    elseif pause then
      shot.hold = tonumber(pause) or 0
      return
    elseif step == "confirm" then
      -- The confirmation needs a funded account and a node that answers, so
      -- the dialog was the one screen no picture could be taken of. The plan
      -- is planted directly; every field in it is what the real one carries.
      if game.model then
        -- In whatever the window is aimed at, so a shot of this dialog with a
        -- token selected shows the token — the same as the real one would.
        local asset = game.model:asset()
        local unit = asset.symbol or "TCRO"
        game.model.confirm = {
          summary = "Send " .. tostring(game.model.form.amount) .. " " .. unit
            .. " from " .. tostring(game.model:active_label())
            .. " to " .. tostring(game.model.form.to)
            .. " on " .. tostring(asset.network) .. ", fee about 0.000021 TCRO",
          from = game.model.active,
          from_label = game.model:active_label(),
          to = game.model.form.to,
          amount = game.model.form.amount,
          symbol = unit,
          argv = game.model:send_argv(game.model.form.to, game.model.form.amount),
        }
      end
    elseif step:match("^click:") then
      -- A press at a point on the canvas, for the buttons that no key reaches.
      -- The pointer stays where it was put, so the shot also shows the button
      -- lit under it — which is what a picture of a click should look like.
      --
      -- `click:390x12`, not `click:390,12`: the comma already separates one
      -- step from the next, so a pair written with one arrives here as two
      -- steps, the second of which is the number 12 and means nothing.
      local cx, cy = step:match("^click:(-?%d+)x(-?%d+)$")
      if cx then
        game.pointer = { x = tonumber(cx), y = tonumber(cy) }
        game.clicked = true
      end
    elseif step:match("^token:") then
      -- Aim the window at one registry row, the way clicking it does. By key
      -- rather than by canvas position, so a shot does not silently start
      -- photographing a different row when the grid's arithmetic changes.
      local key = step:match("^token:(.+)$")
      if game.model then
        for _, entry in ipairs(game.model:rows()) do
          if entry.key == key then
            game.model:pick_row(entry)
            break
          end
        end
      end
    elseif step == "save" or step == "keys" then
      -- The two dialogs that stand in front of a file being written. They open
      -- on a click rather than a key, so there is no keypress to replay.
      if game.model then
        if step == "save" then game.model:ask_save() else game.model:ask_export() end
      end
    elseif step == "mint" then
      -- NEW MNEMONIC has no key, only a button, so a shot of the
      -- mint-then-unlock path cannot be replayed without this.
      if game.login and game.model then
        local phrase = game.model:offer_mnemonic(12)
        if phrase then
          game.login.phrase = phrase
          game.login.minted = true
          game.login.copied = false
        end
      end
    elseif step == "launch" then
      -- The one thing no keypress can reach: the rocket only lifts off once a
      -- transfer is confirmed, and confirming one needs a funded account and a
      -- node. The flight itself is pure animation, so it is started directly
      -- and `CWB_SHOT_AFTER` picks the frame.
      begin_launch()
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
    if replayed then advance_replay(dt) end
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
