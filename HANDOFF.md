# Handoff: Aura v2 Voice Pipeline — Debug & Stabilize

## Goal
The v2 voice pipeline is 90% working. Aura connects to Gemini 2.5 Flash Native Audio, speaks a greeting on launch, but **mic capture → Gemini is not producing replies when the user speaks**. Fix this, then move to Phase 3.

## Branch
`v2` — 20 commits ahead of `main`. 19 files changed, +1,031/-1,618 lines.

## Current State (What Works)
- Gemini 2.5 Flash Native Audio connects via direct WebSocket (v1alpha)
- Daemon receives audio from Gemini and forwards via IPC to Swift
- **Playback works** — Aura speaks its greeting through AVAudioPlayerNode (separate playback engine)
- Transcription shows in the SwiftUI chat panel
- IPC protocol fully wired (AudioFrame, PlaybackAudio, PlaybackStart/Stop, Interrupted, backpressure)
- Session resumption, rotation, and reconnection logic intact
- Screen capture running (1920px JPEG, 2 FPS)
- All 349 Rust tests pass, clippy clean

## Current Bug: Mic Capture Not Reaching Gemini
**Symptom:** Aura greets the user, but when user speaks, no reply comes.

**Root cause investigation so far:**
1. Voice processing (`setVoiceProcessingEnabled(true)`) caused CoreAudio error -10875 (format mismatch) when capture and playback used separate engines. **Disabled voice processing** as a workaround.
2. Without voice processing, `installTap(format: 16kHz)` may not work — AVAudioEngine needs voice processing enabled to do format conversion in the tap. The tap might be silently failing or the silence gate might be blocking all frames.
3. The daemon log shows **zero AudioFrame messages** arriving, confirming the Swift→daemon mic path is broken.

**What to check next:**
1. Is `startCapture()` throwing? Check if the `catch` in AuraApp.swift:452 fires — the status message would say "Mic unavailable"
2. Is the tap firing at all? Check `tapFireCount` — if it stays at 0, the tap never installed correctly
3. Is the silence gate too aggressive? The threshold is 0.001 with 3-frame hysteresis. Try temporarily disabling it.
4. Is the tap format (16kHz) being rejected without voice processing? Try using `nil` (hardware format) and resampling in PCMConverter. **Note:** We deleted `linearResample` — need to add it back or use `AVAudioConverter`.
5. Check VibeCat's capture approach — they use `SpeechRecognizer.swift`, not raw tap capture. Look at `gh api repos/Two-Weeks-Team/vibeCat/contents/VibeCat/Sources/VibeCat/SpeechRecognizer.swift --jq '.content' | base64 -d`

**Key insight from VibeCat:** Their AudioPlayer is a separate class with its own engine (which we now match). Their mic capture may also be separate — check how they send audio to Gemini.

## Architecture (Current)

```
AuraApp (SwiftUI)
  ├── AudioEngine.swift
  │   ├── captureEngine (AVAudioEngine) — voice processing DISABLED
  │   │   └── installTap(16kHz mono) → PCMConverter → base64 → onAudioFrame callback
  │   └── playbackEngine (AVAudioEngine) — separate, clean
  │       └── AVAudioPlayerNode → Int16 PCM buffers → speakers ✓ WORKING
  ├── DaemonConnection.swift — DispatchSource event-driven Unix socket IPC
  ├── AppState.swift — event routing, audio lifecycle
  └── Protocol.swift — DaemonEvent/UICommand codecs

aura-daemon (Rust)
  ├── orchestrator.rs — UICommand::AudioFrame → session.send_audio_base64()
  ├── processor.rs — GeminiEvent::AudioResponse → DaemonEvent::PlaybackAudio → IPC
  ├── session.rs — single base64_audio_tx channel, RealtimeTextMessage for text
  ├── ipc.rs — broadcast channel (2048 capacity), JSONL over Unix socket
  └── config.rs — gemini-2.5-flash-native-audio-preview-12-2025, v1alpha API
```

