# Aura v2 Phase 1+2: Voice Round-Trip MVP

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the end-to-end voice pipeline — Swift mic capture with native echo cancellation, IPC to Rust daemon, direct WebSocket to Gemini 3.1 Flash Live, audio playback — proving the foundation before adding tools, screen capture, or intelligence features.

**Architecture:** Swift AuraApp owns audio (AVAudioEngine with voice processing) and sends PCM frames over Unix socket IPC to a Rust daemon. The daemon maintains a direct WebSocket to Gemini 3.1 Flash Live API, streaming audio bidirectionally. No gateway, no relay, no custom barge-in. The model handles interruption natively.

**Tech Stack:** Swift 6 (AVAudioEngine, SwiftUI), Rust 2024 edition (tokio, tungstenite, serde, tracing), Gemini 3.1 Flash Live API (WebSocket, PCM audio)

**Milestone:** A user speaks into their Mac, Gemini responds with voice, barge-in works naturally. End-to-end latency < 300ms. Works across built-in mic, AirPods, and external USB mic.

---

## File Structure

### Swift (AuraApp/Sources/)

| File | Responsibility |
|---|---|
| `AudioEngine.swift` | **NEW** — AVAudioEngine capture (16kHz) + playback (24kHz) + voice processing |
| `PCMConverter.swift` | **NEW** — Resample from hardware rate to 16kHz, encode to base64 |
| `Protocol.swift` | **MODIFY** — Add AudioFrame, VideoFrame, SetMode, DismissIntervention messages |
| `DaemonConnection.swift` | **MODIFY** — Add sendAudioFrame() and handle new daemon events |
| `AppState.swift` | **MODIFY** — Add audio state, mode state |
| `AuraApp.swift` | **MODIFY** — Wire AudioEngine into daemon lifecycle |
| `ContentView.swift` | **MODIFY** — Wire audio controls |

### Rust (crates/)

| File | Responsibility |
|---|---|
| `crates/aura-daemon/src/main.rs` | **REWRITE** — Simplified entry point for v2 |
| `crates/aura-daemon/src/orchestrator.rs` | **REWRITE** — v2 orchestrator: IPC + audio bridge + Gemini session |
| `crates/aura-daemon/src/ipc.rs` | **MODIFY** — Extended IPC protocol with AudioFrame/VideoFrame |
| `crates/aura-daemon/src/protocol.rs` | **MODIFY** — New event/command types |
| `crates/aura-gemini/src/config.rs` | **MODIFY** — v2 setup config (3.1 Flash Live, proactivity, VAD, affective dialog) |
| `crates/aura-gemini/src/session.rs` | **MODIFY** — v2 setup message with new fields |
| `crates/aura-gemini/src/protocol.rs` | **MODIFY** — New protocol fields (proactivity, VAD, affective dialog) |

---

## Task 1: Swift Audio Capture Engine

**Files:**
- Create: `AuraApp/Sources/AudioEngine.swift`
- Create: `AuraApp/Sources/PCMConverter.swift`

This is the most critical component. AVAudioEngine with voice processing enabled provides native echo cancellation — if this works, we delete hundreds of lines of v1 complexity.

- [ ] **Step 1: Create PCMConverter.swift**

