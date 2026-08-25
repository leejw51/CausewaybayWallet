---
name: causewaybay-wallet
description: Educational Cronos/EVM wallet CLI with matching Rust, Python and Lua front ends. Use to create and manage HD wallets, recall previously used mnemonics and private keys, derive addresses, check balances and nonces, send native CRO/TCRO and ERC-20 tokens, sign and verify EIP-191 messages, look up transactions, and run offline crypto utilities on Cronos testnet and mainnet. State lives in ~/.causewaybaywallet as append-only JSONL.
---

# Causewaybay Wallet

A Cronos EVM wallet you drive from the command line. Three interchangeable
front ends share one on-disk format, so any of them can operate a wallet
another created.

⚠️ **Educational software.** Private keys are stored unencrypted on disk. Use it
on the testnet, and never with funds anyone would miss.

## Invoking it

| Front end | Command |
| --------- | ------- |
| Rust | `rustcli/target/debug/cwbwallet` (build with `make -C rustcli build`) |
| Python | `pythoncli/.venv/bin/python -m causewaybay` (set up with `make -C pythoncli test`) |
| Lua | `luacli/bin/cwbwallet-lua` (needs LuaJIT; build with `make -C luacli build`) |

All three accept identical arguments, because there is one wallet: the argument
tree, the four chains and the store are defined once in Rust, and the Python and
Lua front ends call into it through its C ABI rather than re-implementing any of
it. The examples below use `cwbwallet`. Only the Rust one has `tui`; the other
two have `interactive`, a menu at one prompt.

## Always pass `--json`

Every command supports `--json`, which replaces human text with a single line on
stdout:

```json
{"ok":true,"data":{"address":"0x9858…","balance":"12.5"}}
{"ok":false,"error":{"code":"insufficient_funds","message":"balance 0.01 TCRO cannot cover …"}}
```

Exit codes: `0` success, `1` handled error, `2` bad command line. Branch on
`error.code`, never on the message text. The codes are: `usage`, `not_found`,
`account_not_found`, `duplicate_label`, `invalid_mnemonic`,
`invalid_private_key`, `invalid_address`, `invalid_amount`, `no_active_account`,
`unknown_network`, `rpc_error`, `insufficient_funds`, `confirmation_required`,
`io_error`, `internal`.

## Rules that matter

1. **Anything that spends or destroys requires `--yes`.** `send`, `erc20 send`,
   `account remove`, `recent forget` and `recent clear` return
   `confirmation_required` without it. There is no prompt in `--json` mode — that
   is deliberate, so an automated caller cannot spend funds by accident. Confirm
   with the user in your own words *before* passing `--yes`.
2. **Default to the testnet.** `cronos-testnet` is the default. Only touch
   `cronos-mainnet` when the user has clearly asked for real funds, and say so.
3. **Never print secrets unless asked.** `account export`, `account show
   --secret` and `recent show --secret` are the only commands that reveal a
   mnemonic or private key. Do not echo them into a summary.
4. **Never send a secret as a command line argument in a shared shell.** Prefer
   `-m -` / `-k -` (read from stdin) or the `CAUSEWAYBAY_MNEMONIC` /
   `CAUSEWAYBAY_PRIVATE_KEY` environment variables.
5. **Check before you send.** Run `balance` and `chain-info` first; the wallet
   also refuses a transfer it can see the balance cannot cover, and refuses a
   recipient equal to the sending account (code `usage`) — that moves nothing
   and still pays the gas.

## Accounts

```bash
cwbwallet --json account new --label main                # next address of the wallet's seed
cwbwallet --json account new --new-seed --words 24       # start a separate mnemonic
cwbwallet --json account new --index 7                   # a specific address index
cwbwallet --json account import-mnemonic -m - --label me # mnemonic on stdin
cwbwallet --json account import-key -k - --label cold    # private key on stdin
cwbwallet --json account list                            # `active: true` marks the default
cwbwallet --json account show main                       # public fields only
cwbwallet --json account show main --secret              # includes the mnemonic
cwbwallet --json account use main                        # change the default account
cwbwallet --json account derive --index 1 --label second # another address, same mnemonic
cwbwallet --json account rename old new
cwbwallet --json --yes account remove old
cwbwallet --json account export main                     # print the secrets
```

