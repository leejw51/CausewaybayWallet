# Test vectors

Shared fixtures that both implementations run against. They are what ties this
wallet to the outside world: if the Rust and Python sides ever disagreed with
each other *and* with the published standards, these files would fail first.

Everything here is **generated, not hand-typed** — run

```bash
make vectors        # pythoncli/.venv/bin/python scripts/gen-vectors.py
```

The generator computes each value from a reference implementation and then
asserts it against the constant published by the relevant standard or tool. It
refuses to write a file when the two disagree, so a vector in this directory is
one that two independent sources already agreed on.

## Files

| file | what it pins down | source |
| ---- | ----------------- | ------ |
| `bip39.json` | all 24 official English vectors: entropy → mnemonic → seed, with and without the `TREZOR` passphrase, plus normalisation cases | Trezor `python-mnemonic` |
| `bip39-invalid.json` | phrases that must be rejected, and why | hand-picked failure modes |
| `derivation.json` | 6 well-known mnemonics × 5 BIP-44 addresses each, with private keys and a passphrase variant | `eth-account` |
| `keys.json` | published private keys → address and public keys | `eth-account` |
| `keys-invalid.json` | private keys that must be rejected, and why | secp256k1 boundaries |
| `eip55.json` | the reference checksummed addresses | EIP-55 |
| `keccak.json` | keccak256 digests and the eight ERC-20 function selectors | `eth-utils` |
| `eip191.json` | `personal_sign` prefixed hashes and signatures | `eth-account` |
| `transactions.json` | signed legacy transactions on chain ids 1, 25 and 338 | `eth-account` |
| `units.json` | decimal ↔ smallest-unit conversions, valid and invalid | this project's rules |

## The well-known material

Nothing here is secret, and none of it should ever hold value.

**Mnemonics**

| name | phrase |
| ---- | ------ |
| `bip39-canonical` | `abandon abandon … about` — the all-zero-entropy BIP-39 vector |
| `foundry-anvil-default` | `test test test test test test test test test test test junk` — what Anvil and Hardhat print on startup |
| `ganache-default` | `myth like bonus scare over problem client lizard pioneer submit female collect` |
| `bip39-canonical-24` | `abandon ×23 art` |
| `bip39-legal-winner` | `legal winner thank year wave sausage worth useful legal winner thank yellow` |
| `bip39-zoo-wrong` | `zoo ×11 wrong` |

**Private keys**

| name | key | address |
| ---- | --- | ------- |
| `anvil-account-0` | `0xac0974be…f4f2ff80` | `0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266` |
| `eip155-example` | `0x46464646…46464646` | `0x9d8A62f656a8d1615C1294fd71e9CFb3E4855A4F` |
| `scalar-one` | `0x00…01` | the smallest valid secp256k1 scalar |
| `scalar-two` | `0x00…02` | |

## The anchors

Three of these are worth calling out, because they are checkable against a
document rather than against a library:

* **The BIP-39 canonical seed.** `abandon … about` with the `TREZOR` passphrase
  must produce `c55257c3…7463b04`. Nearly every wallet test suite quotes it.
* **The EIP-155 worked example.** The EIP publishes the complete signed
  transaction for nonce 9 on chain 1 with key `0x4646…46`. Both implementations
  reproduce those bytes exactly, `v = 37` included.
* **The Anvil accounts.** `test … junk` must derive
  `0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266` at index 0 with private key
  `0xac0974be…f4f2ff80`. Anyone who has run a local node has seen these.

## Who reads them

| implementation | test file |
| -------------- | --------- |
| Rust | `rustcli/tests/vectors.rs` |
| Python | `pythoncli/tests/test_vectors.py` |

Both are run by `make test`, which also checks that regenerating the vectors
produces byte-identical files — so a change to the generator cannot silently
move the goalposts.
