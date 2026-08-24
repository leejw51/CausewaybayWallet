--- An interactive prompt for the wallet: a menu, not a full-screen UI.
---
--- `cwbwallet tui` is the full-screen one, and it belongs to the Rust CLI
--- because it takes over the terminal. This is the other shape — a numbered
--- menu, one question at a time, plain lines in and out. It works over a pipe,
--- over SSH, and in a terminal that ratatui would not enjoy, and it is the
--- thing you reach for when you know what you want to do and not what the flag
--- is called.
---
---     cwbwallet-lua interactive
---
--- Everything here goes through the same `Wallet` methods a LÖVE GUI will
--- call, so this doubles as the worked example for that PR: prompt, confirm,
--- act, report.
---
--- ## Confirmation
---
--- The rule the whole FFI is built around shows up here as a feature. A send
--- is attempted first *without* `yes`, which makes the wallet resolve the
--- nonce, the gas price and the gas limit, check the balance covers it, and
--- then refuse with `confirmation_required` — carrying the summary it would
--- have asked a human. That summary is what gets shown. So the confirmation
--- text is the wallet's own, computed from a transaction that is ready to
--- sign, rather than something this file assembled and hoped was accurate.

local echo = require("causewaybay.echo")

local interactive = {}

--- Exit statuses, matching the rest of the CLI.
interactive.EXIT_OK = 0
interactive.EXIT_FAILURE = 1

--- The tail `Headless::confirm` appends, which is CLI advice and reads oddly
--- when the question is right in front of you.
local CONFIRM_SUFFIX = " — re-run with --yes to confirm"

--- The wallet's own summary of what a refused command was about to do.
local function plan_summary(message)
  if not message then return nil end
  local trimmed = message:gsub(CONFIRM_SUFFIX:gsub("%p", "%%%0") .. "$", "")
  return trimmed
end

interactive.plan_summary = plan_summary

-- ------------------------------------------------------------------- prompting

--- Ask a question and return the trimmed answer.
---
--- `nil` means one thing only: the input ended — a closed pipe, a ^D. An empty
--- line is `""`, or the default when there is one. Conflating the two is how
--- "just press enter to print it here" turns into a prompt that silently gives
--- up, so the two are kept apart here and nowhere else has to think about it.
local function ask(ctx, question, default, reader)
  if default and default ~= "" then
    ctx.write(("%s [%s]: "):format(question, default))
  else
    ctx.write(question .. ": ")
  end
  local line = (reader or ctx.read)()
  if line == nil then return nil end
  line = line:gsub("^%s+", ""):gsub("%s+$", "")
  if line == "" then return default or "" end
  return line
end

--- Ask until there is a non-empty answer. nil means the input ended.
local function ask_required(ctx, question, reader)
  while true do
    local answer = ask(ctx, question, nil, reader)
    if answer == nil then return nil end
    if answer ~= "" then return answer end
    ctx.write("  that one is required.\n")
  end
end

--- Ask for something that must not appear on the screen.
---
--- A mnemonic is a password: anyone who reads it over a shoulder, or scrolls
--- back through a shared terminal, has the wallet. `ctx.read_secret` reads with
--- terminal echo off; when there is no terminal to turn it off on, the prompt
--- says so rather than quietly echoing.
local function ask_secret(ctx, question)
  if ctx.hidden then
    ctx.write("  input is hidden.\n")
  else
    ctx.write("  heads up: this terminal cannot hide input, so it will be visible.\n")
  end
  return ask_required(ctx, question, ctx.read_secret)
end

--- A yes/no question, defaulting to no. nil means the input ended.
local function confirm(ctx, question)
  local answer = ask(ctx, question .. " [y/N]")
  if answer == nil then return nil end
  answer = answer:lower()
  return answer == "y" or answer == "yes"
end

--- Offer a numbered list and return the chosen entry.
---
--- Returns the item, or nil when the input ended or the list was empty.
local function choose(ctx, question, items, render)
  if #items == 0 then return nil end
  ctx.write("\n")
  for i, item in ipairs(items) do
    ctx.write(("  %d  %s\n"):format(i, render(item)))
  end
  while true do
    local answer = ask(ctx, question)
    if answer == nil then return nil end
    local index = tonumber(answer)
    if index and items[index] then return items[index] end
    ctx.write("  pick one of the numbers above.\n")
  end
end

interactive.ask = ask
interactive.confirm = confirm
interactive.ask_secret = ask_secret

-- --------------------------------------------------------------------- output

