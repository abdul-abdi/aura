# Aura Security Model

Aura is a voice-first AI that controls your Mac. It executes AppleScript, posts synthetic keyboard and mouse events, reads your screen via `CGDisplay`, and captures microphone audio. Security is not optional -- it is load-bearing.

This document describes the threat model, the defense-in-depth layers that mitigate those threats, the macOS permission model Aura depends on, and the honest gaps that remain.

---

## 1. Threat Model

### What Aura Can Do

| Capability | Mechanism | Scope |
|---|---|---|
| Execute scripts | `osascript` (AppleScript / JXA) | Any macOS app or shell command reachable via `do shell script` |
| Synthetic input | `CGEvent.post()` via `aura-input` | Move mouse, click, type text, press keys, scroll, drag |
| Read screen | `CGDisplay::image()` via `aura-screen` | Full-screen capture at 1 fps, retina resolution |
| Capture audio | `cpal` via `aura-voice` | Default input device, 16 kHz mono PCM |
| Network | WebSocket to Gemini Live API | Bidirectional audio + tool calls over `wss://` |

### Attack Vectors

**Prompt injection.** Gemini receives user speech, screen context, and tool results as input. A malicious document visible on screen, a crafted clipboard payload, or an adversarial audio sample could instruct Gemini to call destructive tools.

**Malicious tool calls.** If an attacker gains control of the Gemini session (via prompt injection or a compromised proxy), they can issue arbitrary `run_applescript`, `type_text`, or input tool calls.

**Exfiltration.** Screen captures and clipboard contents are sent to Gemini. A prompt injection could instruct the model to type sensitive data into a browser field or run a script that posts it to an attacker-controlled URL.

**Lateral movement.** AppleScript can control any application the user has open. A compromised session could open Terminal, type shell commands, or use `do shell script` to execute arbitrary binaries.

**Speaker bleed-through.** Audio played through laptop speakers can re-enter the microphone, potentially causing the model to act on its own output or on attacker-controlled audio from a web page.

---

## 2. Defense-in-Depth Layers

### Layer 1: Script Blocklist

**Source:** `crates/aura-bridge/src/script.rs`

All script content is checked against blocklists *before* execution. This applies to the full script body, not just `do shell script` blocks.

**Blocked shell patterns** (`BLOCKED_SHELL_PATTERNS`):

| Pattern | Threat |
|---|---|
| `rm -rf`, `rm -r` | Recursive file deletion |
| `sudo` | Privilege escalation |
| `mkfs` | Filesystem format |
| `dd if=` | Raw disk write |
| `chmod 777` | Permission escalation |
| `:(){ :\|:` | Fork bomb |
| `> /dev/sd` | Raw device write |
| `unlink ` | File deletion (trailing space prevents matching "unlinked") |
| `diskutil erase` | Disk erase |

**Blocked JXA patterns** (`BLOCKED_JXA_PATTERNS`, checked case-insensitively):

| Pattern | Threat |
|---|---|
| `$.system` | Shell escape from JXA |
| `objc.import` | Arbitrary Objective-C bridge access |
| `.doscript(` | `Terminal.doScript()` -- deprecated, dangerous shell execution |

**Obfuscation detection** (`OBFUSCATED_ATOM_PATTERNS`):

Multi-atom pattern matching catches dangerous commands fragmented across string concatenation or variable assignments. Each pattern is a set of atoms that must ALL appear as standalone tokens in the script. Standalone matching uses word-boundary detection to avoid false positives (e.g., `"rm"` in `"rm_notes.txt"` is not standalone because of the adjacent underscore).

Detected fragmented patterns:
- `rm` + `-rf` -- fragmented `rm -rf`
- `dd` + `if=` -- fragmented `dd if=`
- `chmod` + `777` -- fragmented `chmod 777`

**Script execution constraints:**
- Maximum timeout: 60 seconds (`MAX_TIMEOUT_SECS`), enforced via `Child::kill()` + `child.wait()` (reaps zombie)
- Maximum output: 10,240 bytes (`MAX_OUTPUT_BYTES`), truncated at a valid UTF-8 char boundary
- Execution: `tokio::task::spawn_blocking` with a polling loop -- timeout actually kills the `osascript` child process rather than just dropping the future

