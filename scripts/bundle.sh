#!/usr/bin/env bash
# bundle.sh — Create Aura.app macOS bundle from a built binary
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
APP_NAME="Aura"
BUNDLE_DIR="${PROJECT_DIR}/target/release/${APP_NAME}.app"
BINARY_NAME="aura-daemon"

echo "==> Building release binary..."
cargo build --release -p aura-daemon --manifest-path "${PROJECT_DIR}/Cargo.toml"

BINARY="${PROJECT_DIR}/target/release/${BINARY_NAME}"
if [[ ! -f "$BINARY" ]]; then
    echo "ERROR: Binary not found at ${BINARY}"
    exit 1
fi

echo "==> Creating ${APP_NAME}.app bundle..."
rm -rf "${BUNDLE_DIR}"
mkdir -p "${BUNDLE_DIR}/Contents/MacOS"
mkdir -p "${BUNDLE_DIR}/Contents/Resources"

# Copy binary
cp "${BINARY}" "${BUNDLE_DIR}/Contents/MacOS/${BINARY_NAME}"

# Create Info.plist
cat > "${BUNDLE_DIR}/Contents/Info.plist" << 'PLIST'
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

# Create a simple app icon (colored dot) using Python if available
if command -v python3 &>/dev/null; then
    python3 - "${BUNDLE_DIR}/Contents/Resources/AppIcon.iconset" << 'PYEOF'
import sys, os, subprocess
iconset = sys.argv[1]
os.makedirs(iconset, exist_ok=True)
# Generate simple green dot icons at required sizes
for size in [16, 32, 128, 256, 512]:
    for scale in [1, 2]:
        px = size * scale
        suffix = f"icon_{size}x{size}" + ("@2x" if scale == 2 else "") + ".png"
        path = os.path.join(iconset, suffix)
        # Use sips to create a simple colored image
        subprocess.run([
            "python3", "-c",
            f"""
import struct, zlib
def create_png(w, h, path):
    def chunk(ctype, data):
        c = ctype + data
        return struct.pack('>I', len(data)) + c + struct.pack('>I', zlib.crc32(c) & 0xffffffff)
    raw = b''
    for y in range(h):
        raw += b'\\x00'
        for x in range(w):
            cx, cy = x - w/2, y - h/2
            r = (cx*cx + cy*cy) ** 0.5
            radius = w * 0.4
            if r < radius:
                a = max(0, min(255, int(255 * (1 - max(0, r - radius + 2) / 2))))
                raw += struct.pack('BBBB', 77, 230, 128, a)
            else:
                raw += b'\\x00\\x00\\x00\\x00'
    sig = b'\\x89PNG\\r\\n\\x1a\\n'
    ihdr = struct.pack('>IIBBBBB', w, h, 8, 6, 0, 0, 0)
    with open(path, 'wb') as f:
        f.write(sig + chunk(b'IHDR', ihdr) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b''))
create_png({px}, {px}, '{path}')
"""
        ], check=True, capture_output=True)
PYEOF
    # Convert iconset to icns
    if [[ -d "${BUNDLE_DIR}/Contents/Resources/AppIcon.iconset" ]]; then
        iconutil -c icns "${BUNDLE_DIR}/Contents/Resources/AppIcon.iconset" \
            -o "${BUNDLE_DIR}/Contents/Resources/AppIcon.icns" 2>/dev/null || true
        rm -rf "${BUNDLE_DIR}/Contents/Resources/AppIcon.iconset"
    fi
fi

echo "==> Bundle created at: ${BUNDLE_DIR}"
echo ""
echo "To install:"
echo "  cp -r '${BUNDLE_DIR}' /Applications/"
echo ""
echo "To run:"
echo "  open /Applications/Aura.app"
echo ""
echo "Make sure to set your API key first:"
echo "  mkdir -p ~/.config/aura"
echo "  echo 'api_key = \"YOUR_GEMINI_API_KEY\"' > ~/.config/aura/config.toml"
