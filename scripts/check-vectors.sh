#!/usr/bin/env bash
#
# The vectors are generated, so regenerating them must be a no-op. If this fails,
# either the generator changed on purpose (commit the new files) or a dependency
# changed its behaviour underneath us (worth knowing about).

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PYTHON="$ROOT/pythoncli/.venv/bin/python"

if [[ ! -x "$PYTHON" ]]; then
  echo "missing the Python virtualenv — run 'make build-python' first" >&2
  exit 1
fi

BACKUP="$(mktemp -d)"
trap 'rm -rf "$BACKUP"' EXIT
cp "$ROOT"/testvectors/*.json "$BACKUP/"

"$PYTHON" "$ROOT/scripts/gen-vectors.py" > /dev/null

drift=0
for file in "$BACKUP"/*.json; do
  name="$(basename "$file")"
  if ! diff -q "$file" "$ROOT/testvectors/$name" > /dev/null; then
    echo "  DRIFT $name — the committed file differs from a fresh generation" >&2
    drift=$((drift + 1))
  fi
done

if [[ $drift -ne 0 ]]; then
  echo "  $drift vector file(s) drifted; review the diff and commit if intended" >&2
  exit 1
fi

count=$(ls -1 "$ROOT"/testvectors/*.json | wc -l | tr -d ' ')
echo "  $count vector files reproduce exactly, and every published constant matched"
