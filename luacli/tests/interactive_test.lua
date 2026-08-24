--- Tests for the interactive menu.
---
--- `interactive.loop` takes its wallet and its two streams as arguments and
--- returns a status, so a session is a string of scripted answers in and a
--- string of output out — no subprocess, no pty, no timing.
---
--- Anything that reaches a node is left alone here: the Rust suite covers the
--- send path against a mock, and what this needs to prove is the *prompting*
--- — that a blank line is not mistaken for a closed pipe, that a refused
--- confirmation signs nothing, and that end of input always ends the session.

local t = require("tests.runner")
local support = require("tests.support")
local interactive = require("causewaybay.interactive")

--- A wallet opened the way a real interactive session opens one.
---
--- `support.wallet()` assumes yes, which suits the API tests; a session must
--- not, or the confirmations these tests are about would never be asked.
local function session_wallet()
  return support.wallet({ yes = false })
end

--- Run a session against `wallet`, answering with `answers` in order.
---
--- Returns the status and everything written. Once the answers run out the
--- reader returns nil, which is end of input — so every session terminates
--- whether or not the script remembered to say "q".
local function session(wallet, answers, options)
  options = options or {}
  local out = support.capture()
  local next_answer = 0
  local read = function()
    next_answer = next_answer + 1
    return answers[next_answer]
  end
  local secrets_read = 0
  local status = interactive.loop({
    wallet = wallet,
    write = function(text) out:write(text) end,
    read = read,
    -- A test has no terminal, so nothing is really hidden; `hidden` says what
    -- the session should *claim*, which is what the prompts branch on.
    hidden = options.hidden ~= false,
    read_secret = function()
      secrets_read = secrets_read + 1
      return read()
    end,
  })
  return status, out:text(), secrets_read
end

t.suite("interactive / the loop", function()
  t.case("quits on q", function()
    local status, out = session(support.wallet(), { "q" })
    t.equal(status, interactive.EXIT_OK)
    t.contains(out, "bye.")
  end)

  t.case("quits when the input ends", function()
    -- A closed pipe must end the session, not spin on nil forever. The test
    -- passing at all is the assertion: a regression here hangs the suite.
    local status = session(support.wallet(), {})
    t.equal(status, interactive.EXIT_OK)
  end)

  t.case("shows the menu and the wallet it opened", function()
    local wallet, home = support.wallet()
    local _, out = session(wallet, { "q" })
    t.contains(out, "Causewaybay Wallet — interactive")
    t.contains(out, home)
    t.contains(out, "cronos-testnet")
    t.contains(out, "unencrypted")
    for _, action in ipairs(interactive.ACTIONS) do
      t.contains(out, action.label)
    end
  end)

  t.case("an unrecognised word is tried as a command, not refused", function()
    -- The prompt is a REPL as well as a menu, so anything that is not a menu
    -- key goes to the wallet and comes back with the wallet's own complaint.
    local _, out = session(support.wallet(), { "zzz", "q" })
    t.contains(out, "[usage]")
    t.contains(out, "zzz")
  end)

  t.case("a failed action does not end the session", function()
    -- `4` is balance, which has no node behind it here; the menu must come
    -- back rather than the session falling over.
    local _, out = session(support.seeded_wallet(), { "4", "q" })
    t.contains(out, "bye.")
  end)
end)

