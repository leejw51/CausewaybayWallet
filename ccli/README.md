# Causewaybay Wallet — C

> ⚠️ **Educational software.** Keys are stored unencrypted on disk. Do not use
> with funds you are not prepared to lose. For real value, use a hardware wallet.

The wallet as a single self-contained binary. `libcausewaybay_ffi.a` is linked
in, so `cwbwallet-c` carries the whole thing — key derivation, the store, the
RPC client, the argument parser — with no shared library to find at run time and
nothing to install beside it.

```sh
make build
./cwbwallet-c account new --label main
./cwbwallet-c --json balance
echo "$MNEMONIC" | ./cwbwallet-c account import-mnemonic -m - -l main
```

That is the difference from [`../luacli/`](../luacli), which loads the *dynamic*
library at run time because that is what LÖVE needs. Same library, same wallet,
the two ways a library can be reached — and `scripts/parity.sh` checks the two
front ends emit byte-identical envelopes, since any difference between them can
only be a bug in one of the front ends.

## Building

```sh
make build           # ./cwbwallet-c, statically linked
make test            # build, then the suite
make verify-static   # prove it carries the wallet rather than loading it
make check           # everything CI runs
make package         # signed binary into ../dist/cwbwallet-c
```

Nothing to install: a C compiler and the Rust toolchain, both of which the
repository already needs. `make build` asks the Rust workspace for the release
archive if it is not there.

The system libraries a Rust `staticlib` needs are not hardcoded — the Makefile
asks the toolchain:

```sh
cargo rustc -p causewaybay-ffi --release --crate-type staticlib \
  -- --print native-static-libs
# note: native-static-libs: -framework CoreFoundation -liconv -lSystem -lc -lm
```

Writing that list down by hand is a guess that breaks on the next platform.
Asking is one command and is always right. (On macOS the Makefile then drops
`-lc` and `-lSystem`, which name the same library that `cc` links anyway, and
matches `MACOSX_DEPLOYMENT_TARGET` to the host SDK — without which the linker
reports the version mismatch in `ring`'s prebuilt assembly once per object file.)

The result is around 10 MB, most of it the wallet:

```console
$ otool -L cwbwallet-c
cwbwallet-c:
	/System/Library/Frameworks/CoreFoundation.framework/…/CoreFoundation
	/usr/lib/libiconv.2.dylib
	/usr/lib/libSystem.B.dylib
```

Three system libraries and nothing else. `make verify-static` asserts exactly
that, and then runs the binary with every library search path pointed at a
directory that does not exist — because one Makefile edit from `-lcausewaybay_ffi`
would still build, still pass every other test, and quietly need a `.dylib`
beside it forever after.

## What is in here

| file | what it is |
| --- | --- |
| `src/main.c` | build a request from `argv`, call the ABI, render the reply |
| `src/json.c`, `src/json.h` | an escaping writer and a scanner that finds a key |
| `tests/run.sh` | the suite: escaping, envelopes, streams, exit statuses |
| `tests/verify-static.sh` | the check that the link really is static |

About 550 lines, and the interesting thing is what is missing from them.

**No argument parser.** `argv` goes into the request verbatim and clap parses it
inside the library, so this cannot drift from `cwbwallet` the way a second
hand-written parser would. Every flag, every subcommand, every default is
whatever the Rust CLI has, on the day it gets it.

**No cryptography, no store, no RPC.** All of that is behind one function call.

**Just enough JSON.** Building a request needs an escaping writer; reading a
reply needs four fields out of an object. That is much less than a JSON library,
and vendoring one would put a dependency in a repository whose point is that
each front end stands on its own. The scanner is a real one, though — it skips
nested objects, arrays and escaped strings properly, because a `data` payload
containing a `}` inside a string is an ordinary Tuesday and brace-counting would
break on it.

## The three jobs

```c
/* 1. argv in, JSON out */
json_raw(&buf, "{\"argv\":[");
for (int i = 1; i < argc; i++) {
    if (i > 1) json_raw(&buf, ",");
    json_string(&buf, argv[i]);   /* escapes quotes, backslashes, controls */
}
json_raw(&buf, "]}");

/* 2. one call */
char *reply = cwb_execute(buf.data);

/* 3. read what is needed, free what was given */
if (json_is_true(json_get(reply, "ok"))) {
    char *human = json_unescape(json_get(reply, "human"));
    printf("%s\n", human);
    free(human);
}
cwb_string_free(reply);
```

Every string the library returns belongs to the caller and is released with
`cwb_string_free` — and with nothing else, since it was allocated by Rust.
Strings this program allocates are its own and go to `free`. Keeping the two
straight is most of what the C here has to get right.

`home`, `network` and `yes` are deliberately *not* set in the request: they are
flags inside `argv`, and the library's own parser should be the one to read
them. See [`../SPEC.md`](../SPEC.md) §8 for the request and reply shapes, and
[`../rustcli/README.md`](../rustcli/README.md) for the ABI.

## Tests

```sh
make test
./tests/run.sh ./cwbwallet-c
```

Thirty end-to-end checks against a throwaway wallet home, about the boundary
rather than the wallet: that a quoted or unicode or empty argument reaches the
library byte for byte, that `--json` emits the SPEC §4 envelope with the ABI's
extra `human` field dropped, that the warning banner goes to stderr and never to
stdout, that a command with no `-` in it does not block on a pipe, and that the
exit status is 0, 1 or 2 for the right reasons.

The escaping is checked two ways. Locally, by hashing strings that differ only
in what needs escaping and requiring the hashes to differ — a quote that
truncated an argument would make two of them collide. And in
`scripts/parity.sh`, against the Rust CLI's output for the same awkward
argument, which is the check that says the escaping is not merely
self-consistent but correct.

## Not here

`tui` — the terminal UI belongs to the Rust CLI. Asking for it here says so.
For a menu, `luacli/bin/cwbwallet-lua interactive`.
