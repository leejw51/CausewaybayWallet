"""Causewaybay Wallet for Python — a binding over the Rust core's C ABI.

The Rust core does the cryptography, the storage and the RPC; this turns its
JSON envelopes into Python objects and its error codes into exceptions you can
branch on::

    from causewaybay import open_wallet

    wallet = open_wallet()
    for account in wallet.accounts():
        print(account["label"], account["address"])

Every call raises :class:`~causewaybay.errors.WalletError` on failure, with a
``.code`` that is one of the stable strings in ``SPEC.md``. Nothing here
guesses at what the wallet would do: the command surface, the store format and
the four chains are defined once, in Rust, and read from the library.

The same module backs the Python CLI and its menu: open one wallet for the life
of the program, set ``yes=True`` once your own confirmation is wired up, and
never block on a prompt.
"""

from __future__ import annotations

import json
from typing import Any

from . import errors, ffi

__all__ = ["Wallet", "open_wallet", "COMMANDS"]


class Wallet:
    """One open wallet, over one loaded library.

    ``home``, ``network`` and ``chain`` are defaults for every call; each can be
    overridden per call. A flag inside ``argv`` beats both, which is what lets a
    GUI hold a network in its own state and still honour a one-off override.
    """

    def __init__(
        self,
        library: ffi.Library,
        home: str | None = None,
        network: str | None = None,
        chain: str | None = None,
        yes: bool = False,
    ) -> None:
        self.library = library
        self.home = home
        self.network = network
        self.chain = chain
        self.yes = yes

    # ------------------------------------------------------------- handshake

    def version(self) -> str:
        """The wallet version the loaded library reports."""
        return self.library.version()

    def describe(self) -> dict[str, Any]:
        """Name, version, ABI, networks, chains and error codes."""
        return self._decode(self.library.describe())["data"]

    def chains(self) -> list[dict[str, Any]]:
        """The chains this build supports, straight from the library.

        Read rather than listed here, so a chain picker cannot go stale when a
        chain is added — the same reason ``codes`` and ``commands`` are read.
        """
        return self._decode(self.library.chains())["data"]

    def commands(self) -> list[dict[str, Any]]:
        """Every command the library accepts: ``{path, name, about, args}``.

        The test suite checks :data:`COMMANDS` against this, so adding a
        command in Rust and forgetting the Python method fails the build.
        """
        return self._decode(self.library.commands())["data"]

    def codes(self) -> set[str]:
        """Every error code this build can return."""
        return set(self.describe()["codes"])

    # --------------------------------------------------------------- calling

    def envelope(
        self,
        argv: list[str],
        *,
        home: str | None = None,
        network: str | None = None,
        chain: str | None = None,
        yes: bool | None = None,
        stdin: str | None = None,
    ) -> dict[str, Any]:
        """Run one command and return the whole envelope.

        A command that *failed* still returns an envelope, with ``ok`` false.
        Only a reply this binding could not read raises.
        """
        for index, word in enumerate(argv):
            if not isinstance(word, str):
                # Numbers are the easy mistake — `["utils", "to-wei", 1.5]`
                # would otherwise reach the wallet as "1.5" only by luck of
                # formatting.
                raise errors.usage(
                    f"argv[{index}] is a {type(word).__name__}; every argument must be a string"
                )

        request: dict[str, Any] = {"argv": list(argv)}
        home = home if home is not None else self.home
        network = network if network is not None else self.network
        chain = chain if chain is not None else self.chain
        yes = self.yes if yes is None else yes
        if home:
            request["home"] = str(home)
        if network:
            request["network"] = network
        if chain:
            request["chain"] = chain
        if yes:
            request["yes"] = True
        if stdin is not None:
            request["stdin"] = stdin

        return self._decode(self.library.execute(json.dumps(request)))

    def call(self, argv: list[str], **options: Any) -> Any:
        """Run one command and return its ``data``, raising on failure."""
        envelope = self.envelope(argv, **options)
        if not envelope.get("ok"):
            failure = envelope.get("error") or {}
            raise errors.WalletError(failure.get("code", "internal"), failure.get("message", ""))
        return envelope.get("data")

    def text(self, argv: list[str], **options: Any) -> str:
        """Run one command and return the text the CLI would have printed.

        Used by the Python CLI, so its output matches the Rust CLI's exactly
        without either one knowing the other's formatting rules.
        """
        envelope = self.envelope(argv, **options)
        if not envelope.get("ok"):
            failure = envelope.get("error") or {}
            raise errors.WalletError(failure.get("code", "internal"), failure.get("message", ""))
        return envelope.get("human") or ""

    @staticmethod
    def _decode(reply: str) -> dict[str, Any]:
        if not reply:
            raise errors.internal("the wallet returned nothing")
        try:
            return json.loads(reply)
        except ValueError as exc:
            raise errors.internal(f"the reply was not JSON: {exc}") from None

    # ------------------------------------------------------------ the wallet

    def info(self) -> dict[str, Any]:
        """Where state lives and what is configured."""
        return self.call(["info"])

    # -------------------------------------------------------------- accounts

    def accounts(self, *, secret: bool = False) -> list[dict[str, Any]]:
        """Every account, oldest first."""
        return self.call(_flags(["account", "list"], secret=secret))

    def export_accounts(
        self, fmt: str, *, secret: bool = False, output: str | None = None
    ) -> dict[str, Any]:
        """The account list as ``jsonl``, ``csv``, ``txt`` or ``md``.

        Returns the envelope's data, whose ``content`` holds the text when no
        ``output`` path is given. With ``secret`` the file carries private keys
        and mnemonics, and is written owner-only.
        """
        return self.call(_flags(["account", "list"], format=fmt, output=output, secret=secret))

    def account(self, selector: str | None = None, *, secret: bool = False) -> dict[str, Any]:
        """One account by id, label or address; ``None`` means the active one."""
        return self.call(_positional(_flags(["account", "show"], secret=secret), selector))

    def new_account(
        self,
        *,
        label: str | None = None,
        index: int | None = None,
        chain: str | None = None,
        every_chain: bool = False,
        new_seed: bool = False,
        words: int | None = None,
        show_secret: bool = False,
    ) -> Any:
        """Make a wallet: one index, derived on the chain in view.

        ``every_chain`` derives it on all four at once, which is what the
        wallet means by "a wallet"; ``new_seed`` starts a separate mnemonic.
        """
        return self.call(
            _flags(
                ["account", "new"],
                label=label,
                index=index,
                chain=chain,
                every_chain=every_chain,
                new_seed=new_seed,
                words=words,
                show_secret=show_secret,
            )
        )

    def import_mnemonic(self, mnemonic: str, **opts: Any) -> Any:
        """Import a BIP-39 phrase. Pass the phrase itself, not ``-``."""
        return self.call(
            _flags(["account", "import-mnemonic"], mnemonic=mnemonic, **opts),
            **_call_options(opts),
        )

    def import_key(self, private_key: str, **opts: Any) -> Any:
        """Import a raw private key, in the chain's own encoding."""
        return self.call(
            _flags(["account", "import-key"], private_key=private_key, **opts),
            **_call_options(opts),
        )

    def derive_account(self, index: int, **opts: Any) -> Any:
        """Derive the wallet at ``index`` from an existing mnemonic."""
        return self.call(_flags(["account", "derive"], index=index, **opts))

    def use_account(self, selector: str) -> Any:
        """Make an account the default for later commands."""
        return self.call(["account", "use", selector])

    def rename_account(self, selector: str, label: str) -> Any:
        return self.call(["account", "rename", selector, label])

    def remove_account(self, selector: str, *, yes: bool = False) -> Any:
        """Forget an account. The wallet asks first unless ``yes``."""
        return self.call(["account", "remove", selector], yes=yes)

    def export_account(self, selector: str | None = None) -> dict[str, Any]:
        """One account *with* its secrets."""
        return self.call(_positional(["account", "export"], selector))

    def import_recent(self, selector: str, **opts: Any) -> Any:
        """Import from the recall list rather than by typing a secret again."""
        return self.call(_flags(["account", "import-recent", selector], **opts))

    # ---------------------------------------------------------------- recall

    def recent(self, *, secret: bool = False) -> list[dict[str, Any]]:
        """Mnemonics and keys this wallet has seen before."""
        return self.call(_flags(["recent", "list"], secret=secret))

    def recent_entry(self, selector: str, *, secret: bool = False) -> dict[str, Any]:
        return self.call(_flags(["recent", "show", selector], secret=secret))

    def forget_recent(self, selector: str, *, yes: bool = False) -> Any:
        return self.call(["recent", "forget", selector], yes=yes)

    def clear_recent(self, *, yes: bool = False) -> Any:
        return self.call(["recent", "clear"], yes=yes)

    # ------------------------------------------------------------- the chain

    def chain_list(self) -> list[dict[str, Any]]:
        """The supported chains, through the command surface.

        The same list :meth:`chains` reads from the library. This one goes
        through ``call``, so it honours a per-call ``home`` like everything
        else; the handshake does not need one.
        """
        return self.call(["chains"])

    def networks(self, filter: str | None = None) -> list[dict[str, Any]]:
        """Every network, of every chain — or the ones a search keeps.

        The same search the tokens use: key, name, symbol, chain and tags,
        every word matching. ``"testnet"`` reaches all six test networks
        including the three whose names never say so.
        """
        argv = ["network", "list"]
        if filter:
            argv.append(filter)
        return self.call(argv)

    def current_network(self) -> dict[str, Any]:
        return self.call(["network", "current"])

    def use_network(self, key: str) -> Any:
        """Change the stored default network — and with it, the chain."""
        return self.call(["network", "use", key])

    def set_rpc(self, network: str, url: str | None = None) -> Any:
        """Override a network's RPC URL. An empty URL restores the default."""
        return self.call(["network", "set-rpc", network, url or ""])

    def set_max_fee(self, network: str, amount: str) -> Any:
        """Refuse any fee above ``amount`` on ``network``. ``"0"`` restores the built-in one.

        Counted in the token that pays the fee, which is the native token
        everywhere except Midnight — a Midnight transfer moves NIGHT and pays
        in DUST. Write the unit to have it checked: ``"2 DUST"`` is accepted,
        ``"2 NIGHT"`` is refused rather than read as DUST.
        """
        return self.call(["network", "set-max-fee", network, str(amount)])

    def balance(self, **opts: Any) -> dict[str, Any]:
        """The native token balance."""
        return self.call(_flags(["balance"], **opts), **_call_options(opts))

    def nonce(self, **opts: Any) -> dict[str, Any]:
        return self.call(_flags(["nonce"], **opts), **_call_options(opts))

    def gas_price(self, **opts: Any) -> dict[str, Any]:
        return self.call(_flags(["gas-price"], **opts), **_call_options(opts))

    def chain_info(self, **opts: Any) -> dict[str, Any]:
        """What the node says about itself."""
        return self.call(_flags(["chain-info"], **opts), **_call_options(opts))

    def airdrop(self, **opts: Any) -> dict[str, Any]:
        """Ask a test network for funds, where the wallet can ask at all.

        Only Solana's clusters answer a faucet request over the endpoint the
        balance came from. Everywhere else this refuses and names the web page
        that hands the money out — the ``faucet`` field on the network row —
        because those faucets are forms with captchas and no retry will get
        past one.
        """
        return self.call(_flags(["airdrop"], **opts), **_call_options(opts))

    def send(self, *, yes: bool = False, **opts: Any) -> dict[str, Any]:
        """Send the chain's native coin."""
        return self.call(_flags(["send"], **opts), yes=yes, **_call_options(opts))

    def tx(self, tx_hash: str, **opts: Any) -> dict[str, Any]:
        return self.call(_flags(["tx", tx_hash], **opts), **_call_options(opts))

    def history(self, **opts: Any) -> list[dict[str, Any]]:
        return self.call(_flags(["history"], **opts), **_call_options(opts))

    def sign(self, message: str, **opts: Any) -> dict[str, Any]:
        """Sign a message with the chain's own scheme."""
        return self.call(_flags(["sign", message], **opts), **_call_options(opts))

    def verify(self, message: str, signature: str, address: str | None = None) -> dict[str, Any]:
        """Check a signature. Without an address, EVM recovers the signer."""
        return self.call(_flags(["verify"], message=message, signature=signature, address=address))

    # ---------------------------------------------------------------- tokens

    def erc20_info(self, token: str, **opts: Any) -> dict[str, Any]:
        """A token's name, symbol, decimals and supply, read from its contract."""
        return self.call(_flags(["erc20", "info"], token=token, **opts), **_call_options(opts))

    def erc20_balance(self, token: str, **opts: Any) -> dict[str, Any]:
        return self.call(_flags(["erc20", "balance"], token=token, **opts), **_call_options(opts))

    def erc20_send(self, *, yes: bool = False, **opts: Any) -> dict[str, Any]:
        return self.call(_flags(["erc20", "send"], **opts), yes=yes, **_call_options(opts))

    # The names these three had before the registry arrived. `token_*` now
    # means the flat, named registry below — which is what someone reaching
    # for the name would expect — but a caller written against the old ones
    # must not break for a rename, so both names reach the same command.
    erc20_token_info = erc20_info
    erc20_token_balance = erc20_balance
    erc20_token_send = erc20_send

    # ------------------------------------------------------------- the tokens
    #
    # One flat row per token per network — `usdc-cronos-mainnet`, said "USDC
    # Cronos Mainnet". Naming a row settles the chain, the network and the
    # contract at once, which is the whole reason the table is flat.

    def tokens(self, filter: str | None = None) -> list[dict[str, Any]]:
        """Every token this wallet knows by name, or the ones a search keeps.

        The filter is matched against each row's key, name, symbol, chain,
        network and tags, and every word has to match — so ``"usdc cronos"``
        is one row and ``"stablecoin"`` is all of them. An empty filter is
        every row, because a wallet that hides tokens until you type has lost
        them rather than tidied them.
        """
        argv = ["token", "list"]
        if filter:
            argv.append(filter)
        return self.call(argv)

    def token_info(self, token: str) -> dict[str, Any]:
        """One registry row: where it lives, how it is counted, what moves it."""
        return self.call(["token", "info", token])

    def token_balance(self, token: str, address: str | None = None) -> dict[str, Any]:
        """A token balance, on that token's own network.

        The network in view is not consulted beyond disambiguating a bare
        symbol: ``token_balance("usdc-solana-mainnet")`` answers from Solana
        whatever the wallet is currently pointed at, and moves nothing.
        """
        return self.call(_flags(["token", "balance", token], address=address))

    def token_send(
        self,
        token: str,
        *,
        to: str,
        amount: str,
        yes: bool = False,
        **opts: Any,
    ) -> dict[str, Any]:
        """Transfer a token. ``to`` is the holder's ordinary address on that chain."""
        return self.call(
            _flags(["token", "send", token], to=to, amount=amount, **opts),
            yes=yes,
            **_call_options(opts),
        )

    # ----------------------------------------------------------- offline bits

    def keccak(self, text: str, **opts: Any) -> dict[str, Any]:
        return self.call(_flags(["utils", "keccak", text], **opts))

    def checksum(self, address: str) -> dict[str, Any]:
        return self.call(["utils", "checksum", address])

    def to_wei(self, amount: str, decimals: int | None = None) -> dict[str, Any]:
        return self.call(_flags(["utils", "to-wei", str(amount)], decimals=decimals))

    def from_wei(self, value: str, decimals: int | None = None) -> dict[str, Any]:
        return self.call(_flags(["utils", "from-wei", str(value)], decimals=decimals))

    def new_mnemonic(self, words: int | None = None) -> dict[str, Any]:
        return self.call(_flags(["utils", "new-mnemonic"], words=words))

    def derive(self, **opts: Any) -> dict[str, Any]:
        """Derive an address from a phrase or a key, storing nothing.

        Follows ``chain`` like everything else, so this is how you ask what a
        phrase gives you on Solana without acquiring an account first.
        """
        if (opts.get("mnemonic") is None) == (opts.get("private_key") is None):
            raise errors.usage("pass exactly one of mnemonic or private_key")
        return self.call(_flags(["utils", "derive"], **opts), **_call_options(opts))

    def sign_with(self, private_key: str, message: str) -> dict[str, Any]:
        """Sign with a key the wallet does not hold."""
        return self.call(_flags(["utils", "sign"], private_key=private_key, message=message))

    def validate_mnemonic(self, phrase: str) -> dict[str, Any]:
        return self.call(["utils", "validate-mnemonic", phrase])


