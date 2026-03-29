# Aura v2 Phase 3: Eyes and Hands -- Screen Capture + Navigator + Tools

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Aura vision (screen capture streamed to Gemini) and agency (20+ native macOS tools executed through a confidence-scored Navigator planner with adaptive post-action verification). After this phase, Aura can see the user's screen and take action on it.

**Architecture:** Swift `ScreenCapture.swift` owns ScreenCaptureKit (dual-path: fast 1s video frames + smart 15s deep analysis). JPEG frames flow over Unix socket IPC as `VideoFrame` messages to the Rust daemon, which forwards them to Gemini. When Gemini calls tools, the `aura-navigator` crate builds multi-step plans with screen basis ID verification, executes actions through `aura-tools` (ported from v1's `tools.rs` + `tool_helpers.rs`), and verifies results with adaptive screen hash polling.

**Tech Stack:** Swift 6 (ScreenCaptureKit, SCScreenshotManager), Rust 2024 edition (tokio, serde, uuid, image), existing crates (aura-input, aura-bridge, aura-screen/accessibility.rs), Gemini 3.1 Flash Live API (video frame ingestion)

**Prerequisite:** Phase 1+2 voice MVP is working -- audio round-trip, IPC AudioFrame routing, and Gemini session established.

**Branch:** Continue on `v2` branch (created in Phase 1).

**Important:** v1 line numbers referenced in these plans may have drifted. Always verify line numbers by reading the file before editing.

---

## Crate Map

| Crate | Status | Responsibility |
|---|---|---|
| `aura-navigator` | **NEW** | Action planning, step execution, confidence scoring, screen basis ID verification, adaptive post-action polling, surface preparation, cursor animation |
| `aura-tools` | **NEW** | 20+ tool handlers ported from v1's `aura-daemon/src/tools.rs`, sandbox enforcement, timeouts, result formatting |
| `aura-daemon` | **MODIFY** | VideoFrame IPC routing, Navigator integration, tool dispatch via aura-tools |
| `aura-gemini` | **MODIFY** | Updated tool declarations for v2, video frame sending |
| `aura-input` | **KEEP** | CGEvent mouse/keyboard injection (unchanged from v1) |
| `aura-bridge` | **KEEP** | AppleScript safety gates (unchanged from v1) |
| `aura-screen` | **KEEP** | Accessibility tree walking stays in Rust (click_element targeting) |

---

## File Structure

### Swift (AuraApp/Sources/)

| File | Status | Responsibility |
|---|---|---|
| `ScreenCapture.swift` | **NEW** | ScreenCaptureKit dual-path capture, basis IDs, image diffing, AuraApp exclusion |
| `Protocol.swift` | **MODIFY** | Add VideoFrame command |
| `DaemonConnection.swift` | **MODIFY** | Add sendVideoFrame() |
| `AppState.swift` | **MODIFY** | Add screen capture state, wire ScreenCapture lifecycle |
| `AuraApp.swift` | **MODIFY** | Start/stop ScreenCapture with daemon lifecycle |

### Rust (crates/)

| File | Status | Responsibility |
|---|---|---|
| `crates/aura-navigator/Cargo.toml` | **NEW** | Crate manifest |
| `crates/aura-navigator/src/lib.rs` | **NEW** | Module exports |
| `crates/aura-navigator/src/types.rs` | **NEW** | NavigatorPlan, NavigatorStep, ActionType, TargetDescriptor, VerifyContract |
| `crates/aura-navigator/src/planner.rs` | **NEW** | Plan building from Gemini tool calls, confidence scoring |
| `crates/aura-navigator/src/executor.rs` | **NEW** | Step execution, screen basis ID check, adaptive verification, fallback chains |
| `crates/aura-navigator/src/surface.rs` | **NEW** | Surface-specific prep (Chrome dismiss, Xcode autocomplete, Terminal focus) |
| `crates/aura-navigator/src/cursor.rs` | **NEW** | Smooth cursor animation (~100ms) |
| `crates/aura-tools/Cargo.toml` | **NEW** | Crate manifest |
| `crates/aura-tools/src/lib.rs` | **NEW** | Module exports, execute_tool entry point |
| `crates/aura-tools/src/mouse.rs` | **NEW** | click, click_element, context_menu_click, move_mouse, drag, scroll |
| `crates/aura-tools/src/keyboard.rs` | **NEW** | type_text, press_key, key_state, select_text |
| `crates/aura-tools/src/apps.rs` | **NEW** | activate_app, click_menu_item |
| `crates/aura-tools/src/text.rs` | **NEW** | write_clipboard, paste_text |
| `crates/aura-tools/src/scripts.rs` | **NEW** | run_applescript, run_shell_command |
| `crates/aura-tools/src/memory.rs` | **NEW** | save_memory, recall_memory |
| `crates/aura-tools/src/context.rs` | **NEW** | get_screen_context |
| `crates/aura-tools/src/system.rs` | **NEW** | shutdown_aura |
| `crates/aura-tools/src/safety.rs` | **NEW** | Safety gates: blocked apps, shell metachar rejection, obfuscation detection, JXA blocking, timeouts |
| `crates/aura-tools/src/helpers.rs` | **NEW** | FrameDims, truncate_tool_response, parse_modifiers, capture_post_state (ported from v1 tool_helpers.rs) |
| `crates/aura-daemon/src/protocol.rs` | **MODIFY** | Add VideoFrame command, ModeChanged/ProactiveIntervention events |
| `crates/aura-daemon/src/orchestrator.rs` | **MODIFY** | VideoFrame routing, Navigator integration |
| `crates/aura-gemini/src/tools.rs` | **MODIFY** | Add paste_text tool declaration, update descriptions for v2 |
| `crates/aura-gemini/src/session.rs` | **MODIFY** | Add send_video_frame() method |

---

## Task 1: Swift ScreenCapture.swift -- Dual-Path Capture Engine

**Files:**
- Create: `AuraApp/Sources/ScreenCapture.swift`
- Modify: `AuraApp/Sources/Protocol.swift`
- Modify: `AuraApp/Sources/DaemonConnection.swift`
- Modify: `AuraApp/Sources/AppState.swift`
- Modify: `AuraApp/Sources/AuraApp.swift`

ScreenCaptureKit provides window-specific and display capture with AuraApp exclusion. Two capture paths: fast path (1s interval, JPEG for Gemini video), smart path (15s interval, full screenshot + AX tree for proactive engine).

- [ ] **Step 1: Create ScreenCapture.swift**

```swift
// AuraApp/Sources/ScreenCapture.swift
import ScreenCaptureKit
import CoreImage
import Foundation

/// Dual-path screen capture engine using ScreenCaptureKit.
///
/// **Fast path:** 1s interval JPEG frames streamed to Gemini as video input.
/// **Smart path:** 15s interval (or on app switch) deep captures for proactive engine.
///
/// Each frame gets a UUID `screen_basis_id` for Navigator coordinate validation.
/// Image diffing skips unchanged frames to save bandwidth.
@MainActor
final class ScreenCapture {
    /// Called with (jpeg_base64, screen_basis_id) for each captured frame.
    var onVideoFrame: ((String, String) -> Void)?

    /// Called when Screen Recording permission appears to have been revoked.
    /// The daemon should surface this to the user via IPC (e.g., a status message).
    var onPermissionLost: (() -> Void)?

    /// Current screen basis ID (updated on every captured frame).
    private(set) var currentBasisId: String = UUID().uuidString

    private var fastTimer: Task<Void, Never>?
    private var smartTimer: Task<Void, Never>?
    private var isRunning = false

    /// Previous frame hash for image diffing (skip unchanged frames).
    private var lastFrameHash: UInt64 = 0

    /// Consecutive capture failures (nil result or all-black frame).
    /// When this reaches `maxConsecutiveFailures`, we assume Screen Recording
    /// permission was revoked and notify via `onPermissionLost`.
    private var consecutiveCaptureFailures = 0
    private let maxConsecutiveFailures = 5

    /// Pixel hash threshold — frames with delta below this are skipped.
    /// Value of 0 means any change triggers a send.
    private let hashDeltaThreshold: UInt64 = 0

    /// Maximum JPEG width for fast-path frames.
    private let maxWidth: CGFloat = 1920

    /// JPEG compression quality (0.0 - 1.0).
    private let jpegQuality: CGFloat = 0.80

    /// Fast path interval in seconds.
    private let fastIntervalSec: Double = 1.0

    /// Smart path interval in seconds.
    private let smartIntervalSec: Double = 15.0

    // MARK: - Lifecycle

    func start() {
        guard !isRunning else { return }
        isRunning = true

        fastTimer = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: UInt64(1_000_000_000 * (self?.fastIntervalSec ?? 1.0)))
                guard let self, self.isRunning else { continue }
                await self.captureFastPath()
            }
        }

        smartTimer = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: UInt64(1_000_000_000 * (self?.smartIntervalSec ?? 15.0)))
                guard let self, self.isRunning else { continue }
                await self.captureSmartPath()
            }
        }
    }

    func stop() {
        isRunning = false
        fastTimer?.cancel()
        smartTimer?.cancel()
        fastTimer = nil
        smartTimer = nil
    }

    /// Force an immediate capture (e.g., on app switch event).
    func captureNow() async {
        await captureFastPath()
    }

    // MARK: - Fast Path (1s video frames for Gemini)

    private func captureFastPath() async {
        do {
            // Get available content, excluding AuraApp
            let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)

            // Find AuraApp's windows to exclude
            let auraAppBundleId = Bundle.main.bundleIdentifier ?? "com.aura.app"
            let excludedApps = content.applications.filter { $0.bundleIdentifier == auraAppBundleId }

            // Capture the entire display excluding AuraApp
            guard let display = content.displays.first else { return }

            let filter = SCContentFilter(display: display, excludingApplications: excludedApps, exceptingWindows: [])
            let config = SCStreamConfiguration()
            config.width = Int(maxWidth)
            // Maintain aspect ratio
            let aspectRatio = CGFloat(display.height) / CGFloat(display.width)
            config.height = Int(maxWidth * aspectRatio)
            config.showsCursor = true
            config.captureResolution = .best
            // Downscale from Retina
            config.scalesToFit = true

            let image = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config)

            // Convert CGImage to JPEG data
            guard let jpegData = cgImageToJPEG(image, quality: jpegQuality) else {
                // nil JPEG conversion — treat as a capture failure
                recordCaptureFailure()
                return
            }

            // Detect all-black frames (< 1% non-zero pixels).
            // When Screen Recording permission is revoked mid-session,
            // ScreenCaptureKit silently returns black frames instead of erroring.
            if isFrameBlack(image) {
                recordCaptureFailure()
                return
            }

            // Successful capture — reset failure counter
            consecutiveCaptureFailures = 0

            // Compute frame hash for diffing
            let hash = computeFrameHash(image)

            // Skip if frame unchanged
            if hash == lastFrameHash && lastFrameHash != 0 {
                return
            }
            lastFrameHash = hash

            // Assign new screen basis ID
            let basisId = UUID().uuidString
            currentBasisId = basisId

            // Encode and send
            let base64 = jpegData.base64EncodedString()
            onVideoFrame?(base64, basisId)

        } catch {
            // ScreenCaptureKit errors (permission denied, etc.)
            print("[ScreenCapture] Fast path error: \(error.localizedDescription)")
            recordCaptureFailure()
        }
    }

    // MARK: - Permission Revocation Detection

    /// Record a capture failure and check if we've hit the consecutive failure threshold.
    private func recordCaptureFailure() {
        consecutiveCaptureFailures += 1
        if consecutiveCaptureFailures >= maxConsecutiveFailures {
            print("[ScreenCapture] \(maxConsecutiveFailures) consecutive failures — Screen Recording may have been revoked")
            onPermissionLost?()
            consecutiveCaptureFailures = 0  // Reset to avoid spamming the callback
        }
    }

    /// Check if a captured frame is effectively all black (< 1% non-zero pixels).
    /// When Screen Recording permission is revoked, ScreenCaptureKit returns black frames
    /// rather than throwing an error.
    private func isFrameBlack(_ image: CGImage) -> Bool {
        guard let data = image.dataProvider?.data,
              let bytes = CFDataGetBytePtr(data) else { return false }

        let totalBytes = CFDataGetLength(data)
        guard totalBytes > 0 else { return true }

        // Sample every 100th byte for performance (full scan is unnecessary)
        let stride = 100
        var nonZeroCount = 0
        var sampledCount = 0

        var i = 0
        while i < totalBytes {
            sampledCount += 1
            if bytes[i] != 0 {
                nonZeroCount += 1
            }
            i += stride
        }

        guard sampledCount > 0 else { return true }
        let nonZeroRatio = Double(nonZeroCount) / Double(sampledCount)
        return nonZeroRatio < 0.01  // Less than 1% non-zero pixels → effectively black
    }

    // MARK: - Smart Path (15s deep analysis for proactive engine)

    private func captureSmartPath() async {
        // Smart path: full screenshot + request AX tree from daemon.
        // Produces a richer ScreenContext for the proactive scorer.
        guard let display = try? await SCShareableContent.excludingDesktopWindows(
            false, onScreenWindowsOnly: true
        ).displays.first else { return }

        let filter = SCContentFilter(display: display, excludingApplications: [], exceptingWindows: [])
        let config = SCStreamConfiguration()
        config.width = display.width * 2
        config.height = display.height * 2
        config.capturesAudio = false
        config.showsCursor = false

        guard let image = try? await SCScreenshotManager.captureImage(
            contentFilter: filter, configuration: config
        ) else { return }

        let basisId = UUID().uuidString
        guard let jpegData = cgImageToJPEG(image, quality: 0.85) else { return }
        let base64 = jpegData.base64EncodedString()

        // Send as SmartCapture (includes request for AX tree from daemon)
        onSmartCapture?(base64, basisId)

        // Also send as regular video frame to Gemini
        let hash = computeFrameHash(image)
        if hash != lastFrameHash {
            lastFrameHash = hash
            onVideoFrame?(base64, basisId)
        }
    }

    /// Called when smart path capture completes. Daemon will combine with AX tree.
    var onSmartCapture: ((String, String) -> Void)?

    // MARK: - Image Helpers

    /// Convert CGImage to JPEG Data at the specified quality.
    private func cgImageToJPEG(_ cgImage: CGImage, quality: CGFloat) -> Data? {
        let ciImage = CIImage(cgImage: cgImage)
        let context = CIContext()
        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? cgImage.colorSpace ?? CGColorSpaceCreateDeviceRGB()

        return context.jpegRepresentation(
            of: ciImage,
            colorSpace: colorSpace,
            options: [kCGImageDestinationLossyCompressionQuality as CIImageRepresentationOption: quality]
        )
    }

    /// Compute an FNV-1a hash by sampling pixels across the frame.
    /// Matches the Rust implementation in aura-screen/capture.rs for consistency.
    private func computeFrameHash(_ image: CGImage) -> UInt64 {
        guard let dataProvider = image.dataProvider,
              let data = dataProvider.data,
              let bytes = CFDataGetBytePtr(data) else {
            return 0
        }

        let length = CFDataGetLength(data)
        let sampleCount = 8192
        let step = max(length / sampleCount, 1)

        var hash: UInt64 = 0xcbf29ce484222325 // FNV-1a offset basis
        let prime: UInt64 = 0x100000001b3       // FNV-1a prime

        var i = 0
        while i < length {
            hash ^= UInt64(bytes[i])
            hash = hash &* prime
            i += step
        }

        return hash
    }
}
```

- [ ] **Step 2: Add VideoFrame command to Protocol.swift**

In `AuraApp/Sources/Protocol.swift`, add the `VideoFrame` command to the `UICommand` enum. Read the existing file first, then add:

```swift
// Add to UICommand enum (alongside existing sendText, toggleMic, etc.):
case videoFrame(String, String) // (jpeg_base64, screen_basis_id)

// Add to UICommand encode(to:):
case .videoFrame(let jpeg, let basisId):
    try container.encode("video_frame", forKey: .type)
    try container.encode(jpeg, forKey: .text) // reuse text field for base64
    // screen_basis_id needs a dedicated key
    var nested = encoder.container(keyedBy: VideoFrameKeys.self)
    try nested.encode(basisId, forKey: .screenBasisId)

// Add the VideoFrameKeys enum:
private enum VideoFrameKeys: String, CodingKey {
    case screenBasisId = "screen_basis_id"
}
```

**Implementation note:** The existing `UICommand` in Protocol.swift uses `Encodable` with a `type` tag. Read the actual encoding pattern from the file and follow it exactly. The `VideoFrame` command must serialize as:
```json
{"type":"video_frame","jpeg_base64":"...","screen_basis_id":"..."}
```

- [ ] **Step 3: Add sendVideoFrame() to DaemonConnection.swift**

```swift
// Add to DaemonConnection.swift:
/// Send a screen capture frame to the daemon.
func sendVideoFrame(jpegBase64: String, screenBasisId: String) {
    send(.videoFrame(jpegBase64, screenBasisId))
}
```

- [ ] **Step 4: Wire ScreenCapture into AppState and AuraApp**

In `AppState.swift`, add:
```swift
var screenCapture: ScreenCapture?

func setupScreenCapture(connection: DaemonConnection) {
    let capture = ScreenCapture()
    capture.onVideoFrame = { [weak connection] jpeg, basisId in
        connection?.sendVideoFrame(jpegBase64: jpeg, screenBasisId: basisId)
    }
    capture.onPermissionLost = { [weak connection] in
        // Notify the daemon, which surfaces "Screen Recording disabled" to the user.
        connection?.send(.status("Screen Recording permission was revoked. Re-enable in System Settings > Privacy & Security > Screen Recording."))
    }
    self.screenCapture = capture
}
```

In `AuraApp.swift`, after connection is established (same place AudioEngine is wired):
```swift
appState.setupScreenCapture(connection: connection!)
appState.screenCapture?.start()
```

In `applicationWillTerminate`:
```swift
appState.screenCapture?.stop()
```

- [ ] **Step 5: Build and verify**

```bash
cd /Users/abdullahiabdi/Developer/aura && xcodebuild -project AuraApp/AuraApp.xcodeproj -scheme AuraApp -configuration Debug build 2>&1 | tail -5
```
Expected: BUILD SUCCEEDED. If AuraApp uses SPM, adjust: check `ls AuraApp/` for Package.swift vs xcodeproj.

- [ ] **Step 6: Commit**

```bash
git add AuraApp/Sources/ScreenCapture.swift AuraApp/Sources/Protocol.swift AuraApp/Sources/DaemonConnection.swift AuraApp/Sources/AppState.swift AuraApp/Sources/AuraApp.swift
git commit -m "feat(v2): add Swift ScreenCapture engine with dual-path capture

ScreenCaptureKit dual-path: fast (1s JPEG for Gemini video) and smart
(15s for proactive engine). Each frame gets a UUID screen_basis_id for
Navigator coordinate validation. FNV-1a image diffing skips unchanged
frames. AuraApp excluded from captures via SCContentFilter."
```

---

## Task 2: VideoFrame IPC Routing -- Swift to Daemon to Gemini

**Files:**
- Modify: `crates/aura-daemon/src/protocol.rs`
- Modify: `crates/aura-daemon/src/orchestrator.rs`
- Modify: `crates/aura-gemini/src/session.rs`

Video frames flow: Swift ScreenCapture -> IPC VideoFrame command -> Daemon -> Gemini WebSocket as realtime video input.

- [ ] **Step 1: Add VideoFrame to Rust IPC protocol**

In `crates/aura-daemon/src/protocol.rs`, add to the `UICommand` enum:

```rust
// Add to UICommand enum:
/// Screen capture frame from Swift ScreenCaptureKit.
VideoFrame {
    jpeg_base64: String,
    screen_basis_id: String,
},

/// Set autonomy mode.
SetMode { mode: String },

/// Dismiss a proactive intervention.
DismissIntervention { intervention_id: String },
```

Add to the `DaemonEvent` enum:

```rust
// Add to DaemonEvent enum:
/// Playback audio chunk from Gemini (base64-encoded 24kHz PCM).
PlaybackAudio { audio: String },

/// Start of a new playback stream.
PlaybackStart,

/// End of playback stream.
PlaybackStop,

/// Gemini was interrupted (barge-in).
Interrupted,

/// Autonomy mode changed.
ModeChanged { mode: String },

/// A proactive intervention suggestion.
ProactiveIntervention {
    text: String,
    intervention_id: String,
},
```

- [ ] **Step 2: Add send_video_frame to GeminiLiveSession**

In `crates/aura-gemini/src/session.rs`, add:

```rust
/// Send a JPEG video frame to Gemini as realtime video input.
/// Called when the Swift ScreenCapture sends a VideoFrame over IPC.
pub fn send_video_frame(&self, jpeg_base64: &str) -> Result<()> {
    let msg = serde_json::json!({
        "realtimeInput": {
            "video": {
                "mimeType": "image/jpeg",
                "data": jpeg_base64,
            }
        }
    });
    let json = serde_json::to_string(&msg)?;
    self.send_raw(&json)
}
```

- [ ] **Step 3: Route VideoFrame in orchestrator**

In `crates/aura-daemon/src/orchestrator.rs`, in the IPC command handler loop, add:

```rust
UICommand::VideoFrame { jpeg_base64, screen_basis_id } => {
    // Update the current screen basis ID for Navigator verification
    *current_screen_basis_id.lock().unwrap() = screen_basis_id;

    // Forward video frame to Gemini
    if let Err(e) = session.send_video_frame(&jpeg_base64) {
        tracing::warn!("Failed to send video frame to Gemini: {e}");
    }
}
```

Also add a shared state for the current screen basis ID:

```rust
// Near the top of the orchestrator, alongside existing shared state:
let current_screen_basis_id = Arc::new(std::sync::Mutex::new(String::new()));
```

- [ ] **Step 4: Run tests**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test -p aura-daemon -- protocol 2>&1
cargo test -p aura-gemini 2>&1 | tail -10
```
Expected: All existing tests pass. New VideoFrame type serializes correctly.

- [ ] **Step 5: Commit**

```bash
git add crates/aura-daemon/src/protocol.rs crates/aura-daemon/src/orchestrator.rs crates/aura-gemini/src/session.rs
git commit -m "feat(v2): route VideoFrame from Swift IPC to Gemini WebSocket

VideoFrame IPC command carries jpeg_base64 + screen_basis_id. Daemon
forwards JPEG to Gemini as realtime video input. screen_basis_id stored
in shared state for Navigator coordinate validation."
```

---

## Task 3: Create aura-navigator Crate -- Types and Planner

**Files:**
- Create: `crates/aura-navigator/Cargo.toml`
- Create: `crates/aura-navigator/src/lib.rs`
- Create: `crates/aura-navigator/src/types.rs`
- Create: `crates/aura-navigator/src/planner.rs`
- Modify: `Cargo.toml` (workspace members)

The Navigator builds multi-step plans from Gemini tool calls with confidence scoring.

- [ ] **Step 1: Create crate manifest**

```toml
# crates/aura-navigator/Cargo.toml
[package]
name = "aura-navigator"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
anyhow.workspace = true
serde = { workspace = true, features = ["derive"] }
serde_json.workspace = true
tokio.workspace = true
tracing.workspace = true
uuid = { version = "1", features = ["v4"] }
```

Add `"crates/aura-navigator"` to the workspace members in the root `Cargo.toml`.

- [ ] **Step 2: Create types.rs**

```rust
// crates/aura-navigator/src/types.rs
use serde::{Deserialize, Serialize};

/// A multi-step plan built by the Navigator for executing a Gemini tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigatorPlan {
    /// Ordered steps to execute.
    pub steps: Vec<NavigatorStep>,
    /// Screenshot the plan was built against.
    pub screen_basis_id: String,
    /// Overall plan confidence (0.0 - 1.0).
    pub confidence: f32,
    /// Tool name that triggered this plan.
    pub tool_name: String,
    /// Original tool arguments from Gemini.
    pub tool_args: serde_json::Value,
}

