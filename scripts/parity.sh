#!/usr/bin/env bash
#
# Cross-implementation parity check.
#
# Every CLI is pointed at one throwaway wallet home. Each writes, the others
# read, and the results must agree — that is what "one specification, several
# front ends" has to mean in practice.
#
# Every front end here is a route to the same Rust core, so what this proves is
# not that four implementations agree — there is one implementation — but that
# nothing on the way through corrupts it. The four routes are genuinely
# different: Python loads the cdylib through ctypes, Lua through LuaJIT'"'"'s FFI,
# C has the staticlib compiled in, and the Rust CLI calls the crate directly.
# A difference between any two of them is a difference in a front end, which is
# the only place it could be.
#
# Python was an independent implementation until it became a binding; the
# checks below are unchanged, but what they are evidence *of* is now the ABI
# and the argument passing rather than two agreeing implementations.
#
# Lua is skipped, loudly, when LuaJIT is not installed; C when it has not been
# built.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# By default the development builds are checked. `make package-verify` points
# these at ./dist instead, so the artifacts that ship are the ones verified —
# not merely the source tree they were built from.
RUST="${CWB_RUST_BIN:-$ROOT/rustcli/target/debug/cwbwallet}"
PY="${CWB_PYTHON_BIN:-$ROOT/pythoncli/.venv/bin/python -m causewaybay}"
LUA="${CWB_LUA_BIN:-$ROOT/luacli/bin/cwbwallet-lua}"
C="${CWB_C_BIN:-$ROOT/ccli/cwbwallet-c}"

VERBOSE=0
[[ "${1:-}" == "--verbose" ]] && VERBOSE=1

# The canonical BIP-39 test phrase. Never put anything of value at its addresses.
MNEMONIC="abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
ADDRESS_0="0x9858EfFD232B4033E47d90003D41EC34EcaEda94"
ADDRESS_1="0x6Fac4D18c912343BF86fa7049364Dd4E424Ab9C0"
PRIVATE_KEY="0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727"

# $PY may be a command with arguments ("python -m causewaybay"), so check the
# executable at the front of it rather than the whole string.
if [[ ! -x "$RUST" ]]; then
  echo "missing $RUST — run 'make build-rust' first" >&2
  exit 1
fi
if [[ ! -x "${PY%% *}" ]]; then
  echo "missing ${PY%% *} — run 'make build-python' first" >&2
  exit 1
fi

# Lua is checked only when it can run at all. Skipping is announced rather than
# silent: a parity run that quietly covered less than it looked like is worse
# than one that failed.
LUA_READY=1
if ! command -v "${LUAJIT:-luajit}" > /dev/null 2>&1; then
  LUA_READY=0
  LUA_SKIP="LuaJIT is not installed"
elif [[ ! -x "$LUA" ]]; then
  LUA_READY=0
  LUA_SKIP="missing $LUA"
elif ! "$LUA" --json info > /dev/null 2>&1; then
  LUA_READY=0
  LUA_SKIP="the shared library is not built — run 'make -C rustcli ffi'"
fi

C_READY=1
if [[ ! -x "$C" ]]; then
  C_READY=0
  C_SKIP="missing $C — run 'make -C ccli build'"
elif ! "$C" --json info > /dev/null 2>&1; then
  C_READY=0
  C_SKIP="$C does not run"
fi

HOME_DIR="$(mktemp -d)"
trap 'rm -rf "$HOME_DIR"' EXIT
export CAUSEWAYBAY_HOME="$HOME_DIR"

checks=0
failures=0

# Compare two values, reporting the field being checked.
expect() {
  local what="$1" want="$2" got="$3"
  checks=$((checks + 1))
  if [[ "$want" == "$got" ]]; then
    [[ $VERBOSE -eq 1 ]] && printf '  ok   %-46s %s\n' "$what" "$got"
    return 0
  fi
  printf '  FAIL %-46s want %s, got %s\n' "$what" "$want" "$got" >&2
  failures=$((failures + 1))
}

# Pull one field out of a --json envelope.
field() {
  python3 -c "
import json, sys
envelope = json.load(sys.stdin)
if not envelope.get('ok'):
    print('ERROR:' + envelope['error']['code'])
    raise SystemExit(0)
value = envelope['data']
for key in sys.argv[1:]:
    value = value[int(key)] if key.lstrip('-').isdigit() else value[key]
print(value if not isinstance(value, bool) else str(value).lower())
" "$@"
}

echo "  rust:        $RUST"
echo "  python:      $PY"
if [[ $LUA_READY -eq 1 ]]; then
  echo "  lua:         $LUA"