Accounts are addressed by label, id (`acc_…`), or address — all
case-insensitive. The first account created becomes the active one; later ones
do not steal it.

**A wallet holds one mnemonic and many addresses derived from it.** `account
new` walks that seed: 0, 1, 2, 3, … It only mints a phrase when the wallet has
none, or when `--new-seed` says to. The response carries `new_seed: true/false`
so you can tell which happened. `account derive --index N` is the same thing
with the index chosen explicitly.

## Recalling key material the user already used

Every `account new`, `account import-*` and `account derive` records the
mnemonic or private key it used, so a returning user can pick from a list
instead of retyping a phrase.

```bash
cwbwallet --json recent list                       # newest first, secrets hidden
cwbwallet --json recent list --kind mnemonic --limit 5
cwbwallet --json recent show 1                     # preview only ("abandon … about")
cwbwallet --json recent show 1 --secret            # reveal it
cwbwallet --json account import-recent 1 --index 0 --label restored
cwbwallet --json --yes recent forget 1
cwbwallet --json --yes recent clear
```

Entries are addressed by 1-based position (`1` is the most recent), by id
(`sec_…`), or by address. Re-using the same phrase bumps its `uses` counter and
moves it to the front rather than adding a duplicate.

**Offer this first.** When a user asks to set up a wallet, check `recent list`
before generating anything new — they may simply want an earlier wallet back.

## Networks

```bash
cwbwallet --json network list                                  # both networks, current flagged
cwbwallet --json network current
cwbwallet --json network use cronos-mainnet                    # change the stored default
cwbwallet --json -n mainnet balance                            # override for one command
cwbwallet --json network set-rpc testnet https://my-node:8545  # empty URL restores the default
```

| key | chain id | symbol | explorer |
| --- | -------- | ------ | -------- |
| `cronos-testnet` (default) | 338 | TCRO | https://explorer.cronos.org/testnet |
| `cronos-mainnet` | 25 | CRO | https://explorer.cronos.org |

Aliases `testnet` and `mainnet` work everywhere a network is named.

## Reading the chain

```bash
cwbwallet --json balance                       # active account
cwbwallet --json balance --address 0xabc…      # any address
cwbwallet --json nonce
cwbwallet --json gas-price
cwbwallet --json chain-info                    # verifies the node's chain id matches
```

`chain-info` returns `chain_id_matches: false` when the RPC endpoint is serving a
different chain than expected — worth checking before a mainnet send.

## Sending

```bash
cwbwallet --json --yes send --to 0xabc… --amount 1.5
cwbwallet --json --yes send --to 0xabc… --amount 1.5 --wait          # wait for the receipt
cwbwallet --json --yes send --to 0xabc… --amount 1.5 \
  --gas-price-gwei 5 --gas-limit 21000 --nonce 7                     # optional overrides
cwbwallet --json tx 0xhash…                                          # look one up on chain
cwbwallet --json history --limit 10 --network testnet                # what this wallet sent
```

Amounts are decimal strings in whole tokens (`1.5`), never wei. Without `--wait`
the result is `status: "submitted"`; with it, `confirmed` or `failed` plus the
block number.

## ERC-20 tokens

```bash
cwbwallet --json erc20 info --token 0xtoken…
cwbwallet --json erc20 balance --token 0xtoken… [--address 0xabc…]
cwbwallet --json --yes erc20 send --token 0xtoken… --to 0xabc… --amount 25
```

Amounts are in whole tokens; the wallet reads `decimals()` and scales for you.

## Signing

```bash
cwbwallet --json sign "message to sign"
cwbwallet --json sign -                                    # message on stdin
cwbwallet --json verify --message "…" --signature 0x… --address 0x…
```

EIP-191 personal messages. `verify` returns `valid` plus the `recovered` address;
without `--address` it just reports who signed.

