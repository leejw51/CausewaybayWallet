# Causewaybay Wallet

An educational Cronos EVM wallet — testnet and mainnet — with a command line
interface and a terminal UI, implemented twice: once in Rust, once in Python.
Both write the same append-only JSONL store, so either can drive a wallet the
other created.

A third front end, in Lua, calls the Rust core through a C ABI rather than
reimplementing it — the same wallet reached differently, and the module a
[LÖVE](https://love2d.org) GUI loads.

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

# Python
pythoncli/.venv/bin/python -m causewaybay account new --label main
pythoncli/.venv/bin/python -m causewaybay balance

# Lua (needs LuaJIT, and `make -C luacli build` for the shared library)
luacli/bin/cwbwallet-lua account new --label main
luacli/bin/cwbwallet-lua balance
luacli/bin/cwbwallet-lua interactive   # a menu instead of flags
```

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

Packaging the Python one needs a Rust toolchain, because PyApp is itself a Rust
program. **Its first run downloads the Python dependencies** (eth-account and
friends) into `~/Library/Application Support/pyapp`, which takes ten seconds or
so and needs network; every run after that is local and starts in well under a
second. The interpreter is embedded, the third-party packages are not.

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
make version          # 1.1.0
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
| `SPEC.md` | the shared specification every implementation follows |
| `.github/workflows/` | CI on every push, and the tagged macOS release |
| `rustcli/` | the Rust workspace: `core/` the wallet, `ffi/` the C ABI, `cli/` the `cwbwallet` binary and TUI |
| `pythoncli/` | Python CLI and TUI (`cwbwallet`), its own `Makefile` and `.gitignore` |
| `luacli/` | the Lua CLI over the C ABI, and the module a LÖVE GUI loads |
| `testvectors/` | shared fixtures every implementation is tested against |
| `skills/causewaybay-wallet/` | the skill definition that lets an AI agent drive the wallet |
| `scripts/` | vector generation and cross-implementation checks run by `make test` |

## What it does

**Accounts** — a wallet holds one BIP-39 mnemonic and many addresses derived
from it, so `account new` walks the sequence 0, 1, 2, 3, … `--new-seed` starts a
separate mnemonic when you want one. Import a phrase or a raw private key, label
accounts, and switch between them.

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

**Chain** — balances, nonces, gas prices, chain info, native transfers, ERC-20
metadata/balances/transfers, transaction lookup, and a local history of
everything this wallet has sent.

**Crypto** — EIP-191 message signing and verification, keccak256, EIP-55
checksumming, and wei conversions that never touch floating point.

**Export** — write the wallet list as JSONL, CSV, aligned text or a Markdown
table, from the CLI (`account list --format md -o wallets.md`) or the TUI. Every
format carries the address and both public key encodings (33- and 64-byte);
`--secret` adds the private key and mnemonic and writes the file owner-only.

**Two front ends** — a scriptable CLI and a full-screen TUI (`cwbwallet tui`)
built around a visible command list, so nothing has to be memorised: `Tab` moves
between panes, `Enter` runs the highlighted command, `?` shows the full
reference, and every command keeps a single-key shortcut.

**Embeddable** — the Rust wallet is also a shared library with a C ABI, so a
program in another language can hold the whole thing without shelling out to a
binary. See [`rustcli/README.md`](rustcli/README.md) for the ABI and
[`luacli/README.md`](luacli/README.md) for the Lua binding built on it.

**Three ways to drive it** — flags for a script, the full-screen TUI for a
terminal, and `cwbwallet-lua interactive`, which is a numbered menu and a REPL
at one prompt: pick `1` to create a wallet, or type `account list` and get the
same answer the CLI would give. Seed phrases are typed with terminal echo off,
and the confirmation before anything irreversible is the wallet's own summary of
a transaction it has already priced and funded — not a sentence the menu wrote.

## Driving it from a script or an agent

Every command takes `--json` and answers with one line:

```console
$ cwbwallet --json balance
{"data":{"address":"0x9858…","balance":"12.5","balance_wei":"12500000000000000000","network":"cronos-testnet","symbol":"TCRO"},"ok":true}

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

| key | chain id | symbol | RPC | explorer |
| --- | -------- | ------ | --- | -------- |
| `cronos-testnet` (default) | 338 | TCRO | `https://evm-t3.cronos.org` | [explorer](https://explorer.cronos.org/testnet) |
| `cronos-mainnet` | 25 | CRO | `https://evm.cronos.org` | [explorer](https://explorer.cronos.org) |

Override an endpoint with `cwbwallet network set-rpc testnet <url>` or the
`CAUSEWAYBAY_RPC_CRONOS_TESTNET` environment variable.

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

## Testing

`make test` runs five things:

* **Rust** — 375 tests. BIP-39, BIP-32 and BIP-44 are implemented from scratch
  and checked against the official vectors; the CLI is exercised end to end
  against a scripted in-process JSON-RPC node, and the C ABI is called the way
  a C host would call it.
* **Python** — 450 tests covering the same ground, plus the Textual UI driven
  through its test pilot.
* **Lua** — 187 tests. Not the cryptography again, but the path through the
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
