# Causewaybay Wallet — Shared Specification

Both implementations (`rustcli/`, `pythoncli/`) are byte-compatible: they read and
write the same on-disk store, expose the same command surface, and emit the same
JSON envelope. A store written by the Rust CLI can be driven by the Python CLI and
vice versa.

Sections 1–7 are what an implementation must do. Section 8 describes the C ABI
the Rust implementation additionally exposes, which is not a third
implementation but a way of reaching the first one from another language —
`luacli/` is built on it.

## 1. Storage

Root directory (the "home"), in precedence order:

1. `--home <PATH>` command line flag
2. `CAUSEWAYBAY_HOME` environment variable
3. `~/.causewaybaywallet`

The directory is created on demand with mode `0700`; every file inside is created
with mode `0600`.

```
~/.causewaybaywallet/
├── accounts.jsonl     append-only account event log
├── config.jsonl       append-only settings event log
├── history.jsonl      append-only transaction log
└── recent.jsonl       append-only log of key material the wallet has seen
```

### 1.1 JSONL rules

* One compact JSON object per line, UTF-8, `\n` terminated. No trailing spaces.
* Files are **append-only**. State is derived by replaying every line in order;
  later events supersede earlier ones. Nothing is ever rewritten in place, so a
  crash can at worst lose the last (partial) line.
* Every record carries `schema` (currently `1`), `type`, and an RFC3339 UTC
  timestamp.
* A malformed or unparsable line is skipped with a warning rather than aborting
  the replay. A line whose `schema` is greater than the reader's is skipped.

### 1.2 `accounts.jsonl`

| type              | fields                                                                                                        |
| ----------------- | ------------------------------------------------------------------------------------------------------------- |
| `account.create`  | `id`, `label`, `address`, `source` (`mnemonic`\|`private_key`), `private_key`, `mnemonic?`, `derivation_path?`, `index?`, `created_at` |
| `account.rename`  | `id`, `label`, `updated_at`                                                                                     |
| `account.delete`  | `id`, `deleted_at`                                                                                              |

* `id` — `acc_` followed by the first 8 bytes of
  `keccak256("<lowercase address>|<created_at>|<label>")`, hex encoded; stable for
  the life of the account. The label is part of the preimage so that the same
  address stored twice in one millisecond still gets distinct ids.
* `label` — unique, non-empty, `[A-Za-z0-9._-]{1,64}`. Auto-assigned as
  `account-<n>` when omitted.
* `address` — EIP-55 checksummed.
* `private_key` — `0x`-prefixed, 64 hex characters.
* `mnemonic` / `derivation_path` / `index` — present only when `source` is
  `mnemonic`.

### 1.3 `config.jsonl`

| type         | fields                          |
| ------------ | ------------------------------- |
| `config.set` | `key`, `value`, `updated_at`    |

Recognised keys:

| key                     | value                                                    |
| ----------------------- | -------------------------------------------------------- |
| `network`               | `cronos-testnet` \| `cronos-mainnet`                      |
| `active_account`        | an account `id`                                           |
| `rpc.<network>`         | RPC URL override for that network                         |

### 1.4 `recent.jsonl`

A recall list so a returning user can pick a mnemonic or private key they have
used before instead of retyping it. Every `account new`, `account import-*` and
`account derive` records the key material it used.

| type               | fields                                                                            |
| ------------------ | ---------------------------------------------------------------------------------- |
| `secret.remember`  | `id`, `kind` (`mnemonic`\|`private_key`), `secret`, `address`, `word_count?`, `first_seen_at`, `last_used_at`, `uses` |
| `secret.forget`    | `id`, `deleted_at`                                                                   |

* `id` — `sec_` followed by the first 8 bytes of
  `keccak256("<kind>|<normalised secret>")`, hex encoded. Re-using the same
  mnemonic therefore updates the existing entry rather than adding a duplicate.
* `address` — the address at index 0, so an entry is recognisable without
  revealing the secret.
* `uses` — how many times this material has been used; `last_used_at` drives the
  ordering, newest first.

Entries are addressed by id, by 1-based position in the list (`1` is the most
recent), or by address.

### 1.5 `history.jsonl`

| type        | fields                                                                                                                          |
| ----------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `tx.send`   | `hash`, `from`, `to`, `value_wei`, `value`, `network`, `chain_id`, `nonce`, `gas_limit`, `gas_price_wei`, `status`, `token?`, `created_at` |
| `tx.update` | `hash`, `status`, `block_number?`, `gas_used?`, `updated_at`                                                                          |

`status` is one of `submitted`, `confirmed`, `failed`.

## 2. Networks

| key              | name           | chain id | symbol | RPC                        | explorer                            |
| ---------------- | -------------- | -------- | ------ | -------------------------- | ----------------------------------- |
| `cronos-testnet` | Cronos Testnet | 338      | TCRO   | `https://evm-t3.cronos.org` | `https://explorer.cronos.org/testnet` |
| `cronos-mainnet` | Cronos Mainnet | 25       | CRO    | `https://evm.cronos.org`    | `https://explorer.cronos.org`         |

The default network is `cronos-testnet`. RPC resolution order:
`CAUSEWAYBAY_RPC_<NETWORK_KEY_UPPER_SNAKE>` env var → `rpc.<network>` config key →
the built-in default.

## 3. Key derivation

* BIP-39 English mnemonics, 12/15/18/21/24 words, empty passphrase by default.
* Mnemonics are compared NFKD-normalised, whitespace-collapsed and lowercased.
* BIP-44 path `m/44'/60'/0'/0/<index>`. A wallet normally holds a single
  mnemonic, and its addresses run 0, 1, 2, … along that path; a second mnemonic
  starts its own sequence at 0.
  Exports name this column `address_index` and carry a `seed` column beside it,
  so rows sharing a mnemonic are visible as such.
