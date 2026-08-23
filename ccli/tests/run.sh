#!/usr/bin/env bash
#
# Tests for the C front end.
#
# What is worth testing here is the part that is C's: building a request with
# arguments that need escaping, reading four fields back out of a reply,
# choosing a stream and an exit status. The wallet underneath is the same Rust
# the other suites already exercise — repeating that here would test nothing
# new. So these are end-to-end, against a throwaway home, and they are about
# the boundary.

set -uo pipefail

BIN="${1:-./cwbwallet-c}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"

if [[ ! -x "$BIN" ]]; then
  echo "missing $BIN — run 'make build' first" >&2
  exit 1
fi

HOME_DIR="$(mktemp -d)"
trap 'rm -rf "$HOME_DIR"' EXIT
export CAUSEWAYBAY_HOME="$HOME_DIR"
# Never inherit the developer's own secrets or endpoints.
unset CAUSEWAYBAY_MNEMONIC CAUSEWAYBAY_PRIVATE_KEY

MNEMONIC="abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
ADDRESS_0="0x9858EfFD232B4033E47d90003D41EC34EcaEda94"
PRIVATE_KEY="0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727"

passed=0
failed=0

ok() {
  passed=$((passed + 1))
  printf '    \033[32mok\033[0m   %s\n' "$1"
}

fail() {
  failed=$((failed + 1))
  printf '    \033[31mFAIL\033[0m %s\n' "$1"
  shift
  for line in "$@"; do printf '         %s\n' "$line"; done
}

suite() { printf '  \033[2m%s\033[0m\n' "$1"; }

# Assert two values are equal.
same() {
  local what="$1" want="$2" got="$3"
  if [[ "$want" == "$got" ]]; then
    ok "$what"
  else
    fail "$what" "expected: $want" "got:      $got"
  fi
}

# Assert `haystack` contains `needle`.
has() {
  local what="$1" needle="$2" haystack="$3"
  if [[ "$haystack" == *"$needle"* ]]; then
    ok "$what"
  else
    fail "$what" "expected to contain: $needle" "got: $haystack"
  fi
}

# Assert a command exits with a given status.
exits() {
  local what="$1" want="$2"
  shift 2
  "$@" > /dev/null 2>&1
  same "$what" "$want" "$?"
}

printf '\n  Causewaybay Wallet — C tests\n\n'

# --------------------------------------------------------------------- basics

suite "the binary"
same "reports the version" "cwbwallet $(sed -n 's/^version = "\(.*\)"/\1/p' \
  "$(dirname "$0")/../../rustcli/Cargo.toml" | head -1)" "$("$BIN" --version)"
has "prints help" "Usage:" "$("$BIN" --help)"
exits "help is not a failure" 0 "$BIN" --help
exits "an unknown command is a usage error" 2 "$BIN" teleport
exits "a missing account is a handled error" 1 "$BIN" account show ghost

# ------------------------------------------------------------------- envelope

suite "the JSON envelope"
envelope="$("$BIN" --json account new --label alpha)"
same "one line" "1" "$(wc -l <<< "$envelope" | tr -d ' ')"
# Byte for byte the shape SPEC.md §4 describes, with no `human` field: the
# reply from the ABI carries one and it must not be passed through.
has "starts with data" '{"data":{' "$envelope"
has "ends with ok" ',"ok":true}' "$envelope"
if [[ "$envelope" == *'"human"'* ]]; then
  fail "human is dropped from --json output" "found it in: $envelope"
else
  ok "human is dropped from --json output"
fi

error_envelope="$("$BIN" --json account show ghost)"
has "an error envelope carries the code" '"code":"account_not_found"' "$error_envelope"
has "an error envelope ends with ok:false" ',"ok":false}' "$error_envelope"

# stdout is the single machine channel in --json mode, errors included.
same "nothing on stderr in --json mode" "" "$("$BIN" --json account show ghost 2>&1 > /dev/null)"

# ---------------------------------------------------------------- the streams

suite "streams"
has "the warning goes to stderr" "unencrypted" "$("$BIN" info 2>&1 > /dev/null)"
if [[ "$("$BIN" info 2> /dev/null)" == *"unencrypted"* ]]; then
  fail "the warning must not reach stdout" "it did"
