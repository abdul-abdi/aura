#!/usr/bin/env bash
# Ad-hoc codesign with Hardened Runtime + entitlements for the dev build.
#
# Why: Hardened Runtime is required for CGEvent.post (synthetic input) under
# Apple's modern signing rules. Without HR + correct entitlements, crash dumps
# don't land in ~/Library/Logs/DiagnosticReports/ on macOS 15+ (newer macOS
# uses .ips JSON format and gates on HR + signed identity, even ad-hoc).
#
# This is ad-hoc signing (-sign -). The user does NOT have an Apple Developer
# Program subscription; ad-hoc is the only signing path. Notarization is out
# of scope.
#
# Run after every `swift build`.

set -euo pipefail

cd "$(dirname "$0")/.."

CONFIG="${1:-debug}"
BIN_AURA=".build/${CONFIG}/AuraV3"
BIN_CLI=".build/${CONFIG}/aura"
ENT="Resources/Aura.entitlements"

if [[ ! -f "$ENT" ]]; then
    echo "error: entitlements not found at $ENT" >&2
    exit 1
fi

for bin in "$BIN_AURA" "$BIN_CLI"; do
    if [[ -f "$bin" ]]; then
        codesign --force --options runtime \
                 --entitlements "$ENT" \
                 --sign - \
                 "$bin"
        echo "signed (ad-hoc, runtime, entitled): $bin"
    else
        echo "skip (not built): $bin"
    fi
done