## Model Configuration
- **Model:** `models/gemini-2.5-flash-native-audio-preview-12-2025` (free tier)
- **API version:** `v1alpha` (required for proactivity/affective dialog, but both currently disabled)
- **Proactivity:** `None` (disabled — `enableAffectiveDialog` field rejected by API)
- **Affective dialog:** `None` (disabled — same reason)
- **Transcription:** `AudioTranscriptionConfig {}` (enabled, working)
- **VAD:** `START_SENSITIVITY_HIGH`, `END_SENSITIVITY_LOW`, 20ms prefix, 200ms silence, `START_OF_ACTIVITY_INTERRUPTS`
- **Text input:** `ClientContentMessage` (2.5 protocol, NOT `RealtimeTextMessage` which is 3.1-only)
- **API key:** In `~/.config/aura/config.toml` (no proxy, no cloud services)

## Files Changed in v2

### Swift (AuraApp/Sources/)
| File | What |
|------|------|
| `AudioEngine.swift` | **THE BUG IS HERE** — Two separate AVAudioEngine instances. Playback works. Capture may not. |
| `PCMConverter.swift` | Float32→PCM16 conversion, RMS energy, base64 encode. `linearResample` was deleted. |
| `Protocol.swift` | 6 new DaemonEvent cases, 5 new UICommand cases (incl PausePlayback/ResumePlayback) |
| `DaemonConnection.swift` | DispatchSource.makeReadSource replaces 10ms poll loop |
| `AppState.swift` | Audio event routing, NSLog debug lines for playback events |
| `AuraApp.swift` | Daemon log file redirect, --verbose flag, audio lifecycle |

### Rust (crates/)
| File | What |
|------|------|
| `config.rs` | Model ID, v1alpha WebSocket URL |
| `session.rs` | Single base64 audio channel, RealtimeTextMessage type, transcription config |
| `protocol.rs` (gemini) | AudioTranscriptionConfig, RealtimeTextMessage/TextInput types |
| `protocol.rs` (daemon) | PlaybackAudio, ModeChanged, ProactiveIntervention, PausePlayback/ResumePlayback |
| `orchestrator.rs` | AudioFrame → send_audio_base64, PausePlayback/ResumePlayback handlers |
| `processor.rs` | PlaybackAudio IPC forwarding, latency instrumentation, backpressure flag |
| `context.rs` | playback_paused in SharedFlags |
| `ipc.rs` | Debug log for audio event forwarding |
| `main.rs` | IPC broadcast capacity 2048, removed MenuBarApp |

## Expert Reviews
Three AI persona reviews (Karpathy, Hickey, Carmack) + code reviewer. All said SHIP after fixes.
- Roundtable on 3.1 migration: `~/Developer/roundtables/2026-03-29-aura-v2-gemini-3-1-migration.md`
- All recommended fixes implemented (8 items)

## Temporary Debug Artifacts (Remove After Fix)
- `AuraApp.swift:487-492` — daemon stdout/stderr redirected to `~/Library/Application Support/aura/daemon.log`
- `AuraApp.swift:493` — `--verbose` flag passed to daemon
- `AppState.swift:197-205` — NSLog debug lines for playback events
- `ipc.rs:163-165` — debug log for audio event forwarding

## What Needs to Happen Next
1. **Fix mic capture** — get AudioFrame messages flowing from Swift to daemon
2. **Commit the fix** — clean up debug artifacts
3. **Test voice round-trip** — speak, get reply, test barge-in
4. **Commit stable state** — "Phase 1+2 voice pipeline working"
5. **Investigate proactivity/affective dialog** — find correct model variant or field format
6. **Phase 3: Eyes & Hands** — plan at `docs/superpowers/plans/2026-03-29-aura-v2-phase3-eyes-and-hands.md`

## Build & Run
```bash
bash scripts/dev.sh          # Build + bundle + install + launch
# Or manual:
cargo build --release -p aura-daemon
cd AuraApp && swift build -c release && cd ..
bash scripts/bundle.sh       # Create Aura.app
open /Applications/Aura.app
# Daemon logs: ~/Library/Application Support/aura/daemon.log
```

## Config
```toml
# ~/.config/aura/config.toml
api_key = "YOUR_KEY"
# No proxy, no cloud services for v2
```
