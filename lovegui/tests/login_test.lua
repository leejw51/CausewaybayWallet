--- Tests for the login screen.
---
--- One of these matters more than the rest, and it is the masking. The rule is
--- that a mnemonic is never drawn — not masked-with-a-reveal, *never* — and a
--- rule like that survives exactly as long as somebody is checking. It is one
--- careless edit away from being untrue, the edit looks harmless, and nothing
--- else in the program would notice.
---
--- Everything here runs without LÖVE. `login:draw` needs a window, but every
--- decision the screen makes — what is shown, what is counted, which button
--- appears, what happens on submit — is above the drawing and is asserted
--- directly.

local t = require("tests.runner")
local support = require("tests.support")
local Login = require("login")
local Model = require("model")

local PHRASE = support.MNEMONIC

local function model_over(options)
  return Model.new(support.wallet(options), nil)
end

t.suite("login / the phrase is never shown", function()
  t.case("every word character becomes a star", function()
    local screen = Login.new()
    screen:type_into(PHRASE)
    local shown = screen:shown()

    -- The whole rule, stated as an assertion: nothing of the phrase survives
    -- into what gets drawn.
    t.ok(not shown:find("%a"), "a letter reached the screen: " .. shown)
    t.ok(not shown:find("%d"), "a digit reached the screen: " .. shown)
    for word in PHRASE:gmatch("%S+") do
      t.ok(not shown:find(word, 1, true), "the word '" .. word .. "' reached the screen")
    end
  end)

  t.case("the mask is the same length as the phrase", function()
    -- Word boundaries stay, because the count of words is the feedback that
    -- replaces seeing them. Only the characters go.
    local screen = Login.new()
    screen:type_into("alpha bravo charlie")
    t.equal(screen:shown(), "***** ***** *******")
  end)

  t.case("there is no way to ask for the phrase back", function()
    -- A reveal toggle was deliberately not built. If one is ever added, this
    -- fails and whoever added it has to come and read the comment at the top
    -- of login.lua first.
    local screen = Login.new()
    for _, name in ipairs({ "reveal", "show", "unmask", "plain", "visible" }) do
      t.equal(screen[name], nil, "login should have no '" .. name .. "'")
      t.equal(Login[name], nil, "the module should have no '" .. name .. "'")
    end
  end)

  t.case("a phrase this screen minted is masked too", function()
    -- The one case with an argument for showing it — you cannot write down
    -- what you cannot see — and the answer is still no. COPY takes it to the
    -- clipboard without it ever being on screen.
    local screen = Login.new()
    screen.phrase = PHRASE
    screen.minted = true
    t.ok(not screen:shown():find("%a"), "a minted phrase must be masked as well")
  end)

  t.case("an empty field shows nothing at all", function()
    t.equal(Login.new():shown(), "")
  end)
end)

t.suite("login / counting words", function()
  t.case("it counts what was typed", function()
    local screen = Login.new()
    t.equal(screen:words(), 0)
    screen:type_into("alpha")
    t.equal(screen:words(), 1)
    screen:type_into(" bravo charlie")
    t.equal(screen:words(), 3)
  end)

  t.case("runs of whitespace are one gap, not several", function()
    local screen = Login.new()
    screen:type_into("  alpha   bravo  ")
    t.equal(screen:words(), 2, "spacing must not inflate the count")
  end)

  t.case("a real mnemonic counts twelve", function()
    local screen = Login.new()
    screen:type_into(PHRASE)
    t.equal(screen:words(), 12)
  end)
end)

t.suite("login / typing", function()
  t.case("a pasted phrase arrives without its newlines", function()
    -- A mnemonic copied out of a document comes with line breaks in it, and a
    -- newline in the field is a word boundary that does not look like one.
    local screen = Login.new()
    screen:type_into("alpha\nbravo\r\ncharlie\tdelta")
    t.ok(not screen.phrase:find("[\n\r\t]"), "control characters must not survive")
    t.equal(screen:words(), 4)
  end)

  t.case("backspace removes one character", function()
    local screen = Login.new()
    screen:type_into("abc")
    screen:backspace()
    t.equal(screen.phrase, "ab")
  end)

  t.case("backspacing an empty field is harmless", function()
    local screen = Login.new()
    screen:backspace()
    t.equal(screen.phrase, "")
  end)

  t.case("word-delete removes a whole word and its space", function()
    -- What a person wants when they mistyped the eleventh word: not eight
    -- presses of backspace.
    local screen = Login.new()
    screen:type_into("alpha bravo charlie")
    screen:backword()
    t.equal(screen.phrase, "alpha bravo")
    screen:backword()
    t.equal(screen.phrase, "alpha")
  end)

  t.case("word-delete on an empty field is harmless", function()
    local screen = Login.new()
    screen:backword()
    t.equal(screen.phrase, "")
  end)
end)