```swift
// AuraApp/Sources/PCMConverter.swift
import AVFoundation
import Foundation

/// Converts audio buffers from hardware sample rate to 16kHz mono PCM for Gemini.
enum PCMConverter {
    /// Extract channel 0 (processed voice) from a multi-channel buffer and resample to 16kHz.
    /// Voice processing may output 3 channels; only channel 0 contains the processed voice.
    static func convertToGeminiFormat(
        buffer: AVAudioPCMBuffer,
        sourceRate: Double
    ) -> Data? {
        guard let floatData = buffer.floatChannelData else { return nil }

        let frameCount = Int(buffer.frameLength)
        guard frameCount > 0 else { return nil }

        // Extract channel 0 (processed voice)
        let channel0 = Array(UnsafeBufferPointer(start: floatData[0], count: frameCount))

        // Resample to 16kHz if needed
        let targetRate: Double = 16000
        let resampled: [Float]

        if abs(sourceRate - targetRate) < 1.0 {
            resampled = channel0
        } else {
            let ratio = targetRate / sourceRate
            let outputCount = Int(Double(frameCount) * ratio)
            resampled = linearResample(channel0, outputCount: outputCount)
        }

        // Convert Float32 [-1.0, 1.0] to 16-bit LE PCM bytes
        return floatToPCM16(resampled)
    }

    /// Linear interpolation resample (good enough for voice at 16kHz target).
    static func linearResample(_ input: [Float], outputCount: Int) -> [Float] {
        guard outputCount > 0, !input.isEmpty else { return [] }
        if outputCount == 1 { return [input[0]] }

        var output = [Float](repeating: 0, count: outputCount)
        let ratio = Double(input.count - 1) / Double(outputCount - 1)

        for i in 0..<outputCount {
            let srcIndex = Double(i) * ratio
            let lower = Int(srcIndex)
            let upper = min(lower + 1, input.count - 1)
            let frac = Float(srcIndex - Double(lower))
            output[i] = input[lower] * (1.0 - frac) + input[upper] * frac
        }

        return output
    }

    /// Convert f32 samples to 16-bit little-endian PCM Data.
    static func floatToPCM16(_ samples: [Float]) -> Data {
        var data = Data(capacity: samples.count * 2)
        for sample in samples {
            let clamped = max(-1.0, min(1.0, sample))
            let int16 = Int16(clamped * 32767.0)
            withUnsafeBytes(of: int16.littleEndian) { data.append(contentsOf: $0) }
        }
        return data
    }

    /// Encode PCM Data to base64 string for JSONL transport.
    static func encodeBase64(_ pcmData: Data) -> String {
        pcmData.base64EncodedString()
    }
}
```

- [ ] **Step 2: Create AudioEngine.swift**