/// A single step within a NavigatorPlan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigatorStep {
    /// The primary action to perform.
    pub action: ActionType,
    /// How to find the target element.
    pub target: TargetDescriptor,
    /// What we expect to see after this step.
    pub verify: VerifyContract,
    /// Fallback action if the primary fails.
    pub fallback: Option<ActionType>,
    /// Screen basis ID this step's coordinates were calculated against.
    pub screen_basis_id: String,
}

/// Types of actions the Navigator can execute, in priority order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActionType {
    /// Accessibility API press. Deterministic, ~5ms. Primary path.
    AxPress {
        label: Option<String>,
        role: Option<String>,
        index: usize,
    },
    /// NSPasteboard snapshot -> set text -> Cmd+V -> restore. Fast, Unicode-safe.
    PasteText { text: String },
    /// CGEvent keyboard shortcut. For known app-specific shortcuts.
    Hotkey {
        key: String,
        modifiers: Vec<String>,
    },
    /// CGEvent click at (x, y). Fallback when AX targeting fails.
    ClickCoordinates {
        x: f64,
        y: f64,
        button: String,
        click_count: u32,
        modifiers: Vec<String>,
    },
    /// Type text character-by-character.
    TypeText { text: String },
    /// Scroll the viewport.
    Scroll { dx: i32, dy: i32 },
    /// Drag from one point to another.
    Drag {
        from_x: f64,
        from_y: f64,
        to_x: f64,
        to_y: f64,
        modifiers: Vec<String>,
    },
    /// Move mouse to coordinates.
    MoveMouse { x: f64, y: f64 },
    /// Adaptive wait: poll for screen stability, not fixed sleep.
    Wait {
        /// Maximum wait time in milliseconds.
        max_ms: u64,
    },
    /// Run an AppleScript.
    RunScript {
        script: String,
        timeout_secs: u64,
    },
    /// Run an allowlisted shell command.
    RunShell {
        command: String,
        args: Vec<String>,
        timeout_secs: u64,
    },
    /// Activate an application by name.
    ActivateApp { name: String },
    /// Click a menu bar item by path.
    ClickMenuItem {
        app: Option<String>,
        menu_path: Vec<String>,
    },
}

