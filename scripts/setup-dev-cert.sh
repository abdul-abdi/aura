#!/usr/bin/env bash
# setup-dev-cert.sh — Create a self-signed code signing certificate for dev builds.
#
# Run this ONCE. After that, `bundle.sh --dev-cert` will use it automatically.
# The same key produces a stable CDHash, so macOS TCC permission grants
# (Mic, Screen Recording, Accessibility) survive across rebuilds.
#
# Usage:
#   bash scripts/setup-dev-cert.sh
set -euo pipefail

CERT_NAME="Aura Dev"
KEYCHAIN="${HOME}/Library/Keychains/login.keychain-db"
TMP_DIR=$(mktemp -d)

# Check if cert already exists
if security find-identity -v -p codesigning 2>/dev/null | grep -q "$CERT_NAME"; then
    echo ""
    echo "  '$CERT_NAME' certificate already exists."
    echo "  Use: bash scripts/bundle.sh --dev-cert"
    echo ""
    exit 0
fi

echo ""
echo "  Creating self-signed code signing certificate: '$CERT_NAME'"
echo ""

# Generate self-signed certificate with code signing extensions
openssl req -x509 -newkey rsa:2048 \
    -keyout "${TMP_DIR}/key.pem" \
    -out "${TMP_DIR}/cert.pem" \
    -days 3650 -nodes \
    -subj "/CN=${CERT_NAME}" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=codeSigning" 2>/dev/null

# Export as PKCS#12
openssl pkcs12 -export \
    -out "${TMP_DIR}/cert.p12" \
    -inkey "${TMP_DIR}/key.pem" \
    -in "${TMP_DIR}/cert.pem" \
    -passout pass: 2>/dev/null

# Import into login keychain
echo "  Importing into login keychain (you may be prompted for your password)..."
security import "${TMP_DIR}/cert.p12" \
    -k "$KEYCHAIN" \
    -P "" \
    -T /usr/bin/codesign

# Allow codesign to use the key without prompting each time.
# This requires the keychain password.
echo "  Setting key partition list (may prompt for keychain password)..."
security set-key-partition-list \
    -S apple-tool:,apple:,codesign: \
    -s -k "" "$KEYCHAIN" 2>/dev/null || {
    echo ""
    echo "  NOTE: Could not set partition list automatically."
    echo "  You may be prompted by macOS the first time you sign with this cert."
    echo "  Click 'Always Allow' when prompted."
    echo ""
}

# Cleanup temp files
rm -rf "$TMP_DIR"

# Verify
if security find-identity -v -p codesigning 2>/dev/null | grep -q "$CERT_NAME"; then
    echo ""
    echo "  Done! '$CERT_NAME' certificate installed."
    echo ""
    echo "  Usage:"
    echo "    bash scripts/bundle.sh --dev-cert     # build with stable signing"
    echo "    bash scripts/dev.sh --dev-cert         # full dev pipeline"
    echo ""
else
    echo ""
    echo "  ERROR: Certificate installation failed."
    echo "  Try manually via Keychain Access > Certificate Assistant > Create a Certificate."
    echo ""
    exit 1
fi
