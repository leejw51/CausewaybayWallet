--- Integration tests for the Lua wallet API, over a real store in a temp home.
---
--- These drive the same code a LÖVE GUI will: open once, call methods, read
--- tables. Nothing here needs the network — the chain-facing commands are
--- covered by the Rust suite against a mock node, and repeating that through
--- three layers would only test the mock.

local t = require("tests.runner")
local support = require("tests.support")
local causewaybay = require("causewaybay")

t.suite("wallet / opening", function()
  t.case("reports what it loaded", function()
    local wallet = support.wallet()
    t.ok(wallet:version():match("^%d+%.%d+%.%d+"))
    local described = wallet:describe()
    t.equal(described.name, "causewaybay-wallet")
    t.contains(described.warning, "unencrypted")
  end)

  t.case("the error vocabulary comes from the library, not from Lua", function()
    local codes = support.wallet():codes()
    -- Spot-check the ones this suite branches on, then the shape of the rest.
    for _, code in ipairs({
      "usage", "account_not_found", "confirmation_required", "internal",
    }) do
      t.ok(codes[code], code .. " should be a known code")
    end
    t.equal(codes["code_from_a_future_version"], nil)
  end)

  t.case("a missing library is an error value, not a raise", function()
    local wallet, err = causewaybay.open({ lib = "/no/such/library.so" })
    t.equal(wallet, nil)
    t.equal(err.code, "io_error")
    t.ok(causewaybay.is_error(err))
    t.contains(tostring(err), "[io_error]")
  end)

  t.case("a fresh home starts empty", function()
    local wallet = support.wallet()
    t.equal(#wallet:accounts(), 0)
    t.equal(wallet:info().accounts, 0)
    t.fails_with("no_active_account", wallet:account())
  end)
end)

t.suite("wallet / coverage", function()
  -- The suite that makes "every command is exposed" a fact. It reads the
  -- command list out of the library rather than out of a list kept here, so
  -- adding a command in Rust and forgetting the Lua method fails right here.
  local wallet = support.wallet()
  local commands = assert(wallet:commands())

  t.case("every command the library has is mapped to a method", function()
    local unmapped = {}
    for _, command in ipairs(commands) do
      if causewaybay.COMMANDS[command.name] == nil then
        unmapped[#unmapped + 1] = command.name
      end
    end
    t.equal(table.concat(unmapped, ", "), "", "commands with no entry in COMMANDS")
  end)

  t.case("every mapped method actually exists", function()
    local missing = {}
    for command, methods in pairs(causewaybay.COMMANDS) do
      if methods ~= false then
        if type(methods) == "string" then methods = { methods } end
        for _, name in ipairs(methods) do
          if type(causewaybay.Wallet[name]) ~= "function" then
            missing[#missing + 1] = command .. " -> " .. name
          end
        end
      end
    end
    t.equal(table.concat(missing, ", "), "", "COMMANDS names a method that is not defined")
  end)

  t.case("nothing is mapped that the library does not have", function()
    local known = {}
    for _, command in ipairs(commands) do known[command.name] = true end
    local stale = {}
    for command in pairs(causewaybay.COMMANDS) do
      if not known[command] then stale[#stale + 1] = command end
    end
    t.equal(table.concat(stale, ", "), "", "COMMANDS names a command that is gone")
  end)

  t.case("only the terminal UI is left unexposed", function()
    -- If this ever grows, the reason belongs next to it in COMMANDS.
    local unexposed = {}
    for command, methods in pairs(causewaybay.COMMANDS) do
      if methods == false then unexposed[#unexposed + 1] = command end
    end
    t.equal(table.concat(unexposed, ", "), "tui")
  end)

  t.case("the reported arguments describe what the wallet takes", function()
    local send
    for _, command in ipairs(commands) do
      if command.name == "send" then send = command end
    end
    t.ok(send, "send should be a command")
    local by_name = {}
    for _, arg in ipairs(send.args) do by_name[arg.name] = arg end
    t.equal(by_name.to.required, true)
    t.equal(by_name.to.takes_value, true)
    t.equal(by_name.wait.takes_value, false, "--wait is a flag")
    t.equal(by_name.amount.long, "amount")
  end)
end)

t.suite("wallet / accounts", function()
  t.case("creates an account and finds it again", function()
    local wallet = support.wallet()
    local made = wallet:new_account({ label = "alpha" })
    t.equal(made.label, "alpha")
    t.equal(made.source, "mnemonic")
    t.equal(made.derivation_path, "m/44'/60'/0'/0/0")
    t.ok(made.address:match("^0x%x40$") or #made.address == 42)

    t.equal(#wallet:accounts(), 1)
    t.equal(wallet:account("alpha").address, made.address)
    -- With no selector it is the active account, which is the only one.
    t.equal(wallet:account().address, made.address)
  end)

  t.case("imports the reference mnemonic at the reference address", function()
    local wallet = support.wallet()
    local account = wallet:import_mnemonic(support.MNEMONIC, { label = "main" })
    t.equal(account.address, support.ADDRESS_0)

    local second = wallet:import_mnemonic(support.MNEMONIC, { label = "second", index = 1 })
    t.equal(second.address, support.ADDRESS_1)
  end)

  t.case("imports a raw private key", function()
    local wallet = support.wallet()
    local account = wallet:import_key(support.PRIVATE_KEY, { label = "raw" })
    t.equal(account.address, support.ADDRESS_0)
    t.equal(account.source, "private_key")
  end)

  t.case("secrets stay hidden unless asked for", function()
    local wallet = support.seeded_wallet()
    t.equal(wallet:account("main").private_key, nil)
    t.equal(wallet:account("main", { secret = true }).private_key, support.PRIVATE_KEY)
  end)

  t.case("switches the active account", function()
    local wallet = support.wallet()
    wallet:new_account({ label = "one" })
    local two = wallet:new_account({ label = "two" })
    wallet:use_account("two")
    t.equal(wallet:account().address, two.address)
  end)

  t.case("removes an account", function()
    local wallet = support.wallet()
    wallet:new_account({ label = "doomed" })
    t.ok(wallet:remove_account("doomed"))
    t.equal(#wallet:accounts(), 0)
  end)

  t.case("rejects a duplicate label", function()
    local wallet = support.wallet()
    wallet:new_account({ label = "same" })
    t.fails_with("duplicate_label", wallet:new_account({ label = "same", new_seed = true }))
  end)

  t.case("rejects a bad mnemonic and a bad key", function()
    local wallet = support.wallet()
    t.fails_with("invalid_mnemonic", wallet:import_mnemonic("not a real phrase at all"))
    t.fails_with("invalid_private_key", wallet:import_key("0x1234"))
  end)

  t.case("names a missing account rather than guessing", function()
    local wallet = support.seeded_wallet()
    local account, err = wallet:account("ghost")
    t.fails_with("account_not_found", account, err)
    t.contains(err.message, "ghost")
  end)
end)

t.suite("wallet / confirmation", function()
  t.case("a destructive call is refused without yes", function()
    -- The safety property the FFI exists to preserve: a library cannot prompt,
    -- so it must refuse rather than assume.
    local wallet = support.wallet({ yes = false })
    wallet:new_account({ label = "doomed" })
    local removed, err = wallet:remove_account("doomed")
    t.fails_with("confirmation_required", removed, err)
    t.equal(#wallet:accounts(), 1, "the account survived the refusal")
  end)

  t.case("a per-call yes is enough", function()
    local wallet = support.wallet({ yes = false })
    wallet:new_account({ label = "doomed" })
    t.ok(wallet:remove_account("doomed", { yes = true }))
    t.equal(#wallet:accounts(), 0)
  end)
end)

t.suite("wallet / networks", function()
  t.case("lists every chain's networks, not just the EVM ones", function()
    local wallet = support.wallet()
    local networks = wallet:networks()

    local by_key, chains = {}, {}
    for _, n in ipairs(networks) do
      by_key[n.key] = n
      chains[n.chain] = (chains[n.chain] or 0) + 1
    end

    -- The EVM pair keeps its chain ids; the other chains have none, and
    -- say so with null rather than with a number that would mean something.
    t.equal(by_key["cronos-testnet"].chain_id, 338)
    t.equal(by_key["cronos-mainnet"].chain_id, 25)
    t.equal(tonumber(by_key["solana-devnet"].chain_id), nil)

    -- Every chain the library reports contributes at least one network, so a
    -- chain added in Rust cannot quietly go missing from this list.
    for _, chain in ipairs(wallet:chains()) do
      t.ok(chains[chain.chain], chain.chain .. " has no network in the list")
    end
    t.equal(#networks >= 4, true)
  end)

  t.case("every chain describes itself: path, networks, capabilities", function()
    local wallet = support.wallet()
    local chains = wallet:chains()

    local by_name = {}
    for _, c in ipairs(chains) do by_name[c.chain] = c end
    for _, name in ipairs({ "evm", "solana", "cardano", "midnight", "ecash" }) do
      local chain = by_name[name]
      t.ok(chain, name .. " is missing from the chain list")
      -- Cardano derives on CIP-1852, not BIP-44 — a chain describes its own
      -- path rather than being assumed into someone else's.
      t.ok(chain.derivation_path:match("^m/%d+'"), name .. " has no derivation path")
      t.ok(#chain.networks > 0, name .. " has no networks")
    end
    t.equal(by_name["solana"].derivation_path, "m/44'/501'/0'/0'")
    t.equal(by_name["cardano"].derivation_path, "m/1852'/1815'/0'/0/0")
    -- Capabilities are per chain, and are what a GUI should grey a button on.
    t.equal(by_name["evm"].capabilities.tokens, true)
    t.equal(by_name["solana"].capabilities.faucet, true)
    t.equal(by_name["cardano"].capabilities.tokens, false)

    -- The command surface reports the same chains as the handshake.
    local listed = {}
    for _, c in ipairs(wallet:chain_list()) do listed[c.chain] = true end
    for name in pairs(by_name) do
      t.ok(listed[name], name .. " is missing from `chains`")
    end
  end)

  t.case("defaults to testnet and can be switched", function()
    local wallet = support.wallet()
    t.equal(wallet:current_network().chain_id, 338)
    wallet:use_network("mainnet")
    t.equal(wallet:current_network().chain_id, 25)
  end)

  t.case("a per-call network does not change the stored one", function()
    local wallet = support.wallet()
    t.equal(wallet:call({ "network", "current" }, { network = "mainnet" }).chain_id, 25)
    t.equal(wallet:current_network().chain_id, 338)
  end)

  t.case("an unknown network is named in the error", function()
    local wallet = support.wallet()
    local switched, err = wallet:use_network("ethereum")
    t.fails_with("unknown_network", switched, err)
    t.contains(err.message, "ethereum")
  end)
end)

t.suite("wallet / signing", function()
  t.case("signs and verifies a message", function()
    local wallet = support.seeded_wallet()
    local signed = wallet:sign("hello causewaybay")
    t.equal(signed.address, support.ADDRESS_0)
    t.ok(signed.signature:match("^0x%x+$"))

    local verified = wallet:verify("hello causewaybay", signed.signature, support.ADDRESS_0)
    t.equal(verified.valid, true)
  end)

  t.case("a tampered message does not verify", function()
    local wallet = support.seeded_wallet()
    local signed = wallet:sign("original")
    t.equal(wallet:verify("tampered", signed.signature, support.ADDRESS_0).valid, false)
  end)

  t.case("signs a message that would otherwise look like a flag", function()
    -- The message goes through `stdin`, so a leading dash is text, not argv.
    local wallet = support.seeded_wallet()
    local signed = wallet:sign("--not-a-flag")
    t.equal(wallet:verify("--not-a-flag", signed.signature, support.ADDRESS_0).valid, true)
  end)

  t.case("signs an empty message and a unicode one", function()
    local wallet = support.seeded_wallet()
    for _, message in ipairs({ "", "héllo 🌏" }) do
      local signed = wallet:sign(message)
      t.equal(wallet:verify(message, signed.signature, support.ADDRESS_0).valid, true)
    end
  end)
end)

t.suite("wallet / offline utilities", function()
  t.case("hashes, checksums and converts", function()
    local wallet = support.wallet()
    t.equal(
      wallet:keccak("hello").keccak256,
      "0x1c8aff950685c2ed4bc3174f3472287b56d9517b9c948127319a09a7a36deac8"
    )
    t.equal(
      wallet:checksum("0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed").address,
      "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed"
    )
    t.equal(wallet:to_wei("1.5").value, "1500000000000000000")
    t.equal(wallet:from_wei("1500000000000000000").amount, "1.5")
    t.equal(wallet:to_wei("1.5", 6).value, "1500000")
  end)

  t.case("generates a mnemonic without storing it", function()
    local wallet = support.wallet()
    local generated = wallet:new_mnemonic(24)
    local words = 0
    for _ in generated.mnemonic:gmatch("%S+") do words = words + 1 end
    t.equal(words, 24)
    t.equal(#wallet:accounts(), 0, "utils must not touch the store")
  end)
end)

t.suite("wallet / crypto", function()
  -- The parts a caller can use without a wallet in any meaningful sense: no
  -- network, no account, nothing written. A LÖVE game reaches for these.
  local wallet = support.wallet()

  t.case("derives from a mnemonic without storing anything", function()
    local derived = wallet:derive({ mnemonic = support.MNEMONIC })
    t.equal(derived.address, support.ADDRESS_0)
    t.equal(derived.private_key, support.PRIVATE_KEY)
    t.equal(derived.derivation_path, "m/44'/60'/0'/0/0")
    t.equal(derived.source, "mnemonic")
    -- The point of it: the wallet is untouched.
    t.equal(#wallet:accounts(), 0)
    t.equal(#wallet:recent(), 0, "nothing was remembered either")
  end)

  t.case("derives at an index", function()
    t.equal(wallet:derive({ mnemonic = support.MNEMONIC, index = 1 }).address,
      support.ADDRESS_1)
  end)

  t.case("derives from a private key", function()
    local derived = wallet:derive({ private_key = support.PRIVATE_KEY })
    t.equal(derived.address, support.ADDRESS_0)
    t.equal(derived.source, "private_key")
    t.equal(derived.derivation_path, nil, "a raw key has no path")
  end)

  t.case("gives both public key encodings", function()
    local derived = wallet:derive({ private_key = support.PRIVATE_KEY })
    t.equal(#derived.public_key, 2 + 128)
    t.equal(#derived.public_key_compressed, 2 + 66)
    -- The compressed form carries the same X coordinate.
    t.equal(derived.public_key_compressed:sub(5), derived.public_key:sub(3, 66))
  end)

  t.case("insists on exactly one source", function()
    t.fails_with("usage", wallet:derive({}))
    t.fails_with("usage", wallet:derive({
      mnemonic = support.MNEMONIC, private_key = support.PRIVATE_KEY,
    }))
  end)

  t.case("signs with a key the wallet does not hold", function()
    local signed = wallet:sign_with(support.PRIVATE_KEY, "hello")
    t.equal(signed.address, support.ADDRESS_0)
    t.equal(#wallet:accounts(), 0, "still no account")
    t.equal(wallet:verify("hello", signed.signature, support.ADDRESS_0).valid, true)
  end)

  t.case("validates a mnemonic instead of refusing it", function()
    local good = wallet:validate_mnemonic(support.MNEMONIC)
    t.equal(good.valid, true)
    t.equal(good.words, 12)

    -- The difference from import_mnemonic: a bad phrase is a value here.
    local bad, err = wallet:validate_mnemonic("abandon abandon")
    t.ok(bad, err and err.message)
    t.equal(bad.valid, false)
    t.equal(bad.words, 2)
    t.ok(#bad.reason > 0)
  end)

  t.case("verify recovers the signer when no address is given", function()
    local signed = wallet:sign_with(support.PRIVATE_KEY, "who signed this")
    local recovered = wallet:verify("who signed this", signed.signature)
    t.equal(recovered.recovered, support.ADDRESS_0)
    t.equal(recovered.valid, true)
  end)

  t.case("crypto() binds every function it names", function()
    local crypto = wallet:crypto()
    for name, method in pairs(causewaybay.CRYPTO) do
      t.equal(type(crypto[name]), "function", name .. " should be bound")
      t.equal(type(causewaybay.Wallet[method]), "function",
        "CRYPTO names " .. method .. ", which must exist")
    end
  end)

  t.case("the bound functions work without naming the wallet", function()
    local crypto = wallet:crypto()
    t.equal(crypto.keccak("hello").keccak256,
      "0x1c8aff950685c2ed4bc3174f3472287b56d9517b9c948127319a09a7a36deac8")
    t.equal(crypto.checksum("0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed").address,
      "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed")
    t.equal(crypto.to_wei("1.5").value, "1500000000000000000")
    t.equal(crypto.derive({ mnemonic = support.MNEMONIC }).address, support.ADDRESS_0)

    local signed = crypto.sign(support.PRIVATE_KEY, "gg")
    t.equal(crypto.verify("gg", signed.signature).recovered, support.ADDRESS_0)
    t.equal(crypto.validate_mnemonic(support.MNEMONIC).valid, true)
    t.equal(#crypto.new_mnemonic(24).mnemonic:gsub("%S+", ""), 23, "24 words, 23 gaps")
  end)
end)

t.suite("wallet / the raw call", function()
  t.case("passes an arbitrary command through", function()
    local wallet = support.wallet()
    local data, human = wallet:call({ "info" })
    t.ok(data.home)
    t.contains(human, "Network")
  end)

  t.case("returns the envelope when asked", function()
    local wallet = support.wallet()
    local envelope = wallet:envelope({ "account", "show", "ghost" })
    t.equal(envelope.ok, false)
    t.equal(envelope.error.code, "account_not_found")
  end)

  t.case("catches an argv that is not a list of strings", function()
    -- The mistake a GUI makes first: passing a number straight through.
    local wallet = support.wallet()
    t.fails_with("usage", wallet:call({ "utils", "to-wei", 1.5 }))
    t.fails_with("usage", wallet:call("account list"))
  end)

  t.case("an empty argv is help, not a crash", function()
    local wallet = support.wallet()
    local _, human = wallet:call({})
    t.contains(human, "Usage:")
  end)

  t.case("two wallets see one store", function()
    -- What a GUI and a CLI open at the same time actually do.
    local first, home = support.wallet()
    first:new_account({ label = "shared" })
    local second = support.wallet({ home = home })
    t.equal(second:account("shared").address, first:account("shared").address)
  end)
end)

return true