```swift
// AuraApp/Sources/AudioEngine.swift
import AVFoundation
import Foundation

/// Manages audio capture and playback using AVAudioEngine with voice processing.
/// Voice processing provides native echo cancellation, noise suppression, and AGC.
@MainActor
final class AudioEngine {
    private var engine = AVAudioEngine()
    private var playerNode = AVAudioPlayerNode()
    private var isTapInstalled = false
    private var tapFireCount: UInt64 = 0
    private var healthCheckTask: Task<Void, Never>?

    /// Called with base64-encoded 16kHz PCM audio frames from the mic.
    var onAudioFrame: ((String) -> Void)?

    /// Whether the engine is currently capturing.
    private(set) var isCapturing = false

    /// Whether audio is currently playing (for UI state).
    private(set) var isPlaying = false

    // Playback format: 24kHz, 16-bit mono (Gemini's native audio output format)
    private let playbackFormat = AVAudioFormat(
        commonFormat: .pcmFormatFloat32,
        sampleRate: 24000,
        channels: 1,
        interleaved: false
    )!

    // Coalescing buffer for playback: accumulate >= 960 bytes (~20ms at 24kHz) before scheduling
    private var playbackBuffer = Data()
    private let minPlaybackBytes = 960

    deinit {
        healthCheckTask?.cancel()
    }

    // MARK: - Capture

    func startCapture() throws {
        let inputNode = engine.inputNode

        // Enable voice processing — provides echo cancellation, noise suppression, AGC
        try inputNode.setVoiceProcessingEnabled(true)

        let hwFormat = inputNode.outputFormat(forBus: 0)
        let sourceRate = hwFormat.sampleRate

        inputNode.installTap(onBus: 0, bufferSize: 2048, format: hwFormat) { [weak self] buffer, _ in
            guard let self else { return }
            self.tapFireCount += 1

            guard let pcmData = PCMConverter.convertToGeminiFormat(
                buffer: buffer,
                sourceRate: sourceRate
            ) else { return }

            let base64 = PCMConverter.encodeBase64(pcmData)

            Task { @MainActor [weak self] in
                self?.onAudioFrame?(base64)
            }
        }
        isTapInstalled = true

        // Attach player node for playback
        engine.attach(playerNode)
        engine.connect(playerNode, to: engine.mainMixerNode, format: playbackFormat)

        try engine.start()
        isCapturing = true

        // Start tap health check
        startHealthCheck(sourceRate: sourceRate)

        // Listen for audio device changes (AirPods connect, etc.)
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(handleConfigChange),
            name: .AVAudioEngineConfigurationChange,
            object: engine
        )
    }

    func stopCapture() {
        healthCheckTask?.cancel()
        healthCheckTask = nil
        NotificationCenter.default.removeObserver(self, name: .AVAudioEngineConfigurationChange, object: engine)

        if isTapInstalled {
            engine.inputNode.removeTap(onBus: 0)
            isTapInstalled = false
        }
        engine.stop()
        isCapturing = false
    }

    // MARK: - Playback

    /// Append incoming PCM audio from Gemini (base64-encoded 24kHz 16-bit LE mono).
    func appendPlaybackAudio(_ base64: String) {
        guard let data = Data(base64Encoded: base64) else { return }
        playbackBuffer.append(data)

        // Coalesce: don't schedule until we have enough for smooth playback
        guard playbackBuffer.count >= minPlaybackBytes else { return }

        let chunk = playbackBuffer
        playbackBuffer = Data()

        schedulePlayback(pcm16Data: chunk)
    }

    /// Start a new playback stream (resets buffer).
    func startPlaybackStream() {
        playbackBuffer = Data()
        if !playerNode.isPlaying {
            playerNode.play()
        }
        isPlaying = true
    }

    /// Stop playback immediately (barge-in).
    func stopPlayback() {
        playerNode.stop()
        playbackBuffer = Data()
        isPlaying = false
    }

    private func schedulePlayback(pcm16Data: Data) {
        // Convert 16-bit LE PCM to Float32 for AVAudioPlayerNode
        let sampleCount = pcm16Data.count / 2
        guard sampleCount > 0 else { return }

        guard let buffer = AVAudioPCMBuffer(
            pcmFormat: playbackFormat,
            frameCapacity: AVAudioFrameCount(sampleCount)
        ) else { return }

        buffer.frameLength = AVAudioFrameCount(sampleCount)
        let floatPtr = buffer.floatChannelData![0]

        pcm16Data.withUnsafeBytes { rawBuffer in
            let int16Ptr = rawBuffer.bindMemory(to: Int16.self)
            for i in 0..<sampleCount {
                floatPtr[i] = Float(int16Ptr[i]) / 32767.0
            }
        }

        playerNode.scheduleBuffer(buffer)
        if !playerNode.isPlaying {
            playerNode.play()
        }
    }

    // MARK: - Health Check

    private func startHealthCheck(sourceRate: Double) {
        healthCheckTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 5_000_000_000) // 5 seconds
                guard let self, self.isCapturing else { continue }

                let previousCount = self.tapFireCount
                try? await Task.sleep(nanoseconds: 1_000_000_000) // 1 more second
                if self.tapFireCount == previousCount && self.isCapturing {
                    // Tap stalled — restart
                    self.restartCapture()
                }
            }
        }
    }

    private func restartCapture() {
        stopCapture()
        try? startCapture()
    }

    // MARK: - Device Change

    @objc private func handleConfigChange(_ notification: Notification) {
        // Audio device changed (AirPods connected, external mic plugged in, etc.)
        // Restart the engine with the new device configuration
        Task { @MainActor [weak self] in
            self?.restartCapture()
        }
    }
}
```

- [ ] **Step 3: Build and verify compilation**

Run:
```bash
cd /Users/abdullahiabdi/Developer/aura && xcodebuild -project AuraApp/AuraApp.xcodeproj -scheme AuraApp -configuration Debug build 2>&1 | tail -5
```
Expected: BUILD SUCCEEDED (or equivalent — verify the Swift files compile)

Note: If AuraApp uses a Package.swift instead of xcodeproj, adjust the build command accordingly. Check with `ls AuraApp/` first.

