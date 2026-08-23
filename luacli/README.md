# Causewaybay Wallet — Lua

> ⚠️ **Educational software.** Keys are stored unencrypted on disk. Do not use
> with funds you are not prepared to lose. For real value, use a hardware wallet.

A Lua front end for the wallet: a CLI, and the module a [LÖVE][love] GUI loads.
It is not a third implementation — there is no cryptography here. Every command
goes through a C ABI into `causewaybay-core`, the same Rust code the `cwbwallet`
binary runs, so the answers cannot drift.

```lua
local causewaybay = require("causewaybay")
local wallet = assert(causewaybay.open())

for _, account in ipairs(assert(wallet:accounts())) do
  print(account.label, account.address)
end

local balance, err = wallet:balance()
if not balance then
  print("could not read the balance:", err.code, err.message)
end
```

## Requirements

**LuaJIT.** The wallet is reached through LuaJIT's `ffi` module, which plain Lua
5.x does not have. LÖVE embeds LuaJIT, which is why the GUI will load these same
files unchanged.

```sh
brew install luajit            # macOS
sudo apt install luajit        # Debian/Ubuntu
```

There is nothing else: no LuaRocks, no C compiler, no JSON library. The one
thing that has to be built is the shared library, which belongs to the Rust
workspace.

## Getting started

```sh
make build                     # builds ../rustcli's shared library
./bin/cwbwallet-lua interactive   # a menu, if you'd rather not type flags
./bin/cwbwallet-lua info
./bin/cwbwallet-lua account import-mnemonic -m "abandon abandon … about" -l main
./bin/cwbwallet-lua --json account list
```

`make run ARGS="account list"` does the same through the Makefile. Everything
`cwbwallet --help` lists works here too — the argument tree is defined once, in
Rust, and this CLI does not parse it.

## Interactive mode

`cwbwallet-lua interactive` (or `make interactive`) opens a session that is a
menu and a REPL at once. It is not the TUI — that one is full-screen and lives
in the Rust CLI. This is plain lines in and out, so it works over SSH, over a
pipe, and in a terminal ratatui would not enjoy.

```
  Causewaybay Wallet — interactive
  cronos-testnet · 2 wallets · /Users/you/.causewaybaywallet

  1  create a wallet
  2  list wallets
  3  select the active wallet
  4  balance
  5  send
  6  export wallets to a file
  7  reveal a wallet's secrets
  8  switch network

  Pick a number, or type a command — `balance`, `account list`, …
  help [command]   menu   quit

cwb> 4
  12.5 TCRO
  0x9858EfFD232B4033E47d90003D41EC34EcaEda94 on cronos-testnet

cwb> account rename main "cold storage"
Renamed main to cold storage

cwb> help account new
Usage: cwbwallet account new [OPTIONS]
…
```

**One prompt, two ways in.** A number runs the menu action beside it; anything
else is a wallet command, run exactly as `cwbwallet-lua` would run it. Nothing
is ambiguous because menu keys are digits and commands begin with a letter — so
the menu is there while you are learning the surface, and never in the way once
you know it. `menu` redraws the list, `help` (or `help <command>`) is the
wallet's own help, `q` quits, and so does end of input.

Typed arguments are split the way a shell splits them, quotes included, so
`account rename main "cold storage"` arrives as three arguments and not four. A
lone `-` — where the CLI would read a pipe — is asked for at the prompt instead,
with echo off when what it wants is a phrase or a key.

Three more things are worth knowing.

**A mnemonic is typed like a password.** Terminal echo is turned off while a
seed phrase or a private key is entered, so it does not appear on screen or in
the scrollback of a shared session. Where that cannot be done — no terminal, or
no `stty` — the prompt says so first rather than quietly echoing.

**The confirmation is the wallet's own.** Anything irreversible is attempted
once *without* `yes`, which makes the wallet do all the real work — resolve the
nonce, the gas price and the gas limit, check the balance covers all of it — and
then refuse with `confirmation_required`, carrying the summary it would have put
to a human. That summary is what you are shown, so you are approving a
transaction that is already priced and ready to sign rather than a sentence this
front end assembled and hoped was right. Answer anything but yes and nothing is
signed. It applies to typed commands too: `account remove old` asks here, where
the CLI would tell you to re-run with `--yes`.

