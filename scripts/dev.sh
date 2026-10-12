#!/usr/bin/env bash
# dev.sh — Build Aura, install it to /Applications and launch it.
set -euo pipefail
cd "$(dirname "$0")/.."

bash scripts/bundle.sh

echo "==> Installing to /Applications..."
pkill -x Aura 2>/dev/null || true
pkill -x AuraApp 2>/dev/null || true      # pre-2.0 versions
pkill -x aura-daemon 2>/dev/null || true  # pre-2.0 versions
sleep 0.5
rm -rf /Applications/Aura.app
cp -R build/Aura.app /Applications/

open /Applications/Aura.app
echo "==> Aura is running. Click the dot in your menu bar."
