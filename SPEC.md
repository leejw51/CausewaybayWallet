# Causewaybay Wallet — Shared Specification

One implementation (`rustcli/`), reached four ways. This document is what that
core does: the on-disk store, the command surface and the JSON envelope every
front end returns. `pythoncli/`, `luacli/` and `ccli/` are bindings over its C
ABI rather than second implementations, so a store written through any of them
can be driven through any other.

> **Multi-chain, as of ABI 2.** The wallet holds accounts on four chains: `evm`
> (Cronos), `solana`, `cardano` and `midnight`. Everything below that does not
> name a chain applies to all of them. A record written before this — an account
> with no `chain` field — is an EVM account, so an existing store replays
> unchanged and needs no migration.
>
> Every front end has all four, because there is one implementation of them.

Sections 1–7 are what the implementation does. Section 8 describes the C ABI it
exposes — the way the other front ends reach it from another language.

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
| `account.create`  | `id`, `label`, `address`, `chain`, `source` (`mnemonic`\|`private_key`), `private_key`, `mnemonic?`, `derivation_path?`, `index?`, `created_at` |
| `account.rename`  | `id`, `label`, `updated_at`                                                                                     |
| `account.delete`  | `id`, `deleted_at`                                                                                              |

* `id` — `acc_` followed by the first 8 bytes of
  `keccak256("<lowercase address>|<created_at>|<label>")`, hex encoded; stable for
  the life of the account. The label is part of the preimage so that the same
  address stored twice in one millisecond still gets distinct ids.
* `label` — unique, non-empty, `[A-Za-z0-9._-]{1,64}`. Auto-assigned as
  `account<index>-<chain>` when omitted — `account0-evm`, `account0-solana` —
  so the name says which wallet and which chain. An account with no index (an
  imported private key), or a name already taken, falls back to `account-<n>`.
* `chain` — `evm`, `solana`, `cardano` or `midnight`. **Absent means `evm`**:
  every record written before the wallet was multi-chain is an EVM account.
* `address` — in the chain's own rendering: EIP-55 checksummed hex for `evm`,
  base58 for `solana`, bech32 for `cardano`, bech32m for `midnight`.
* `private_key` — in the chain's own encoding, and opaque to everything but
  that chain:

  | chain | form |
  | --- | --- |
  | `evm` | `0x`-prefixed, 64 hex characters |
  | `solana` | base58 of the 64-byte keypair |
  | `cardano` | 384 hex characters: the 96-byte payment extended key, then the 96-byte staking one |
  | `midnight` | 128 hex characters: the 32-byte night key, then its 32-byte DUST seed |

  Cardano and Midnight store two keys because one is not derivable from the
  other and both are needed: Cardano's address contains the staking credential,
  and Midnight's fees are paid from a key at a different role of the same path.
  A Midnight account imported as a bare 32-byte night key is accepted, can
  receive and sign, and is refused when it tries to pay a fee.
* `mnemonic` / `derivation_path` / `index` — present only when `source` is
  `mnemonic`. Index sequences are **per chain**: one mnemonic's Solana index 0
  and its Cardano index 0 are different keys, and neither advances the other.

### 1.3 `config.jsonl`

| type         | fields                          |
| ------------ | ------------------------------- |
| `config.set` | `key`, `value`, `updated_at`    |

Recognised keys:

| key                     | value                                                    |
| ----------------------- | -------------------------------------------------------- |
| `network`               | the wallet's overall selected network key                 |
| `network.<chain>`       | the selected network for one chain                        |
| `active_account`        | an account `id`                                           |
| `active_account.<chain>`| the account a command on that chain defaults to           |
| `rpc.<network>`         | endpoint override for that network                        |
| `submit.<network>`      | override for where transactions are submitted, where that differs from where reads come from |

A command on chain *C* uses `network.C` when it is set, else `network` if it
names a network of *C*, else *C*'s default. The account it acts on is the
overall active account when that is already on *C*, else `active_account.C`,
else the first account on *C* — so `--chain solana` lands somewhere sensible
without an `account use` first.

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
| `tx.send`   | `hash`, `from`, `to`, `value_wei`, `value`, `chain`, `network`, `chain_id`, `nonce`, `gas_limit`, `gas_price_wei`, `status`, `token?`, `created_at` |
| `tx.update` | `hash`, `status`, `block_number?`, `gas_used?`, `updated_at`                                                                          |

`status` is one of `submitting`, `submitted`, `unconfirmed`, `confirmed`,
`failed`. A record is written *before* the broadcast, as `submitting`: if a node
accepts a transaction and the reply is then lost, the history still names it,
which is what stops the same transfer going out twice.

`chain` is absent on records written before the wallet was multi-chain, and
those are EVM. On chains with no per-account sequence or gas, `nonce` and
`gas_limit` are `0` and `gas_price_wei` holds the whole fee rather than a rate.