- [ ] **Step 4: Commit**

```bash
git add AuraApp/Sources/AudioEngine.swift AuraApp/Sources/PCMConverter.swift
git commit -m "feat(v2): add Swift audio engine with native echo cancellation

AVAudioEngine with setVoiceProcessingEnabled(true) provides built-in
echo cancellation, noise suppression, and AGC. Replaces all custom
barge-in/energy-gating/reverb-guard code from v1."
```

---

## Task 2: Extend IPC Protocol for Audio Frames

**Files:**
- Modify: `AuraApp/Sources/Protocol.swift`
- Modify: `crates/aura-daemon/src/protocol.rs`

Both sides need new message types: AudioFrame (UI->Daemon), PlaybackAudio/PlaybackStart/PlaybackStop/Interrupted (Daemon->UI), plus the new autonomy-related messages.

- [ ] **Step 1: Add new events and commands to Swift Protocol.swift**

Add these types to the existing Protocol.swift file:

```swift
// Add to DaemonEvent enum — new cases:
case modeChanged(ModeChangedUpdate)
case proactiveIntervention(ProactiveInterventionUpdate)
case playbackAudio(PlaybackAudioUpdate)
case playbackStart
case playbackStop
case interrupted

// Add to EventType enum:
case modeChanged = "mode_changed"
case proactiveIntervention = "proactive_intervention"
case playbackAudio = "playback_audio"
case playbackStart = "playback_start"
case playbackStop = "playback_stop"
case interrupted

// Add decoder cases in init(from:):
case .modeChanged:
    self = .modeChanged(try ModeChangedUpdate(from: decoder))
case .proactiveIntervention:
    self = .proactiveIntervention(try ProactiveInterventionUpdate(from: decoder))
case .playbackAudio:
    self = .playbackAudio(try PlaybackAudioUpdate(from: decoder))
case .playbackStart:
    self = .playbackStart
case .playbackStop:
    self = .playbackStop
case .interrupted:
    self = .interrupted

// New structs:
struct ModeChangedUpdate: Decodable {
    let mode: String  // "ghost", "copilot", "autopilot"
}

struct ProactiveInterventionUpdate: Decodable {
    let text: String
    let interventionId: String

    private enum CodingKeys: String, CodingKey {
        case text
        case interventionId = "intervention_id"
    }
}

struct PlaybackAudioUpdate: Decodable {
    let audio: String  // base64-encoded 24kHz 16-bit PCM
}
```

Add to UICommand enum:

```swift
case audioFrame(String)          // base64 PCM
case setMode(String)             // "ghost" / "copilot" / "autopilot"
case dismissIntervention(String) // intervention_id

// Add to encode(to:):
case .audioFrame(let pcm):
    try container.encode("audio_frame", forKey: .type)
    try container.encode(pcm, forKey: .text)  // reuse text field for base64
case .setMode(let mode):
    try container.encode("set_mode", forKey: .type)
    try container.encode(mode, forKey: .text)
case .dismissIntervention(let id):
    try container.encode("dismiss_intervention", forKey: .type)
    try container.encode(id, forKey: .text)
```

- [ ] **Step 2: Add corresponding Rust protocol types**

In `crates/aura-daemon/src/protocol.rs`, add the new event and command variants. The existing file has `DaemonEvent` and `UICommand` enums — extend them:

```rust
// Add to DaemonEvent (Daemon -> UI):
PlaybackAudio { audio: String },    // base64 24kHz PCM
PlaybackStart,
PlaybackStop,
Interrupted,
ModeChanged { mode: String },
ProactiveIntervention { text: String, intervention_id: String },

// Add to UICommand (UI -> Daemon):
AudioFrame { pcm: String },         // base64 16kHz PCM
SetMode { mode: String },
DismissIntervention { intervention_id: String },
```

- [ ] **Step 3: Run Rust tests**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test -p aura-daemon -- protocol 2>&1
```
Expected: All existing protocol serialization tests pass. New types serialize correctly.

- [ ] **Step 4: Commit**

```bash
git add AuraApp/Sources/Protocol.swift crates/aura-daemon/src/protocol.rs
git commit -m "feat(v2): extend IPC protocol with audio frames and autonomy messages

