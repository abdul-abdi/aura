#!/usr/bin/env bash
# install.sh — Build Aura, install it to /Applications and launch it.
set -euo pipefail
cd "$(dirname "$0")/.."

swift build -c release
APP=/Applications/Aura.app

pkill -x Aura 2>/dev/null || true
pkill -x AuraApp 2>/dev/null || true; pkill -x aura-daemon 2>/dev/null || true  # old versions
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "$(swift build -c release --show-bin-path)/Aura" "$APP/Contents/MacOS/Aura"
cat > "$APP/Contents/Info.plist" << 'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Aura</string>
    <key>CFBundleIdentifier</key><string>com.aura.desktop</string>
    <key>CFBundleExecutable</key><string>Aura</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>LSUIElement</key><true/>
    <key>NSMicrophoneUsageDescription</key><string>Aura listens so you can talk to it.</string>
    <key>NSAppleEventsUsageDescription</key><string>Aura controls apps for you when you ask it to.</string>
</dict>
</plist>
PLIST

# Stable signature so macOS keeps Aura's permissions across rebuilds.
codesign --force --sign - -r='designated => identifier "com.aura.desktop"' "$APP"

open "$APP"
echo "Aura is running. Click the dot in your menu bar."
