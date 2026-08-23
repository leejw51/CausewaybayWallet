#!/usr/bin/env bash
#
# Prove the binary carries the wallet rather than reaching for a copy of it.
#
# This is the claim the whole directory rests on, and it is exactly the kind of
# claim that quietly stops being true — one Makefile edit from `-lcausewaybay_ffi`
# and everything still builds, still passes every other test, and now needs a
# .dylib beside it. So it is checked rather than assumed.

set -euo pipefail

BIN="${1:-./cwbwallet-c}"

if [[ ! -x "$BIN" ]]; then
  echo "missing $BIN — run 'make build' first" >&2
  exit 1
fi

case "$(uname -s)" in
  Darwin) LINKED=$(otool -L "$BIN" | tail -n +2) ;;
  *)      LINKED=$(ldd "$BIN" 2>/dev/null || true) ;;
esac

if grep -qi 'causewaybay' <<< "$LINKED"; then
  echo "  FAIL: $BIN loads a Causewaybay library at run time" >&2
  grep -i 'causewaybay' <<< "$LINKED" >&2
  exit 1
fi

# The other half of the claim: the wallet has to actually be inside it. A
# binary that links nothing and does nothing would pass the check above.
if ! "$BIN" --version > /dev/null; then
  echo "  FAIL: $BIN does not run" >&2
  exit 1
fi

# And it must keep working with no library anywhere on the search path.
if ! env CAUSEWAYBAY_LIB=/nonexistent DYLD_LIBRARY_PATH=/nonexistent \
     LD_LIBRARY_PATH=/nonexistent "$BIN" --version > /dev/null; then
  echo "  FAIL: $BIN needs a library on the search path" >&2
  exit 1
fi

echo "  statically linked: $(wc -l <<< "$LINKED" | tr -d ' ') system libraries, no wallet library"