Add PlaybackAudio, PlaybackStart/Stop, Interrupted events (daemon->UI).
Add AudioFrame, SetMode, DismissIntervention commands (UI->daemon).
Both Swift and Rust sides updated in sync."
```

---

## Task 3: Wire Audio Engine into DaemonConnection

**Files:**
- Modify: `AuraApp/Sources/DaemonConnection.swift`
- Modify: `AuraApp/Sources/AppState.swift`
- Modify: `AuraApp/Sources/AuraApp.swift`

Connect AudioEngine to DaemonConnection: mic frames flow UI->daemon, playback frames flow daemon->UI.

- [ ] **Step 1: Add audio frame sending to DaemonConnection**

Add to `DaemonConnection.swift`:

```swift
/// Send a mic audio frame to the daemon (base64-encoded 16kHz PCM).
func sendAudioFrame(_ base64PCM: String) {
    send(.audioFrame(base64PCM))
}

/// Send a mode change command.
func sendSetMode(_ mode: String) {
    send(.setMode(mode))
}

/// Send an intervention dismissal.
func sendDismissIntervention(_ id: String) {
    send(.dismissIntervention(id))
}
```

- [ ] **Step 2: Add audio state to AppState**

Add to `AppState.swift`:

```swift
// Add properties:
var audioEngine: AudioEngine?
var currentMode: String = "copilot"  // ghost / copilot / autopilot
var isAudioCapturing = false

// Add method to wire audio engine with daemon connection:
func setupAudioEngine(connection: DaemonConnection) {
    let engine = AudioEngine()
    engine.onAudioFrame = { [weak connection] base64 in
        connection?.sendAudioFrame(base64)
    }
    self.audioEngine = engine
}

// Add handler for new daemon events (call from existing event handler):
func handleAudioEvent(_ event: DaemonEvent) {
    switch event {
    case .playbackAudio(let update):
        audioEngine?.appendPlaybackAudio(update.audio)
    case .playbackStart:
        audioEngine?.startPlaybackStream()
    case .playbackStop:
        audioEngine?.stopPlayback()
    case .interrupted:
        audioEngine?.stopPlayback()
    case .modeChanged(let update):
        currentMode = update.mode
    default:
        break
    }
}
```

- [ ] **Step 3: Wire AudioEngine lifecycle in AuraApp.swift**

In `AppDelegate.launchDaemonAndConnect()`, after establishing the connection, start audio capture:

```swift
// After connection is established:
appState.setupAudioEngine(connection: connection!)

do {
    try appState.audioEngine?.startCapture()
    appState.isAudioCapturing = true
} catch {
    appState.statusMessage = "Mic unavailable: \(error.localizedDescription)"
}
```

In `applicationWillTerminate`, stop audio:

```swift
appState.audioEngine?.stopCapture()
```

- [ ] **Step 4: Build and verify**

```bash
cd /Users/abdullahiabdi/Developer/aura && xcodebuild -project AuraApp/AuraApp.xcodeproj -scheme AuraApp -configuration Debug build 2>&1 | tail -5
```
Expected: BUILD SUCCEEDED

- [ ] **Step 5: Commit**

```bash
git add AuraApp/Sources/DaemonConnection.swift AuraApp/Sources/AppState.swift AuraApp/Sources/AuraApp.swift
git commit -m "feat(v2): wire audio engine into daemon connection