t.suite("interactive / the REPL", function()
  t.case("runs a typed command and prints what the CLI would", function()
    local wallet = support.wallet()
    local _, out = session(wallet, { "account new -l alpha", "q" })
    -- One command, one wallet, however many chains it landed on.
    t.contains(out, "Created 1 account")
    t.contains(out, "alpha")
    t.equal(#wallet:accounts(), 1)
  end)

  t.case("a number is still the menu", function()
    -- The two live at one prompt; nothing is ambiguous because menu keys are
    -- digits and commands begin with a letter.
    local wallet = support.wallet()
    wallet:new_account({ label = "alpha" })
    local _, out = session(wallet, { "2", "q" })
    t.contains(out, "1 wallet")
  end)

  t.case("prints the wallet's own error for an unknown command", function()
    local _, out = session(support.wallet(), { "teleport", "q" })
    t.contains(out, "[usage]")
    t.contains(out, "teleport")
    t.contains(out, "bye.", "the session must survive it")
  end)

  t.case("a bare Enter just prompts again", function()
    local _, out = session(support.wallet(), { "", "", "q" })
    t.contains(out, "bye.")
    t.equal(out:find("not a choice"), nil)
  end)

  t.case("menu redraws it, and it is drawn once at the start", function()
    local _, out = session(support.wallet(), { "menu", "q" })
    local seen = select(2, out:gsub("switch network", ""))
    t.equal(seen, 2, "once at the start, once for `menu`")
  end)

  t.case("help is the wallet's own help", function()
    local _, out = session(support.wallet(), { "help", "q" })
    t.contains(out, "Usage:")
    t.contains(out, "account")
  end)

  t.case("help takes a command", function()
    local _, out = session(support.wallet(), { "help account new", "q" })
    t.contains(out, "--words")
  end)

  t.case("asks instead of telling you to re-run with --yes", function()
    local wallet = session_wallet()
    wallet:new_account({ label = "doomed" })
    local _, out = session(wallet, { "account remove doomed", "y", "q" })
    t.contains(out, "Remove account doomed")
    -- The CLI's advice has no place in a prompt that is asking right now.
    t.equal(out:find("re-run with"), nil)
    t.equal(#wallet:accounts(), 0)
  end)

  t.case("declining a confirmation changes nothing", function()
    local wallet = session_wallet()
    wallet:new_account({ label = "doomed" })
    local _, out = session(wallet, { "account remove doomed", "n", "q" })
    t.contains(out, "cancelled.")
    t.equal(#wallet:accounts(), 1)
  end)

  t.case("a confirmed command runs exactly once", function()
    -- The bug this pins: reading the data and the human text from two separate
    -- calls, which would broadcast a confirmed send twice.
    local calls = {}
    local fake = {
      envelope = function(_, argv)
        calls[#calls + 1] = { kind = "envelope", argv = argv }
        return { ok = false, error = { code = "confirmation_required", message = "Send 1 TCRO" } }
      end,
      call = function(_, argv, options)
        calls[#calls + 1] = { kind = "call", argv = argv, yes = options and options.yes }
        return { hash = "0xabc" }, "sent 0xabc"
      end,
    }
    local out = support.capture()
    local answers, i = { "y" }, 0
    interactive.run_command({
      wallet = fake,
      write = function(text) out:write(text) end,
      read = function() i = i + 1; return answers[i] end,
    }, { "send", "--to", "0xabc", "--amount", "1" })

    local ran = 0
    for _, entry in ipairs(calls) do
      if entry.kind == "call" then ran = ran + 1 end
    end
    t.equal(ran, 1, "the confirmed command must be executed once")
    t.contains(out:text(), "sent 0xabc")
  end)
end)

t.suite("interactive / typed arguments", function()
  local split = interactive.split

  t.case("splits on whitespace", function()
    local words = split("account list")
    t.equal(#words, 2)
    t.equal(words[1], "account")
    t.equal(words[2], "list")
    t.equal(#split("   spaced   out   "), 2)
  end)

  t.case("keeps a quoted argument in one piece", function()
    -- A label with a space, and a message with one, are ordinary things to
    -- type; splitting them would send the wallet two arguments.
    local words = split('account rename old "my main wallet"')
    t.equal(#words, 4)
    t.equal(words[4], "my main wallet")
    t.equal(split("sign 'hello there'")[2], "hello there")
  end)

  t.case("keeps an empty quoted argument", function()
    -- `--label ""` must reach the wallet so it can reject it, not vanish here.
    local words = split('account new --label ""')
    t.equal(#words, 4)
    t.equal(words[4], "")
  end)

  t.case("handles escapes", function()
    t.equal(split('sign "say \\"hi\\""')[2], 'say "hi"')
    t.equal(split("sign hello\\ there")[2], "hello there")
  end)

  t.case("reports an unbalanced quote rather than guessing", function()
    local words, reason = split('account rename old "unfinished')
    t.equal(words, nil)
    t.contains(reason, "unbalanced")
  end)

  t.case("an unbalanced quote is reported to the user", function()
    local _, out = session(support.wallet(), { 'account rename a "b', "q" })
    t.contains(out, "unbalanced")
    t.contains(out, "bye.")
  end)

  t.case("a dash is asked for rather than piped", function()
    -- The CLI reads a pipe here; a prompt has no pipe, so it asks.
    local wallet = support.wallet()
    local _, out, secrets = session(wallet, {
      "account import-mnemonic -m - -l typed", support.MNEMONIC, "q",
    })
    t.equal(secrets, 1, "a mnemonic must be read with echo off")
    t.contains(out, support.ADDRESS_0)
    t.equal(wallet:account("typed").address, support.ADDRESS_0)
  end)

  t.case("a dash that is not key material is asked for in the open", function()
    local wallet = support.seeded_wallet()
    local _, out, secrets = session(wallet, { "sign -", "hello there", "q" })
    t.equal(secrets, 0, "a message is not a secret")
    t.contains(out, "text for `-`")
    t.contains(out, support.ADDRESS_0)
  end)
end)

t.suite("interactive / prompting", function()
  --- A context that answers from a list, for testing the prompts directly.
  local function ctx_for(answers)
    local out = support.capture()
    local i = 0
    return {
      wallet = false,
      write = function(text) out:write(text) end,
      read = function()
        i = i + 1
        return answers[i]
      end,
    }, out
  end

  t.case("an empty line is an empty answer, not the end of input", function()
    -- The bug this pins: "press enter to print it here" silently abandoning
    -- the action, because a blank line and a closed pipe looked the same.
    local ctx = ctx_for({ "" })
    t.equal(interactive.ask(ctx, "path"), "")
  end)

  t.case("an empty line takes the default when there is one", function()
    local ctx = ctx_for({ "" })
    t.equal(interactive.ask(ctx, "words", "12"), "12")
  end)

  t.case("end of input is nil, and only that", function()
    local ctx = ctx_for({})
    t.equal(interactive.ask(ctx, "anything"), nil)
    t.equal(interactive.ask(ctx_for({}), "with a default", "12"), nil)
  end)

  t.case("answers are trimmed", function()
    t.equal(interactive.ask(ctx_for({ "  0xabc  " }), "address"), "0xabc")
  end)

  t.case("the prompt shows the default", function()
    local ctx, out = ctx_for({ "" })
    interactive.ask(ctx, "words", "12")
    t.contains(out:text(), "words [12]: ")
  end)

  t.case("confirmation defaults to no", function()
    t.equal(interactive.confirm(ctx_for({ "" }), "really"), false)
    t.equal(interactive.confirm(ctx_for({ "n" }), "really"), false)
    t.equal(interactive.confirm(ctx_for({ "nonsense" }), "really"), false)
    t.equal(interactive.confirm(ctx_for({ "y" }), "really"), true)
    t.equal(interactive.confirm(ctx_for({ "YES" }), "really"), true)
    t.equal(interactive.confirm(ctx_for({}), "really"), nil)
  end)
end)

t.suite("interactive / creating a wallet", function()
  t.case("adds an address on the wallet's own mnemonic", function()
    local wallet = support.wallet()
    local _, out = session(wallet, { "1", "1", "alpha", "q" })
    t.contains(out, "created alpha at 0x")
    t.equal(#wallet:accounts(), 1)
    t.equal(wallet:account("alpha").source, "mnemonic")
  end)

  t.case("takes an automatic label when none is given", function()
    local wallet = support.wallet()
    session(wallet, { "1", "1", "", "q" })
    local accounts = wallet:accounts()
    t.equal(#accounts, 1)
    t.ok(#accounts[1].label > 0)
  end)

  t.case("mints a fresh mnemonic on request", function()
    local wallet = support.wallet()
    local _, out = session(wallet, { "1", "2", "fresh", "24", "q" })
    t.contains(out, "created fresh at 0x")
    local exported = wallet:export_account("fresh")
    local words = 0
    for _ in exported.mnemonic:gmatch("%S+") do words = words + 1 end
    t.equal(words, 24)
  end)

  t.case("imports a mnemonic at the reference address", function()
    local wallet = support.wallet()
    local _, out = session(wallet, { "1", "3", "imported", support.MNEMONIC, "0", "q" })
    t.contains(out, support.ADDRESS_0)
    t.equal(wallet:account("imported").address, support.ADDRESS_0)
  end)

  t.case("imports at a chosen index", function()
    local wallet = support.wallet()
    session(wallet, { "1", "3", "second", support.MNEMONIC, "1", "q" })
    t.equal(wallet:account("second").address, support.ADDRESS_1)
  end)

  t.case("imports a private key", function()
    local wallet = support.wallet()
    session(wallet, { "1", "4", "raw", support.PRIVATE_KEY, "q" })
    t.equal(wallet:account("raw").address, support.ADDRESS_0)
    t.equal(wallet:account("raw").source, "private_key")
  end)

  t.case("reads a mnemonic through the hidden reader, not the plain one", function()
    -- The property that matters: the phrase never goes through the reader that
    -- a terminal would be echoing. Counting the hidden reads is how a
    -- regression — someone swapping ask_secret back for ask_required — fails.
    local wallet = support.wallet()
    local _, out, secrets = session(wallet, { "1", "3", "x", support.MNEMONIC, "0", "q" })
    t.equal(secrets, 1, "the mnemonic should be read with echo off")
    t.contains(out, "input is hidden")
    t.equal(wallet:account("x").address, support.ADDRESS_0)
  end)

  t.case("reads a private key the same way", function()
    local wallet = support.wallet()
    local _, _, secrets = session(wallet, { "1", "4", "raw", support.PRIVATE_KEY, "q" })
    t.equal(secrets, 1)
    t.equal(wallet:account("raw").address, support.ADDRESS_0)
  end)

  t.case("a label is not a secret", function()
    -- Only the phrase and the key are hidden; hiding the label too would be
    -- baffling to type into.
    local _, _, secrets = session(support.wallet(), { "1", "1", "alpha", "q" })
    t.equal(secrets, 0)
  end)

  t.case("says so when the terminal cannot hide input", function()
    local _, out = session(support.wallet(), { "1", "3", "x", support.MNEMONIC, "0", "q" },
      { hidden = false })
    t.contains(out, "cannot hide")
    t.contains(out, "will be visible")
  end)

  t.case("reports a bad mnemonic and carries on", function()
    local wallet = support.wallet()
    local _, out = session(wallet, { "1", "3", "bad", "not a real phrase", "0", "q" })
    t.contains(out, "[invalid_mnemonic]")
    t.equal(#wallet:accounts(), 0)
    t.contains(out, "bye.")
  end)
end)

t.suite("interactive / listing and selecting", function()
  t.case("lists every wallet and marks the active one", function()
    local wallet = support.wallet()
    wallet:new_account({ label = "one" })
    wallet:new_account({ label = "two" })
    local _, out = session(wallet, { "2", "q" })
    t.contains(out, "one")
    t.contains(out, "two")
    t.contains(out, "(active)")
    t.contains(out, "2 wallets")
  end)

  t.case("says there are none rather than printing an empty list", function()
    local _, out = session(support.wallet(), { "2", "q" })
    t.contains(out, "no wallets yet")
  end)

  t.case("switches the active wallet", function()
    local wallet = support.wallet()
    wallet:new_account({ label = "one" })
    local two = wallet:new_account({ label = "two" })
    local _, out = session(wallet, { "3", "2", "q" })
    t.contains(out, "active wallet is now two")
    t.equal(wallet:account().address, two.address)
  end)

  t.case("re-asks when the choice is not on the list", function()
    local wallet = support.wallet()
    wallet:new_account({ label = "one" })
    local _, out = session(wallet, { "3", "9", "1", "q" })
    t.contains(out, "pick one of the numbers")
    t.equal(wallet:account().label, "one")
  end)
end)

t.suite("interactive / sending", function()
  -- There is no node here, so a send cannot complete. What these check is the
  -- part before that: that nothing is signed without an explicit yes, and that
  -- the summary shown is the wallet's own rather than one assembled locally.

  t.case("a cancelled send signs nothing", function()
    local wallet = support.seeded_wallet()
    local _, out = session(wallet, { "5", support.ADDRESS_1, "0.5", "n", "q" })
    -- Either the node was unreachable or the confirmation was declined; in
    -- neither case may anything have been broadcast.
    t.equal(#(wallet:history() or {}), 0)
    t.contains(out, "bye.")
  end)

  t.case("asks for a recipient and an amount before anything else", function()
    local _, out = session(support.seeded_wallet(), { "5", "", "", "q" })
    t.contains(out, "recipient address")
    t.contains(out, "that one is required")
  end)

  t.case("the confirmation drops the CLI's advice", function()
    -- The wallet's refusal ends with "— re-run with --yes to confirm", which
    -- is guidance for a shell and nonsense in a prompt.
    local summary = interactive.plan_summary(
      "Send 0.5 TCRO from main to 0xabc on Cronos EVM Testnet — re-run with --yes to confirm")
    t.equal(summary, "Send 0.5 TCRO from main to 0xabc on Cronos EVM Testnet")
    -- A message without the tail is left exactly as it is.
    t.equal(interactive.plan_summary("Forget account old"), "Forget account old")
    t.equal(interactive.plan_summary(nil), nil)
  end)
end)

t.suite("interactive / export", function()
  t.case("prints the export when no path is given", function()
    local wallet = support.wallet()
    wallet:new_account({ label = "alpha" })
    local _, out = session(wallet, { "6", "1", "n", "", "q" })
    t.contains(out, '"label":"alpha"')
    -- Without asking for them, no secrets.
    t.equal(out:find("private_key"), nil)
  end)

  t.case("includes secrets only when asked", function()
    local wallet = support.wallet()
    wallet:new_account({ label = "alpha" })
    local _, out = session(wallet, { "6", "1", "y", "", "q" })
    t.contains(out, "private_key")
  end)

  t.case("writes to a file when given a path", function()
    local wallet, home = support.wallet()
    wallet:new_account({ label = "alpha" })
    local path = home .. "/wallets.md"
    local _, out = session(wallet, { "6", "4", "n", path, "q" })
    t.contains(out, "wrote ")

    local file = io.open(path, "r")
    t.ok(file, "the export should exist at " .. path)
    local text = file:read("*a")
    file:close()
    t.contains(text, "alpha")
  end)

  t.case("offers every format the wallet supports", function()
    local _, out = session(support.seeded_wallet(), { "6", "q" })
    for _, format in ipairs({ "jsonl", "csv", "txt", "md" }) do
      t.contains(out, format)
    end
  end)

  t.case("reveals one wallet's secrets, after asking twice", function()
    local wallet = support.seeded_wallet()
    local _, out = session(wallet, { "7", "1", "y", "q" })
    t.contains(out, support.PRIVATE_KEY)
    t.contains(out, support.MNEMONIC)
  end)

  t.case("reveals nothing when the answer is no", function()
    local wallet = support.seeded_wallet()
    local _, out = session(wallet, { "7", "1", "n", "q" })
    t.equal(out:find(support.PRIVATE_KEY, 1, true), nil)
  end)
end)

t.suite("interactive / networks", function()
  t.case("switches network and remembers it", function()
    local wallet = support.wallet()
    local _, out = session(wallet, { "9", "2", "q" })
    t.contains(out, "now on Cronos EVM Mainnet")
    t.equal(wallet:current_network().chain_id, 25)
  end)

  t.case("marks the one already in use", function()
    local _, out = session(support.wallet(), { "9", "q" })
    t.contains(out, "(current)")
    t.contains(out, "cronos-testnet")
    t.contains(out, "cronos-mainnet")
    -- Every chain's networks are on offer, not only the current chain's.
    t.contains(out, "solana-devnet")
    t.contains(out, "cardano-preprod")
    t.contains(out, "midnight-preview")
  end)

  t.case("switches chain, and lands on a network of it", function()
    local wallet = support.wallet()
    -- 8 is the chain menu; solana is the second chain the registry reports.
    local _, out = session(wallet, { "8", "2", "q" })
    t.contains(out, "now on Solana")
    t.ok(wallet:current_network().key:match("^solana%-"), "should be on a Solana network")
  end)

  t.case("the chain menu says what each chain can do", function()
    local _, out = session(support.wallet(), { "8", "q" })
    t.contains(out, "evm")
    t.contains(out, "m/44'/501'/0'/0'")
    t.contains(out, "cardano")
    t.contains(out, "midnight")
    -- Capabilities are the reason to pick one chain over another.
    t.contains(out, "faucet")
    t.contains(out, "(current)")
  end)
end)

return true
