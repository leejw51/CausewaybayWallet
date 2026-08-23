--- What the window knows, with nothing that draws it.
---
--- Every decision the GUI makes lives here: which screen is showing, which
--- wallet is selected, what is in flight, what the last thing to go wrong was.
--- There is not one `love.` call in this file, which is the point — a GUI whose
--- logic only runs inside `love.draw` cannot be tested, and this one is driven
--- headlessly by `tests/model_test.lua` exactly the way the CLI\'s interactive
--- loop is.
---
--- ## Work that blocks
---
--- Anything reaching a node — a balance, a send — takes a network round trip,
--- and doing that on the main thread freezes the window. So this module never
--- calls the wallet for those. It hands a request to `jobs`, carries on, and
--- takes the answer later through `pump`. In LÖVE `jobs` is the worker thread;
--- in the tests it is a table that answers immediately.
---
--- Requests are `{argv, stdin, yes}` rather than closures, because a closure
--- cannot cross a LÖVE thread boundary and neither can the wallet handle.
---
--- ## Confirming a send
---
--- The GUI does not compose the sentence it asks you to approve. It sends once
--- *without* `yes`, which makes the wallet resolve the nonce, the gas price and
--- the gas limit, check the balance covers all of it, and refuse with
--- `confirmation_required` carrying the summary it would have put to a human.
--- That summary is what the dialog shows, so what you approve is a transaction
--- that is real and already priced. It is the same trick the CLI\'s interactive
--- mode uses, for the same reason.

local Model = {}
Model.__index = Model

--- The screens, in the order the tabs show them.
Model.SCREENS = { "wallets", "send", "network" }

--- The fields the send screen tabs between.
Model.FIELDS = { "to", "amount" }

--- Create a model over an open wallet.
---
--- `jobs` needs `submit(request)` and `poll()`. Passing nil makes every call
--- synchronous, which is what the tests want and what a LÖVE build must never
--- do.
function Model.new(wallet, jobs)
  local self = setmetatable({
    wallet = wallet,
    jobs = jobs,
    screen = "wallets",
    wallets = {},
    active = nil,
    info = nil,
    balance = nil,
    status = nil,
    pending = {},
    next_id = 0,
    form = { to = "", amount = "" },
    confirm = nil,
    focus = "to",
    selected = 1,
    scroll = 0,      -- first visible row of the wallet list
    session = nil,   -- the wallet a mnemonic unlocked; nil means logged out
    -- Bumped whenever something happened that the view may want to celebrate;
    -- screens compare it against their own copy rather than being called back,
    -- which keeps the model free of anything view-shaped.
    events = {},
  }, Model)
  self:refresh()
  return self
end

-- -------------------------------------------------------------------- status

function Model:say(text, kind)
  self.status = { text = text, kind = kind or "info" }
end

function Model:fail(err)
  local code = err and err.code or "internal"
  local message = err and err.message or "something went wrong"
  self.status = { text = message, kind = "error", code = code }
  self:emit("error")
  return false
end

