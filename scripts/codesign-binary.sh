#!/usr/bin/env bash
#
# Sign a standalone macOS executable, and notarize it when there is a real
# identity to notarize with.
#
# Environment contract (the same one PocketSkynet uses, so a single exported set
# of credentials covers both repositories):
#
#   APPLE_SIGNING_IDENTITY  Developer ID Application: …   (default: `-`, ad-hoc)
#   APPLE_ID                the Apple ID for notarytool
#   APPLE_PASSWORD          its app-specific password
#   APPLE_TEAM_ID           the team the certificate belongs to
#
# Why this stops at notarization and does not staple: a ticket cannot be
# attached to a bare Mach-O. `stapler` writes into a bundle's
# Contents/CodeResources or a disk image's metadata, and a single executable has
# nowhere to put one. A notarized CLI binary therefore still costs the first
# machine that runs it one online check with Apple. Shipping it inside a signed,
# notarized, stapled .dmg is the only way around that, and a wallet binary handed
# over as a build artifact does not need it.
#
# A no-op off macOS, so the Linux and Windows packaging paths can call it
# unconditionally.

set -euo pipefail

BIN="${1:?usage: codesign-binary.sh <path-to-executable>}"

if [[ "$(uname -s)" != "Darwin" ]]; then
    exit 0
fi

if [[ ! -f "$BIN" ]]; then
    echo "error: no such file: $BIN" >&2
    exit 1
fi

IDENTITY="${APPLE_SIGNING_IDENTITY:--}"

if ! command -v codesign >/dev/null 2>&1; then
    echo "  note: codesign not available, leaving $(basename "$BIN") unsigned"
    exit 0
fi

if [[ "$IDENTITY" == "-" ]]; then
    # Ad-hoc. On arm64 every executable must carry *some* signature to run at
    # all, and this keeps Gatekeeper's complaint to the one it should be making
    # — unknown developer — rather than a broken-binary error.
    echo "  signing $(basename "$BIN") ad-hoc"
    codesign --force --sign - "$BIN"
    exit 0
fi

# `--options runtime` is the hardened runtime, without which Apple will not
# notarize; `--timestamp` gets a secure timestamp, without which the signature
# expires along with the certificate.
echo "  signing $(basename "$BIN") as $IDENTITY"
codesign --force --options runtime --timestamp --sign "$IDENTITY" "$BIN"
codesign --verify --strict "$BIN"

if [[ -z "${APPLE_ID:-}" || -z "${APPLE_PASSWORD:-}" || -z "${APPLE_TEAM_ID:-}" ]]; then
    echo "  note: no notarization credentials, $(basename "$BIN") is signed only"
    exit 0
fi

# notarytool takes an archive, never a loose executable.
ZIP="$(mktemp -d)/$(basename "$BIN").zip"
trap 'rm -rf "$(dirname "$ZIP")"' EXIT
ditto -c -k "$BIN" "$ZIP"

echo "  notarizing $(basename "$BIN")"
xcrun notarytool submit "$ZIP" \
    --apple-id "$APPLE_ID" \
    --password "$APPLE_PASSWORD" \
    --team-id "$APPLE_TEAM_ID" \
    --wait --timeout 30m

# No `stapler staple` here — see the header. The check below is what a user's
# machine does on first run, so failing it now is worth knowing about.
if ! spctl --assess --type execute "$BIN" 2>/dev/null; then
    echo "  note: spctl still refuses $(basename "$BIN") — expected for a CLI"
    echo "        binary until Gatekeeper's online check sees the ticket"
fi
