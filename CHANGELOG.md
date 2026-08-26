# Changelog

Every release is one number across every front end — see
[Versioning and releases](README.md#versioning-and-releases) for where it lives
and how a tag is checked against it.

## Unreleased

### The faucet, and the block explorer (#30, #31)

The wallet now knows where money is given away, and the GUI can go and get it.

**Every test network names its faucet, and no mainnet does.** The address is a
field on the network table, so `network list`, `network show` and `info` all
report it — alongside `faucet_automatic`, which answers the *other* question.

| network | faucet | `airdrop` |
| --- | --- | --- |
| `cronos-testnet` | <https://faucet.cronos.com/> | no |
| `solana-devnet` · `solana-testnet` | <https://faucet.solana.com/> | **yes** |
| `cardano-preprod` · `cardano-preview` | <https://docs.cardano.org/cardano-testnets/tools/faucet> | no |
| `midnight-preview` | <https://midnight-tmnight-preview.nethermind.dev/> | no |
| `midnight-devnet` | <https://midnight.network/test-faucet> | no |
| `ecash-testnet` | <https://texplorer.e.cash/testnet-faucet> | no |

Those two questions have different answers on ten of the twelve rows. Only
Solana's clusters answer a faucet request over the endpoint the balance came
from; every other faucet here is a web form with a captcha, built precisely so
that a program cannot drain one. So `airdrop` refuses on those networks *by
naming the page* rather than making a request that could never succeed — the
old refusal was "this chain has no faucet the wallet can call", which is true
and is a dead end.

**Every account carries its explorer link.** `account list` and `account show`
now hand one down beside the address, and `balance` does too. Assembled by the
wallet rather than by whoever draws it, because assembling one is not a matter
of appending a path: Solana keeps its cluster in a query string, so
`https://explorer.solana.com/?cluster=devnet` with `/address/<addr>` on the end
is a *mainnet* link that loads and shows an empty account.

**A node that refuses with a sentence is quoted, not its envelope.** Solana's
exhausted faucet answers 429 with "You've either reached your airdrop limit
today or the airdrop faucet has run dry", wrapped in JSON-RPC. The wallet now
prefers the sentence wherever a failing body carries one, and falls back to an
excerpt of the raw text otherwise, which is what every reply did before.

### FAUCET and EXPLORER, in the GUI (#30, #31)

Two buttons under the card, because both are answers about *the card on
screen*. They are not in the action bar beside SAVE and KEYS and could not have
been: five verbs are 236 pixels of button in a 260-pixel column, and seven
labels at the GUI's eight pixels a character come to 264 before any padding.
The card gave up 22 pixels of height instead and kept its proportions.

- **EXPLORER** copies the link and makes the copy visible — a ring leaves the
  button and the URL lands on a plate under the toast, because a clipboard is
  invisible and this screen has two buttons within forty pixels that both copy
  something.
- **FAUCET** reads the label off the network: `FAUCET` where the wallet asks
  the faucet itself, `FAUCET >` where the press copies a web page's address to
  the clipboard — which is the whole interaction on the ten networks whose
  faucet is behind a captcha. Nothing in the GUI launches a browser.
- The arrival is three round trips, not one — read the balance, ask, read it
  again — because a difference needs two readings, and the second cannot be
  taken immediately: a faucet answers when it has *accepted* the request, not
  when the money is spendable. Reading straight back gives the number that was
  already there and animates a value counting to itself.
- Both readings then sit side by side and the second climbs to meet the first
  over about a second on a `cubic_out` curve, with a stream of coins homing on
  it, staggered to last exactly as long as the number is moving.
- A refusal gets its own animation rather than a quieter version of that one,
  and a faucet that said yes and then delivered nothing says exactly that.
- **DEMO** plays the arrival with nothing moved, because the one moment worth
  watching was otherwise reachable only by being on Solana and being lucky.
  Every number in it is invented and the panel says so three times over.

Also in the GUI: a second particle layer that draws above the modal scrim
rather than under it, and a toast detail line that is trimmed from the middle
to fit the canvas — it was sized for a filesystem path and an explorer link is
sixty-odd characters, which ran off both edges losing the host and the address.

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