--- Report a failed call. Returns false so an action can `return report(...)`.
local function report(ctx, err)
  ctx.write(("  error [%s]: %s\n"):format(err.code or "internal", err.message or ""))
  return false
end

local function say(ctx, line)
  ctx.write("  " .. line .. "\n")
end

--- The active account's address, for marking it in a list.
---
--- `info` reports the active account by *label*, and the address beside it.
--- The address is the one to compare on: it is the account's identity, where a
--- label is a name that can be changed under you.
local function active_address(ctx)
  local info = ctx.wallet:info()
  return info and info.active_address
end

--- Every account, with a message when there are none rather than an empty list.
local function accounts_or_complain(ctx)
  local accounts, err = ctx.wallet:accounts()
  if not accounts then
    report(ctx, err)
    return nil
  end
  if #accounts == 0 then
    say(ctx, "no wallets yet — create one first.")
    return nil
  end
  return accounts
end

local function describe_account(account, active)
  return ("%-16s %s  %s%s"):format(
    account.label,
    account.address,
    account.source,
    account.address == active and "  (active)" or ""
  )
end

-- -------------------------------------------------------------------- actions

--- Create a wallet: a new address, a new mnemonic, or an import.
local function create_wallet(ctx)
  local how = choose(ctx, "how", {
    { key = "next", label = "another address on this wallet's mnemonic" },
    { key = "seed", label = "a brand new mnemonic" },
    { key = "mnemonic", label = "import a mnemonic I already have" },
    { key = "key", label = "import a private key" },
  }, function(item) return item.label end)
  if not how then return end

  local label = ask(ctx, "label (blank for an automatic one)")
  if label == nil then return end
  local opts = { label = label ~= "" and label or nil }

  local account, err
  if how.key == "next" then
    account, err = ctx.wallet:new_account(opts)
  elseif how.key == "seed" then
    local words = ask(ctx, "word count (12, 15, 18, 21 or 24)", "12")
    if words == nil then return end
    opts.new_seed, opts.words = true, words
    account, err = ctx.wallet:new_account(opts)
  elseif how.key == "mnemonic" then
    local phrase = ask_secret(ctx, "mnemonic")
    if phrase == nil then return end
    local index = ask(ctx, "address index", "0")
    if index == nil then return end
    opts.index = index
    account, err = ctx.wallet:import_mnemonic(phrase, opts)
  else
    local key = ask_secret(ctx, "private key")
    if key == nil then return end
    account, err = ctx.wallet:import_key(key, opts)
  end

  if not account then return report(ctx, err) end
  say(ctx, ("created %s at %s"):format(account.label, account.address))
  if account.mnemonic then say(ctx, "mnemonic: " .. account.mnemonic) end
end

