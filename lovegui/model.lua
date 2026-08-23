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

--- File formats live next door, because they are pure text and this is not.
local export = require("export")

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
    -- Whether the selection has ever been aimed at the active account. See
    -- `refresh`: it happens once, and never again.
    aimed = false,
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

--- The label of the wallet being spent from, if it has one.
function Model:active_label()
  for _, account in ipairs(self.wallets) do
    if account.address == self.active then return account.label end
  end
  return nil
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
--- An error about a mnemonic, with the mnemonic taken back out of it.
---
--- The three calls that take a phrase — validate, derive, import — pass it as
--- an argument, and the argument parser quotes back anything it does not
--- understand. A phrase copied out of a bulleted list arrives as
--- "- abandon abandon …", the parser refuses it as an unexpected argument, and
--- the message it returns contains the whole phrase. That message goes into
--- `status.text`, and `status.text` is drawn.
---
--- ## This is a guard, not a live fix
---
--- The leak is currently unreachable, and it is worth being exact about why:
--- `login` calls `validate-mnemonic` first, that command answers
--- `{valid = false, reason = "unsupported word count 13"}` for anything
--- malformed without ever quoting its input, and `login` returns there. Only
--- phrases it approved — well-formed BIP-39, no leading hyphen — ever reach
--- `derive` or `import`.
---
--- So the ordering is what makes it safe, and nothing said so. That ordering
--- looks redundant: both commands reject the same phrases, which is exactly
--- the argument for deleting the extra call to save an FFI round trip. Delete
--- it and the quoting path opens.
---
--- The other half is that `status.text` is not drawn on the login screen
--- today. That is layout, not policy — a status line there is an obvious
--- thing to add, and adding it would publish whatever this held.
---
--- Two accidents standing between a mnemonic and the screen is not a promise.
--- A `usage` error is always the parser quoting its input and is replaced
--- outright; any other message is replaced only if it really contains the
--- phrase, so a genuine failure — the library missing, the store unwritable —
--- still says what went wrong.
---
--- Exposed on the module so it can be tested directly. The end-to-end path
--- cannot reach it, which means an end-to-end test would pass whether or not
--- this function did anything at all.
function Model.without_phrase(err, phrase)
  if not err then return err end

  if err.code == "usage" then
    return { code = "invalid_mnemonic", message = "that does not look like a mnemonic" }
  end

  local message = tostring(err.message or "")
  local trimmed = (tostring(phrase or ""):gsub("^%s+", ""):gsub("%s+$", ""))
  if trimmed == "" then return err end

  -- The whole phrase, or enough of it to matter. Three words in a row is the
  -- test rather than one, because "about" and "abandon" are ordinary words
  -- that a legitimate message could contain on its own.
  local head = trimmed:match("^(%S+%s+%S+%s+%S+)") or trimmed
  if message:find(trimmed, 1, true) or message:find(head, 1, true) then
    return { code = err.code, message = "that does not look like a mnemonic" }
  end
  return err
end

--- How far to look for the addresses a phrase controls, and when to give up.
---
--- BIP-44 wallets are scanned with a gap limit rather than to a fixed depth:
--- keep deriving until several indices in a row are absent from the store, and
--- stop. A wallet with accounts at 0, 1 and 5 is found; one with a hundred is
--- not scanned a hundred times on every login.
Model.SESSION_GAP = 5
Model.SESSION_MAX = 40

--- Every address this phrase controls, as a set keyed by lower-case address.
---
--- Derived rather than read. The store keeps each account's mnemonic, but
--- `accounts()` deliberately does not hand it out — which is correct, and
--- means the only honest way to ask "which of these wallets does this phrase
--- own?" is to derive the addresses and see which ones are there.
---
--- Addresses that are *not* in the store are kept in the set too. They cost
--- nothing, and the next account made in this session lands on one of them:
--- `account new` continues the active account's mnemonic, so it is simply the
--- next index of this same phrase.
function Model:derived_addresses(phrase, stored)
  local known = {}
  for _, account in ipairs(stored) do
    known[tostring(account.address):lower()] = true
  end

  local addresses, misses = {}, 0
  for index = 0, Model.SESSION_MAX - 1 do
    local derived = self.wallet:derive({ mnemonic = phrase, index = index })
    if not derived then break end
    local address = tostring(derived.address):lower()
    addresses[address] = true
    if known[address] then
      misses = 0
    else
      misses = misses + 1
      -- Index 0 is the wallet itself and is never a reason to stop: a phrase
      -- being imported for the first time has none of its addresses stored.
      if index > 0 and misses >= Model.SESSION_GAP then break end
    end
  end
  return addresses
