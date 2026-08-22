#!/usr/bin/env bash
#
# Cross-implementation parity check.
#
# Both CLIs are pointed at one throwaway wallet home. Each writes, the other
# reads, and the results must agree — that is what "one specification, two
# implementations" has to mean in practice.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# By default the development builds are checked. `make package-verify` points
# these at ./dist instead, so the artifacts that ship are the ones verified —
# not merely the source tree they were built from.
RUST="${CWB_RUST_BIN:-$ROOT/rustcli/target/debug/cwbwallet}"
PY="${CWB_PYTHON_BIN:-$ROOT/pythoncli/.venv/bin/python -m causewaybay}"

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

echo
if [[ $failures -eq 0 ]]; then
  echo "  $checks parity checks passed"
else
  echo "  $failures of $checks parity checks FAILED" >&2
  exit 1
fi
