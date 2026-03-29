# Handoff: Aura v2 — Voice Pipeline Working, Ready for Phase 3

## Goal
Voice pipeline is fully working. Next: Phase 3 (Eyes & Hands — tool execution, screen interaction).

## Branch
`v2` — ahead of `main`.

## Current State (All Working)
- Gemini 2.5 Flash Native Audio connects via direct WebSocket (v1alpha)
- **Full voice round-trip**: mic capture → daemon → Gemini → audio response → playback
- **Mic capture fixed**: uses hardware format tap + manual 16kHz resampling (matches VibeCat pattern)
- Voice processing (echo cancellation) enabled on capture engine
- Playback via separate AVAudioPlayerNode engine
- Barge-in / interruption working
- Transcription shows in the SwiftUI chat panel
- IPC protocol fully wired (AudioFrame, PlaybackAudio, PlaybackStart/Stop, Interrupted, backpressure)
- Session resumption, rotation, and reconnection logic intact
- Screen capture running (1920px JPEG, 2 FPS)
- All Rust tests pass, clippy clean

## What Was Fixed (Mic Capture Bug)
**Root cause:** `installTap(format: 16kHz)` silently fails without voice processing — AVAudioEngine can't do format conversion in the tap without the Voice Processing IO unit.

**Fix (mirrors VibeCat's proven approach):**
1. Re-enabled voice processing on capture engine (non-fatal if unavailable)
2. Read hardware format AFTER enabling VP (VP changes output format)
3. Install tap with hardware format (NOT 16kHz) — this is the key fix
4. Extract channel 0 (processed voice) into mono buffer
5. Resample to 16kHz via linear interpolation in PCMConverter
6. PCM16 encode → base64 → daemon → Gemini

**Files changed:**
- `AudioEngine.swift` — capture uses hwFormat tap + mono extraction + resample
- `PCMConverter.swift` — added `resample()` and updated `convertToGeminiFormat(sourceSampleRate:)`

## Model Configuration
- **Model:** `models/gemini-2.5-flash-native-audio-preview-12-2025` (free tier)
- **API version:** `v1alpha`
- **Transcription:** `AudioTranscriptionConfig {}` (enabled, working)
- **VAD:** `START_SENSITIVITY_HIGH`, `END_SENSITIVITY_LOW`, 20ms prefix, 200ms silence
- **API key:** In `~/.config/aura/config.toml`

## What Needs to Happen Next
1. **Phase 3: Eyes & Hands** — plan at `docs/superpowers/plans/2026-03-29-aura-v2-phase3-eyes-and-hands.md`
2. **Investigate proactivity/affective dialog** — find correct model variant or field format
3. **Production hardening** — see memory for 15-task plan

## Build & Run
```bash
bash scripts/dev.sh          # Build + bundle + install + launch
# Daemon logs: ~/Library/Application Support/aura/daemon.log
```

## Config
```toml
# ~/.config/aura/config.toml
api_key = "YOUR_KEY"
```