def open_wallet(
    *,
    home: str | None = None,
    network: str | None = None,
    chain: str | None = None,
    yes: bool = False,
    lib: str | None = None,
) -> Wallet:
    """Open a wallet over the shared library.

    Raises ``io_error`` when the library cannot be found — a setup problem
    rather than a wallet one.
    """
    return Wallet(ffi.load(lib), home=home, network=network, chain=chain, yes=yes)


# --------------------------------------------------------------- argv helpers

# Keyword arguments that are *not* command flags: they configure the call.
_CALL_ONLY = frozenset({"home", "network", "chain", "yes", "stdin"})

# Flags the wallet spells differently from Python.
_RENAMED = {"private_key": "private-key", "new_seed": "new-seed", "show_secret": "show-secret"}


def _call_options(opts: dict[str, Any]) -> dict[str, Any]:
    """The subset of ``opts`` that belongs on the request, not in argv."""
    return {name: opts[name] for name in _CALL_ONLY if name in opts}


def _flags(argv: list[str], **opts: Any) -> list[str]:
    """Append ``--flag value`` pairs, skipping the ones left None.

    ``True`` means a flag that stands alone (``--secret``); anything else is
    rendered as its own word after the flag. Underscores become dashes, so
    Python's ``gas_price_gwei`` reaches the wallet as ``--gas-price-gwei``.

    ``chain`` and the other request fields are deliberately not rendered here:
    they travel on the request, where a flag inside argv can still beat them.
    """
    for name, value in opts.items():
        if name in _CALL_ONLY or value is None or value is False:
            continue
        argv.append("--" + _RENAMED.get(name, name.replace("_", "-")))
        if value is not True:
            argv.append(str(value))
    return argv


