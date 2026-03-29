# Aura v2 Design Spec: "The Companion That Notices"

**Date:** 2026-03-29
**Status:** Draft
**Goal:** Ground-up rewrite of Aura to cross from "impressive demo" to "daily driver" — a Mac companion that notices things, remembers preferences, and shuts up when it should.

## Design Principles

1. **Proactive, not reactive.** Aura watches the screen and speaks when genuinely useful — not just when asked.
2. **Memory changes behavior.** Aura doesn't just remember facts — it acts differently based on what it knows about you.
3. **User controls autonomy.** Ghost/Copilot/Autopilot — the user decides how much Aura should do on its own.
4. **Fewer things that always work.** Reliability over feature count. Every interaction must feel solid.
5. **Direct connection, no relay.** Minimum latency. One WebSocket to Gemini, no gateway.

## Architecture Overview

```
+-----------------------------------------------------------+
|  AuraApp (Swift)                                          |
|  +----------+ +--------------+ +------------------------+ |
|  | SwiftUI  | | AVAudioEngine| | ScreenCaptureKit       | |
|  | Menu bar | | VoiceProcess | | Window-specific capture | |
|  | Chat UI  | | 16kHz capture| | Fast path: 1s interval  | |
|  | Autonomy | | 24kHz play   | | Smart path: 15s / event | |
|  | Controls | | Native echo  | | Screen basis ID per     | |
|  | Settings | | cancellation | | frame, image diffing    | |
|  +----+-----+ +------+-------+ +------------+-----------+ |
|       |              |                       |             |
|       +--------------+-----------------------+             |
|                      | Unix Socket (JSONL)                 |
+----------------------+-------------------------------------+
                       |
+----------------------+-------------------------------------+
|  aura-daemon (Rust)                                       |
|  +-----------------------------------------------------+  |
|  | Orchestrator                                         |  |
|  | +-------------+ +------------+ +------------------+  |  |
|  | | Context     | | Proactive  | | Autonomy Gate    |  |  |
|  | | Assembly    | | Scorer     | | Ghost/Copilot/   |  |  |
|  | | (pure fn)   | | (single fn)| | Autopilot        |  |  |
|  | | profile +   | | threshold  | | Standing instr   |  |  |
|  | | recent +    | | + backoff  | | fed to scorer    |  |  |
|  | | screen ctx  | |            | |                  |  |  |
|  | +-------------+ +------------+ +------------------+  |  |
|  +-----------------------------------------------------+  |
|  +-------------+ +------------+ +------------------+       |
|  | Navigator   | | Tools      | | Memory Engine    |       |
|  | Planner     | | 20+ native | | SQLite WAL       |       |
|  | confidence  | | sandbox +  | | Append-only facts|       |
|  | screen basis| | timeouts   | | Derived profile  |       |
|  | adaptive    | |            | | FTS5 search      |       |
|  | verify      | |            | |                  |       |
|  +-------------+ +------------+ +------------------+       |
|                      |                                     |
|              Direct WebSocket (TLS)                        |
|              Gemini 3.1 Flash Live API                     |
|              Native audio-to-audio + barge-in              |
+------------------------------------------------------------+
```

### Language Boundary (The Seam)

**Swift (AuraApp)** owns everything that touches Apple platform APIs:
- AVAudioEngine: mic capture (16kHz), playback (24kHz), voice processing (echo cancellation)
- ScreenCaptureKit: window-specific and display capture, Retina-aware
- Accessibility: AXUIElement tree walking, element interaction
- SwiftUI: menu bar app, chat panel, onboarding, settings
- Keychain: API key and device token storage
- IPC client: Unix socket connection to daemon

**Rust (aura-daemon + crates)** owns everything computational:
- Orchestrator: session lifecycle, event loop, IPC server
- Gemini WebSocket: direct connection, protocol, setup, streaming
- Context assembly: pure functions that build the context window
- Proactive scorer: single function, threshold management, backoff
- Navigator planner: action planning, verification, confidence
- Tool execution: 20+ handlers with sandbox and timeouts
- Memory engine: SQLite, fact extraction, profile derivation
- Input injection: CGEvent mouse/keyboard (via Rust FFI to CoreGraphics)
- Bridge: AppleScript/JXA execution with safety gates

**IPC protocol:** Unix domain socket, JSONL, bidirectional. Same proven pattern as v1.

### Crate Structure (8 crates)

