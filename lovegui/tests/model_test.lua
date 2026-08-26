--- Tests for the model — the GUI with the drawing taken away.
---
--- Driven headlessly against a real wallet in a temp home, the same way
--- `luacli`'s interactive loop is. Passing `nil` for `jobs` makes every call
--- synchronous, so a test asserts on the result immediately instead of pumping
--- a thread that does not exist outside LÖVE.

local t = require("tests.runner")
local support = require("tests.support")
local Model = require("model")

--- A model over a fresh wallet, synchronous.
local function model_over(options)
  local wallet = support.wallet(options)
  return Model.new(wallet, nil), wallet
end

t.suite("model / opening", function()
  t.case("starts on the wallets screen with nothing in it", function()
    local model = model_over()
    t.equal(model.screen, "wallets")
    t.equal(#model.wallets, 0)
    t.equal(model.active, nil)
    t.equal(model.balance, nil)
  end)

  t.case("reads wallets that already exist", function()
    local wallet = support.wallet()
    wallet:new_account({ label = "alpha" })
    wallet:new_account({ label = "beta" })
    local model = Model.new(wallet, nil)
    t.equal(#model.wallets, 2)
    t.ok(model.active, "one of them should be active")
  end)

  t.case("an absent active account is nil, not json.null", function()
    -- `info` reports null when there is none, and the decoder turns that into
    -- a table. Left alone it would compare unequal to every address and read
    -- as truthy — the sort of bug that shows as "no wallet is ever selected".
    local model = model_over()
    t.equal(type(model.active), "nil")
  end)
end)

t.suite("model / wallets", function()
  t.case("creates one and selects it", function()
    local model = model_over()
    local account = model:create("treasury")
    t.ok(account, "should have returned the account")
    t.equal(#model.wallets, 1)
    t.equal(model.active, account.address)
    t.contains(model.status.text, "treasury")
  end)

  t.case("an empty label is allowed and the wallet names it", function()
    local model = model_over()
    local account = model:create("")
    t.ok(account and #account.label > 0)
  end)

  t.case("a duplicate label fails without throwing", function()
    local model = model_over()
    model:create("same")
    local again = model:create("same")
    t.equal(again, false)
    t.equal(model.status.kind, "error")
    t.equal(model.status.code, "duplicate_label")
    t.equal(#model.wallets, 1, "the failed one must not have been added")
  end)

  t.case("selecting switches the active wallet and drops the balance", function()
    local model = model_over()
    model:create("one")
    model:create("two")
    model.balance = { balance = "5", symbol = "TCRO" }

    t.ok(model:select(1))
    t.equal(model.active, model.wallets[1].address)
    -- A different wallet has a different balance; keeping the old number
    -- beside the new name is the worst of both.
    t.equal(model.balance, nil)
  end)

  t.case("creating one selects it without making it active", function()
    -- Two separate ideas that used to be one. The card should follow what was
    -- just made; what the wallet *spends* from should not move because a
    -- wallet was added.
    local model = model_over()
    model:create("one")
    local spending = model.active
    model.balance = { balance = "5", symbol = "TCRO" }

    model:create("two")
    model:create("three")
    t.equal(model.selected, 3, "the card should show the one just created")
    t.equal(model.active, spending, "but spending should not have moved")
    t.ok(model.balance ~= nil,
      "and the active wallet's balance is still that wallet's, so it stands")
  end)

  t.case("it opens on the wallet that is actually in use", function()
    -- The card shows whatever is selected. Starting on row 1 when the wallet
    -- is spending from row 2 means the first thing a person sees at every
    -- launch is a card that is not the one their money is in.
    local model = model_over()
    model:create("one")
    model:create("two")
    model:create("three")
    model:select(2)

    local fresh = Model.new(model.wallet, nil)
    fresh:refresh()
    t.equal(fresh.active, model.wallets[2].address, "two should be active")
    t.equal(fresh.selected, 2, "and the selection should have found it")
  end)

  t.case("aiming happens once and does not fight the arrow keys", function()
    local model = model_over()
    model:create("one")
    model:create("two")
    model:select(2)
    -- What pressing up does: the view moves `selected` directly.
    model.selected = 1
    model:refresh()
    t.equal(model.selected, 1, "a refresh must not drag it back to the active row")
  end)

  t.case("selecting nothing is refused rather than crashing", function()
    local model = model_over()
    t.equal(model:select(1), false)
    t.equal(model:select(99), false)
  end)
end)

t.suite("model / the session", function()
  t.case("a phrase the store does not know is imported once", function()
    local model = model_over()
    local account = model:login(support.MNEMONIC)
    t.ok(account, "should have got in")
    t.equal(account.address, support.ADDRESS_0)
    t.equal(#model.wallets, 1, "and the phrase should now be a wallet")
    t.ok(model:logged_in())

    -- Again with the same phrase: recognised, not imported a second time.
    local again = model:login(support.MNEMONIC)
    t.ok(again, "should have got in again")
    t.equal(#model.wallets, 1, "without adding a duplicate")
    t.contains(model.status.text, "Welcome back")
  end)

  t.case("a new phrase becomes the wallet in use, not just the one on screen", function()
    -- NEW MNEMONIC, COPY, UNLOCK should start a new wallet — all the way,
    -- including spending from it. Logging in *as* a wallet and then spending
    -- from a different one is exactly the mismatch this screen exists to
    -- prevent, and the known-phrase branch below already activates, so a new
    -- phrase behaving differently was simply a gap.
    local model = model_over()
    model:create("first")
    local first = model.active

    local phrase = model:offer_mnemonic(12)
    local account = model:login(phrase)

    t.ok(account, "should have got in")
    t.not_equal(model.active, first, "spending must have moved to the new wallet")
    t.equal(model.active, account.address, "and it should be the one unlocked")
    t.equal(model.wallets[model.selected].address, account.address,
      "with the card showing it too")
    t.equal(model.session.address, account.address)
  end)

  t.case("logging in with a known phrase does the same", function()
    -- The two branches must not disagree about what logging in means.
    local model = model_over()
    model:login(support.MNEMONIC)
    model:create("other")
    model:select(2)
    t.not_equal(model.active, support.ADDRESS_0, "spending is elsewhere now")

    model:login(support.MNEMONIC)
    t.equal(model.active, support.ADDRESS_0, "logging back in should return to it")
  end)

  t.case("logging in points the card at the wallet that was unlocked", function()
    local model = model_over()
    model:create("one")
    model:create("two")
    model:login(support.MNEMONIC)
    t.equal(model.wallets[model.selected].address, support.ADDRESS_0,
      "the selection should be on the wallet just unlocked")
  end)

  t.case("an empty phrase is refused before the wallet is asked", function()
    local model = model_over()
    t.equal(model:login(""), false)
    t.equal(model:login("   "), false)
    t.equal(model:login(nil), false)
    t.equal(model.status.kind, "error")
    t.equal(#model.wallets, 0, "and nothing should have been written")
  end)

  t.case("the guard replaces a parser error outright", function()
    -- Tested directly, because the end-to-end path cannot reach it: `login`
    -- validates first, and a malformed phrase is refused there without ever
    -- being quoted. An end-to-end test would pass whether or not this
    -- function did anything, which is the least useful kind of test.
    local leaky = {
      code = "usage",
      message = "error: unexpected argument '- " .. support.MNEMONIC .. "' found",
    }
    local safe = Model.without_phrase(leaky, "- " .. support.MNEMONIC)
    t.ok(not safe.message:find("abandon", 1, true),
      "the phrase must be gone: " .. safe.message)
    t.equal(safe.code, "invalid_mnemonic", "and it is a mnemonic problem, not usage")
  end)

  t.case("the guard replaces any message holding the phrase", function()
    local leaky = { code = "internal", message = "failed on " .. support.MNEMONIC }
    local safe = Model.without_phrase(leaky, support.MNEMONIC)
    t.ok(not safe.message:find("abandon", 1, true), "gone: " .. safe.message)
  end)

  t.case("three words in a row is enough to count as the phrase", function()
    local leaky = { code = "internal", message = "near 'abandon abandon abandon' here" }
    local safe = Model.without_phrase(leaky, support.MNEMONIC)
    t.ok(not safe.message:find("abandon", 1, true), "gone: " .. safe.message)
  end)

  t.case("the guard leaves an unrelated failure alone", function()
    -- It must not swallow everything. A missing library or an unwritable
    -- store still has to say so.
    local real = { code = "io_error", message = "libcausewaybay_ffi.dylib not found" }
    local kept = Model.without_phrase(real, support.MNEMONIC)
    t.equal(kept.message, real.message, "a real reason survives")
    t.equal(kept.code, "io_error")
  end)

  t.case("one shared word is not the phrase", function()
    -- "about" and "abandon" are ordinary words. A message containing one of
    -- them is not a leak, and blanking it would hide real failures.
    local real = { code = "io_error", message = "cannot write about the store" }
    t.equal(Model.without_phrase(real, support.MNEMONIC).message, real.message)
  end)

  t.case("the guard copes with nothing to guard", function()
    t.equal(Model.without_phrase(nil, support.MNEMONIC), nil)
    local err = { code = "io_error", message = "gone" }
    t.equal(Model.without_phrase(err, "").message, "gone")
    t.equal(Model.without_phrase(err, nil).message, "gone")
  end)

  t.case("a rejected phrase is never quoted back in the status", function()
    -- The leak this closes. A mnemonic copied out of a bulleted list arrives
    -- with the bullet still on it; the argument parser refuses it as an
    -- unexpected argument and quotes the whole thing back, and that message
    -- becomes `status.text`, which is drawn.
    local model = model_over()
    local phrase = "- " .. support.MNEMONIC

    t.equal(model:login(phrase), false, "it should still be refused")
    local text = model.status.text
    t.ok(not text:find("abandon", 1, true),
      "the phrase must not be in the status: " .. text)
    t.ok(not text:find("about", 1, true), "nor any part of it: " .. text)
    t.ok(#text > 0, "but there should still be a message")
  end)

  t.case("nothing typed into the field is ever quoted back", function()
    -- Not only mnemonic-shaped input: whatever is in that field is treated as
    -- a secret, because the person typing believed it was one.
    local model = model_over()
    for _, phrase in ipairs({
      "--home /tmp/somewhere",
      "-abandon abandon abandon",
      "--mnemonic hunter2 hunter2 hunter2",
      "correct horse battery staple correct horse battery staple",
    }) do
      model:login(phrase)
      local text = model.status.text
      for word in phrase:gmatch("[%w/%-%.]+") do
        if #word > 4 then
          t.ok(not text:find(word, 1, true),
            ("%q leaked into the status: %s"):format(word, text))
        end
      end
    end
  end)

  t.case("a real failure still says what went wrong", function()
    -- The scrub must not swallow everything. A wrong-checksum phrase has a
    -- message worth reading, and it does not contain the phrase.
    local model = model_over()
    model:login("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon")
    t.contains(model.status.text:lower(), "checksum",
      "the reason should survive, since it quotes nothing")
  end)

  t.case("a phrase that is not a phrase is refused", function()
    local model = model_over()
    t.equal(model:login("abandon abandon banana"), false)
    t.equal(model.status.kind, "error")
    t.equal(#model.wallets, 0, "a bad phrase must never reach the store")
    t.ok(not model:logged_in())
  end)

  t.case("a new phrase shows its own wallet and nobody else's", function()
    -- The complaint this exists for: logout, NEW MNEMONIC, COPY, UNLOCK, and
    -- the old wallets were all still listed. A store is one home directory
    -- and may hold wallets from a dozen phrases; showing all of them behind
    -- any one of them made the login screen a doorway rather than a gate.
    local model = model_over()
    model:create("stranger-one")
    model:create("stranger-two")
    t.equal(#model.wallets, 2, "two wallets in the store to begin with")

    local phrase = model:offer_mnemonic(12)
    local account = model:login(phrase)

    t.ok(account, "should have got in")
    t.equal(#model.wallets, 1, "a new phrase owns exactly one wallet")
    t.equal(model.wallets[1].address, account.address, "and it is that one")
    t.equal(model.selected, 1, "with the card on it")
  end)

  t.case("the store still holds everything that was there", function()
    -- Scoped, not deleted. Nothing is removed from disk by logging in.
    local model = model_over()
    model:create("stranger")
    model:login(model:offer_mnemonic(12))
    t.equal(#model.wallets, 1, "the session sees one")
    -- Two wallets, one account per chain each: the stranger's and the
    -- session's.
    t.equal(#model.all_wallets, 2 * #model:chains(),
      "the store still holds both, whole")
  end)

  t.case("a phrase sees every wallet it controls", function()
    -- `account new` continues the active account's mnemonic, so a wallet
    -- made inside a session is the next index of that same phrase and must
    -- come back next time it is unlocked.
    local model = model_over()
    model:create("stranger")

    local phrase = model:offer_mnemonic(12)
    model:login(phrase)
    model:create("second")
    model:create("third")
    t.equal(#model.wallets, 3, "the wallet and the two made inside it")

    -- A fresh session over the same store: the three must be found again,
    -- by derivation rather than by anything remembered.
    local fresh = Model.new(model.wallet, nil)
    fresh:refresh()
    t.equal(#fresh.wallets, 4, "logged out, it sees the whole store")
    fresh:login(phrase)
    t.equal(#fresh.wallets, 3, "logged in, only what the phrase controls")

    local seen = {}
    for _, entry in ipairs(fresh.wallets) do seen[entry.label] = true end
    -- A named wallet spans every chain, so each face carries the chain's
    -- name; the list shows the chain in view's.
    t.ok(seen["second-evm"] and seen["third-evm"], "including the ones made last time")
    t.ok(not seen["stranger-evm"], "and not the one it does not own")
  end)

  t.case("a wallet with a gap in its indices is still found whole", function()
    -- The scan uses a gap limit rather than stopping at the first absence,
    -- because indices need not be contiguous: deriving one by hand leaves
    -- holes, and a wallet whose accounts are 0 and 3 is still one wallet.
    local model = model_over()
    local phrase = model:offer_mnemonic(12)
    model.wallet:import_mnemonic(phrase, { label = "first" })
    model.wallet:import_mnemonic(phrase, { index = 3, label = "fourth" })

    model:login(phrase)
    t.equal(#model.wallets, 2, "both sides of the gap should be found")
  end)

  t.case("more accounts than the scan reached stay visible", function()
    -- The set is built once, at login, and the scan stops a few indices past
    -- the last one stored. Making enough accounts in a single session walks
    -- off the end of it, so each one records itself as it is made.
    local model = model_over()
    local phrase = model:offer_mnemonic(12)
    model:login(phrase)
    for i = 1, 8 do model:create("extra-" .. i) end
    t.equal(#model.wallets, 9, "the wallet and all eight made inside it")

    -- And they are genuinely that phrase's, so a fresh session finds them.
    local fresh = Model.new(model.wallet, nil)
    fresh:refresh()
    fresh:login(phrase)
    t.equal(#fresh.wallets, 9, "and a later login finds them again")
  end)

  t.case("logging out shows the store again", function()
    local model = model_over()
    model:create("stranger")
    model:login(model:offer_mnemonic(12))
    t.equal(#model.wallets, 1)

    model:logout()
    t.equal(#model.wallets, 2, "logged out, the whole store is visible again")
  end)

  t.case("a phrase already in the store keeps its own wallets", function()
    local model = model_over()
    model:login(support.MNEMONIC)
    model:create("second")
    model:logout()

    -- Imported straight through the wallet, because that is the only way to
    -- get a genuinely unrelated one: `+ NEW` continues the *active* account's
    -- mnemonic, so it would have made another account of the same wallet
    -- rather than a stranger. NEW MNEMONIC on the login screen is how a
    -- separate wallet is started, which is exactly the distinction the
    -- scoping makes visible.
    local other = model:offer_mnemonic(12)
    model.wallet:import_mnemonic(other, { label = "stranger" })
    model:refresh()
    t.equal(#model.wallets, 3, "three in the store while logged out")

    model:login(support.MNEMONIC)
    t.equal(#model.wallets, 2, "the seeded wallet and its second account")
    t.equal(model.active, support.ADDRESS_0)
  end)

  t.case("+ NEW adds to the wallet you are in, not a separate one", function()
    -- Worth pinning: it is the difference between the two buttons. NEW
    -- MNEMONIC starts a wallet; + NEW adds an account to the one you are in,
    -- recoverable from the same phrase.
    local model = model_over()
    local phrase = model:offer_mnemonic(12)
    model:login(phrase)
    model:create("second")

    local fresh = Model.new(model.wallet, nil)
    fresh:refresh()
    fresh:login(phrase)
    t.equal(#fresh.wallets, 2,
      "the one phrase should bring both back, so nothing is stranded")
  end)

  t.case("logging out forgets the session and not the store", function()
    local model = model_over()
    model:login(support.MNEMONIC)
    model.balance = { balance = "5", symbol = "TCRO" }
    model:go("send")
    model:set_field("to", support.ADDRESS_1)

    t.ok(model:logout())
    t.ok(not model:logged_in(), "the session should be gone")
    t.equal(model.balance, nil, "and the balance with it")
    t.equal(model.screen, "wallets", "back to the first screen")
    t.equal(model.form.to, "", "with nothing left in the form")

    model:refresh()
    t.equal(#model.wallets, 1, "but the wallet is still on disk")
  end)

  t.case("a minted phrase is handed over and not stored", function()
    -- The point of `offer_mnemonic`: it is shown so it can be written down,
    -- and only becomes an account if it is used to log in. A wallet whose
    -- phrase was never seen cannot be recovered.
    local model = model_over()
    local phrase = model:offer_mnemonic(12)
    t.ok(phrase, "should have been handed a phrase")

    local words = 0
    for _ in phrase:gmatch("%S+") do words = words + 1 end
    t.equal(words, 12)
    t.equal(#model.wallets, 0, "offering one must write nothing")

    t.ok(model:login(phrase), "and it should work as a login")
    t.equal(#model.wallets, 1, "which is the point at which it is stored")
  end)

  t.case("two offered phrases differ", function()
    local model = model_over()
    t.not_equal(model:offer_mnemonic(12), model:offer_mnemonic(12))
  end)
end)

t.suite("model / saving and exporting", function()
  --- A model over a store with a known home, so the files can be looked at.
  local function stocked()
    local wallet, home = support.wallet()
    local model = Model.new(wallet, nil)
    model:refresh()
    model:create("one")
    model:create("two")
    return model, home
  end

  local function read(path)
    local handle = io.open(path, "r")
    if not handle then return nil end
    local body = handle:read("*a")
    handle:close()
    return body
  end

  t.case("saving writes all four formats beside the store", function()
    local model, home = stocked()
    local written = model:save_wallets()
    t.ok(written, "it should have reported what it wrote")
    t.equal(#written, 4, "four formats")

    for _, name in ipairs({ "wallets.csv", "wallets.md", "wallets.txt", "wallets.jsonl" }) do
      local body = read(home .. "/" .. name)
      t.ok(body ~= nil, name .. " should exist")
      for _, account in ipairs(model.wallets) do
        t.contains(body, account.address, name .. " should carry every wallet")
      end
    end
  end)

  t.case("what is saved holds no secret", function()
    -- The whole reason this is a separate verb from exporting. Losing this
    -- file should cost nothing.
    local model, home = stocked()
    model:save_wallets()
    local secret = model.wallet:export_account(model.wallets[1].address)
    for _, name in ipairs({ "wallets.csv", "wallets.md", "wallets.txt", "wallets.jsonl" }) do
      local body = read(home .. "/" .. name)
      t.ok(not body:find(secret.private_key, 1, true), "a private key reached " .. name)
      t.ok(not body:find(secret.mnemonic, 1, true), "a mnemonic reached " .. name)
    end
  end)

  t.case("exporting writes the keys, and everything asked of it", function()
    local model, home = stocked()
    local path = model:export_wallets()
    t.ok(path, "it should have reported the file")

    local lines = {}
    for line in read(path):gmatch("[^\n]+") do lines[#lines + 1] = line end
    t.equal(#lines, #model.wallets, "one line per wallet")

    local json = require("causewaybay.json")
    local record = json.decode(lines[1])
    t.ok(#record.mnemonic > 0, "a mnemonic")
    t.ok(record.index ~= nil, "an index")
    t.contains(record.private_key, "0x", "a private key")
    t.equal(#record.public_key_compressed, 68, "a 33-byte public key")
    t.equal(#record.public_key, 130, "a 64-byte public key")
    t.equal(record.address, record.address_checksummed:lower(), "both spellings")
  end)

  t.case("the exported keys are the real ones", function()
    -- A file of plausible-looking keys that do not open the wallets would be
    -- the worst possible outcome here: discovered only when it is needed.
    local model, home = stocked()
    model:export_wallets()

    local json = require("causewaybay.json")
    local body = read(home .. "/" .. require("export").SECRET_FILE)
    local first = json.decode(body:match("[^\n]+"))

    local derived = model.wallet:derive({ private_key = first.private_key })
    t.equal(derived.address:lower(), first.address,
      "the private key must derive the address it is filed under")

    local from_phrase = model.wallet:derive({ mnemonic = first.mnemonic, index = first.index })
    t.equal(from_phrase.address:lower(), first.address,
      "and so must the mnemonic at that index")
  end)

  t.case("saving and exporting are scoped to the session", function()
    -- The list on screen is scoped to the phrase; the files must be too, or
    -- the scoping quietly does not apply to the thing that leaves the machine.
    local model, home = stocked()
    local phrase = model:offer_mnemonic(12)
    model:login(phrase)
    t.equal(#model.wallets, 1, "the session sees one")

    model:save_wallets()
    model:export_wallets()

    local body = read(home .. "/wallets.csv")
    local count = 0
    for _ in body:gmatch("0x%x+") do count = count + 1 end
    t.equal(count, 1, "only the session's wallet should be in the file")
  end)

  t.case("nothing to write is refused rather than writing nothing", function()
    local wallet = support.wallet()
    local model = Model.new(wallet, nil)
    model:refresh()
    t.equal(model:save_wallets(), false)
    t.equal(model.status.kind, "error")
    t.equal(model:export_wallets(), false)
  end)

  t.case("saving asks first, and names the directory and every file", function()
    local model, home = stocked()
    t.ok(model:ask_save(), "it should have something to ask about")
    t.ok(model.write ~= nil, "a pending write")
    t.equal(model.write.dir, home, "the full directory, which is the point")
    t.equal(#model.write.files, 4, "every file it would write")
    t.equal(model.write.secret, false)
    -- Asked, not done: nothing may exist yet.
    t.equal(read(home .. "/wallets.csv"), nil, "nothing may be written before the answer")
  end)

  t.case("exporting asks first, and says what the file is worth", function()
    local model, home = stocked()
    t.ok(model:ask_export())
    t.equal(model.write.dir, home)
    t.equal(model.write.files[1], require("export").SECRET_FILE)
    t.equal(model.write.secret, true, "the dialog has to be able to shout")
    t.contains(model.write.note, "owns the money")
    t.equal(read(home .. "/" .. require("export").SECRET_FILE), nil,
      "no keys may be written before the answer")
  end)

  t.case("answering yes writes what was described", function()
    local model, home = stocked()
    model:ask_export()
    local path = model:confirm_write()
    t.equal(path, home .. "/" .. require("export").SECRET_FILE)
    t.ok(read(path) ~= nil, "the keys should be there now")
    t.equal(model.write, nil, "and nothing should still be pending")
    -- Where they went, in full, because a file name alone answers nothing.
    t.contains(model.status.text, home)
  end)

  t.case("it records where the file went, for the view to say so", function()
    -- The status line on the wallet screen is a hundred pixels wide and every
    -- path is longer, so the path has to survive somewhere the view can put it
    -- in more room than that.
    local model, home = stocked()
    model:ask_export()
    model:confirm_write()
    t.ok(model.written ~= nil)
    t.equal(model.written.dir, home)
    t.contains(model.written.where, require("export").SECRET_FILE)

    model:ask_save()
    model:confirm_write()
    t.equal(model.written.dir, home, "and the save records the directory it filled")
  end)

  t.case("answering no writes nothing at all", function()
    local model, home = stocked()
    model:ask_export()
    t.ok(model:cancel_write())
    t.equal(model.write, nil)
    t.equal(read(home .. "/" .. require("export").SECRET_FILE), nil,
      "cancelling must leave the disk alone")
    t.contains(model.status.text, home, "and still say which directory was spared")
  end)

  t.case("a save that is asked about and approved lands beside the store", function()
    local model, home = stocked()
    model:ask_save()
    local written = model:confirm_write()
    t.equal(#written, 4)
    t.ok(read(home .. "/wallets.csv") ~= nil)
    t.contains(model.status.text, home)
  end)

  t.case("a path is shown the way a person says it", function()
    local home = os.getenv("HOME") or "/root"
    t.equal(Model.tilde(home .. "/.causewaybaywallet"), "~/.causewaybaywallet")
    t.equal(Model.tilde(home), "~")
    -- Only a real prefix folds; anything else is left exactly as it resolved.
    t.equal(Model.tilde("/opt/wallets"), "/opt/wallets")
    t.equal(Model.tilde(home .. "-elsewhere/x"), home .. "-elsewhere/x")
  end)

  t.case("nothing to write is refused before anything is asked", function()
    local wallet = support.wallet()
    local model = Model.new(wallet, nil)
    model:refresh()
    t.equal(model:ask_save(), false)
    t.equal(model.write, nil, "a refusal must not leave a dialog behind")
    t.equal(model:ask_export(), false)
    t.equal(model.write, nil)
    -- And with nothing pending there is nothing to confirm.
    t.equal(model:confirm_write(), false)
    t.equal(model:cancel_write(), false)
  end)
end)

t.suite("model / wiping the store", function()
  t.case("logging out normally leaves everything alone", function()
    local wallet, home = support.wallet()
    local model = Model.new(wallet, nil)
    model:refresh()
    model:create("keep-me")

    model:logout()
    local handle = io.open(home .. "/accounts.jsonl", "r")
    t.ok(handle ~= nil, "the store must survive an ordinary logout")
    if handle then handle:close() end
  end)

  t.case("logging out with a wipe removes the store", function()
    local wallet, home = support.wallet()
    local model = Model.new(wallet, nil)
    model:refresh()
    model:create("goodbye")
    model:save_wallets()
    model:export_wallets()

    model:logout({ wipe = true })

    for _, name in ipairs({ "accounts.jsonl", "config.jsonl", "recent.jsonl",
        "wallets.csv", "wallets.jsonl", require("export").SECRET_FILE }) do
      local handle = io.open(home .. "/" .. name, "r")
      t.equal(handle, nil, name .. " should be gone")
      if handle then handle:close() end
    end
  end)

  t.case("an exported key file does not survive the wipe", function()
    -- Leaving the keys behind after deleting the store they came from would
    -- make the wipe a gesture rather than a fact.
    local wallet, home = support.wallet()
    local model = Model.new(wallet, nil)
    model:refresh()
    model:create("one")
    model:export_wallets()
    model:logout({ wipe = true })

    local handle = io.open(home .. "/" .. require("export").SECRET_FILE, "r")
    t.equal(handle, nil, "the exported keys must go too")
    if handle then handle:close() end
  end)

  t.case("it says how much it removed", function()
    local wallet = support.wallet()
    local model = Model.new(wallet, nil)
    model:refresh()
    model:create("one")
    model:logout({ wipe = true })
    t.equal(model.status.kind, "error", "a wipe is reported loudly")
    t.contains(model.status.text, "Wiped")
  end)
end)

t.suite("model / remembering a session", function()
  t.case("a snapshot carries no secret", function()
    -- The point of remembering a session is to skip the gate, not to keep the
    -- thing the gate asks for.
    local model = model_over()
    local phrase = model:login(model:offer_mnemonic(12)) and nil
    phrase = model:offer_mnemonic(12)
    model:login(phrase)

    local snapshot = model:session_snapshot()
    local blob = require("causewaybay.json").encode(snapshot)
    t.ok(not blob:find("abandon", 1, true), "no phrase")
    for word in phrase:gmatch("%S+") do
      t.ok(not blob:lower():find(word:lower() .. " ", 1, true),
        "no run of the phrase reached the snapshot")
    end
    local secret = model.wallet:export_account(model.session.address)
    t.ok(not blob:find(secret.private_key, 1, true), "no private key")
    t.ok(not blob:find(secret.mnemonic, 1, true), "no mnemonic")
  end)

  t.case("a restored session sees the same wallets", function()
    local model = model_over()
    -- An unrelated wallet, so the store genuinely holds more than the session
    -- does and the scoping has something to exclude.
    model:create("stranger")

    local phrase = model:offer_mnemonic(12)
    model:login(phrase)
    model:create("second")
    local before = #model.wallets
    local snapshot = model:session_snapshot()

    local fresh = Model.new(model.wallet, nil)
    fresh:refresh()
    t.ok(#fresh.wallets > before, "logged out it sees the whole store")

    t.ok(fresh:restore_session(snapshot), "it should come back")
    t.ok(fresh:logged_in(), "and be a session")
    t.equal(#fresh.wallets, before, "scoped exactly as it was")
    t.equal(fresh.active, snapshot.address, "and spending from the same wallet")
  end)

  t.case("there is no session to restore before logging in", function()
    t.equal(model_over():session_snapshot(), nil)
  end)

  t.case("a snapshot for a wallet that is gone is refused", function()
    -- Which is exactly what a wipe leaves behind. An empty bank with no way
    -- to say why is worse than being asked for the phrase again.
    local model = model_over()
    model:login(model:offer_mnemonic(12))
    local snapshot = model:session_snapshot()
    model:logout({ wipe = true })

    local fresh = Model.new(model.wallet, nil)
    t.equal(fresh:restore_session(snapshot), false, "the wallet is not there")
    t.ok(not fresh:logged_in(), "so there is no session")
  end)

  t.case("junk is refused rather than trusted", function()
    local model = model_over()
    for _, junk in ipairs({ nil, {}, "text", 42, { address = 7 },
        { address = "0xnope", addresses = {} } }) do
      t.equal(model:restore_session(junk), false)
      t.ok(not model:logged_in())
    end
  end)
end)

t.suite("model / the list window", function()
  local function model_with(count)
    local model = model_over()
    for i = 1, count do model:create("w" .. i) end
    return model
  end

  t.case("scrolling stops at both ends", function()
    local model = model_with(10)
    t.equal(model:scroll_by(-5, 6), 0, "it cannot scroll above the first row")
    t.equal(model:scroll_by(100, 6), 4, "nor past the last screenful")
    t.equal(model:scroll_by(2, 6), 4, "and staying there is not an error")
  end)

  t.case("a list that fits does not scroll at all", function()
    local model = model_with(3)
    t.equal(model:scroll_by(5, 6), 0)
  end)

  t.case("revealing pulls the row into view from either side", function()
    local model = model_with(20)

    model:reveal(15, 6)
    t.ok(model.scroll + 1 <= 15 and 15 <= model.scroll + 6,
      "row 15 should be on screen, scroll is " .. model.scroll)

    model:reveal(2, 6)
    t.ok(model.scroll + 1 <= 2 and 2 <= model.scroll + 6,
      "row 2 should be on screen, scroll is " .. model.scroll)
  end)

  t.case("revealing a row already on screen does not move the list", function()
    -- Otherwise the list jumps under the cursor for no reason on every press.
    local model = model_with(20)
    model:reveal(10, 6)
    local settled = model.scroll
    model:reveal(settled + 2, 6)
    t.equal(model.scroll, settled)
  end)

  t.case("stepping down moves the window by exactly one row", function()
    -- The precision the clamps hide. `reveal` scrolling one row too far is
    -- invisible at the ends of the list and invisible in the middle — the row
    -- asked for is still on screen either way — but it means holding the down
    -- arrow scrolls the list twice as fast as the cursor moves.
    local model = model_with(20)
    model:reveal(6, 6)
    t.equal(model.scroll, 0, "six rows fit, so nothing should have moved yet")

    model:reveal(7, 6)
    t.equal(model.scroll, 1, "the seventh row costs exactly one row of scroll")
    model:reveal(8, 6)
    t.equal(model.scroll, 2, "and the eighth exactly one more")
  end)

  t.case("stepping up moves the window by exactly one row", function()
    local model = model_with(20)
    model:reveal(20, 6)
    local bottom = model.scroll
    model:reveal(bottom, 6)
    t.equal(model.scroll, bottom - 1, "one row up is one row of scroll")
  end)

  t.case("the last row can be reached", function()
    local model = model_with(20)
    model:reveal(20, 6)
    t.equal(model.scroll, 14, "the window should end on the last row")
  end)
end)

t.suite("model / setting fields directly", function()
  t.case("a pasted value is trimmed and takes focus", function()
    local model = model_over()
    t.ok(model:set_field("to", "  " .. support.ADDRESS_1 .. "  "))
    t.equal(model.form.to, support.ADDRESS_1, "surrounding space must not survive")
    t.equal(model.focus, "to")
  end)

  t.case("clearing empties the field", function()
    local model = model_over()
    model:set_field("amount", "1.5")
    t.ok(model:clear_field("amount"))
    t.equal(model.form.amount, "")
  end)

  t.case("neither touches the form while a confirmation is up", function()
    -- The dialog owns the keyboard, and a paste that edited the amount
    -- underneath a confirmation would change what was approved.
    local model = model_over()
    model:set_field("to", support.ADDRESS_1)
    model.confirm = { summary = "pretend" }

    t.equal(model:set_field("to", "0xdeadbeef"), false)
    t.equal(model:clear_field("to"), false)
    t.equal(model.form.to, support.ADDRESS_1, "the form must be untouched")
  end)

  t.case("a non-string is refused rather than stored", function()
    -- `love.system.getClipboardText` returns nil when there is nothing on the
    -- clipboard, and that nil reaches here.
    local model = model_over()
    model:set_field("to", support.ADDRESS_1)
    t.equal(model:set_field("to", nil), false)
    t.equal(model:set_field("to", 42), false)
    t.equal(model.form.to, support.ADDRESS_1)
  end)
end)

t.suite("model / screens", function()
  t.case("goes to each named screen", function()
    local model = model_over()
    for _, name in ipairs(Model.SCREENS) do
      t.ok(model:go(name))
      t.equal(model.screen, name)
    end
  end)

  t.case("refuses a screen that does not exist", function()
    local model = model_over()
    t.equal(model:go("nowhere"), false)
    t.equal(model.screen, "wallets")
  end)
end)

t.suite("model / networks", function()
  t.case("lists every chain's networks and switches between them", function()
    local model = model_over()
    local networks = model:networks()

    local chains = {}
    for _, n in ipairs(networks) do chains[n.chain] = true end
    for _, chain in ipairs({ "evm", "solana", "cardano", "midnight", "ecash" }) do
      t.ok(chains[chain], chain .. " has no network to switch to")
    end

    t.ok(model:switch_network("cronos-mainnet"))
    t.equal(model.info.chain_id, 25)
    t.equal(model:chain(), "evm")
  end)

  t.case("the fee ceiling arrives with the token it is counted in", function()
    -- The GUI prints this on the network screen and nowhere else. A bare
    -- number would be unreadable there: fees are paid in a different token on
    -- every chain, and on Midnight in a different token from the balance drawn
    -- two frames away — "2" taken as NIGHT rather than DUST is out by a
    -- billion. So the symbol travels with the number, always.
    local model = model_over()
    t.ok(model.info.max_fee, "no ceiling reported at all")
    t.equal(model.info.max_fee_symbol, "TCRO")
    t.equal(model.info.max_fee_is_default, true)

    t.ok(model:switch_network("midnight-preview"))
    t.equal(model.info.max_fee_symbol, "DUST",
      "a Midnight fee is paid in DUST, not in the NIGHT a transfer moves")
  end)

  t.case("an unknown network is an error, not a crash", function()
    local model = model_over()
    t.equal(model:switch_network("ethereum"), false)
    t.equal(model.status.code, "unknown_network")
  end)
end)

t.suite("model / the asset in view", function()
  --- The registry row for one token, as `rows` hands it over.
  local function row(model, key)
    for _, entry in ipairs(model:rows()) do
      if entry.kind == "token" and entry.key == key then return entry end
    end
    return nil
  end

  t.case("a network row means that network's own coin", function()
    local model = model_over()
    t.ok(model:switch_network("cronos-mainnet"))
    local asset = model:asset()
    t.equal(asset.is_token, false)
    t.equal(asset.symbol, "CRO")
    t.equal(model.token, nil)
  end)

  t.case("picking a token row moves the window to it, network and all",
    function()
      -- The whole point: one click settles both halves. Reading a Cronos
      -- ERC-20 balance while the wallet is pointed at the testnet is not a
      -- smaller mistake than sending to it.
      local model = model_over()
      model:create("only")
      t.equal(model.info.network, "cronos-testnet")

      t.ok(model:pick_row(row(model, "usdc-cronos-mainnet")))
      t.equal(model.info.network, "cronos-mainnet", "the network moved too")
      t.equal(model:asset().is_token, true)
      t.equal(model:symbol(), "USDC")
      t.equal(model:asset().key, "usdc-cronos-mainnet")
    end)

  t.case("the balance and the send both follow the token", function()
    local model = model_over()
    model:create("only")
    t.ok(model:pick_row(row(model, "usdc-cronos-mainnet")))

    -- The command a send would run, without running it. A window showing a
    -- USDC balance that then sends CRO is the bug this pins.
    local argv = model:send_argv("0xabc", "25")
    t.equal(argv[1], "token")
    t.equal(argv[2], "send")
    t.equal(argv[3], "usdc-cronos-mainnet")

    -- And back to the coin, where it is the plain command again.
    t.ok(model:pick_row({ kind = "network", key = "cronos-mainnet" }))
    t.equal(model:send_argv("0xabc", "25")[1], "send")
  end)

  t.case("moving network drops the token rather than carrying it across",
    function()
      -- A token belongs to one network. Carried across, the window would
      -- claim to hold Cronos USDC on Solana.
      local model = model_over()
      model:create("only")
      t.ok(model:pick_row(row(model, "usdc-cronos-mainnet")))
      t.equal(model:symbol(), "USDC")

      t.ok(model:switch_network("solana-devnet"))
      t.equal(model.token, nil, "the token should not have followed")
      t.equal(model:asset().is_token, false)
    end)

  t.case("picking the network already in view drops the token", function()
    local model = model_over()
    model:create("only")
    t.ok(model:pick_row(row(model, "usdc-cronos-mainnet")))
    -- The row is `cronos-mainnet`, which is where the window already is — so
    -- there is no network to switch, only the token to let go of. Left as a
    -- no-op, this row would have been the one way into USDC with no way out.
    t.ok(model:pick_row({ kind = "network", key = "cronos-mainnet", current = true }))
    t.equal(model.token, nil)
    t.equal(model:symbol(), "CRO")
  end)

  t.case("an asset this wallet cannot move is refused before a node is asked",
    function()
      local model = model_over()
      model:create("only")
      t.ok(model:pick_row(row(model, "djed-cardano-mainnet")))
      t.equal(model:asset().transferable, false)
      t.equal(model:begin_send("addr1abc", "1"), false)
      t.equal(model.status.code, "usage")
      t.equal(model.confirm, nil, "nothing may be priced, let alone signed")
    end)

  t.case("a confirmation keeps the asset it was priced in", function()
    -- The window must not be able to move underneath an open question: a
    -- dialog priced in USDC and broadcast in CRO describes a transfer nobody
    -- made. The plan carries its own argv for exactly that reason.
    local model = model_over()
    model:create("only")
    t.ok(model:pick_row(row(model, "usdc-cronos-mainnet")))
    model.confirm = {
      summary = "Send 25 USDC", to = "0xabc", amount = "25",
      symbol = "USDC",
      argv = model:send_argv("0xabc", "25"),
    }
    local plan = model.confirm
    t.ok(model:switch_network("solana-devnet"))
    t.equal(plan.argv[3], "usdc-cronos-mainnet", "the plan moved with the window")
  end)
end)

t.suite("model / chains", function()
  t.case("lists the chains the library has, not a list kept here", function()
    local model = model_over()
    local chains = model:chains()
    t.equal(#chains, 5)

    local by_name = {}
    for _, c in ipairs(chains) do by_name[c.chain] = c end
    t.ok(by_name.solana, "solana is missing")
    t.ok(by_name.cardano, "cardano is missing")
    t.ok(by_name.midnight, "midnight is missing")
    t.ok(by_name.ecash, "ecash is missing")
    t.equal(by_name.solana.derivation_path, "m/44'/501'/0'/0'")
  end)

  t.case("switching chain lands on that chain's network", function()
    local model = model_over()
    t.equal(model:chain(), "evm")

    t.ok(model:switch_chain("solana"))
    t.equal(model:chain(), "solana")
    t.ok(model.info.network:match("^solana%-"), model.info.network)

    -- And back, without having to know which network key belongs to which.
    t.ok(model:switch_chain("cardano"))
    t.equal(model:chain(), "cardano")
    t.ok(model.info.network:match("^cardano%-"), model.info.network)
  end)

  t.case("a network is named by its key, however it was described", function()
    -- `chains` lists networks by key; the library's handshake hands back whole
    -- records. The switcher takes either, because it reads one and is given
    -- the other.
    t.equal(Model.network_key("solana-devnet"), "solana-devnet")
    t.equal(Model.network_key({ key = "solana-devnet", name = "Solana Devnet" }),
      "solana-devnet")
  end)

  t.case("switching to a chain the wallet holds nothing on still lands", function()
    -- The path that runs for a new wallet: no account anywhere, so the chain's
    -- own first network is the answer rather than one the store remembers.
    local model = model_over()
    t.equal(#model.wallets, 0)
    t.ok(model:switch_chain("cardano"), model.status and model.status.message)
    t.equal(model:chain(), "cardano")
    t.ok(model.info.network:match("^cardano%-"), model.info.network)
  end)

  t.case("a chain this build does not have is an error, not a crash", function()
    local model = model_over()
    t.equal(model:switch_chain("bitcoin"), false)
    t.equal(model.status.code, "unknown_chain")
    t.equal(model:chain(), "evm", "the chain in view did not move")
  end)
end)

t.suite("model / ecash", function()
  --- The eCash faces of the shared test phrase, at index 0. Both, because the
  --- prefix is inside a CashAddr's checksum: these are two different strings
  --- for one key, and neither decodes as the other.
  local TESTNET = "ectest:qrwzys2q6xq98vwz0kjn6ulu5m6yljr5fy7w393sue"
  local MAINNET = "ecash:qrwzys2q6xq98vwz0kjn6ulu5m6yljr5fyc909kalg"

  t.case("the chain is reachable and lands on an eCash network", function()
    local model = model_over()
    t.ok(model:switch_chain("ecash"), model.status and model.status.message)
    t.equal(model:chain(), "ecash")
    t.ok(model.info.network:match("^ecash%-"), model.info.network)
  end)

  --- The number the GUI draws largest, and the one this chain is easiest to
  --- get wrong: XEC has two decimal places where Bitcoin has eight. A balance
  --- read with eight is out by a factor of a million, in the direction that
  --- makes a large holding look like nothing.
  t.case("the asset is counted in two places, not Bitcoin's eight", function()
    local model = model_over()
    t.ok(model:switch_chain("ecash"))

    local asset = model:asset()
    t.equal(asset.is_token, false)
    t.equal(asset.symbol, "tXEC")
    t.equal(model:symbol(), "tXEC")
    t.equal(model.info.decimals, 2, "XEC is quoted in two places")

    -- And mainnet is the same chain under its own ticker.
    t.ok(model:switch_network("ecash-mainnet"))
    t.equal(model:symbol(), "XEC")
    t.equal(model.info.decimals, 2)
  end)

  --- The failure the wallet list exists to prevent, in the words of the
  --- comment in `Model:refresh`: an address drawn under the wrong chain's
  --- heading is an address offered for a deposit that cannot arrive there.
  t.case("a wallet shows its CashAddr face under the eCash heading", function()
    local model = model_over()
    model:create("mine")
    t.ok(model:switch_chain("ecash"), model.status and model.status.message)

    t.equal(#model.wallets, 1, "the wallet has an eCash face")
    local shown = model.wallets[1].address
    t.equal(model.wallets[1].chain, "ecash")
    t.ok(shown:match("^ectest:"), "not a CashAddr under eCash: " .. tostring(shown))
    t.equal(shown:match("^0[xX]"), nil, "an EVM address is drawn under eCash")

    -- Switching away and back must not leave the other chain's face behind.
    t.ok(model:switch_chain("evm"))
    t.ok(model.wallets[1].address:match("^0[xX]"), model.wallets[1].address)
    t.ok(model:switch_chain("ecash"))
    t.equal(model.wallets[1].address, shown, "the face came back unchanged")
  end)

  --- The address the GUI *offers* has to be the one for the network it is
  --- pointed at, because a person reads it off the screen and hands it over.
  t.case("the face follows the network within the chain", function()
    local model = model_over()
    model:create("mine")
    t.ok(model:switch_network("ecash-testnet"))
    local testnet_face = model.wallets[1].address

    t.ok(model:switch_network("ecash-mainnet"))
    local mainnet_face = model.wallets[1].address
    t.ok(mainnet_face:match("^ecash:"), "mainnet face: " .. tostring(mainnet_face))
    t.ok(testnet_face ~= mainnet_face,
      "one string served both networks, which is how funds cross the line")
  end)

  t.case("the derived faces are the ones the vectors pin down", function()
    -- Through the same C ABI the GUI uses, against the shared file rather
    -- than a string typed into this test.
    local model = model_over()
    local derived = model.wallet:derive({ mnemonic = support.MNEMONIC, index = 0, chain = "ecash" })
    t.ok(derived, "nothing derived")
    t.equal(derived.address, TESTNET)
    t.equal(derived.address_mainnet, MAINNET)
  end)

  --- A session is the set of addresses one phrase controls, and a chain whose
  --- addresses it failed to derive would make that phrase's own wallets
  --- invisible after a login.
  t.case("a login sees the wallets it owns on eCash too", function()
    local model = model_over()
    local phrase = support.MNEMONIC
    model.wallet:import_mnemonic(phrase, { label = "mine", every_chain = true })
    model:refresh()

    t.ok(model:login(phrase), model.status and model.status.message)
    t.ok(model:switch_chain("ecash"), model.status and model.status.message)
    t.equal(#model.wallets, 1, "the phrase cannot see its own eCash wallet")
    t.ok(model.wallets[1].address:match("^ectest:"), model.wallets[1].address)
  end)

  --- The fee ceiling is drawn on the network screen, and a bare number there
  --- means nothing: every chain pays in a different token.
  t.case("the fee ceiling arrives in XEC", function()
    local model = model_over()
    t.ok(model:switch_network("ecash-mainnet"))
    t.ok(model.info.max_fee, "no ceiling reported at all")
    t.equal(model.info.max_fee_symbol, "XEC")
  end)

  --- `switch_chain` takes the name the library uses. `bitcoin` is not it, and
  --- must not become it: eCash shares Bitcoin's transaction format and none of
  --- its chain, so a Bitcoin address accepted under that name would name an
  --- eCash account nobody holds the key to.
  t.case("bitcoin is not a name for this chain", function()
    local model = model_over()
    t.equal(model:switch_chain("bitcoin"), false)
    t.equal(model.status.code, "unknown_chain")
    t.equal(model:chain(), "evm", "the chain in view did not move")
  end)
end)

t.suite("model / sending", function()
  t.case("refuses an empty form before reaching a node", function()
    local model = model_over()
    t.equal(model:begin_send("", ""), false)
    t.equal(model.status.code, "usage")
    t.equal(model.confirm, nil)
  end)

  t.case("refuses a transfer to the wallet it would leave", function()
    -- The sender's own address is the one most likely to be on the clipboard,
    -- so this is a realistic paste — and it moves nothing while paying the gas.
    -- The wallet refuses it too; this is the same refusal without the round
    -- trip, so the form says so as soon as SEND is pressed.
    local model = model_over()
    model:create("only")
    t.equal(model:begin_send(model.active, "1"), false)
    t.equal(model.status.code, "usage")
    t.equal(model.confirm, nil, "nothing may be priced, let alone signed")

    -- Lower case is the same account: EIP-55 is a property of the text.
    t.equal(model:begin_send(model.active:lower(), "1"), false)
    t.equal(model.status.code, "usage")
  end)

  t.case("the confirmation drops the CLI's advice", function()
    -- The wallet's refusal ends with "— re-run with --yes to confirm", which
    -- is guidance for a shell and nonsense in a dialog with a button under it.
    t.equal(
      Model.plan_summary("Send 0.5 TCRO to 0xabc — re-run with --yes to confirm"),
      "Send 0.5 TCRO to 0xabc")
    t.equal(Model.plan_summary("Forget account old"), "Forget account old")
    t.equal(Model.plan_summary(nil), nil)
  end)

  t.case("cancelling clears the dialog and signs nothing", function()
    local model = model_over()
    model.confirm = { summary = "Send 1", to = "0xabc", amount = "1" }
    t.ok(model:cancel_send())
    t.equal(model.confirm, nil)
    t.contains(model.status.text, "nothing was signed")
  end)

  t.case("cancelling when nothing is pending does nothing", function()
    local model = model_over()
    t.equal(model:cancel_send(), false)
  end)

  t.case("confirming with no plan does nothing", function()
    local model = model_over()
    t.equal(model:confirm_send(), false)
  end)
end)

t.suite("model / quick send", function()
  --- `begin_send` replaced by a recorder, so these assert what the button asks
  --- for rather than what a node three seconds away thinks of it.
  local function watched(model)
    local seen = {}
    model.begin_send = function(_, to, amount)
      seen.to, seen.amount = to, amount
      return true
    end
    return seen
  end

  t.case("pays the row's own wallet, at the fixed amount", function()
    local model = model_over()
    model:create("alpha")
    model:create("beta")
    local seen = watched(model)

    t.ok(model:quick_send(2))
    t.equal(seen.to, model.wallets[2].address)
    t.equal(seen.amount, Model.QUICK_AMOUNT)
  end)

  t.case("does not make the row it pays the wallet it pays from", function()
    -- The whole point of the button: the row is the recipient. A press that
    -- also selected the row would move the money out of an account nobody
    -- chose — and the next send after it, too.
    local model = model_over()
    model:create("alpha")
    model:create("beta")
    local was_active, was_selected = model.active, model.selected
    watched(model)

    local other
    for index, account in ipairs(model.wallets) do
      if account.address ~= model.active then other = index end
    end
    t.ok(model:quick_send(other))
    t.equal(model.active, was_active, "the wallet being spent from must not move")
    t.equal(model.selected, was_selected, "and neither must the highlight")
  end)

  t.case("a wallet cannot pay itself", function()
    -- The view hides the button on the active row; this is the same refusal
    -- without depending on the view getting that right.
    local model = model_over()
    model:create("only")
    local mine
    for index, account in ipairs(model.wallets) do
      if account.address == model.active then mine = index end
    end
    t.equal(model:quick_send(mine), false)
    t.equal(model.status.code, "usage")
    t.equal(model.confirm, nil, "nothing may be priced, let alone signed")
  end)

  t.case("a row that is not there is refused, not indexed into", function()
    local model = model_over()
    t.equal(model:quick_send(1), false)
    t.equal(model.status.code, "usage")
  end)
end)

t.suite("model / the form", function()
  t.case("types, deletes and moves between fields", function()
    local model = model_over()
    model:type_into("0xab")
    t.equal(model.form.to, "0xab")
    model:backspace()
    t.equal(model.form.to, "0xa")

    model:next_field()
    t.equal(model.focus, "amount")
    model:type_into("1.5")
    t.equal(model.form.amount, "1.5")
    t.equal(model.form.to, "0xa", "the other field is untouched")

    model:next_field()
    t.equal(model.focus, "to", "it wraps")
  end)

  t.case("backspacing an empty field is harmless", function()
    local model = model_over()
    model:backspace()
    t.equal(model.form.to, "")
  end)

  t.case("a second send cannot start under an open confirmation", function()
    -- What went wrong: the dialog's SEND IT overlapped the screen's own SEND
    -- button by a few pixels, and both saw the same click. Confirming a
    -- transfer also began another, so a send that had gone through came back
    -- asking to be confirmed.
    --
    -- The view now blocks clicks behind a dialog. This is the guard that does
    -- not depend on the view getting its layout right.
    local model = model_over()
    model.confirm = { summary = "pretend", to = support.ADDRESS_1, amount = "1" }

    t.equal(model:begin_send(support.ADDRESS_1, "2"), false,
      "it must refuse while a confirmation is pending")
    t.equal(model.confirm.amount, "1",
      "and must not have replaced the plan that was on screen")
  end)

  t.case("sending again is fine once the dialog is gone", function()
    local model = model_over()
    model.confirm = { summary = "pretend", to = support.ADDRESS_1, amount = "1" }
    model:cancel_send()
    t.ok(model:begin_send(support.ADDRESS_1, "2"), "cancelled, so the way is clear")
  end)

  t.case("the dialog owns the keyboard while it is up", function()
    -- Typing behind a modal is how a person edits a transaction they think
    -- they are confirming.
    local model = model_over()
    model.confirm = { summary = "Send 1", to = "0xabc", amount = "1" }
    model:type_into("9")
    model:backspace()
    t.equal(model.form.to, "")
  end)
end)

t.suite("model / events", function()
  t.case("reports what happened, once", function()
    local model = model_over()
    model:create("alpha")
    local events = model:drain()
    t.ok(#events > 0, "creating should have emitted something")
    t.equal(#model:drain(), 0, "draining twice must not repeat them")
  end)

  t.case("a failure emits an error for the view to react to", function()
    local model = model_over()
    model:switch_network("nowhere")
    local seen = false
    for _, event in ipairs(model:drain()) do
      if event == "error" then seen = true end
    end
    t.ok(seen)
  end)
end)

t.suite("model / the faucet", function()
  --- A worker that answers whatever it is asked, by command, when pumped.
  ---
  --- Scripted rather than live: every faucet in the table is somebody else's
  --- server, and a test that spent real devnet SOL to check an animation would
  --- be a test that fails when a faucet is rate limited.
  local function scripted(replies)
    local queue, answers = {}, {}
    local jobs = {
      submit = function(request) queue[#queue + 1] = request end,
      poll = function()
        local answer = table.remove(answers, 1)
        if not answer then return nil end
        return answer.id, answer.envelope
      end,
    }
    --- Answer everything outstanding, then deliver it.
    function jobs.settle()
      while queue[1] do
        local request = table.remove(queue, 1)
        local name = request.argv[1]
        local reply = replies[name]
        if type(reply) == "function" then reply = reply() end
        answers[#answers + 1] = { id = request.id, envelope = reply }
      end
    end
    jobs.queue = queue
    return jobs
  end

  local function funded_model(jobs, network)
    local wallet = support.wallet()
    wallet:new_account({ label = "alpha", every_chain = true })
    local model = Model.new(wallet, jobs)
    if network then model:switch_network(network) end
    return model
  end

  t.case("a mainnet is refused without a panel, because there is no page", function()
    local model = funded_model(nil, "cronos-mainnet")
    t.equal(model:faucet_url(), nil)
    t.equal(model:request_faucet(), false)
    t.equal(model:faucet_showing(), false)
    t.contains(model.status.text, "mainnet")
  end)

  t.case("a web faucet opens on its address rather than pretending to ask", function()
    -- Cronos hands out TCRO from a page with a captcha on it. The wallet
    -- cannot answer a captcha, and the honest button hands over the address.
    local jobs = scripted({})
    local model = funded_model(jobs, "cronos-testnet")
    t.equal(model:faucet_is_automatic(), false)
    t.ok(model:request_faucet())
    t.equal(model.faucet.phase, "manual")
    t.contains(model.faucet.url, "https://")
    t.equal(#jobs.queue, 0, "nothing should have been asked of the node")
  end)

  t.case("solana's clusters are asked directly, and the arrival is the difference",
    function()
      local balances = { "0", "1" }
      local jobs = scripted({
        balance = function()
          local value = table.remove(balances, 1) or "1"
          return { ok = true, data = { balance = value, symbol = "SOL" } }
        end,
        airdrop = { ok = true, data = { id = "5xSig", amount = "1" } },
      })
      local model = funded_model(jobs, "solana-devnet")
      t.equal(model:faucet_is_automatic(), true)

      t.ok(model:request_faucet())
      t.equal(model.faucet.phase, "reading")

      -- The balance before, then the request, then the wait.
      jobs.settle(); model:pump()
      t.equal(model.faucet.before, 0)
      t.equal(model.faucet.phase, "asking")

      jobs.settle(); model:pump()
      t.equal(model.faucet.phase, "waiting")
      t.equal(model.faucet.id, "5xSig")

      -- Nothing is asked until the money has had time to land: a balance read
      -- fired the instant the airdrop returns shows the number that was
      -- already there.
      t.equal(model:tick(0.1), false, "it asked before waiting")
      t.equal(model:tick(Model.FAUCET_SETTLE), true)
      jobs.settle(); model:pump()

      t.equal(model.faucet.phase, "landed")
      t.equal(model.faucet.after, 1)
      t.equal(model.balance.balance, "1", "the card behind it moves too")
    end)

  t.case("a faucet that says no leaves the reason on the panel and in the line",
    function()
      local jobs = scripted({
        balance = { ok = true, data = { balance = "0", symbol = "SOL" } },
        airdrop = { ok = false,
          error = { code = "rpc_error", message = "airdrop request failed" } },
      })
      local model = funded_model(jobs, "solana-devnet")
      model:request_faucet()
      jobs.settle(); model:pump()
      jobs.settle(); model:pump()

      t.equal(model.faucet.phase, "failed")
      t.equal(model.faucet.error.code, "rpc_error")
      t.equal(model.status.kind, "error")
    end)

  t.case("a yes that never lands says so rather than counting to itself", function()
    -- The failure worth naming: the faucet accepted the request and the money
    -- has not turned up. Reporting that as an arrival would animate a change
    -- from a number to the same number, which reads as a bug, not as patience.
    local jobs = scripted({
      balance = { ok = true, data = { balance = "0", symbol = "SOL" } },
      airdrop = { ok = true, data = { id = "5xSig" } },
    })
    local model = funded_model(jobs, "solana-devnet")
    model:request_faucet()
    jobs.settle(); model:pump()
    jobs.settle(); model:pump()

    for _ = 1, Model.FAUCET_TRIES do
      model:tick(Model.FAUCET_GAP + Model.FAUCET_SETTLE)
      jobs.settle(); model:pump()
    end
    t.equal(model.faucet.phase, "slow")
    t.equal(model.faucet.tries, 0)
  end)

  t.case("closing it mid-flight leaves the answers with nowhere to land", function()
    -- The panel is what every callback checks itself against. Without that, a
    -- balance read still in the air would write into a run that had been
    -- dismissed, and reopen it.
    local jobs = scripted({
      balance = { ok = true, data = { balance = "0", symbol = "SOL" } },
      airdrop = { ok = true, data = { id = "5xSig" } },
    })
    local model = funded_model(jobs, "solana-devnet")
    model:request_faucet()
    t.ok(model:dismiss_faucet())
    jobs.settle(); model:pump()
    t.equal(model.faucet, nil)
  end)

  t.case("two runs cannot overlap", function()
    local jobs = scripted({ balance = { ok = true, data = { balance = "0" } } })
    local model = funded_model(jobs, "solana-devnet")
    t.ok(model:request_faucet())
    t.equal(model:request_faucet(), false)
  end)

  t.case("the panel is modal, like the other two", function()
    local jobs = scripted({})
    local model = funded_model(jobs, "cronos-testnet")
    t.equal(model:asking(), false)
    model:request_faucet()
    t.equal(model:asking(), true)
    model:dismiss_faucet()
    t.equal(model:asking(), false)
  end)

  t.case("the demo says it is one, and moves nothing", function()
    -- It exists because ten of the twelve networks cannot be asked at all, so
    -- the one moment worth watching was otherwise reachable only by being on
    -- Solana and being lucky. Every number in it is invented, and the flag is
    -- what lets the panel say so.
    local model = funded_model(nil, "cronos-testnet")
    t.ok(model:demo_faucet())
    t.equal(model.faucet.phase, "demo")
    t.equal(model.faucet.demo, true)
    t.ok(model.faucet.after > model.faucet.before)
    t.equal(model.balance, nil, "nothing was read and nothing moved")
  end)

  t.case("logging out takes the panel with it", function()
    local model = funded_model(nil, "cronos-testnet")
    model:request_faucet()
    model:logout({})
    t.equal(model.faucet, nil)
  end)
end)

t.suite("model / the explorer", function()
  t.case("each wallet carries the link that looks it up", function()
    local model = model_over()
    model:create("alpha")
    local link, account = model:explorer_link()
    t.ok(link, "the library hands one down with every account")
    t.contains(link, "explorer.cronos.org")
    t.contains(link, account.address)
  end)

  t.case("the link follows the network, cluster and all", function()
    -- The reason this comes from the library rather than being assembled
    -- here: Solana's explorer carries its cluster in a query string, so
    -- appending `/address/…` to the base URL gives a mainnet link that loads
    -- and shows an empty account.
    local wallet = support.wallet()
    wallet:new_account({ label = "alpha", every_chain = true })
    local model = Model.new(wallet, nil)
    model:switch_network("solana-devnet")
    local link = model:explorer_link()
    t.contains(link, "cluster=devnet")
    t.contains(link, "/address/")
  end)

  t.case("no wallet is nil rather than a link to nothing", function()
    local model = model_over()
    t.equal(model:explorer_link(), nil)
  end)
end)

t.suite("model / asynchrony", function()
  t.case("a submitted request is not answered until it is pumped", function()
    -- The shape the worker thread gives the real GUI, faked with a queue.
    local queue, answers = {}, {}
    local jobs = {
      submit = function(request) queue[#queue + 1] = request end,
      poll = function()
        local answer = table.remove(answers, 1)
        if not answer then return nil end
        return answer.id, answer.envelope
      end,
    }
    local wallet = support.wallet()
    wallet:new_account({ label = "alpha" })
    local model = Model.new(wallet, jobs)

    model:fetch_balance()
    t.equal(#queue, 1, "it should have been submitted")
    t.ok(model:busy(), "and the model should say it is busy")
    t.equal(model.balance, nil, "with no answer yet")

    answers[1] = {
      id = queue[1].id,
      envelope = { ok = true, data = { balance = "12.5", symbol = "TCRO" } },
    }
    model:pump()
    t.equal(model.balance.balance, "12.5")
    t.equal(model:busy(), false)
  end)

  t.case("an error from the worker becomes a status, not a crash", function()
    local answers = {}
    local jobs = {
      submit = function(request)
        answers[#answers + 1] = {
          id = request.id,
          envelope = { ok = false, error = { code = "rpc_error", message = "node is down" } },
        }
      end,
      poll = function()
        local answer = table.remove(answers, 1)
        if not answer then return nil end
        return answer.id, answer.envelope
      end,
    }
    local wallet = support.wallet()
    wallet:new_account({ label = "alpha" })
    local model = Model.new(wallet, jobs)

    model:fetch_balance()
    model:pump()
    t.equal(model.status.kind, "error")
    t.equal(model.status.code, "rpc_error")
  end)

  t.case("a balance with no wallets is refused before the node", function()
    local model = model_over()
    t.equal(model:fetch_balance(), false)
    t.equal(model.status.code, "no_active_account")
  end)
end)

return true