/// Describes how to locate the target element for an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetDescriptor {
    /// Accessibility role (e.g., "AXButton").
    pub role: Option<String>,
    /// Accessibility label text.
    pub label: Option<String>,
    /// Window title the target belongs to.
    pub window_title: Option<String>,
    /// Application name.
    pub app_name: Option<String>,
    /// Pixel coordinates in image space.
    pub coordinates: Option<(f64, f64)>,
    /// Human-readable description for vision escalation.
    pub description: Option<String>,
}

/// What we expect to see after an action completes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyContract {
    /// Type of verification to perform.
    pub check: VerifyCheck,
    /// Description of expected state for error reporting.
    pub expected_description: String,
}

/// Types of post-action verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VerifyCheck {
    /// Screen hash changed (any visual change).
    ScreenChanged,
    /// A specific element became focused.
    ElementFocused { label: Option<String>, role: Option<String> },
    /// App became frontmost.
    AppFrontmost { app_name: String },
    /// No verification needed (read-only actions).
    None,
}

/// Result of executing a NavigatorStep.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// Whether the step succeeded.
    pub success: bool,
    /// Which action was actually used (primary or fallback).
    pub method: String,
    /// Observed outcome after the action.
    pub observed_outcome: String,
    /// Error message if the step failed.
    pub error: Option<String>,
    /// Whether the screen basis ID was valid when checked.
    pub basis_id_valid: bool,
}

/// Result of executing an entire NavigatorPlan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanResult {
    /// Whether the entire plan succeeded.
    pub success: bool,
    /// Individual step results.
    pub step_results: Vec<StepResult>,
    /// Overall tool result to return to Gemini.
    pub tool_result: serde_json::Value,
}
```

- [ ] **Step 3: Create planner.rs**

```rust
// crates/aura-navigator/src/planner.rs
use crate::types::*;

/// Frame dimensions for coordinate mapping (image pixels -> logical screen points).
#[derive(Clone, Copy, Debug)]
pub struct FrameDims {
    pub img_w: u32,
    pub img_h: u32,
    pub logical_w: u32,
    pub logical_h: u32,
}

impl FrameDims {
    /// Map an x coordinate from image pixels to logical screen points.
    pub fn to_logical_x(self, x: f64) -> f64 {
        if self.img_w == 0 { return x; }
        x * (self.logical_w as f64 / self.img_w as f64)
    }

    /// Map a y coordinate from image pixels to logical screen points.
    pub fn to_logical_y(self, y: f64) -> f64 {
        if self.img_h == 0 { return y; }
        y * (self.logical_h as f64 / self.img_h as f64)
    }
}

/// Build a NavigatorPlan from a Gemini tool call.
///
/// Maps each tool name to a sequence of NavigatorSteps with appropriate
/// action types, verification contracts, and fallback chains.
pub fn build_plan(
    tool_name: &str,
    tool_args: &serde_json::Value,
    screen_basis_id: &str,
    dims: FrameDims,
) -> NavigatorPlan {
    let steps = match tool_name {
        "click" => build_click_plan(tool_args, screen_basis_id, dims),
        "click_element" => build_click_element_plan(tool_args, screen_basis_id),
        "type_text" => build_type_text_plan(tool_args, screen_basis_id),
        "press_key" => build_press_key_plan(tool_args, screen_basis_id),
        "scroll" => build_scroll_plan(tool_args, screen_basis_id),
        "drag" => build_drag_plan(tool_args, screen_basis_id, dims),
        "move_mouse" => build_move_mouse_plan(tool_args, screen_basis_id, dims),
        "activate_app" => build_activate_app_plan(tool_args, screen_basis_id),
        "click_menu_item" => build_click_menu_item_plan(tool_args, screen_basis_id),
        "paste_text" => build_paste_text_plan(tool_args, screen_basis_id),
        // Script/memory/context tools bypass Navigator planning -- executed directly
        _ => vec![],
    };

    let confidence = compute_confidence(tool_name, tool_args, steps.len());

    NavigatorPlan {
        steps,
        screen_basis_id: screen_basis_id.to_string(),
        confidence,
        tool_name: tool_name.to_string(),
        tool_args: tool_args.clone(),
    }
}

/// Compute confidence for a plan (0.0 - 1.0).
///
/// AX-based actions score highest (deterministic).
/// Coordinate-based actions score based on target description quality.
fn compute_confidence(tool_name: &str, args: &serde_json::Value, step_count: usize) -> f32 {
    if step_count == 0 {
        return 1.0; // Direct-execution tools (scripts, memory) are always confident
    }

    match tool_name {
        // AX-based: high confidence
        "click_element" => 0.95,
        "click_menu_item" => 0.90,
        "activate_app" => 0.95,
        // Keyboard: high confidence (no coordinates)
        "press_key" | "type_text" | "paste_text" => 0.95,
        // Coordinate-based: depends on target description
        "click" => {
            let has_target = args.get("target").and_then(|v| v.as_str()).is_some();
            let has_bounds = args.get("expected_bounds").is_some();
            match (has_target, has_bounds) {
                (true, true) => 0.85,
                (true, false) => 0.75,
                (false, _) => 0.55,
            }
        }
        "scroll" => 0.85,
        "drag" => 0.70,
        "move_mouse" => 0.90,
        _ => 0.70,
    }
}

fn build_click_plan(
    args: &serde_json::Value,
    basis_id: &str,
    dims: FrameDims,
) -> Vec<NavigatorStep> {
    let raw_x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let raw_y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let x = dims.to_logical_x(raw_x);
    let y = dims.to_logical_y(raw_y);
    let button = args.get("button").and_then(|v| v.as_str()).unwrap_or("left").to_string();
    let count = args.get("click_count").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
    let count = count.clamp(1, 3);
    let target_desc = args.get("target").and_then(|v| v.as_str()).map(String::from);
    let modifiers: Vec<String> = args.get("modifiers")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    vec![NavigatorStep {
        action: ActionType::ClickCoordinates {
            x, y,
            button: button.clone(),
            click_count: count,
            modifiers: modifiers.clone(),
        },
        target: TargetDescriptor {
            role: None,
            label: None,
            window_title: None,
            app_name: None,
            coordinates: Some((x, y)),
            description: target_desc,
        },
        verify: VerifyContract {
            check: VerifyCheck::ScreenChanged,
            expected_description: "Screen should change after click".to_string(),
        },
        fallback: None,
        screen_basis_id: basis_id.to_string(),
    }]
}

fn build_click_element_plan(
    args: &serde_json::Value,
    basis_id: &str,
) -> Vec<NavigatorStep> {
    let label = args.get("label").and_then(|v| v.as_str()).map(String::from);
    let role = args.get("role").and_then(|v| v.as_str()).map(String::from);
    let index = args.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

    vec![NavigatorStep {
        action: ActionType::AxPress {
            label: label.clone(),
            role: role.clone(),
            index,
        },
        target: TargetDescriptor {
            role: role.clone(),
            label: label.clone(),
            window_title: None,
            app_name: None,
            coordinates: None,
            description: label.clone(),
        },
        verify: VerifyContract {
            check: VerifyCheck::ScreenChanged,
            expected_description: format!("Element {:?} should respond to press", label),
        },
        fallback: None, // click_element_inner handles its own AXPress -> PID -> HID fallback
        screen_basis_id: basis_id.to_string(),
    }]
}

fn build_type_text_plan(args: &serde_json::Value, basis_id: &str) -> Vec<NavigatorStep> {
    let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();

    vec![NavigatorStep {
        action: ActionType::TypeText { text: text.clone() },
        target: TargetDescriptor {
            role: args.get("role").and_then(|v| v.as_str()).map(String::from),
            label: args.get("label").and_then(|v| v.as_str()).map(String::from),
            window_title: None,
            app_name: None,
            coordinates: None,
            description: Some("Currently focused text field".to_string()),
        },
        verify: VerifyContract {
            check: VerifyCheck::ScreenChanged,
            expected_description: "Text should appear in the field".to_string(),
        },
        fallback: Some(ActionType::PasteText { text }),
        screen_basis_id: basis_id.to_string(),
    }]
}

fn build_press_key_plan(args: &serde_json::Value, basis_id: &str) -> Vec<NavigatorStep> {
    let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let modifiers: Vec<String> = args.get("modifiers")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    vec![NavigatorStep {
        action: ActionType::Hotkey { key, modifiers },
        target: TargetDescriptor {
            role: None, label: None, window_title: None, app_name: None,
            coordinates: None,
            description: Some("Keyboard shortcut".to_string()),
        },
        verify: VerifyContract {
            check: VerifyCheck::ScreenChanged,
            expected_description: "Screen should respond to key press".to_string(),
        },
        fallback: None,
        screen_basis_id: basis_id.to_string(),
    }]
}

fn build_scroll_plan(args: &serde_json::Value, basis_id: &str) -> Vec<NavigatorStep> {
    let dx = args.get("dx").and_then(|v| v.as_i64()).unwrap_or(0).clamp(-1000, 1000) as i32;
    let dy = args.get("dy").and_then(|v| v.as_i64()).unwrap_or(0).clamp(-1000, 1000) as i32;

    vec![NavigatorStep {
        action: ActionType::Scroll { dx, dy },
        target: TargetDescriptor {
            role: None, label: None, window_title: None, app_name: None,
            coordinates: None,
            description: Some("Current viewport".to_string()),
        },
        verify: VerifyContract {
            check: VerifyCheck::ScreenChanged,
            expected_description: "Viewport should scroll".to_string(),
        },
        fallback: None,
        screen_basis_id: basis_id.to_string(),
    }]
}

fn build_drag_plan(
    args: &serde_json::Value,
    basis_id: &str,
    dims: FrameDims,
) -> Vec<NavigatorStep> {
    let fx = dims.to_logical_x(args.get("from_x").and_then(|v| v.as_f64()).unwrap_or(0.0));
    let fy = dims.to_logical_y(args.get("from_y").and_then(|v| v.as_f64()).unwrap_or(0.0));
    let tx = dims.to_logical_x(args.get("to_x").and_then(|v| v.as_f64()).unwrap_or(0.0));
    let ty = dims.to_logical_y(args.get("to_y").and_then(|v| v.as_f64()).unwrap_or(0.0));
    let modifiers: Vec<String> = args.get("modifiers")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    vec![NavigatorStep {
        action: ActionType::Drag { from_x: fx, from_y: fy, to_x: tx, to_y: ty, modifiers },
        target: TargetDescriptor {
            role: None, label: None, window_title: None, app_name: None,
            coordinates: Some((fx, fy)),
            description: Some(format!("Drag from ({fx:.0},{fy:.0}) to ({tx:.0},{ty:.0})")),
        },
        verify: VerifyContract {
            check: VerifyCheck::ScreenChanged,
            expected_description: "Element should move to new position".to_string(),
        },
        fallback: None,
        screen_basis_id: basis_id.to_string(),
    }]
}

fn build_move_mouse_plan(
    args: &serde_json::Value,
    basis_id: &str,
    dims: FrameDims,
) -> Vec<NavigatorStep> {
    let x = dims.to_logical_x(args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0));
    let y = dims.to_logical_y(args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0));

    vec![NavigatorStep {
        action: ActionType::MoveMouse { x, y },
        target: TargetDescriptor {
            role: None, label: None, window_title: None, app_name: None,
            coordinates: Some((x, y)),
            description: Some(format!("Move cursor to ({x:.0},{y:.0})")),
        },
        verify: VerifyContract {
            check: VerifyCheck::None,
            expected_description: "Cursor moved (no visual verification needed)".to_string(),
        },
        fallback: None,
        screen_basis_id: basis_id.to_string(),
    }]
}