else
  echo "  lua:         SKIPPED — $LUA_SKIP"
fi
if [[ $C_READY -eq 1 ]]; then
  echo "  c:           $C"
else
  echo "  c:           SKIPPED — $C_SKIP"
fi
echo "  wallet home: $HOME_DIR"

# --- Rust writes, Python reads ------------------------------------------------
$RUST --json account import-mnemonic -m "$MNEMONIC" -l from-rust > /dev/null
expect "python reads the rust account" "$ADDRESS_0" \
  "$($PY --json account show from-rust | field address)"
expect "python sees the rust recall entry" "mnemonic" \
  "$($PY --json recent list | field 0 kind)"

# --- Python writes, Rust reads ------------------------------------------------
$PY --json account derive -i 1 -l from-python > /dev/null
expect "rust reads the python account" "$ADDRESS_1" \
  "$($RUST --json account show from-python | field address)"
expect "rust agrees on the account id" \
  "$($PY --json account show from-python | field id)" \
  "$($RUST --json account show from-python | field id)"

# --- Both derive the same key material ----------------------------------------
expect "same private key from the same mnemonic" \
  "$($RUST --json account show from-rust --secret | field private_key)" \
  "$($PY --json account show from-rust --secret | field private_key)"

# --- Signatures verify across implementations ---------------------------------
RUST_SIG="$($RUST --json sign "cross-check" --account from-rust | field signature)"
PY_SIG="$($PY --json sign "cross-check" --account from-rust | field signature)"
expect "identical deterministic signatures" "$RUST_SIG" "$PY_SIG"
expect "rust verifies the python signature" "true" \
  "$($RUST --json verify --message cross-check --signature "$PY_SIG" --address "$ADDRESS_0" | field valid)"
expect "python verifies the rust signature" "true" \
  "$($PY --json verify --message cross-check --signature "$RUST_SIG" --address "$ADDRESS_0" | field valid)"

# --- Offline helpers agree ----------------------------------------------------
expect "same keccak256" \
  "$($RUST --json utils keccak hello | field keccak256)" \
  "$($PY --json utils keccak hello | field keccak256)"
expect "same wei conversion" \
  "$($RUST --json utils to-wei 1.5 | field value)" \
  "$($PY --json utils to-wei 1.5 | field value)"

# --- The recall list round-trips ----------------------------------------------
$RUST --json account import-key -k "$PRIVATE_KEY" -l rust-key > /dev/null
expect "python resolves the rust recall id" \
  "$($RUST --json recent show 1 | field id)" \
  "$($PY --json recent show 1 | field id)"
$PY --json --yes account remove rust-key > /dev/null
expect "python restores from the rust recall entry" "$ADDRESS_0" \
  "$($PY --json account import-recent 1 -l restored | field address)"

# --- Both binaries carry the version of record --------------------------------
# The Makefile checks the two manifests agree before packaging; this checks the
# artifacts that came out of it, which is the claim a release actually makes.
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/rustcli/Cargo.toml" | head -1)"
expect "rust reports the manifest version" "$VERSION" "$($RUST --json info | field version)"
expect "python reports the manifest version" "$VERSION" "$($PY --json info | field version)"
expect "rust --version banner" "cwbwallet $VERSION" "$($RUST --version)"
expect "python --version banner" "cwbwallet $VERSION" "$($PY --version)"

# --- Both agree on what is stored ---------------------------------------------
expect "same account count" \
  "$($RUST --json info | field accounts)" \
  "$($PY --json info | field accounts)"
expect "same remembered count" \
  "$($RUST --json info | field remembered)" \
  "$($PY --json info | field remembered)"

# --- One seed, many addresses -------------------------------------------------
# Each implementation adds an address; both must continue the same sequence.
$RUST --json account new -l seq-rust > /dev/null
expect "rust continues the address sequence" "2" \
  "$($RUST --json account show seq-rust | field index)"
$PY --json account new -l seq-python > /dev/null
expect "python continues the same sequence" "3" \
  "$($PY --json account show seq-python | field index)"
# `--format` wraps the rendering in `content`, so read the first row out of it.
first_seed() {
  python3 -c "
import json, sys
content = json.load(sys.stdin)['data']['content']
print(json.loads(content.splitlines()[0])['seed'])"
}
expect "both derived from one seed" \
  "$($RUST --json account list --format jsonl | first_seed)" \
  "$($PY --json account list --format jsonl | first_seed)"