end

function Model:login(phrase)
  if not phrase or phrase:gsub("%s", "") == "" then
    return self:fail({ code = "usage", message = "enter your mnemonic" })
  end

  local check, check_error = self.wallet:validate_mnemonic(phrase)
  if not check then return self:fail(Model.without_phrase(check_error, phrase)) end
  if not check.valid then
    return self:fail(Model.without_phrase(
      { code = "invalid_mnemonic", message = check.reason or "not a valid phrase" }, phrase))
  end

  local derived, derive_error = self.wallet:derive({ mnemonic = phrase })
  if not derived then return self:fail(Model.without_phrase(derive_error, phrase)) end

  -- Asked of the store, not of `self.wallets` — that list may still be scoped
  -- to the session which is ending, and the phrase being unlocked has nothing
  -- to do with it.
  local stored, list_error = self.wallet:accounts()
  if not stored then return self:fail(list_error) end

  local addresses = self:derived_addresses(phrase, stored)

  local account, welcome
  for _, entry in ipairs(stored) do
    if entry.address == derived.address then account = entry end
  end

  if account then
    welcome = "Welcome back, " .. account.label
  else
    -- New to this store: import it, which is the one place a phrase is written.
    local imported, import_error = self.wallet:import_mnemonic(phrase, {})
    if not imported then return self:fail(Model.without_phrase(import_error, phrase)) end
    account = imported
    welcome = "Imported " .. imported.label
  end

  -- Set before the refresh, so the list comes back already scoped to it.
  self.session = {
    address = account.address,
    label = account.label,
    addresses = addresses,
  }
  self.session.addresses[tostring(account.address):lower()] = true

  -- Made active, not merely shown. Logging in *as* a wallet while the money
  -- moves from a different one is exactly the mismatch this screen exists to
  -- prevent, and a known phrase and a new one used to disagree about it.
  local ok, use_error = self.wallet:use_account(account.address)
  if not ok then return self:fail(use_error) end
  self.balance = nil
  self:refresh()

  self.scroll = 0
  for index, entry in ipairs(self.wallets) do
    if entry.address == account.address then self.selected = index end
  end

  self:say(welcome)
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

--- Leave, and optionally destroy the store on the way out.
---
--- `opts.wipe` deletes every file the wallet keeps. That is the end of those
--- wallets: the mnemonics and the private keys are in those files and nowhere
--- else, so anything not exported first is gone. The caller is expected to
--- have asked, twice.
---
function Model:logout(opts)
  opts = opts or {}
  local wiped = nil
  if opts.wipe then
    wiped = self:wipe_store()
  end

  self.session = nil
  -- Unscoped again, so nothing of the session's view is left behind. The list
  -- is not on screen at this point; it is cleared because leaving stale state
  -- around for the next login to inherit is how the next bug gets in.
  self:refresh()
  self.balance = nil
  self.confirm = nil
  self.form.to, self.form.amount = "", ""
  self.screen = "wallets"
  self.scroll = 0
  self.status = nil
  self:emit("logout")
  if wiped then
    self:say(("Wiped the store - %d files removed"):format(wiped), "error")
    self:emit("wiped")
  end
  return true
end

