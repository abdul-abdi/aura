# Aura v3 — Design Spec

**Date:** 2026-04-29
**Status:** Draft, pending user review
**Supersedes:** v1 + v2 (clean-slate rewrite — no code or architecture inherited)
**Author:** Brainstormed with abdullahi, pressure-tested by `/carmack` + `/hickey`, platform research dated April 2026

---

## 1. The product, in one paragraph

Aura is a local-only macOS voice assistant. You speak, it streams your voice and your screen to Gemini's Live API, and Gemini responds either by speaking back or by directly controlling your Mac — clicking, typing, scrolling, sending mail, running scripts. It remembers what matters across sessions, learns from your corrections, and stays out of your way until you want it. Single Swift app, one binary, ~3K lines, no servers, no cloud infra of any kind beyond direct API calls to `generativelanguage.googleapis.com`.

## 2. Why a clean rebuild

v1 and v2 reached working voice round-trip but accumulated architectural debt: two languages (Rust + Swift), 10 crates, deployable Cloud Run services for proxy and memory, Firestore for sync, ~20K LOC. The user wants the assistant, not the infrastructure. Stripping to a single Swift app removes:

- Rust/Swift FFI boundary
- Cloud Run proxy + memory-agent (Python FastAPI)
- Firestore + Firebase auth
- Cross-language build pipeline
- All deployment/devops surface area

What's preserved as engineering DNA — not as code:

- The VibeCat-pattern hardware-format-tap mic capture (the only voice fix worth re-deriving)
- The "verify after click" instinct (re-implemented as per-tool ground-truth probes)
- The 2 FPS perceptual-hash screen capture rate (re-implemented in `ScreenCaptureKit`)
- The hard safety gates on shell/AppleScript escape hatches

## 3. Hard constraints

- **Local-only.** No Cloud Run, no Firestore, no proxy, no Python, no remote infra. Only network egress: `generativelanguage.googleapis.com` (Live WebSocket + Flash REST).
- **Single Swift target.** SwiftPM. No FFI. No code generation. No external daemon process.
- **≤ 3,500 LOC.** If we exceed this, something is wrong.
- **No analytics, no telemetry, no auto-update.** First version ships as a notarized DMG; the user updates by re-downloading.
- **Keychain for the API key.** Not a config file. (v2 stored it in `config.toml` — fix.)
- **macOS 14+** (the LOC budget assumes modern APIs; 13 support is not worth the bloat).

## 4. The single architectural idea

**Everything that persists is one append-only event log. Everything else is a query over the log.**

This is not aesthetic. It's the lever that collapses what would otherwise be six independent subsystems (episodic memory, semantic memory, behavioral memory, procedural memory, transcript history, observability) into one. The log is the only mutable thing in the persistent layer. "Forget" is an append, not a delete. "Recall" is a select. "Self-learning" is choosing which queries we trust enough to inject.

```
events(
  id            INTEGER PRIMARY KEY,
  ts            INTEGER NOT NULL,           -- unix ms
  kind          TEXT NOT NULL,              -- enum, see §6.1
  payload       TEXT NOT NULL,              -- JSON
  session_id    TEXT,
  embedding     BLOB,                       -- 768-dim f32, optional
  tombstone_for INTEGER REFERENCES events(id)
)
CREATE INDEX events_ts ON events(ts);
CREATE INDEX events_kind ON events(kind);
CREATE VIRTUAL TABLE events_fts USING fts5(payload, content='events', content_rowid='id');
```

Memory layers (§6) are _named queries_. Proactivity policies (§7) _subscribe_ to event kinds. The Live session (§5) _appends_ turn events. Tool execution (§8) _appends_ operation events and _reads_ observation events. Nothing else has its own storage.

## 5. The conversation, the transport, the session

