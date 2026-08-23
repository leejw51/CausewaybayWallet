"""Key material: derivation from mnemonics, address computation, EIP-191 signing.

BIP-39/32/44 work is delegated to ``mnemonic`` and ``eth_account`` rather than
reimplemented; the Rust side implements the same standards from scratch and both
are checked against the official test vectors.
"""

from __future__ import annotations

import unicodedata
from dataclasses import dataclass

from eth_account import Account
from eth_account.messages import encode_defunct
from eth_utils import is_hex_address, keccak, to_checksum_address
from mnemonic import Mnemonic

from . import errors

# Required before ``Account.from_mnemonic`` may be used.
Account.enable_unaudited_hdwallet_features()

WORD_COUNTS = {12: 128, 15: 160, 18: 192, 21: 224, 24: 256}

_MNEMONIC = Mnemonic("english")


def ethereum_path(index: int) -> str:
    """The BIP-44 Ethereum account path for an address index."""
    if not isinstance(index, int) or isinstance(index, bool) or index < 0:
        raise errors.usage("address index must be a non-negative integer")
    if index >= 2**31:
        raise errors.usage("address index is out of range")
    return f"m/44'/60'/0'/0/{index}"


def entropy_bits_for_words(words: int) -> int:
    try:
        return WORD_COUNTS[words]
    except KeyError:
        raise errors.invalid_mnemonic(
            f"unsupported word count {words}; use 12, 15, 18, 21 or 24"
        ) from None


def generate_mnemonic(words: int = 12) -> str:
    """Generate a fresh mnemonic with the given number of words."""
    return _MNEMONIC.generate(strength=entropy_bits_for_words(words))


def validate_mnemonic(phrase: str) -> bool:
    """True when the phrase is well formed and its checksum matches."""
    if not phrase or not isinstance(phrase, str):
        return False
    return _MNEMONIC.check(normalize_mnemonic(phrase))


def wordlist() -> list[str]:
    """The 2048 English BIP-39 words, for telling a typo from a bad checksum."""
    return _MNEMONIC.wordlist


def normalize_mnemonic(phrase: str) -> str:
    """NFKD-normalise, collapse whitespace and lowercase, as BIP-39 requires.

    The NFKD step is what SPEC.md §3 mandates and the Rust side applies. Without
    it the two CLIs sharing one store would write different bytes — and
    different recall ids — for the same phrase typed with a compatibility
    character such as the U+FB01 'ﬁ' ligature.
    """
    normalized = unicodedata.normalize("NFKD", str(phrase))
    return " ".join(normalized.split()).lower()


@dataclass(frozen=True)
class Keypair:
    """A private key plus everything derivable from it."""

    private_key: str  # 0x-prefixed, 64 hex characters

    def __repr__(self) -> str:
        # Redacted on purpose: a repr must never leak the secret.
        return f"Keypair(address={self.address!r}, private_key='<redacted>')"

    @property
    def _account(self):
        return Account.from_key(self.private_key)

    @property
    def address(self) -> str:
        return to_checksum_address(self._account.address)

    @property
    def public_key(self) -> str:
        """Uncompressed public key without the SEC1 tag (128 hex characters)."""
        return "0x" + self._account._key_obj.public_key.to_hex()[2:]

    @property
    def public_key_compressed(self) -> str:
        return "0x" + self._account._key_obj.public_key.to_compressed_bytes().hex()

    @classmethod
    def from_private_key(cls, raw: str) -> Keypair:
        """Accept a private key with or without the ``0x`` prefix."""
        if raw is None:
            raise errors.invalid_private_key("no private key supplied")
        body = str(raw).strip()
        if body[:2].lower() == "0x":
            body = body[2:]
        if len(body) != 64:
            raise errors.invalid_private_key(
                f"private key must be 64 hex characters, got {len(body)}"
            )
        # `int(body, 16)` accepts underscores, signs and surrounding whitespace;
        # a private key is exactly 64 hex digits and nothing else.
        if not all(c in "0123456789abcdefABCDEF" for c in body):
            raise errors.invalid_private_key("private key is not valid hexadecimal")
        value = int(body, 16)
        # Must be a valid secp256k1 scalar: non-zero and below the curve order.
        curve_order = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141
        if value == 0 or value >= curve_order:
            raise errors.invalid_private_key("private key is not a valid secp256k1 scalar")
        return cls("0x" + body.lower())

    @classmethod
    def from_mnemonic(cls, phrase: str, index: int = 0, passphrase: str = "") -> Keypair:
        """Derive the account at BIP-44 address index ``index``."""
        if not validate_mnemonic(phrase):
            raise errors.invalid_mnemonic(
                "not a valid BIP-39 mnemonic (check the words and the checksum)"
            )
        path = ethereum_path(index)
        try:
            account = Account.from_mnemonic(
                normalize_mnemonic(phrase), account_path=path, passphrase=passphrase
            )
        except Exception as exc:  # pragma: no cover - defensive
            raise errors.invalid_mnemonic(f"cannot derive from mnemonic: {exc}") from exc
        return cls("0x" + account.key.hex().removeprefix("0x"))

    def sign_message(self, message: bytes | str) -> str:
        """Sign a personal message per EIP-191, returning 0x + 65 bytes."""
        if isinstance(message, str):
            message = message.encode("utf-8")
        signed = Account.sign_message(encode_defunct(primitive=message), self.private_key)
        return "0x" + signed.signature.hex().removeprefix("0x")

    def sign_hash(self, digest: bytes) -> str:
        """Sign a 32-byte digest directly."""
        if len(digest) != 32:
            raise errors.usage(f"digest must be 32 bytes, got {len(digest)}")
        signed = Account.unsafe_sign_hash(digest, self.private_key)
        return "0x" + signed.signature.hex().removeprefix("0x")


def eip191_hash(message: bytes | str) -> bytes:
    """``keccak256("\\x19Ethereum Signed Message:\\n" || len || message)``."""
    if isinstance(message, str):
        message = message.encode("utf-8")
    return keccak(b"\x19Ethereum Signed Message:\n" + str(len(message)).encode() + message)


def recover_message(message: bytes | str, signature: str) -> str:
    """Recover the signer of an EIP-191 personal message."""
    if isinstance(message, str):
        message = message.encode("utf-8")
    raw = parse_hex(signature)
    if len(raw) != 65:
        raise errors.usage(f"signature must be 65 bytes, got {len(raw)}")
    try:
        recovered = Account.recover_message(encode_defunct(primitive=message), signature=raw)
    except Exception as exc:
        raise errors.usage(f"could not recover a public key: {exc}") from exc
    return to_checksum_address(recovered)


def parse_address(value: str) -> str:
    """Parse and checksum-normalise an address."""
    text = str(value).strip()
    if not is_hex_address(text):
        raise errors.invalid_address(f"not a valid EVM address: {text}")
    return to_checksum_address(text)


def parse_hex(value: str) -> bytes:
    """Parse ``0x…`` hex into bytes, tolerating a missing prefix."""
    text = str(value).strip()
    if text[:2].lower() == "0x":
        text = text[2:]
    try:
        return bytes.fromhex(text)
    except ValueError as exc:
        raise errors.usage(f"invalid hex: {exc}") from exc
