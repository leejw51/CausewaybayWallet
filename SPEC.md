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
* Files are **append-only**, with one exception below. State is derived by
  replaying every line in order; later events supersede earlier ones, so a crash
  can at worst lose the last (partial) line.
* **Removal rewrites.** `account remove`, `recent forget` and `recent clear`
  drop the records themselves rather than appending an `account.delete` or
  `secret.forget` tombstone: those records carry plaintext key material, and a
  tombstone only stops the replay showing it. The rewrite goes through a
  sibling temp file created `0600` and renamed over the original, so a reader
  sees either the old log or the new one. Lines the writer cannot parse, and
  lines from a newer `schema`, are copied through untouched. Tombstones written
  by an older binary are still honoured on replay.
  This removes the record from the file; it is not a guarantee of erasure from
  the underlying device.
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
`account derive` records the key material it used, and says so in its output —
this is a second plaintext copy of the phrase, and it is not a secret that it
exists.

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

| key                | chain      | chain id | symbol | decimals | tags                             | endpoint                                             |
| ------------------ | ---------- | -------- | ------ | -------- | -------------------------------- | ---------------------------------------------------- |
| `cronos-testnet`   | `evm`      | 338      | TCRO   | 18       | evm testnet smart-contracts erc20 | `https://evm-t3.cronos.org`                           |
| `cronos-mainnet`   | `evm`      | 25       | CRO    | 18       | evm smart-contracts erc20        | `https://evm.cronos.org`                              |
| `solana-devnet`    | `solana`   | —        | SOL    | 9        | svm testnet faucet spl           | `https://api.devnet.solana.com`                       |
| `solana-testnet`   | `solana`   | —        | SOL    | 9        | svm testnet faucet spl           | `https://api.testnet.solana.com`                      |
| `solana-mainnet`   | `solana`   | —        | SOL    | 9        | svm spl                          | `https://api.mainnet-beta.solana.com`                 |
| `cardano-preprod`  | `cardano`  | —        | tADA   | 6        | utxo testnet native-assets       | `https://preprod.koios.rest/api/v1`                   |
| `cardano-preview`  | `cardano`  | —        | tADA   | 6        | utxo testnet native-assets       | `https://preview.koios.rest/api/v1`                   |
| `cardano-mainnet`  | `cardano`  | —        | ADA    | 6        | utxo native-assets               | `https://api.koios.rest/api/v1`                       |
| `midnight-preview` | `midnight` | —        | NIGHT  | 6        | privacy testnet shielded zk      | `https://indexer.preview.midnight.network/api/v4/graphql` |
| `midnight-devnet`  | `midnight` | —        | NIGHT  | 6        | privacy testnet shielded zk      | `https://indexer.devnet.midnight.network/api/v4/graphql`  |

Only EVM networks have a chain id; it is the EIP-155 replay-protection number
and is omitted rather than faked for the others.

Midnight reads from an indexer and **submits to a different service**, its node
RPC (`https://rpc.<network>.midnight.network`). No other chain separates the
two.

The default network is `cronos-testnet`. Endpoint resolution order:
`CAUSEWAYBAY_RPC_<NETWORK_KEY_UPPER_SNAKE>` env var → `rpc.<network>` config key
→ the built-in default; and for the submission half,
`CAUSEWAYBAY_SUBMIT_<NETWORK_KEY_UPPER_SNAKE>` → `submit.<network>` → default.

### 2.1 Tags, and finding a row

A tag says what a row's **name does not**. Search already reads the key, the
name, the symbol and the chain, so `cronos-mainnet` carries no `cronos` tag and
no `mainnet` one; it carries `evm`, which appears nowhere else on the row. The
one deliberate exception is `testnet`, because `devnet`, `preprod` and `preview`
are test networks whose names never say so and "show me where I can lose
nothing" has to be one query. There is no matching `mainnet` tag: every mainnet
row is already called one.

The same matching rule serves the networks and the tokens, in every front end:

* An **empty query matches everything.** Filtering is opted into; a wallet that
  hid rows until you typed would have lost them rather than tidied them.
* A query splits on whitespace and commas into terms, and **every term must
  match** — `usdc cronos` is USDC *and* Cronos. Narrowing by adding words is
  the one habit every search box has taught.