| Crate | Responsibility |
|---|---|
| `aura-daemon` | Orchestrator, session lifecycle, IPC server, proactive scorer, context assembly, autonomy gate |
| `aura-gemini` | Direct WebSocket to Gemini 3.1 Flash Live, protocol types, setup message, session resumption |
| `aura-navigator` | Action planning, step execution, confidence scoring, screen basis ID verification, adaptive post-action polling |
| `aura-tools` | 20+ tool handlers, sandbox enforcement, timeouts, result formatting |
| `aura-input` | CGEvent mouse (PID-targeted + HID fallback) and keyboard injection |
| `aura-bridge` | AppleScript/JXA execution, safety gates, obfuscation detection, blocked app list |
| `aura-memory` | SQLite WAL, append-only facts table, FTS5 search, profile derivation, session tracking |
| `aura-cloud` | Optional: Firestore sync, memory agent client, proxy relay client |

Proactive scoring and context assembly live in `aura-daemon` as modules — they're functions, not independent subsystems.

---

## 1. Audio Pipeline (Swift — Build and Prove First)

This is the foundation. If the audio doesn't feel natural, nothing else matters.

### Capture

```swift
let engine = AVAudioEngine()
let inputNode = engine.inputNode
try inputNode.setVoiceProcessingEnabled(true)  // Native echo cancellation

let hwFormat = inputNode.outputFormat(forBus: 0)
inputNode.installTap(onBus: 0, bufferSize: 2048, format: hwFormat) { buffer, time in
    // Voice processing outputs 3 channels; channel 0 = processed voice
    let processedSamples = extractChannel0(buffer)
    let resampled = resampleTo16kHz(processedSamples, from: hwFormat.sampleRate)
    sendToGemini(resampled)  // via IPC → daemon → WebSocket
}
```

- `setVoiceProcessingEnabled(true)` provides: echo cancellation, noise suppression, automatic gain control
- No custom energy gating, no ambient calibration, no reverb guard — the model handles barge-in natively
- Tap health check every 5s: if tap fire count hasn't increased, restart the tap

### Playback

```swift
let playerNode = AVAudioPlayerNode()
engine.attach(playerNode)
engine.connect(playerNode, to: engine.mainMixerNode, format: playbackFormat)
// 24kHz, 16-bit mono PCM from Gemini

// Coalesce incoming chunks — buffer >= 960 bytes (~20ms) before scheduling
// Barge-in: on GeminiEvent::Interrupted → playerNode.stop() + clear buffer
```

### Device Switching

- Listen for `AVAudioEngineConfigurationChange` notification
- On change: stop tap, reconfigure engine with new device format, reinstall tap
- Handles AirPods connect/disconnect, external mic plug/unplug seamlessly

### Validation Requirement

Before writing any Rust code for v2, build a standalone Swift audio test harness:
1. Capture mic → resample to 16kHz → send to Gemini Live API → receive audio → play back
2. Test on: built-in MacBook mic, AirPods, external USB mic, studio monitors
3. Verify echo cancellation works with speakers playing Gemini's response at normal volume
4. Measure end-to-end latency (target: < 300ms mouth-to-ear)

---

## 2. Screen Capture (Swift — Dual Path)

### Fast Path (real-time video for Gemini)

```
Every 1 second (during activity, slower when idle):
  ScreenCaptureKit → capture frontmost window or full display
  → JPEG encode (1920px max, Q80)
  → Image differ: skip if pixel delta below threshold
  → Assign screen_basis_id (UUID)
  → Send as video frame to Gemini via WebSocket
```

Uses `SCScreenshotManager` with `SCContentFilter` for window-specific capture. Excludes AuraApp from captures. Retina-aware: captures at 2x, downscales for Gemini.

### Smart Path (deep analysis for proactive engine)

```
Every 15 seconds, OR on app switch, OR on forced capture:
  Full screenshot + accessibility tree snapshot
  → Combined into ScreenContext:
     { screenshot_base64, app_name, window_title,
       ui_elements[], focused_element, screen_basis_id }
  → Fed to proactive scorer for intervention evaluation
  → Available to context assembly for Gemini system prompt
```

### Screen Basis ID Tracking

Every captured frame gets a UUID `screen_basis_id`. When the Navigator plans a coordinate-based action, it records which `screen_basis_id` the coordinates were calculated against. Before executing, the Navigator verifies the current screen's basis ID matches. If the screen changed, the action is rejected with `ScreenChanged` error and the plan is re-evaluated.

---

## 3. Proactive Awareness Engine

### Single Scoring Function

