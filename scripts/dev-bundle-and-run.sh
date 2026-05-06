#!/usr/bin/env bash
# Bundles the freshly-built Aura binary into a minimal Aura.app, codesigns it
# (ad-hoc + Hardened Runtime + entitlements), and launches it via `open`.
#
# Why a bundle: the bare `.build/debug/Aura` binary doesn't pick up
# Resources/Info.plist — so LSUIElement (menu-bar-only mode) and the TCC
# usage descriptions don't apply. The bundle wraps Info.plist around the
# binary so AppKit reads the right metadata.
#
# Idempotent — `open -n` launches a fresh instance even if one is running.

set -euo pipefail

cd "$(dirname "$0")/.."

CONFIG="${1:-debug}"
BIN=".build/${CONFIG}/Aura"
APP=".build/Aura.app"

if [[ ! -f "$BIN" ]]; then
    echo "error: binary not found at $BIN — run \`swift build\` first" >&2
    exit 1
fi

# Build the bundle layout
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
mkdir -p "$APP/Contents/Resources"
cp Resources/Info.plist "$APP/Contents/Info.plist"
cp "$BIN" "$APP/Contents/MacOS/Aura"
cp Resources/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns" 2>/dev/null || true

# Sign the whole bundle so AppKit + TCC see a coherent identity.
codesign --force --options runtime \
         --entitlements Resources/Aura.entitlements \
         --sign - \
         "$APP"

echo "bundled: $APP"
echo "launching..."
# -n: new instance even if one is running
# -F: fresh launch (don't restore saved state)
open -n -F "$APP"
echo "done — look for the menu-bar dot (top-right). ⌘⇧A toggles the floating panel."
echo "to quit: right-click the dot → Quit Aura, or:"
echo "  pkill -f .build/Aura.app/Contents/MacOS/Aura"
