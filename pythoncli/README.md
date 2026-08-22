# Causewaybay Wallet — Python

The Python implementation of the wallet specified in [`../SPEC.md`](../SPEC.md).
Installs a `cwbwallet` console script; `python -m causewaybay` works too.

```bash
make            # list the targets
make install    # create .venv and install the package
make test       # the full test suite
make coverage   # tests with a coverage report
make tui        # launch the terminal UI
make check      # ruff and tests, as CI runs them
make package    # standalone binary into ../dist/cwbwallet-python
```

`make package` builds a wheel and hands it to [PyApp](https://ofek.dev/pyapp/),
which embeds it together with a redistributable CPython in one executable. That
is why packaging a Python program here needs a Rust toolchain, and why the result
is ~17 MB rather than ~40 KB. The interpreter travels with the binary; the
third-party dependencies do not — the first run pip-installs them into
`~/Library/Application Support/pyapp` (needs network, takes about ten seconds),
and every run after that starts in well under a second.

## Design notes

**Standards come from established libraries.** `mnemonic` handles BIP-39 and
`eth_account` handles BIP-32/44 derivation and transaction signing — the mirror
image of the Rust side, which implements them directly. Both are checked against
the same official vectors and against each other by `../scripts/parity.sh`, so
the two approaches corroborate rather than duplicate.

**Layers**, matching the Rust modules one-for-one:

| module | responsibility |
| ------ | -------------- |
| `wallet` | key material: mnemonics, derivation, addresses, EIP-191 |
| `txs` | legacy transaction construction and EIP-155 signing |
| `store` | the append-only JSONL logs and their replay |
| `networks`, `rpc`, `erc20` | endpoints, JSON-RPC, ABI codec |
| `cli`, `app`, `output` | argument parsing, command implementations, rendering |
| `tui` | the Textual front end, built on the same `App` |

Commands return a `CommandOutput` carrying both the structured data and its human
rendering, so `--json` is a rendering choice rather than a separate code path.

**Errors** are `WalletError` with a stable `code` from the shared vocabulary;
`cli.main` turns it into an exit status and, in JSON mode, an error envelope.

## Testing

```bash
.venv/bin/pytest                    # everything
.venv/bin/pytest tests/test_tui.py  # the Textual UI, through its test pilot
.venv/bin/pytest -k recall          # one area
```

`tests/conftest.py` gives every test an isolated wallet home, scrubs the
developer's own environment variables, and provides `MockRpc` — a real HTTP
server answering scripted JSON-RPC — so the send path is exercised end to end
without touching a network.