--- The session as plain data, for remembering it between runs.
---
--- Addresses and a label. **No phrase and no key** — there is nothing secret
--- in here to write down, and that is deliberate: the point of remembering a
--- session is to skip the gate, not to keep the thing the gate asks for.
--- Everything in this snapshot is already public, and already in the store.
function Model:session_snapshot()
  if not self.session then return nil end
  local addresses = {}
  for address in pairs(self.session.addresses) do
    addresses[#addresses + 1] = address
  end
  table.sort(addresses)
  return {
    address = self.session.address,
    label = self.session.label,
    addresses = addresses,
  }
end

--- Put a remembered session back, if it still fits the store.
---
--- It may not. The wallet it named can have been wiped, or the home pointed
--- somewhere else, and a session over wallets that are not there is worse than
--- no session — it would show an empty bank and no way to say why. So this
--- checks the account is really there and refuses otherwise, which puts the
--- login screen back where it belongs.
function Model:restore_session(snapshot)
  if type(snapshot) ~= "table" or type(snapshot.address) ~= "string" then
    return false
  end

  local stored = self.wallet:accounts()
  if not stored then return false end

  local found
  for _, entry in ipairs(stored) do
    if entry.address == snapshot.address then found = entry end
  end
  if not found then return false end

  local set = {}
  for _, address in ipairs(snapshot.addresses or {}) do
    set[tostring(address):lower()] = true
  end
  set[tostring(found.address):lower()] = true

  local ok = self.wallet:use_account(found.address)
  if not ok then return false end

  self.session = { address = found.address, label = found.label, addresses = set }
  self.balance = nil
  self:refresh()
  self.scroll = 0
  for index, entry in ipairs(self.wallets) do
    if entry.address == found.address then self.selected = index end
  end
  self:say("Welcome back, " .. found.label)
  self:emit("login")
  return true
end

function Model:logged_in()
  return self.session ~= nil
end

-- ------------------------------------------------------------------- files
--
-- Writing the wallet list out, and clearing it away. Plain `io`, not
-- `love.filesystem`: these go where the wallet's own files are, which is
-- outside the sandbox, and it keeps the whole of this testable.

--- Where this window writes: the directory the wallet already keeps its store
--- in. Somewhere a person can find, beside the thing it describes.
function Model:home()
  return self.info and self.info.home or nil
end

local function write_file(path, contents, private)
  local handle, err = io.open(path, "w")
  if not handle then return nil, err end
  handle:write(contents)
  handle:close()
  if private then
    -- Owner-only. Lua cannot chmod, and a file of private keys left at the
    -- umask default is readable by anything else running as anyone on a
    -- shared machine. A failure here is not fatal — the file exists and the
    -- caller is told where — but it is the reason this is worth doing at all.
    os.execute(("chmod 600 %q 2>/dev/null"):format(path))
  end
  return path
end

--- Save the address list, in every format at once.
---
--- Public information only: labels, addresses, indices, derivation paths.
--- Losing this file costs nothing, which is exactly why it is a separate verb
--- from the one below.
---
--- Scoped to the session, like the list on screen. Exporting wallets a phrase
--- does not control would be the scoping quietly not applying to files.
function Model:save_wallets()
  local home = self:home()
  if not home then return self:fail({ code = "io_error", message = "no wallet home" }) end
  if #self.wallets == 0 then
    return self:fail({ code = "usage", message = "no wallets to save" })
  end

  local written = {}
  for name, contents in pairs(export.addresses(self.wallets)) do
    local path, err = write_file(home .. "/" .. name, contents, false)
    if not path then
      return self:fail({ code = "io_error", message = "cannot write " .. name .. ": " .. tostring(err) })
    end
    written[#written + 1] = name
  end
  table.sort(written)

  self:say(("Saved %d to %s"):format(#self.wallets, home))
  self:emit("saved")
  return written
end

--- Export everything needed to reconstruct these wallets somewhere else.
---
--- Mnemonics, private keys, both public keys, both spellings of the address.
--- Anyone who reads the file owns the money in it.
---
--- The public keys are not in the store — it keeps what it needs to sign, and
--- a public key is derivable — so each one is derived here from the private
--- key the export already has. That costs a round trip per wallet and is the
--- honest way to produce a field the store does not hold.
function Model:export_wallets()
  local home = self:home()
  if not home then return self:fail({ code = "io_error", message = "no wallet home" }) end
  if #self.wallets == 0 then
    return self:fail({ code = "usage", message = "no wallets to export" })
  end

  local rows = {}
  for _, account in ipairs(self.wallets) do
    local secret, err = self.wallet:export_account(account.address)
    if not secret then return self:fail(err) end

    local keys = self.wallet:derive({ private_key = secret.private_key }) or {}
    local address = tostring(secret.address or account.address)

    rows[#rows + 1] = {
      mnemonic = type(secret.mnemonic) == "string" and secret.mnemonic or "",
      index = secret.index or account.index or 0,
      address_checksummed = address,
      address = address:lower(),
      private_key = secret.private_key,
      public_key_compressed = keys.public_key_compressed,
      public_key = keys.public_key,
    }
  end

  local path, err = write_file(home .. "/" .. export.SECRET_FILE,
    export.secrets(rows), true)
  if not path then
    return self:fail({ code = "io_error", message = "cannot write: " .. tostring(err) })
  end

  self:say(("Exported %d keys to %s"):format(#rows, export.SECRET_FILE), "error")
  self:emit("exported")
  return path
end

--- Delete the store: every `.jsonl` file in the wallet's home.
---
--- This is the end of those wallets. The mnemonics and the private keys are in
--- those files and nowhere else, so anything not written down or exported
--- first is gone — not locked, gone. The caller is expected to have asked.
---
--- The names come from the wallet's own `info().files` rather than from a
--- glob, so this removes what the wallet says its store is and cannot wander
--- into a directory that happens to hold something else.
function Model:wipe_store()
  local files = self.info and self.info.files
  if type(files) ~= "table" then
    return self:fail({ code = "io_error", message = "the wallet did not say where its files are" })
  end

  local removed = 0
  for _, path in pairs(files) do
    if type(path) == "string" and path:match("%.jsonl$") then
      if os.remove(path) then removed = removed + 1 end
    end
  end

  -- Anything this window wrote beside them goes too. Leaving an export of the
  -- keys behind after deleting the store they came from would make the wipe a
  -- gesture rather than a fact.
  local home = self:home()
  if home then
    for _, name in ipairs(export.ADDRESS_FILES) do
      if os.remove(home .. "/" .. name) then removed = removed + 1 end
    end
    if os.remove(home .. "/" .. export.SECRET_FILE) then removed = removed + 1 end
  end

  return removed
end

-- ----------------------------------------------------------- local commands
--
-- These read the store on disk. No node is involved, so they run directly
-- rather than going to the worker.

function Model:refresh()
  local accounts, err = self.wallet:accounts()
  if not accounts then return self:fail(err) end
  self.all_wallets = accounts

  -- A session is one mnemonic, and it shows the wallets that mnemonic
  -- controls — not everything the store happens to hold.
  --
  -- The store is one home directory and may hold wallets from a dozen
  -- different phrases. Showing all of them behind any one of them made the
  -- login screen a doorway rather than a gate: unlocking with a brand new
  -- phrase produced a "new wallet" sitting in a list of somebody else's.
  --
  -- `session.addresses` is the set derived from the phrase at login. Nothing
  -- is hidden that the phrase can reach, and nothing is shown that it cannot.
  if self.session then
    local mine = {}
    for _, account in ipairs(accounts) do
      if self.session.addresses[tostring(account.address):lower()] then
        mine[#mine + 1] = account
      end
    end
    self.wallets = mine
  else
    self.wallets = accounts
  end

  local info = self.wallet:info()
  self.info = info
  local active = info and info.active_address
  -- `json.null` decodes to a table, so an absent active account is flattened
  -- here rather than compared against in three places.
  self.active = type(active) == "string" and active or nil

  if self.selected > #self.wallets then self.selected = math.max(1, #self.wallets) end

  -- Open on the wallet that is actually in use, once.
  --
  -- The selection used to start at row 1 regardless, so a store whose active
  -- account was any other row opened showing the wrong one — which was merely
  -- untidy when the panel was a list of fields and is misleading now that it
  -- is a card, because the first thing a person sees is a card that is not the
  -- one their money is in.
  --
  -- Once, and only once: doing it on every refresh would drag the selection
  -- back to the active account every time a balance arrived, and the arrow
  -- keys would fight it.
  if not self.aimed and self.active then
    for index, entry in ipairs(self.wallets) do
      if entry.address == self.active then
        self.selected = index
        break
      end
    end
  end
  if #self.wallets > 0 then self.aimed = true end

  return true
end

function Model:create(label)
  local account, err = self.wallet:new_account({
    label = label ~= "" and label or nil,
  })
  if not account then return self:fail(err) end

  -- `account new` continues the active account's mnemonic, so a wallet made
  -- inside a session belongs to that session's phrase and must stay visible.
  -- Recorded rather than assumed: the scan that built the set stops at a gap
  -- and may not have reached this index.
  if self.session then
    self.session.addresses[tostring(account.address):lower()] = true
  end

  self:refresh()
  -- Land on the wallet that was just made, without making it active.
  --
  -- Creating one does not switch the store's active account, and it should
  -- not: spending should never move because a wallet was added. But the *card*
  -- should — the new wallet is the one thing a person is looking for at that
  -- moment, and leaving the selection on the previous row means the card on
  -- screen is not the card that was just created. USE CARD is one press away
  -- if they want to spend from it.
  --
  -- The balance is deliberately left alone: the active wallet did not change,
  -- so the number that is on screen is still that wallet's and still true.
  for index, entry in ipairs(self.wallets) do
    if entry.address == account.address then self.selected = index end
  end
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
  -- The dialog owns the decision while it is up. Starting a second transfer
  -- from underneath an open confirmation is never what anyone meant, and it
  -- is how a confirmed send came back asking to be confirmed again — the
  -- click that approved one also reached the button that began another.
  --
  -- The view is supposed to prevent that and now does; this is the guard that
  -- does not depend on the view getting its layout right.
  if self.confirm then return false end

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
    -- Who is paying, captured with the plan rather than read again when the
    -- dialog draws. It is the wallet that was active when the wallet priced
    -- this, which is the one that will be debited, and it is the single most
    -- important thing on a confirmation to be sure of.
    self.confirm = {
      summary = Model.plan_summary(err.message),
      from = self.active,
      from_label = self:active_label(),
      to = to,
      amount = amount,
    }
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