--- List every wallet, marking the active one.
local function list_wallets(ctx)
  local accounts = accounts_or_complain(ctx)
  if not accounts then return end
  local active = active_address(ctx)
  for _, account in ipairs(accounts) do
    say(ctx, describe_account(account, active))
  end
  say(ctx, ("%d wallet%s"):format(#accounts, #accounts == 1 and "" or "s"))
end

--- Choose which wallet later commands act on.
local function select_wallet(ctx)
  local accounts = accounts_or_complain(ctx)
  if not accounts then return end
  local active = active_address(ctx)
  local chosen = choose(ctx, "which wallet", accounts, function(account)
    return describe_account(account, active)
  end)
  if not chosen then return end

  local ok, err = ctx.wallet:use_account(chosen.id)
  if not ok then return report(ctx, err) end
  say(ctx, "active wallet is now " .. chosen.label)
end

--- Read the native balance of the active wallet.
local function show_balance(ctx)
  if not accounts_or_complain(ctx) then return end
  say(ctx, "asking the node…")
  local balance, err = ctx.wallet:balance()
  if not balance then return report(ctx, err) end
  say(ctx, ("%s %s"):format(balance.balance, balance.symbol))
  say(ctx, ("%s on %s"):format(balance.address, balance.network))
end

--- Send native CRO/TCRO, confirming with the wallet's own summary.
local function send_amount(ctx)
  if not accounts_or_complain(ctx) then return end

  local to = ask_required(ctx, "recipient address")
  if to == nil then return end
  local amount = ask_required(ctx, "amount")
  if amount == nil then return end

  -- Deliberately without `yes`: let the wallet resolve gas, check the balance
  -- and refuse, so what gets shown below is a real, funded, ready-to-sign
  -- transaction rather than a guess assembled here.
  say(ctx, "checking the transaction…")
  local sent, err = ctx.wallet:send({ to = to, amount = amount, yes = false })
  if sent then
    -- Only reachable if the wallet was opened with `yes` already set.
    say(ctx, "sent " .. (sent.hash or ""))
    return
  end
  if err.code ~= "confirmation_required" then return report(ctx, err) end

  say(plan_summary(err.message))
  local go = confirm(ctx, "send it")
  if go == nil then return end
  if not go then
    say(ctx, "cancelled — nothing was signed.")
    return
  end

  local receipt, send_err = ctx.wallet:send({ to = to, amount = amount, yes = true })
  if not receipt then return report(ctx, send_err) end
  say(ctx, "sent " .. (receipt.hash or ""))
  if receipt.explorer then say(ctx, receipt.explorer) end
end

--- Write the wallet list out as a file.
local function export_wallets(ctx)
  if not accounts_or_complain(ctx) then return end

  local format = choose(ctx, "format", { "jsonl", "csv", "txt", "md" }, function(f) return f end)
  if not format then return end

  local secret = confirm(ctx, "include private keys and mnemonics")
  if secret == nil then return end
  local path = ask(ctx, "write to (blank to print it here)")
  if path == nil then return end

  local exported, err = ctx.wallet:export_accounts(format, {
    secret = secret or nil,
    output = path ~= "" and path or nil,
  })
  if not exported then return report(ctx, err) end

  if path ~= "" then
    say(ctx, "wrote " .. (exported.path or path))
    if secret then say(ctx, "it holds key material — the file is owner-only.") end
  else
    ctx.write((exported.content or "") .. "\n")
  end
end

--- Reveal one wallet's private key and mnemonic, after asking.
local function reveal_secrets(ctx)
  local accounts = accounts_or_complain(ctx)
  if not accounts then return end
  local active = active_address(ctx)
  local chosen = choose(ctx, "which wallet", accounts, function(account)
    return describe_account(account, active)
  end)
  if not chosen then return end

  local sure = confirm(ctx, "print " .. chosen.label .. "'s private key on screen")
  if sure == nil or not sure then return end

  local exported, err = ctx.wallet:export_account(chosen.id)
  if not exported then return report(ctx, err) end
  say(ctx, "address:     " .. (exported.address or ""))
  say(ctx, "private key: " .. (exported.private_key or ""))
  if exported.mnemonic then
    say(ctx, "mnemonic:    " .. exported.mnemonic)
    say(ctx, "path:        " .. (exported.derivation_path or ""))
  end
end

--- Switch the network later commands use.
local function switch_network(ctx)
  local networks, err = ctx.wallet:networks()
  if not networks then return report(ctx, err) end
  local current = ctx.wallet:current_network()
  local chosen = choose(ctx, "network", networks, function(network)
    -- Only EVM networks have a numeric chain id; the others carry JSON null,
    -- which `%d` cannot render and which is not a fact about them anyway.
    -- What every network has is the chain it belongs to.
    local chain_id = tonumber(network.chain_id)
    local where = chain_id and ("%s %d"):format(network.chain, chain_id) or network.chain
    return ("%-17s %-11s %-5s%s"):format(
      network.key,
      where,
      network.symbol,
      current and network.key == current.key and "  (current)" or ""
    )
  end)
  if not chosen then return end

  local ok, switch_err = ctx.wallet:use_network(chosen.key)
  if not ok then return report(ctx, switch_err) end
  say(ctx, "now on " .. chosen.name)
end

--- Move to another chain, on whichever of its networks the wallet last used.
---
--- The wallet has two axes — the chain and the network within it — and a menu
--- that only offered networks made "work on Solana" a matter of knowing which
--- keys begin with `solana-`. This offers the chains themselves, with what
--- each one can do beside it.
local function switch_chain(ctx)
  local chains, err = ctx.wallet:chains()
  if not chains then return report(ctx, err) end
  local info = ctx.wallet:info()
  local here = info and info.chain

  local chosen = choose(ctx, "chain", chains, function(chain)
    local can = {}
    for name, allowed in pairs(chain.capabilities or {}) do
      if allowed then can[#can + 1] = name end
    end
    table.sort(can)
    return ("%-9s %-22s %s%s"):format(
      chain.chain,
      chain.derivation_path,
      table.concat(can, ", "),
      chain.chain == here and "  (current)" or ""
    )
  end)
  if not chosen then return end

  -- The chain is settled by the network, so moving to a chain means moving to
  -- one of its networks: the one already selected there, or its first.
  local target = chosen.networks and chosen.networks[1]
  for _, held in ipairs((info and info.chains) or {}) do
    if held.chain == chosen.chain and held.network then target = held.network end
  end
  if not target then
    return report(ctx, { code = "usage", message = chosen.chain .. " has no networks" })
  end

  local ok, switch_err = ctx.wallet:use_network(target)
  if not ok then return report(ctx, switch_err) end
  say(ctx, ("now on %s · %s"):format(chosen.name, target))
end

--- The menu. Order is roughly the order a new wallet is used in.
interactive.ACTIONS = {
  { key = "1", label = "create a wallet", run = create_wallet },
  { key = "2", label = "list wallets", run = list_wallets },
  { key = "3", label = "select the active wallet", run = select_wallet },
  { key = "4", label = "balance", run = show_balance },
  { key = "5", label = "send", run = send_amount },
  { key = "6", label = "export wallets to a file", run = export_wallets },
  { key = "7", label = "reveal a wallet's secrets", run = reveal_secrets },
  { key = "8", label = "switch chain", run = switch_chain },
  { key = "9", label = "switch network", run = switch_network },
}

-- ------------------------------------------------------------------ the REPL
--
-- The prompt does double duty. A number runs the menu action beside it; a word
-- is a wallet command, run exactly as `cwbwallet-lua` would run it. Menu
-- entries are numbers and commands begin with a letter, so nothing is
-- ambiguous — and someone who has learned the flags never has to go back
-- through the menu to use them.

--- Split a typed line into argv, the way a shell would.
---
--- Quotes matter here: a label with a space in it, and a message with one, are
--- both ordinary things to type. Returns nil plus a reason for an unbalanced
--- quote rather than guessing where the word ended.
local function split(line)
  local words, current, quote = {}, {}, nil
  -- Tracks a word that exists but is empty, so `--label ""` survives as an
  -- argument the wallet can reject rather than vanishing before it is sent.
  local started = false
  local i = 1
  while i <= #line do
    local c = line:sub(i, i)
    if quote then
      if c == quote then
        quote = nil
      elseif c == "\\" and quote == '"' and i < #line then
        i = i + 1
        current[#current + 1] = line:sub(i, i)
      else
        current[#current + 1] = c
      end
    elseif c == '"' or c == "'" then
      quote, started = c, true
    elseif c == "\\" and i < #line then
      i = i + 1
      current[#current + 1] = line:sub(i, i)
      started = true
    elseif c:match("%s") then
      if started or #current > 0 then
        words[#words + 1] = table.concat(current)
        current, started = {}, false
      end
    else
      current[#current + 1] = c
      started = true
    end
    i = i + 1
  end
  if quote then return nil, "unbalanced " .. quote .. " quote" end
  if started or #current > 0 then words[#words + 1] = table.concat(current) end
  return words
end

interactive.split = split

--- The commands whose `-` argument is key material rather than a message.
local SECRET_FLAGS = {
  ["import-mnemonic"] = "mnemonic",
  ["import-key"] = "private key",
}

--- What a lone `-` in `argv` stands for, asked for rather than piped.
---
--- The CLI reads a pipe here; a prompt has no pipe, so it asks — and asks with
--- echo off when what is wanted is a phrase or a key.
local function resolve_dash(ctx, argv)
  local wants = nil
  for _, word in ipairs(argv) do
    if word == "-" then wants = true end
  end
  if not wants then return nil, false end

  local what = "value"
  for _, word in ipairs(argv) do
    if SECRET_FLAGS[word] then what = SECRET_FLAGS[word] end
  end
  if what == "value" then
    return ask(ctx, "text for `-`"), true
  end
  return ask_secret(ctx, what), true
end

--- Run one typed command, asking rather than refusing when it needs a yes.
---
--- The CLI's answer to `confirmation_required` is "re-run with --yes". In a
--- prompt there is someone right there to ask, so it asks — with the wallet's
--- own summary of what it had already resolved and priced.
local function run_command(ctx, argv)
  local options = {}
  local stdin, wanted = resolve_dash(ctx, argv)
  if wanted then
    if stdin == nil then return end
    options.stdin = stdin
  end

  local envelope, err = ctx.wallet:envelope(argv, options)
  if not envelope then return report(ctx, err) end

  if envelope.ok then
    local text = envelope.human or ""
    if text ~= "" then ctx.write(text .. "\n") end
    return
  end

  local failure = envelope.error or {}
  if failure.code ~= "confirmation_required" then return report(ctx, failure) end

  say(ctx, plan_summary(failure.message))
  local go = confirm(ctx, "go ahead")
  if go == nil then return end
  if not go then
    say(ctx, "cancelled.")
    return
  end

  -- Exactly one call. `call` returns `data, human` when it worked and
  -- `nil, err` when it did not, so both come out of the same invocation —
  -- asking twice would broadcast a confirmed send twice.
  options.yes = true
  local data, human_or_err = ctx.wallet:call(argv, options)
  if not data then return report(ctx, human_or_err) end
  if human_or_err and human_or_err ~= "" then
    ctx.write(human_or_err .. "\n")
  end
end

interactive.run_command = run_command

-- ------------------------------------------------------------------- the loop

local PROMPT = "cwb> "

local function draw_menu(ctx)
  ctx.write("\n")
  for _, action in ipairs(interactive.ACTIONS) do
    ctx.write(("  %s  %s\n"):format(action.key, action.label))
  end
  ctx.write("\n")
  ctx.write("  Pick a number, or type a command — `balance`, `account list`, …\n")
  ctx.write("  help [command]   menu   quit\n\n")
end

--- Run the session until the user quits or the input ends.
---
--- `ctx` needs `wallet`, `write` and `read`; `interactive.run` below builds one
--- from real streams. Returns an exit status rather than calling `os.exit`, so
--- a test can drive it with scripted input and assert on what came out.
function interactive.loop(ctx)
  local header = ctx.wallet:info()
  ctx.write("\n  Causewaybay Wallet — interactive\n")
  if header then
    ctx.write(("  %s · %d wallet%s · %s\n"):format(
      header.network, header.accounts, header.accounts == 1 and "" or "s", header.home))
  end
  ctx.write("  Educational software. Keys are stored unencrypted.\n")

  -- A session with nowhere to hide input is worth saying once, up front,
  -- rather than only at the moment a phrase is about to be typed.
  if ctx.read_secret and not ctx.hidden then
    ctx.write("  Note: this terminal cannot hide typed input.\n")
  end

  local by_key = {}
  for _, action in ipairs(interactive.ACTIONS) do by_key[action.key] = action end

  -- Drawn once. Redrawing before every prompt is right for a menu and wrong
  -- for a REPL, where it would push the last answer off the screen; `menu`
  -- brings it back.
  draw_menu(ctx)

  while true do
    ctx.write(PROMPT)
    local line = ctx.read()

    -- End of input is a quit, not a spin: this has to survive a closed pipe.
    if line == nil then
      ctx.write("\n")
      return interactive.EXIT_OK
    end

    line = line:gsub("^%s+", ""):gsub("%s+$", "")
    local word = line:lower()

    if line == "" then -- luacheck: ignore — a bare Enter is just a new prompt
    elseif word == "q" or word == "quit" or word == "exit" then
      ctx.write("  bye.\n")
      return interactive.EXIT_OK
    elseif word == "menu" or word == "m" then
      draw_menu(ctx)
    elseif by_key[word] then
      -- One bad action must not end the session, and a crash in one is a bug
      -- worth showing rather than swallowing.
      local ok, err = pcall(by_key[word].run, ctx)
      if not ok then
        ctx.write(("  error [internal]: %s\n"):format(tostring(err)))
      end
    else
      local argv, reason = split(line)
      if not argv then
        say(ctx, reason)
      elseif #argv > 0 then
        -- `help` and `?` are the shell's spelling of `--help`, which is what
        -- the wallet understands.
        if argv[1] == "help" or argv[1] == "?" then
          table.remove(argv, 1)
          argv[#argv + 1] = "--help"
        end
        local ok, err = pcall(run_command, ctx, argv)
        if not ok then
          ctx.write(("  error [internal]: %s\n"):format(tostring(err)))
        end
      end
    end
  end
end

--- Build a context from real streams and run the loop.
---
--- `io_streams` is the same shape `cli.run` takes, so the tests drive both the
--- same way. The context carries three things beyond the wallet: how to write,
--- how to read, and how to read something that must not be echoed.
function interactive.run(wallet, io_streams)
  local out = (io_streams and io_streams.stdout) or io.stdout
  local read_line = (io_streams and io_streams.read_line) or function() return io.read("*l") end
  local write = function(text) out:write(text) end

  return interactive.loop({
    wallet = wallet,
    write = write,
    read = read_line,
    hidden = echo.available(),
    read_secret = function() return echo.read_hidden(read_line, write) end,
  })
end

return interactive
