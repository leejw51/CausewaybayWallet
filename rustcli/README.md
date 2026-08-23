# Causewaybay Wallet — Rust

The Rust implementation of the wallet specified in [`../SPEC.md`](../SPEC.md).
A workspace of three crates: the wallet itself, a C ABI over it, and the
`cwbwallet` binary.

```bash
make          # list the targets
make build    # the whole workspace; binary at target/debug/cwbwallet
make ffi      # only the shared library the Lua front ends load
make test     # unit, integration and doc tests
make tui      # launch the terminal UI
make check    # fmt, clippy and tests, as CI runs them
make package  # signed release artefacts into ../dist/
```

Packaging is the release build plus a copy and a signature — a Rust binary
carries no runtime with it, which is the whole difference from `pythoncli/`.

## The three crates

```
core/  causewaybay-core   the wallet: keys, the store, RPC, every command
ffi/   causewaybay-ffi    a C ABI over core — JSON in, JSON out
cli/   causewaybay-cli    the cwbwallet binary: clap, the TUI, a terminal
```

The split exists because of what `core` is *not* allowed to do. It never prints,
never reads stdin, never exits the process, and never seizes the screen. A
command is a function from a `Command` to a `CommandOutput`; the two things one
genuinely needs from the outside world — a secret passed as `-`, and a yes
before spending money — arrive through the `Host` trait:

```rust
pub trait Host: Send + Sync {
    fn read_input(&self, what: &str) -> Result<String>;
    fn confirm(&self, prompt: &str) -> Result<()>;
}
```

`cli` supplies a `TerminalHost` that reads a real pipe and prompts on a real
tty. `ffi` supplies core's own `Headless`, which answers from values the caller
put in the request. Same commands, same store, same output — a different way of
being asked.

That is what makes the wallet loadable from anywhere. The Lua CLI and the LÖVE
GUI in [`../luacli/`](../luacli) are not reimplementations; they are this code,
reached through `ffi`.

## The C ABI

Six functions and one data type, declared in
[`ffi/include/causewaybay.h`](ffi/include/causewaybay.h):

```c
int   cwb_abi_version(void);
char *cwb_version(void);
char *cwb_describe(void);
char *cwb_commands(void);
char *cwb_execute(const char *request_json);
void  cwb_string_free(char *s);
```

A request is the argument vector plus the few things that would otherwise be
global state; a reply is the envelope from the spec, with the human rendering
alongside it:

```jsonc
// in
{"argv": ["account", "list"], "home": "/tmp/w", "network": null,
 "yes": false, "stdin": null}
// out
{"ok": true, "data": [...], "human": "* main  0x9858…  mnemonic"}
```

Keeping the command surface inside the request — rather than exposing a C
function per command — is what lets the wallet grow without the header moving.
It also means a front end in another language does not re-parse arguments and so
cannot drift from `cwbwallet`; the Lua CLI is 160 lines and has no argument
parser at all.

`cwb_commands` reads the command tree out of clap rather than out of a list
someone wrote down, so it cannot fall behind the commands that exist. Each leaf
carries its `path`, its `about`, and its arguments with `long`, `short`,
`positional`, `takes_value`, `required` and `default`. Two callers want that: a
GUI, which can build a form from it instead of hardcoding one per command, and a
binding's test suite, which asserts it has a method for every entry — that is
what makes "the whole surface is exposed" a fact the build checks rather than a
claim a README makes.

Four properties the FFI holds to, each with a test:

* **Nothing unwinds.** A panic crossing into LuaJIT is undefined behaviour, so
  every entry point catches one and reports `{"ok":false,…,"code":"internal"}`.
* **Nothing blocks on a terminal.** There is no stdin to read; `-` resolves from
  the request or fails.
* **A null or non-UTF-8 pointer is an error envelope**, not a crash.
* **Every returned string is the caller's**, freed with `cwb_string_free` and
  nothing else.

`cwb_abi_version` is the handshake. A host that loaded the library at runtime
compares it against the number it was built for and refuses a mismatch rather
than guessing at an envelope shape it does not know.

## Design notes

**Cryptography is implemented here, not delegated.** BIP-39 mnemonic encoding,
PBKDF2 seed derivation, BIP-32 child key derivation and BIP-44 pathing are all in
`core/src/bip39.rs` and `core/src/bip32.rs`, checked against the official Trezor
and BIP-32 test vectors. `k256` provides the curve arithmetic and
`alloy-primitives` the keccak/address types; everything above that is this
crate's own. RLP encoding for legacy transactions is a 40-line module
(`core/src/rlp.rs`) with its own vectors, and the resulting signed transactions
are asserted byte-for-byte against a reference signer.

**Layers.**

| module | responsibility |
| ------ | -------------- |
| `core::bip39`, `bip32`, `wallet` | key material: mnemonics, derivation, addresses, EIP-191 |
| `core::rlp`, `tx` | legacy transaction encoding and EIP-155 signing |
| `core::store` | the append-only JSONL logs and their replay |
| `core::network`, `rpc`, `erc20` | endpoints, JSON-RPC, ABI codec |
| `core::command`, `app`, `output` | the argument tree, command implementations, rendering |
| `core::host`, `request`, `api` | the boundary: who answers, what was asked, JSON in and out |
| `cli::tui`, `terminal`, `clipboard` | the ratatui front end and the terminal behaviours core lacks |

Commands return a `CommandOutput` carrying both the structured data and its human
rendering, so `--json` is a rendering choice rather than a separate code path.

**The wallet is also a calculator.** `utils derive`, `utils sign` and
`utils validate-mnemonic` take key material as an argument, compute, and store
nothing — no account, no recall entry. They exist because the alternatives have
side effects a caller may not want: `account import-mnemonic` keeps the phrase,
`sign` needs a stored account, and importing an invalid phrase is an error where
what was wanted was an answer.

**Errors** carry a stable `Code` that maps to the shared vocabulary in the spec;
`main` turns it into an exit status and, in JSON mode, an error envelope.

**The command tree lives in `core`, not in `cli`.** It is not a terminal concern
— it is the wallet's vocabulary, and there is exactly one definition of what
`account new --words 24` means. clap parses it for the binary and for anything
calling in over the ABI alike.

## Testing

```bash
cargo test --workspace              # everything
cargo test -p causewaybay-core      # the wallet's own unit tests
cargo test -p causewaybay-ffi       # the C ABI, called as a C host would
cargo test --test vectors           # the shared test vectors
cargo test --test cli_rpc           # chain-facing end-to-end tests
```

`cli/tests/common/mod.rs` provides an isolated wallet home plus `MockRpc`, a real
HTTP server that answers scripted JSON-RPC responses — so the send path is
tested end to end, through argument parsing, signing and broadcast, without
touching a network.

`core/tests/vectors.rs` reads [`../testvectors/`](../testvectors), the same files
the Python and Lua suites read. `make test-vector-coverage` from the repository
root proves each suite really reads them, by corrupting one value per file and
requiring every suite to notice.