A conversation is a value: an ordered sequence of `Turn`s. A `Turn` is one user-input + zero-or-more model-output events. A session is _the WebSocket the next turn happens to be travelling on_. Rotation is **not** a "handoff" — it is opening a new transport for the next turn. The conversation persists in the event log; the transport is incidental.

### 5.1 Live transport

- Endpoint: `wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent`
- Model: `gemini-2.5-flash-native-audio-preview-12-2025` (pinned — see §10.1)
- API version: `v1beta`
- Modalities: `[AUDIO]` in/out, `VIDEO` in
- Voice: `Kore` (configurable via `~/.config/aura/config.toml`)
- VAD: server-side, `START_SENSITIVITY_HIGH`, `END_SENSITIVITY_LOW`, 20ms prefix, 200ms silence
- Compression: sliding window, 500K-token cap (server-managed)
- Tools: ~25 declarations (§8)
- Affective dialog: enabled
- Proactive audio: **disabled at the API level** (we control proactivity ourselves — §7)

### 5.2 Hard rotation (Carmack: kill the soft handoff)

Native-audio sessions cap around 15 min audio-only and 2 min audio+video without compression. With sliding-window compression they can run indefinitely; rotation is still done proactively for context hygiene.

**Rotation procedure (executes at T = 9 min from session start, or on disconnect):**

1. Mic capture pauses (raw frames buffered into a small ring, dropped if buffer fills).
2. Active TTS playback drains to silence; if mid-utterance, finishes; if mid-tool-call, the tool completes and its result is appended to the event log but **not** sent to the dying socket.
3. WebSocket closes cleanly with goodbye frame.
4. **Audio cue**: 80ms 440Hz-blended sine, -24dB. The user knows.
5. New WebSocket connects. On `setupComplete`, send a **deterministic state digest**:
   - The last 2 user turns (raw text)
   - The active goal string (a single short sentence the system maintains)
   - The pending tool intent if any (`{operation, args}` only — never the result, which lives in the log)
6. Mic resumes.
7. Total gap target: **≤ 300ms** between cue end and mic resume.

No Flash summary. No "smart" handoff. No two sockets crossing streams. The cue is honest; the digest is byte-controlled.

### 5.3 Graceful end (the user's ask)

When session age exceeds 8 minutes AND user is not actively speaking AND no tool call is pending:

- Aura whispers (low volume, brief): _"Refreshing for the next stretch — give me a sec."_
- Hard rotation begins.

If user speaks during these conditions, defer to the next idle window. Never interrupt mid-turn. Never miss a hard deadline — at T = 9 min the rotation happens regardless.

### 5.4 Reconnect

Exponential backoff: 200ms → 400ms → 800ms → 1600ms → 3200ms, ±25% jitter. Max 5 attempts. After 5, surface to UI: "Aura's offline — check connection." User can manually retry. Session resumption handles are honored when offered by the server (token valid 2 hours), but we never depend on them — the state digest in §5.2 is enough on its own.

## 6. Memory — one log, four lenses

### 6.1 Event kinds

```
turn.user           — user said something (transcript)
turn.assistant      — model said something (transcript)
turn.tool_call      — model invoked a tool (operation, args)
turn.tool_result    — tool produced a result (output)
state.observation   — system observed a state change (after a tool ran)
correction          — user explicitly corrected behavior ("don't", "always")
fact                — semantic fact extracted by Flash
trigger.fired       — proactive trigger emitted
config.change       — user changed settings
session.start
session.end
tombstone           — invalidates an earlier event (for "forget")
```

### 6.2 Lenses (queries, not tables)

#### Episodic

```sql
SELECT * FROM events
WHERE ts > :since AND kind LIKE 'turn.%'
ORDER BY ts ASC
```

Used for: rebuilding the active conversation, audit logs, debugging.

#### Semantic

At session end, a Flash REST call extracts up to 10 facts from the session's turns:

```json
{
  "entity": "user",
  "predicate": "lives_in",
  "value": "Kenya",
  "confidence": 0.9
}
```