fn build_activate_app_plan(args: &serde_json::Value, basis_id: &str) -> Vec<NavigatorStep> {
    let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();

    vec![NavigatorStep {
        action: ActionType::ActivateApp { name: name.clone() },
        target: TargetDescriptor {
            role: None, label: None, window_title: None,
            app_name: Some(name.clone()),
            coordinates: None,
            description: Some(format!("Activate {name}")),
        },
        verify: VerifyContract {
            check: VerifyCheck::AppFrontmost { app_name: name },
            expected_description: "App should become frontmost".to_string(),
        },
        fallback: None,
        screen_basis_id: basis_id.to_string(),
    }]
}

fn build_click_menu_item_plan(args: &serde_json::Value, basis_id: &str) -> Vec<NavigatorStep> {
    let menu_path: Vec<String> = args.get("menu_path")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let app = args.get("app").and_then(|v| v.as_str()).map(String::from);

    vec![NavigatorStep {
        action: ActionType::ClickMenuItem { app, menu_path },
        target: TargetDescriptor {
            role: Some("AXMenuItem".to_string()),
            label: None, window_title: None, app_name: None,
            coordinates: None,
            description: Some("Menu bar item".to_string()),
        },
        verify: VerifyContract {
            check: VerifyCheck::ScreenChanged,
            expected_description: "Menu action should take effect".to_string(),
        },
        fallback: None,
        screen_basis_id: basis_id.to_string(),
    }]
}

