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
    t.equal(#model.all_wallets, 2, "the store still holds both")
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
    t.ok(seen["second"] and seen["third"], "including the ones made last time")
    t.ok(not seen["stranger"], "and not the one it does not own")
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
  t.case("lists them and switches", function()
    local model = model_over()
    t.equal(#model:networks(), 2)
    t.ok(model:switch_network("cronos-mainnet"))
    t.equal(model.info.chain_id, 25)
  end)

  t.case("an unknown network is an error, not a crash", function()
    local model = model_over()
    t.equal(model:switch_network("ethereum"), false)
    t.equal(model.status.code, "unknown_network")
  end)
end)

t.suite("model / sending", function()
  t.case("refuses an empty form before reaching a node", function()
    local model = model_over()
    t.equal(model:begin_send("", ""), false)
    t.equal(model.status.code, "usage")
    t.equal(model.confirm, nil)
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