* A term matches as a **substring** of any of the row's searchable fields —
  key, name, symbol, chain, tags, and a token's network. `net` finds
  `cronos-testnet`; `main` finds every mainnet.
* Case, and the `-`/`_`/space difference, are ignored, so `Cronos Mainnet`,
  `cronos-mainnet` and `CRONOS_MAINNET` are one query.

There is no ranking. These are tables of fixed rows where the user is looking
for one they can already name, and sorting the survivors by a score would only
move a row someone had learned the position of.

`network list [FILTER…]` applies it; `network list --tags` lists the tags
themselves, because a tag nobody can discover is a tag nobody uses.

### 2.2 Naming a network

A key is matched in full first. A bare name that only one chain uses resolves to
that chain's network — `preprod` means `cardano-preprod`. A bare name several
chains share is **refused** rather than guessed, because guessing sends funds to
the wrong chain; `--chain` disambiguates it.

The two exceptions are `testnet` and `mainnet`, which meant Cronos before the
wallet had other chains and still do. `--chain solana -n testnet` reaches
`solana-testnet`, because inside a chain a short name is unambiguous.

### 2.3 Moving to another chain's network

`network use` writes both `network` and `network.<chain>`, and moves the wallet
onto that chain: `active_account` is repointed at the chain's account, so the
next command does not run on the chain just left.

Where a wallet holds nothing on that chain, the missing accounts are **derived
first** — same phrase, same index, one per wallet the store already has. This
creates no new wallet: a wallet is one mnemonic and one index, and each chain's
address at that index exists whether or not the store has written it down. The
addresses written come back as `derived` in the command's data.

Two kinds of account are skipped, because for them the phrase alone would not
reproduce the wallet: one imported from a bare private key, which has no
mnemonic, and one made with a BIP-39 passphrase, which the store does not keep.
Such a wallet simply has no account on the new chain, and a front end must say
so rather than falling back to the chain it came from.

## 2.4 Tokens

A token is not a thing you hold; a token **on a network** is. USDC on Cronos and
USDC on Solana share a name, a peg and nothing else — different issuer,
different decimals, different bytes on the wire, and an address that means
nothing on the other chain. So the registry row is the pair, and it is named as
the pair: key `usdc-cronos-mainnet`, name **"USDC Cronos Mainnet"**. Naming a
row settles the chain, the network, the decimals and the on-chain id at once,
and there is never a second act of choosing.

| key                    | network           | standard        | dp | on-chain id                                                                        |
| ---------------------- | ----------------- | --------------- | -- | ---------------------------------------------------------------------------------- |
| `usdc-cronos-mainnet`  | `cronos-mainnet`  | `erc20`         | 6  | `0xc21223249CA28397B4B6541dfFaEcC539BfF0c59`                                        |
| `usdt-cronos-mainnet`  | `cronos-mainnet`  | `erc20`         | 6  | `0x66e428c3f67a68878562e79A0234c1F83c208770`                                        |
| `dai-cronos-mainnet`   | `cronos-mainnet`  | `erc20`         | 18 | `0xF2001B145b43032AAF5Ee2884e456CCd805F677D`                                        |
| `usdc-solana-mainnet`  | `solana-mainnet`  | `spl-token`     | 6  | `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v`                                      |
| `usdt-solana-mainnet`  | `solana-mainnet`  | `spl-token`     | 6  | `Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB`                                      |
| `usdc-solana-devnet`   | `solana-devnet`   | `spl-token`     | 6  | `4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU`                                      |
| `usdc-cardano-mainnet` | `cardano-mainnet` | `cardano-asset` | 8  | `25c5de5f…55534443` (Wanchain-bridged)                                              |
| `usdm-cardano-mainnet` | `cardano-mainnet` | `cardano-asset` | 6  | `c48cbb3d…5553444d`                                                                 |
| `djed-cardano-mainnet` | `cardano-mainnet` | `cardano-asset` | 6  | `8db269c3…6f555344`                                                                 |
| `iusd-cardano-mainnet` | `cardano-mainnet` | `cardano-asset` | 6  | `f66d78b4…69555344`                                                                 |

Every id above was read **off the chain it names**, not off a list: the EVM
contracts answered `symbol()` and `decimals()` on Cronos mainnet, the Solana
mints answered `getAccountInfo` as SPL mints, and the Cardano assets answered
Koios with their registered metadata. Note the decimals are not uniform — the
bridged USDC on Cardano carries **eight** places, not the six it has everywhere
else, and assuming otherwise misstates a balance by a factor of a hundred.

