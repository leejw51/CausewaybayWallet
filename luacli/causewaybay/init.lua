--- Causewaybay Wallet for Lua.
---
--- ⚠️  EDUCATIONAL SOFTWARE. Keys are stored unencrypted on disk.
---
--- A thin, typed-feeling layer over the wallet's C ABI. The Rust core does the
--- cryptography, the storage and the RPC; this turns its JSON envelopes into
--- Lua tables and its error codes into values you can branch on.
---
---     local causewaybay = require("causewaybay")
---     local wallet = assert(causewaybay.open())
---
---     local accounts = assert(wallet:accounts())
---     for _, a in ipairs(accounts) do print(a.label, a.address) end
---
---     local balance, err = wallet:balance()
---     if not balance then print(err.code, err.message) end
---
--- Every call returns `value` or `nil, err`, where `err` has `.code` (one of
--- the stable strings in SPEC.md) and `.message`. Nothing raises for an
--- ordinary wallet failure — a missing account is a value, not an exception.
--- Add `assert(...)` where you would rather it did.
---
--- The same module backs the Lua CLI and the LÖVE GUI: a GUI holds one
--- `Wallet` for the life of the program, sets `yes = true` once its own
--- confirmation dialog is wired up, and never blocks on a prompt.

local binding = require("causewaybay.ffi")
local json = require("causewaybay.json")

local M = {}

M.json = json

-- There is no version constant here on purpose. The version belongs to the
-- library this loads, and a second copy of it in Lua is a copy that goes stale
-- without anything noticing. Ask the wallet: `wallet:version()`.

-- --------------------------------------------------------------------- errors

local Error = {}
Error.__index = Error
Error.__tostring = function(self)
  return ("[%s] %s"):format(self.code, self.message)
end

--- Build an error value. Kept as a table so `err.code` works even for the
--- failures that never reached the Rust side at all.
local function make_error(code, message)
  return setmetatable({ code = code, message = message }, Error)
end

M.error = make_error
M.Error = Error

--- True when `value` is one of this module's error tables.
function M.is_error(value)
  return getmetatable(value) == Error
end

-- --------------------------------------------------------------------- wallet

local Wallet = {}
Wallet.__index = Wallet
M.Wallet = Wallet

--- Open a wallet.
---
--- `options`:
---   `home`     wallet directory; default `$CAUSEWAYBAY_HOME` or `~/.causewaybaywallet`
---   `network`  default network for every call ("testnet", "cronos-mainnet", …)
---   `yes`      answer confirmations with yes. A GUI sets this once it shows
---              its own dialog; a CLI passes the user's `--yes`.
---   `lib`      an explicit path to the shared library, skipping the search
---
--- Returns `wallet` or `nil, err`. The only failure here is not finding the
--- library, which is a setup problem rather than a wallet one.
function M.open(options)
  options = options or {}
  local lib, err = binding.load(options.lib)
  if not lib then return nil, make_error("io_error", err) end

  return setmetatable({
    lib = lib,
    home = options.home,
    network = options.network,
    yes = options.yes and true or false,
  }, Wallet)
end

--- The wallet version the loaded library reports.
function Wallet:version()
  return binding.version(self.lib)
end

--- The library's handshake: name, version, ABI, networks, and error codes.
function Wallet:describe()
  local envelope, err = json.try_decode(binding.describe(self.lib))
  if not envelope then return nil, make_error("internal", err) end
  return envelope.data
end

--- Every command the library accepts: `{path, name, about, args}` each.
---
--- Read from the library, so a GUI can build its panels from `args` instead of
--- hardcoding a form per command — and so the test suite can check this
--- module's coverage against what actually exists.
function Wallet:commands()
  local envelope, err = json.try_decode(binding.commands(self.lib))
  if not envelope then return nil, make_error("internal", err) end
  return envelope.data
end

--- Every error code the wallet can return, as a set.
---
--- Read from the library rather than listed here, so a `code` a caller
--- branches on can be checked against what this build actually emits — and so
--- there is no second copy to go stale.
function Wallet:codes()
  local described, err = self:describe()
  if not described then return nil, err end
  local codes = {}
  for _, code in ipairs(described.codes or {}) do codes[code] = true end
  return codes
end

