#!/usr/bin/env bash
# bundle.sh — Build build/Aura.app (and optionally a .dmg)
#
# Usage:
#   bash scripts/bundle.sh          # build/Aura.app
#   bash scripts/bundle.sh --dmg    # also build/Aura-<version>.dmg
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(cat VERSION)
APP=build/Aura.app

echo "==> Building Aura ${VERSION} (release)..."
swift build -c release
BIN="$(swift build -c release --show-bin-path)/Aura"

echo "==> Creating ${APP}..."
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/Aura"

cat > "$APP/Contents/Info.plist" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Aura</string>
    <key>CFBundleDisplayName</key><string>Aura</string>
    <key>CFBundleIdentifier</key><string>com.aura.desktop</string>
    <key>CFBundleExecutable</key><string>Aura</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>${VERSION}</string>
    <key>CFBundleShortVersionString</key><string>${VERSION}</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>LSUIElement</key><true/>
    <key>NSMicrophoneUsageDescription</key><string>Aura listens so you can talk to it.</string>
    <key>NSAppleEventsUsageDescription</key><string>Aura controls apps for you when you ask it to.</string>
</dict>
</plist>
PLIST

echo "==> Generating icon..."
swift scripts/generate-icon.swift "$APP/Contents/Resources" > /dev/null 2>&1 \
    || echo "WARNING: icon generation failed; the app will use the default icon"

# Ad-hoc signature with a stable designated requirement, so macOS keeps the
# Microphone, Screen Recording and Accessibility permissions across rebuilds.
echo "==> Signing..."
codesign --force --sign - -r='designated => identifier "com.aura.desktop"' "$APP"

if [[ "${1:-}" == "--dmg" ]]; then
    DMG="build/Aura-${VERSION}.dmg"
    STAGE=$(mktemp -d)
    cp -R "$APP" "$STAGE/"
    ln -s /Applications "$STAGE/Applications"
    rm -f "$DMG"
    hdiutil create -volname Aura -srcfolder "$STAGE" -ov -format UDZO "$DMG" > /dev/null
    rm -rf "$STAGE"
    echo "==> Created $DMG"
fi

echo "==> Built $APP"