## 2. Networks

Every network belongs to exactly one chain, so naming a network settles the
chain too. Each chain's first row is its default.

| key                | chain      | chain id | symbol | decimals | endpoint                                             |
| ------------------ | ---------- | -------- | ------ | -------- | ---------------------------------------------------- |
| `cronos-testnet`   | `evm`      | 338      | TCRO   | 18       | `https://evm-t3.cronos.org`                           |
| `cronos-mainnet`   | `evm`      | 25       | CRO    | 18       | `https://evm.cronos.org`                              |
| `solana-devnet`    | `solana`   | —        | SOL    | 9        | `https://api.devnet.solana.com`                       |
| `solana-testnet`   | `solana`   | —        | SOL    | 9        | `https://api.testnet.solana.com`                      |
| `solana-mainnet`   | `solana`   | —        | SOL    | 9        | `https://api.mainnet-beta.solana.com`                 |
| `cardano-preprod`  | `cardano`  | —        | tADA   | 6        | `https://preprod.koios.rest/api/v1`                   |
| `cardano-preview`  | `cardano`  | —        | tADA   | 6        | `https://preview.koios.rest/api/v1`                   |
| `cardano-mainnet`  | `cardano`  | —        | ADA    | 6        | `https://api.koios.rest/api/v1`                       |
| `midnight-preview` | `midnight` | —        | NIGHT  | 6        | `https://indexer.preview.midnight.network/api/v4/graphql` |
| `midnight-devnet`  | `midnight` | —        | NIGHT  | 6        | `https://indexer.devnet.midnight.network/api/v4/graphql`  |

Only EVM networks have a chain id; it is the EIP-155 replay-protection number
and is omitted rather than faked for the others.

Midnight reads from an indexer and **submits to a different service**, its node
RPC (`https://rpc.<network>.midnight.network`). No other chain separates the
two.

The default network is `cronos-testnet`. Endpoint resolution order:
`CAUSEWAYBAY_RPC_<NETWORK_KEY_UPPER_SNAKE>` env var → `rpc.<network>` config key
→ the built-in default; and for the submission half,
`CAUSEWAYBAY_SUBMIT_<NETWORK_KEY_UPPER_SNAKE>` → `submit.<network>` → default.

### 2.1 Naming a network

A key is matched in full first. A bare name that only one chain uses resolves to
that chain's network — `preprod` means `cardano-preprod`. A bare name several
chains share is **refused** rather than guessed, because guessing sends funds to
the wrong chain; `--chain` disambiguates it.

The two exceptions are `testnet` and `mainnet`, which meant Cronos before the
wallet had other chains and still do. `--chain solana -n testnet` reaches
`solana-testnet`, because inside a chain a short name is unambiguous.

## 3. Key derivation

* BIP-39 English mnemonics, 12/15/18/21/24 words, empty passphrase by default.
* Mnemonics are compared NFKD-normalised, whitespace-collapsed and lowercased.
* A wallet normally holds a single mnemonic and derives every chain's addresses
  from it. Addresses run 0, 1, 2, … along each chain's own path, **per chain**;
  a second mnemonic starts its own sequences at 0. Exports name this column
  `address_index` and carry a `seed` column beside it, so rows sharing a
  mnemonic are visible as such.
* An export row is one account **on one network**, ordered by wallet index,
  then by chain, then by network, and named `account<index>-<network>`
  (`account0-cronos-testnet`) — or `account<index>-<chain>` where the chain
  ships a single exported network. EVM writes both Cronos networks; the other
  three chains are testnet-only until their mainnets are supported.

Each chain derives differently, and the differences are load-bearing — three of
the four disagree about what the mnemonic even turns into:

| chain | derives from | scheme | path | address |
| --- | --- | --- | --- | --- |
| `evm` | the BIP-39 seed | BIP-32, secp256k1 | `m/44'/60'/0'/0/<i>` | EIP-55 hex of keccak(pubkey)[12..] |
| `solana` | the BIP-39 seed | SLIP-0010, ed25519, **hardened only** | `m/44'/501'/<i>'/0'` | base58 of the raw public key |
| `cardano` | the BIP-39 **entropy** | Icarus/CIP-3 + BIP32-Ed25519 | `m/1852'/1815'/0'/0/<i>` | bech32 of blake2b-224 key hashes |
| `midnight` | the BIP-39 seed | BIP-32, secp256k1 → BIP-340 | `m/44'/2400'/0'/0/<i>` | bech32m of SHA-256(x-only pubkey) |

Three details an implementer will otherwise get wrong, each of which produces a
plausible, wrong, unfunded address rather than an error:

1. **Cardano hashes the entropy, not the seed**, and passes the passphrase as
   the PBKDF2 *password* with the entropy as the *salt*. That reads backwards
   and is what every Cardano wallet does.
