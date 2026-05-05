#!/usr/bin/env bash
# Ad-hoc codesign with Hardened Runtime + entitlements for the dev build.
#
# Why: Hardened Runtime is required for CGEvent.post (synthetic input) under
# Apple's modern signing rules. Without HR + correct entitlements, crash dumps
# don't land in ~/Library/Logs/DiagnosticReports/ on macOS 15+ (newer macOS
# uses .ips JSON format and gates on HR + signed identity, even ad-hoc).
#
# Ad-hoc signing only (--sign -). User does NOT have an Apple Developer
# Program subscription; ad-hoc is the only signing path. Notarization is
# out of scope.
#
# Run after every `swift build`.

set -euo pipefail

cd "$(dirname "$0")/.."

CONFIG="${1:-debug}"
BIN=".build/${CONFIG}/Aura"
ENT="Resources/Aura.entitlements"

if [[ ! -f "$ENT" ]]; then
    echo "error: entitlements not found at $ENT" >&2
    exit 1
fi
if [[ ! -f "$BIN" ]]; then
    echo "error: binary not found at $BIN — run \`swift build\` first" >&2
    exit 1
fi

codesign --force --options runtime \
         --entitlements "$ENT" \
         --sign - \
         "$BIN"
echo "signed (ad-hoc, runtime, entitled): $BIN"
