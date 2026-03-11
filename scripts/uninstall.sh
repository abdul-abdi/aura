#!/usr/bin/env bash
# uninstall.sh — Completely remove Aura and all its data
set -euo pipefail

APP_PATH="/Applications/Aura.app"
DATA_DIR="$HOME/Library/Application Support/aura"
CONFIG_DIR="$HOME/.config/aura"

echo ""
echo "  ╔══════════════════════════════════════╗"
echo "  ║         Aura Uninstaller             ║"
echo "  ╚══════════════════════════════════════╝"
echo ""
echo "  This will permanently remove:"
echo ""
echo "    • $APP_PATH"
echo "    • $DATA_DIR"
echo "      (SQLite database, logs, wake word models)"
echo "    • $CONFIG_DIR"
echo "      (config.toml, API key)"
echo "    • All macOS TCC permissions (Mic, Screen, Accessibility, Automation)"
echo ""

# Confirm
read -rp "  Continue? [y/N] " confirm
if [[ "${confirm:-N}" != [yY] ]]; then
    echo ""
    echo "  Cancelled. Aura is still installed."
    echo ""
    exit 0
fi

echo ""

# Kill running instances
if pgrep -x "aura-daemon" > /dev/null 2>&1 || pgrep -f "Aura.app" > /dev/null 2>&1; then
    echo "  ==> Stopping Aura processes..."
    pkill -x "aura-daemon" 2>/dev/null || true
    pkill -f "Aura.app/Contents/MacOS" 2>/dev/null || true
    sleep 1
    echo "      Done."
fi

# Reset macOS TCC permissions so a reinstall requires fresh grants.
# Both bundle IDs must be reset — the SwiftUI shell (com.aura.desktop) and
# the Rust daemon (com.aura.daemon) each have independent TCC entries.
echo "  ==> Resetting macOS permissions..."
tccutil reset All com.aura.desktop 2>/dev/null || true
tccutil reset All com.aura.daemon  2>/dev/null || true
echo "      Done."

# Remove app bundle
if [[ -d "$APP_PATH" ]]; then
    echo "  ==> Removing $APP_PATH..."
    rm -rf "$APP_PATH"
    echo "      Done."
else
    echo "  ==> $APP_PATH not found (skipped)"
fi

# Remove data directory (SQLite DB, logs, models, socket)
if [[ -d "$DATA_DIR" ]]; then
    echo "  ==> Removing $DATA_DIR..."
    rm -rf "$DATA_DIR"
    echo "      Done."
else
    echo "  ==> $DATA_DIR not found (skipped)"
fi

# Remove config directory (API key, config.toml)
if [[ -d "$CONFIG_DIR" ]]; then
    echo "  ==> Removing $CONFIG_DIR..."
    rm -rf "$CONFIG_DIR"
    echo "      Done."
else
    echo "  ==> $CONFIG_DIR not found (skipped)"
fi

# Clear UserDefaults (onboarding state persists across reinstalls otherwise)
echo "  ==> Clearing UserDefaults..."
defaults delete com.aura.desktop 2>/dev/null || true
echo "      Done."

echo ""
echo "  ╔══════════════════════════════════════╗"
echo "  ║   Aura has been completely removed.  ║"
echo "  ╚══════════════════════════════════════╝"
echo ""
echo "  To reinstall:  bash scripts/install.sh"
echo ""