else
  ok "the warning stays off stdout"
fi
has "an error goes to stderr with its code" "[account_not_found]" \
  "$("$BIN" account show ghost 2>&1 > /dev/null)"
same "nothing on stdout when a command fails" "" "$("$BIN" account show ghost 2> /dev/null)"

# ------------------------------------------------------------ argument passing

suite "arguments"
# The escaping path. `account show` quotes back whatever it could not find,
# which is exactly the proof wanted here: the argument reached the wallet byte
# for byte through a JSON round trip, quotes and backslashes and all.
rejected="$("$BIN" --json account show 'a "quoted" \ label' 2> /dev/null)"
has "a quoted argument reaches the wallet whole" 'a \"quoted\" \\ label' "$rejected"

# Distinct inputs that differ only in what needs escaping must hash distinctly.
# If a quote or a backslash truncated the argument, two of these would collide.
plain="$("$BIN" utils keccak 'ab' 2> /dev/null)"
quoted="$("$BIN" utils keccak 'a"b' 2> /dev/null)"
escaped="$("$BIN" utils keccak 'a\"b' 2> /dev/null)"
newline="$("$BIN" utils keccak "$(printf 'a\nb')" 2> /dev/null)"
if [[ "$plain" != "$quoted" && "$quoted" != "$escaped" && "$escaped" != "$newline" \
      && "$plain" != "$newline" ]]; then
  ok "quotes, backslashes and newlines survive as themselves"
else
  fail "quotes, backslashes and newlines survive as themselves" \
    "plain=$plain" "quoted=$quoted" "escaped=$escaped" "newline=$newline"
fi

# The empty string is a published vector, and passing it is its own small trap.
same "an empty argument is still an argument" \
  "0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470" \
  "$("$BIN" utils keccak "" 2> /dev/null)"

# A unicode argument must reach the wallet as the bytes that were typed.
same "unicode reaches the wallet intact" \
  "0xb4bc30d9eb64e786d25d430a7f333951d56db400100aae7039ce6bdf7d0aa590" \
  "$("$BIN" utils keccak "🌏" 2> /dev/null)"

# A negative number is an argument, not a flag.
exits "a negative amount is rejected by the wallet, not the shell" 1 \
  "$BIN" utils to-wei -- -1

suite "standard input"
imported="$(printf '%s\n' "$MNEMONIC" | "$BIN" --json account import-mnemonic -m - -l piped)"
has "a dash reads the pipe" "$ADDRESS_0" "$imported"
# Nothing else may block on a pipe nobody wrote to; if this hangs, that broke.
exits "a command with no dash does not wait on stdin" 0 "$BIN" --json info

# ------------------------------------------------------------------- the wallet

suite "the wallet underneath"
"$BIN" --json account import-key -k "$PRIVATE_KEY" -l fromkey > /dev/null
has "a known key gives its known address" "$ADDRESS_0" "$("$BIN" --json account show fromkey)"
same "wei conversion is exact" "1500000000000000000" "$("$BIN" utils to-wei 1.5 2> /dev/null)"
same "the checksum is EIP-55" "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed" \
  "$("$BIN" utils checksum 0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed 2> /dev/null)"

signature="$("$BIN" --json sign "cross-check" --account fromkey \
  | sed -n 's/.*"signature":"\([^"]*\)".*/\1/p')"
has "it signs and verifies its own signature" '"valid":true' \
  "$("$BIN" --json verify --message cross-check --signature "$signature" --address "$ADDRESS_0")"

suite "what it refuses"
exits "the terminal UI belongs to the Rust CLI" 2 "$BIN" tui
has "and it says where to go" "cwbwallet tui" "$("$BIN" tui 2>&1)"
# The global flag's value must not be mistaken for the subcommand.
exits "a home flag before tui is still tui" 2 "$BIN" --home "$HOME_DIR" tui

# ---------------------------------------------------------------------- report

printf '\n'
if [[ $failed -eq 0 ]]; then
  printf '  \033[32m%d passed\033[0m\n\n' "$passed"
else
  printf '  \033[31m%d of %d failed\033[0m\n\n' "$failed" "$((passed + failed))"
  exit 1
fi