```rust
fn score_intervention(
    screen_delta: &ScreenDelta,     // What changed on screen
    app_context: &AppContext,       // Current app, window, focused element
    idle_duration: Duration,         // How long user has been idle
    mode: AutonomyMode,             // Ghost / Copilot / Autopilot
    standing_instructions: &[Instruction],  // Active standing rules
    dismissal_history: &BackoffState,       // Recent dismissals
) -> Option<Intervention> {
    // Compute raw score: confidence * utility
    let score = evaluate_triggers(screen_delta, app_context, idle_duration);

    // Standing instructions boost relevant interventions
    let boosted = apply_standing_instructions(score, standing_instructions, app_context);

    // Mode-specific threshold
    let threshold = match mode {
        AutonomyMode::Ghost => return None,  // Never intervene
        AutonomyMode::Copilot => 0.85,
        AutonomyMode::Autopilot => 0.60,
    };

    // Dismissal backoff: 2x per dismiss, reset after 30 min
    let adjusted_threshold = dismissal_history.apply_backoff(threshold);

    if boosted.confidence * boosted.utility > adjusted_threshold {
        Some(boosted.into_intervention())
    } else {
        None
    }
}
```

### Triggers (evaluated from screen delta + app context)

| Trigger | Detection | Example |
|---|---|---|
| App switch | ScreenCaptureKit content change, new app PID | "I see you switched to Xcode — want to pick up where you left off?" |
| Error appeared | Semantic diff via Gemini (smart path) | "That build failed — want me to read the error?" |
| User idle | No screen changes > 60s | "Stuck? I can see the code on screen." |
| Build/deploy completed | Terminal output pattern matching | "Deploy finished. Want me to check the dashboard?" |
| Calendar event | EventKit (if permission granted) | "You have a meeting in 5 minutes." |

### Anti-Annoyance Mechanisms

- **Rate limit**: max 1 unprompted intervention per 5 min (Copilot), 2 min (Autopilot)
- **Dismissal backoff**: threshold doubles on each dismissal, resets after 30 min of no dismissals
- **"Go quiet" command**: voice command sets Ghost mode for a configurable duration (default 30 min)
- **Client-side gate**: even when Gemini's `ProactiveAudio` flag is enabled, the client suppresses speech if the scorer says no — the scorer is the final authority

### Gemini Integration

The proactive engine uses Gemini's native `Proactivity.ProactiveAudio: true` in the setup config. Gemini may independently decide to speak based on video frames. The client-side scorer acts as a gate: if the scorer's threshold isn't met, the proactive audio is suppressed (playback skipped, but the event is logged for threshold tuning).

For smart-path analysis: every 15s, the combined ScreenContext is sent as part of the live session context (not a separate API call). The proactive scorer evaluates Gemini's response — if the model generates proactive audio, the scorer decides whether to play it. This keeps the architecture simple: one Gemini session, one decision point.

---

## 4. Memory Engine

### Storage: Append-Only Facts (SQLite)

```sql
CREATE TABLE facts (
    id INTEGER PRIMARY KEY,
    timestamp TEXT NOT NULL,          -- ISO 8601
    session_id TEXT NOT NULL,
    category TEXT NOT NULL,           -- preference, correction, habit, workflow, entity
    content TEXT NOT NULL,            -- Natural language description
    importance REAL NOT NULL DEFAULT 0.5,  -- 0.0 to 1.0
    source TEXT NOT NULL DEFAULT 'transcript'  -- transcript, explicit, inferred
);

CREATE VIRTUAL TABLE facts_fts USING fts5(content, content=facts, content_rowid=id);
```

Rules:
- **Never UPDATE.** Never DELETE. Facts are immutable.
- Contradictions resolved by recency during profile derivation — newer facts with higher importance win.
- `source = 'explicit'` for facts the user directly stated ("Remember that I prefer dark mode")
- `source = 'inferred'` for patterns extracted from behavior

### Fact Extraction (After Each Session)

After session ends, the transcript is sent to Gemini for structured fact extraction:

```
Input: session transcript (user turns + tool calls)
Output: [
  { category: "preference", content: "Prefers terse responses, no narration during automation", importance: 0.8 },
  { category: "workflow", content: "Deploy workflow: git push origin main then open GCP console", importance: 0.9 },
  { category: "correction", content: "Don't use Safari — always use Chrome", importance: 0.7 }
]
```

Facts are appended to the SQLite table. Importance is model-assigned but bounded [0.0, 1.0].

### Profile Derivation (Two-Tier Injection)