Mic frames stream from AudioEngine through DaemonConnection to daemon.
Playback frames from daemon route to AudioEngine for speaker output.
Audio lifecycle tied to daemon connection lifecycle."
```

---

## Task 4: Update Gemini Setup for 3.1 Flash Live

**Files:**
- Modify: `crates/aura-gemini/src/config.rs`
- Modify: `crates/aura-gemini/src/protocol.rs`
- Modify: `crates/aura-gemini/src/session.rs`

Update the Gemini connection to use the full v2 setup: 3.1 Flash Live model, proactive audio, affective dialog, VAD configuration, input/output transcription.

- [ ] **Step 1: Add new protocol types for v2 setup fields**

In `crates/aura-gemini/src/protocol.rs`, add:

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeInputConfig {
    pub automatic_activity_detection: AutomaticActivityDetection,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomaticActivityDetection {
    pub start_sensitivity: String,
    pub end_sensitivity: String,
    pub prefix_padding_ms: u32,
    pub silence_duration_ms: u32,
    pub activity_handling: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Proactivity {
    pub proactive_audio: bool,
}
```

- [ ] **Step 2: Update Setup struct to include new fields**

In the existing `Setup` struct in `protocol.rs`, add:

```rust
// Add to Setup struct:
#[serde(skip_serializing_if = "Option::is_none")]
pub realtime_input_config: Option<RealtimeInputConfig>,
#[serde(skip_serializing_if = "Option::is_none")]
pub proactivity: Option<Proactivity>,
#[serde(skip_serializing_if = "Option::is_none")]
pub enable_affective_dialog: Option<bool>,
#[serde(skip_serializing_if = "Option::is_none")]
pub output_audio_transcription: Option<bool>,
#[serde(skip_serializing_if = "Option::is_none")]
pub input_audio_transcription: Option<bool>,
```

- [ ] **Step 3: Update build_setup_message in session.rs**

In `crates/aura-gemini/src/session.rs`, update `build_setup_message()` to populate the new fields:

```rust
fn build_setup_message(config: &GeminiConfig, resumption_handle: Option<String>) -> SetupMessage {
    SetupMessage {
        setup: Setup {
            model: config.model.clone(),
            generation_config: Some(GenerationConfig {
                temperature: Some(config.temperature),
                response_modalities: Some(vec!["AUDIO".into()]),
                speech_config: Some(SpeechConfig {
                    voice_config: VoiceConfig {
                        prebuilt_voice_config: PrebuiltVoiceConfig {
                            voice_name: config.voice.clone(),
                        },
                    },
                }),
                media_resolution: Some("MEDIA_RESOLUTION_MEDIUM".to_string()),
            }),
            system_instruction: Some(Content {
                role: None,
                parts: vec![Part {
                    text: Some(config.system_prompt.clone()),
                    inline_data: None,
                }],
            }),
            tools: Some(build_tool_declarations()),
            session_resumption: Some(SessionResumptionConfig {
                handle: resumption_handle,
            }),
            context_window_compression: Some(ContextWindowCompression {
                sliding_window: SlidingWindow {
                    target_tokens: Some(250_000),
                },
            }),
            realtime_input_config: Some(RealtimeInputConfig {
                automatic_activity_detection: AutomaticActivityDetection {
                    start_sensitivity: "START_SENSITIVITY_HIGH".into(),
                    end_sensitivity: "END_SENSITIVITY_LOW".into(),
                    prefix_padding_ms: 20,
                    silence_duration_ms: 200,
                    activity_handling: "START_OF_ACTIVITY_INTERRUPTS".into(),
                },
            }),
            proactivity: Some(Proactivity {
                proactive_audio: true,
            }),
            enable_affective_dialog: Some(true),
            output_audio_transcription: Some(true),
            input_audio_transcription: Some(true),
        },
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test -p aura-gemini 2>&1
```
Expected: All 80 tests pass. Setup message serialization includes new fields.

- [ ] **Step 5: Commit**

```bash
git add crates/aura-gemini/src/config.rs crates/aura-gemini/src/protocol.rs crates/aura-gemini/src/session.rs
git commit -m "feat(v2): configure Gemini 3.1 Flash Live with proactivity and VAD

Add RealtimeInputConfig (VAD sensitivity, padding, silence duration),
Proactivity (proactive_audio: true), enable_affective_dialog, and
input/output audio transcription to the setup message."
```

---

## Task 5: Rewrite Daemon Orchestrator for v2 Audio Pipeline

**Files:**
- Modify: `crates/aura-daemon/src/orchestrator.rs`