* Addresses are rendered EIP-55 checksummed everywhere.

Both implementations are tested against the shared fixtures in `testvectors/`,
which carry the official BIP-39 and EIP-55 vectors, the worked example from
EIP-155, and the mnemonics and keys published by Anvil, Hardhat and Ganache.

## 4. Output envelope

Human output is the default. `--json` switches every command to a single-line
envelope on stdout:

```json
{"ok":true,"data": ...}
{"ok":false,"error":{"code":"account_not_found","message":"no account matching 'bob'"}}
```

Exit codes: `0` success, `1` handled error, `2` usage error.

Error codes: `usage`, `not_found`, `account_not_found`, `duplicate_label`,
`invalid_mnemonic`, `invalid_private_key`, `invalid_address`, `invalid_amount`,
`no_active_account`, `unknown_network`, `rpc_error`, `insufficient_funds`,
`confirmation_required`, `io_error`, `internal`.

Secrets (`private_key`, `mnemonic`) are **never** included in output unless the
command is `account export`, or `--secret` is given to `account show` or
`recent show`.

## 5. JSON-RPC

Plain HTTP JSON-RPC 2.0, `POST`, `content-type: application/json`. Methods used:
`eth_chainId`, `eth_blockNumber`, `eth_getBalance`, `eth_getTransactionCount`,
`eth_gasPrice`, `eth_estimateGas`, `eth_call`, `eth_sendRawTransaction`,
`eth_getTransactionByHash`, `eth_getTransactionReceipt`.

## 6. Transactions

Native sends are legacy (type `0x0`) transactions signed with EIP-155 replay
protection: `nonce`, `to`, `value`, `gas` (default 21000), `gasPrice` (from
`eth_gasPrice`, overridable), `chainId`.

ERC-20 transfers call `transfer(address,uint256)` (`0xa9059cbb`) with a gas limit
from `eth_estimateGas` plus a 25 % headroom, falling back to 100000.

Both refuse a recipient equal to the sending account with code `usage`, before
any node is asked: the transfer would move nothing and still pay the gas, and
the sender's own address is the one most likely to have been pasted by mistake.
The comparison is case-insensitive, because EIP-55 is a property of the text.

## 7. Message signing

EIP-191 personal messages: `keccak256("\x19Ethereum Signed Message:\n" + len + msg)`,
signed with secp256k1; signatures are `0x` + 65 bytes (r‖s‖v) with `v` in {27,28}.

## 8. The C ABI

`rustcli/ffi/` exposes the Rust implementation as a shared library
(`libcausewaybay_ffi.{dylib,so}`, `causewaybay_ffi.dll`) whose header is
`rustcli/ffi/include/causewaybay.h`. It is an embedding surface, not a second
specification: everything above still holds, because the code behind it is the
same code the `cwbwallet` binary runs.

```c
int   cwb_abi_version(void);
char *cwb_version(void);
char *cwb_describe(void);
char *cwb_commands(void);
char *cwb_execute(const char *request_json);
void  cwb_string_free(char *s);
```

`cwb_commands` reports the command tree — one entry per leaf, with `path`,
`about`, and an `args` list carrying `long`, `short`, `positional`,
`takes_value`, `required` and `default`. It is read out of the argument parser
rather than written down, so it is always the surface §8.1 accepts.

### 8.1 The request

`cwb_execute` takes a NUL-terminated JSON object. Unknown fields are rejected
rather than ignored, so a misspelled `yes` cannot silently disable a
confirmation.

| field | type | meaning |
| --- | --- | --- |
| `argv` | array of strings | the command, without the program name |
| `home` | string or null | the wallet home; else `$CAUSEWAYBAY_HOME`, else `~/.causewaybaywallet` |
| `network` | string or null | the network for this call only |
| `yes` | boolean | answer confirmations with yes |
| `stdin` | string or null | what an argument written as `-` stands for |

`home`, `network` and `yes` are defaults: a flag inside `argv` wins, except
`yes`, which is a floor — a caller that has already asked cannot have its answer
overridden by an argument.

### 8.2 The reply

The envelope of §4, plus the human rendering the CLI would have printed:

```json
{"ok":true,"data": ..., "human":"* main  0x9858…  mnemonic"}
{"ok":false,"error":{"code":"account_not_found","message":"no account matching 'bob'"}}
```

`human` exists so a front end in another language prints what `cwbwallet`
prints without knowing its formatting rules. It is **not** part of §4: a CLI
built on this ABI must drop it before emitting a `--json` envelope, or its
output will differ from the two implementations'.

### 8.3 Guarantees

* Every returned `char *` belongs to the caller and is released with
  `cwb_string_free`. Passing null to it is a no-op.
* No entry point unwinds. A panic inside the wallet is reported as an envelope
  with code `internal`; a null or non-UTF-8 request as one with code `usage`.
* Nothing reads standard input or writes to a terminal. An argument of `-`
  resolves from the request's `stdin` field or fails with `usage`.
* A confirmation that was not pre-answered fails with `confirmation_required`.
  A library cannot prompt, so it refuses rather than assumes.
* `tui` is refused with `usage`. A library does not seize its host's screen.

### 8.4 Versioning

`cwb_abi_version` returns the version of *this contract*, currently `1`, and is
bumped when the request or envelope shape changes incompatibly. Within one
version the functions never change meaning, but the list may grow — and a host
that declares a function an older library does not export fails when it first
calls it, not when it loads, which is the other reason to check this number. A host that
loads the library at runtime must compare it against the number it was written
for and refuse a mismatch rather than guess. The wallet version — the number of
§ "Versioning" in the README — is separate and moves independently;
`cwb_version` and `cwb_describe` report it.