2. **Solana's scheme is hardened-only.** `m/44'/501'/0'/0` — one apostrophe
   short — must be refused, not silently hardened.
3. **BIP-340 negates about half of all secret keys**, so the scalar a Midnight
   signing key reports is not always the one BIP-32 derived. The *derived*
   bytes are what is stored and exported, or roughly half of all indices
   disagree with the Midnight SDK.

Addresses are rendered in each chain's own form, and one that carries its
network — Cardano's header nibble, Midnight's bech32m prefix — is checked
against the network in play before a transfer, because crossing that line puts
the funds on a chain nobody is watching.

The core and its bindings are tested against the shared fixtures in `testvectors/`,
which carry the official BIP-39 and EIP-55 vectors, the worked example from
EIP-155, and the mnemonics and keys published by Anvil, Hardhat and Ganache.
`testvectors/multichain.json` carries the Solana, Cardano and Midnight
derivations, addresses and transaction encodings, generated with each chain's
official SDK.

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

Each chain signs in its own scheme, and the schemes differ in what verification
needs to be told:

| chain | scheme | verifying needs |
| --- | --- | --- |
| `evm` | EIP-191 personal message, secp256k1 | nothing — the signature recovers its signer |
| `solana` | ed25519 | the address (which *is* the public key) |
| `cardano` | ed25519 over the BIP32-Ed25519 payment key | key material — the address is only a hash |
| `midnight` | BIP-340 Schnorr, secp256k1 | key material — the address is only a hash |

EIP-191 is `keccak256("\x19Ethereum Signed Message:\n" + len + msg)`; signatures
are `0x` + 65 bytes (r‖s‖v) with `v` in {27,28}. The others are `0x` + 64 bytes.

Only EVM can name a signer from a signature alone. Where it cannot, `verify`
checks against an account this wallet holds — and says so rather than reporting
a signature invalid because it was handed something it could not use.

## 7.1 Chains

`chains` lists what this build supports and what each can do: its derivation
path, its networks, and a capability set (`faucet`, `tokens`, `gas_limit`,
`recoverable_signatures`). A front end builds a chain picker from it rather than
keeping a list that goes stale.

Commands that only one chain has are refused elsewhere with `usage`, naming the
chain and the flag: `erc20` is EVM only, `airdrop` needs a chain with a faucet,
`nonce` needs an account-based chain.

## 8. The C ABI

`rustcli/ffi/` exposes the wallet as a shared library
(`libcausewaybay_ffi.{dylib,so}`, `causewaybay_ffi.dll`) whose header is
`rustcli/ffi/include/causewaybay.h`. It is an embedding surface, not a second
specification: everything above still holds, because the code behind it is the
same code the `cwbwallet` binary runs.

```c
int   cwb_abi_version(void);
char *cwb_version(void);
char *cwb_describe(void);
char *cwb_chains(void);
char *cwb_commands(void);
char *cwb_execute(const char *request_json);
void  cwb_string_free(char *s);
```

`cwb_chains` reports the chains this build supports — `chain`, `name`,
`derivation_path`, `networks` and `capabilities` — so a host builds a chain
picker from the library rather than from its own copy of the list. The same
data is inside `cwb_describe` under `chains`. Added in ABI 2.

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
| `chain` | string or null | the chain for this call only: `evm`, `solana`, `cardano`, `midnight` |
| `yes` | boolean | answer confirmations with yes |
| `stdin` | string or null | what an argument written as `-` stands for |

`home`, `network`, `chain` and `yes` are defaults: a flag inside `argv` wins,
except `yes`, which is a floor — a caller that has already asked cannot have its
answer overridden by an argument.

Naming a network settles the chain too, so a host with a network picker never
needs to set `chain`. Naming both and having them disagree is refused rather
than resolved in either direction.

### 8.2 The reply

The envelope of §4, plus the human rendering the CLI would have printed:

```json
{"ok":true,"data": ..., "human":"* main  0x9858…  mnemonic"}
{"ok":false,"error":{"code":"account_not_found","message":"no account matching 'bob'"}}
```

`human` exists so a front end in another language prints what `cwbwallet`
prints without knowing its formatting rules. It is **not** part of §4: a CLI
built on this ABI must drop it before emitting a `--json` envelope, or its
output will differ from the Rust CLI's.

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

`cwb_abi_version` returns the version of *this contract*, currently `2`, and is
bumped when the request or envelope shape changes incompatibly. Version 2 added
the request's `chain` field, the `chain` key on account and history records, and
`cwb_chains`. Within one
version the functions never change meaning, but the list may grow — and a host
that declares a function an older library does not export fails when it first
calls it, not when it loads, which is the other reason to check this number. A host that
loads the library at runtime must compare it against the number it was written
for and refuse a mismatch rather than guess. The wallet version — the number of
§ "Versioning" in the README — is separate and moves independently;
`cwb_version` and `cwb_describe` report it.
