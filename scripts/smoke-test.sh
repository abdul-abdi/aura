#!/usr/bin/env bash
set -euo pipefail

MODEL="qwen3.5:4b"
WHISPER_MODEL="$HOME/Library/Application Support/aura/models/ggml-base.en.bin"
OLLAMA_URL="http://localhost:11434/api/tags"

errors=()

# 1. Check if Ollama is installed
if ! which ollama > /dev/null 2>&1; then
  errors+=("Ollama is not installed. Install it from https://ollama.com")
fi

# 2. Check if Ollama is running
tags_response=$(curl -s "$OLLAMA_URL" 2>/dev/null || true)
if [ -z "$tags_response" ]; then
  errors+=("Ollama is not running. Start it with: ollama serve")
else
  # 3. Check if the model is pulled
  if ! echo "$tags_response" | grep -q "$MODEL"; then
    errors+=("Model $MODEL is not pulled. Pull it with: ollama pull $MODEL")
  fi
fi

# 4. Check if the whisper model exists
if [ ! -f "$WHISPER_MODEL" ]; then
  errors+=("Whisper model not found at: $WHISPER_MODEL")
fi

# Report errors and exit if any
if [ ${#errors[@]} -gt 0 ]; then
  echo "Smoke test failed. Missing prerequisites:"
  echo ""
  for err in "${errors[@]}"; do
    echo "  - $err"
  done
  exit 1
fi

echo "All prerequisites met. Building and launching aura-daemon..."
echo ""
cargo run --release -p aura-daemon -- --verbose