### Layer 2: Input Validation

**Source:** `crates/aura-daemon/src/main.rs` (tool dispatch)

All tool call parameters are validated and clamped before execution:

| Tool | Constraint | Constant |
|---|---|---|
| `type_text` | Max 10,000 characters (char-aware, not byte count) | `TYPE_TEXT_MAX_CHARS` |
| `click` | Click count clamped to 1..=3 | `CLICK_COUNT_MAX` |
| `scroll` | dx and dy clamped to -1000..=1000 | `SCROLL_MAX` |
| `run_applescript` | Timeout clamped to max 60 seconds | `MAX_TIMEOUT_SECS` |

Clamping is applied at the `i64` level before casting to `i32` to prevent integer wrapping.

### Layer 3: Permission Gating

**Source:** `crates/aura-input/src/accessibility.rs`, `crates/aura-daemon/src/main.rs`

Accessibility permission is checked **before every input tool call**. The six guarded tools are: `move_mouse`, `click`, `type_text`, `press_key`, `scroll`, `drag`.

```rust
"move_mouse" | "click" | "type_text" | "press_key" | "scroll" | "drag"
    if !aura_input::accessibility::check_accessibility(false) =>
{
    // Return honest failure to Gemini
}
```

Without Accessibility permission, `CGEvent.post()` silently drops events -- the OS gives no error. Aura's pre-check ensures Gemini receives an honest failure message (`"Accessibility permission not granted"`) instead of a silent no-op that appears to succeed.

Screen Recording permission is checked at startup via `CGPreflightScreenCaptureAccess()` (a silent, read-only check). If denied, the daemon sets the menu bar to red and displays a status message. Additionally, captured frames are checked for censorship (uniform pixel variance) at runtime, since macOS returns valid but blanked-out images without the permission.

### Layer 4: Destructive Action Guardrail

**Source:** `crates/aura-daemon/src/main.rs` (`DESTRUCTIVE_ACTION_GUARDRAIL`)

The system prompt includes an explicit safety directive injected at runtime:

> Before deleting files, emptying trash, quitting unsaved apps, reformatting drives, or any action that permanently destroys data, ALWAYS confirm with the user first.

Non-destructive actions (opening apps, clicking, typing, moving files) do not require confirmation. This is a behavioral guardrail enforced by the model, not by code -- it depends on Gemini following instructions.

### Layer 5: Audio Energy Gating

**Source:** `crates/aura-daemon/src/main.rs` (mic bridge)

Speaker bleed-through from laptop speakers can re-enter the microphone and trigger unintended barge-in. Aura mitigates this with an adaptive RMS energy gate:

- **Default threshold:** 0.04 RMS (`BARGE_IN_ENERGY_THRESHOLD_DEFAULT`)
- **Calibration:** First 500ms of audio (~100 chunks at ~5ms each) is collected for ambient noise measurement
- **Algorithm:** `mean + 3 * stddev` of the collected RMS samples
- **Clamped range:** [0.02, 0.15] (`CALIBRATION_THRESHOLD_MIN` / `CALIBRATION_THRESHOLD_MAX`)
  - Floor of 0.02 prevents any noise from triggering barge-in
  - Ceiling of 0.15 prevents the gate from suppressing real speech
- **Effect:** While Aura is speaking (audio playback active), mic frames below the threshold are dropped, preventing the model from interrupting itself

---

## 3. macOS Permission Model

Aura requires three macOS permissions. The SwiftUI app (`AuraApp/Sources/PermissionChecker.swift`) handles onboarding; the Rust daemon inherits grants via macOS's "responsible process" mechanism.

### Microphone

- **API:** `AVCaptureDevice.requestAccess(for: .audio)`
- **UX:** Native inline grant/deny dialog -- user clicks Allow or Don't Allow
- **Failure mode:** `cpal` fails to open the input stream; daemon logs a warning and sets menu bar to red
- **Check:** `AVCaptureDevice.authorizationStatus(for: .audio) == .authorized`

### Screen Recording