fn build_paste_text_plan(args: &serde_json::Value, basis_id: &str) -> Vec<NavigatorStep> {
    let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();

    vec![NavigatorStep {
        action: ActionType::PasteText { text },
        target: TargetDescriptor {
            role: None, label: None, window_title: None, app_name: None,
            coordinates: None,
            description: Some("Currently focused text field".to_string()),
        },
        verify: VerifyContract {
            check: VerifyCheck::ScreenChanged,
            expected_description: "Text should appear in the field".to_string(),
        },
        fallback: None,
        screen_basis_id: basis_id.to_string(),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_click_plan_creates_single_step() {
        let args = serde_json::json!({"x": 100.0, "y": 200.0, "target": "Submit button"});
        let dims = FrameDims { img_w: 1920, img_h: 1080, logical_w: 1440, logical_h: 900 };
        let plan = build_plan("click", &args, "test-basis-id", dims);
        assert_eq!(plan.steps.len(), 1);
        assert!(plan.confidence > 0.7);
        assert_eq!(plan.screen_basis_id, "test-basis-id");
    }

    #[test]
    fn click_element_has_high_confidence() {
        let args = serde_json::json!({"label": "OK", "role": "button"});
        let dims = FrameDims { img_w: 1920, img_h: 1080, logical_w: 1440, logical_h: 900 };
        let plan = build_plan("click_element", &args, "basis", dims);
        assert!(plan.confidence >= 0.90);
    }

    #[test]
    fn direct_tools_have_empty_steps() {
        let args = serde_json::json!({"query": "test"});
        let dims = FrameDims { img_w: 1920, img_h: 1080, logical_w: 1440, logical_h: 900 };
        let plan = build_plan("recall_memory", &args, "basis", dims);
        assert!(plan.steps.is_empty());
        assert_eq!(plan.confidence, 1.0);
    }

    #[test]
    fn scroll_clamps_to_bounds() {
        let args = serde_json::json!({"dx": 0, "dy": 5000});
        let dims = FrameDims { img_w: 1920, img_h: 1080, logical_w: 1440, logical_h: 900 };
        let plan = build_plan("scroll", &args, "basis", dims);
        if let ActionType::Scroll { dy, .. } = &plan.steps[0].action {
            assert_eq!(*dy, 1000); // Clamped
        } else {
            panic!("Expected Scroll action");
        }
    }

    #[test]
    fn frame_dims_coordinate_mapping() {
        let dims = FrameDims { img_w: 1920, img_h: 1080, logical_w: 1440, logical_h: 900 };
        assert!((dims.to_logical_x(1920.0) - 1440.0).abs() < 0.01);
        assert!((dims.to_logical_y(1080.0) - 900.0).abs() < 0.01);
        assert!((dims.to_logical_x(960.0) - 720.0).abs() < 0.01);
    }
}
```

- [ ] **Step 4: Create lib.rs**

```rust
// crates/aura-navigator/src/lib.rs
pub mod types;
pub mod planner;
pub mod executor;
pub mod surface;
pub mod cursor;

pub use planner::{build_plan, FrameDims};
pub use types::*;
```

- [ ] **Step 5: Build and test**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test -p aura-navigator 2>&1
```
Expected: All planner tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/aura-navigator/ Cargo.toml
git commit -m "feat(v2): create aura-navigator crate with types and planner

NavigatorPlan, NavigatorStep, ActionType, TargetDescriptor, VerifyContract
types. Planner builds multi-step plans from Gemini tool calls with
confidence scoring (AX-based > coordinate-based). FrameDims ported from
v1 tool_helpers.rs."
```

---

## Task 4: Navigator Executor -- Screen Basis ID Verification + Adaptive Polling

**Files:**
- Create: `crates/aura-navigator/src/executor.rs`
- Create: `crates/aura-navigator/src/surface.rs`
- Create: `crates/aura-navigator/src/cursor.rs`

The executor runs NavigatorPlans step-by-step with screen basis ID verification and adaptive post-action polling (NOT fixed 500ms sleep).

- [ ] **Step 1: Create cursor.rs -- Smooth cursor animation**

```rust
// crates/aura-navigator/src/cursor.rs
use anyhow::Result;
use std::time::Duration;

/// Smoothly animate the cursor from its current position to (target_x, target_y).
/// Duration is ~100ms with 10 interpolation steps.
///
/// This gives the user visual feedback about where Aura is acting, making
/// automation feel less jarring than teleporting the cursor.
pub async fn animate_cursor_to(target_x: f64, target_y: f64) -> Result<()> {
    // Get current cursor position
    let (start_x, start_y) = tokio::task::spawn_blocking(|| {
        get_cursor_position().unwrap_or((0.0, 0.0))
    }).await?;

    let steps = 10u32;
    let step_duration = Duration::from_millis(10); // 10ms * 10 = ~100ms total

    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        // Ease-out cubic for natural deceleration
        let eased = 1.0 - (1.0 - t).powi(3);
        let ix = start_x + (target_x - start_x) * eased;
        let iy = start_y + (target_y - start_y) * eased;

        let move_x = ix;
        let move_y = iy;
        tokio::task::spawn_blocking(move || {
            let _ = aura_input::mouse::move_mouse(move_x, move_y);
        }).await?;

        tokio::time::sleep(step_duration).await;
    }

    Ok(())
}

/// Get the current cursor position via CGEvent.
fn get_cursor_position() -> Result<(f64, f64)> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| anyhow::anyhow!("Failed to create event source"))?;
    let event = CGEvent::new(source)
        .map_err(|_| anyhow::anyhow!("Failed to create event for position query"))?;
    let point = event.location();
    Ok((point.x, point.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_cursor_position_returns_finite_coords() {
        if let Ok((x, y)) = get_cursor_position() {
            assert!(x.is_finite());
            assert!(y.is_finite());
        }
        // May fail in headless CI -- that's OK
    }
}
```

- [ ] **Step 2: Create surface.rs -- Surface-specific preparation**

```rust
// crates/aura-navigator/src/surface.rs

/// Send surface-specific preparation keystrokes before executing an action.
///
/// Some apps need prep work before actions land correctly:
/// - Chrome: dismiss address bar if focused (Escape)
/// - Xcode: dismiss autocomplete popup (Escape)
/// - Terminal: ensure focus on command line
pub async fn prepare_surface(app_name: &str) -> Vec<String> {
    let app_lower = app_name.to_lowercase();
    let mut prep_actions = Vec::new();

    match app_lower.as_str() {
        "google chrome" | "chrome" => {
            // Dismiss address bar focus with Escape so clicks hit the page
            if is_address_bar_focused().await {
                let _ = tokio::task::spawn_blocking(|| {
                    aura_input::keyboard::press_key(53, &[]) // 53 = Escape
                }).await;
                prep_actions.push("Chrome: dismissed address bar".to_string());
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
        "xcode" => {
            // Dismiss autocomplete popup if visible
            let _ = tokio::task::spawn_blocking(|| {
                aura_input::keyboard::press_key(53, &[]) // Escape
            }).await;
            prep_actions.push("Xcode: dismissed autocomplete".to_string());
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
        _ => {}
    }

    prep_actions
}

/// Check if Chrome's address bar is focused by examining the AX tree.
async fn is_address_bar_focused() -> bool {
    tokio::task::spawn_blocking(|| {
        let focused = aura_screen::accessibility::get_focused_element();
        focused
            .as_ref()
            .map(|el| {
                el.role == "AXTextField"
                    && el
                        .label
                        .as_deref()
                        .is_some_and(|l| l.contains("Address") || l.contains("URL") || l.contains("address"))
            })
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}
```

- [ ] **Step 3: Create executor.rs -- The core execution engine**

```rust
// crates/aura-navigator/src/executor.rs
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tracing;

use crate::types::*;

/// Interval between screen hash polls for adaptive verification.
const POLL_INTERVAL_MS: u64 = 100;
/// Number of consecutive stable hash checks required to consider the screen settled.
const STABLE_CHECKS_REQUIRED: u32 = 2;
/// Maximum time to wait for screen to settle after an action.
const VERIFY_TIMEOUT_MS: u64 = 3000;

/// Shared screen state the executor reads from the orchestrator.
pub struct ScreenState {
    /// Current screen basis ID (updated by VideoFrame IPC handler).
    pub current_basis_id: Arc<std::sync::Mutex<String>>,
}

/// Execute a NavigatorPlan step-by-step.
///
/// For each step:
/// 1. Verify screen_basis_id matches current screen (reject if changed)
/// 2. Prepare surface (app-specific keystrokes)
/// 3. Animate cursor to target position
/// 4. Execute the action (with fallback chain)
/// 5. Adaptive verification (poll screen hash, not fixed sleep)
pub async fn execute_plan(
    plan: &NavigatorPlan,
    screen_state: &ScreenState,
) -> PlanResult {
    let mut step_results = Vec::new();

    for (i, step) in plan.steps.iter().enumerate() {
        tracing::info!(
            step = i,
            action = ?step.action,
            "Executing navigator step"
        );

        let result = execute_step(step, screen_state).await;
        let success = result.success;
        step_results.push(result);

        if !success {
            // Step failed -- stop executing remaining steps
            tracing::warn!(step = i, "Navigator step failed, aborting plan");
            break;
        }
    }

    let overall_success = step_results.iter().all(|r| r.success);

    PlanResult {
        success: overall_success,
        step_results,
        tool_result: serde_json::json!({
            "success": overall_success,
            "plan_confidence": plan.confidence,
        }),
    }
}

/// Execute a single NavigatorStep with basis ID verification and adaptive polling.
async fn execute_step(
    step: &NavigatorStep,
    screen_state: &ScreenState,
) -> StepResult {
    // 1. Verify screen basis ID
    let basis_id_valid = verify_basis_id(&step.screen_basis_id, screen_state);
    if !basis_id_valid {
        // For coordinate-based actions, screen change means coordinates are stale
        if is_coordinate_action(&step.action) {
            return StepResult {
                success: false,
                method: "rejected".to_string(),
                observed_outcome: "Screen changed since plan was built".to_string(),
                error: Some("ScreenChanged: screen_basis_id mismatch. The screen has changed since this plan was built. Coordinates may be stale -- request a fresh screenshot and re-plan.".to_string()),
                basis_id_valid: false,
            };
        }
        // Non-coordinate actions (keyboard, AX press) can proceed with a warning
        tracing::warn!("Screen basis ID mismatch but action is non-coordinate -- proceeding");
    }

    // 2. Prepare surface (app-specific prep)
    if let Some(app) = get_current_app().await {
        let prep = crate::surface::prepare_surface(&app).await;
        if !prep.is_empty() {
            tracing::debug!(prep = ?prep, "Surface prepared");
        }
    }

    // 3. Vision escalation for low-confidence coordinate clicks
    // If ClickCoordinates with confidence < 0.7, send a high-res screenshot
    // + target description to the Gemini Live session for refined coordinates.
    // This replaces the v1 Vision Oracle (REST) with an in-session approach.
    if is_coordinate_action(&step.action) && step.confidence.unwrap_or(1.0) < 0.7 {
        if let Some(ref target_desc) = step.target.description {
            tracing::info!(
                confidence = step.confidence,
                target = target_desc,
                "Low confidence coordinate action — requesting vision escalation from Gemini"
            );
            // Send the current screenshot + target description to Gemini
            // via the live session's text input. Gemini responds with refined
            // coordinates as a tool call or text response.
            // The caller (execute_plan) should handle this by:
            // 1. Sending a text message: "Look at the screen. Find the exact
            //    coordinates of: {target_desc}. Return click(x, y, target='{target_desc}')"
            // 2. Waiting for Gemini's response (a new tool call with refined coords)
            // 3. Using the refined coordinates for this step
            //
            // For v2 MVP: if vision escalation is not wired yet, proceed with
            // original coordinates and log a warning.
            tracing::warn!(
                "Vision escalation not yet wired — proceeding with original coordinates"
            );
        }
    }

    // 4. Animate cursor to target (for coordinate-based actions)
    if let Some((x, y)) = step.target.coordinates {
        if let Err(e) = crate::cursor::animate_cursor_to(x, y).await {
            tracing::warn!("Cursor animation failed: {e}");
            // Fall back to direct move
            let _ = tokio::task::spawn_blocking(move || {
                aura_input::mouse::move_mouse(x, y)
            }).await;
        }
    }

    // 4. Execute the primary action
    let pre_hash = capture_screen_hash().await;
    let action_result = execute_action(&step.action).await;

    if !action_result.success {
        // Try fallback if available
        if let Some(ref fallback) = step.fallback {
            tracing::info!("Primary action failed, trying fallback");
            let fallback_result = execute_action(fallback).await;
            if fallback_result.success {
                let verified = adaptive_verify(&step.verify, pre_hash).await;
                return StepResult {
                    success: verified,
                    method: format!("fallback: {}", fallback_result.method),
                    observed_outcome: if verified {
                        "Fallback succeeded, screen changed".to_string()
                    } else {
                        "Fallback executed but screen unchanged".to_string()
                    },
                    error: None,
                    basis_id_valid,
                };
            }
        }
        return StepResult {
            success: false,
            method: action_result.method,
            observed_outcome: "Action failed".to_string(),
            error: action_result.error,
            basis_id_valid,
        };
    }

    // 5. Adaptive post-action verification
    let verified = adaptive_verify(&step.verify, pre_hash).await;

    StepResult {
        success: action_result.success,
        method: action_result.method,
        observed_outcome: if verified {
            "Action succeeded, verified".to_string()
        } else {
            "Action executed but verification inconclusive".to_string()
        },
        error: None,
        basis_id_valid,
    }
}

/// Verify the screen basis ID matches the current screen.
fn verify_basis_id(expected: &str, screen_state: &ScreenState) -> bool {
    if expected.is_empty() {
        return true; // No basis ID to check
    }
    let current = screen_state
        .current_basis_id
        .lock()
        .map(|id| id.clone())
        .unwrap_or_default();
    if current.is_empty() {
        return true; // No current ID yet (first frame hasn't arrived)
    }
    current == expected
}

/// Check if an action uses coordinates that would be invalidated by screen changes.
fn is_coordinate_action(action: &ActionType) -> bool {
    matches!(
        action,
        ActionType::ClickCoordinates { .. }
            | ActionType::Drag { .. }
            | ActionType::MoveMouse { .. }
    )
}

/// Adaptive post-action verification.
///
/// Instead of a fixed 500ms sleep, polls the screen hash at 100ms intervals.
/// Screen is considered settled when 2 consecutive checks show the same hash
/// AND the hash differs from pre_hash (meaning the action had visible effect).
/// Times out after 3 seconds.
async fn adaptive_verify(contract: &VerifyContract, pre_hash: u64) -> bool {
    match &contract.check {
        VerifyCheck::None => return true,
        VerifyCheck::AppFrontmost { app_name } => {
            // Poll for app to become frontmost
            let target = app_name.clone();
            let deadline = Instant::now() + Duration::from_millis(VERIFY_TIMEOUT_MS);
            while Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
                if let Some(current) = get_current_app().await {
                    if current.to_lowercase().contains(&target.to_lowercase()) {
                        return true;
                    }
                }
            }
            return false;
        }
        VerifyCheck::ElementFocused { label, role } => {
            let target_label = label.clone();
            let target_role = role.clone();
            let deadline = Instant::now() + Duration::from_millis(VERIFY_TIMEOUT_MS);
            while Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
                let focused = tokio::task::spawn_blocking(|| {
                    aura_screen::accessibility::get_focused_element()
                }).await.ok().flatten();
                if let Some(el) = focused {
                    let label_match = target_label.as_ref().map_or(true, |l| {
                        el.label.as_ref().is_some_and(|el_l| el_l.to_lowercase().contains(&l.to_lowercase()))
                    });
                    let role_match = target_role.as_ref().map_or(true, |r| {
                        el.role.to_lowercase().contains(&r.to_lowercase())
                    });
                    if label_match && role_match {
                        return true;
                    }
                }
            }
            return false;
        }
        VerifyCheck::ScreenChanged => {
            // Adaptive polling: hash at 100ms intervals
            let deadline = Instant::now() + Duration::from_millis(VERIFY_TIMEOUT_MS);
            let mut stable_count: u32 = 0;
            let mut last_hash = pre_hash;

            while Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
                let current_hash = capture_screen_hash().await;

                if current_hash != pre_hash {
                    // Screen changed from pre-action state
                    if current_hash == last_hash {
                        stable_count += 1;
                        if stable_count >= STABLE_CHECKS_REQUIRED {
                            // Screen changed and is now stable
                            return true;
                        }
                    } else {
                        // Still changing -- reset stability counter
                        stable_count = 0;
                    }
                }
                last_hash = current_hash;
            }

            // Timeout -- check if screen changed at all
            last_hash != pre_hash
        }
    }
}

/// Capture a screen hash for comparison.
async fn capture_screen_hash() -> u64 {
    tokio::task::spawn_blocking(|| {
        aura_screen::capture::capture_screen()
            .map(|f| f.hash)
            .unwrap_or(0)
    })
    .await
    .unwrap_or(0)
}

/// Get the current frontmost app name.
async fn get_current_app() -> Option<String> {
    tokio::task::spawn_blocking(|| {
        aura_screen::macos::get_frontmost_app()
    })
    .await
    .ok()
    .flatten()
}

struct ActionResult {
    success: bool,
    method: String,
    error: Option<String>,
}

/// Execute a single action via the appropriate aura-input / aura-bridge handler.
async fn execute_action(action: &ActionType) -> ActionResult {
    match action {
        ActionType::AxPress { label, role, index } => {
            let l = label.clone();
            let r = role.clone();
            let idx = *index;
            let result = tokio::task::spawn_blocking(move || {
                aura_screen::accessibility::ax_perform_action_nth(
                    l.as_deref(),
                    r.as_deref(),
                    "AXPress",
                    idx,
                )
            })
            .await;
            match result {
                Ok(ax_result) if ax_result.success => ActionResult {
                    success: true,
                    method: "ax_press".to_string(),
                    error: None,
                },
                Ok(ax_result) => ActionResult {
                    success: false,
                    method: "ax_press".to_string(),
                    error: ax_result.error,
                },
                Err(e) => ActionResult {
                    success: false,
                    method: "ax_press".to_string(),
                    error: Some(format!("Task panicked: {e}")),
                },
            }
        }

        ActionType::ClickCoordinates { x, y, button, click_count, modifiers } => {
            let cx = *x;
            let cy = *y;
            let btn = button.clone();
            let count = *click_count;
            let mods = modifiers.clone();

            // Pre-move hover (40ms settle)
            let _ = tokio::task::spawn_blocking(move || {
                aura_input::mouse::move_mouse(cx, cy)
            }).await;
            tokio::time::sleep(Duration::from_millis(40)).await;

            // Try PID-targeted first, then HID fallback
            let pid = tokio::task::spawn_blocking(|| {
                aura_screen::macos::get_frontmost_pid()
            }).await.ok().flatten();

            let cx2 = cx;
            let cy2 = cy;
            let btn2 = btn.clone();
            let mods2 = mods.clone();

            if let Some(pid) = pid {
                let result = tokio::task::spawn_blocking(move || {
                    let mod_refs: Vec<&str> = mods.iter().map(|s| s.as_str()).collect();
                    aura_input::mouse::click_pid(cx, cy, &btn, count, &mod_refs, pid)
                }).await;
                if result.is_ok() && result.unwrap().is_ok() {
                    return ActionResult {
                        success: true,
                        method: "pid_click".to_string(),
                        error: None,
                    };
                }
            }

            // HID fallback
            let result = tokio::task::spawn_blocking(move || {
                let mod_refs: Vec<&str> = mods2.iter().map(|s| s.as_str()).collect();
                aura_input::mouse::click(cx2, cy2, &btn2, count, &mod_refs)
            }).await;
            match result {
                Ok(Ok(())) => ActionResult {
                    success: true,
                    method: "hid_click".to_string(),
                    error: None,
                },
                Ok(Err(e)) => ActionResult {
                    success: false,
                    method: "hid_click".to_string(),
                    error: Some(format!("{e}")),
                },
                Err(e) => ActionResult {
                    success: false,
                    method: "hid_click".to_string(),
                    error: Some(format!("Task panicked: {e}")),
                },
            }
        }

        ActionType::Hotkey { key, modifiers } => {
            let k = key.clone();
            let mods = modifiers.clone();
            let result = tokio::task::spawn_blocking(move || {
                match aura_input::keyboard::keycode_from_name(&k) {
                    Some(kc) => {
                        let mod_refs: Vec<&str> = mods.iter().map(|s| s.as_str()).collect();
                        aura_input::keyboard::press_key(kc, &mod_refs)
                    }
                    None => Err(anyhow::anyhow!("Unknown key: {k}")),
                }
            }).await;
            match result {
                Ok(Ok(())) => ActionResult { success: true, method: "hotkey".to_string(), error: None },
                Ok(Err(e)) => ActionResult { success: false, method: "hotkey".to_string(), error: Some(format!("{e}")) },
                Err(e) => ActionResult { success: false, method: "hotkey".to_string(), error: Some(format!("Task panicked: {e}")) },
            }
        }

        ActionType::TypeText { text } => {
            let t = text.clone();
            let result = tokio::task::spawn_blocking(move || {
                aura_input::keyboard::type_text(&t)
            }).await;
            match result {
                Ok(Ok(())) => ActionResult { success: true, method: "type_text".to_string(), error: None },
                Ok(Err(e)) => ActionResult { success: false, method: "type_text".to_string(), error: Some(format!("{e}")) },
                Err(e) => ActionResult { success: false, method: "type_text".to_string(), error: Some(format!("Task panicked: {e}")) },
            }
        }

        ActionType::PasteText { text } => {
            let t = text.clone();
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                // Snapshot current clipboard
                let prev = aura_screen::macos::get_clipboard();
                // Set clipboard to new text
                aura_screen::macos::set_clipboard(&t)?;
                // Cmd+V to paste
                let v_key = aura_input::keyboard::keycode_from_name("v").unwrap();
                aura_input::keyboard::press_key(v_key, &["cmd"])?;
                // Brief delay for paste to land
                std::thread::sleep(std::time::Duration::from_millis(50));
                // Restore previous clipboard
                if let Some(ref prev_text) = prev {
                    let _ = aura_screen::macos::set_clipboard(prev_text);
                } else {
                    let _ = aura_screen::macos::set_clipboard("");
                }
                Ok(())
            }).await;
            match result {
                Ok(Ok(())) => ActionResult { success: true, method: "paste_text".to_string(), error: None },
                Ok(Err(e)) => ActionResult { success: false, method: "paste_text".to_string(), error: Some(format!("{e}")) },
                Err(e) => ActionResult { success: false, method: "paste_text".to_string(), error: Some(format!("Task panicked: {e}")) },
            }
        }

        ActionType::Scroll { dx, dy } => {
            let sdx = *dx;
            let sdy = *dy;
            let result = tokio::task::spawn_blocking(move || {
                aura_input::mouse::scroll(sdx, sdy)
            }).await;
            match result {
                Ok(Ok(())) => ActionResult { success: true, method: "scroll".to_string(), error: None },
                Ok(Err(e)) => ActionResult { success: false, method: "scroll".to_string(), error: Some(format!("{e}")) },
                Err(e) => ActionResult { success: false, method: "scroll".to_string(), error: Some(format!("Task panicked: {e}")) },
            }
        }

        ActionType::Drag { from_x, from_y, to_x, to_y, modifiers } => {
            let fx = *from_x;
            let fy = *from_y;
            let tx = *to_x;
            let ty = *to_y;
            let mods = modifiers.clone();
            let result = tokio::task::spawn_blocking(move || {
                let mod_refs: Vec<&str> = mods.iter().map(|s| s.as_str()).collect();
                aura_input::mouse::drag(fx, fy, tx, ty, &mod_refs)
            }).await;
            match result {
                Ok(Ok(())) => ActionResult { success: true, method: "drag".to_string(), error: None },
                Ok(Err(e)) => ActionResult { success: false, method: "drag".to_string(), error: Some(format!("{e}")) },
                Err(e) => ActionResult { success: false, method: "drag".to_string(), error: Some(format!("Task panicked: {e}")) },
            }
        }

        ActionType::MoveMouse { x, y } => {
            let mx = *x;
            let my = *y;
            let result = tokio::task::spawn_blocking(move || {
                aura_input::mouse::move_mouse(mx, my)
            }).await;
            match result {
                Ok(Ok(())) => ActionResult { success: true, method: "move_mouse".to_string(), error: None },
                Ok(Err(e)) => ActionResult { success: false, method: "move_mouse".to_string(), error: Some(format!("{e}")) },
                Err(e) => ActionResult { success: false, method: "move_mouse".to_string(), error: Some(format!("Task panicked: {e}")) },
            }
        }

        ActionType::Wait { max_ms } => {
            // Adaptive wait: poll screen hash until stable
            let deadline = Instant::now() + Duration::from_millis(*max_ms);
            let mut last_hash = capture_screen_hash().await;
            let mut stable = 0u32;
            while Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
                let hash = capture_screen_hash().await;
                if hash == last_hash {
                    stable += 1;
                    if stable >= STABLE_CHECKS_REQUIRED {
                        break;
                    }
                } else {
                    stable = 0;
                    last_hash = hash;
                }
            }
            ActionResult { success: true, method: "wait".to_string(), error: None }
        }

        ActionType::RunScript { script, timeout_secs } => {
            let s = script.clone();
            let t = *timeout_secs;
            let executor = aura_bridge::script::ScriptExecutor::new();
            let result = executor
                .run(&s, aura_bridge::script::ScriptLanguage::AppleScript, t)
                .await;
            ActionResult {
                success: result.success,
                method: "applescript".to_string(),
                error: if result.success {
                    None
                } else {
                    Some(result.stderr)
                },
            }
        }

        ActionType::RunShell { command, args, timeout_secs } => {
            // Shell execution is handled by aura-tools/scripts.rs
            // This is a placeholder -- actual execution routes through execute_tool
            ActionResult {
                success: false,
                method: "shell".to_string(),
                error: Some("Shell commands should be routed through aura-tools".to_string()),
            }
        }

        ActionType::ActivateApp { name } => {
            let n = name.clone();
            let script = format!(
                "tell application \"{}\" to activate",
                n.replace(['\\', '"'], "")
            );
            let executor = aura_bridge::script::ScriptExecutor::new();
            let result = executor
                .run(&script, aura_bridge::script::ScriptLanguage::AppleScript, 10)
                .await;
            ActionResult {
                success: result.success,
                method: "activate_app".to_string(),
                error: if result.success { None } else { Some(result.stderr) },
            }
        }

        ActionType::ClickMenuItem { app, menu_path } => {
            let frontmost = if let Some(a) = app {
                a.clone()
            } else {
                get_current_app().await.unwrap_or_default()
            };
            if menu_path.is_empty() {
                return ActionResult {
                    success: false,
                    method: "click_menu_item".to_string(),
                    error: Some("Empty menu path".to_string()),
                };
            }
            // Build AppleScript for menu click (same pattern as v1 tool_helpers.rs)
            let process = frontmost.replace(['\\', '"'], "");
            let escaped: Vec<String> = menu_path.iter().map(|s| s.replace(['\\', '"'], "")).collect();
            let script = match escaped.len() {
                2 => format!(
                    "tell application \"System Events\" to tell process \"{process}\"\n\
                     \tclick menu item \"{}\" of menu 1 of menu bar item \"{}\" of menu bar 1\n\
                     end tell",
                    escaped[1], escaped[0]
                ),
                3 => format!(
                    "tell application \"System Events\" to tell process \"{process}\"\n\
                     \tclick menu item \"{}\" of menu 1 of menu item \"{}\" of menu 1 of menu bar item \"{}\" of menu bar 1\n\
                     end tell",
                    escaped[2], escaped[1], escaped[0]
                ),
                _ => {
                    let leaf = escaped.last().unwrap();
                    let mut chain = format!("menu item \"{leaf}\"");
                    for item in escaped[1..escaped.len() - 1].iter().rev() {
                        chain = format!("{chain} of menu 1 of menu item \"{item}\"");
                    }
                    chain = format!("{chain} of menu 1 of menu bar item \"{}\"", escaped[0]);
                    format!(
                        "tell application \"System Events\" to tell process \"{process}\"\n\
                         \tclick {chain} of menu bar 1\n\
                         end tell"
                    )
                }
            };
            let executor = aura_bridge::script::ScriptExecutor::new();
            let result = executor
                .run(&script, aura_bridge::script::ScriptLanguage::AppleScript, 15)
                .await;
            ActionResult {
                success: result.success,
                method: "click_menu_item".to_string(),
                error: if result.success { None } else { Some(result.stderr) },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_basis_id_matches() {
        let state = ScreenState {
            current_basis_id: Arc::new(std::sync::Mutex::new("abc-123".to_string())),
        };
        assert!(verify_basis_id("abc-123", &state));
        assert!(!verify_basis_id("xyz-456", &state));
    }

    #[test]
    fn verify_basis_id_empty_is_ok() {
        let state = ScreenState {
            current_basis_id: Arc::new(std::sync::Mutex::new(String::new())),
        };
        assert!(verify_basis_id("any-id", &state));
        assert!(verify_basis_id("", &state));
    }

    #[test]
    fn is_coordinate_action_detects_correctly() {
        assert!(is_coordinate_action(&ActionType::ClickCoordinates {
            x: 0.0, y: 0.0, button: "left".into(), click_count: 1, modifiers: vec![],
        }));
        assert!(is_coordinate_action(&ActionType::Drag {
            from_x: 0.0, from_y: 0.0, to_x: 1.0, to_y: 1.0, modifiers: vec![],
        }));
        assert!(!is_coordinate_action(&ActionType::AxPress {
            label: None, role: None, index: 0,
        }));
        assert!(!is_coordinate_action(&ActionType::Hotkey {
            key: "a".into(), modifiers: vec![],
        }));
    }
}
```

- [ ] **Step 4: Add aura-navigator dependencies to Cargo.toml**

The crate needs `aura-input`, `aura-screen`, and `aura-bridge` as dependencies:

```toml
# Add to crates/aura-navigator/Cargo.toml [dependencies]:
aura-input = { path = "../aura-input" }
aura-screen = { path = "../aura-screen" }
aura-bridge = { path = "../aura-bridge" }
core-graphics = "0.24"
```

- [ ] **Step 5: Build and test**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test -p aura-navigator 2>&1
```
Expected: All tests pass (basis ID verification, coordinate detection, cursor position).

- [ ] **Step 6: Commit**

```bash
git add crates/aura-navigator/
git commit -m "feat(v2): navigator executor with basis ID verification and adaptive polling

Screen basis ID check rejects coordinate actions if screen changed.
Adaptive post-action verification polls screen hash at 100ms intervals
with 2 consecutive stable checks and 3s timeout (replaces fixed 500ms
sleep). Smooth cursor animation (~100ms ease-out). Surface preparation
for Chrome/Xcode."
```

---

## Task 5: Create aura-tools Crate -- Port v1 Tool Handlers

**Files:**
- Create: `crates/aura-tools/Cargo.toml`
- Create: `crates/aura-tools/src/lib.rs`
- Create: `crates/aura-tools/src/safety.rs`
- Create: `crates/aura-tools/src/helpers.rs`
- Create: `crates/aura-tools/src/mouse.rs`
- Create: `crates/aura-tools/src/keyboard.rs`
- Create: `crates/aura-tools/src/apps.rs`
- Create: `crates/aura-tools/src/text.rs`
- Create: `crates/aura-tools/src/scripts.rs`
- Create: `crates/aura-tools/src/memory.rs`
- Create: `crates/aura-tools/src/context.rs`
- Create: `crates/aura-tools/src/system.rs`
- Modify: `Cargo.toml` (workspace members)

Port all 20+ tool handlers from `crates/aura-daemon/src/tools.rs` (63KB) and `crates/aura-daemon/src/tool_helpers.rs` into a dedicated crate, organized by category.

- [ ] **Step 1: Create crate manifest**

```toml
# crates/aura-tools/Cargo.toml
[package]
name = "aura-tools"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
anyhow.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tracing.workspace = true
aura-input = { path = "../aura-input" }
aura-screen = { path = "../aura-screen" }
aura-bridge = { path = "../aura-bridge" }
aura-navigator = { path = "../aura-navigator" }
base64 = "0.22"
image = "0.25"

[dev-dependencies]
tokio = { workspace = true, features = ["test-util"] }
```

Add `"crates/aura-tools"` to the workspace members in root `Cargo.toml`.

- [ ] **Step 2: Create safety.rs -- Port all safety gates**

```rust
// crates/aura-tools/src/safety.rs
//! Safety gates for tool execution.
//!
//! Ported from v1's aura-daemon/src/tools.rs and aura-bridge/src/script.rs.
//! All gates are checked BEFORE any tool execution.

/// Terminal emulator apps that Aura must never activate or open.
pub const BLOCKED_TERMINAL_APPS: &[&str] = &[
    "terminal", "iterm", "iterm2", "kitty", "alacritty", "warp",
    "hyper", "tabby", "rio", "wezterm", "ghostty",
];

/// Bundle IDs for blocked terminal apps.
pub const BLOCKED_TERMINAL_BUNDLES: &[&str] = &[
    "com.apple.terminal", "com.googlecode.iterm2", "net.kovidgoyal.kitty",
    "io.alacritty", "dev.warp.warp-stable", "co.zeit.hyper",
    "com.github.nicehash.tabby", "com.raphaelamorim.rio",
    "com.github.wez.wezterm", "com.mitchellh.ghostty",
];

/// Shell metacharacters that must not appear in any argument.
pub const SHELL_METACHARACTERS: &[&str] = &["|", ";", "`", "$(", ">", "<", "&&", "||"];

/// Allowlisted shell commands with their absolute paths.
pub const ALLOWED_SHELL_COMMANDS: &[(&str, &str)] = &[
    ("defaults", "/usr/bin/defaults"),
    ("open", "/usr/bin/open"),
    ("killall", "/usr/bin/killall"),
    ("say", "/usr/bin/say"),
    ("launchctl", "/bin/launchctl"),
];

/// Blocked `defaults` domains that could compromise system security.
pub const BLOCKED_DEFAULTS_DOMAINS: &[&str] = &[
    "com.apple.security", "com.apple.loginwindow", "com.apple.screensaver",
];

/// Maximum characters allowed in a single type_text call.
pub const TYPE_TEXT_MAX_CHARS: usize = 10_000;

/// Maximum click count (1 = single, 2 = double, 3 = triple).
pub const CLICK_COUNT_MAX: u32 = 3;

/// Maximum absolute scroll amount in either axis.
pub const SCROLL_MAX: i32 = 1000;

/// Maximum timeout for AppleScript calls.
pub const MAX_APPLESCRIPT_TIMEOUT_SECS: u64 = 120;

/// Maximum timeout for shell commands.
pub const MAX_SHELL_TIMEOUT_SECS: u64 = 60;

/// Maximum output size from shell commands.
pub const MAX_SHELL_OUTPUT_BYTES: usize = 10_240;

/// Default tool execution timeout (30s, configurable per tool).
pub const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 30;

/// Check if an app name is a blocked terminal emulator.
pub fn is_blocked_terminal(name: &str) -> bool {
    let lower = name.to_lowercase();
    BLOCKED_TERMINAL_APPS.iter().any(|app| lower.contains(app))
}

/// Check if a bundle ID is a blocked terminal emulator.
pub fn is_blocked_terminal_bundle(bundle_id: &str) -> bool {
    let lower = bundle_id.to_lowercase();
    BLOCKED_TERMINAL_BUNDLES.iter().any(|b| lower == *b)
}

/// Check if any shell metacharacter appears in an argument.
pub fn contains_shell_metachar(arg: &str) -> bool {
    SHELL_METACHARACTERS.iter().any(|mc| arg.contains(mc))
}

/// Check if "sudo" appears in shell arguments.
pub fn contains_sudo(args: &[String]) -> bool {
    args.iter().any(|a| a == "sudo" || a.starts_with("sudo "))
}

/// Validate shell command arguments for safety.
/// Returns Some(error_message) if the arguments are unsafe.
pub fn validate_shell_args(command: &str, args: &[String]) -> Option<String> {
    // Check for shell metacharacters in all arguments
    for arg in args {
        if contains_shell_metachar(arg) {
            return Some(format!(
                "Shell metacharacter detected in argument '{}'. \
                 Pipe, redirect, and chaining operators are not allowed.",
                arg
            ));
        }
    }

    // Check for sudo
    if contains_sudo(args) {
        return Some("'sudo' is not allowed in shell commands.".to_string());
    }

    // Check blocked defaults domains
    if command == "defaults" {
        for arg in args {
            let lower = arg.to_lowercase();
            for domain in BLOCKED_DEFAULTS_DOMAINS {
                if lower.contains(domain) {
                    return Some(format!("Blocked defaults domain: {domain}"));
                }
            }
        }
    }

    // Check for terminal app in 'open' command
    if command == "open" {
        for arg in args {
            if is_blocked_terminal(arg) || is_blocked_terminal_bundle(arg) {
                return Some(format!("Cannot open terminal app: {arg}"));
            }
        }
    }

    None
}

/// Check if an automation denial error is present in osascript stderr.
pub fn is_automation_denied(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("not allowed assistive access")
        || lower.contains("not allowed to send keystrokes")
        || lower.contains("errordomain=nscocoaerrordomain code=-1743")
        || lower.contains("errordomain=nscocoaerrordomain code=-600")
        || lower.contains("not authorized to perform")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_terminal_apps() {
        assert!(is_blocked_terminal("Terminal"));
        assert!(is_blocked_terminal("iTerm2"));
        assert!(is_blocked_terminal("warp"));
        assert!(is_blocked_terminal("Ghostty"));
        assert!(!is_blocked_terminal("Safari"));
        assert!(!is_blocked_terminal("Chrome"));
    }

    #[test]
    fn detects_shell_metachars() {
        assert!(contains_shell_metachar("ls | grep foo"));
        assert!(contains_shell_metachar("echo $(whoami)"));
        assert!(contains_shell_metachar("cmd1 && cmd2"));
        assert!(!contains_shell_metachar("hello world"));
    }

    #[test]
    fn validates_shell_args_rejects_metachar() {
        let result = validate_shell_args("defaults", &["read | malicious".to_string()]);
        assert!(result.is_some());
    }

    #[test]
    fn validates_shell_args_rejects_sudo() {
        let result = validate_shell_args("defaults", &["sudo".to_string(), "rm".to_string()]);
        assert!(result.is_some());
    }

    #[test]
    fn validates_shell_args_rejects_blocked_domain() {
        let result = validate_shell_args(
            "defaults",
            &["write".to_string(), "com.apple.security".to_string(), "foo".to_string()],
        );
        assert!(result.is_some());
    }

    #[test]
    fn validates_shell_args_allows_safe_command() {
        let result = validate_shell_args(
            "defaults",
            &["read".to_string(), "com.apple.dock".to_string()],
        );
        assert!(result.is_none());
    }
}
```

- [ ] **Step 3: Create helpers.rs -- Port v1 tool_helpers.rs utilities**

Port `truncate_tool_response`, `parse_modifiers`, `capture_post_state`, `is_state_changing_tool` from `crates/aura-daemon/src/tool_helpers.rs`. These are exact ports -- read the original file and replicate the logic.

```rust
// crates/aura-tools/src/helpers.rs
use std::time::Duration;

/// Maximum characters a tool response may contain before truncation.
pub const MAX_TOOL_RESPONSE_CHARS: usize = 8000;

/// Per-tool-type settle delay before polling for screen changes.
pub fn settle_delay_for_tool(name: &str) -> Duration {
    match name {
        "type_text" | "press_key" | "move_mouse" | "select_text" | "paste_text" => {
            Duration::from_millis(30)
        }
        "scroll" | "drag" => Duration::from_millis(50),
        "click" | "click_element" | "context_menu_click" => Duration::from_millis(100),
        "activate_app" | "click_menu_item" => Duration::from_millis(150),
        "run_applescript" | "run_shell_command" => Duration::from_millis(200),
        _ => Duration::from_millis(150),
    }
}

/// Returns true if the tool changes screen state and should get verification.
pub fn is_state_changing_tool(name: &str) -> bool {
    matches!(
        name,
        "click"
            | "type_text"
            | "press_key"
            | "key_state"
            | "scroll"
            | "drag"
            | "click_element"
            | "activate_app"
            | "click_menu_item"
            | "context_menu_click"
            | "run_applescript"
            | "select_text"
            | "run_shell_command"
            | "paste_text"
    )
}

/// Parse optional modifier keys from tool args JSON.
pub fn parse_modifiers(args: &serde_json::Value) -> Vec<String> {
    args.get("modifiers")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Truncate a string to max_chars, respecting UTF-8 char boundaries.
pub fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        return s.to_string();
    }
    let mut end = max_chars.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...[truncated]", &s[..end])
}

/// Truncate a tool response to stay within context budget.
/// Exact port of v1's truncate_tool_response from tool_helpers.rs.
pub fn truncate_tool_response(response: &mut serde_json::Value) {
    // Trim elements list in get_screen_context
    if let Some(arr) = response
        .get_mut("context")
        .and_then(|c| c.get_mut("elements"))
        .and_then(|e| e.as_array_mut())
    {
        if arr.len() > 30 {
            let original_count = arr.len();
            arr.truncate(30);
            arr.push(serde_json::json!({"truncated": true, "original_count": original_count}));
        }
        for el in arr.iter_mut().skip(1) {
            if let Some(obj) = el.as_object_mut() {
                obj.remove("bounds");
            }
        }
    }

    let serialized_len = response.to_string().len();
    if serialized_len > MAX_TOOL_RESPONSE_CHARS {
        if let Some(obj) = response.as_object_mut() {
            let success = obj.get("success").cloned();
            let verified = obj.get("verified").cloned();
            let error = obj.get("error").and_then(|v| v.as_str()).map(|s| truncate_str(s, 500));
            let stdout = obj.get("stdout").and_then(|v| v.as_str()).map(|s| truncate_str(s, 500));
            let warning = obj.get("warning").and_then(|v| v.as_str()).map(|s| truncate_str(s, 200));
            let post_state = obj.get("post_state").cloned();
            obj.clear();
            if let Some(s) = success { obj.insert("success".to_string(), s); }
            if let Some(v) = verified { obj.insert("verified".to_string(), v); }
            if let Some(e) = error { obj.insert("error".to_string(), serde_json::Value::String(e)); }
            if let Some(o) = stdout { obj.insert("stdout".to_string(), serde_json::Value::String(o)); }
            if let Some(w) = warning { obj.insert("warning".to_string(), serde_json::Value::String(w)); }
            if let Some(ps) = post_state { obj.insert("post_state".to_string(), ps); }
            obj.insert(
                "truncated".to_string(),
                serde_json::json!(format!("Response truncated from {serialized_len} chars")),
            );
        }
    }
}

/// Capture post-action state: frontmost app, focused element.
/// Must be called from a blocking thread (AX FFI is synchronous).
pub fn capture_post_state() -> serde_json::Value {
    let frontmost_app = aura_screen::macos::get_frontmost_app().unwrap_or_default();
    let focused = aura_screen::accessibility::get_focused_element();
    let focused_json = focused
        .as_ref()
        .map(|el| {
            serde_json::json!({
                "role": el.role,
                "label": el.label,
                "value": el.value,
                "focused": el.focused,
            })
        })
        .unwrap_or(serde_json::json!(null));

    serde_json::json!({
        "frontmost_app": frontmost_app,
        "focused_element": focused_json,
    })
}
```

- [ ] **Step 4: Create lib.rs with execute_tool entry point**

```rust
// crates/aura-tools/src/lib.rs
pub mod safety;
pub mod helpers;
pub mod mouse;
pub mod keyboard;
pub mod apps;
pub mod text;
pub mod scripts;
pub mod memory;
pub mod context;
pub mod system;

use aura_bridge::script::ScriptExecutor;
use aura_navigator::FrameDims;

/// Main tool execution entry point.
///
/// Routes a Gemini tool call to the appropriate handler module.
/// Safety gates are checked before execution.
/// Returns a JSON result to send back to Gemini.
pub async fn execute_tool(
    name: &str,
    args: &serde_json::Value,
    executor: &ScriptExecutor,
    dims: FrameDims,
) -> serde_json::Value {
    // Check accessibility permission for input tools
    let input_tools = [
        "move_mouse", "click", "type_text", "press_key", "scroll",
        "drag", "key_state", "select_text", "paste_text",
    ];
    if input_tools.contains(&name)
        && !aura_input::accessibility::check_accessibility(false)
    {
        return serde_json::json!({
            "success": false,
            "error": "Accessibility permission is not granted. \
                      Enable in System Settings > Privacy & Security > Accessibility.",
            "error_kind": "accessibility_denied",
        });
    }

    match name {
        // Mouse tools
        "click" => mouse::handle_click(args, dims).await,
        "click_element" => mouse::handle_click_element(args).await,
        "context_menu_click" => mouse::handle_context_menu_click(args, dims).await,
        "move_mouse" => mouse::handle_move_mouse(args, dims).await,
        "drag" => mouse::handle_drag(args, dims).await,
        "scroll" => mouse::handle_scroll(args).await,
        // Keyboard tools
        "type_text" => keyboard::handle_type_text(args).await,
        "press_key" => keyboard::handle_press_key(args).await,
        "key_state" => keyboard::handle_key_state(args).await,
        "select_text" => keyboard::handle_select_text(args, dims).await,
        // App tools
        "activate_app" => apps::handle_activate_app(args, executor).await,
        "click_menu_item" => apps::handle_click_menu_item(args, executor).await,
        // Text tools
        "write_clipboard" => text::handle_write_clipboard(args).await,
        "paste_text" => text::handle_paste_text(args).await,
        // Script tools
        "run_applescript" => scripts::handle_run_applescript(args, executor).await,
        "run_shell_command" => scripts::handle_run_shell_command(args).await,
        // Memory tools
        "save_memory" => memory::handle_save_memory(args).await,
        "recall_memory" => memory::handle_recall_memory(args).await,
        // Context tools
        "get_screen_context" => context::handle_get_screen_context(dims).await,
        // System tools
        "shutdown_aura" => system::handle_shutdown().await,
        // Unknown tool
        _ => serde_json::json!({
            "success": false,
            "error": format!("Unknown tool: {name}"),
        }),
    }
}
```

- [ ] **Step 5: Create stub modules for each tool category**

Each module file (`mouse.rs`, `keyboard.rs`, `apps.rs`, `text.rs`, `scripts.rs`, `memory.rs`, `context.rs`, `system.rs`) should contain the handler functions. The implementation logic is a direct port from `crates/aura-daemon/src/tools.rs`.

For example, `text.rs` (the new `paste_text` tool):

```rust
// crates/aura-tools/src/text.rs

/// Handle write_clipboard tool -- write text to system clipboard.
pub async fn handle_write_clipboard(args: &serde_json::Value) -> serde_json::Value {
    let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
    match aura_screen::macos::set_clipboard(text) {
        Ok(()) => serde_json::json!({"success": true}),
        Err(e) => serde_json::json!({"success": false, "error": format!("{e}")}),
    }
}

/// Handle paste_text tool -- NSPasteboard snapshot, set text, Cmd+V, restore.
/// Unicode-safe. Faster than character-by-character type_text for large blocks.
pub async fn handle_paste_text(args: &serde_json::Value) -> serde_json::Value {
    let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if text.is_empty() {
        return serde_json::json!({"success": false, "error": "No text provided"});
    }

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        // Snapshot current clipboard
        let prev = aura_screen::macos::get_clipboard();

        // Set clipboard to new text
        aura_screen::macos::set_clipboard(&text)?;

        // Cmd+V to paste
        let v_key = aura_input::keyboard::keycode_from_name("v")
            .ok_or_else(|| anyhow::anyhow!("Failed to get keycode for 'v'"))?;
        aura_input::keyboard::press_key(v_key, &["cmd"])?;

        // Brief delay for paste to land
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Restore previous clipboard
        if let Some(ref prev_text) = prev {
            let _ = aura_screen::macos::set_clipboard(prev_text);
        } else {
            let _ = aura_screen::macos::set_clipboard("");
        }

        Ok(())
    }).await;

    match result {
        Ok(Ok(())) => serde_json::json!({
            "success": true,
            "method": "clipboard_paste",
        }),
        Ok(Err(e)) => serde_json::json!({
            "success": false,
            "error": format!("{e}"),
        }),
        Err(e) => serde_json::json!({
            "success": false,
            "error": format!("Task panicked: {e}"),
        }),
    }
}
```

Each remaining module (`mouse.rs`, `keyboard.rs`, `apps.rs`, `scripts.rs`, `memory.rs`, `context.rs`, `system.rs`) should port the corresponding handler from `crates/aura-daemon/src/tools.rs`. The port is mechanical -- read the v1 handler, extract it into the module function, update imports. The v1 code is in `crates/aura-daemon/src/tools.rs` lines 85-800+.

Key patterns to preserve from v1:
- `run_with_pid_fallback()` for all input tools (PID-targeted first, HID fallback)
- `run_input_blocking()` for synchronous FFI calls on blocking threads
- `click_element_inner()` 3-tier fallback: AXPress -> PID click -> HID click
- Terminal app blocking for `activate_app`
- Shell metacharacter rejection for `run_shell_command`
- Automation permission pre-check for `run_applescript`
- Input clamping (scroll +/-1000, text 10K chars)

- [ ] **Step 6: Build and test**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo check -p aura-tools 2>&1
cargo test -p aura-tools 2>&1 | tail -20
```
Expected: Compilation succeeds. Safety gate tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/aura-tools/ Cargo.toml
git commit -m "feat(v2): create aura-tools crate with 20+ tool handlers and safety gates

Port tool handlers from v1 aura-daemon/src/tools.rs into dedicated crate.
Organized by category: mouse, keyboard, apps, text, scripts, memory,
context, system. Safety gates: blocked terminal apps, shell metachar
rejection, obfuscation detection, JXA blocking, input clamping, script
timeouts. New paste_text tool uses NSPasteboard snapshot/restore."
```

---

## Task 6: Update Gemini Tool Declarations for v2

**Files:**
- Modify: `crates/aura-gemini/src/tools.rs`

Add the `paste_text` tool declaration and update existing descriptions for v2 (Navigator awareness, screen basis IDs).

- [ ] **Step 1: Add paste_text tool declaration**

In `crates/aura-gemini/src/tools.rs`, add to the `function_declarations` vec inside `build_tool_declarations()`:

```rust
FunctionDeclaration {
    name: "paste_text".into(),
    description: "Paste text into the focused element via the clipboard. More reliable than \
        type_text for large blocks, Unicode text, or password fields. Uses NSPasteboard: \
        snapshots current clipboard, sets text, Cmd+V, restores previous clipboard. \
        Invoke this tool only after you have the text to paste and a target field is focused."
        .into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "text": {
                "type": "string",
                "description": "The text to paste"
            }
        },
        "required": ["text"]
    }),
    behavior: Some("NON_BLOCKING".into()),
},
```

- [ ] **Step 2: Update test assertion for tool count**

Update the test in `tools.rs` that asserts `decls.len() == 20` to `decls.len() == 21`, and add `"paste_text"` to the expected names list.

- [ ] **Step 3: Run tests**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test -p aura-gemini -- tools 2>&1
```
Expected: All tool declaration tests pass with 21 function declarations.

