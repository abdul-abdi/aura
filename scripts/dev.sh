#!/usr/bin/env bash
# dev.sh — Local development pipeline: build → bundle → install → relaunch
#
# Usage:
#   bash scripts/dev.sh
#
# Make executable (one-time):
#   chmod +x scripts/dev.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
APP_NAME="Aura"
BUNDLE_SRC="${PROJECT_DIR}/target/release/${APP_NAME}.app"
INSTALL_DEST="/Applications/${APP_NAME}.app"

# ── Step 1: Build & Bundle Aura.app ──────────────────────────────────────────
# bundle.sh handles both the Rust daemon build and SwiftUI app build internally.

echo "==> Building and bundling ${APP_NAME}.app..."
bash "${SCRIPT_DIR}/bundle.sh"

if [[ ! -d "$BUNDLE_SRC" ]]; then
    echo "ERROR: Bundle not found at ${BUNDLE_SRC}"
    exit 1
fi

# ── Step 3: Kill any running Aura processes ────────────────────────────────

echo "==> Stopping running Aura processes (if any)..."
pkill -x "aura-daemon" 2>/dev/null || true
pkill -x "AuraApp"     2>/dev/null || true
# Give the OS a moment to release file locks on the bundle
sleep 0.5

# ── Step 4: Install to /Applications ──────────────────────────────────────

echo "==> Installing ${APP_NAME}.app to /Applications/..."
rm -rf "$INSTALL_DEST"
cp -R "$BUNDLE_SRC" "$INSTALL_DEST"

# ── Step 5: Relaunch ───────────────────────────────────────────────────────

echo "==> Launching ${APP_NAME}.app..."
open "$INSTALL_DEST"

# ── Done ───────────────────────────────────────────────────────────────────

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  ${APP_NAME} deployed and running"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "  Installed: ${INSTALL_DEST}"
echo ""
