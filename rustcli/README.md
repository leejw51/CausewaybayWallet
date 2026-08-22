# Causewaybay Wallet — Rust

The Rust implementation of the wallet specified in [`../SPEC.md`](../SPEC.md).
Builds a single binary, `cwbwallet`, with both the CLI and the TUI.

```bash
make          # list the targets
make build    # debug binary at target/debug/cwbwallet
make test     # unit and integration tests
make tui      # launch the terminal UI
make check    # fmt, clippy and tests, as CI runs them
make package  # signed release binary into ../dist/cwbwallet-rust
```

Packaging is just the release build plus a copy and a signature — a Rust binary
carries no runtime with it, which is the whole difference from `pythoncli/`.

## Design notes

**Cryptography is implemented here, not delegated.** BIP-39 mnemonic encoding,
PBKDF2 seed derivation, BIP-32 child key derivation and BIP-44 pathing are all in
`src/bip39.rs` and `src/bip32.rs`, checked against the official Trezor and BIP-32
test vectors. `k256` provides the curve arithmetic and `alloy-primitives` the
keccak/address types; everything above that is this crate's own. RLP encoding for
legacy transactions is a 40-line module (`src/rlp.rs`) with its own vectors, and
the resulting signed transactions are asserted byte-for-byte against a reference
signer.

**Layers.**

| module | responsibility |
| ------ | -------------- |
| `bip39`, `bip32`, `wallet` | key material: mnemonics, derivation, addresses, EIP-191 |
| `rlp`, `tx` | legacy transaction encoding and EIP-155 signing |
| `store` | the append-only JSONL logs and their replay |
| `network`, `rpc`, `erc20` | endpoints, JSON-RPC, ABI codec |
| `cli`, `app`, `output` | argument parsing, command implementations, rendering |
| `tui` | the ratatui front end, built on the same `App` |

Commands return a `CommandOutput` carrying both the structured data and its human
rendering, so `--json` is a rendering choice rather than a separate code path.

**Errors** carry a stable `Code` that maps to the shared vocabulary in the spec;
`main` turns it into an exit status and, in JSON mode, an error envelope.

## Testing

```bash
cargo test                 # everything
cargo test --lib           # unit tests only
cargo test --test cli_rpc  # chain-facing end-to-end tests
```

`tests/common/mod.rs` provides an isolated wallet home plus `MockRpc`, a real
HTTP server that answers scripted JSON-RPC responses — so the send path is
tested end to end, through argument parsing, signing and broadcast, without
touching a network.