- [ ] **Step 4: Commit**

```bash
git add crates/aura-gemini/src/tools.rs
git commit -m "feat(v2): add paste_text tool declaration for Gemini

paste_text uses NSPasteboard snapshot/restore for reliable Unicode
text insertion. More reliable than type_text for large blocks and
password fields."
```

---

## Task 7: Integrate Navigator into Daemon Orchestrator

**Files:**
- Modify: `crates/aura-daemon/Cargo.toml`
- Modify: `crates/aura-daemon/src/orchestrator.rs`

Wire the Navigator and aura-tools into the daemon's tool dispatch pipeline.

- [ ] **Step 1: Add dependencies**

In `crates/aura-daemon/Cargo.toml`, add:
```toml
aura-navigator = { path = "../aura-navigator" }
aura-tools = { path = "../aura-tools" }
```

- [ ] **Step 2: Update tool dispatch in orchestrator**

In the Gemini event handler where `GeminiEvent::ToolCall` is processed, replace the v1 direct tool execution with Navigator-mediated execution:

```rust
GeminiEvent::ToolCall { id, name, args } => {
    let tool_name = name.clone();
    let tool_args = args.clone();
    let tool_id = id.clone();
    let basis_id = current_screen_basis_id.lock().unwrap().clone();
    let dims = aura_navigator::FrameDims {
        img_w: last_frame_width.load(Ordering::Relaxed),
        img_h: last_frame_height.load(Ordering::Relaxed),
        logical_w: last_logical_width.load(Ordering::Relaxed),
        logical_h: last_logical_height.load(Ordering::Relaxed),
    };

    // Build a Navigator plan
    let plan = aura_navigator::build_plan(&tool_name, &tool_args, &basis_id, dims);

    let result = if plan.steps.is_empty() {
        // Direct-execution tool (scripts, memory, context) -- bypass Navigator
        let executor = executor.clone();
        aura_tools::execute_tool(&tool_name, &tool_args, &executor, dims).await
    } else {
        // Navigator-mediated execution with basis ID verification
        let screen_state = aura_navigator::executor::ScreenState {
            current_basis_id: current_screen_basis_id.clone(),
        };
        let plan_result = aura_navigator::executor::execute_plan(&plan, &screen_state).await;
        plan_result.tool_result
    };

    // Truncate and send result back to Gemini
    let mut response = result;
    aura_tools::helpers::truncate_tool_response(&mut response);

    // Emit tool status to IPC
    let _ = ipc_tx.send(DaemonEvent::ToolStatus {
        name: tool_name.clone(),
        status: if response.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
            ToolRunStatus::Completed
        } else {
            ToolRunStatus::Failed
        },
        output: Some(response.to_string()),
        summary: None,
    });

    // Send tool response to Gemini
    if let Err(e) = session.send_tool_response(&tool_id, &response) {
        tracing::warn!("Failed to send tool response: {e}");
    }
}
```

