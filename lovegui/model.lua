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

--- What the per-row send button pays, in the chain's own token.
---
--- One number, written down once. A quick send exists to save typing an
--- address that is already on the screen, not to be a second, quieter way of
--- spending an arbitrary amount — so the amount is fixed, small, and the same
--- for every row.
Model.QUICK_AMOUNT = "0.01"

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
    form = { to = "", amount = "", search = "" },
    -- The asset the whole window is working in, or nil for the network's own
    -- token. Picking a token row on the NETWORK screen sets it; picking a
    -- network row clears it. See `Model:asset`.
    token = nil,
    confirm = nil,
    -- A faucet run, from the balance read that opens it to the arrival that
    -- ends it — or the link to a web faucet on the networks the wallet cannot
    -- ask itself. See the faucet section for the shape and why it has one.
    faucet = nil,
    -- A file the window is about to write, waiting to be approved. See
    -- `ask_save` below for why writing one is a question and not a button.
    write = nil,
    -- And where the last one actually went, so the view can say so somewhere
    -- roomier than the status line.
    written = nil,
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

--- Whether a modal is up. All three take the keyboard and the mouse.
---
--- The send confirmation must not have the form edited underneath it — that
--- would change what was approved — and the write dialog must not have the
--- list scrolled or a field typed into behind it either. The faucet panel is
--- here for a third reason: it is a running thing, and switching network or
--- wallet underneath it would leave it reporting a before and an after read on
--- two different chains.
function Model:asking()
  return self.confirm ~= nil or self.write ~= nil or self.faucet ~= nil
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

  -- Every chain at each index, not just the one in view. The session set is
  -- what the wallet list is filtered against for as long as the login lasts;
  -- a set derived on one chain made switching chains inside a session show a
  -- list the phrase supposedly did not own.
  local chains = {}
  for _, chain in ipairs(self:chains()) do
    chains[#chains + 1] = chain.chain
  end
  if #chains == 0 then chains = { "evm" } end

  local addresses, misses = {}, 0
  for index = 0, Model.SESSION_MAX - 1 do
    local hit = false
    for _, chain in ipairs(chains) do
      local derived = self.wallet:derive({ mnemonic = phrase, index = index, chain = chain })
      if derived then
        -- Every face of the address, not just the default network's. The
        -- wallet list renders each account for the network its chain is on,
        -- and on Cardano, Midnight and eCash that is a different string —
        -- a set holding only the default form made a logged-in wallet
        -- vanish the moment its chain moved to another network.
        for _, key in ipairs({ "address", "address_mainnet", "address_devnet" }) do
          local face = derived[key]
          if type(face) == "string" then
            addresses[face:lower()] = true
            if known[face:lower()] then hit = true end
          end
        end
      end
    end
    if hit then
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

  -- The listing renders each account's address for the network its chain is
  -- on, and the phrase's index-0 account may therefore wear any of its
  -- faces there — so the match accepts all of them, not just the default
  -- network's string `derive` leads with.
  local faces = {}
  for _, key in ipairs({ "address", "address_mainnet", "address_devnet" }) do
    if type(derived[key]) == "string" then faces[derived[key]:lower()] = true end
  end
  local account, welcome
  for _, entry in ipairs(stored) do
    if faces[tostring(entry.address):lower()] then account = entry end
  end

  if account then
    welcome = "Welcome back, " .. account.label
  else
    -- New to this store: import it, which is the one place a phrase is
    -- written — and import all of it. A wallet is one index across every
    -- chain, and a login that restored one chain's account left the other
    -- three to be discovered missing later.
    local imported, import_error = self.wallet:import_mnemonic(phrase, { every_chain = true })
    if not imported then return self:fail(Model.without_phrase(import_error, phrase)) end
    account = imported[1] or imported
    welcome = "Imported " .. account.label
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
  -- Selected by id where one is known: an id names the account on every
  -- network, where the address only names it on one.
  local ok, use_error = self.wallet:use_account(account.id or account.address)
  if not ok then return self:fail(use_error) end
  self.balance = nil
  self:refresh()

  self.scroll = 0
  for index, entry in ipairs(self.wallets) do
    if (account.id and entry.id == account.id) or entry.address == account.address then
      self.selected = index
    end
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
  -- A panel reporting one session's balances must not survive into the next.
  self.faucet = nil
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
---
--- `~/.causewaybaywallet` unless `CAUSEWAYBAY_HOME` or `--home` said otherwise.
--- The wallet is the one that resolved it and this asks rather than guessing,
--- so what the window shows is where the bytes really go — and there is only
--- ever this one directory. Nothing here writes anywhere else.
function Model:home()
  return self.info and self.info.home or nil
end

--- A path as a person says it: `~/.causewaybaywallet`, not `/Users/…`.
---
--- Only the leading home directory is folded, and only when it really is the
--- prefix. Everywhere else the path is left exactly as the wallet resolved it,
--- because a path nobody can paste is not an answer to "where is my file".
function Model.tilde(path)
  local home = os.getenv("HOME")
  if not home or home == "" or type(path) ~= "string" then return path end
  if path == home then return "~" end
  if path:sub(1, #home + 1) == home .. "/" then
    return "~" .. path:sub(#home + 1)
  end
  return path
end

-- ------------------------------------------------------- asking before writing
--
-- Both verbs below write into a directory nobody chose and most people have
-- never opened, and one of them writes every private key this wallet holds.
-- Somebody who does not know where the file went cannot delete it, cannot move
-- it onto the machine they meant to move it to, and cannot tell whether the
-- copy they found is the only one.
--
-- So neither writes on the click. The click describes the write — the full
-- directory, every file by name, and for the keys what the file is worth to
-- whoever reads it — and the write happens on the answer.

--- Describe saving the address list, and wait to be told to go ahead.
function Model:ask_save()
  local home = self:home()
  if not home then return self:fail({ code = "io_error", message = "no wallet home" }) end
  if #self.wallets == 0 then
    return self:fail({ code = "usage", message = "no wallets to save" })
  end

  local files = {}
  for _, name in ipairs(export.ADDRESS_FILES) do files[#files + 1] = name end
  table.sort(files)

  self.write = {
    kind = "addresses",
    title = "SAVE ADDRESSES",
    verb = "SAVE",
    dir = home,
    files = files,
    count = #self.wallets,
    note = "Addresses and labels only. Public information.",
    secret = false,
  }
  self.status = nil
  self:emit("ask")
  return true
end

--- Describe exporting the keys, and wait to be told to go ahead.
function Model:ask_export()
  local home = self:home()
  if not home then return self:fail({ code = "io_error", message = "no wallet home" }) end
  if #self.wallets == 0 then
    return self:fail({ code = "usage", message = "no wallets to export" })
  end

  self.write = {
    kind = "secrets",
    title = "EXPORT PRIVATE KEYS",
    verb = "WRITE KEYS",
    dir = home,
    files = { export.SECRET_FILE },
    count = #self.wallets,
    note = "Mnemonics and private keys. Anyone who reads this file owns the money.",
    secret = true,
  }
  self.status = nil
  self:emit("ask")
  return true
end

--- Do the write that was described. Returns whatever the writer returned.
function Model:confirm_write()
  local pending = self.write
  if not pending then return false end
  self.write = nil
  if pending.kind == "secrets" then return self:export_wallets() end
  return self:save_wallets()
end

--- Change your mind. Nothing is written and nothing is lost.
function Model:cancel_write()
  if not self.write then return false end
  local pending = self.write
  self.write = nil
  self:say(("Nothing written to %s"):format(Model.tilde(pending.dir)))
  self:emit("cancelled")
  return true
end

--- Make sure the directory a file is about to land in exists.
---
--- The wallet creates its home when it opens, so this is only ever a no-op —
--- unless somebody moved or deleted the directory while the window was up, in
--- which case a save would otherwise fail with a bare "No such file". There is
--- exactly one directory this window writes to, and it is the wallet's own.
local function ensure_dir(dir)
  os.execute(("mkdir -p %q 2>/dev/null"):format(dir))
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

  ensure_dir(home)
  local written = {}
  for name, contents in pairs(export.addresses(self.wallets)) do
    local path, err = write_file(home .. "/" .. name, contents, false)
    if not path then
      return self:fail({ code = "io_error", message = "cannot write " .. name .. ": " .. tostring(err) })
    end
    written[#written + 1] = name
  end
  table.sort(written)

  -- The directory, not just a count. It is the only place this window ever
  -- says where the files it just wrote can be found.
  self:say(("Saved %d wallets to %s"):format(#self.wallets, Model.tilde(home)))
  self.written = { dir = home, where = Model.tilde(home) }
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
    -- By id for the same reason `select` is: the listed address is the
    -- network's rendering, not necessarily the stored selector.
    local secret, err = self.wallet:export_account(account.id or account.address)
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

  ensure_dir(home)
  local path, err = write_file(home .. "/" .. export.SECRET_FILE,
    export.secrets(rows), true)
  if not path then
    return self:fail({ code = "io_error", message = "cannot write: " .. tostring(err) })
  end

  -- The whole path. A bare file name is not an answer to "where are my keys
  -- now?" — it is the half of the answer that a person already had.
  self:say(("Exported %d keys to %s"):format(#rows, Model.tilde(path)), "error")
  self.written = { dir = home, where = Model.tilde(path) }
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

  -- One row per wallet, not per account — the same model the Rust TUI shows.
  --
  -- A wallet is one mnemonic and one index; each chain derives its own account
  -- at that index. Listing all of them flat made two wallets read as eight, so
  -- the list holds the chain in view and switching chain moves what every row
  -- points at rather than making the list four times longer. The other chains'
  -- accounts are a chain switch away, not a scroll away.
  --
  -- The chain in view decides the list outright, even when it empties it.
  -- This used to keep the previous chain's accounts whenever the new one had
  -- none, which is how the network screen came to lie: pick Cardano, and the
  -- header said Cardano over a column of `0x…` EVM addresses. An address shown
  -- under the wrong chain's name is not a cosmetic fault — it is an address
  -- offered for a deposit that cannot arrive there.
  --
  -- Almost nothing reaches the empty case any more: moving to a chain derives
  -- each wallet's account on it from the same phrase and index. What is left
  -- is the wallet that genuinely has no face there — one imported from a bare
  -- private key, or from a phrase used with a BIP-39 passphrase the store does
  -- not keep — and for those, empty is the honest answer.
  local chain = info and info.chain
  if chain then
    local here = {}
    for _, account in ipairs(self.wallets) do
      if account.chain == chain then here[#here + 1] = account end
    end
    self.wallets = here
  end
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
  -- Every chain at once: a wallet is one mnemonic and one index, and each
  -- chain derives its own account there. Creating only the chain in view made
  -- "+ NEW" produce one chain's worth of a wallet — the other chains showed
  -- nothing until the CLI filled them in. One press, one whole wallet, and the
  -- chain in view just decides which of its faces the list shows.
  local created, err = self.wallet:new_account({
    label = label ~= "" and label or nil,
    every_chain = true,
  })
  if not created then return self:fail(err) end
  -- One account still comes back as a single object; every chain, as a list.
  local accounts = created[1] and created or { created }

  -- `account new` continues the active account's mnemonic, so a wallet made
  -- inside a session belongs to that session's phrase and must stay visible.
  -- Recorded rather than assumed: the scan that built the set stops at a gap
  -- and may not have reached this index.
  if self.session then
    for _, account in ipairs(accounts) do
      -- Every face the creation reports, as at login: the listing renders
      -- per network, and a set holding one face hides the wallet on the
      -- others.
      self.session.addresses[tostring(account.address):lower()] = true
      if type(account.extra) == "table" then
        for _, key in ipairs({ "address_mainnet", "address_devnet" }) do
          if type(account.extra[key]) == "string" then
            self.session.addresses[account.extra[key]:lower()] = true
          end
        end
      end
    end
  end
  -- The face to land on and name: the chain in view's, since that is the one
  -- the filtered list will actually show.
  local account = accounts[1]
  for _, made in ipairs(accounts) do
    if made.chain == self:chain() then account = made end
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
    if (account.id and entry.id == account.id) or entry.address == account.address then
      self.selected = index
    end
  end
  self:say(#accounts > 1 and ("Created " .. account.label .. " — one wallet, " .. #accounts .. " chains")
    or ("Created " .. account.label))
  self:emit("created")
  return account
end

function Model:select(index)
  local account = self.wallets[index]
  if not account then return false end
  -- By id, not address: the listed address is the network's rendering, and
  -- on a non-default network it is not the string the store holds.
  local ok, err = self.wallet:use_account(account.id or account.address)
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
  -- A token belongs to one network. Carrying it across would leave the window
  -- claiming to hold Cronos USDC on Solana — so moving network means the
  -- network's own coin until something says otherwise. `pick_row` sets the
  -- token again, after.
  self.token = nil
  -- The rows carry a `current` mark, so they are stale the moment this lands.
  self._rows = nil
  self:refresh()
  self:say("Now on " .. key)
  self:emit("network")
  return true
end

function Model:networks()
  return self.wallet:networks() or {}
end

--- What the search box holds, as a string, whatever the form is up to.
function Model:search()
  return self.form.search or ""
end

--- Type into the search box and drop the cached rows.
---
--- Every screen but this one owns the keyboard through `form`/`focus`; the
--- network screen borrows the same machinery so that typing, backspacing and
--- pasting behave identically there without a second implementation.
function Model:search_for(text)
  self.form.search = text or ""
  self._rows = nil
  self._rows_for = nil
  return true
end

--- The rows the NETWORK screen draws: every network, then every token, both
--- narrowed by the search box.
---
--- Networks first because a network is a place you go and a token is a thing
--- you read, and the screen should not offer the two as the same kind of move.
--- Both are in one list because the user has one question — "where is my
--- USDC?" — and answering it should not require knowing whether the answer is
--- a network or a token.
---
--- The filtering is the library's, not this file's. `network list <filter>`
--- and `token list <filter>` apply the one matching rule the CLI and the
--- terminal UI use, so a tag that finds a row in one finds it in all three.
--- The result is cached against the query that produced it, because this is
--- called once per frame and the answer only changes when someone types.
function Model:rows()
  local query = self:search()
  local network = self.info and self.info.network or ""
  local key = query .. "\0" .. network
  if self._rows and self._rows_for == key then return self._rows end

  local rows = {}
  for _, entry in ipairs(self.wallet:networks(query) or {}) do
    entry.kind = "network"
    rows[#rows + 1] = entry
  end
  for _, entry in ipairs(self.wallet:tokens(query) or {}) do
    entry.kind = "token"
    rows[#rows + 1] = entry
  end
  self._rows = rows
  self._rows_for = key
  return rows
end

--- How many rows there are in total, so the screen can say "7 of 24".
---
--- Counted unfiltered, and cached separately: a count that moved with the
--- filter would say "7 of 7" and tell nobody anything.
function Model:row_total()
  if not self._row_total then
    self._row_total = #(self.wallet:networks() or {}) + #(self.wallet:tokens() or {})
  end
  return self._row_total
end

--- Act on a row of the NETWORK screen: this is where the window is aimed.
---
--- A row is a *destination*, not a preview. Picking `cronos-mainnet` puts the
--- window on Cronos mainnet in CRO; picking the USDC row on it puts the window
--- on Cronos mainnet in USDC — and from then on the balance shown is the
--- ERC-20 balance, the send screen sends USDC, and the header says so. One
--- click settles both halves, which is the whole reason the token table is
--- flat: the row already names the chain, the network and the contract.
---
--- A network row clears the token, because the network's own coin is what
--- "cronos-mainnet" means with nothing further said.
function Model:pick_row(row)
  if not row then return false end
  if row.kind == "network" then
    if row.current and not self.token then return false end
    -- `switch_network` clears the token itself; when the row is the network
    -- already in view there is nothing to switch, only the token to drop.
    if row.current then
      self:select_token(nil)
      return true
    end
    return self:switch_network(row.key)
  end

  -- A token row moves the window to that token's network first, so the two
  -- can never disagree: an ERC-20 balance read against the wrong chain is not
  -- a smaller mistake than a transfer to it.
  if self.info and self.info.network ~= row.network then
    if not self:switch_network(row.network) then return false end
  end
  self:select_token(row)
  return true
end

--- Aim the window at one token, or at the network's own coin with nil.
function Model:select_token(row)
  self.token = row
  self.balance = nil -- it is a different number now
  self._rows = nil   -- and a different row is marked
  self:emit("network")
  if row then
    -- And why SEND is greyed out, where it is. A disabled button with no
    -- reason beside it is a dead end: the user is left to guess whether the
    -- wallet is busy, the form is wrong, or the asset simply cannot be moved.
    if row.transferable == false then
      self:say(("%s on %s — read-only here, this wallet reads it but does not move it")
        :format(row.symbol or row.key, row.network))
    else
      self:say(("Now in %s on %s"):format(row.symbol or row.key, row.network))
    end
  end
  self:fetch_balance()
  return true
end

--- What the window is working in: a registry token, or the network's own coin.
---
--- One place answers it, because four screens ask — the header, the wallet
--- list's balance, the send form's unit, and the confirmation. A screen that
--- worked it out for itself is a screen that can disagree with the one beside
--- it about what is being spent.
function Model:asset()
  local token = self.token
  if token then
    return {
      key = token.key,
      symbol = token.symbol,
      name = token.name,
      network = token.network,
      is_token = true,
      -- Cardano native assets are read but not moved; the send screen has to
      -- know before it offers a button.
      transferable = token.transferable ~= false,
    }
  end
  local info = self.info or {}
  return {
    key = nil,
    symbol = info.symbol,
    name = info.network,
    network = info.network,
    is_token = false,
    transferable = true,
  }
end

--- The unit an amount on this window is counted in.
function Model:symbol()
  return self:asset().symbol or ""
end

--- The chains this build supports, each with its own networks.
---
--- Read from the library rather than written down here, so a chain added in
--- Rust appears in the GUI without anyone editing this file.
function Model:chains()
  return self.wallet:chains() or {}
end

--- The key of a network, however the wallet described it.
---
--- The two places that list networks describe them differently: `chains` names
--- them by key, and the library's handshake gives whole records. A switcher
--- reading one and handed the other would pass a table where a key belongs,
--- which is not a mistake worth making twice.
function Model.network_key(entry)
  if type(entry) == "table" then return entry.key end
  return entry
end

--- Move to a chain, on whichever of its networks the wallet last used.
---
--- The two axes of the wallet are the chain and the network within it. A GUI
--- that only switched networks made "go to Solana" a matter of knowing which
--- network keys begin with `solana-`; this asks the wallet instead.
function Model:switch_chain(chain)
  local where = self.info and self.info.chains
  if where then
    for _, held in ipairs(where) do
      if held.chain == chain and held.network then
        return self:switch_network(held.network)
      end
    end
  end
  -- Nothing held on that chain yet: its first network is its default.
  for _, known in ipairs(self:chains()) do
    if known.chain == chain and known.networks and known.networks[1] then
      return self:switch_network(Model.network_key(known.networks[1]))
    end
  end
  return self:fail({ code = "unknown_chain", message = chain .. " is not a chain this build has" })
end

--- The chain in view, as the wallet reports it.
function Model:chain()
  return self.info and self.info.chain or "evm"
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
  -- Whichever asset the window is aimed at. `token balance` answers in the
  -- same shape as `balance` — a `balance` and a `symbol` — so nothing
  -- downstream has to know which of the two it is reading.
  local asset = self:asset()
  local argv = asset.is_token and { "token", "balance", asset.key } or { "balance" }
  self:say(("Asking the node for %s…"):format(asset.symbol or "the balance"), "busy")
  self:submit({ argv = argv }, function(envelope)
    local data, err = unwrap(envelope)
    if not data then return self:fail(err) end
    self.balance = data
    self:say((data.balance or "?") .. " " .. (data.symbol or ""))
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

--- The command that moves this window's asset, ready for `submit`.
---
--- Built once and used by both halves of a send — the pricing round trip and
--- the broadcast that follows it — because the two must not be able to differ.
--- A confirmation priced in USDC and broadcast in CRO would be a dialog that
--- described a transfer nobody made.
function Model:send_argv(to, amount)
  local asset = self:asset()
  if asset.is_token then
    return { "token", "send", asset.key, "--to", to, "--amount", amount }
  end
  return { "send", "--to", to, "--amount", amount }
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
  -- Paying the wallet you are paying *from* moves nothing and still costs the
  -- gas. The wallet refuses it too, and that refusal is the one that counts —
  -- but it costs a round trip through the worker to hear, and the answer is
  -- already on this side of it. Compared case-insensitively, because EIP-55 is
  -- a property of the text: the same account pasted in lower case is the same
  -- account.
  if self.active and to:lower() == tostring(self.active):lower() then
    return self:fail({
      code = "usage",
      message = "sending to yourself would only pay the gas",
    })
  end
  -- An asset this wallet reads but cannot move is refused here, with the
  -- reason, rather than after a round trip that was never going to succeed.
  local asset = self:asset()
  if not asset.transferable then
    return self:fail({
      code = "usage",
      message = ("%s is read-only here — this wallet reads it but does not move it")
        :format(asset.symbol or asset.key),
    })
  end
  self:say("Pricing the transaction…", "busy")
  self:submit({
    argv = self:send_argv(to, amount),
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
      -- Captured with the plan rather than read again when the dialog draws,
      -- for the same reason `from` is: it is what this transfer was priced in,
      -- and the window must not be able to move underneath an open question.
      symbol = asset.symbol,
      argv = self:send_argv(to, amount),
    }
    self.status = nil
    self:emit("confirm")
  end)
  return true
end

--- Pay one wallet in the list `QUICK_AMOUNT`, from whichever wallet is active.
---
--- The row is the *recipient*, and pressing its button must not quietly make
--- it the sender as well. Paying a wallet and switching to it are two different
--- intentions, and a button that did both would move the money out of an
--- account nobody had selected — so this deliberately does not go anywhere near
--- `select`, and the sender stays whatever `active` already was.
---
--- Goes through `begin_send` like every other transfer: the wallet prices it,
--- checks the balance covers it, and the same dialog asks before anything is
--- signed. A quick send is quicker to *start*, not quicker to approve.
function Model:quick_send(index)
  local account = self.wallets[index]
  if not account then
    return self:fail({ code = "usage", message = "no such wallet" })
  end
  return self:begin_send(account.address, Model.QUICK_AMOUNT)
end

function Model:confirm_send()
  local plan = self.confirm
  if not plan then return false end
  self.confirm = nil
  self:say("Signing and broadcasting…", "busy")
  self:submit({
    -- The very command that was priced and agreed to, not one rebuilt from a
    -- window that may have moved since.
    argv = plan.argv or self:send_argv(plan.to, plan.amount),
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

-- ------------------------------------------------------------------- faucet
--
-- Being given money, on a network where somebody gives it away.
--
-- ## Why this is a small state machine and not one call
--
-- What makes a faucet worth watching is the *difference* it made, and a
-- difference needs two readings. So a run is three round trips rather than
-- one: read the balance, ask the faucet, read the balance again. The first
-- reading has to happen before the request or there is nothing to compare
-- against, and the last has to happen after the money has had time to land.
--
-- That last part is the awkward one. A faucet answers the moment it has
-- accepted the request, not when the funds are spendable — Solana hands back a
-- signature and confirms it a second or so later — so a balance read fired the
-- instant the airdrop returns shows the number that was already there, and the
-- animation counts from a value to itself. Hence `FAUCET_SETTLE` and the
-- retries: the wallet waits, asks, and asks again a few times before accepting
-- that what is there is what arrived.
--
-- ## Why some networks have a button that cannot be pressed
--
-- Only Solana's clusters answer a faucet request over the endpoint the balance
-- came from. Every other faucet in the table is a web page with a captcha,
-- built precisely so that a program cannot drain it, and pretending otherwise
-- would be a button that fails every single time. Those networks get the
-- address of the page instead — which is the answer, just one a person has to
-- carry to a browser. `Model:faucet_is_automatic` is what the view branches on.

--- How much to ask for, in whole tokens.
---
--- One, not the two Solana's RPC will cap a single request at: a devnet faucet
--- is a shared thing and this button is pressable as often as somebody likes.
Model.FAUCET_AMOUNT = "1"

--- How long to let the money land before asking what arrived.
---
--- A faucet answers when it has accepted the request, not when the funds are
--- spendable. Reading the balance straight back gives the number that was
--- already there, and an animation that counts from a value to itself is a
--- worse outcome than a spinner: it says, convincingly, that nothing happened.
Model.FAUCET_SETTLE = 1.5

--- How many times to ask before taking what is there as the answer.
Model.FAUCET_TRIES = 4

--- And how long to wait between those asks.
Model.FAUCET_GAP = 2.0

--- Where this network's faucet is, or nil where there is none.
function Model:faucet_url()
  local url = self.info and self.info.faucet
  if type(url) ~= "string" or url == "" then return nil end
  return url
end

--- Whether the wallet can ask that faucet itself, rather than sending a person
--- to a web form. The library answers this; see `Network::faucet_is_callable`.
function Model:faucet_is_automatic()
  return self.info ~= nil and self.info.faucet_automatic == true
end

--- Where a block explorer shows one wallet.
---
--- The link is the library's, not this file's: Solana's explorer carries its
--- cluster in a query string, so a link built by appending `/address/…` to the
--- base URL points at mainnet whatever address it is given — which loads, shows
--- nothing, and reads as an empty wallet. `account list` hands each row the
--- link for the network its own chain is on.
---
--- `index` is a row of the wallet list; nil means whichever row is selected.
function Model:explorer_link(index)
  local account = self.wallets[index or self.selected]
  if not account then return nil end
  local link = account.explorer
  if type(link) ~= "string" or link == "" then return nil end
  return link, account
end

--- Whether a faucet panel is on screen. It is modal like the other two.
function Model:faucet_showing()
  return self.faucet ~= nil
end

--- Close it, whatever it was showing.
function Model:dismiss_faucet()
  if not self.faucet then return false end
  self.faucet = nil
  self:emit("faucet_closed")
  return true
end

--- The state a run starts in, so the three ways of starting one agree.
function Model:_faucet_panel(phase, extra)
  local asset = self:asset()
  local account = self.wallets[self.selected]
  local panel = {
    phase = phase,
    network = self.info and self.info.network or "?",
    symbol = self.info and self.info.symbol or asset.symbol or "",
    address = self.active,
    label = self:active_label(),
    url = self:faucet_url(),
    automatic = self:faucet_is_automatic(),
    amount = Model.FAUCET_AMOUNT,
    before = nil,
    after = nil,
    tries = Model.FAUCET_TRIES,
    wait = 0,
  }
  -- The card on screen when there is no active account at all, so the panel
  -- still names an address rather than a blank.
  if not panel.address and account then
    panel.address, panel.label = account.address, account.label
  end
  for key, value in pairs(extra or {}) do panel[key] = value end
  self.faucet = panel
  return panel
end

--- Ask this network for money, into the wallet being spent from.
---
--- Deliberately the *active* wallet and not the selected one. A faucet pays
--- whoever it is told to, and the wallet the window is aimed at is the one
--- whose balance every other screen is showing — funding a card somebody had
--- merely scrolled past would leave the balance on screen unchanged and the
--- money somewhere they were not looking.
function Model:request_faucet()
  if self.faucet then return false end
  if self:busy() then
    return self:fail({ code = "usage", message = "the node is already being asked something" })
  end
  if #self.wallets == 0 then
    return self:fail({ code = "no_active_account", message = "create a wallet first" })
  end

  local url = self:faucet_url()
  if not url then
    -- A mainnet. Nobody gives its coin away, and saying so is the whole
    -- answer — there is no page to send anyone to.
    return self:fail({
      code = "usage",
      message = ("%s is a mainnet — nobody gives its coin away")
        :format(self.info and self.info.network or "this network"),
    })
  end

  if not self:faucet_is_automatic() then
    -- A web form with a captcha on it. The wallet cannot answer a captcha, and
    -- a button that pretends to try is worse than one that hands over the
    -- address — so the panel opens on the link, and the view copies it.
    self:_faucet_panel("manual")
    self:say(("%s hands out %s from a web page"):format(
      self.faucet.network, self.faucet.symbol))
    self:emit("faucet_manual")
    return true
  end

  local panel = self:_faucet_panel("reading")
  self:say("Reading the balance before asking…", "busy")
  self:submit({ argv = { "balance" } }, function(envelope)
    -- Gone, because the panel was closed while the read was in flight.
    if self.faucet ~= panel then return end
    local data = unwrap(envelope)
    -- A balance that cannot be read is not a reason to refuse the money. The
    -- panel simply has no "before" to count from, and says so.
    panel.before = data and tonumber(data.balance) or nil
    self:_ask_the_faucet(panel)
  end)
  return true
end

--- The middle of a run: the request itself.
function Model:_ask_the_faucet(panel)
  panel.phase = "asking"
  self:say(("Asking %s for %s %s…"):format(panel.network, panel.amount, panel.symbol), "busy")
  self:submit({
    argv = { "airdrop", "--amount", panel.amount },
  }, function(envelope)
    if self.faucet ~= panel then return end
    local data, err = unwrap(envelope)
    if not data then
      panel.phase = "failed"
      panel.error = err
      -- The status line carries it too, because the panel can be dismissed
      -- and the reason should outlive it.
      self:say(err.message or "the faucet said no", "error")
      self:emit("faucet_failed")
      return
    end
    panel.id = data.id
    panel.phase = "waiting"
    panel.wait = Model.FAUCET_SETTLE
    self:say("The faucet said yes — waiting for it to land…", "busy")
    self:emit("faucet_asked")
  end)
  return true
end

--- Move a waiting run along. Called once a frame with the frame's own dt.
---
--- A clock, in the file that holds the decisions rather than in the one that
--- draws them: *when to ask again* is a fact about the run, and the tests drive
--- it here with a dt they choose.
function Model:tick(dt)
  local panel = self.faucet
  if not panel or panel.phase ~= "waiting" then return false end
  if self:busy() then return false end

  panel.wait = panel.wait - (dt or 0)
  if panel.wait > 0 then return false end
  panel.wait = Model.FAUCET_GAP
  panel.tries = panel.tries - 1

  local last = panel.tries <= 0
  self:submit({ argv = { "balance" } }, function(envelope)
    if self.faucet ~= panel then return end
    local data = unwrap(envelope)
    local after = data and tonumber(data.balance) or nil
    if data then
      -- The window's own balance moves with it, so the card behind the panel
      -- is not still showing the old number when the panel closes.
      self.balance = data
    end

    -- More than there was, or out of patience. Either way this is the number,
    -- and `landed` is what the view celebrates.
    if after and panel.before and after > panel.before then
      panel.after = after
      panel.phase = "landed"
      self:say(("%s %s arrived"):format(
        Model.format_amount(after - panel.before), panel.symbol))
      self:emit("faucet_landed")
    elseif last then
      panel.after = after
      -- Accepted and yet nothing more is there. Honest about which of the two
      -- it is: the faucet did say yes, and a devnet can be slow enough that
      -- the money turns up after this panel has been closed.
      panel.phase = "slow"
      self:say(panel.before
        and "The faucet said yes; nothing has arrived yet"
        or "The faucet said yes; the balance would not read", "error")
      self:emit("faucet_slow")
    end
  end)
  return true
end

--- Play the arrival, with nothing moved and nothing claimed.
---
--- The reason this exists: on ten of the twelve networks the wallet cannot ask
--- the faucet at all, so the one moment in this program worth watching was
--- reachable only by being on Solana and being lucky. That is a poor reason to
--- hide it.
---
--- Every number in it is made up, and the panel says so in as many words —
--- a celebration that could be mistaken for money arriving would be the one
--- animation in this wallet that lies.
function Model:demo_faucet()
  local panel = self:_faucet_panel("demo", {
    before = 12.5,
    after = 13.5,
    amount = "1",
    demo = true,
  })
  panel.symbol = panel.symbol ~= "" and panel.symbol or "COIN"
  self:say("Showing what an arrival looks like — nothing moved")
  self:emit("faucet_landed")
  return true
end

--- Four places, with the trailing zeros taken off. The card draws its balance
--- this way and the panel has to agree with it, so the rule is written once.
function Model.format_amount(value)
  local text = ("%.4f"):format(tonumber(value) or 0)
  text = text:gsub("0+$", ""):gsub("%.$", "")
  return text
end

-- --------------------------------------------------------------- text entry

function Model:type_into(text)
  if self:asking() then return end -- the dialog owns the keyboard
  self.form[self.focus] = (self.form[self.focus] or "") .. text
end

function Model:backspace()
  if self:asking() then return end
  local field = self.form[self.focus] or ""
  self.form[self.focus] = field:sub(1, -2)
end

--- Replace a field outright, as a paste does.
---
--- Trimmed, because a clipboard address copied out of a block explorer or a
--- chat message arrives wrapped in whitespace more often than not, and the
--- wallet would reject it for a reason nobody could see.
function Model:set_field(field, text)
  if self:asking() then return false end
  if type(text) ~= "string" then return false end
  self.form[field] = (text:gsub("^%s+", ""):gsub("%s+$", ""))
  self.focus = field
  return true
end

function Model:clear_field(field)
  if self:asking() then return false end
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