At session start, derive a compressed profile from all accumulated facts:

**Tier 1 — Stable Profile (400 tokens):**
- Long-term behavioral preferences (recency-weighted, importance-sorted)
- Active projects/workflows
- Personality preferences (terse/verbose, humor level, explanation depth)
- Personalized shortcuts and aliases

**Tier 2 — Recent Corrections (200 tokens):**
- Last 20 facts with category = 'correction' or 'preference', uncompressed
- Newest first — ensures recent feedback isn't squeezed out

**Injection format:**
```xml
<user_profile>
[Tier 1: Stable profile — 400 tokens max]
Abdullah prefers terse responses. No narration during multi-step automation.
Primarily works in VS Code on Rust projects. Deploy = git push origin main.
Uses Chrome, never Safari. Dark mode everywhere. Hates being asked "shall I?"
</user_profile>

<recent_context>
[Tier 2: Last 20 corrections/preferences — 200 tokens max]
- [Mar 28] Don't summarize what you just did — I can see the screen
- [Mar 27] When switching apps, don't announce it
- [Mar 26] Terminal commands should use zsh syntax, not bash
</recent_context>
```

Total injection: ~600 tokens. Cheap in Gemini's context window.

### Cloud Sync (Optional)

Facts sync to Firestore for cross-device persistence. Same architecture as v1: deterministic document IDs, anonymous Firebase auth, pending sync queue for offline resilience.

---

## 5. Autonomy Slider

### Three Modes

| Aspect | Ghost | Copilot (default) | Autopilot |
|---|---|---|---|
| Listen to voice | Yes | Yes | Yes |
| Observe screen | Yes | Yes | Yes |
| Speak unprompted | Never | High confidence (0.85) | Yes (0.60) |
| Execute safe tools | On explicit request | On explicit request | Automatically |
| Execute destructive tools | On explicit request + confirm | On explicit request + confirm | Confirm always |
| Standing instructions | Inactive | Inactive | Active |
| Proactive rate limit | N/A | 1 per 5 min | 1 per 2 min |

### Standing Instructions

Persistent natural language rules for Autopilot mode. Stored in SQLite:

```sql
CREATE TABLE standing_instructions (
    id INTEGER PRIMARY KEY,
    instruction TEXT NOT NULL,       -- "When a build fails, show me the error"
    created_at TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1
);
```

Standing instructions are injected into the proactive scorer's context. They serve as Gemini context for evaluating triggers — the scorer feeds them alongside screen context to a Gemini evaluation call, which decides if any instruction matches the current situation.

Voice commands for mode switching:
- "Aura, go quiet" → Ghost mode (duration configurable, default 30 min)
- "Aura, help me with this" → Copilot mode
- "Aura, take the wheel" → Autopilot mode
- "Aura, from now on [instruction]" → Add standing instruction

### Approval Gate

```rust
fn requires_approval(
    action: &ToolAction,
    mode: AutonomyMode,
    standing_instructions: &[Instruction],
) -> ApprovalResult {
    // Destructive actions ALWAYS require confirmation regardless of mode
    if action.is_destructive() {
        return ApprovalResult::Confirm;
    }

    match mode {
        AutonomyMode::Ghost => ApprovalResult::RequireExplicitRequest,
        AutonomyMode::Copilot => ApprovalResult::RequireExplicitRequest,
        AutonomyMode::Autopilot => {
            if matches_standing_instruction(action, standing_instructions) {
                ApprovalResult::Allow
            } else {
                ApprovalResult::Suggest  // "Want me to...?"
            }
        }
    }
}
```

---

## 6. Navigator Planner

### Action Planning

When Gemini calls a tool, the Navigator builds a multi-step plan:

```rust
struct NavigatorPlan {
    steps: Vec<NavigatorStep>,
    screen_basis_id: String,     // Screenshot the plan was built against
    confidence: f32,              // Overall plan confidence
}

struct NavigatorStep {
    action: ActionType,           // AxPress, PasteText, Hotkey, ClickCoordinates, Wait
    target: TargetDescriptor,     // role, label, window_title, app_name, coordinates
    verify: VerifyContract,       // Expected post-action state
    fallback: Option<ActionType>, // Alternative if primary fails
    screen_basis_id: String,      // Must match current screen to execute
}
```

### Action Types (Priority Order)