The globals still apply: `cwbwallet-lua --home /tmp/w -n mainnet interactive`
opens that wallet on that network. `--yes` is deliberately *not* inherited — a
menu that had pre-answered its own questions would be the worst of both shapes.

### Where the library is found

In order:

1. `$CAUSEWAYBAY_LIB`, if set — an exact path, for a packaged or unusual layout
2. next to `cwbwallet.lua`, which is how `make package` stages a bundle
3. `../rustcli/target/debug/`, then `target/release/`, then `../dist/` — freshest
   first, since `make build` writes the debug one and the other two come only
   from `make package`
4. whatever the system linker can find under the name `causewaybay_ffi`

When none of them work, the error names every path it tried. The binding also
refuses a library whose `cwb_abi_version` is not the one it was written for,
rather than calling into an envelope shape it does not know.

## What is in here

| file | what it is |
| --- | --- |
| `causewaybay/init.lua` | the wallet API — `open`, `call`, and a method per command |
| `causewaybay/ffi.lua` | the LuaJIT binding: loading the library, and the string-ownership rules |
| `causewaybay/json.lua` | a JSON codec, so nothing has to be installed to use this |
| `causewaybay/cli.lua` | the CLI: streams, exit statuses, and the SPEC.md envelope |
| `causewaybay/interactive.lua` | the session: the menu, the REPL, prompts and confirmations |
| `causewaybay/echo.lua` | turning terminal echo off, so a seed phrase is not typed in the open |
| `cwbwallet.lua` | the entry point — sets `package.path`, then exits with `cli.run`'s status |
| `bin/cwbwallet-lua` | a wrapper that finds LuaJIT and the entry point, for `$PATH` |
| `tests/` | the test suite and its runner |

## The API

`causewaybay.open(options)` returns a wallet, or `nil, err`.

| option | meaning |
| --- | --- |
| `home` | wallet directory; defaults to `$CAUSEWAYBAY_HOME` or `~/.causewaybaywallet` |
| `network` | the network for every call — `"testnet"`, `"cronos-mainnet"`, … |
| `yes` | answer confirmations with yes; a GUI sets this once its own dialog is wired up |
| `lib` | an explicit path to the shared library, skipping the search |

Every call returns `value` or `nil, err`, where `err.code` is one of the stable
strings in [`SPEC.md`](../SPEC.md) and `err.message` explains it. Nothing raises
for an ordinary wallet failure — a missing account is a value, not an exception.
Wrap a call in `assert(...)` where you would rather it did.

```lua
local account, err = wallet:account("ghost")
if not account then
  if err.code == "account_not_found" then …
  elseif err.code == "no_active_account" then …
  end
end
```

Three levels, from most convenient to most direct:

```lua
wallet:accounts()                          -- a method per command
wallet:call({"account", "list"})           -- data, or nil + err
wallet:envelope({"account", "list"})       -- the whole envelope, ok and all
```

`wallet:call` also returns the human text as its second value, which is how the
CLI prints exactly what `cwbwallet` would.

### Every command has a method