## Saving the wallet list

```bash
cwbwallet --json account list --format csv                    # to stdout, in data.content
cwbwallet --json account list --format md -o wallets.md       # to a file
cwbwallet --json account list --format jsonl --secret         # includes keys — see rule 3
```

Formats: `jsonl`, `csv`, `txt`, `md` — all carrying the same columns. Without
`--format` the command behaves as before.

| column | |
| ------ | - |
| `position` | 1-based row number |
| `label`, `address`, `source` | |
| `address_index`, `seed`, `derivation_path` | see below |
| `created_at`, `active` | |
| `public_key_compressed` | 33 bytes, hex — a parity prefix and X |
| `public_key` | 64 bytes, hex — X‖Y, the SEC1 `0x04` tag dropped |

`--secret` appends `private_key` and `mnemonic` and writes the file owner-only.
Public keys are not secrets, so they are present either way; the long two sit at
the end of each row so the readable columns stay on the left.

`address_index` is the BIP-44 index **within one mnemonic**, not a position in
the list — every generated wallet has its own seed and so starts again at 0.
`seed` is the recall id of that mnemonic, so rows sharing a phrase share a seed;
it is blank for a wallet imported from a bare private key.

## Offline utilities

```bash
cwbwallet --json utils keccak "hello" [--hex]
cwbwallet --json utils checksum 0xabc…            # EIP-55
cwbwallet --json utils to-wei 1.5 [--decimals 6]
cwbwallet --json utils from-wei 1500000000000000000
cwbwallet --json utils new-mnemonic --words 24    # generated, not stored
cwbwallet --json info                             # where state lives, what is configured
```

### Crypto without touching the wallet

These take key material as an argument and store nothing — no account, no
recall entry. Reach for them when the side effects of the wallet commands are
not wanted.

```bash
# Derive an address and keys. One of -m/-k, never both.
cwbwallet --json utils derive -m "abandon … about" [-i 3] [--passphrase ""]
cwbwallet --json utils derive -k 0x1ab42c…
#   -> {address, private_key, public_key, public_key_compressed, source,
#       derivation_path?, index?}

# Sign with a key the wallet does not hold. `sign` needs a stored account;
# this one does not.
cwbwallet --json utils sign -k 0x1ab42c… -m "hello"
#   -> {address, message, signature}

# Ask about a phrase instead of being refused it: an invalid mnemonic is an
# answer here, where `account import-mnemonic` fails with invalid_mnemonic.
cwbwallet --json utils validate-mnemonic "abandon abandon"
#   -> {valid: false, words: 2, reason: "unsupported word count 2; …"}
```

Every one of these accepts `-` in place of the value to read it from stdin,
which keeps a phrase or a key out of the process list.

## Where state lives

`~/.causewaybaywallet/` (override with `--home PATH` or `CAUSEWAYBAY_HOME`),
holding four append-only JSONL logs: `accounts.jsonl`, `config.jsonl`,
`history.jsonl`, `recent.jsonl`. Nothing is ever rewritten in place — state is
the fold of every line — so the files are safe to read directly when you want to
inspect what happened. The directory is `0700` and the files `0600`.

## Interactive use

`cwbwallet tui` opens a terminal UI with the same capabilities. It shows a
command list on the left, so nothing has to be memorised: `Tab` moves between
panes, `↑↓` selects, `Enter` runs, and `?` opens a full reference. Every command
also has a single key — `b` balance, `s` send, `n` next address, `m` import mnemonic, `p`
import key, `N` new seed, `c` recall, `d` derive, `a` activate, `y` copy the
address to the
clipboard, `g` sign, `1`–`4` save the wallet list as jsonl/csv/txt/md, `v`
secrets, `w` network, `x` remove, `q` quit.

Mnemonic and private-key prompts are masked: the TUI shows `•••••••••••• (12
words)` rather than the phrase, so a pasted seed never reaches the terminal
buffer or a session recording.

Do not launch it from an automated session — it takes over the terminal.