### What a row does not promise

That the wallet can **move** it. The standard says how a token is held, and the
chains are not equal:

| standard        | balance | transfer |
| --------------- | ------- | -------- |
| `erc20`         | yes     | yes — `transfer(address,uint256)` |
| `spl-token`     | yes     | yes — `transferChecked` between associated token accounts, creating the recipient's if it is missing |
| `cardano-asset` | yes     | **no** |

A Cardano native asset is read but not moved: spending the output holding it
means rebuilding every other asset riding on the same UTxO, and dropping one
silently is not something a wallet may do. The refusal happens before anything
is signed and names the reason. Such rows carry a `read-only` tag.

Two guards apply to every transfer, both aimed at the same failure — a registry
row whose decimals are wrong would scale the amount by a power of ten, silently
and irreversibly. The decimals are read from the contract or the mint and
compared against the row, and the transfer is refused if they disagree; on
Solana they are additionally signed into the instruction, so the cluster
refuses it too.

### Naming a token

Full key first, then the on-chain id, then a bare symbol — which resolves only
when **one** row carries it. `dai` is unambiguous; `usdc` is not, and the error
names the rows rather than picking whichever came first. Naming the network
settles it: with `-n cronos-mainnet`, `usdc` can only mean one row.

### Commands

    token list [FILTER…]      the rows a search keeps; empty is all of them
    token list --tags         the tags, so a search box can be filled in
    token list --here         only tokens on the network in view
    token info <token>        where it lives, how it is counted, what moves it
    token balance <token>     on that token's own network, without moving there
    token send <token> --to <address> --amount <n>

`token balance` and `token send` bind to the token's network for the call and
leave the stored one alone: asking what USDC is on Solana from a Cronos wallet
is a question with an answer, not a relocation.

`erc20` is unchanged and still takes a contract address, for a token the
registry has never heard of.

### Where a command runs

`token balance` and `token send` bind a **client** to the token's network, not
a second wallet. The distinction matters beyond tidiness: opening another `App`
means another store, another client stack and a great deal of stack, and doing
that inside a host thread with a modest one — the LÖVE GUI's worker has 512 KB
— took the process down. Reading a balance on another network costs a client.

More generally, the C ABI runs every command on a thread of its own with an
8 MB stack, because a library loaded into someone else's process does not
choose which thread calls it. Argument parsing alone does not fit in 512 KB:
clap builds its command tree on the stack, one frame per subcommand, and a
stack overflow is not a panic — there is nothing to catch and nothing to
report. See `guarded` in the `ffi` crate.

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

An EVM address is accepted all-lowercase or all-uppercase, neither of which
carries a checksum. A **mixed-case** address is checked against EIP-55 and
refused with `invalid_address` when it does not verify: the case pattern is a
checksum, and it fails exactly when a character has been corrupted.

### Fees

The confirmation question names the fee as well as the amount, on every chain
and every front end, because it is built once from the prepared transfer rather
than written out by each chain.

The fee itself is the endpoint's number — `eth_gasPrice` on EVM, `min_fee_a`
and `min_fee_b` from Koios on Cardano — and a hostile or broken one can make it
almost anything. So each network in the table carries a `max_fee` the wallet
will not sign past, in the base units of whatever token pays the fee, checked
before any key touches the transaction.

Three things can name that ceiling, tried in order of how deliberate they are:

1. `--max-fee` on the one command.
2. `max_fee.<network>` in `config.jsonl`, written by `network set-max-fee`. The
   value carries its denomination — `3 TCRO`, `2 DUST` — because a bare number
   means nothing on its own, and a reader that guesses wrong on Midnight is
   wrong by a factor of 10⁹. A value whose denomination is not the network's
   fee token is refused on the way in and treated as unset on the way out.
   `0` means the built-in ceiling, which is what every network starts at.
3. Nothing, which leaves `Network::max_fee` in force.

The unit is the **fee's**, not the transfer's. They are the same token on three
chains out of four; a Midnight transfer moves NIGHT (6 decimals) and pays its
fee in DUST (15), so every place that shows or asks for a ceiling names the
token it means.

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