| command | method |
| --- | --- |
| `info` | `wallet:info()` |
| `account new` | `wallet:new_account{label=, new_seed=, words=, index=, show_secret=}` |
| `account import-mnemonic` | `wallet:import_mnemonic(phrase, {index=, label=, passphrase=})` |
| `account import-key` | `wallet:import_key(key, {label=})` |
| `account list` | `wallet:accounts{secret=, format=, output=}` · `wallet:export_accounts(format, opts)` |
| `account show` | `wallet:account(selector, {secret=})` |
| `account use` | `wallet:use_account(selector)` |
| `account derive` | `wallet:derive_account(index, {label=, from=})` |
| `account rename` | `wallet:rename_account(selector, label)` |
| `account remove` | `wallet:remove_account(selector, {yes=})` |
| `account export` | `wallet:export_account(selector)` |
| `account import-recent` | `wallet:import_recent(selector, {index=, label=, passphrase=})` |
| `recent list` | `wallet:recent{kind=, limit=}` |
| `recent show` | `wallet:recent_entry(selector, {secret=})` |
| `recent forget` | `wallet:forget_recent(selector, {yes=})` |
| `recent clear` | `wallet:clear_recent{yes=}` |
| `network list` | `wallet:networks()` |
| `network current` | `wallet:current_network()` |
| `network use` | `wallet:use_network(key)` |
| `network set-rpc` | `wallet:set_rpc(network, url)` |
| `balance` | `wallet:balance{address=, account=}` |
| `nonce` | `wallet:nonce{address=, account=}` |
| `gas-price` | `wallet:gas_price()` |
| `chain-info` | `wallet:chain_info()` |
| `send` | `wallet:send{to=, amount=, gas_limit=, gas_price_gwei=, nonce=, data=, wait=, account=, yes=}` |
| `tx` | `wallet:tx(hash)` |
| `history` | `wallet:history{limit=, network=}` |
| `sign` | `wallet:sign(message, {account=})` |
| `verify` | `wallet:verify(message, signature, address)` |
| `erc20 info` | `wallet:token_info(token)` |
| `erc20 balance` | `wallet:token_balance(token, {address=})` |
| `erc20 send` | `wallet:token_send{token=, to=, amount=, wait=, account=, yes=}` |
| `utils keccak` | `wallet:keccak(input, {hex=})` |
| `utils checksum` | `wallet:checksum(address)` |
| `utils to-wei` | `wallet:to_wei(amount, decimals)` |
| `utils from-wei` | `wallet:from_wei(value, decimals)` |
| `utils new-mnemonic` | `wallet:new_mnemonic(words)` |
| `utils derive` | `wallet:derive{mnemonic=, private_key=, index=, passphrase=}` |
| `utils sign` | `wallet:sign_with(private_key, message)` |
| `utils validate-mnemonic` | `wallet:validate_mnemonic(phrase)` |
| `tui` | — the full-screen UI is the Rust CLI's; `interactive` is the menu here |

That table is not maintained by hand-checking. `causewaybay.COMMANDS` holds the
same mapping in code, `wallet:commands()` asks the library what commands it
actually has, and the test suite compares the two in both directions — so a
command added in Rust with no Lua method fails the build, and so does a method
named in the map that does not exist.

`wallet:commands()` is also what a GUI should build its panels from: each entry
carries `path`, `about` and an `args` list with `long`, `short`, `positional`,
`takes_value`, `required` and `default`, which is enough to render a form
without hardcoding one per command.

### Crypto without a wallet

Some of those are pure: no network, no account, nothing written. `wallet:crypto()`
binds them into a namespace for code that does cryptography and nothing else —
a game hashing an identifier, checking an address a player pasted in, deriving a
throwaway key:

```lua
local crypto = wallet:crypto()

crypto.keccak("hello").keccak256        --> 0x1c8aff95…
crypto.checksum("0x5aaeb605…").address  --> 0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed
crypto.to_wei("1.5").value              --> 1500000000000000000
crypto.new_mnemonic(24).mnemonic        --> a fresh phrase, stored nowhere

-- Derive without acquiring an account or a recall entry:
local key = crypto.derive{ mnemonic = phrase, index = 3 }
print(key.address, key.private_key, key.public_key_compressed)

-- Sign with a key you hold yourself, rather than one the wallet stores:
local signed = crypto.sign(key.private_key, "gg")
print(crypto.verify("gg", signed.signature).recovered)   --> key.address

-- And ask about a phrase instead of being refused one:
local check = crypto.validate_mnemonic(typed)
if not check.valid then print(check.reason) end
```

`derive`, `sign` and `validate_mnemonic` are the three that exist for this:
`account import-mnemonic` would store the phrase and remember it, `sign` needs a
stored account, and importing an invalid phrase is an error rather than an
answer. These do the arithmetic and leave the wallet alone —
`causewaybay.CRYPTO` names exactly which calls those are.

