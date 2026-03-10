#!/usr/bin/env bash
# bundle.sh — Build Aura.app (SwiftUI + Rust daemon) and optionally create a .dmg
#
# Usage:
#   bash scripts/bundle.sh          # Build .app only
#   bash scripts/bundle.sh --dmg    # Build .app + create .dmg installer
#   bash scripts/bundle.sh --legacy # Build old Cocoa-only .app (no SwiftUI)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
APP_NAME="Aura"
VERSION="1.0.0"
BUNDLE_DIR="${PROJECT_DIR}/target/release/${APP_NAME}.app"
DMG_PATH="${PROJECT_DIR}/target/release/${APP_NAME}-${VERSION}.dmg"

BUILD_DMG=false
LEGACY=false
for arg in "$@"; do
    case "$arg" in
        --dmg) BUILD_DMG=true ;;
        --legacy) LEGACY=true ;;
    esac
done

# ── Step 1: Build Rust daemon ──────────────────────────────────────────────

echo "==> Building Rust daemon (release)..."
cargo build --release -p aura-daemon --manifest-path "${PROJECT_DIR}/Cargo.toml"

DAEMON_BINARY="${PROJECT_DIR}/target/release/aura-daemon"
if [[ ! -f "$DAEMON_BINARY" ]]; then
    echo "ERROR: Daemon binary not found at ${DAEMON_BINARY}"
    exit 1
fi

# ── Step 2: Build SwiftUI app (unless --legacy) ───────────────────────────

if [[ "$LEGACY" == false ]]; then
    SWIFTUI_DIR="${PROJECT_DIR}/AuraApp"
    if [[ -d "$SWIFTUI_DIR" ]]; then
        echo "==> Building SwiftUI app (release)..."
        cd "$SWIFTUI_DIR"
        swift build -c release 2>&1 | tail -3
        cd "$PROJECT_DIR"

        SWIFTUI_BINARY="${SWIFTUI_DIR}/.build/release/AuraApp"
        if [[ ! -f "$SWIFTUI_BINARY" ]]; then
            echo "ERROR: SwiftUI binary not found at ${SWIFTUI_BINARY}"
            echo "Falling back to legacy build."
            LEGACY=true
        fi
    else
        echo "WARNING: AuraApp/ directory not found. Falling back to legacy build."
        LEGACY=true
    fi
fi

# ── Step 3: Create .app bundle ─────────────────────────────────────────────

echo "==> Creating ${APP_NAME}.app bundle..."
rm -rf "${BUNDLE_DIR}"
mkdir -p "${BUNDLE_DIR}/Contents/MacOS"
mkdir -p "${BUNDLE_DIR}/Contents/Resources"

if [[ "$LEGACY" == true ]]; then
    # Legacy mode: daemon is the main executable (Cocoa menu bar)
    MAIN_EXECUTABLE="aura-daemon"
    cp "${DAEMON_BINARY}" "${BUNDLE_DIR}/Contents/MacOS/aura-daemon"
else
    # New mode: SwiftUI app is main executable, daemon is a helper
    MAIN_EXECUTABLE="AuraApp"
    cp "${SWIFTUI_BINARY}" "${BUNDLE_DIR}/Contents/MacOS/AuraApp"
    cp "${DAEMON_BINARY}" "${BUNDLE_DIR}/Contents/MacOS/aura-daemon"
fi

# ── Step 4: Info.plist ─────────────────────────────────────────────────────

cat > "${BUNDLE_DIR}/Contents/Info.plist" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleDisplayName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleIdentifier</key>
    <string>com.aura.desktop</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleExecutable</key>
    <string>${MAIN_EXECUTABLE}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>LSUIElement</key>
    <true/>
    <key>LSMinimumSystemVersion</key>
    <string>14.0</string>
    <key>NSMicrophoneUsageDescription</key>
    <string>Aura needs microphone access for voice interaction with your AI assistant.</string>
    <key>NSAppleEventsUsageDescription</key>
    <string>Aura uses AppleScript to automate macOS tasks on your behalf.</string>
    <key>NSHumanReadableCopyright</key>
    <string>Copyright © 2026 Aura. All rights reserved.</string>
    <key>SUFeedURL</key>
    <string>https://github.com/user/aura/releases/latest/download/appcast.xml</string>
    <key>SUPublicEDKey</key>
    <string></string>
</dict>
</plist>
PLIST

# ── Step 5: Generate app icon ──────────────────────────────────────────────

echo "==> Generating app icon..."
swift "${SCRIPT_DIR}/generate-icon.swift" "${BUNDLE_DIR}/Contents/Resources" 2>/dev/null

if [[ ! -f "${BUNDLE_DIR}/Contents/Resources/AppIcon.icns" ]]; then
    echo "WARNING: Icon generation failed. App will use default icon."
fi

# ── Step 6: Ad-hoc code sign (local execution without Gatekeeper warnings) ─

echo "==> Code signing (ad-hoc)..."
codesign --force --deep --sign - "${BUNDLE_DIR}" 2>/dev/null || {
    echo "WARNING: Code signing failed. App may trigger Gatekeeper warnings."
}

# ── Step 7: Create .dmg installer (optional) ──────────────────────────────

if [[ "$BUILD_DMG" == true ]]; then
    echo "==> Creating DMG installer..."

    # Staging directory with app + Applications symlink
    DMG_STAGING=$(mktemp -d)
    cp -R "${BUNDLE_DIR}" "${DMG_STAGING}/"
    ln -s /Applications "${DMG_STAGING}/Applications"

    # Remove old DMG if it exists
    rm -f "${DMG_PATH}"

    # Create compressed DMG
    hdiutil create \
        -volname "${APP_NAME}" \
        -srcfolder "${DMG_STAGING}" \
        -ov \
        -format UDZO \
        -imagekey zlib-level=9 \
        "${DMG_PATH}" \
        2>/dev/null

    rm -rf "${DMG_STAGING}"

    DMG_SIZE=$(du -h "${DMG_PATH}" | cut -f1 | tr -d ' ')
    echo ""
    echo "  DMG created: ${DMG_PATH} (${DMG_SIZE})"
    echo "  Share this file — recipients drag Aura to Applications to install."
fi

# ── Done ───────────────────────────────────────────────────────────────────

APP_SIZE=$(du -sh "${BUNDLE_DIR}" | cut -f1 | tr -d ' ')
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  ${APP_NAME}.app built successfully (${APP_SIZE})"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "  Install:  cp -r '${BUNDLE_DIR}' /Applications/"
echo "  Run:      open /Applications/${APP_NAME}.app"
echo ""
if [[ "$LEGACY" == false ]]; then
    echo "  Mode:     SwiftUI app + Rust daemon"
    echo "  Hotkey:   Cmd+Shift+A to toggle floating panel"
else
    echo "  Mode:     Legacy (Cocoa menu bar only)"
fi
echo ""
echo "  First run will prompt for your Gemini API key."
echo ""