Each fact is appended as `kind='fact'` with a 768-dim NLContextualEmbedding embedding.

Recall:

```sql
SELECT id, payload FROM events
WHERE kind = 'fact' AND tombstone_for IS NULL
ORDER BY cosine(embedding, :query_embedding) DESC
LIMIT 8
```

Done off the audio path, at session start, results cached in memory for the session.

#### Behavioral (the corrections lens)

A small in-process pattern matcher scans every `turn.user` for negation/imperative correction patterns ("no", "stop", "don't", "always", "never", "use X not Y"). When matched, the user is offered a one-tap confirmation in the floating panel: _"Should I remember: `<rule>`?"_ On confirmation, append `kind='correction'`.

Active rules are concatenated into the system prompt at session start (under a "Things this user has asked you to remember:" header). Capped at 30 rules; oldest non-recently-used drop off after 90 days.

#### Procedural (gated, off by default)

Only enabled if user opts in. After every successful tool sequence (≥ 3 calls, no failures, no user interruption), append a candidate playbook event. After 3 occurrences with similar intent (cosine > 0.85 on the user's preceding utterance), the playbook becomes "trusted." Trusted playbooks are surfaced to Gemini as additional tools (`run_playbook(name)`).

If after 30 days of opt-in no playbook has been used twice, the feature is silently disabled and the user is told: _"I tried learning your routines and didn't find clean patterns — turning that off."_ Honest failure beats hidden creep.

### 6.3 Embeddings

- API: `NLContextualEmbedding` (English, 768-dim, on-device CoreML).
- `MLModelConfiguration.computeUnits = .cpuOnly` to avoid ANE contention with the live audio path. (Research finding — ANE throttling causes 200-400ms stalls when shared.)
- Embeddings computed at write time, never on the recall path.
- Cosine similarity computed with `Accelerate.vDSP` (SIMD, ~5µs per pair on M-series).

### 6.4 "Forget" semantics

`forget(id)` appends a `tombstone` event referencing `id`. All queries filter `tombstone_for IS NULL`. Storage is never reclaimed automatically; a `vacuum` command (CLI subcommand of the binary, not user-facing) does compaction if the user asks.

## 7. Proactivity — one pipeline, many policies

```
ObservationStream  ──►  [Policy 1, Policy 2, ..., Policy N]  ──►  Action
```

An `Observation` is a value:

```swift
struct Observation {
  let ts: Date
  let kind: ObservationKind  // .screenChange, .tick, .userKeystroke, .calendarEvent, .terminalExit, .axDialog, .idleDuration, .appCrash, .userInvoked
  let context: [String: JSONValue]
}
```

A `Policy` is a function `(Observation, History) -> Action?`:

```swift
protocol Policy {
  func evaluate(_ obs: Observation, history: HistoryView) async -> Action?
}
```

An `Action` is `whisper(text)` or `notify(payload)` or `runTool(name, args)`. Whispers go through the Live socket as injected user-context messages; notifications hit a small "anything?" buffer; tool actions execute through §8.

### 7.1 The shipped policies

| Policy                | Mode                | Trigger                                                                                                                                                                                |
| --------------------- | ------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `UserInvokedPolicy`   | always              | `kind == .userInvoked` (⌘⇧A pressed). Action: dump notes buffer.                                                                                                                       |
| `CalendarPolicy`      | always              | event in next 5 min. Action: whisper 1× per event.                                                                                                                                     |
| `BuildFailurePolicy`  | always              | terminal exit-code > 0 in foreground Terminal/iTerm/Ghostty. Action: whisper.                                                                                                          |
| `ErrorDialogPolicy`   | always              | AX detects modal error dialog within 2s of last tool call. Action: whisper + offer dismiss.                                                                                            |
| `IdlePolicy`          | accountability-only | idle > 30 min. Action: whisper.                                                                                                                                                        |
| `AppCrashPolicy`      | always              | `NSWorkspace` crash notification. Action: notify.                                                                                                                                      |
| `DriftDetectorPolicy` | accountability-only | every 30s, ask Flash on a _separate_ URLSession with 2s hard deadline: "user is trying to focus on `<goal>` — is the current screen state aligned?" Action: whisper if drift detected. |

Each policy is its own `Task`, supervised by a parent `PolicyHost` that restarts on crash with exponential backoff (1s → 60s capped). One policy failing does not affect others. Each policy's last error is observable in the menu bar's debug menu.

### 7.2 Modes

There is no global "push-mode" toggle. The active _policy set_ is a value (`enabled: Set<Policy.ID>`). Modes are presets:

- **Default**: 5 always-on policies + `UserInvokedPolicy`.
- **Accountability**: above + `IdlePolicy` + `DriftDetectorPolicy`. Activated by user saying _"focus me on X for the next N minutes"_. Aura sets `activeGoal = "X"`, schedules a `Mode.Default` swap at `now + N`, replies _"Got it — checking in if you drift."_

The user's words can change the policy set; _that's_ the toggle.

## 8. Action — tools as actions on the world

### 8.1 The contract

A tool call returns:

```swift
enum ToolOutcome {
  case ok(output: JSONValue)
  case needsClarification(question: String)
  case failed(reason: String)
}
```

That's it. The "before/after state" Hickey criticized is not part of the tool result. The system independently records `state.observation` events. Tools do their job; observers observe.

### 8.2 Targeting — three primitives, one composer

```swift
func find_ax(_ description: String) async -> AXTarget?
func find_ocr(_ description: String) async -> OCRTarget?
func find_vision(_ description: String, hint: NormalizedRect?) async -> VisionTarget?

// Convenience composer; callers can also call the primitives directly.
func locate(_ description: String,
            strategies: [TargetStrategy] = [.ax, .ocr, .vision]) async -> Target?
```

Each primitive has its own truth conditions:

- `find_ax` — structural; succeeds when an AXElement matches by role+label+title.
- `find_ocr` — perceptual; uses Vision framework's `VNRecognizeTextRequest` against a fresh capture.
- `find_vision` — inferential; one Gemini Flash REST call (`gemini-2.5-flash`) on the screen + description, returns 0–1 normalized coords. Has its own circuit breaker (3 fails → 30s cooldown) so it can't take down the system.

Gemini sees `locate` and the three primitives, and picks. Most calls go through `locate`. Heavy callers (e.g. browser-aware code) call `find_vision` directly.

### 8.3 Tool catalogue

#### Interaction (5)

`click(target)`, `type(text)`, `key(combo)`, `scroll(target, direction, amount)`, `drag(from, to)`

Each has a **per-tool ground-truth probe**, not a generic state diff:

- `click` → after CGEvent post, poll AX focus / pixel hash for change; on no change, retry once via keyboard shortcut if applicable.
- `type` → read AXValue of focused element after typing; verify substring match on what we typed.
- `key` → check AX focus or active app changed.
- `scroll` → AX scroll-position changed.
- `drag` → both endpoints' AX states changed.

If verification fails, return `failed("posted CGEvent, no observable change")`. Don't lie to Gemini.

#### App-native (10)

Direct Swift APIs — no AppleScript bridge:

- `mail.send(to, subject, body)` — `MailKit` if available, AppleScript fallback only for send.
- `mail.search(query)`, `mail.read(id)` — read via AppleScript (MailKit doesn't expose reads). **No JXA** (broken on Sonoma+).
- `messages.send(recipient, body)` — AppleScript (the only path that works post-Sonoma).
- `calendar.add(title, start, end, notes?)`, `calendar.next_event()` — `EventKit`.
- `contacts.find(query)` — `Contacts.framework`.
- `reminders.add(title, due?)` — `EventKit`.
- `clipboard.read()`, `clipboard.write(value)` — `NSPasteboard`.
- `spotlight.query(text)` — `MDQuery`.
- `shortcuts.run(name, input?)` — `shortcuts run` via `Process` (every Shortcut becomes a tool).

#### Browser (3)

Chrome DevTools Protocol over local WebSocket:

- `browser.navigate(url)`
- `browser.eval(js)` — full page-eval, returns JSON
- `browser.find(selector or text)` — DOM-side, no AX involvement

Chrome must be launched with `--remote-debugging-port=9222`. Aura detects this and prompts the user to enable it (one click in onboarding). For Safari/Arc support: deferred to v3.1.

#### Escape hatch (3, sandboxed)

- `run_applescript(source)` — 10s timeout, 10KB output cap. **Blocks**: `do shell script`, `run script`, AppleScript-concatenated obfuscation patterns (the v2 detector logic, re-derived).
- `run_shell(command)` — 10s timeout, 10KB cap. **Blocks**: `|`, `;`, `` ` ``, `$()`, `>`, `<`, `&&`, `||`, `sudo`. Writes restricted to `~/Documents`, `~/Downloads`, `~/Desktop`, `~/Library/Caches/aura/`.
- `run_swift(source)` — `swift -e` one-shot. Same timeouts and write restrictions.

First call to any escape-hatch tool in a session prompts the user inline: _"Aura wants to run a Swift snippet. Show me what / Approve / Always allow this session."_

#### Memory (3)

- `remember(fact)` — appends a manual `kind='fact'` event with full embedding.
- `recall(query)` — returns top-8 matching facts.
- `forget(id)` — appends tombstone.

Gemini calls these explicitly. No magic auto-memory inside tool calls.

### 8.4 Permission gates

| Action class            | Asked when                              | Stored as                          |
| ----------------------- | --------------------------------------- | ---------------------------------- |
| Microphone              | first launch                            | TCC (system)                       |
| Screen recording        | onboarding via `SCContentSharingPicker` | TCC (system)                       |
| Accessibility           | onboarding                              | TCC (system)                       |
| Apple Events automation | first AppleScript tool                  | per-app TCC                        |
| Shell writes            | first `run_shell`                       | per-session bool, persisted to log |
| Browser CDP             | first browser tool                      | one-time onboarding step           |

## 9. Threads & concurrency (Carmack: explicit lanes)

```
┌──────────────────────────────────────────────────────────────────┐
│ Main (Cocoa runloop)                                              │
│   – Menu bar, FloatingPanel, Onboarding                           │
│   – Never blocks > 16ms                                           │
├──────────────────────────────────────────────────────────────────┤
│ Realtime audio (CoreAudio render context, RT-priority)            │
│   – Owns mic ring buffer (lock-free SPSC)                         │
│   – Owns playback ring buffer                                     │
│   – No actor hops, no allocations, no system calls                │
├──────────────────────────────────────────────────────────────────┤
│ ScreenCaptureKit queue (SCK-owned dispatch queue)                 │
│   – On every frame: hash, drop if unchanged, drop if backpressure │
│   – Never queues — overwrites the single-slot "next frame to send" │
├──────────────────────────────────────────────────────────────────┤
│ Network writer (single Task on a serial dispatch queue)           │
│   – Sole writer to the WebSocket                                  │
│   – Drains: audio chunks (every 100ms), screen frames (when       │
│     changed), tool results                                         │
├──────────────────────────────────────────────────────────────────┤
│ Tool dispatch (Task pool, max 8 concurrent)                       │
│   – Never blocks; tools are async by contract                     │
├──────────────────────────────────────────────────────────────────┤
│ Policy host (one Task per Policy, supervised)                     │
│   – Drift detector uses a *separate* URLSession                   │
│   – 2s hard deadline on Flash calls                               │
└──────────────────────────────────────────────────────────────────┘
```

**Forbidden patterns** (will fail review):

- Awaiting on the audio thread.
- Calling Cocoa APIs from any non-main thread.
- Sharing the realtime URLSession with policy/Flash calls.
- Holding a lock across `await`.

## 10. Platform pinning

### 10.1 Gemini

- Model: `gemini-2.5-flash-native-audio-preview-12-2025` (Developer API path; AI Studio key)
- API version: `v1beta`
- Endpoint: `wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent`
- Flash REST: `gemini-2.5-flash` for fact extraction, drift detection, vision oracle
- **Do not migrate to `gemini-3.1-flash-live-preview`** — it does not support affective dialog or proactive audio (research, April 2026). Revisit when 3.x reaches feature parity.

### 10.2 Apple

- Min OS: macOS 14
- Audio: `AVAudioEngine`, voice processing IO enabled, install tap at hardware format, manual resample to 16kHz f32 → PCM16 → base64
- Screen: `ScreenCaptureKit` + `SCContentSharingPicker` for first-run permission (mandatory on macOS 15+)
- Embeddings: `NLContextualEmbedding`, `.cpuOnly`
- Input synth: `CGEvent.post`, requires Accessibility (Hardened Runtime, no extra entitlement beyond automation)
- Apple Events: requires `com.apple.security.automation.apple-events` (for AppleScript-based mail/messages)
- Distribution: notarized DMG. App Store rejects synthetic-input apps; not a path.

## 11. UI

Three surfaces. Total Swift UI code budget: 600 LOC.

1. **Menu bar dot** — `NSStatusItem`. Color encodes state: gray (idle), green (listening), blue (speaking), amber (rotating session), red (offline). Click → opens panel. Right-click → mute mic / mute proactive / quit.
2. **Floating panel** — `NSPanel`, translucent, always-on-top, 360×480. Contents: live transcript scroll, last 6 turns, text-input fallback, "Anything?" button (the D-mode pull), settings sprocket. Hotkey `⌘⇧A` toggles.
3. **Onboarding** — single SwiftUI window; 4 steps: API key → mic permission → SCContentSharingPicker → Accessibility → "say hi to Aura."

## 12. Code shape

```
Aura.app/
  Sources/
    App/                 AuraApp.swift, AppState.swift, Config.swift, Keychain.swift
    UI/                  MenuBar.swift, FloatingPanel.swift, Onboarding.swift
    Conversation/        Conversation.swift, Turn.swift, StateDigest.swift
    Transport/           LiveTransport.swift, Reconnect.swift, AudioCue.swift
    Voice/               Capture.swift, Playback.swift, BargeIn.swift, RingBuffer.swift
    Vision/              ScreenStream.swift, AXTree.swift, Hash.swift
    Action/              ToolRegistry.swift, Outcomes.swift,
                         Targeting.swift,                 // find_ax / find_ocr / find_vision / locate
                         Interaction.swift,               // click/type/key/scroll/drag + per-tool probes
                         AppNative.swift,                 // mail/messages/calendar/contacts/reminders/clipboard/spotlight/shortcuts
                         Browser.swift,                   // CDP
                         EscapeHatch.swift                // applescript/shell/swift, sandboxed
    Memory/              Log.swift,                       // the events table — sole persistent store
                         Lenses.swift,                    // episodic/semantic/behavioral/procedural queries
                         Embeddings.swift,                // NLContextualEmbedding wrapper, cpuOnly
                         Forget.swift                     // tombstones
    Observation/         Observation.swift, ObservationStream.swift,
                         AXObserver.swift, IdleClock.swift, CalendarWatcher.swift,
                         TerminalWatcher.swift, AppCrashWatcher.swift
    Policy/              Policy.swift, PolicyHost.swift,
                         UserInvoked.swift, Calendar.swift, BuildFailure.swift,
                         ErrorDialog.swift, Idle.swift, AppCrash.swift, DriftDetector.swift
    Util/                Log.swift, JSONValue.swift, Cosine.swift
  Resources/             Info.plist, Aura.entitlements, AppIcon.icns
  Tests/                 one test target, focused on Memory/Lenses, Policy/* and Targeting
```

Total LOC budget: **3,500** (hard cap). Per-folder soft caps (allow ~10% slack — not every folder will max out): App 250, UI 500, Conversation 150, Transport 350, Voice 300, Vision 200, Action 700, Memory 250, Observation 200, Policy 250, Util 150. Sum ≈ 3,300; the remaining ~200 is overflow buffer. If any cap is hit, that folder is wrong.

## 13. Out of scope (v3.0)

- Cross-device memory sync (single-Mac only, on purpose)
- Multi-user
- Plugin system
- Auto-updater
- Safari/Arc support (Chrome only via CDP)
- Procedural memory enabled by default (it's an opt-in experiment)
- Local LLM (no MLX, no Ollama — Gemini Live is the brain)
- Cloud anything

## 14. Risks & their mitigations

| Risk                                         | Mitigation                                                                                                           |
| -------------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| Gemini API key in config file leaks          | Keychain only; no fallback to file.                                                                                  |
| Live API model deprecated mid-development    | Pinned in `Config`, updateable in one file; CI check fetches `models?key=` to confirm availability daily during dev. |
| Hard rotation gap is audible/jarring         | 80ms 440Hz cue is intentional. User-feedback gate before optimizing further.                                         |
| ANE contention stalling audio                | `.cpuOnly` on embeddings. If still seen, move embeddings to a low-priority background task.                          |
| AX trees lying on Electron / WKWebView       | Per-tool ground-truth probes catch silent-no-op clicks; `find_vision` available as fallback.                         |
| Procedural memory drift (playbooks rotting)  | Off by default; auto-disables after 30 days of low utility (§6.2).                                                   |
| Drift detector spamming whispers             | Hard 60s minimum between whispers in accountability mode; cosine-dedup against the last whisper.                     |
| User turns Aura off and we never know        | We don't run telemetry — that's the price of "no cloud shit." Accepted.                                              |
| Voice processing IO unavailable on some Macs | Non-fatal: degrade gracefully to no echo cancel, log once.                                                           |

## 15. What "done for v3.0" means

- Cold launch to first listening state ≤ 1.5s
- Round-trip user-speech to assistant-speech ≤ 600ms p50 (Gemini's own SLO is the floor)
- Hard rotation ≤ 300ms gap
- 1 hour continuous use without crash, leak, or audio glitch
- All 7 policies have at least one passing test
- Onboarding completes in < 90 seconds for a fresh user with permissions to grant
- Binary size ≤ 25 MB
- ≤ 3,500 LOC

When all 8 are green, v3.0 ships.

---

## Appendix A — What v2 had that v3 deliberately drops

- Rust core + Swift shell → Swift only
- 10 crates → flat Swift modules
- Cloud Run proxy (`aura-proxy`) → direct API calls
- Memory agent on Cloud Run + Firestore → local SQLite event log
- 4 separate memory subsystems → 1 log, 4 queries
- 20 tools with mixed contracts → ~25 tools, uniform contract, three-strategy targeting
- Soft session handoff → hard rotation with audible cue
- Coordinate-first click → seek-first click with per-tool probes
- AnonymousFirebase auth → no auth (no remote)
- Daemon process + IPC → one process

## Appendix B — Pressure-test sources

- Carmack persona — flagged the soft handoff, threading model, push-mode contention, AX/hash false positives, vector recall on audio path, trigger supervision.
- Hickey persona — flagged memory complecting, tool result coupling, proactivity duplication, `seek` polysemy, conversation/socket complecting, configuration as scattered mutable state, layout by implementation kind.
- Web research (April 2026) — model ID stability, JXA breakage on Sonoma+, `SCContentSharingPicker` requirement, ANE contention with concurrent CoreML, async function calling semantics, affective dialog gating to 2.5.

All findings folded in.