def _positional(argv: list[str], value: Any) -> list[str]:
    """Append a positional argument, when there is one to append."""
    if value is not None:
        argv.append(str(value))
    return argv


# Every command the library has, and the method that covers it.
#
# Checked against what the library reports, so adding a command in Rust and
# forgetting the Python method fails in the test suite rather than in a user's
# hands. Anything here can still be reached the long way with ``wallet.call``.
COMMANDS: dict[str, str | tuple[str, ...] | None] = {
    "info": "info",
    "account new": "new_account",
    "account import-mnemonic": "import_mnemonic",
    "account import-key": "import_key",
    "account list": ("accounts", "export_accounts"),
    "account show": "account",
    "account use": "use_account",
    "account derive": "derive_account",
    "account rename": "rename_account",
    "account remove": "remove_account",
    "account export": "export_account",
    "account import-recent": "import_recent",
    "recent list": "recent",
    "recent show": "recent_entry",
    "recent forget": "forget_recent",
    "recent clear": "clear_recent",
    "chains": ("chains", "chain_list"),
    "network list": "networks",
    "network current": "current_network",
    "network use": "use_network",
    "network set-rpc": "set_rpc",
    "network set-max-fee": "set_max_fee",
    "balance": "balance",
    "nonce": "nonce",
    "gas-price": "gas_price",
    "chain-info": "chain_info",
    "airdrop": "airdrop",
    "send": "send",
    "tx": "tx",
    "history": "history",
    "sign": "sign",
    "verify": "verify",
    "erc20 info": ("erc20_info", "erc20_token_info"),
    "erc20 balance": ("erc20_balance", "erc20_token_balance"),
    "erc20 send": ("erc20_send", "erc20_token_send"),
    "token list": "tokens",
    "token info": "token_info",
    "token balance": "token_balance",
    "token send": "token_send",
    "utils keccak": "keccak",
    "utils checksum": "checksum",
    "utils to-wei": "to_wei",
    "utils from-wei": "from_wei",
    "utils new-mnemonic": "new_mnemonic",
    "utils derive": "derive",
    "utils sign": "sign_with",
    "utils validate-mnemonic": "validate_mnemonic",
    # The terminal UI belongs to the Rust CLI, which owns the terminal.
    "tui": None,
}
