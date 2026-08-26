# Changelog

Every release is one number across every front end — see
[Versioning and releases](README.md#versioning-and-releases) for where it lives
and how a tag is checked against it.

## 1.0.4

The fifth chain, a token registry, one search box, and a security pass that
changed what the wallet refuses rather than what it computes.

### eCash (#26, #27)

A wallet now holds XEC. `ecash-testnet` is the default and `ecash-mainnet` is
there beside it, both picked up from the registry by the Rust CLI and TUI, the
Python and Lua bindings and the LÖVE GUI — nothing had to learn the chain by
name.

- CashAddr, BIP-44 on coin type 1899, Bitcoin-format transactions with BIP-143
  and `SIGHASH_FORKID`, and a protobuf reader for Chronik.
- XEC is quoted in **two** decimal places, not Bitcoin's eight.
- An output carrying an eToken is counted in a balance and never spent, and a
  transfer that would leave one behind says so in the sentence it asks you to
  agree to.

Anchored on published ground rather than on itself: the encoding reproduces the
vector published with the CashAddr specification (`gen-vectors.py` refuses to
write `testvectors/ecash.json` otherwise), and the signing digest is checked
against a real mainnet transaction whose signatures eCash's consensus accepted
in block 963,838.

### Tokens (#24, #25)

A token is not a thing you hold; a token *on a network* is. The registry row is
the pair, named as the pair — `usdc-cronos-mainnet`, said "USDC Cronos Mainnet"
— so naming a row settles the chain, the network, the decimals and the contract
at once.

| token | network | reads | moves |
| --- | --- | --- | --- |
| USDC, USDT, DAI | `cronos-mainnet` | yes | yes |
| USDC, USDT | `solana-mainnet` | yes | yes |
| USDC | `solana-devnet` | yes | yes |
| USDC, USDM, DJED, iUSD | `cardano-mainnet` | yes | **no** |

- Every contract address, mint and asset id was read off the chain it names.
  Cardano's USDC is Wanchain-bridged and carries **eight** decimal places, and
  Solana devnet's USDC is a different mint from mainnet's — so before any
  transfer the wallet re-reads the decimals from the contract or the mint and
  refuses if they disagree with its own table.
- **Solana SPL** is real support: the associated token account is derived for
  you, three of those derivations pinned against mainnet itself. Sending to
  someone who has never held the token creates their account, and that rent is
  added to the fee you approve rather than hidden.
- **Cardano native assets are read, not moved.** Spending the output that holds
  one means rebuilding every other asset on the same UTxO; the refusal happens
  before anything is signed and says why.

### Search (#25)

Every list is searchable and the rule is the same everywhere: empty matches
everything, every word must match, substring, case and `-`/`_`/space ignored. A
tag says what a row's *name does not* — `cronos-mainnet` needs no `cronos` tag,
it needs `evm`.

```sh
cwbwallet network list testnet          # 8 of 12, however each is named
cwbwallet token list stablecoin cronos
cwbwallet token list --tags             # what there is to search by
```

`/` opens a live filter in the TUI over one flat pane of commands, networks and
tokens. The GUI's network screen leads with an always-focused search box, and a
row there is a destination: picking the USDC row aims balance, send form, amount
label and header at it, and SEND greys out with a reason for an asset that
cannot move.

### The address a wallet shows follows the network (#22, #23, #27)

Switching network left every front end showing the chain it had just left — the
header said Cardano over a column of `0x…`. That is a deposit address that
cannot receive on the network named above it.

- `network use` now derives the accounts a store has no record for. Same phrase,
  same index, one per wallet; they come back as `derived` in the command's data.
  Two wallets are skipped, because the phrase alone would not reproduce them: one
  imported from a bare private key, and one made with a BIP-39 passphrase. Both
  front ends say so rather than showing something else.
- Every display path reads `Chain::address_on` rather than the account's stored
  string, so `-n cardano-mainnet balance` no longer asks a mainnet node about a
  testnet address and reports zero for a funded account. Account `id`s are
  unaffected.
- `SPEC.md` §2.2 states the behaviour, including the two skips.

### Security (#18, #21)

Nothing here changed what the wallet computes — derivation, signing and the send
pipeline are untouched.

- **The fee is in the confirmation now, and it has a ceiling.** The sentence is
  built once from the prepared transfer, so no chain can omit it and every front
  end shares it, `--yes` included. Each network carries a `max_fee` and every
  chain tests against it *before signing*: a Koios instance answering
  `min_fee_a = 10^9` is refused rather than signed and confirmed.
- **The bare library name is gone.** `dlopen` and `LoadLibrary` resolve a
  slash-less name against the working directory, so running `cwbwallet` from
  Downloads could load a planted `libcausewaybay_ffi.dylib` and hand it every
  mnemonic crossing `cwb_execute`. Both bindings end their search with absolute
  system paths, and a test asserts no candidate is relative.
- **"Forget" means it.** `account remove`, `recent forget` and `recent clear`
  rewrite their log without the record — temp file created `0600`, renamed over
  the original — instead of appending a tombstone over plaintext that stayed.
  Tombstones an older binary wrote are still honoured on replay.
- **EIP-55 is enforced where it exists.** All-lower and all-upper carry no
  checksum and stay ordinary; a mixed-case address whose checksum does not hold
  is refused.
- Zeroization of seeds, entropy, derived scalars, PBKDF2 buffers, `Keypair` and
  a stored `Account`'s two secret fields; checked or saturating arithmetic on
  every endpoint-supplied number; a 16 MiB ceiling on a reply body and a
  100,000-event cap on a history replay; the send prompt no longer pre-fills
  from the clipboard; a typo'd mnemonic word is reported by position rather than
  quoted; `JobHost::confirm` refuses instead of returning `Ok(())`.

### Also

- **Pay a wallet from its own row.** Every non-active row carries a `SEND`
  button for a fixed `0.01`. The row is the recipient and only that — pressing
  it does not also select the wallet, and the press goes through `begin_send`
  like every other transfer, so the same dialog names the amount, the recipient
  and the wallet being debited before anything is signed.
- **A different card for every address, on every chain.** `card.design` read hex
  pairs out of an address, which scavenges almost nothing from the non-hex
  alphabets — two wallets could share a face. Over 2000 random addresses of each
  shape, eCash went from 569 distinct faces to 1998 and bech32 from 967 to 2000.
  Every EVM card is byte-for-byte unchanged; that was the constraint.
- `cwbwallet` can run a command against a stack the wallet owns.
- Four error messages got their line breaks back — a `\`-continued literal loses
  the newline, and these four shipped with a run of spaces mid-sentence.

## 1.0.3

The version of record, bumped ahead of the multichain release so a tag could be
cut (#19, #20).

## 1.0.2

CAUSEWAYBAY BANK — the wallet as an 8-bit LÖVE game, shipped as a signed and
notarized macOS `.app` with a login gate, wallet cards, and export and wipe on
logout. Four chains behind one wallet index, and the TUI rework that came with
them.

## 1.0.1

The Rust wallet split into a core, a C ABI and a CLI, with Lua and C front ends
over the same library. Packaging verified on pull requests rather than only
after merge.

## 1.0.0

First release.
