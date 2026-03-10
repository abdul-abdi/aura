#!/usr/bin/env bash
# install.sh — Install Aura on macOS
# Usage: curl -fsSL https://raw.githubusercontent.com/.../install.sh | bash
set -euo pipefail

APP_NAME="Aura"
REPO_URL="https://github.com/abdul-abdi/aura"
INSTALL_DIR="/Applications"

echo ""
echo "  ╔══════════════════════════════════════╗"
echo "  ║         Installing Aura v0.2         ║"
echo "  ║   Voice-first AI desktop companion   ║"
echo "  ╚══════════════════════════════════════╝"
echo ""

# --- Pre-flight checks ---

# macOS only
if [[ "$(uname)" != "Darwin" ]]; then
    echo "ERROR: Aura only runs on macOS."
    exit 1
fi

# Check macOS version (need 13+)
MACOS_VERSION=$(sw_vers -productVersion | cut -d. -f1)
if [[ "$MACOS_VERSION" -lt 13 ]]; then
    echo "ERROR: Aura requires macOS 13 (Ventura) or later."
    exit 1
fi

# --- Install Rust if needed ---
if ! command -v cargo &>/dev/null; then
    echo "==> Rust not found. Installing via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
    echo "    Rust installed."
else
    echo "==> Rust found: $(rustc --version)"
fi

# --- Clone or update repo ---
WORK_DIR="${TMPDIR:-/tmp}/aura-install-$$"
cleanup() { rm -rf "$WORK_DIR"; }
trap cleanup EXIT

if [[ -d "$1" ]] 2>/dev/null; then
    # If a local path is provided, use it
    PROJECT_DIR="$1"
    echo "==> Using local source: $PROJECT_DIR"
else
    echo "==> Cloning Aura..."
    git clone --depth 1 "$REPO_URL" "$WORK_DIR"
    PROJECT_DIR="$WORK_DIR"
fi

# --- Build ---
echo "==> Building Aura (release mode)..."
echo "    This may take a few minutes on first build."
cd "$PROJECT_DIR"
cargo build --release -p aura-daemon 2>&1 | tail -5

BINARY="target/release/aura-daemon"
if [[ ! -f "$BINARY" ]]; then
    echo "ERROR: Build failed — binary not found."
    exit 1
fi

# --- Create .app bundle ---
echo "==> Creating ${APP_NAME}.app..."
BUNDLE="${APP_NAME}.app"
rm -rf "$BUNDLE"
mkdir -p "${BUNDLE}/Contents/MacOS"
mkdir -p "${BUNDLE}/Contents/Resources"
cp "$BINARY" "${BUNDLE}/Contents/MacOS/aura-daemon"

cat > "${BUNDLE}/Contents/Info.plist" << 'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Aura</string>
    <key>CFBundleDisplayName</key>
    <string>Aura</string>
    <key>CFBundleIdentifier</key>
    <string>com.aura.desktop</string>
    <key>CFBundleVersion</key>
    <string>0.2.0</string>
    <key>CFBundleShortVersionString</key>
    <string>0.2.0</string>
    <key>CFBundleExecutable</key>
    <string>aura-daemon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSMicrophoneUsageDescription</key>
    <string>Aura needs microphone access for voice interaction with your AI assistant.</string>
    <key>NSAppleEventsUsageDescription</key>
    <string>Aura uses AppleScript to automate macOS tasks on your behalf.</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
</dict>
</plist>
PLIST

# --- Install ---
echo "==> Installing to ${INSTALL_DIR}/${APP_NAME}.app..."
if [[ -d "${INSTALL_DIR}/${APP_NAME}.app" ]]; then
    echo "    Removing previous installation..."
    rm -rf "${INSTALL_DIR}/${APP_NAME}.app"
fi
cp -r "$BUNDLE" "${INSTALL_DIR}/"

# --- API Key setup ---
CONFIG_DIR="$HOME/.config/aura"
CONFIG_FILE="${CONFIG_DIR}/config.toml"

if [[ ! -f "$CONFIG_FILE" ]]; then
    echo ""
    echo "==> Setting up API key..."
    echo "    Get your key at: https://aistudio.google.com/apikey"
    echo ""
    read -rp "    Enter your Gemini API key (or press Enter to skip): " API_KEY

    if [[ -n "$API_KEY" ]]; then
        mkdir -p "$CONFIG_DIR"
        echo "api_key = \"${API_KEY}\"" > "$CONFIG_FILE"
        chmod 600 "$CONFIG_FILE"
        echo "    API key saved to ${CONFIG_FILE}"
    else
        echo "    Skipped. Set it later:"
        echo "      mkdir -p ~/.config/aura"
        echo "      echo 'api_key = \"YOUR_KEY\"' > ~/.config/aura/config.toml"
    fi
else
    echo "==> Existing config found at ${CONFIG_FILE}"
fi

# --- Done ---
echo ""
echo "  ╔══════════════════════════════════════╗"
echo "  ║          Aura installed!             ║"
echo "  ╚══════════════════════════════════════╝"
echo ""
echo "  Launch:  open /Applications/Aura.app"
echo "  Config:  ~/.config/aura/config.toml"
echo ""
echo "  First launch will request:"
echo "    - Microphone access (for voice)"
echo "    - Accessibility (for AppleScript automation)"
echo ""
echo "  Voice commands:"
echo "    - Talk naturally — Aura is always listening"
echo "    - Say 'shutdown Aura' or 'quit Aura' to exit"
echo ""