1. **AxPress** — Accessibility API press. Deterministic, ~5ms. Primary path.
2. **PasteText** — NSPasteboard snapshot → set text → Cmd+V → restore. Fast, Unicode-safe.
3. **Hotkey** — CGEvent keyboard shortcut. For known app-specific shortcuts.
4. **ClickCoordinates** — CGEvent click at (x,y). Fallback when AX targeting fails.
5. **Wait** — Pause for content to load. Adaptive: poll for stability, not fixed sleep.

### Execution Flow

```
Gemini calls tool(action, target)
  |
  v
Navigator builds plan (1-N steps)
  |
  v
For each step:
  1. Verify screen_basis_id matches current screen
     - Mismatch -> reject, request fresh screenshot, re-plan
  2. If ClickCoordinates and confidence < 0.7:
     - Escalate to Gemini vision (fresh high-res screenshot + target description)
     - Get refined coordinates
  3. Animate cursor to target position (smooth, ~100ms)
  4. Execute action (AxPress -> PasteText -> Hotkey -> ClickCoordinates fallback chain)
  5. Adaptive verification:
     - Poll screen hash every 100ms
     - Stable = hash unchanged over 2 consecutive checks
     - Timeout after 3 seconds
     - Compare against verify_contract
  6. If verification fails:
     - Try fallback action if available
     - Otherwise report failure to Gemini with post-action screenshot
  |
  v
Return structured result to Gemini:
  { status, observed_outcome, screenshot_after }
```

### Surface-Specific Preparation

Before certain actions, the Navigator sends app-specific prep keystrokes:
- Chrome: dismiss address bar if focused (Escape)
- Terminal: ensure focus on command line
- Xcode: dismiss autocomplete popup

---

## 7. Gemini Integration

### Setup Message

```rust
SetupMessage {
    model: "models/gemini-3.1-flash-live-preview",
    generation_config: GenerationConfig {
        response_modalities: ["AUDIO"],
        speech_config: SpeechConfig { voice: "Kore" },
        media_resolution: "MEDIA_RESOLUTION_MEDIUM",
    },
    system_instruction: system_prompt + user_profile + recent_context,
    tools: build_tool_declarations(),  // 20+ function declarations
    realtime_input_config: RealtimeInputConfig {
        automatic_activity_detection: ActivityDetection {
            start_sensitivity: "START_SENSITIVITY_HIGH",
            end_sensitivity: "END_SENSITIVITY_LOW",
            prefix_padding_ms: 20,
            silence_duration_ms: 200,
            activity_handling: "START_OF_ACTIVITY_INTERRUPTS",
        },
    },
    proactivity: Proactivity {
        proactive_audio: true,     // Enable Gemini-initiated speech
    },
    enable_affective_dialog: true,  // Emotional awareness in voice
    output_audio_transcription: true,
    input_audio_transcription: true,
    session_resumption: SessionResumptionConfig {
        handle: previous_handle,   // For seamless reconnection
    },
    context_window_compression: ContextWindowCompression {
        trigger_tokens: 500_000,
        sliding_window_target: 250_000,
    },
}
```

### Session Resilience

- **Session resumption**: server-provided handles for seamless reconnection
- **Exponential backoff**: 200ms -> 400ms -> ... -> 30s with +/- 25% jitter, max 5 attempts
- **Poison counter**: 2 consecutive short-lived sessions (<30s) -> clear stale handle, fresh start
- **Plan persistence**: active Navigator plan saved to SQLite on disconnect, resumed on reconnect
- **Audio drain**: stale audio/video frames drained before reconnect to prevent protocol violations

---

## 8. Tool Definitions

20+ native macOS tools, organized by category:

| Category | Tools |
|---|---|
| Mouse | `click`, `click_element`, `context_menu_click`, `move_mouse`, `drag`, `scroll` |
| Keyboard | `type_text`, `press_key`, `key_state` (hold/release), `select_text` |
| Apps | `activate_app`, `click_menu_item` |
| Text | `write_clipboard`, `paste_text` |
| Scripts | `run_applescript`, `run_shell_command` |
| Memory | `save_memory`, `recall_memory` |
| Context | `get_screen_context` |
| System | `shutdown_aura` |

### Safety Gates (Carried from v1, enhanced)

1. Terminal apps blocked from activation (Terminal, iTerm, Warp, etc.)
2. `do shell script` blocked in AppleScript (not filtered, blocked entirely)
3. Shell metacharacters rejected in `run_shell_command` (|, ;, backtick, $(), >, <, &&, ||, sudo)
4. AppleScript obfuscation detection (concatenation tricks)
5. JXA blocked unconditionally
6. Input clamping: scroll +/-1000, text 10K chars max
7. Script timeout: 60s, output cap: 10KB
8. **New: tool execution timeout with kill** — 30s default, configurable per tool
9. **New: destructive action confirmation** — requires user voice/tap confirmation regardless of mode

