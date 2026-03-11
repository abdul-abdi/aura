#!/usr/bin/env bash
# generate-readme-images.sh — Generate README images using Gemini Nano Banana.
#
# Usage:
#   bash scripts/generate-readme-images.sh              # uses config file API key
#   GEMINI_API_KEY=your-key bash scripts/generate-readme-images.sh
#
# Generates: assets/hero.png, assets/features.png, assets/icon.png
# Requires: Gemini API quota for image generation (free tier resets daily)
set -euo pipefail

# --- Load API key ---
if [[ -z "${GEMINI_API_KEY:-}" ]]; then
  CONFIG_FILE="${HOME}/.config/aura/config.toml"
  if [[ -f "$CONFIG_FILE" ]]; then
    GEMINI_API_KEY=$(grep 'api_key' "$CONFIG_FILE" | sed 's/.*"\(.*\)"/\1/')
  fi
fi

if [[ -z "${GEMINI_API_KEY:-}" ]]; then
  echo "Error: No API key found. Set GEMINI_API_KEY or configure ~/.config/aura/config.toml"
  exit 1
fi

MODEL="gemini-2.5-flash-image"
ENDPOINT="https://generativelanguage.googleapis.com/v1beta/models/${MODEL}:generateContent?key=${GEMINI_API_KEY}"

mkdir -p assets

generate_image() {
  local output_file="$1"
  local prompt="$2"
  local aspect_ratio="${3:-16:9}"

  echo "Generating ${output_file}..."

  local response
  response=$(curl -s -X POST "$ENDPOINT" \
    -H "Content-Type: application/json" \
    -d "{
      \"contents\": [{\"parts\": [{\"text\": \"${prompt}\"}]}],
      \"generationConfig\": {
        \"responseModalities\": [\"TEXT\", \"IMAGE\"],
        \"imageConfig\": {\"aspectRatio\": \"${aspect_ratio}\"}
      }
    }")

  # Check for errors
  if echo "$response" | python3 -c "import sys,json; d=json.load(sys.stdin); sys.exit(0 if 'error' not in d else 1)" 2>/dev/null; then
    echo "$response" | python3 -c "
import sys, json, base64
data = json.load(sys.stdin)
for part in data.get('candidates', [{}])[0].get('content', {}).get('parts', []):
    if 'inlineData' in part:
        img_data = base64.b64decode(part['inlineData']['data'])
        with open('${output_file}', 'wb') as f:
            f.write(img_data)
        print(f'  Saved ${output_file} ({len(img_data):,} bytes)')
        break
else:
    print('  Warning: No image in response, keeping existing file')
"
  else
    local err
    err=$(echo "$response" | python3 -c "import sys,json; print(json.load(sys.stdin)['error']['message'][:100])" 2>/dev/null || echo "Unknown error")
    echo "  Error: ${err}"
    echo "  Keeping existing file (if any)"
    return 1
  fi
}

# --- Generate hero banner ---
generate_image "assets/hero.png" \
  "Generate a wide cinematic hero image for a software product called Aura. Show a sleek dark macOS desktop with a glowing green orb in the menu bar. Futuristic, minimal, dark background with subtle green luminous accents. Faint sound waves from a microphone connecting to the green orb, then faint lines going to the screen suggesting AI watching and controlling the computer. Powerful, elegant, dark-mode, premium. No text overlays. Color palette: deep navy/black (#0D0F1A) with luminous green (#4DE680) accents." \
  "16:9" || true

# --- Generate features image ---
generate_image "assets/features.png" \
  "Generate a dark, minimal product concept showing three glowing green icons in a row connected by faint dotted lines: a sound wave icon (voice), a scanning eye icon (vision), and a cursor arrow icon (control). Each icon sits inside a thin green circle. Dark navy/black background (#0D0F1A) with luminous green (#4DE680) elements. Clean, futuristic, no text. Wide format." \
  "16:9" || true

echo ""
echo "Done. Run 'swift scripts/generate-icon.swift assets' to regenerate the app icon."
echo "If you hit quota limits, wait for the daily reset and try again."