--- Record that something happened, for the view to react to.
function Model:emit(name)
  self.events[#self.events + 1] = name
end

--- Take every event since the last call.
function Model:drain()
  local taken = self.events
  self.events = {}
  return taken
end

function Model:busy()
  return next(self.pending) ~= nil
end

-- ------------------------------------------------------------------- session
--
-- A session gate, and worth being precise about what it is not. The store is
-- not encrypted and this does not encrypt it: a mnemonic here selects which
-- wallet the window is showing and gates the screens behind it, and anyone with
-- the disk still has the keys. The UI says so on the login screen, because a
-- lock that implies more safety than it provides is worse than no lock.

--- Unlock with a mnemonic.
---
--- The phrase is checked and derived *without touching the store* — that is
--- what `validate-mnemonic` and `derive` exist for. Only once an address is in
--- hand does this decide whether the wallet is already known (select it) or new
--- (import it), which means a typo never leaves a stray account behind.
function Model:login(phrase)
  if not phrase or phrase:gsub("%s", "") == "" then
    return self:fail({ code = "usage", message = "enter your mnemonic" })
  end

  local check, check_error = self.wallet:validate_mnemonic(phrase)
  if not check then return self:fail(check_error) end
  if not check.valid then
    return self:fail({ code = "invalid_mnemonic", message = check.reason or "not a valid phrase" })
  end

  local derived, derive_error = self.wallet:derive({ mnemonic = phrase })
  if not derived then return self:fail(derive_error) end

  for index, account in ipairs(self.wallets) do
    if account.address == derived.address then
      -- Already known: select it and let them in, storing nothing new.
      if not self:select(index) then return false end
      self.session = { address = derived.address, label = account.label }
      self:say("Welcome back, " .. account.label)
      self:emit("login")
      return account
    end
  end

  -- New to this store: import it, which is the one place a phrase is written.
  local account, import_error = self.wallet:import_mnemonic(phrase, {})
  if not account then return self:fail(import_error) end
  self:refresh()
  for index, entry in ipairs(self.wallets) do
    if entry.address == account.address then self.selected = index end
  end
  self.session = { address = account.address, label = account.label }
  self:say("Imported " .. account.label)
  self:emit("login")
  return account
end

--- Mint a phrase without storing it, for someone starting fresh.
---
--- Deliberately not imported here: it is shown first so it can be written
--- down, and only becomes an account when it is used to log in. A wallet whose
--- mnemonic was never seen is a wallet that cannot be recovered.
function Model:offer_mnemonic(words)
  local generated, err = self.wallet:new_mnemonic(words or 12)
  if not generated then return self:fail(err) end
  return generated.mnemonic
end

--- Leave. The store is untouched; this only forgets what the window was showing.
function Model:logout()
  self.session = nil
  self.balance = nil
  self.confirm = nil
  self.form.to, self.form.amount = "", ""
  self.screen = "wallets"
  self.scroll = 0
  self.status = nil
  self:emit("logout")
  return true
end

function Model:logged_in()
  return self.session ~= nil
end

-- ----------------------------------------------------------- local commands
--
-- These read the store on disk. No node is involved, so they run directly
-- rather than going to the worker.

function Model:refresh()
  local accounts, err = self.wallet:accounts()
  if not accounts then return self:fail(err) end
  self.wallets = accounts

  local info = self.wallet:info()
  self.info = info
  local active = info and info.active_address
  -- `json.null` decodes to a table, so an absent active account is flattened
  -- here rather than compared against in three places.
  self.active = type(active) == "string" and active or nil

  if self.selected > #self.wallets then self.selected = math.max(1, #self.wallets) end
  return true
end

function Model:create(label)
  local account, err = self.wallet:new_account({
    label = label ~= "" and label or nil,
  })
  if not account then return self:fail(err) end
  self:refresh()
  self:say("Created " .. account.label)
  self:emit("created")
  return account
end

function Model:select(index)
  local account = self.wallets[index]
  if not account then return false end
  local ok, err = self.wallet:use_account(account.address)
  if not ok then return self:fail(err) end
  -- A different wallet has a different balance; showing the old one beside a
  -- new name is worse than showing nothing.
  self.balance = nil
  self.selected = index
  self:refresh()
  self:say(account.label .. " is active")
  self:emit("selected")
  return true
end

function Model:switch_network(key)
  local ok, err = self.wallet:use_network(key)
  if not ok then return self:fail(err) end
  self.balance = nil
  self:refresh()
  self:say("Now on " .. key)
  self:emit("network")
  return true
end

function Model:networks()
  return self.wallet:networks() or {}
end

function Model:go(screen)
  for _, name in ipairs(Model.SCREENS) do
    if name == screen then
      self.screen = screen
      self:emit("screen")
      return true
    end
  end
  return false
end

-- ------------------------------------------------------------ work that waits

function Model:submit(request, on_answer)
  if not self.jobs then
    local envelope = self.wallet:envelope(request.argv, {
      stdin = request.stdin,
      yes = request.yes,
    })
    return on_answer(envelope or { ok = false, error = { code = "internal" } })
  end

  self.next_id = self.next_id + 1
  local id = self.next_id
  self.pending[id] = on_answer
  request.id = id
  self.jobs.submit(request)
  return id
end

--- Take whatever the worker has finished. Called once per frame.
function Model:pump()
  if not self.jobs then return 0 end
  local handled = 0
  while true do
    local id, envelope = self.jobs.poll()
    if not id then break end
    local on_answer = self.pending[id]
    self.pending[id] = nil
    if on_answer then
      on_answer(envelope)
      handled = handled + 1
    end
  end
  return handled
end

local function unwrap(envelope)
  if not envelope then
    return nil, { code = "internal", message = "the worker returned nothing" }
  end
  if envelope.ok then return envelope.data end
  return nil, envelope.error or { code = "internal", message = "no reason given" }
end

function Model:fetch_balance()
  if #self.wallets == 0 then
    return self:fail({ code = "no_active_account", message = "create a wallet first" })
  end
  self:say("Asking the node…", "busy")
  self:submit({ argv = { "balance" } }, function(envelope)
    local data, err = unwrap(envelope)
    if not data then return self:fail(err) end
    self.balance = data
    self:say(data.balance .. " " .. data.symbol)
    self:emit("balance")
  end)
  return true
end

-- --------------------------------------------------------------------- send

--- The tail `Headless::confirm` appends, which is advice for a shell and reads
--- as nonsense in a dialog with a button under it.
local CONFIRM_SUFFIX = " — re-run with --yes to confirm"

function Model.plan_summary(message)
  if not message then return nil end
  return (message:gsub(CONFIRM_SUFFIX:gsub("%p", "%%%0") .. "$", ""))
end

function Model:begin_send(to, amount)
  if to == "" or amount == "" then
    return self:fail({ code = "usage", message = "a recipient and an amount are needed" })
  end
  self:say("Pricing the transaction…", "busy")
  self:submit({
    argv = { "send", "--to", to, "--amount", amount },
    yes = false,
  }, function(envelope)
    if envelope and envelope.ok then
      -- Only reachable if the wallet was opened with `yes` already set, which
      -- this GUI must not do — the dialog is how it asks.
      self.confirm = nil
      self:say("Sent " .. ((envelope.data or {}).hash or ""))
      self:emit("sent")
      return
    end
    local err = (envelope or {}).error or {}
    if err.code ~= "confirmation_required" then return self:fail(err) end
    self.confirm = { summary = Model.plan_summary(err.message), to = to, amount = amount }
    self.status = nil
    self:emit("confirm")
  end)
  return true
end

function Model:confirm_send()
  local plan = self.confirm
  if not plan then return false end
  self.confirm = nil
  self:say("Signing and broadcasting…", "busy")
  self:submit({
    argv = { "send", "--to", plan.to, "--amount", plan.amount },
    yes = true,
  }, function(envelope)
    local data, err = unwrap(envelope)
    if not data then return self:fail(err) end
    self.form.to, self.form.amount = "", ""
    self.balance = nil -- it just changed
    self:say("Sent " .. (data.hash or ""))
    self:emit("sent")
  end)
  return true
end

function Model:cancel_send()
  if not self.confirm then return false end
  self.confirm = nil
  self:say("Cancelled — nothing was signed.")
  self:emit("cancelled")
  return true
end

-- --------------------------------------------------------------- text entry

function Model:type_into(text)
  if self.confirm then return end -- the dialog owns the keyboard
  self.form[self.focus] = (self.form[self.focus] or "") .. text
end

function Model:backspace()
  if self.confirm then return end
  local field = self.form[self.focus] or ""
  self.form[self.focus] = field:sub(1, -2)
end

--- Replace a field outright, as a paste does.
---
--- Trimmed, because a clipboard address copied out of a block explorer or a
--- chat message arrives wrapped in whitespace more often than not, and the
--- wallet would reject it for a reason nobody could see.
function Model:set_field(field, text)
  if self.confirm then return false end
  if type(text) ~= "string" then return false end
  self.form[field] = (text:gsub("^%s+", ""):gsub("%s+$", ""))
  self.focus = field
  return true
end

function Model:clear_field(field)
  if self.confirm then return false end
  self.form[field] = ""
  return true
end

--- Move the list window, keeping it inside the list.
---
--- `visible` is how many rows fit, which only the view knows — so it is passed
--- in rather than assumed here, and the model stays free of layout.
function Model:scroll_by(delta, visible)
  local most = math.max(0, #self.wallets - visible)
  self.scroll = math.max(0, math.min(most, self.scroll + delta))
  return self.scroll
end

--- Keep the selected row on screen after the selection moves.
function Model:reveal(index, visible)
  if index < self.scroll + 1 then
    self.scroll = index - 1
  elseif index > self.scroll + visible then
    self.scroll = index - visible
  end
  self.scroll = math.max(0, math.min(math.max(0, #self.wallets - visible), self.scroll))
end

function Model:next_field()
  for i, name in ipairs(Model.FIELDS) do
    if name == self.focus then
      self.focus = Model.FIELDS[i % #Model.FIELDS + 1]
      return
    end
  end
  self.focus = Model.FIELDS[1]
end

return Model