--- Run one command and return the whole envelope table.
---
--- `argv` is the command as a list — `{"account", "list"}` — without the
--- program name. `options` may carry `stdin` (what an argument of `-` means),
--- and per-call `home`, `network` and `yes` overrides.
---
--- Returns the envelope, or `nil, err` if the reply itself was unusable. A
--- command that *failed* still returns an envelope, with `ok = false`.
function Wallet:envelope(argv, options)
  options = options or {}
  if type(argv) ~= "table" then
    return nil, make_error("usage", "argv must be a list of strings")
  end

  local request = { argv = {} }
  for i, word in ipairs(argv) do
    if type(word) ~= "string" then
      -- Numbers are the easy mistake — `{"utils", "to-wei", 1.5}` would
      -- otherwise reach the wallet as "1.5" only by luck of formatting.
      return nil, make_error("usage",
        ("argv[%d] is a %s; every argument must be a string"):format(i, type(word)))
    end
    request.argv[i] = word
  end
  if #request.argv == 0 then request.argv = json.empty_array end

  local home = options.home or self.home
  local network = options.network or self.network
  local yes = options.yes
  if yes == nil then yes = self.yes end

  if home then request.home = home end
  if network then request.network = network end
  if yes then request.yes = true end
  if options.stdin then request.stdin = options.stdin end

  local reply = binding.execute(self.lib, json.encode(request))
  if not reply then
    return nil, make_error("internal", "the wallet returned nothing")
  end

  local envelope, decode_err = json.try_decode(reply)
  if not envelope then
    return nil, make_error("internal", "the reply was not JSON: " .. tostring(decode_err))
  end
  return envelope
end

--- Run one command and return its `data`.
---
--- The call every caller wants: `wallet:call{"account", "list"}` is either a
--- table of accounts or `nil` plus an error with a code to branch on.
function Wallet:call(argv, options)
  local envelope, err = self:envelope(argv, options)
  if not envelope then return nil, err end
  if envelope.ok then
    return envelope.data, envelope.human
  end
  local e = envelope.error or {}
  return nil, make_error(e.code or "internal", e.message or "the wallet gave no reason")
end

--- Run one command and return the text a CLI would have printed.
---
--- Used by the Lua CLI, so its output matches the Rust CLI's exactly without
--- either one knowing the other's formatting rules.
function Wallet:text(argv, options)
  local envelope, err = self:envelope(argv, options)
  if not envelope then return nil, err end
  if envelope.ok then return envelope.human or "" end
  local e = envelope.error or {}
  return nil, make_error(e.code or "internal", e.message or "the wallet gave no reason")
end

-- ------------------------------------------------------------ the whole surface
--
-- One method per command the wallet has, so a GUI's button handler reads as
-- what it does and a typo is a Lua error at the call site rather than a
-- `usage` envelope at runtime. `M.COMMANDS` below maps each command to the
-- method that covers it, and the test suite checks that map against the list
-- the library itself reports — so "every command is exposed" is a fact the
-- build enforces, not a promise this comment makes.
--
-- Anything here can still be reached the long way with `wallet:call{...}`.

