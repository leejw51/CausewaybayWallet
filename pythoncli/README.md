# Causewaybay Wallet — Python

A Python binding over the Rust core described in [`../SPEC.md`](../SPEC.md).
The wallet itself — key derivation for all four chains, the append-only store,
the RPC — is in [`../rustcli/`](../rustcli); this package loads it through its C
ABI and gives Python a wallet object over it.

Installs a `cwbwallet` console script; `python -m causewaybay` works too.

```bash
make            # list the targets
make build      # build the shared library this loads
make test       # the full test suite (builds the library first)
make check      # ruff and tests, as CI runs them
make interactive  # the menu and REPL at one prompt
make package    # standalone binary into ../dist/cwbwallet-python
```

## Using it

```python
from causewaybay import open_wallet

wallet = open_wallet()                    # or open_wallet(home=…, chain="solana")
wallet.new_account(every_chain=True)      # one wallet, four chains

for account in wallet.accounts():
    print(account["label"], account["chain"], account["address"])

print(wallet.balance()["balance"])
```

Every call raises `WalletError` on failure, with a `.code` from the stable
vocabulary in `SPEC.md` — `account_not_found`, `insufficient_funds`,
`confirmation_required` and the rest. A command that *failed* is still a value
if you ask for the envelope rather than the data:

```python
envelope = wallet.envelope(["account", "show", "nobody"])
envelope["ok"]              # False
envelope["error"]["code"]   # "account_not_found"
```

## Design notes

**There is no second implementation here.** `ffi.py` is the only file that
knows about the library: it finds it, refuses one that speaks a different ABI,
and frees every string the library hands back. Above that, `wallet.py` is
argument shaping and JSON, and `cli.py` does not parse the wallet's arguments at
all — it passes argv through and prints what comes back, so it cannot drift from
`cwbwallet` the way a hand-written second parser would.

| module | responsibility |
| ------ | -------------- |
| `ffi` | ctypes: finding the library, the ABI check, string ownership |
| `wallet` | the `Wallet` object: requests in, dicts out, `WalletError` on failure |
| `cli` | argv → request → stdout, and the exit statuses a terminal expects |
| `interactive` | the numbered menu and REPL, over the same binding |
| `errors` | `WalletError`, carrying the core's own code |

**Nothing is written down twice.** The command list, the error codes, the
networks and the chains are all read from the library — `wallet.commands()`,
`wallet.codes()`, `wallet.chains()` — so a chain or a command added in Rust
appears here without an edit. The one list this package does keep, `COMMANDS`,
maps each command to the method that covers it, and the test suite checks it
against what the library reports: adding a command in Rust and forgetting the
Python method fails the build.

**No third-party dependencies.** ctypes is in the standard library, and the
cryptography is the core's. `make package` stages the shared library inside the
wheel, so the packaged binary carries its own wallet and needs no network on
first run.

## Testing

```bash
.venv/bin/pytest                       # everything
.venv/bin/pytest tests/test_vectors.py # the shared vectors, through the ABI
.venv/bin/pytest -k chains             # one area
```

`tests/conftest.py` gives every test an isolated wallet home and scrubs the
developer's own environment variables, so nothing here can reach a real store.
The suites are:

* `test_wallet.py` — the binding: opening, the coverage map, accounts, chains,
  networks, and the request shaping.
* `test_vectors.py` — the shared fixtures in `../testvectors/`, driven through
  ctypes and the C ABI. That path is what this adds over the Rust suite: a
  truncated string or a number silently turned into a float shows up here.
* `test_cli.py` — the CLI end to end against a real store, including the exit
  statuses and the `--json` envelope.
