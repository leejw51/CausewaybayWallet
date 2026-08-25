#!/usr/bin/env python3
"""Regenerate the shared test vectors in ../testvectors.

Values come from reference implementations (the Trezor `mnemonic` library,
`eth_account`, `eth_utils`) rather than being typed in by hand, and every
published constant this project claims to match is asserted here before it is
written out. The one encoding with no Python library behind it — eCash's
CashAddr — is written out longhand and pinned to the vector published with its
own specification. Run it with the Python virtualenv:

    pythoncli/.venv/bin/python scripts/gen-vectors.py
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

from eth_account import Account
from eth_account.messages import encode_defunct
from eth_keys.datatypes import PrivateKey
from eth_utils import keccak, to_checksum_address
from mnemonic import Mnemonic

Account.enable_unaudited_hdwallet_features()

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "testvectors"
MNEMO = Mnemonic("english")

# Every entropy value from the official Trezor BIP-39 English vector set.
TREZOR_ENTROPY = [
    "00000000000000000000000000000000",
    "7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f",
    "80808080808080808080808080808080",
    "ffffffffffffffffffffffffffffffff",
    "000000000000000000000000000000000000000000000000",
    "7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f",
    "808080808080808080808080808080808080808080808080",
    "ffffffffffffffffffffffffffffffffffffffffffffffff",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f",
    "8080808080808080808080808080808080808080808080808080808080808080",
    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    "9e885d952ad362caeb4efe34a8e91bd2",
    "77c2b00716cec7213839159e404db50d",
    "6610b25967cdcca9d59875f5cb50b0ea75433311869e930b",
    "68a79eaca2324873eacc50cb9c6eca8cc68ea5d936f98787c60c7ebc74e6ce7c",
    "c0ba5a8e914111210f2bd131f3d5e08d",
    "6d9be1ee6ebd27a258115aad99b7317b9c8d28b6d76431c3",
    "9f6a2878b2520799a44ef18bc7df394e7061a224d2c33cd015b157d746869863",
    "23db8160a31d3e0dca3688ed941adbf3",
    "8197a4a47f0425faeaa69deebc05ca29c0a5b5cc76ceacc0",
    "066dca1a2bb7e8a1db2832148ce9933eea0f3ac9548d793112d9a95c9407efad",
    "f30f8c1da665478f49b001d94c5fc452",
    "c10ec20dc3cd9f652c7fac2f1230f7a3c828389a14392f05",
    "f585c11aec520db57dd353c69554b21a89b20fb0650966fa0a9d6f74fd989d8f",
]

# The phrase every multi-chain vector in this project derives from, so that a
# reader can compare one wallet across five chains rather than five wallets.
CANONICAL_PHRASE = (
    "abandon abandon abandon abandon abandon abandon abandon abandon "
    "abandon abandon abandon about"
)

# Mnemonics anyone working with Ethereum tooling will recognise.
KNOWN_MNEMONICS = [
    {
        "name": "bip39-canonical",
        "note": "The all-zero-entropy phrase from the BIP-39 test vectors.",
        "phrase": CANONICAL_PHRASE,
    },
    {
        "name": "foundry-anvil-default",
        "note": "The default mnemonic of Anvil and Hardhat's local test node.",
        "phrase": "test test test test test test test test test test test junk",
    },
    {
        "name": "ganache-default",
        "note": "The mnemonic Ganache historically shipped in its quickstart output.",
        "phrase": "myth like bonus scare over problem client lizard pioneer submit female collect",
    },
    {
        "name": "bip39-canonical-24",
        "note": "The 24-word all-zero-entropy phrase from the BIP-39 test vectors.",
        "phrase": "abandon abandon abandon abandon abandon abandon abandon abandon "
        "abandon abandon abandon abandon abandon abandon abandon abandon "
        "abandon abandon abandon abandon abandon abandon abandon art",
    },
    {
        "name": "bip39-legal-winner",
        "note": "The 0x7f-entropy phrase from the BIP-39 test vectors.",
        "phrase": "legal winner thank year wave sausage worth useful legal winner thank yellow",
    },
    {
        "name": "bip39-zoo-wrong",
        "note": "The all-ones-entropy phrase from the BIP-39 test vectors.",
        "phrase": "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong",
    },
]

# Private keys with addresses that are published and widely quoted.
KNOWN_KEYS = [
    {
        "name": "anvil-account-0",
        "note": "Anvil/Hardhat account #0, derived from the 'test … junk' mnemonic.",
        "private_key": "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        "published_address": "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266",
    },
    {
        "name": "eip155-example",
        "note": "The signing key used by the worked example in EIP-155.",
        "private_key": "0x4646464646464646464646464646464646464646464646464646464646464646",
        "published_address": "0x9d8A62f656a8d1615C1294fd71e9CFb3E4855A4F",
    },
    {
        "name": "scalar-one",
        "note": "The smallest valid secp256k1 scalar.",
        "private_key": "0x0000000000000000000000000000000000000000000000000000000000000001",
        "published_address": "0x7E5F4552091A69125d5DFCB7b8C2659029395BdF",
    },
    {
        "name": "scalar-two",
        "private_key": "0x0000000000000000000000000000000000000000000000000000000000000002",
        "published_address": "0x2B5AD5c4795c026514f8317c7a215E218DcCD6cF",
    },
]

# The five reference addresses published in EIP-55 itself.
EIP55_PUBLISHED = [
    "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
    "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359",
    "0xdbF03B407c01E7cD3CBea99509d93f8DDDC8C6FB",
    "0xD1220A0cf47c7B9Be7A2E6BA89F429762e7b9aDb",
    "0x52908400098527886E0F7030069857D2E4169EE7",
    "0x8617E340B3D01FA5F11F306F4090FD50E238070D",
    "0xde709f2102306220921060314715629080e2fb77",
    "0x27b1fdb04752bbc536007a920d24acb045561c26",
    "0x5A4EAB120fB44eb6684E5e32785702FF45ea344D",
]


def check(what: str, expected, actual) -> None:
    """Fail loudly rather than writing a vector this project cannot substantiate."""
    if expected != actual:
        raise SystemExit(f"vector mismatch for {what}:\n  published {expected}\n  computed  {actual}")


def check_address(what: str, expected: str, actual: str) -> None:
    """Compare addresses by their bytes.

    What is published about an address is the 20 bytes; the mixed-case form is a
    function of them, and `eip55.json` verifies that function separately.
    """
    if expected.lower() != actual.lower():
        raise SystemExit(f"address mismatch for {what}:\n  published {expected}\n  computed  {actual}")


def bip39_vectors() -> dict:
    vectors = []
    for entropy_hex in TREZOR_ENTROPY:
        entropy = bytes.fromhex(entropy_hex)
        phrase = MNEMO.to_mnemonic(entropy)
        vectors.append(
            {
                "entropy": entropy_hex,
                "mnemonic": phrase,
                "word_count": len(phrase.split()),
                "seed_trezor": MNEMO.to_seed(phrase, "TREZOR").hex(),
                "seed_empty_passphrase": MNEMO.to_seed(phrase, "").hex(),
            }
        )
    # The most-quoted BIP-39 seed of all; if this is wrong, everything else is.
    check(
        "canonical seed with the TREZOR passphrase",
        "c55257c360c07c72029aebc1b53c05ed0362ada38ead3e3e9efa3708e53495531f09a698"
        "7599d18264c1e1c92f2cf141630c7a3c4ab7c81b2f001698e7463b04",
        vectors[0]["seed_trezor"],
    )
    check(
        "canonical seed with no passphrase",
        "5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc19a5ac40b"
        "389cd370d086206dec8aa6c43daea6690f20ad3d8d48b2d2ce9e38e4",
        vectors[0]["seed_empty_passphrase"],
    )
    canonical = vectors[0]["mnemonic"]
    # BIP-39 compares NFKD-normalised, whitespace-collapsed, lowercase text, so
    # all of these name the same wallet.
    normalization = [
        {"input": canonical.upper(), "canonical": canonical},
        {"input": f"  {canonical}  ", "canonical": canonical},
        {"input": canonical.replace(" ", "   "), "canonical": canonical},
        {"input": canonical.replace(" ", "\t", 1).replace(" ", "\n", 1), "canonical": canonical},
        {"input": "ABANDON " + canonical.split(" ", 1)[1], "canonical": canonical},
    ]
    for case in normalization:
        assert MNEMO.check(" ".join(case["input"].split()).lower()), case["input"]
    return {
        "source": "Trezor python-mnemonic reference vectors (English wordlist)",
        "passphrase": "TREZOR",
        "vectors": vectors,
        "normalization": normalization,
    }


def invalid_mnemonics() -> dict:
    return {
        "note": "Phrases that must be rejected, with the reason they are invalid.",
        "vectors": [
            {"mnemonic": "", "reason": "empty"},
            {"mnemonic": "abandon about", "reason": "wrong word count"},
            {
                "mnemonic": " ".join(["abandon"] * 13),
                "reason": "wrong word count",
            },
            {
                "mnemonic": " ".join(["abandon"] * 12),
                "reason": "checksum does not match",
            },
            {
                "mnemonic": " ".join(["abandon"] * 11 + ["notaword"]),
                "reason": "not a BIP-39 word",
            },
            {
                "mnemonic": " ".join(["abandon"] * 11 + ["abandonn"]),
                "reason": "near-miss word that is not in the wordlist",
            },
            {
                "mnemonic": " ".join(["abandon"] * 11 + ["about"] + ["about"]),
                "reason": "wrong word count",
            },
        ],
    }


def derivation_vectors() -> dict:
    entries = []
    for known in KNOWN_MNEMONICS:
        accounts = []
        for index in range(5):
            path = f"m/44'/60'/0'/0/{index}"
            account = Account.from_mnemonic(known["phrase"], account_path=path)
            accounts.append(
                {
                    "index": index,
                    "path": path,
                    "address": to_checksum_address(account.address),
                    "private_key": "0x" + account.key.hex().removeprefix("0x"),
                }
            )
        entry = {**known, "accounts": accounts}
        # A passphrase must produce a completely different wallet.
        salted = Account.from_mnemonic(
            known["phrase"], account_path="m/44'/60'/0'/0/0", passphrase="TREZOR"
        )
        entry["passphrase_trezor_index_0"] = to_checksum_address(salted.address)
        entries.append(entry)

    by_name = {entry["name"]: entry for entry in entries}
    # Anvil prints these on every start; they are the best-known Ethereum addresses
    # in local development.
    check_address(
        "anvil account 0",
        "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266",
        by_name["foundry-anvil-default"]["accounts"][0]["address"],
    )
    check_address(
        "anvil account 1",
        "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
        by_name["foundry-anvil-default"]["accounts"][1]["address"],
    )
    check(
        "anvil account 0 private key",
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        by_name["foundry-anvil-default"]["accounts"][0]["private_key"],
    )
    check_address(
        "bip39 canonical account 0",
        "0x9858EfFD232B4033E47d90003D41EC34EcaEda94",
        by_name["bip39-canonical"]["accounts"][0]["address"],
    )
    return {
        "source": "eth-account BIP-44 derivation over well-known mnemonics",
        "purpose": "m/44'/60'/0'/0/<index>",
        "mnemonics": entries,
    }


def key_vectors() -> dict:
    entries = []
    for known in KNOWN_KEYS:
        account = Account.from_key(known["private_key"])
        address = to_checksum_address(account.address)
        check_address(f"address for {known['name']}", known["published_address"], address)
        entries.append(
            {
                "name": known["name"],
                "note": known.get("note"),
                "private_key": known["private_key"],
                "address": address,
                "public_key": "0x" + account._key_obj.public_key.to_hex()[2:],
                "public_key_compressed": "0x"
                + account._key_obj.public_key.to_compressed_bytes().hex(),
            }
        )
    return {"source": "eth-account, cross-checked against published addresses", "keys": entries}


def invalid_keys() -> dict:
    curve_order = "fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141"
    return {
        "note": "Private keys that must be rejected, with the reason.",
        "vectors": [
            {"private_key": "", "reason": "empty"},
            {"private_key": "0x1234", "reason": "too short"},
            {"private_key": "0x" + "z" * 64, "reason": "not hexadecimal"},
            {"private_key": "0x" + "00" * 32, "reason": "zero is not a valid scalar"},
            {"private_key": "0x" + curve_order, "reason": "equal to the curve order"},
            {"private_key": "0x" + "ff" * 32, "reason": "above the curve order"},
            {"private_key": "0x" + "ab" * 33, "reason": "too long"},
        ],
    }


def eip55_vectors() -> dict:
    """Here the mixed-case form *is* the thing under test, so compare exactly."""
    entries = []
    mismatches = []
    for address in EIP55_PUBLISHED:
        computed = to_checksum_address(address.lower())
        if computed != address:
            mismatches.append(f"    published {address}\n    computed  {computed}")
        entries.append({"lowercase": address.lower(), "checksummed": computed})
    if mismatches:
        raise SystemExit("EIP-55 vectors disagree:\n" + "\n".join(mismatches))
    return {"source": "EIP-55 reference addresses", "vectors": entries}


def keccak_vectors() -> dict:
    inputs = ["", "abc", "hello", "The quick brown fox jumps over the lazy dog", "🌏"]
    entries = [
        {"text": text, "keccak256": "0x" + keccak(text=text).hex()} for text in inputs
    ]
    check(
        "keccak256 of the empty string",
        "0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470",
        entries[0]["keccak256"],
    )
    # Function selectors are the first four bytes of these hashes.
    selectors = {
        "transfer(address,uint256)": "0xa9059cbb",
        "balanceOf(address)": "0x70a08231",
        "decimals()": "0x313ce567",
        "symbol()": "0x95d89b41",
        "name()": "0x06fdde03",
        "totalSupply()": "0x18160ddd",
        "allowance(address,address)": "0xdd62ed3e",
        "approve(address,uint256)": "0x095ea7b3",
    }
    for signature, selector in selectors.items():
        check(f"selector for {signature}", selector, "0x" + keccak(text=signature)[:4].hex())
    return {
        "source": "eth-utils keccak, cross-checked against published digests",
        "hashes": entries,
        "selectors": [{"signature": k, "selector": v} for k, v in selectors.items()],
    }


def eip191_vectors() -> dict:
    key = KNOWN_KEYS[1]["private_key"]  # the EIP-155 example key
    account = Account.from_key(key)
    entries = []
    for message in ["", "Hello World", "hello causewaybay", "héllo 🌏", "a" * 1000]:
        signed = Account.sign_message(encode_defunct(text=message), key)
        entries.append(
            {
                "message": message,
                "prefixed_hash": "0x"
                + keccak(
                    b"\x19Ethereum Signed Message:\n"
                    + str(len(message.encode())).encode()
                    + message.encode()
                ).hex(),
                "signature": "0x" + signed.signature.hex().removeprefix("0x"),
                "signer": to_checksum_address(account.address),
            }
        )
    check(
        "EIP-191 hash of 'Hello World'",
        "0xa1de988600a42c4b4ab089b619297c17d53cffae5d5120d82d8a92d0bb3b78f2",
        entries[1]["prefixed_hash"],
    )
    return {
        "source": "eth-account personal_sign (EIP-191)",
        "signing_key": key,
        "vectors": entries,
    }


def transaction_vectors() -> dict:
    """Legacy EIP-155 transactions, including the worked example from the EIP."""
    cases = [
        {
            "name": "eip155-official-example",
            "note": "The worked example in EIP-155 itself (chain id 1).",
            "private_key": "0x4646464646464646464646464646464646464646464646464646464646464646",
            "tx": {
                "nonce": 9,
                "gasPrice": 20_000_000_000,
                "gas": 21_000,
                "to": "0x3535353535353535353535353535353535353535",
                "value": 1_000_000_000_000_000_000,
                "data": b"",
                "chainId": 1,
            },
        },
        {
            "name": "cronos-testnet-transfer",
            "note": "The same transaction on Cronos testnet (chain id 338).",
            "private_key": "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727",
            "tx": {
                "nonce": 9,
                "gasPrice": 20_000_000_000,
                "gas": 21_000,
                "to": "0x3535353535353535353535353535353535353535",
                "value": 1_000_000_000_000_000_000,
                "data": b"",
                "chainId": 338,
            },
        },
        {
            "name": "cronos-mainnet-transfer",
            "note": "The same transaction on Cronos mainnet (chain id 25).",
            "private_key": "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727",
            "tx": {
                "nonce": 0,
                "gasPrice": 5_000_000_000,
                "gas": 21_000,
                "to": "0x9858EfFD232B4033E47d90003D41EC34EcaEda94",
                "value": 0,
                "data": b"",
                "chainId": 25,
            },
        },
        {
            "name": "erc20-transfer-calldata",
            "note": "A transfer(address,uint256) call carrying 1.5 tokens of 18 decimals.",
            "private_key": "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727",
            "tx": {
                "nonce": 3,
                "gasPrice": 5_000_000_000,
                "gas": 60_000,
                "to": "0x6Fac4D18c912343BF86fa7049364Dd4E424Ab9C0",
                "value": 0,
                "data": bytes.fromhex(
                    "a9059cbb"
                    "0000000000000000000000009858effd232b4033e47d90003d41ec34ecaeda94"
                    "00000000000000000000000000000000000000000000000014d1120d7b160000"
                ),
                "chainId": 338,
            },
        },
        {
            "name": "large-values",
            "note": "Boundary case: a nonce, gas price and value far above the usual range.",
            "private_key": "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727",
            "tx": {
                "nonce": 4_294_967_295,
                "gasPrice": 1_000_000_000_000,
                "gas": 8_000_000,
                "to": "0x3535353535353535353535353535353535353535",
                "value": 2**128,
                "data": b"",
                "chainId": 338,
            },
        },
    ]

    entries = []
    for case in cases:
        signed = Account.sign_transaction(case["tx"], case["private_key"])
        tx = dict(case["tx"])
        entries.append(
            {
                "name": case["name"],
                "note": case["note"],
                "private_key": case["private_key"],
                "signer": to_checksum_address(Account.from_key(case["private_key"]).address),
                "nonce": tx["nonce"],
                "gas_price": str(tx["gasPrice"]),
                "gas_limit": tx["gas"],
                "to": tx["to"],
                "value": str(tx["value"]),
                "data": "0x" + tx["data"].hex(),
                "chain_id": tx["chainId"],
                "raw": "0x" + signed.raw_transaction.hex().removeprefix("0x"),
                "hash": "0x" + signed.hash.hex().removeprefix("0x"),
                "v": signed.v,
                "r": "0x" + format(signed.r, "064x"),
                "s": "0x" + format(signed.s, "064x"),
            }
        )

    # The signed transaction printed in EIP-155.
    check(
        "EIP-155 worked example, signed transaction",
        "0xf86c098504a817c800825208943535353535353535353535353535353535353535880de0"
        "b6b3a76400008025a028ef61340bd939bc2195fe537567866003e1a15d3c71ff63e1590620"
        "aa636276a067cbe9d8997f761aecb703304b3800ccf555c9f3dc64214b297fb1966a3b6d83",
        entries[0]["raw"],
    )
    check("EIP-155 worked example, v", 37, entries[0]["v"])
    return {
        "source": "eth-account legacy (type 0x0) signing with EIP-155 replay protection",
        "vectors": entries,
    }


def unit_vectors() -> dict:
    return {
        "note": "Decimal <-> smallest-unit conversions that must never use floating point.",
        "valid": [
            {"amount": "0", "decimals": 18, "value": "0"},
            {"amount": "1", "decimals": 18, "value": "1000000000000000000"},
            {"amount": "1.5", "decimals": 18, "value": "1500000000000000000"},
            {"amount": "0.000000000000000001", "decimals": 18, "value": "1"},
            {"amount": "123456.789", "decimals": 18, "value": "123456789000000000000000"},
            {"amount": "1.5", "decimals": 6, "value": "1500000"},
            {"amount": "1", "decimals": 0, "value": "1"},
            {"amount": "5", "decimals": 9, "value": "5000000000"},
            {
                "amount": "115792089237316195423570985008687907853269984665640564039457.584007913129639935",
                "decimals": 18,
                "value": str(2**256 - 1),
            },
        ],
        "invalid": [
            {"amount": "", "decimals": 18, "reason": "empty"},
            {"amount": "abc", "decimals": 18, "reason": "not a number"},
            {"amount": "1.2.3", "decimals": 18, "reason": "two decimal points"},
            {"amount": "-1", "decimals": 18, "reason": "negative"},
            {"amount": "1e18", "decimals": 18, "reason": "exponent notation"},
            {"amount": "0x10", "decimals": 18, "reason": "hexadecimal"},
            {"amount": "1,5", "decimals": 18, "reason": "comma separator"},
            {"amount": "0.1234567", "decimals": 6, "reason": "more precision than the token has"},
        ],
    }


# ================================================================== eCash

# The base32 alphabet CashAddr writes an address with, and the generator of
# the 40-bit BCH code that guards it. Both are from the CashAddr
# specification; eCash adopted it whole and changed only the prefix.
CASHADDR_CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"
CASHADDR_GENERATOR = [0x98F2BC8E61, 0x79B76D99E2, 0xF33E5FB3C4, 0xAE2EABE2A8, 0x1E4F43E470]

# The version byte's type nibble: 0 for a public key hash, 1 for a script hash.
CASHADDR_TYPE = {"p2pkh": 0x00, "p2sh": 0x08}

# eCash's SLIP-0044 coin type. It is not Bitcoin's 0 and not the testnet 1:
# a wallet that reached for either would derive real, funded-looking, wrong
# addresses.
ECASH_COIN_TYPE = 1899


def cashaddr_polymod(values) -> int:
    checksum = 1
    for value in values:
        top = checksum >> 35
        checksum = ((checksum & 0x07FFFFFFFF) << 5) ^ value
        for bit, generator in enumerate(CASHADDR_GENERATOR):
            if top & (1 << bit):
                checksum ^= generator
    return checksum ^ 1


def cashaddr_regroup(data, from_bits: int, to_bits: int):
    """Regroup bit-widths, which is how 21 bytes become 34 base32 digits."""
    accumulator = 0
    bits = 0
    out = []
    for value in data:
        accumulator = (accumulator << from_bits) | value
        bits += from_bits
        while bits >= to_bits:
            bits -= to_bits
            out.append((accumulator >> bits) & ((1 << to_bits) - 1))
    if bits:
        out.append((accumulator << (to_bits - bits)) & ((1 << to_bits) - 1))
    return out


def cashaddr(prefix: str, kind: str, payload: bytes) -> str:
    body = cashaddr_regroup([CASHADDR_TYPE[kind]] + list(payload), 8, 5)
    # The prefix is inside the checksum, which is what makes a mainnet address
    # fail as a testnet one instead of silently decoding to the same bytes.
    checksum = cashaddr_polymod([ord(c) & 0x1F for c in prefix] + [0] + body + [0] * 8)
    digits = body + [(checksum >> (5 * (7 - i))) & 0x1F for i in range(8)]
    return prefix + ":" + "".join(CASHADDR_CHARSET[d] for d in digits)


def hash160(data: bytes) -> bytes:
    return hashlib.new("ripemd160", hashlib.sha256(data).digest()).digest()


def base58check(payload: bytes) -> str:
    alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
    full = payload + hashlib.sha256(hashlib.sha256(payload).digest()).digest()[:4]
    number = int.from_bytes(full, "big")
    out = ""
    while number:
        number, remainder = divmod(number, 58)
        out = alphabet[remainder] + out
    for byte in full:
        if byte != 0:
            break
        out = "1" + out
    return out


def ecash_vectors() -> dict:
    """eCash derivation and addresses, from two independent halves.

    The private key comes from `eth_account`'s BIP-32, the same code that
    produces `derivation.json` — eCash is plain secp256k1 over BIP-44, so
    nothing about the key is eCash-specific except the coin type.

    The address comes from the CashAddr encoder above, which is checked
    against the vector published *with the specification* before any of this
    is written out. So each row is one library's key run through one
    document's encoding, and neither can drift without the other noticing.
    """
    # The specification's own worked example: 20 bytes, and the three
    # renderings of them it prints.
    spec_payload = bytes.fromhex("f5bf48b397dae70be82b3cca4793f8eb2b6cdac9")
    check(
        "the CashAddr specification's public-key-hash vector",
        "bitcoincash:qr6m7j9njldwwzlg9v7v53unlr4jkmx6eylep8ekg2",
        cashaddr("bitcoincash", "p2pkh", spec_payload),
    )
    # And a script-hash address read off the live eCash chain, which pins the
    # other type nibble and the `ecash` prefix at the same time.
    check(
        "a script-hash address observed on eCash mainnet",
        "ecash:pquc59839pv8fga4h8eayy5fty0s00aj5czp4d547x",
        cashaddr("ecash", "p2sh", bytes.fromhex("398a14f1285874a3b5b9f3d21289591f07bfb2a6")),
    )

    accounts = []
    for index in range(5):
        path = f"m/44'/{ECASH_COIN_TYPE}'/0'/0/{index}"
        account = Account.from_mnemonic(CANONICAL_PHRASE, account_path=path)
        private_key = bytes(account.key)
        compressed = PrivateKey(private_key).public_key.to_compressed_bytes()
        key_hash = hash160(compressed)
        accounts.append(
            {
                "index": index,
                "path": path,
                "private_key": "0x" + private_key.hex(),
                "public_key_compressed": compressed.hex(),
                "public_key_hash": key_hash.hex(),
                # Testnet first, because it is the address the wallet stores:
                # eCash's default network here is its testnet.
                "address": cashaddr("ectest", "p2pkh", key_hash),
                "address_mainnet": cashaddr("ecash", "p2pkh", key_hash),
                # What Electrum ABC exports and imports. The trailing 0x01 is
                # what says the address hashes the *compressed* key; without
                # it the same scalar names a different address.
                "wif": base58check(b"\xef" + private_key + b"\x01"),
                "wif_mainnet": base58check(b"\x80" + private_key + b"\x01"),
            }
        )

    return {
        "source": "eth-account BIP-44 derivation; CashAddr per its specification",
        "purpose": f"m/44'/{ECASH_COIN_TYPE}'/0'/0/<index>",
        "mnemonic": CANONICAL_PHRASE,
        "coin_type": ECASH_COIN_TYPE,
        "dust_limit_sats": 546,
        "decimals": 2,
        "$note": (
            "XEC has two decimal places, not Bitcoin's eight: eCash "
            "redenominated at the 2021 rebrand, so one XEC is 100 satoshis."
        ),
        "spec_vectors": [
            {
                "$source": "the CashAddr specification",
                "prefix": "bitcoincash",
                "kind": "p2pkh",
                "payload": spec_payload.hex(),
                "address": cashaddr("bitcoincash", "p2pkh", spec_payload),
            },
            {
                "$source": "observed on eCash mainnet",
                "prefix": "ecash",
                "kind": "p2sh",
                "payload": "398a14f1285874a3b5b9f3d21289591f07bfb2a6",
                "address": "ecash:pquc59839pv8fga4h8eayy5fty0s00aj5czp4d547x",
                "script_pubkey": "a914398a14f1285874a3b5b9f3d21289591f07bfb2a687",
            },
        ],
        "accounts": accounts,
    }


def write(name: str, payload: dict) -> None:
    header = {
        "$comment": "Generated by scripts/gen-vectors.py — do not edit by hand.",
        **payload,
    }
    path = OUT / name
    path.write_text(json.dumps(header, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    count = sum(len(v) for v in payload.values() if isinstance(v, list))
    print(f"  {path.relative_to(ROOT)}  ({count} vectors)")


def main() -> None:
    OUT.mkdir(exist_ok=True)
    print("Regenerating test vectors:")
    write("bip39.json", bip39_vectors())
    write("bip39-invalid.json", invalid_mnemonics())
    write("derivation.json", derivation_vectors())
    write("keys.json", key_vectors())
    write("keys-invalid.json", invalid_keys())
    write("eip55.json", eip55_vectors())
    write("keccak.json", keccak_vectors())
    write("eip191.json", eip191_vectors())
    write("transactions.json", transaction_vectors())
    write("units.json", unit_vectors())
    write("ecash.json", ecash_vectors())
    print("All published constants matched their computed values.")


if __name__ == "__main__":
    main()