- [ ] **Step 3: Add frame dimension tracking**

Near the top of the orchestrator, add atomic frame dimension tracking that gets updated when VideoFrame arrives:

```rust
let last_frame_width = Arc::new(AtomicU32::new(1920));
let last_frame_height = Arc::new(AtomicU32::new(1080));
let last_logical_width = Arc::new(AtomicU32::new(1440));
let last_logical_height = Arc::new(AtomicU32::new(900));
```

In the VideoFrame handler, update these from the frame metadata (the Swift side sends fixed 1920px-width frames, but logical dimensions come from display info).

- [ ] **Step 4: Build and test**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo build -p aura-daemon 2>&1 | tail -10
cargo test -p aura-daemon 2>&1 | tail -20
cargo clippy -p aura-daemon -- -D warnings 2>&1 | tail -10
```
Expected: Compiles, tests pass, no clippy warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/aura-daemon/Cargo.toml crates/aura-daemon/src/orchestrator.rs
git commit -m "feat(v2): integrate Navigator into daemon tool dispatch

Tool calls from Gemini now go through Navigator planning. Tools with
coordinate-based actions get screen basis ID verification and adaptive
post-action polling. Direct-execution tools (scripts, memory) bypass
Navigator for zero overhead."
```

---

## Task 8: Accessibility Tree Integration for Navigator