# --- The exports are byte-identical -------------------------------------------
for fmt in jsonl csv txt md; do
  rust_out="$(mktemp)"; py_out="$(mktemp)"
  $RUST --json account list --format "$fmt" --secret \
    | python3 -c "import json,sys;sys.stdout.write(json.load(sys.stdin)['data']['content'])" > "$rust_out"
  $PY --json account list --format "$fmt" --secret \
    | python3 -c "import json,sys;sys.stdout.write(json.load(sys.stdin)['data']['content'])" > "$py_out"
  checks=$((checks + 1))
  if cmp -s "$rust_out" "$py_out"; then
    [[ $VERBOSE -eq 1 ]] && printf '  ok   %-46s %s bytes\n' "identical $fmt export" "$(wc -c < "$rust_out" | tr -d ' ')"
  else
    printf '  FAIL %-46s the two exports differ\n' "identical $fmt export" >&2
    diff "$rust_out" "$py_out" | head -4 >&2
    failures=$((failures + 1))
  fi
  rm -f "$rust_out" "$py_out"
done

# --- Networks agree ------------------------------------------------------------
expect "same network names" \
  "$($RUST --json network list | field 0 name)" \
  "$($PY --json network list | field 0 name)"

# --- Error codes are shared vocabulary ----------------------------------------
expect "same error code for a missing account" \
  "$($RUST --json account show ghost | field)" \
  "$($PY --json account show ghost | field)"

# --- The crypto utilities agree, and store nothing ----------------------------
# `utils derive`, `utils sign` and `utils validate-mnemonic` take key material
# as an argument and are expected to leave no trace. Both implementations
# compute them independently, so agreement here is real evidence — and the
# counts afterwards are the check that neither quietly kept a copy.
BEFORE_ACCOUNTS="$($RUST --json info | field accounts)"
BEFORE_RECALL="$($RUST --json info | field remembered)"

expect "same derived address from one phrase" \
  "$($RUST --json utils derive -m "$MNEMONIC" -i 2 | field address)" \
  "$($PY --json utils derive -m "$MNEMONIC" -i 2 | field address)"
expect "same derived private key" \
  "$($RUST --json utils derive -m "$MNEMONIC" -i 2 | field private_key)" \
  "$($PY --json utils derive -m "$MNEMONIC" -i 2 | field private_key)"
expect "same public key from one private key" \
  "$($RUST --json utils derive -k "$PRIVATE_KEY" | field public_key)" \
  "$($PY --json utils derive -k "$PRIVATE_KEY" | field public_key)"
expect "same ad-hoc signature" \
  "$($RUST --json utils sign -k "$PRIVATE_KEY" -m "off the books" | field signature)" \
  "$($PY --json utils sign -k "$PRIVATE_KEY" -m "off the books" | field signature)"
expect "both accept a valid mnemonic" \
  "$($RUST --json utils validate-mnemonic "$MNEMONIC" | field valid)" \
  "$($PY --json utils validate-mnemonic "$MNEMONIC" | field valid)"
expect "both reject the same bad one" \
  "$($RUST --json utils validate-mnemonic "abandon abandon" | field valid)" \
  "$($PY --json utils validate-mnemonic "abandon abandon" | field valid)"

expect "deriving stored no account" "$BEFORE_ACCOUNTS" "$($RUST --json info | field accounts)"
expect "deriving remembered nothing" "$BEFORE_RECALL" "$($PY --json info | field remembered)"

# --- The Lua front end reads the same store -----------------------------------
# Not an independent implementation — it is the Rust core reached over the C
# ABI. What these checks cover is the trip through that boundary: strings that
# could be truncated, big integers that could become floats, an envelope that
# could be reshaped by a JSON codec written in Lua.
if [[ $LUA_READY -eq 1 ]]; then
  expect "lua reads an account rust wrote" "$ADDRESS_0" \
    "$($LUA --json account show from-rust | field address)"
  expect "lua agrees on the account id" \
    "$($RUST --json account show from-python | field id)" \
    "$($LUA --json account show from-python | field id)"
  expect "lua agrees on the account count" \
    "$($RUST --json info | field accounts)" \
    "$($LUA --json info | field accounts)"

  LUA_SIG="$($LUA --json sign "cross-check" --account from-rust | field signature)"
  expect "lua produces the same signature" "$RUST_SIG" "$LUA_SIG"
  expect "rust verifies the lua signature" "true" \
    "$($RUST --json verify --message cross-check --signature "$LUA_SIG" --address "$ADDRESS_0" | field valid)"
  expect "lua verifies the python signature" "true" \
    "$($LUA --json verify --message cross-check --signature "$PY_SIG" --address "$ADDRESS_0" | field valid)"

  expect "lua derives the same address" \
    "$($RUST --json utils derive -m "$MNEMONIC" -i 2 | field address)" \
    "$($LUA --json utils derive -m "$MNEMONIC" -i 2 | field address)"
  expect "lua agrees on keccak256" \
    "$($RUST --json utils keccak hello | field keccak256)" \
    "$($LUA --json utils keccak hello | field keccak256)"
  # The value every layer between here and Rust would like to make a double.
  expect "lua carries a 256-bit integer intact" \
    "115792089237316195423570985008687907853269984665640564039457584007913129639935" \
    "$($LUA --json utils to-wei 115792089237316195423570985008687907853269984665640564039457.584007913129639935 | field value)"

  expect "lua reports the manifest version" "$VERSION" "$($LUA --json info | field version)"
  expect "lua shares the error vocabulary" \
    "$($RUST --json account show ghost | field)" \
    "$($LUA --json account show ghost | field)"

  # Lua writes; the other two must see it, which is the direction that proves
  # the store is genuinely shared rather than merely readable.
  $LUA --json account new -l seq-lua > /dev/null
  expect "rust sees the account lua added" "4" \
    "$($RUST --json account show seq-lua | field index)"
  expect "python sees it too" \
    "$($LUA --json account show seq-lua | field address)" \
    "$($PY --json account show seq-lua | field address)"
