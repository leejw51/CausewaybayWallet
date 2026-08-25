# Causewaybay Wallet

An educational multi-chain wallet — Cronos EVM, Solana, Cardano, Midnight and
eCash — with a command line interface and a terminal UI.

The wallet is one implementation, in Rust: the key derivation for all five
chains, the append-only JSONL store, the RPC and the command surface. Everything
else is a front end over that core's C ABI — Python and Lua load the shared
library at run time, C links the static one in, and a
[LÖVE](https://love2d.org) GUI sits on the Lua binding. Four routes to one
wallet, so a store any of them creates is a store all of them can drive.

> ⚠️ **Educational software.** Private keys are stored unencrypted on disk. Use
> it on the testnet. For anything of value, use a hardware wallet.

## Quick start

```bash
make                # list the targets
make test           # run every test in all three front ends
make build          # build the Rust workspace and install the Python package
```

Then, with any of them:

```bash
# Rust
rustcli/target/debug/cwbwallet account new --label main
rustcli/target/debug/cwbwallet balance

# one mnemonic, an address on every chain
rustcli/target/debug/cwbwallet account new --every-chain --label main
rustcli/target/debug/cwbwallet balance --all
rustcli/target/debug/cwbwallet --chain solana balance

# Python
pythoncli/.venv/bin/python -m causewaybay account new --label main
pythoncli/.venv/bin/python -m causewaybay balance

# Lua (needs LuaJIT, and `make -C luacli build` for the shared library)
luacli/bin/cwbwallet-lua account new --label main
luacli/bin/cwbwallet-lua balance
luacli/bin/cwbwallet-lua interactive   # a menu instead of flags
```

## The chains

| chain | networks | curve and derivation | address |
| --- | --- | --- | --- |
| `evm` | Cronos testnet, mainnet | secp256k1, BIP-32 · `m/44'/60'/0'/0/i` | EIP-55 hex |
| `solana` | devnet, testnet, mainnet | ed25519, SLIP-0010 hardened-only · `m/44'/501'/i'/0'` | base58 of the public key |
| `cardano` | preprod, preview, mainnet | extended ed25519, Icarus + BIP32-Ed25519 · `m/1852'/1815'/0'/0/i` | bech32 of blake2b-224 hashes |
| `midnight` | preview, devnet | secp256k1 → BIP-340 Schnorr, BIP-32 · `m/44'/2400'/0'/0/i` | bech32m of SHA-256(x-only pubkey) |
| `ecash` | testnet, mainnet | secp256k1, BIP-32 · `m/44'/1899'/0'/0/i` | CashAddr of ripemd160(sha256(pubkey)) |

Every network belongs to one chain, so naming a network settles the chain:
`-n solana-devnet` and `--chain solana` reach the same place. `--chain` alone
uses whichever of that chain's networks the wallet was last on.

A bare network name that several chains share — `devnet` is Solana's *and*
Midnight's, `testnet` names a row on three chains — is refused rather than
guessed, because guessing sends funds to the wrong chain. `testnet` and
`mainnet` are the exceptions: they meant Cronos before the wallet had other
chains, and still do.

Four things worth knowing, because each otherwise produces a plausible, wrong,
unfunded address rather than an error:

* **Cardano hashes the mnemonic's entropy, not its seed**, and passes the
  passphrase as the PBKDF2 password with the entropy as the salt. Backwards, and
  what every Cardano wallet does.
* **Solana's derivation is hardened-only.** `m/44'/501'/0'/0` — one apostrophe
  short — is refused rather than silently hardened.
* **BIP-340 negates about half of all secret keys**, so Midnight stores the
  scalar BIP-32 derived rather than the one the signing key reports.
* **eCash hashes the compressed public key**, and uses coin type `1899` —
  neither Bitcoin's `0` nor the generic testnet `1`. The uncompressed key is an
  equally valid encoding of the same key and hashes to a different address.

Each chain's derivation, addresses and transaction encoding are checked against
that chain's own SDK, through the vectors in `testvectors/multichain.json` —
and, for eCash, against `testvectors/ecash.json`, whose addresses come from an
encoder pinned to the vector published with the CashAddr specification.

### eCash amounts

**XEC has two decimal places, not Bitcoin's eight.** eCash redenominated at its
2021 rebrand, so one XEC is a hundred satoshis where one BCH was a hundred
million. A balance read with Bitcoin's decimals is out by a factor of a million,
in the direction that makes a large transfer look like a rounding error.

Two other numbers come with the format. The **dust limit** is 546 satoshis
(5.46 XEC): below it an output is not relayed at all, so a smaller transfer is
refused here rather than signed and dropped. And the **fee** is a flat satoshi
per byte — there is no fee market to estimate against — which means the cost of
a send is really the cost of its *inputs*, about 148 bytes each. `send
--dry-run` shows the coin selection and the size the fee was derived from.

eCash carries eTokens as annotations on ordinary outputs, so an output holding
one is spendable in the plain sense and spending it as XEC burns the token.
This wallet moves XEC only: it counts those outputs in a balance, because they
are genuinely there, and never selects one to spend. A transfer that had to
leave some behind says so in the sentence it asks you to agree to.

### Midnight fees

Midnight moves NIGHT and pays its fees in DUST, and spending DUST normally needs
a zero-knowledge proof. There is one exception, and the wallet takes it when it
can: NIGHT that was never registered for DUST generation accrues an implicit fee
allowance, and a transfer spending it can pay from that allowance with
signatures alone.

That registration is permanent for the address, so every later send — including
one spending the change from the first — takes the other path: replay the dust
ledger, generate a real proof locally (the parameters, ~4 MB, are fetched once
and cached), and pay from a proved DUST spend. It works, and it takes minutes
rather than milliseconds. `send --dry-run` shows which path a transfer would
take, and its fee, before committing to it.

The proving is for the *fee*. NIGHT is Midnight's unshielded token, so amounts,
sender and recipient are all public; shielded Zswap transfers are not
implemented.

## Packaging

```bash
make package          # every artifact into ./dist
make package-rust     # only the Rust binary and shared library
make package-python   # only the Python one
make package-lua      # only the Lua bundle
make package-c        # only the C binary
make package-smoke    # everything but the Python one, checked — ~1 min, what PRs run
make package-verify   # package, then run the parity checks against ./dist
```

| artifact | size | what it is |
| -------- | ---- | ---------- |
| `dist/cwbwallet-rust` | ~4.8 MB | the release binary; nothing to carry alongside it |
| `dist/cwbwallet-python` | ~17 MB | [PyApp](https://ofek.dev/pyapp/) wrapping the wheel and a redistributable CPython |
| `dist/libcausewaybay_ffi.dylib` | ~4.2 MB | the wallet as a shared library, with `causewaybay.h` beside it |
| `dist/cwbwallet-lua/` | ~4.4 MB | the Lua front end and its own copy of the library; needs LuaJIT |

All of them are the same CLI and share `~/.causewaybaywallet`, so they can be
used interchangeably — `make package-verify` proves it by running the cross-
implementation checks against the packaged artifacts rather than the source
tree. The Lua bundle is self-contained: copy the directory anywhere and it finds
the library beside it.

Packaging the Python one needs a Rust toolchain twice over: PyApp is itself a
Rust program, and the wallet it wraps is the Rust shared library, staged into
the wheel so a packaged binary carries its own core. There are no third-party
Python dependencies to fetch — the binding is ctypes from the standard library
— so the first run is local like every run after it.

### macOS signing

`scripts/codesign-binary.sh` signs each binary as it lands in `dist/`, using the
same environment contract as PocketSkynet so one exported set of credentials
covers both repositories:

| variable | meaning |
| -------- | ------- |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: …` — defaults to `-`, ad-hoc |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | notarization credentials |

With no identity exported the binaries are signed ad-hoc, which is what arm64
requires for an executable to run at all and keeps Gatekeeper's objection to the
honest one — unknown developer — rather than a broken-binary error. With a real
identity the script signs with the hardened runtime and a secure timestamp, then
notarizes. It deliberately stops short of stapling: a ticket cannot be attached
to a bare Mach-O, so a notarized CLI still costs the first machine that runs it
one online check with Apple.

## Versioning and releases

One number covers everything. It lives in `rustcli/Cargo.toml` (the workspace
manifest, which all three Rust crates inherit) and `pythoncli/pyproject.toml`,
and `make version` prints it only when the two agree — a mismatch is an error,
not a warning, and packaging refuses to run:

```bash
make version          # 1.0.3
```

Nothing repeats that number anywhere else. The Rust crates report
`CARGO_PKG_VERSION`, the Python one reads its own installed distribution
metadata, the Lua front end has no version of its own — it reports whatever
library it loaded — and `scripts/parity.sh` checks that what each front end
actually prints — `cwbwallet --version` and the `version` field of
`cwbwallet info` — matches the manifests. So a version a release claims is one
that was verified.

To cut a release, bump both manifests, then tag `main`:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

[`.github/workflows/release.yml`](.github/workflows/release.yml) refuses the tag
unless it matches `make version`, builds both binaries on macOS, signs and
notarizes them with the repository's Apple credentials, runs the parity checks
against `./dist`, and attaches the archives — plus `SHA256SUMS` — to a GitHub
release:

```
cwbwallet-rust-<version>-<arch>-apple-darwin.tar.gz
cwbwallet-python-<version>-<arch>-apple-darwin.tar.gz
cwbwallet-lua-<version>-<arch>-apple-darwin.tar.gz
```

The two compiled archives hold the binary as `cwbwallet`, so either installs the
same way; the Lua one holds the whole bundle, since it is a script that needs
its modules and its shared library beside it. `workflow_dispatch` runs the whole
path as a dry run: it uploads the artifacts and creates no release.

### Continuous integration

[`.github/workflows/ci.yml`](.github/workflows/ci.yml) runs on every push to
`main` and every pull request, split so a failure names itself: `rust`
(rustfmt, clippy, tests), `lua` (LuaJIT against the freshly built shared
library), `python` (ruff and pytest on 3.10 and 3.12 — the floor and the
interpreter PyApp embeds), `parity` (vector reproducibility, the mutation
check, and the cross-implementation script), and `version`. A sixth
job packages on macOS with ad-hoc signing, skipped on pull requests because
PyApp downloads a CPython distribution to do it — the release path should not
be exercised for the first time at a tag, but a pull request should not pay for
it either.

The signing secrets the release needs: `MACOS_CERTIFICATE_P12_BASE64`,
`MACOS_CERTIFICATE_PASSWORD`, `APPLE_ID`, `APPLE_APP_SPECIFIC_PASSWORD` and
`TEAM_ID`. Without them everything still builds and signs ad-hoc.

## Layout

| path | what it is |
| ---- | ---------- |
| `SPEC.md` | the specification the core implements and every front end relies on |
| `.github/workflows/` | CI on every push, and the tagged macOS release |
| `rustcli/` | the Rust workspace: `core/` the wallet, `ffi/` the C ABI, `cli/` the `cwbwallet` binary and TUI |
| `pythoncli/` | the Python binding over the C ABI, its CLI and menu (`cwbwallet`) |
| `luacli/` | the Lua CLI over the C ABI, and the module the LÖVE GUI loads |
| `lovegui/` | the wallet as an 8-bit LÖVE game, built on that module |
| `testvectors/` | shared fixtures every implementation is tested against |
| `skills/causewaybay-wallet/` | the skill definition that lets an AI agent drive the wallet |
| `scripts/` | vector generation and cross-implementation checks run by `make test` |

## What it does

**Accounts** — a wallet holds one BIP-39 mnemonic and many addresses derived
from it, so `account new` walks the sequence 0, 1, 2, 3, … on the chain in play.
`--every-chain` derives the same mnemonic on every one at once; `--new-seed`
starts a separate mnemonic when you want one. Import a phrase or a raw private
key in the chain's own format, label accounts, and switch between them. Index
sequences are per chain, so a new Solana address does not push Cardano's along.

**Recall** — every mnemonic and private key the wallet has used is remembered, so
a returning user picks from a list instead of retyping a phrase:

```bash
cwbwallet recent list
#  1. mnemonic     0x9858EfFD…  abandon … about   used 2x  2026-08-22T05:12:03
#  2. private_key  0x6Fac4D18…  0x1ab42c…b727     used 1x  2026-08-21T18:44:51
cwbwallet account import-recent 1 --label restored
```

Previews identify an entry without revealing it; the secret itself needs an
explicit `--secret`.

**Chain** — balances (`balance --all` reads every chain at once, concurrently),
nonces, fee quotes, chain info, native transfers, a Solana faucet, transaction
lookup, and a local history of everything this wallet has sent. ERC-20
metadata/balances/transfers on EVM. `send --dry-run` builds and signs a transfer
and shows exactly what would go out without broadcasting it — which is the only
way to see a Midnight fee, or a Cardano coin selection, before committing.

**Crypto** — message signing and verification in each chain's own scheme
(EIP-191 on EVM, ed25519 on Solana and Cardano, BIP-340 Schnorr on Midnight,
Bitcoin's signed-message scheme on eCash),
keccak256, EIP-55 checksumming, and unit conversions that never touch floating
point.

**Export** — write the wallet list as JSONL, CSV, aligned text or a Markdown
table, from the CLI (`account list --format md -o wallets.md`) or the TUI. The
list is flattened wallet by wallet, chain by chain, network by network, and each
row names itself accordingly — `account0-cronos-testnet`,
`account0-cronos-mainnet`, `account0-solana`, `account0-cardano`,
`account0-midnight`, `account0-ecash`, then index 1's. A chain whose address
carries the network (Cardano, Midnight, eCash) renders the right address for
each row rather than repeating one. Every format carries the address and both public key encodings (33- and
64-byte); `--secret` adds the private key and mnemonic and writes the file
owner-only.

**Two front ends** — a scriptable CLI and a full-screen TUI (`cwbwallet tui`)
built around a visible command list, so nothing has to be memorised: every row
leads with its key, `Tab` moves between panes, `↑↓` picks a wallet and `←→` the
chain in view, `Enter` runs the highlighted command, and the bottom line always
says what the keys under your fingers do in the pane you are in. `?` shows the
full reference for when you want the rest.

The TUI is chain-first. Every network is a row of its own in one flat list —
`cronos testnet`, `cronos mainnet`, `solana devnet`, … — so going anywhere is one
press from anywhere, and the rows never move. Each wears its chain's colour, a ●
marks the network in use, and `←→` steps between chains without leaving the
wallet list. The wallet list is one row per wallet — `index 0`,
`index 1` — and the accounts of the highlighted one are laid out in the
detail pane beside it, each named for the wallet and the chain it belongs to
(`account0-evm`, `account0-solana`, …), with the chain in view marked. Choosing
a chain re-points balance, send and the rest at that chain's account without the
list moving; a row says nothing but its index unless the wallet is missing a
chain, and the header tallies what is held on each — so a wallet spread over
every chain never looks like a wallet that lost most of them. Anything that
waits on a node — a balance, a transfer being prepared — runs on a thread with a
clock in the status line and Esc to stop waiting, so the screen never freezes.

**Embeddable** — the Rust wallet is also a shared library with a C ABI, so a
program in another language can hold the whole thing without shelling out to a
binary. See [`rustcli/README.md`](rustcli/README.md) for the ABI and
[`luacli/README.md`](luacli/README.md) for the Lua binding built on it.

**Four ways to drive it** — flags for a script, the full-screen TUI for a
terminal, `cwbwallet-lua interactive`, which is a numbered menu and a REPL
at one prompt, and `make gui` — the wallet as an 8-bit game, rendered at 480×270
with springs, particles and art drawn by Grok. Seed phrases are typed with terminal echo off,
and the confirmation before anything irreversible is the wallet's own summary of
a transaction it has already priced and funded — not a sentence the menu wrote.

## Driving it from a script or an agent

Every command takes `--json` and answers with one line:

```console
$ cwbwallet --json balance
{"data":{"address":"0x9858…","balance":"12.5","balance_wei":"12500000000000000000","chain":"evm","network":"cronos-testnet","symbol":"TCRO"},"ok":true}

$ cwbwallet --json --chain solana balance
{"data":{"address":"HAgk14Jp…","balance":"5","balance_raw":"5000000000","chain":"solana","network":"solana-devnet","symbol":"SOL"},"ok":true}

$ cwbwallet --json account show ghost; echo "exit=$?"
{"error":{"code":"account_not_found","message":"no account matching 'ghost'"},"ok":false}
exit=1
```

Error codes are a fixed vocabulary (see `SPEC.md` §4) shared by both
implementations. Anything that spends or destroys requires an explicit `--yes`;
in `--json` mode there is no prompt, so an automated caller cannot spend by
accident.

`skills/causewaybay-wallet/SKILL.md` documents the whole surface for an AI agent.

## Networks

`cwbwallet network list` prints them grouped by chain; `cwbwallet chains` prints
what each chain can do. The default is `cronos-testnet`.

| key | chain | chain id | symbol | tags | endpoint |
| --- | ----- | -------- | ------ | ---- | -------- |
| `cronos-testnet` (default) | evm | 338 | TCRO | evm testnet smart-contracts erc20 | `https://evm-t3.cronos.org` |
| `cronos-mainnet` | evm | 25 | CRO | evm smart-contracts erc20 | `https://evm.cronos.org` |
| `solana-devnet` | solana | — | SOL | svm testnet faucet spl | `https://api.devnet.solana.com` |
| `solana-testnet` | solana | — | SOL | svm testnet faucet spl | `https://api.testnet.solana.com` |
| `solana-mainnet` | solana | — | SOL | svm spl | `https://api.mainnet-beta.solana.com` |
| `cardano-preprod` | cardano | — | tADA | utxo testnet native-assets | `https://preprod.koios.rest/api/v1` |
| `cardano-preview` | cardano | — | tADA | utxo testnet native-assets | `https://preview.koios.rest/api/v1` |
| `cardano-mainnet` | cardano | — | ADA | utxo native-assets | `https://api.koios.rest/api/v1` |
| `midnight-preview` | midnight | — | NIGHT | privacy testnet shielded zk | `https://indexer.preview.midnight.network/api/v4/graphql` |
| `midnight-devnet` | midnight | — | NIGHT | privacy testnet shielded zk | `https://indexer.devnet.midnight.network/api/v4/graphql` |
| `ecash-testnet` | ecash | — | tXEC | utxo testnet bitcoin-fork | `https://chronik-testnet.fabien.cash` |
| `ecash-mainnet` | ecash | — | XEC | utxo bitcoin-fork | `https://chronik.e.cash` |

Only EVM networks have a chain id — it is the EIP-155 replay-protection number,
omitted rather than faked for the rest.

### Finding one

There are twenty rows now — ten networks and ten tokens — and there will be
more as coins are added, so every list is searchable and the search works the
same way everywhere:

    cwbwallet network list                # all of them; this is the default
    cwbwallet network list evm            # the two Cronos rows
    cwbwallet network list testnet        # all six test networks, however named
    cwbwallet network list --tags         # what there is to search by

A **tag says what the row's name does not.** Searching already reads the key,
the name, the symbol and the chain, so `cronos-mainnet` needs no `cronos` tag;
it needs `evm`, which appears nowhere else on the row. Every word in a query
has to match, so adding a word narrows: `evm testnet` is one network. Case and
`-`/`_`/space are ignored. An empty query is everything — the search narrows a
list that is already in front of you, it never gates it.

In the **terminal UI**, `/` opens the same search over the command pane: type
`usdc cro`, press Enter, done. In the **LÖVE GUI**, the network screen has a
search box at the top of its frame that is always focused — arrive and type.

### Picking one, in the GUI

A row on the network screen is a **destination**, not a preview. Picking
`cronos-mainnet` puts the window on Cronos mainnet in CRO; picking the USDC row
on it puts the window on Cronos mainnet **in USDC** — and from then on the
balance shown is the ERC-20 balance, the send screen sends USDC, the amount
field is labelled `AMOUNT (USDC)`, and the header says `USDC · cronos-mainnet`.
One click settles both halves, which is the whole reason the token table is
flat: the row already names the chain, the network and the contract.

Picking a network row afterwards drops back to that network's own coin.
Switching network at any point drops the token too — a token belongs to one
network, and carrying it across would leave the window claiming to hold Cronos
USDC on Solana. An asset the wallet reads but cannot move greys out SEND and
says why.

Override an endpoint with `cwbwallet network set-rpc testnet <url>` or the
`CAUSEWAYBAY_RPC_<NETWORK>` environment variable. Midnight reads from an indexer
and submits to a different service, its node RPC; that half is overridden with
`CAUSEWAYBAY_SUBMIT_MIDNIGHT_PREVIEW` or the `submit.<network>` config key.

## Tokens

A token is not a thing you hold; a token **on a network** is. USDC on Cronos and
USDC on Solana share a name, a peg and nothing else — different issuer,
different decimals, an address that means nothing on the other chain. So a
registry row is the pair, and it is named as the pair: `usdc-cronos-mainnet`,
said **"USDC Cronos Mainnet"**. Naming it settles the chain, the network, the
decimals and the contract at once.

    cwbwallet token list                   # all of them, grouped by network
    cwbwallet token list usdc              # the four USDC rows
    cwbwallet token list stablecoin cronos # the three on Cronos mainnet
    cwbwallet token list --tags            # what there is to search by

    cwbwallet token info usdc-cronos-mainnet
    cwbwallet token balance usdc-cronos-mainnet
    cwbwallet token send usdc-cronos-mainnet --to 0x… --amount 25

| token | network | dp | reads | moves |
| ----- | ------- | -- | ----- | ----- |
| USDC, USDT, DAI | `cronos-mainnet` | 6, 6, 18 | yes | yes |
| USDC, USDT | `solana-mainnet` | 6 | yes | yes |
| USDC | `solana-devnet` | 6 | yes | yes |
| USDC, USDM, DJED, iUSD | `cardano-mainnet` | 8, 6, 6, 6 | yes | no |

Every contract address, mint and asset id was **read off the chain it names**,
not copied from a list. The decimals are not uniform and it matters: Cardano's
bridged USDC carries eight places, not six, and assuming otherwise misstates a
balance by a factor of a hundred. Before any transfer the wallet re-reads the
decimals from the contract or the mint and refuses if they disagree with its
own table — a wrong number there would scale the amount by a power of ten.

Cardano native assets are **read but not moved**. Spending the output holding
one means rebuilding every other asset riding on the same UTxO, and dropping one
silently is not something a wallet may do; the refusal says so before anything
is signed. Those rows are tagged `read-only`.

On Solana, a balance does not live on your address — it lives in an *associated
token account* derived from your address and the mint, which this wallet finds
for you. Sending to someone who has never held that token creates their account
and costs the sender its rent (about 0.002 SOL); that cost is added to the fee
you are asked to approve rather than hidden, and both instructions ride one
transaction, so there is no state where the account was made and the tokens did
not arrive.

`cwbwallet erc20 …` is unchanged and still takes a contract address, for a token
the registry has never heard of.

### The fee ceiling

Every network refuses a fee above a number the wallet keeps itself, checked
before anything is signed. The fee is the endpoint's number — an inflated
`eth_gasPrice`, a Koios instance answering `min_fee_a = 10⁹` — and nothing else
in a transfer questions it: the transaction balances, the signature is valid,
and the confirmation used to name only the amount.

    cwbwallet network current                       # shows the one in force
    cwbwallet network set-max-fee testnet 3         # refuse anything over 3 TCRO
    cwbwallet network set-max-fee testnet 0         # back to the built-in one

It is a **refusal threshold, not a price** — setting it low does not make sends
cheaper, it makes them fail. Nearly everyone should leave it alone; it is there
to catch an endpoint that lies, not to bid for block space.

The number is counted in the token that *pays* the fee, which is the native
token everywhere except Midnight — a Midnight transfer moves NIGHT and pays in
DUST, nine decimal places apart. So write the unit if you want it checked:
`set-max-fee midnight-preview "2 DUST"` is accepted and `"2 NIGHT"` is refused
rather than quietly read as DUST. Stored with its denomination either way.

The TUI sets it from the **Fee ceiling** row; the LÖVE GUI shows the one in
force on its network screen and leaves the changing to the command line.

## Where state lives

`~/.causewaybaywallet/` — override with `--home PATH` or `CAUSEWAYBAY_HOME`.

```
accounts.jsonl   account created / renamed / deleted
config.jsonl     selected network, active account, RPC overrides
history.jsonl    transactions this wallet submitted
recent.jsonl     mnemonics and private keys offered back for reuse
```

Every file is append-only: state is the fold of every line, so a crash can at
worst lose the last partial line, and the whole history stays readable with
`cat`. The directory is `0700`, the files `0600`.

Removal is the exception. `account remove`, `recent forget` and `recent clear`
rewrite their file without the record, rather than appending a tombstone — an
account record holds a plaintext private key and a recall entry is nothing but
a phrase, and a tombstone would leave both exactly where they were. "Forget"
means the line is gone.

## Testing

`make test` runs five things:

* **Rust** — 680 tests. BIP-39, BIP-32 and BIP-44 are implemented from scratch
  and checked against the official vectors; the CLI is exercised end to end
  against a scripted in-process JSON-RPC node, and the C ABI is called the way
  a C host would call it.
* **Python** — 174 tests over the binding: the shared vectors driven through
  ctypes and the C ABI, a coverage suite that reads the command list out of the
  library and fails if any command has no Python method, and the CLI end to end
  against a real store.
* **Lua** — 198 tests. Not the cryptography again, but the path through the
  boundary: that a 256-bit integer stays a string rather than becoming a
  double, that an emoji arrives as the bytes that were hashed, that an error
  code is the same word on both sides. Plus the interactive menu, driven by
  scripted answers, and a coverage suite that reads the command list out of
  the library and fails if any command has no Lua method.
* **Vectors** — `scripts/check-vectors.sh` confirms the shared fixtures in
  `testvectors/` regenerate byte-identically, so the goalposts cannot move
  silently; `scripts/check-vector-coverage.py` then corrupts one value per file
  and requires every suite to notice, so a suite that quietly skips a file
  cannot pass.
* **Parity** — `scripts/parity.sh` points all three front ends at one wallet,
  has each write and the others read, and checks they agree on addresses,
  account ids, signatures, recall entries, error codes and the version they
  report.

No test touches a real network or a real key.

### Shared test vectors

`testvectors/` holds generated fixtures that every implementation runs against —
the official BIP-39 and EIP-55 vectors, the worked example from EIP-155, and the
mnemonics and keys that Anvil, Hardhat and Ganache print on startup:

```
test test test test test test test test test test test junk
  → 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266   (Anvil account #0)
```

The generator computes every value from a reference implementation and then
asserts it against the published constant, refusing to write a file when the two
disagree. See [`testvectors/README.md`](testvectors/README.md); regenerate with
`make vectors`.

## Licence

MIT — see `LICENSE`.