- **API:** `CGRequestScreenCaptureAccess()` (macOS 15+)
- **UX:** Registers Aura in System Settings and shows a one-time popup directing the user to Privacy & Security > Screen Recording. User must manually toggle the switch.
- **Failure mode:** `CGDisplay::image()` returns a valid image but with blanked-out window contents (only wallpaper and window chrome visible). Aura detects this via pixel variance analysis.
- **Check:** `CGPreflightScreenCaptureAccess()` (silent, no popup)

### Accessibility

- **API:** `AXIsProcessTrustedWithOptions` with `AXTrustedCheckOptionPrompt`
- **UX:** Registers Aura in System Settings and shows a one-time popup directing the user to Privacy & Security > Accessibility. User must manually toggle the switch.
- **Failure mode:** `CGEvent.post()` silently drops all synthetic input events. Aura pre-checks before every input tool call and returns an explicit error to Gemini.
- **Check:** `AXIsProcessTrustedWithOptions` with `prompt: false`

### Responsible Process Inheritance

The daemon process (`aura-daemon`) is launched by the SwiftUI app (`Aura.app`). macOS TCC (Transparency, Consent, and Control) tracks permissions by the "responsible process" -- the app that spawned the daemon. This means:

- Permissions granted to `Aura.app` during onboarding are inherited by the daemon
- Running the daemon standalone (outside the app bundle) requires granting permissions to Terminal or the parent process

---

## 4. What's NOT Sandboxed (Honest Assessment)

### AppleScript Runs Through osascript

The blocklist (`BLOCKED_SHELL_PATTERNS`, `BLOCKED_JXA_PATTERNS`, `OBFUSCATED_ATOM_PATTERNS`) is **defense-in-depth, not a sandbox**. `osascript` is invoked as a child process with no `sandbox-exec` profile, no filesystem restrictions, and no network filtering. A sufficiently creative script that avoids all blocked patterns can:

- Read and write arbitrary files the user owns
- Open network connections
- Launch applications
- Send keystrokes to other apps via `System Events`

The blocklist catches known dangerous patterns but cannot guarantee coverage of all possible destructive commands.

### CGEvent Posts Are Not Reversible

Once a synthetic mouse click or keystroke is posted via `CGEvent.post()`, it is indistinguishable from real hardware input. There is no undo mechanism. A malicious tool call that clicks "Delete" or types a destructive command into Terminal cannot be rolled back.

### Network Access Is Unrestricted

The daemon maintains an open WebSocket to Gemini's API (or an optional proxy). There are no restrictions on outbound network connections from the daemon process or from scripts executed via `osascript`. A compromised script could exfiltrate data to an arbitrary endpoint.

### No Process Isolation Between Daemon and Tools

All tool execution (script execution, input posting, screen capture) happens in the same process as the daemon. There is no privilege separation -- a panic or memory corruption in tool handling affects the entire daemon.

### Model Behavioral Guardrails Are Not Enforceable

The destructive action confirmation (Layer 4) depends entirely on Gemini following its system prompt. A sufficiently strong prompt injection could override this instruction. The blocklist (Layer 1) provides a hard safety floor, but only for the specific patterns it covers.

### Screen Captures Contain Sensitive Data

Screen captures are sent to Gemini at 1 fps. Anything visible on screen -- passwords in a password manager, private messages, financial data -- is transmitted. There is no content filtering or redaction of captured frames.

---

## Summary

| Layer | Type | Enforced By | Bypassable? |
|---|---|---|---|
| Script blocklist | Hard block | Code (`check_dangerous()`) | Only by patterns not in the list |
| Input validation | Hard clamp | Code (daemon tool dispatch) | No -- values are clamped before use |
| Permission gating | Hard block | Code + macOS TCC | No -- OS enforces permission checks |
| Destructive action confirmation | Soft guardrail | Model behavior (system prompt) | Yes -- via prompt injection |
| Audio energy gating | Soft filter | Code (RMS threshold) | Partially -- loud bleed-through above threshold passes |

The security model is designed around the principle that **hard code-level blocks catch known dangerous patterns**, while **behavioral guardrails handle the long tail of ambiguous actions**. The honest gap is between these two: actions that are destructive but don't match any blocked pattern and that the model is successfully instructed to execute without confirmation.