fi

# --- The C front end reads the same store -------------------------------------
# Same library as Lua, reached the other way: compiled in rather than loaded.
# What that makes worth checking is the C side of the boundary — the JSON it
# builds from argv, and the four fields it reads back out by hand.
if [[ $C_READY -eq 1 ]]; then
  expect "c reads an account rust wrote" "$ADDRESS_0" \
    "$($C --json account show from-rust | field address)"
  expect "c agrees on the account id" \
    "$($RUST --json account show from-python | field id)" \
    "$($C --json account show from-python | field id)"

  C_SIG="$($C --json sign "cross-check" --account from-rust | field signature)"
  expect "c produces the same signature" "$RUST_SIG" "$C_SIG"
  expect "python verifies the c signature" "true" \
    "$($PY --json verify --message cross-check --signature "$C_SIG" --address "$ADDRESS_0" | field valid)"

  # The escaping the C front end does by hand, checked against the one serde
  # does for the Rust CLI. A quote or a backslash mangled on the way in would
  # hash differently here and nowhere else.
  TRICKY='a "quoted" \ argument'
  expect "c escapes arguments exactly as rust does" \
    "$($RUST --json utils keccak "$TRICKY" | field keccak256)" \
    "$($C --json utils keccak "$TRICKY" | field keccak256)"
  expect "c derives the same address" \
    "$($RUST --json utils derive -m "$MNEMONIC" -i 2 | field address)" \
    "$($C --json utils derive -m "$MNEMONIC" -i 2 | field address)"
  expect "c carries a 256-bit integer intact" \
    "115792089237316195423570985008687907853269984665640564039457584007913129639935" \
    "$($C --json utils to-wei 115792089237316195423570985008687907853269984665640564039457.584007913129639935 | field value)"

  expect "c reports the manifest version" "$VERSION" "$($C --json info | field version)"
  expect "c --version banner" "cwbwallet $VERSION" "$($C --version)"
  expect "c shares the error vocabulary" \
    "$($RUST --json account show ghost | field)" \
    "$($C --json account show ghost | field)"

  # C writes; everything else must see it.
  $C --json account new -l seq-c > /dev/null
  expect "rust sees the account c added" \
    "$($C --json account show seq-c | field address)" \
    "$($RUST --json account show seq-c | field address)"
  expect "python sees it too" \
    "$($C --json account show seq-c | field address)" \
    "$($PY --json account show seq-c | field address)"

  # The two ABI front ends must be byte-identical in --json mode: they are the
  # same library, so any difference is one of them mishandling the envelope.
  if [[ $LUA_READY -eq 1 ]]; then
    expect "c and lua emit the same envelope" \
      "$($LUA --json account show from-rust)" \
      "$($C --json account show from-rust)"
    expect "…and the same one for a failure" \
      "$($LUA --json account show ghost)" \
      "$($C --json account show ghost)"
  fi
  expect "c and rust emit the same envelope" \
    "$($RUST --json account show from-rust)" \
    "$($C --json account show from-rust)"
fi

echo
if [[ $LUA_READY -eq 0 ]]; then
  echo "  note: the Lua checks were skipped — $LUA_SKIP" >&2
fi
if [[ $C_READY -eq 0 ]]; then
  echo "  note: the C checks were skipped — $C_SKIP" >&2
fi
if [[ $failures -eq 0 ]]; then
  echo "  $checks parity checks passed"
else
  echo "  $failures of $checks parity checks FAILED" >&2
  exit 1
fi