t.suite("login / tidying a pasted phrase", function()
  t.case("it trims and collapses", function()
    t.equal(Login.tidy("  alpha   bravo  "), "alpha bravo")
  end)

  t.case("newlines become single spaces", function()
    t.equal(Login.tidy("alpha\n\nbravo\r\ncharlie"), "alpha bravo charlie")
  end)

  t.case("an already-clean phrase is untouched", function()
    t.equal(Login.tidy(PHRASE), PHRASE)
  end)

  t.case("it returns one value, not two", function()
    -- `gsub` returns a count as its second result. Letting that escape means
    -- `tidy(x)` in an argument list silently becomes two arguments.
    local a, b = Login.tidy("  spaced  ")
    t.equal(a, "spaced")
    t.equal(b, nil, "the gsub count must not leak out")
  end)
end)

t.suite("login / paste or copy", function()
  -- The rule: PASTE for a phrase you already have, COPY for one just minted.
  -- `minted` is the flag the button reads, so these assert the flag.

  t.case("a fresh screen offers paste", function()
    t.equal(Login.new().minted, false)
  end)

  t.case("typing clears minted, because it is no longer that phrase", function()
    local screen = Login.new()
    screen.minted = true
    screen:type_into("x")
    t.equal(screen.minted, false,
      "a phrase that has been typed into is not the one that was minted")
  end)

  t.case("minting sets it, and a successful login clears it", function()
    local model = model_over()
    local screen = Login.new()

    local phrase = model:offer_mnemonic(12)
    t.ok(phrase and #phrase > 0, "should have been handed a phrase")
    screen.phrase, screen.minted, screen.copied = phrase, true, false

    t.ok(screen:submit(model), "a freshly minted phrase should unlock")
    t.equal(screen.minted, false, "and the screen is no longer holding one")
    t.equal(screen.phrase, "", "nor the phrase itself")
  end)
end)

t.suite("login / submitting", function()
  t.case("a good phrase gets in and clears the field", function()
    local model = model_over()
    local screen = Login.new()
    screen:type_into(PHRASE)

    local account = screen:submit(model)
    t.ok(account, "should have returned the account")
    t.equal(account.address, support.ADDRESS_0)
    t.ok(model:logged_in(), "the model should hold a session")
    t.equal(screen.phrase, "", "the field must not keep the phrase")
  end)

  t.case("a bad phrase is refused and the field is kept", function()
    -- Kept on purpose: a typo in the eleventh word should not cost all
    -- twelve. The shake is the feedback instead.
    local model = model_over()
    local screen = Login.new()
    screen:type_into("abandon abandon not-a-word")

    t.equal(screen:submit(model), false)
    t.ok(not model:logged_in(), "nothing should have been unlocked")
    t.ok(#screen.phrase > 0, "the phrase should still be there to correct")
    t.ok(screen.shake > 0, "and the screen should have reacted")
  end)

  t.case("a rejected phrase writes nothing to the store", function()
    -- The check happens before anything is written, so a typo produces a
    -- message and never a stray account.
    local model = model_over()
    local before = #model.wallets
    local screen = Login.new()
    screen:type_into("abandon abandon abandon")
    screen:submit(model)
    model:refresh()
    t.equal(#model.wallets, before, "a bad phrase must not add a wallet")
  end)

  t.case("an empty field is refused before the wallet is asked", function()
    local model = model_over()
    local screen = Login.new()
    t.equal(screen:submit(model), false)
    t.equal(model.status.kind, "error")
  end)

  t.case("a phrase already in the store selects it rather than importing", function()
    local wallet = support.seeded_wallet()
    local model = Model.new(wallet, nil)
    model:refresh()
    local before = #model.wallets

    local screen = Login.new()
    screen:type_into(PHRASE)
    local account = screen:submit(model)

    t.ok(account, "should have got in")
    t.equal(#model.wallets, before, "and stored nothing new")
    t.contains(model.status.text, "Welcome back")
  end)
end)

t.suite("login / motion", function()
  t.case("the shake settles rather than ringing forever", function()
    local screen = Login.new()
    screen.shake = 5
    for _ = 1, 60 do screen:update(1 / 60) end
    t.ok(screen.shake < 0.05, "after a second it should be still, got " .. screen.shake)
  end)

  t.case("the shake decays the same however the frames fall", function()
    -- The frame-rate independence the rest of the UI has. A field that
    -- shook differently on a slow machine would be a field that shook
    -- differently every time the wallet blocked.
    local one = Login.new()
    one.shake = 5
    one:update(0.5)

    local many = Login.new()
    many.shake = 5
    for _ = 1, 500 do many:update(0.001) end

    t.ok(math.abs(one.shake - many.shake) < 0.001,
      "one big step and five hundred small ones should agree: "
        .. one.shake .. " vs " .. many.shake)
  end)
end)