They still touch the wallet's home directory, because the library opens its
store before running anything; they read nothing from it and write nothing to
it.

### Confirmation

A library cannot prompt, so it refuses instead. Anything irreversible fails with
`confirmation_required` unless `yes` was set — on the wallet, or on that one
call:

```lua
wallet:remove_account("old")                  -- confirmation_required
wallet:remove_account("old", { yes = true })  -- done
```

For a GUI this is the right shape: show your own dialog, and pass `yes = true`
only once the person has answered it.

### Standard input

There is no stdin inside a shared library. Where the CLI would accept `-`, the
Lua API takes the value directly:

```lua
wallet:sign("hello")                       -- no pipe, no `-`
```

The CLI does read a real pipe, and passes what it read in the request, so
`echo … | cwbwallet-lua account import-mnemonic -m -` works as expected.

## Using it from LÖVE

LÖVE embeds LuaJIT, so nothing changes but where the files sit. Open the wallet
once and keep it:

```lua
local causewaybay = require("causewaybay")

local wallet, err

function love.load()
  wallet, err = causewaybay.open({
    -- The GUI shows its own "really send?" dialog, so the wallet underneath
    -- must not refuse on its own.
    yes = true,
    -- love.filesystem sandboxes writes; the wallet writes with plain io, so
    -- give it a directory of its own rather than a save-directory path.
    home = os.getenv("HOME") .. "/.causewaybaywallet",
  })
end

function love.draw()
  if not wallet then
    love.graphics.print("Wallet unavailable: " .. tostring(err), 20, 20)
    return
  end
  for i, account in ipairs(wallet:accounts() or {}) do
    love.graphics.print(account.label .. "  " .. account.address, 20, i * 20)
  end
end
```

Two things to know. A call that touches the network blocks — `balance`, `send`,
anything reading from a node — so run those from a `love.thread` rather than
from `love.update`, or the window stops drawing while an RPC round trip
finishes. And put the shared library where the binding will find it (a
`$CAUSEWAYBAY_LIB` pointing at your `.love` bundle's copy is the simplest
answer).

## Tests

```sh
make test              # everything, building the shared library first
make test-json         # only the JSON codec — needs no library
make test-vectors      # only the shared test vectors
luajit tests/init.lua wallet cli
```

| suite | what it covers |
| --- | --- |
| `json` | the codec: null against nil, `[]` against `{}`, surrogate pairs, malformed input |
| `echo` | turning terminal echo off, and restoring it even when the read throws |
| `ffi` | finding the library, the ABI check, and the free-exactly-once contract |
| `wallet` | the API against a real store in a temp home, and that it covers every command |
| `cli` | exit statuses, which stream each thing goes to, the envelope byte for byte |
| `interactive` | the menu and the REPL: prompts, masked input, argument splitting, cancelled sends, end of input |
| `vectors` | the shared files in [`testvectors/`](../testvectors), the same ones Rust and Python read |

`interactive.loop` takes its wallet and its two streams as arguments and returns
a status, so a whole session is a list of scripted answers in and a string out —
no subprocess, no pty, no timing. The masked reader is separate from the plain
one in that context, which is how a test can assert that a mnemonic went through
the hidden one and a label did not.

The vector suite is the one that matters most here. It is not re-checking the
cryptography — the Rust suite already did — it is checking the *path*: that a
256-bit integer survives as a string rather than becoming a double, that an
emoji in `keccak.json` arrives as the bytes that were hashed, that an error code
is the same word on both sides. `make test-vector-coverage` from the repository
root proves the suite really reads those files, by corrupting each one and
requiring every suite to notice.

The harness is `tests/runner.lua`, about a hundred lines, for the same reason
there is no JSON dependency: a wallet whose tests need a package manager is a
wallet whose tests do not get run.

## Not here

`tui` — the full-screen terminal UI belongs to the Rust CLI, which owns a
terminal. Running it is `cwbwallet tui`; asking this CLI for it says so, and
points at `interactive` instead.

[love]: https://love2d.org