The v2 orchestrator receives AudioFrame commands from the Swift UI over IPC and forwards them directly to Gemini. Audio responses from Gemini are forwarded back as PlaybackAudio events. No energy gating, no calibration, no reverb guard — the model handles everything.

- [ ] **Step 1: Simplify the orchestrator audio bridge**

Rewrite the audio bridge in `orchestrator.rs`. The v1 bridge used `std::sync::mpsc` channels from a cpal mic capture thread. The v2 bridge receives base64 PCM from Swift via IPC and forwards to Gemini:

```rust
// In the main event loop, handle AudioFrame from IPC:
UICommand::AudioFrame { pcm } => {
    // Forward mic audio directly to Gemini — no energy gating, no VAD.
    // AVAudioEngine voice processing handles echo cancellation on the Swift side.
    // Gemini 3.1 Flash Live handles barge-in detection server-side.
    if let Err(e) = session.send_audio_base64(&pcm) {
        tracing::warn!("Failed to send audio to Gemini: {e}");
    }
}
```

And in the Gemini event handler, forward playback audio to Swift:

```rust
Ok(GeminiEvent::Audio { data }) => {
    // Forward Gemini's audio response to Swift for playback
    let _ = ipc_tx.send(DaemonEvent::PlaybackAudio { audio: data });
}
Ok(GeminiEvent::AudioStreamStart) => {
    let _ = ipc_tx.send(DaemonEvent::PlaybackStart);
}
Ok(GeminiEvent::TurnComplete) => {
    let _ = ipc_tx.send(DaemonEvent::PlaybackStop);
}
Ok(GeminiEvent::Interrupted) => {
    let _ = ipc_tx.send(DaemonEvent::Interrupted);
}
```

- [ ] **Step 2: Add send_audio_base64 to GeminiLiveSession**

In `crates/aura-gemini/src/session.rs`, add a method that accepts pre-encoded base64 audio (from Swift, already at 16kHz) and sends it without re-encoding:

```rust
/// Send pre-encoded base64 PCM audio directly to Gemini.
/// Used when audio arrives already encoded from the Swift audio engine.
pub fn send_audio_base64(&self, base64_pcm: &str) -> Result<()> {
    let msg = RealtimeAudioMessage {
        realtime_input: RealtimeAudioInput {
            audio: Blob {
                mime_type: "audio/pcm;rate=16000".into(),
                data: base64_pcm.to_string(),
            },
        },
    };
    let json = serde_json::to_string(&msg)?;
    self.send_raw(&json)
}
```

- [ ] **Step 3: Run tests**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test 2>&1 | tail -20
```
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/aura-daemon/src/orchestrator.rs crates/aura-gemini/src/session.rs
git commit -m "feat(v2): rewrite orchestrator audio bridge for Swift-native capture

Audio frames arrive from Swift (pre-encoded base64 16kHz PCM) via IPC
and forward directly to Gemini. Playback audio from Gemini routes back
to Swift via IPC. No client-side energy gating — echo cancellation
handled by AVAudioEngine, barge-in by Gemini server-side."
```

---

## Task 6: End-to-End Voice Round-Trip Smoke Test

**Files:** No new files — this is a manual integration test.

