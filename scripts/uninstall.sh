#!/usr/bin/env bash
# uninstall.sh — Remove Aura, its settings, memory and macOS permissions.
set -euo pipefail

echo "This removes /Applications/Aura.app, ~/.config/aura (API key),"
echo "~/Library/Application Support/Aura (memory) and Aura's macOS permissions."
read -rp "Continue? [y/N] " confirm
[[ "${confirm:-N}" == [yY] ]] || { echo "Cancelled."; exit 0; }

pkill -x Aura 2>/dev/null || true
pkill -x AuraApp 2>/dev/null || true
pkill -x aura-daemon 2>/dev/null || true

tccutil reset All com.aura.desktop 2>/dev/null || true
tccutil reset All com.aura.daemon 2>/dev/null || true  # pre-2.0 helper

rm -rf /Applications/Aura.app "$HOME/.config/aura" "$HOME/Library/Application Support/Aura"
defaults delete com.aura.desktop 2>/dev/null || true

echo "Aura has been removed."