---

## 9. IPC Protocol (Swift <-> Rust)

Unix domain socket at `~/Library/Application Support/aura/daemon.sock`. JSONL format.

### Daemon -> UI

| Event | Fields |
|---|---|
| `DotColor` | color (green/amber/red/gray), pulsing |
| `Transcript` | role (user/assistant), text, done, source |
| `ToolStatus` | tool_name, status (running/done/failed), output |
| `Status` | message |
| `Shutdown` | — |
| `ModeChanged` | mode (ghost/copilot/autopilot) |
| `ProactiveIntervention` | text, intervention_id (for dismiss tracking) |

### UI -> Daemon

| Command | Fields |
|---|---|
| `SendText` | text |
| `ToggleMic` | — |
| `Reconnect` | — |
| `Shutdown` | — |
| `SetMode` | mode |
| `DismissIntervention` | intervention_id |
| `AddStandingInstruction` | instruction text |
| `AudioFrame` | pcm_base64 (mic capture from Swift, 16kHz mono) |
| `VideoFrame` | jpeg_base64, screen_basis_id (screen capture from Swift). Note: binary frames are base64-encoded within JSONL for protocol simplicity. If profiling shows encoding overhead > 5%, switch to a length-prefixed binary frame interleaved with JSONL text frames. |

---

## 10. SwiftUI App (AuraApp)

### Structure

| File | Purpose |
|---|---|
| `AuraApp.swift` | App entry point, daemon lifecycle |
| `AppState.swift` | Observable state: connection, mode, mic, dot color |
| `DaemonConnection.swift` | Unix socket client, JSONL codec |
| `AudioEngine.swift` | **New**: AVAudioEngine capture + playback + voice processing |
| `ScreenCapture.swift` | **New**: ScreenCaptureKit dual-path + basis IDs + diffing |
| `ContentView.swift` | Root navigation |
| `ConversationView.swift` | Chat history with message bubbles |
| `AutonomyView.swift` | **New**: Mode picker + standing instructions management |
| `StatusHeader.swift` | Connection dot + mode indicator |
| `WelcomeView.swift` | Onboarding: API key, permissions |
| `PermissionChecker.swift` | TCC polling for mic, screen, accessibility |
| `KeychainHelper.swift` | Secure storage for API key + device token |
| `Protocol.swift` | DaemonEvent enum, mirrors Rust IPC types |

### Key Changes from v1

- Audio capture and playback move from Rust (aura-voice) to Swift (AudioEngine.swift)
- Screen capture moves from Rust (aura-screen/capture.rs) to Swift (ScreenCapture.swift)
- Accessibility tree reading stays in Rust (aura-tools needs it for click_element)
- New AutonomyView for mode switching + standing instruction management
- IPC protocol extended with AudioFrame, VideoFrame, mode commands

---

## What's Cut from v1

| Removed | Reason |
|---|---|
| Custom barge-in (energy gating, ambient calibration, reverb guard) | Gemini 3.1 Flash Live + AVAudioEngine voice processing handles natively |
| Vision Oracle (Gemini 3 Flash REST for click refinement) | Replaced by Navigator confidence scoring + Gemini vision escalation in the live session |
| aura-voice crate (Rust audio) | Replaced by Swift AVAudioEngine with voice processing |
| aura-screen/capture.rs (Rust screen capture) | Replaced by Swift ScreenCaptureKit |
| aura-menubar crate (Rust Cocoa menu bar) | Replaced by SwiftUI menu bar (already exists in AuraApp) |
| Proactive session rotation (10 min) | Test if 3.1 Flash Live handles long sessions; remove if unnecessary |
| rodio playback | Replaced by AVAudioPlayerNode in Swift |

---

## Success Criteria

1. **Audio round-trip < 300ms** (mouth-to-ear) across MacBook mic, AirPods, and external USB mic
2. **Zero false barge-ins** during normal conversation with speakers at comfortable volume
3. **Proactive intervention accepted rate > 60%** (user acts on the suggestion, doesn't dismiss)
4. **Session survives > 1 hour** without connection drops requiring user intervention
5. **Memory-influenced behavior visible** within first 3 sessions (model references learned preferences)
6. **Mode switching < 500ms** from voice command to behavioral change