- [ ] **Step 1: Build everything**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo build -p aura-daemon 2>&1 | tail -5
```
Expected: Compilation succeeds.

```bash
cd /Users/abdullahiabdi/Developer/aura && xcodebuild -project AuraApp/AuraApp.xcodeproj -scheme AuraApp -configuration Debug build 2>&1 | tail -5
```
Expected: BUILD SUCCEEDED.

- [ ] **Step 2: Run the smoke test**

1. Ensure `GEMINI_API_KEY` is set in `~/.config/aura/config.toml`
2. Launch AuraApp from Xcode or the build output
3. Speak "Hello, Aura" into the mic
4. Verify: Gemini responds with voice through the speakers
5. While Gemini is speaking, interrupt by saying "Actually wait" — verify barge-in works (Gemini stops immediately)
6. Test with AirPods (if available): disconnect/reconnect, verify audio resumes

- [ ] **Step 3: Measure latency**

In AudioEngine.swift, add temporary timestamps:
- Record `Date()` when tap fires (mic capture)
- Record `Date()` when first playback audio arrives from daemon
- Log the difference

Target: < 300ms mouth-to-ear.

- [ ] **Step 4: Commit any fixes**

```bash
git add -A
git commit -m "fix(v2): address issues found in voice round-trip smoke test"
```

---

## Task 7: Remove Dead v1 Code

**Files:**
- Remove: `crates/aura-voice/` (entire crate — audio now in Swift)
- Remove: `crates/aura-menubar/` (entire crate — UI now in SwiftUI)
- Modify: `Cargo.toml` — Remove aura-voice and aura-menubar from workspace members
- Modify: `crates/aura-daemon/Cargo.toml` — Remove aura-voice and aura-menubar dependencies

- [ ] **Step 1: Remove aura-voice from workspace**

Remove `"crates/aura-voice"` from the `[workspace] members` array in the root `Cargo.toml`. Remove any `aura-voice` dependency from `crates/aura-daemon/Cargo.toml`.

- [ ] **Step 2: Remove aura-menubar from workspace**

Remove `"crates/aura-menubar"` from `[workspace] members`. Remove any `aura-menubar` dependency from `crates/aura-daemon/Cargo.toml`.

- [ ] **Step 3: Remove all aura-voice and aura-menubar imports from daemon code**

Search for `use aura_voice` and `use aura_menubar` in `crates/aura-daemon/src/` and remove all references. The orchestrator no longer creates `AudioCapture` or `AudioPlayer` — audio comes from Swift.

- [ ] **Step 4: Build and test**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo build 2>&1 | tail -10
cargo test 2>&1 | tail -20
```
Expected: Compiles and all remaining tests pass.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/aura-daemon/Cargo.toml crates/aura-daemon/src/
git commit -m "refactor(v2): remove aura-voice and aura-menubar crates

Audio capture and playback moved to Swift AVAudioEngine (Task 1).
Menu bar UI already handled by SwiftUI AuraApp.
Removes ~800 lines of Rust audio code (cpal, rodio, VAD, barge-in)."
```

Note: Do NOT `git rm` the actual crate directories yet — keep them around in case you need to reference patterns. Remove them in a separate cleanup commit after Phase 2 is fully validated.

---

## Phase 2 Remaining Tasks (Skeleton)

The following tasks complete Phase 2. Each follows the same TDD pattern as above. Detailed code will be written when Phase 1 tasks are validated.

### Task 8: v2 IPC Server with AudioFrame Routing
Modify `crates/aura-daemon/src/ipc.rs` to parse the new `AudioFrame`, `SetMode`, `DismissIntervention` commands from the Swift client. Route audio frames to the Gemini session. Route mode changes to the autonomy state.

### Task 9: Gemini Session Audio Response Routing
Ensure `GeminiEvent::Audio` variants are correctly parsed from the WebSocket and forwarded as `DaemonEvent::PlaybackAudio` to the IPC broadcast channel. Verify transcription events flow to the UI.

### Task 10: Session Resumption and Reconnection
Port v1's session resumption logic (resumption handles, poison counter, exponential backoff with jitter) into the v2 orchestrator. Verify seamless reconnection after network drop.

### Task 11: Text Input Support
Verify `UICommand::SendText` still works — user types in the chat panel, text is forwarded to Gemini via the live session. This is the fallback input when voice isn't appropriate.

---

## Phase 3+4 Plans

Phase 3 (Screen Capture + Navigator + Tools) and Phase 4 (Memory + Proactive + Autonomy) will be planned as separate documents after Phase 1+2 is validated with the voice round-trip smoke test.

- **Phase 3 plan:** `docs/superpowers/plans/2026-XX-XX-aura-v2-phase3-eyes-and-hands.md`
- **Phase 4 plan:** `docs/superpowers/plans/2026-XX-XX-aura-v2-phase4-intelligence.md`