--- Append `--flag value` pairs to an argv, skipping the ones left nil.
---
--- A `true` value means a flag that stands alone (`--secret`); anything else
--- is rendered as its own word after the flag. Underscores become dashes, so
--- Lua's `gas_price_gwei` reaches the wallet as `--gas-price-gwei`.
local function with_flags(argv, flags, order)
  for _, name in ipairs(order) do
    local value = flags[name]
    if value ~= nil and value ~= false then
      argv[#argv + 1] = "--" .. name:gsub("_", "-")
      if value ~= true then argv[#argv + 1] = tostring(value) end
    end
  end
  return argv
end

M.with_flags = with_flags

--- Append a positional argument, when there is one to append.
local function with_positional(argv, value)
  if value ~= nil then argv[#argv + 1] = tostring(value) end
  return argv
end

M.with_positional = with_positional

-- ----------------------------------------------------------------- the wallet

--- Where state lives and what is configured.
function Wallet:info()
  return self:call({ "info" })
end

-- -------------------------------------------------------------------- accounts

--- Every account, oldest first.
---
--- `opts.secret` includes private keys and mnemonics; `opts.format`
--- (`jsonl`, `csv`, `txt`, `md`) renders the list as a file instead, and
--- `opts.output` writes it to a path rather than returning its text.
function Wallet:accounts(opts)
  opts = opts or {}
  return self:call(with_flags({ "account", "list" }, opts, { "format", "output", "secret" }))
end

--- The account list rendered as a file format: `jsonl`, `csv`, `txt` or `md`.
---
--- Returns `data.content` when no `opts.output` path is given, so a GUI can put
--- it straight in a text box. With `opts.secret` the file carries private keys
--- and mnemonics, and is written owner-only.
function Wallet:export_accounts(format, opts)
  opts = opts or {}
  opts.format = format
  return self:accounts(opts)
end

--- One account. `selector` is an id, a label or an address; nil means active.
function Wallet:account(selector, opts)
  opts = opts or {}
  local argv = with_positional({ "account", "show" }, selector)
  return self:call(with_flags(argv, opts, { "secret" }))
end

--- Add the next address of the wallet's mnemonic.
---
--- A wallet holds one mnemonic and many addresses derived from it, so this
--- continues the sequence. `opts.new_seed` starts a separate mnemonic instead.
function Wallet:new_account(opts)
  opts = opts or {}
  return self:call(with_flags({ "account", "new" }, opts,
    { "label", "new_seed", "words", "index", "show_secret" }))
end

--- Import a BIP-39 mnemonic. Pass the phrase itself, not `-`.
function Wallet:import_mnemonic(mnemonic, opts)
  opts = opts or {}
  local argv = { "account", "import-mnemonic", "--mnemonic", mnemonic }
  return self:call(with_flags(argv, opts, { "index", "label", "passphrase" }))
end

--- Import a raw private key.
function Wallet:import_key(private_key, opts)
  opts = opts or {}
  local argv = { "account", "import-key", "--private-key", private_key }
  return self:call(with_flags(argv, opts, { "label" }))
end

--- Derive another address from an existing mnemonic account.
---
--- `index` is the BIP-44 address index; `opts.from` picks which mnemonic
--- account to derive from, defaulting to the active one.
function Wallet:derive_account(index, opts)
  opts = opts or {}
  opts.index = index
  return self:call(with_flags({ "account", "derive" }, opts, { "index", "label", "from" }))
end

--- Make an account the default for later calls.
function Wallet:use_account(selector)
  return self:call({ "account", "use", selector })
end

--- Change an account's label.
function Wallet:rename_account(selector, label)
  return self:call({ "account", "rename", selector, label })
end

--- Forget an account. Needs `yes`, on the wallet or on the call.
function Wallet:remove_account(selector, opts)
  return self:call({ "account", "remove", selector }, opts)
end

--- Print an account's secrets: the private key, and the mnemonic if it has one.
---
--- Unlike `account(selector, {secret = true})` this always reveals, which is
--- why it is a separate call rather than a flag — a GUI should be able to grep
--- its own source for the places that show a key.
function Wallet:export_account(selector)
  return self:call(with_positional({ "account", "export" }, selector))
end

--- Create an account from key material the wallet already remembers.
---
--- `selector` is a recall id, a 1-based position, or an address; nil means the
--- newest entry.
function Wallet:import_recent(selector, opts)
  opts = opts or {}
  local argv = with_positional({ "account", "import-recent" }, selector)
  return self:call(with_flags(argv, opts, { "index", "label", "passphrase" }))
end

-- ---------------------------------------------------------------------- recall

--- Remembered mnemonics and private keys, most recently used first.
---
--- `opts.kind` narrows to `"mnemonic"` or `"private-key"`; `opts.limit` caps
--- the list. Previews identify an entry without revealing it.
function Wallet:recent(opts)
  opts = opts or {}
  return self:call(with_flags({ "recent", "list" }, opts, { "kind", "limit" }))
end

--- One remembered entry. `opts.secret` reveals the mnemonic or private key.
function Wallet:recent_entry(selector, opts)
  opts = opts or {}
  local argv = with_positional({ "recent", "show" }, selector)
  return self:call(with_flags(argv, opts, { "secret" }))
end

--- Drop one remembered entry. Needs `yes`.
function Wallet:forget_recent(selector, opts)
  return self:call({ "recent", "forget", selector }, opts)
end

--- Drop every remembered entry. Needs `yes`.
function Wallet:clear_recent(opts)
  return self:call({ "recent", "clear" }, opts)
end

-- -------------------------------------------------------------------- networks

--- The supported networks.
function Wallet:networks()
  return self:call({ "network", "list" })
end

--- The selected network.
function Wallet:current_network()
  return self:call({ "network", "current" })
end

--- Change the stored default network.
function Wallet:use_network(key)
  return self:call({ "network", "use", key })
end

--- Override a network's RPC URL. An empty URL restores the default.
function Wallet:set_rpc(network, url)
  return self:call({ "network", "set-rpc", network, url or "" })
end

-- ----------------------------------------------------------------------- chain

--- The native token balance.
function Wallet:balance(opts)
  opts = opts or {}
  return self:call(with_flags({ "balance" }, opts, { "address", "account" }), opts)
end

--- The next transaction nonce.
function Wallet:nonce(opts)
  opts = opts or {}
  return self:call(with_flags({ "nonce" }, opts, { "address", "account" }), opts)
end

--- The gas price the node reports.
function Wallet:gas_price(opts)
  return self:call({ "gas-price" }, opts)
end

--- Network, chain id and latest block, as the node sees them.
function Wallet:chain_info(opts)
  return self:call({ "chain-info" }, opts)
end

--- Send native CRO/TCRO. Requires `opts.to` and `opts.amount`; needs `yes`.
---
--- Everything else has a sensible default: the gas limit is estimated, the gas
--- price comes from the node, and the nonce is the account's pending one.
function Wallet:send(opts)
  opts = opts or {}
  return self:call(with_flags({ "send" }, opts,
    { "to", "amount", "gas_limit", "gas_price_gwei", "nonce", "data", "wait", "account" }), opts)
end

--- Look a transaction up on chain.
function Wallet:tx(hash, opts)
  return self:call({ "tx", hash }, opts)
end

--- Transactions this wallet has sent. Local, so it needs no node.
function Wallet:history(opts)
  opts = opts or {}
  return self:call(with_flags({ "history" }, opts, { "limit", "network" }))
end

-- --------------------------------------------------------------------- signing

--- Sign a message with EIP-191.
function Wallet:sign(message, opts)
  opts = opts or {}
  -- The message goes through `stdin` rather than argv so a leading `-` or a
  -- newline inside it cannot be read as a flag.
  local argv = with_flags({ "sign", "-" }, opts, { "account" })
  return self:call(argv, { stdin = message })
end

--- Verify an EIP-191 signature. With no `address`, recovers the signer.
function Wallet:verify(message, signature, address)
  local argv = { "verify", "--message", "-", "--signature", signature }
  if address then
    argv[#argv + 1] = "--address"
    argv[#argv + 1] = address
  end
  return self:call(argv, { stdin = message })
end

-- ---------------------------------------------------------------------- erc-20

--- A token's name, symbol, decimals and total supply.
function Wallet:token_info(token, opts)
  return self:call({ "erc20", "info", "--token", token }, opts)
end

--- A token balance. `opts.address` checks somewhere other than the active account.
function Wallet:token_balance(token, opts)
  opts = opts or {}
  local argv = with_flags({ "erc20", "balance", "--token", token }, opts, { "address" })
  return self:call(argv, opts)
end

--- Transfer tokens. Requires `opts.token`, `opts.to`, `opts.amount`; needs `yes`.
function Wallet:token_send(opts)
  opts = opts or {}
  return self:call(with_flags({ "erc20", "send" }, opts,
    { "token", "to", "amount", "wait", "account" }), opts)
end

-- ------------------------------------------------------------------- utilities

--- keccak256 of a UTF-8 string, or of hex bytes with `opts.hex`.
function Wallet:keccak(input, opts)
  opts = opts or {}
  return self:call(with_flags({ "utils", "keccak", input }, opts, { "hex" }))
end

--- Apply the EIP-55 checksum to an address.
function Wallet:checksum(address)
  return self:call({ "utils", "checksum", address })
end

--- A decimal amount as its smallest unit. `decimals` defaults to 18.
function Wallet:to_wei(amount, decimals)
  local argv = { "utils", "to-wei", tostring(amount) }
  return self:call(with_flags(argv, { decimals = decimals }, { "decimals" }))
end

--- A smallest-unit integer as a decimal amount. `decimals` defaults to 18.
function Wallet:from_wei(value, decimals)
  local argv = { "utils", "from-wei", tostring(value) }
  return self:call(with_flags(argv, { decimals = decimals }, { "decimals" }))
end

--- Generate a mnemonic without storing it.
function Wallet:new_mnemonic(words)
  return self:call(with_flags({ "utils", "new-mnemonic" }, { words = words }, { "words" }))
end

--- Derive an address and keys from a mnemonic or a private key.
---
--- Nothing is stored and nothing is remembered — this is the calculator, not
--- the wallet. Pass `mnemonic` (with an optional `index` and `passphrase`) or
--- `private_key`, and get back `address`, `private_key`, `public_key` and
--- `public_key_compressed`.
---
---     wallet:derive{ mnemonic = phrase, index = 3 }
---     wallet:derive{ private_key = key }
function Wallet:derive(opts)
  opts = opts or {}
  if (opts.mnemonic == nil) == (opts.private_key == nil) then
    return nil, make_error("usage", "pass exactly one of mnemonic or private_key")
  end
  return self:call(with_flags({ "utils", "derive" }, opts,
    { "mnemonic", "private_key", "index", "passphrase" }))
end

--- Sign a message with a private key the wallet does not hold.
---
--- `sign` uses a stored account; this takes the key itself, for a caller with
--- its own. Both the key and the message travel as arguments rather than
--- through the store, so nothing is written anywhere.
function Wallet:sign_with(private_key, message)
  return self:call({ "utils", "sign", "--private-key", private_key, "--message", "-" },
    { stdin = message })
end

--- Check whether a phrase is a valid BIP-39 mnemonic.
---
--- An invalid phrase is an answer here, not an error: this returns
--- `{valid = false, words = n, reason = "…"}` where `import_mnemonic` would
--- fail with `invalid_mnemonic`.
function Wallet:validate_mnemonic(phrase)
  return self:call({ "utils", "validate-mnemonic", "-" }, { stdin = phrase })
end

-- ------------------------------------------------------------------- crypto

--- The calls that are pure: no network, no account, nothing written.
---
--- Named here so `wallet:crypto()` can be built from the list rather than from
--- a second copy of it, and so a caller can see at a glance which parts of the
--- API are safe to reach for from a draw loop.
M.CRYPTO = {
  keccak = "keccak",
  checksum = "checksum",
  to_wei = "to_wei",
  from_wei = "from_wei",
  new_mnemonic = "new_mnemonic",
  derive = "derive",
  sign = "sign_with",
  verify = "verify",
  validate_mnemonic = "validate_mnemonic",
}

--- The crypto functions, bound to this wallet.
---
--- A convenience for code that does cryptography and nothing else — a LÖVE
--- game hashing an identifier, checking an address a player pasted in, or
--- deriving a throwaway key:
---
---     local crypto = wallet:crypto()
---     print(crypto.keccak("hello").keccak256)
---     print(crypto.derive{ mnemonic = phrase }.address)
---     local signed = crypto.sign(key, "gg")
---     print(crypto.verify("gg", signed.signature).recovered)
---
--- These still reach the wallet's home directory, because the library opens
--- its store before it runs anything — but they read nothing from it and write
--- nothing to it.
function Wallet:crypto()
  local bound = {}
  for name, method in pairs(M.CRYPTO) do
    bound[name] = function(...) return Wallet[method](self, ...) end
  end
  return bound
end

-- ------------------------------------------------------------------- coverage

--- Which method covers which command, keyed by the name the library reports.
---
--- `false` marks a command deliberately not exposed. `tui` is the only one: it
--- takes over a terminal, which a library has no business doing and a LÖVE
--- window has nowhere to put.
---
--- The test suite reads the command list out of the library and checks this map
--- against it in both directions — a command with no method fails, and a method
--- named here that does not exist fails too.
M.COMMANDS = {
  ["info"] = "info",

  ["account new"] = "new_account",
  ["account import-mnemonic"] = "import_mnemonic",
  ["account import-key"] = "import_key",
  ["account list"] = { "accounts", "export_accounts" },
  ["account show"] = "account",
  ["account use"] = "use_account",
  ["account derive"] = "derive_account",
  ["account rename"] = "rename_account",
  ["account remove"] = "remove_account",
  ["account export"] = "export_account",
  ["account import-recent"] = "import_recent",

  ["recent list"] = "recent",
  ["recent show"] = "recent_entry",
  ["recent forget"] = "forget_recent",
  ["recent clear"] = "clear_recent",

  ["network list"] = "networks",
  ["network current"] = "current_network",
  ["network use"] = "use_network",
  ["network set-rpc"] = "set_rpc",

  ["balance"] = "balance",
  ["nonce"] = "nonce",
  ["gas-price"] = "gas_price",
  ["chain-info"] = "chain_info",
  ["send"] = "send",
  ["tx"] = "tx",
  ["history"] = "history",
  ["sign"] = "sign",
  ["verify"] = "verify",

  ["erc20 info"] = "token_info",
  ["erc20 balance"] = "token_balance",
  ["erc20 send"] = "token_send",

  ["utils keccak"] = "keccak",
  ["utils checksum"] = "checksum",
  ["utils to-wei"] = "to_wei",
  ["utils from-wei"] = "from_wei",
  ["utils new-mnemonic"] = "new_mnemonic",
  ["utils derive"] = "derive",
  ["utils sign"] = "sign_with",
  ["utils validate-mnemonic"] = "validate_mnemonic",

  ["tui"] = false,
}

return M