**Files:**
- No new files (uses existing `crates/aura-screen/src/accessibility.rs`)

The accessibility tree stays in Rust (not moved to Swift). It feeds the Navigator for `click_element` targeting and the `get_screen_context` tool. Verify all existing AX functions compile and are accessible from `aura-tools` and `aura-navigator`.

- [ ] **Step 1: Verify AX tree functions are accessible**

Confirm these public functions exist in `crates/aura-screen/src/accessibility.rs` and are usable from `aura-tools`:

```rust
// These should all be pub and importable:
aura_screen::accessibility::get_focused_app_elements()  // Vec<UIElement>
aura_screen::accessibility::get_focused_element()       // Option<UIElement>
aura_screen::accessibility::ax_perform_action_nth()     // AXActionResult
aura_screen::accessibility::ax_set_focused()            // AXActionResult
aura_screen::accessibility::find_elements()             // Vec<&UIElement>
aura_screen::accessibility::is_focused_element_secure() // bool
aura_screen::accessibility::scroll_to_visible_and_get_element() // Option<UIElement>
```

Run a check:
```bash
cd /Users/abdullahiabdi/Developer/aura && cargo doc -p aura-screen --no-deps 2>&1 | tail -5
```

- [ ] **Step 2: Write an integration test**

```rust
// crates/aura-tools/tests/ax_integration.rs
#[test]
fn accessibility_functions_compile() {
    // Verify all AX functions are importable and have the expected signatures.
    // Actual execution requires Accessibility permission (skip in CI).
    let _fn1: fn() -> Vec<aura_screen::context::UIElement> =
        aura_screen::accessibility::get_focused_app_elements;
    let _fn2: fn() -> Option<aura_screen::context::UIElement> =
        aura_screen::accessibility::get_focused_element;
    let _fn3: fn() -> bool =
        aura_screen::accessibility::is_focused_element_secure;
}
```

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo test -p aura-tools -- ax_integration 2>&1
```

- [ ] **Step 3: Commit**

```bash
git add crates/aura-tools/tests/
git commit -m "test(v2): verify accessibility tree functions accessible from aura-tools

Integration test confirms AX tree walker functions (get_focused_element,
ax_perform_action_nth, etc.) are importable from aura-tools crate.
AX tree stays in Rust aura-screen crate, feeds Navigator for click_element."
```

---

## Task 9: End-to-End Integration Test

**Files:** No new files -- manual integration test.

- [ ] **Step 1: Build everything**

```bash
cd /Users/abdullahiabdi/Developer/aura && cargo build 2>&1 | tail -10
cargo test 2>&1 | tail -20
cargo clippy -- -D warnings 2>&1 | tail -10
```
Expected: Full workspace compiles. All tests pass. No clippy warnings.

```bash
cd /Users/abdullahiabdi/Developer/aura && xcodebuild -project AuraApp/AuraApp.xcodeproj -scheme AuraApp -configuration Debug build 2>&1 | tail -5
```
Expected: BUILD SUCCEEDED.

- [ ] **Step 2: Run smoke test**

1. Launch AuraApp
2. Verify screen capture frames appear in daemon logs (`tracing::info` for VideoFrame receipt)
3. Speak "What do you see on my screen?" -- Gemini should describe the visible content
4. Speak "Click the [visible button/element]" -- Gemini should call click/click_element, Navigator should execute with basis ID verification
5. Verify adaptive polling: daemon logs should show hash polling at 100ms intervals, not fixed sleep
6. Verify cursor animation: cursor should glide smoothly to click targets

- [ ] **Step 3: Test safety gates**

1. Speak "Open Terminal" -- should be blocked (terminal app safety gate)
2. Speak "Run 'ls | grep foo'" -- shell metachar should be rejected
3. Verify all 8 safety gates from the spec are active

- [ ] **Step 4: Commit any fixes**

```bash
git add -A
git commit -m "fix(v2): address issues found in Phase 3 integration test"
```

---

## Dependency Graph

Tasks can be parallelized where there are no dependencies:

```
Task 1 (ScreenCapture.swift)
    |
    v
Task 2 (VideoFrame IPC routing) <--- depends on Task 1
    |
    v
Task 7 (Navigator integration) <--- depends on Tasks 3, 4, 5
    ^       ^       ^
    |       |       |
Task 3  Task 4  Task 5
(types) (exec)  (tools)
    ^
    |
Task 6 (Gemini tool decls) -- independent, can run in parallel
Task 8 (AX integration) -- independent, can run in parallel

Task 9 (Integration test) <--- depends on all above
```

**Parallel batches:**
- **Batch 1:** Tasks 1, 3, 5, 6, 8 (all independent)
- **Batch 2:** Tasks 2, 4 (depend on Batch 1)
- **Batch 3:** Task 7 (depends on Batch 2)
- **Batch 4:** Task 9 (depends on all)

---

## Key Design Decisions

1. **Screen capture in Swift, AX tree in Rust.** ScreenCaptureKit requires Swift (Apple framework). The AX tree walker uses raw FFI that works fine in Rust and is needed by `aura-tools` (click_element). No reason to move it.

2. **Adaptive polling, not fixed sleep.** The spec explicitly calls this out: "Poll screen hash every 100ms, stable = hash unchanged over 2 consecutive checks, timeout after 3 seconds." This replaces v1's `150ms settle + 10x200ms poll` pattern with a simpler, faster approach.

3. **Screen basis ID is a UUID per frame.** The Swift ScreenCapture assigns a UUID to each captured frame. The daemon stores the latest ID. The Navigator checks it before executing coordinate-based actions. Non-coordinate actions (keyboard, AX press) proceed with a warning.

4. **Navigator wraps tools, doesn't replace them.** The Navigator adds planning, confidence scoring, and verification on top of the existing tool handlers. Tools that don't need Navigator mediation (scripts, memory, context) bypass it entirely.

5. **aura-tools is a clean extraction from aura-daemon.** The v1 tools.rs (63KB) was too large. Splitting by category (mouse, keyboard, apps, text, scripts, memory, context, system) makes each file manageable and testable.

6. **paste_text is the only new tool.** The spec calls for it. All other tools are ports from v1. The clipboard snapshot/restore pattern already exists in v1's type_text secure field handler -- paste_text makes it a first-class tool.
