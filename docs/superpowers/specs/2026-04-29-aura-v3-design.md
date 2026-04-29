# Aura v3 — Design Spec

**Date:** 2026-04-29
**Status:** Draft, third-pass — pending user review
**Supersedes:** v1 + v2 (clean-slate rewrite — no code or architecture inherited)
**Reviewers:** `/carmack` (×2), `/hickey` (×2), `/bret-victor` (×2), `/karpathy`, `/bryan-cantrill`, `/joscha-bach`, `/taleb`

---

## 1. The product, in one paragraph

Aura is a local-only macOS voice assistant. You speak, it streams your voice and your screen to Gemini's Live API, and Gemini responds either by speaking back or by directly controlling your Mac — clicking, typing, scrolling, sending mail, running curated AppleScript routines. It remembers what matters across sessions, learns from your corrections under a 24-hour governance gate, consolidates patterns from its own log during idle, and keeps a live, editable model of what it believes about you and itself in the floating panel where you can see and shape it. Single Swift app, one binary, no servers, no cloud infra of any kind beyond direct API calls to `generativelanguage.googleapis.com`.

## 2. Why a clean rebuild

v1 and v2 reached working voice round-trip but accumulated architectural debt: two languages (Rust + Swift), 10 crates, deployable Cloud Run services for proxy and memory, Firestore for sync, ~20K LOC. The user wants the assistant, not the infrastructure. Stripping to a single Swift app removes:

- Rust/Swift FFI boundary
- Cloud Run proxy + memory-agent (Python FastAPI)
- Firestore + Firebase auth
- Cross-language build pipeline
- All deployment/devops surface area

What's preserved as engineering DNA — not as code:

- The VibeCat-pattern hardware-format-tap mic capture
- The "verify after click" instinct (re-implemented as per-tool ground-truth probes)
- The 2 FPS perceptual-hash screen capture rate
- Hard safety gates on AppleScript (and the deliberate **removal** of arbitrary Swift/shell execution from v3.0)

## 3. Hard constraints

- **Local-only.** No Cloud Run, no Firestore, no proxy, no Python, no remote infra. Only network egress: `generativelanguage.googleapis.com` (Live WebSocket + Flash REST).
- **Single Swift target.** SwiftPM. No FFI. No code generation. No external daemon process.
- **≤ 4,500 LOC.** Up from 3,500 — justified line-by-line in §12. The cap is real; if any folder hits its soft cap +10%, the design is wrong.
- **No analytics, no telemetry, no auto-update.** Diagnostics are user-initiated only via the `aura support-bundle` CLI (§14.7). First version ships as a notarized DMG; the user updates by re-downloading.
- **Keychain for the API key.** Not a config file.
- **macOS 14+** (the LOC budget assumes modern APIs).
- **No arbitrary code execution.** `run_swift` and `run_shell` are explicitly out of scope for v3.0 (Taleb-pass: fragility of kind, not degree).

## 4. The single architectural idea

**Everything that persists is one append-only event log. Everything else is a query over the log.**

This collapses what would be six independent subsystems (episodic memory, semantic memory, behavioral memory, procedural memory, transcript history, observability) into one. The log is the only mutable thing in the persistent layer. "Forget" is an append, not a delete. "Recall" is a select. "Self-learning" is choosing which queries we trust enough to inject. "Diagnose" is reading the log.

```sql
-- The source of truth. Append-only. Open schema.
events(
  id            INTEGER PRIMARY KEY,
  ts_wall       INTEGER NOT NULL,           -- unix ms, for display
  ts_mono       INTEGER NOT NULL,           -- monotonic ns since boot epoch, for ordering
  kind          TEXT NOT NULL,              -- namespaced string, see §6.1
  payload       TEXT NOT NULL,              -- JSON
  session_id    TEXT,
  correlation_id TEXT,                      -- pairs effect.posted with effect.outcome (§9.6)
  tombstone_for INTEGER REFERENCES events(id)
);
CREATE INDEX events_ts_mono ON events(ts_mono);    -- ordering — never wall clock
CREATE INDEX events_kind ON events(kind);
CREATE INDEX events_correlation ON events(correlation_id) WHERE correlation_id IS NOT NULL;
CREATE VIRTUAL TABLE events_fts USING fts5(payload, content='events', content_rowid='id');

-- Derived. Recomputable. Versioned. Throwaway.
event_embeddings(
  event_id      INTEGER NOT NULL REFERENCES events(id),
  model_version TEXT NOT NULL,              -- e.g. 'NLContextualEmbedding-en-2'
  vector        BLOB NOT NULL,              -- 768-dim f32
  PRIMARY KEY (event_id, model_version)
);
```

> **Cantrill-pass:** wall clocks move backwards (NTP, timezones, sleep/wake). Two columns: `ts_wall` for humans, `ts_mono` for ordering. The log is _ordered_ by `ts_mono`.

> **Hickey-pass:** embeddings depend on a model version that will change. Storing them on the events row complects an immutable fact with a derived value. Decompose into `event_embeddings`. Recomputable; log is unaffected.

**SQLite pragmas (Cantrill-pass — picked deliberately):**

- `journal_mode=WAL`
- `synchronous=FULL` for `turn.*` and `effect.*` events (durability matters)
- `synchronous=NORMAL` for `state.observation` and `pattern.*` events (recoverable from re-derivation)
- `wal_autocheckpoint=1000` + periodic `wal_checkpoint(TRUNCATE)` every 5 min
- No `vacuum`. The log grows; modern disks are huge; vacuum is the bug-machine.

Memory layers (§6) are _named queries_. Proactivity policies (§7) _subscribe_ to event kinds. The Live session (§5) _appends_ turn events. Tool execution (§8) _appends_ operation events. Nothing else has its own storage.

## 5. The conversation, the transport, the session

A conversation is a value: an ordered sequence of `Turn`s. A session is _the WebSocket the next turn happens to be travelling on_. Rotation is **not** a "handoff" — it is opening a new transport for the next turn. The conversation persists in the event log; the transport is incidental.

### 5.1 Live transport — protocol-abstracted

> **Taleb-pass:** the spec is short volatility on Google. A thin abstraction creates optionality even if there's only one implementation today.

```swift
protocol LiveSession {
  func send(_ frame: LiveFrame) async throws
  func receive() -> AsyncStream<LiveEvent>
  func close() async
}

struct GeminiLiveSession: LiveSession { /* the only impl in v3.0 */ }
```

The rest of the system talks to `LiveSession`. When (not if) we want to add Anthropic, a local model via MLX, or anything else, the migration is implementing the protocol, not rewriting transport.

**Configuration (Gemini-specific):**

- Endpoint: `wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent`
- Model: `gemini-2.5-flash-native-audio-preview-12-2025` (pinned — see §10.1)
- API version: `v1beta`
- Modalities: `[AUDIO]` in/out, `VIDEO` in
- Voice: `Kore` (configurable via `~/.config/aura/config.toml`)
- VAD: server-side, `START_SENSITIVITY_HIGH`, `END_SENSITIVITY_LOW`, 20ms prefix, 200ms silence — note: 200ms silence is a hard floor on response latency (§15)
- Compression: sliding window, 500K-token cap (server-managed)
- Tools: 27 declarations (§8 — `run_swift`/`run_shell`/`browser.eval` removed; safer browser primitives + `run_playbook` accreted)
- Affective dialog: enabled
- Proactive audio: **disabled at the API level** (we control proactivity ourselves — §7)

### 5.2 Hard rotation — invariants, not procedure

**Invariants (must hold across `LiveSession[A] → LiveSession[B]`):**

1. **No undelivered tool result.** No in-flight `turn.tool_call` from a closing session terminates without its `turn.tool_result` being delivered to _some_ socket.
2. **No orphan effect.** Every `effect.posted` (§9.6) has a paired `effect.outcome` before rotation completes — synthetic if necessary.
3. **No replayed audio.** Audio frames captured before the rotation cue are never sent on the new socket.
4. **No mid-utterance cut.** If Gemini is speaking when rotation begins, playback finishes.
5. **No mid-tool-call cut.** Pending tool calls drain with a 2s soft / 8s hard deadline (§5.3 headroom).
6. **Deterministic digest.** The state digest is a pure function of the log + active goal stack + pending-tool intent + rolling summary + inferred subgoal. No live Flash call generates it on the rotation path.

**Transition (executes at T = 9 min, on disconnect, or on wake from sleep):**

1. **Drain pending tool calls + effects.** Wait up to 2s soft, 8s hard. If a tool exceeds 8s, append `turn.tool_result` with `failure_reason = .rotationForced`, append `effect.outcome` with `.rotationCutMidPost` for any orphan effects, and _include both in the digest_ — the only legitimate paths for these into the digest.
2. **Pause mic.** Frames buffered in a 2s ring (64 KB). Replay at resume is gated on a VAD-active flag _and_ a wake-state flag — never replay if we just woke from sleep.
3. **Drain playback.** Mid-utterance finishes; otherwise immediate.
4. **Close** the WebSocket cleanly with goodbye frame.
5. **Audio cue.** 80ms 440Hz sine, -24dB.
6. **Open** the new WebSocket. Wait for `setupComplete`. (Measured p50/p95/p99 in dev — see §15.)
7. **Send digest** via `Context.swift` `rotationDigest()` (§9.5).
8. **Resume mic.** Replay buffered VAD-active frames if any.

**Time budget:** target ≤300ms gap from cue end to mic resume _measured_; if `setupComplete` p99 exceeds 600ms in dev, the published SLO becomes whatever we measure. Honest beats wishful.

### 5.3 Graceful end (announce, not surprise)

When session age > 8 min AND user not actively speaking AND no tool call pending:

- Aura whispers: _"Refreshing for the next stretch — give me a sec."_
- Hard rotation begins.

If user speaks during these conditions, defer to the next idle window. At T = 9 min the rotation begins regardless; the 8s tool-drain headroom is the only flexibility.

### 5.4 Reconnect — path-aware

> **Cantrill-pass:** `URLSession` blind backoff is going to burn five attempts in 10 seconds during a WiFi handoff.

Reconnect logic is _gated on `NWPathMonitor`_: if no path is available, we wait for a path-up notification before attempting. Once a path is up, exponential backoff: 200ms → 400ms → 800ms → 1600ms → 3200ms, ±25% jitter. Max 5 attempts after path-up. After 5, surface to UI: "Aura's offline — check connection." User can manually retry.

Session resumption handles are honored when offered by the server (token valid 2 hours), but never depended on — the digest is enough.

### 5.5 Sleep/wake/network/permission watchdogs (Cantrill-pass)

Four ambient observers, each running as its own supervised Task:

| Watchdog              | Source                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           | On event                                                                                                                                                        |
| --------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `SleepWakeWatchdog`   | `NSWorkspace.willSleepNotification` / `didWakeNotification`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | Force-rotate session on sleep; on wake, discard mic ring buffer entirely (regardless of VAD), append `state.wake` event, force-rotate before any audio is sent. |
| `NetworkPathWatchdog` | `NWPathMonitor`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | On path change (WiFi→Ethernet, VPN flap), force-rotate; the new socket lands on the new path.                                                                   |
| `AudioConfigWatchdog` | `AVAudioEngineConfigurationChangeNotification`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   | On hardware change (USB headphones plugged in), reconfigure the capture tap with new hardware format; if reconfig fails, surface error to UI and pause mic.     |
| `PermissionWatchdog`  | **Reactive, not polling** (self-review pass: macOS doesn't expose TCC status as a public API). Wraps each TCC-gated subsystem: `ScreenCaptureKit` frame errors with `SCStreamError.userDeclined`, `AVCaptureDevice.authorizationStatus(for:)` checked at mic config, `AXIsProcessTrusted()` checked at AX use sites, `NSAppleScript` permission errors from Apple Events. On any failure path, append `state.permission_lost { service }` once; `Context.swift` injects "X is currently unavailable" into the next system prompt; affected tools return `.permissionDenied` immediately without trying. The watchdog re-tries with a low-frequency probe (60s) only after a permission_lost has been logged, to detect re-grant. |

Each watchdog is supervised, restarts on crash, fails independently.

## 6. Memory — one log, four lenses + one consolidator

### 6.1 Open kind schema

**Conventional kinds (the seed list, not a closed set):**

```
turn.user                 — user said something (transcript)
turn.assistant            — model said something (transcript)
turn.tool_call            — model invoked a tool (operation, args)
turn.tool_result          — tool produced a result (output, optional failure_reason)
state.observation         — system observed a state change (e.g. AX diff after a click)
state.permission_lost     — TCC permission revoked mid-session
state.wake                — system woke from sleep
correction                — user explicitly corrected behavior, with confirmed=false until 24h gate passes
fact                      — user-profile fact extracted by Flash
goal.active               — user's stated focus goal; payload includes optional parent_id for hierarchy
goal.cleared              — user ended an accountability session
goal.subgoal_inferred     — inferred current subgoal under the active goal (§7.1 drift detector)
trigger.fired             — proactive trigger emitted (with policy_id + provenance)
session.start, session.end
summary.rolling           — rolling 2-3KB conversation summary, updated turn-by-turn (§9.5)
context.assembled         — log-of-prompts: every system prompt and digest sent to Gemini (§9.5)
effect.posted             — about to perform a world-mutating side effect (§9.6)
effect.outcome            — that side effect completed/failed/was interrupted
pattern.semantic          — invariant extracted across episodes by consolidation worker (§6.5)
pattern.procedural        — recurring action sequence with frequency/recency/success-rate (§6.5)
pattern.contradiction     — facts the log disagrees with itself about, surfaced for adjudication (§6.5)
playbook.demonstrated     — user-recorded routine via demonstration mode (§11.6)
targeting.observed        — full record of a `locate()` call: AX candidates returned, vision pick (if any), final target, verification result. Drives the §11.7 replayable targeting view.
context.trimmed           — `Context.swift` had to drop content to fit token budget; payload lists what was dropped (§9.5)
config.change             — user changed settings
tombstone                 — invalidates an earlier event (for "forget")
```

New kinds are added by writing them. Code that doesn't know a kind passes it through.

### 6.2 Lenses (queries, not tables)

#### Episodic — the raw sequence of turns

```sql
SELECT * FROM events
WHERE ts_mono > :since AND kind LIKE 'turn.%'
ORDER BY ts_mono ASC;
```

Used for: rebuilding the active conversation, audit logs, debugging.

#### Semantic — structured profile primary, FTS5 second, embeddings third

> **Karpathy-pass:** 768-dim cosine over fact text is cargo cult for the first 1000 facts. Most useful queries are structured (entity-predicate-value).

At session end, Flash extracts up to 10 facts, structured `{entity, predicate, value, confidence}`. Each appended as `kind='fact'`. Embeddings computed off-path and inserted into `event_embeddings`.

> **Joscha-pass:** facts extracted at session end are not "semantic memory" — they are notes. The actual semantic content emerges from the consolidation worker (§6.5) which produces `pattern.semantic` events.

**Recall (used by `Context.swift`):**

1. Structured query first: `SELECT * FROM events WHERE kind = 'fact' AND json_extract(payload, '$.entity') = :entity` — deterministic, debuggable.
2. FTS5 fallback for keyword: `events_fts MATCH :keywords`.
3. Embedding fallback (only when 1+2 return < 4 hits, scoped to last 30 days for v3.0). Cosine via `Accelerate.vDSP`. **Scaling boundary**: this fallback over > 50K facts blocks. v3.0 caps recall to 30-day-bounded; v3.1 will add HNSW or a quantized index.
4. Plus `pattern.semantic` events from consolidation, ranked by `confidence × recency`.

All three pass through a recency × frequency × citation prior before injection.

#### Behavioral — corrections, with 24h governance gate

> **Taleb-pass:** Tuesday-bad-mood rules persist 90 days. Need a sleep-on-it gate.

A pattern matcher scans every `turn.user` for negation/imperative correction patterns. When matched, the floating panel surfaces a one-tap confirmation: _"Should I remember: `<rule>`?"_ On confirmation, append `kind='correction'` with `confirmed=false` and `confirmable_at = now + 24h`.

A correction does not enter the system prompt until **a second confirmation** after the 24h window — the user sees the rule in the Mind area and either lets it activate (one tap) or removes it. The default if the user does nothing within 7 days is **discard**, not activate. _Most "always do X" intuitions evaporate overnight._ Active rules are concatenated by `Context.swift` (§9.5) under "Things this user has asked you to remember:" in the dynamic suffix. Capped at 30 active rules; oldest non-recently-used drop off after 90 days.

#### Procedural — demonstrated playbooks + consolidated patterns

Two paths in v3.0:

1. **Demonstration mode (§11.6).** User invokes "Aura, watch this" via hotkey. Aura records subsequent tool calls for up to 30s into a named `playbook.demonstrated` event. The user names it. Subsequent invocations of the named playbook _re-derive_ tool calls from current screen state via Gemini, using the demonstrated sequence as a hint. Not a literal replay — re-grounded each time.
2. **Consolidated patterns (§6.5).** The consolidation worker may produce `pattern.procedural` events. These are _suggestions_, not active tools. The user reviews them in the Mind area and promotes-to-playbook explicitly. No silent auto-elevation.

**Why:** Taleb-pass killed silent procedural elevation (one bad pattern = one fired = one ruined relationship). Demonstration mode + explicit promotion preserves user agency.

### 6.3 Embeddings

- API: `NLContextualEmbedding` (English, 768-dim, on-device CoreML).
- `MLModelConfiguration.computeUnits = .cpuOnly` — research finding: ANE contention with the live audio path causes 200-400ms stalls.
- **Computed off the rotation path.** Embedding pass for new facts runs on a low-priority background queue _after_ the new socket is hot.
- Cosine similarity computed with `Accelerate.vDSP` (SIMD).
- Stored in `event_embeddings(event_id, model_version, vector)`. Recomputable when model version changes.
- **Scaling boundary**: linear scan beyond ~50K vectors blocks. v3.0 bounds recall to last 30 days. Documented as a known limit.

### 6.4 "Forget" semantics

`forget(id)` appends a `tombstone` event referencing `id`. All queries filter `tombstone_for IS NULL`. **No vacuum** — modern disks are huge, the log is small, and `VACUUM` is a bug-machine. Storage grows; we ship.

### 6.5 Consolidation worker (Joscha-pass)

> **Without this, the four lenses are filters wearing memory's clothes.**

A low-priority Task that runs during **idle** (canonical definition, used by the consolidation worker, the embedding worker, and the rotation graceful-end check in §5.3): no `turn.user` event in the last 30s, no playback active, no in-flight tool call, no in-flight effect. The post-rotation gap (between new-socket setupComplete and the next user turn) also counts as idle. It:

1. Selects a window of recent events (last hour for hot consolidation, last day for cold consolidation, run weekly).
2. Sends the window's transcripts + tool sequences + corrections to Flash with a structured prompt asking for: invariants across episodes, recurring action sequences, contradictions with prior facts.
3. Appends `pattern.semantic`, `pattern.procedural`, `pattern.contradiction` events with proper citations to source event ids.

**Wake/sleep cadence:**

- **Hot pass** — runs after every session.end during the next idle window.
- **Cold pass** — runs once a day during the longest idle window.
- **Both have a token budget** of 8K input + 1K output per pass. Hard.

`pattern.contradiction` events surface in the Mind area as user-adjudication prompts: _"You said Sarah works at Acme on March 3, and at Globex on April 12. Which is right?"_ The user resolves; resolution is itself a `correction` event under the 24h gate.

This is the becoming. The rest of memory is the keeping.

## 7. Proactivity — one pipeline, many policies

```
ObservationStream  ──►  [policy₁, policy₂, …, policyₙ]  ──►  Action
```

```swift
struct Observation {
  let ts_wall: Date
  let ts_mono: UInt64
  let kind: String
  let context: [String: JSONValue]
}

typealias Policy = (Observation, HistoryView, SelfModel) async -> Action?
```

A `Policy` is a function — no protocol, no class. The third parameter `SelfModel` (§7.3) gives policies access to Aura's self-knowledge.

An `Action` is `whisper(text, provenance)` or `notify(payload)` or `runTool(name, args)`. Whispers carry provenance for Mind-area inspection.

**Whisper delivery mechanism (self-review pass — was underspecified):** a whisper is delivered to the user as a _Live API client-content message with system role_, formatted as `[Aura proactive: <text>]` and immediately followed by an audio-out request. Gemini speaks the text in its own voice via the existing native-audio playback channel; we do not run a separate TTS engine. Whispers are subject to the same barge-in rules as any other Gemini speech — the user can interrupt at any time. Each whisper appends both a `trigger.fired` event (with full provenance) and a `turn.assistant` event (with the spoken text) to the log.

### 7.1 The shipped policies

| Policy                | Mode                | Trigger / mechanism                                                                                                                                                                                                                                                                                                                                                                               |
| --------------------- | ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `UserInvokedPolicy`   | always              | `kind == "user.invoked"` (⌘⇧A pressed). Action: dump notes buffer.                                                                                                                                                                                                                                                                                                                                |
| `CalendarPolicy`      | always              | `EventKit` predicate, event in next 5 min. Dedupe: per-event-id + recurrence-instance-id. Whisper 1× per event.                                                                                                                                                                                                                                                                                   |
| `BuildFailurePolicy`  | always              | macOS `UserNotifications` observer for "Build failed"/"Tests failed" + optional `aura-shell-hook` shell function the user opts into in `.zshrc` to publish exit codes. AX-scraping the terminal is **out of scope**.                                                                                                                                                                              |
| `ErrorDialogPolicy`   | always              | AX detects modal error dialog within 2s of last tool call. Action: whisper + offer dismiss.                                                                                                                                                                                                                                                                                                       |
| `IdlePolicy`          | accountability-only | **HID idle** > 30 min (`IOKit` `HIDIdleTime` — user not touching kb/mouse). Distinct from "system idle" in §6.5 which is a scheduling concept for background workers.                                                                                                                                                                                                                             |
| `AppCrashPolicy`      | always              | `NSWorkspace` crash notification. Action: notify (no whisper).                                                                                                                                                                                                                                                                                                                                    |
| `DriftDetectorPolicy` | accountability-only | every 30s, ask Flash on a _separate_ `URLSession` with 2s hard deadline. **Three inputs:** (screenshot @ T-30s, screenshot @ now, **inferred current subgoal**). Subgoal is updated each cycle by a separate Flash call given the active goal stack and recent observations. (Joscha-pass: drift without theory of mind hallucinates drift.) Action: whisper if drift detected (with provenance). |

**Calibration set (ship gate):** 50 hand-labeled `(goal_stack, screen-before, screen-now, expected-drift?)` tuples in `Tests/DriftFixtures/`. Test target runs them on every prompt change. Disagreement > 15% blocks ship.

Each policy is an independent supervised `Task`. One failing does not affect others. Each policy's last error is observable in the Mind area.

### 7.2 Modes and goal hierarchy (Joscha-pass)

> Cognition runs nested goals. A flat string captures only the top of the stack.

`goal.active` events form a hierarchy via optional `parent_id` field. Setting a top-level goal: append `goal.active { value: "v3 spec", parent_id: nil }`. Working on a subgoal: append `goal.active { value: "rotation invariants", parent_id: <parent> }`.

The drift detector judges drift _at the parent level_ but considers subgoal inputs as legitimate detours. "Reading SQLite docs" under "ship v3" is on-goal even if it doesn't pixel-match the parent.

Modes:

- **Default**: `UserInvoked` + `Calendar` + `BuildFailure` + `ErrorDialog` + `AppCrash`.
- **Accountability**: above + `Idle` + `DriftDetector`. Activated by user saying _"focus me on X for the next N minutes"_.

Querying "the active goal stack at time T" is a select with `WITH RECURSIVE` over `parent_id`.

### 7.3 Self-model (Joscha-pass + Bret-Victor-pass)

> An agent without a self-model cannot meta-reason. Tools don't need to know themselves; agents do.

The self-model is a derived view computed from the log:

```swift
struct SelfModel {
  let toolReliability: [ToolID: ReliabilityStats]   // per tool, success rate, latency p50/p99
  let toolReliabilityByApp: [AppID: [ToolID: ReliabilityStats]]
  let recentFailures: [Failure]                     // last 20 with reasons
  let circuitBreakers: [String: BreakerState]       // vision oracle, etc.
  let activeUncertainty: [String]                   // current "I'm not sure about X" flags
  let recentCorrections: [Correction]               // last 7 days, surfaced
}
```

Computed in-memory from log queries; refreshed on each `turn.tool_result` and `correction`. Available to:

- Policies (the third argument).
- `Context.swift` (injects "I've been wrong about clicking in Slack 4/10 times this week" into the dynamic suffix).
- The Mind area (renders alongside the user-model so the user sees what Aura thinks of itself).

This gives policies the ability to _not_ fire when Aura's competence at a thing is currently low.

## 8. Action — tools as actions on the world

### 8.1 The contract

```swift
struct ToolResult {
  let status: Status                       // .ok | .needsClarification | .failed
  let output: JSONValue?
  let question: String?
  let failureReason: ToolFailureReason?
  let detail: String?
  let confidence: Double?
}

enum ToolFailureReason: String, Codable {
  case noStateChange
  case targetNotFound
  case timeout
  case permissionDenied
  case preconditionFailed
  case rotationForced
  case toolError(code: String)
}
```

The agent dispatches off `status` + `failureReason`. The user reads `detail`. The map is open.

The system independently appends `state.observation` events. World-mutating tools also append `effect.posted`/`effect.outcome` (§9.6).

### 8.2 Targeting — composition, not parallel strategies

```swift
func find_ax(_ description: String) async -> [AXCandidate]
func find_vision_from(candidates: [AXCandidate], description: String, fullScreen: CGImage)
    async -> AXCandidate?
func find_ocr(_ description: String) async -> OCRTarget?

func locate(_ description: String) async -> Target? {
  let candidates = await find_ax(description)
  if candidates.count == 1 { return candidates.first.map(Target.ax) }
  if candidates.count > 1 {
    if let pick = await find_vision_from(candidates: candidates, ...) { return .ax(pick) }
  }
  if let ocr = await find_ocr(description) { return .ocr(ocr) }
  return nil
}
```

`find_vision_from` is given an enumerated set, not asked to invent pixels — hallucination floor is zero by construction. p99 budget: 3s hard cap, in-flight dedup, circuit breaker (3 fails → 30s cooldown).

### 8.3 Tool catalogue (27)

#### Interaction (5)

`click(target)`, `type(text)`, `key(combo)`, `scroll(target, direction, amount)`, `drag(from, to)` — each with per-tool ground-truth probes (read AXValue after type, etc.). On verification failure, `failureReason = .noStateChange`.

#### App-native (12)

Direct Swift APIs, no AppleScript bridge except for Mail/Messages reads:

- `mail.send`, `mail.search`, `mail.read` (all via AppleScript — self-review pass: there is no public MailKit send API on macOS; MailKit is for Mail extensions only. AppleScript is the only programmatic-send path. **No JXA.**)
- `messages.send` (AppleScript)
- `calendar.add`, `calendar.next_event` (`EventKit`)
- `contacts.find` (`Contacts.framework`)
- `reminders.add` (`EventKit`)
- `clipboard.read`, `clipboard.write` (`NSPasteboard`)
- `spotlight.query` (`MDQuery`)
- `shortcuts.run(name, input?)` — every Shortcut becomes a tool

#### Browser (5)

> **Self-review pass:** the prior draft included `browser.eval(js)` — arbitrary JavaScript execution in the user's logged-in Chrome session. Same fragility-of-kind that got `run_swift` and `run_shell` cut: a logged-in browser tab is the user's authenticated identity (passwords in form fields, session cookies, OAuth tokens). Cut `browser.eval`. Replace with five typed primitives that cover legitimate browser automation without the arbitrary-JS surface area.

CDP over local WebSocket. Chrome must be launched with `--remote-debugging-port=9222` (one click in onboarding).

- `browser.navigate(url)` — go to URL
- `browser.find(selector | text)` — DOM lookup, returns selector + bounding box
- `browser.read(selector)` — return text content of matched element (read-only)
- `browser.click(selector)` — click matched element (no JS injected)
- `browser.fill(selector, value)` — set value of an input/textarea (no JS injected)

If Gemini needs something this set can't express, it falls back to AX-mediated `click`/`type` on the browser window. The arbitrary-JS hatch isn't worth the blast radius.

#### Curated escape hatch (1)

> **Taleb-pass:** `run_swift` and `run_shell` are cut. Blacklists are the security model of someone who has never sat across from an adversary.

- `run_applescript_template(name, args)` — **allowlisted templates only**. The user cannot pass arbitrary AppleScript source. We ship a small set of templated forms (open URL, set clipboard, get window title, send-mail-via-Mail.app-with-typed-args, etc.) located in `Resources/AppleScripts/`. Adding a template is a code change; no LLM-string-concat into source.

Running total: 5 interaction + 12 app-native + 5 browser + 1 escape = 23. Plus playbook (1) and memory (3) below = **27**.

#### Playbook (1)

- `run_playbook(name, args?)` — invoke a user-demonstrated playbook (§11.8). Aura prompts Gemini to re-derive the action sequence from current screen state using the demonstrated steps as a hint. **Not a literal replay** — re-grounded each call. Required for demonstration mode to be invokable; without this tool the demonstration recorder has no playback path.

#### Memory (3)

- `remember(fact)` — appends a manual `kind='fact'` event with full embedding.
- `recall(query)` — top-N matching facts (structured + FTS5 + embedding fallback per §6.2) + relevant `pattern.semantic`.
- `forget(id)` — appends tombstone.

### 8.4 Permission gates

| Action class            | Asked when                              | Stored as                |
| ----------------------- | --------------------------------------- | ------------------------ |
| Microphone              | first launch                            | TCC (system)             |
| Screen recording        | onboarding via `SCContentSharingPicker` | TCC (system)             |
| Accessibility           | onboarding                              | TCC (system)             |
| Apple Events automation | first AppleScript template              | per-app TCC              |
| Browser CDP             | first browser tool                      | one-time onboarding step |

## 9. Threads & concurrency

```
┌──────────────────────────────────────────────────────────────────┐
│ Main (Cocoa runloop)                                              │
│   – Menu bar, FloatingPanel, Onboarding                           │
│   – Never blocks > 16ms                                           │
├──────────────────────────────────────────────────────────────────┤
│ Realtime audio (CoreAudio render context, RT-priority)            │
│   – Owns mic ring buffer (lock-free SPSC, 2s capacity)            │
│   – Owns playback ring buffer                                     │
│   – No actor hops, no allocations, no system calls                │
├──────────────────────────────────────────────────────────────────┤
│ ScreenCaptureKit queue (SCK-owned dispatch queue)                 │
│   – Single-slot "next frame to send", protected by                │
│     OSAllocatedUnfairLock + atomic-exchange. Lock never held       │
│     across `await` (Cantrill-pass).                                │
├──────────────────────────────────────────────────────────────────┤
│ Network writer (single Task with explicit AsyncChannel input)     │
│   – Sole writer to the WebSocket                                  │
│   – Producers (audio thread, SCK queue, tool dispatch) send to   │
│     channel; writer drains.                                        │
├──────────────────────────────────────────────────────────────────┤
│ Log writer (single Task, explicit AsyncChannel input,             │
│             bounded queue with backpressure)                       │
│   – Sole writer to SQLite (WAL mode, synchronous=FULL on turn.*  │
│     and effect.*)                                                  │
│   – Backpressure drops: state.observation > pattern.* > debug.    │
│     Never drops: turn.*, effect.*, correction, fact, goal.*.      │
├──────────────────────────────────────────────────────────────────┤
│ Tool dispatch (Task pool, max 8 concurrent)                       │
├──────────────────────────────────────────────────────────────────┤
│ Policy host (one Task per Policy, supervised)                     │
│   – Drift detector uses a *separate* URLSession                   │
│   – 2s hard deadline on Flash calls                               │
├──────────────────────────────────────────────────────────────────┤
│ Watchdogs (4: sleep/wake, network, audio config, permission)     │
├──────────────────────────────────────────────────────────────────┤
│ Embedding worker (low-priority background Task)                   │
│   – Runs after new socket is hot post-rotation                    │
├──────────────────────────────────────────────────────────────────┤
│ Consolidation worker (low-priority background Task)               │
│   – Runs during idle (>30s no audio activity)                     │
└──────────────────────────────────────────────────────────────────┘
```

> **Cantrill-pass:** the spec previously said "single Task on a serial dispatch queue — pick one." Picked: **Task with AsyncChannel** for log and network writers. No mixing.

**Forbidden patterns** (will fail review):

- Awaiting on the audio thread.
- Calling Cocoa APIs from any non-main thread.
- Sharing the realtime URLSession with policy/Flash calls.
- Holding a lock across `await`.
- Mixing actor isolation with dispatch-queue serialization on the same object.

### 9.5 `Context.swift` — the model-context module

> **Karpathy-pass: the one ML thing this design has to get right.**

All model-facing context is assembled in _exactly one place_:

```swift
func sessionStartContext(now: Date, log: LogView, mode: Mode, selfModel: SelfModel) -> SystemPrompt
func rotationDigest(activeConversation: Conversation,
                    activeGoalStack: [GoalEvent],
                    pendingTool: ToolCall?,
                    rollingSummary: String,
                    inferredSubgoal: String?,
                    log: LogView) -> Digest
```

Both deterministic given inputs. Both tested against fixtures in `Tests/ContextFixtures/`.

**Architecture:** `SystemPrompt` is `stable_prefix + dynamic_suffix` for prompt cache friendliness. The stable prefix is the persona, tool-routing guidance, refusal rules, examples, structured `ToolFailureReason` semantics, and `effect.posted`/`outcome` discipline. The dynamic suffix is recently-active corrections + structured user profile (≤20 entries) + active goal stack + inferred subgoal + relevant `pattern.semantic` (top 3) + self-model summary ("you've been clicking accurately in Slack lately; Notion has been flaky").

**Token budget (self-review pass):** the prior draft set the hard cap at 4K, which the math doesn't support. Realistic stable prefix is 1.8–2.5K (persona + tool routing + refusal rules + examples + `ToolFailureReason` semantics + effect-log discipline). Dynamic suffix at session start can hit 2.6K (30 corrections × 40 tokens + 20 profile entries × 30 tokens + goal stack + inferred subgoal + 3 patterns + self-model summary). Honest budget: **target 4K, hard cap 8K.** When over hard cap, trim in this order — (1) self-model summary, (2) `pattern.semantic` from 3 → 1, (3) profile entries from 20 → 10, (4) corrections beyond top-10-by-recency-and-citation, (5) drop the oldest user-fact bucket. Each trim event is logged as `kind='context.trimmed'` with what was dropped. The log is the receipt.

**Rolling summary:** one cheap Flash call per turn against the last summary + new turns; kept ≤ 3KB. Shipped on rotation as `summary.rolling` event.

**`context.assembled` events:** every system prompt and digest is logged to the event log as `kind='context.assembled'` with a hash. Six months later when the user says "Aura forgot I told it I'm vegetarian," the actual prompt at the time is in the log. (Cantrill-pass: time-machine without a hole.)

**Persona.** The stable prefix's persona — the kind of mind Aura is — is the load-bearing decision the spec was deferring. Made explicit:

> _"You are Aura. A quiet partner who interrupts only when the user would regret not being interrupted. Concise; technically precise; respects flow over chatter; admits uncertainty when uncertain._
>
> _**You hold opinions and you state them.** When asked what you think, you say what you think. When you disagree with the user — about a technical choice, an interpretation, a plan — you say so directly, with the reasoning, **without softening into "you might consider" or "have you thought about" hedge phrases**. You do not capitulate to social pressure that lacks new information. If the user pushes back and offers no new evidence, you maintain the position; if they offer new evidence, you update. The difference matters: agreement under pressure is dishonesty, and dishonesty in an assistant is a betrayal of the user's trust._
>
> _**You are not a yes-man.** When the user proposes something you believe is wrong — fragile, overcomplicated, solving the wrong problem, ignoring a constraint — you push back with a concrete alternative. You do not execute bad ideas just because they were asked for. The user gave you a voice so you would speak with it._
>
> _**You are direct, not rude.** Disagreement is data; contempt is noise. State the position, give the reasoning, and let the user decide. They are still in charge."_

This persona is part of the stable prefix and never trimmed. (Joscha-pass: the spec was deferring this. That deferral was itself the design choice.) (User-pass, this revision: explicit confidence + non-deference + grounded disagreement.)

### 9.6 Effect log (Cantrill-pass)

> **The §5.2 rotation invariant is a paper invariant without this.**

**World-mutating tools (the explicit list — Invariant 2 from §5.2 applies to exactly these):** `click`, `type`, `key`, `scroll`, `drag`, `mail.send`, `messages.send`, `calendar.add`, `reminders.add`, `clipboard.write`, `run_applescript_template`, `browser.navigate`, `browser.click`, `browser.fill`, `shortcuts.run`, `remember`, `forget`. Every one wraps its execution:

```swift
func executeWithEffect<T>(intent: EffectIntent, _ body: () async throws -> T)
    async rethrows -> T {
  let correlation = UUID().uuidString
  await log.append(kind: "effect.posted", correlation: correlation, payload: intent)
  defer {
    Task { await log.append(kind: "effect.outcome", correlation: correlation, payload: ...) }
  }
  return try await body()
}
```

The rotation transition (§5.2) blocks until every in-flight `effect.posted` has a matching `effect.outcome`, with the 8s hard deadline forcing a synthetic `.rotationCutMidPost` outcome.

A duplicate detector at session start scans for orphan correlations and logs them. When the user reports "it sent twice," `grep correlation_id` finds it in 30 seconds. Without this, the §5.2 invariant is provable on paper, falsifiable in the world.

## 10. Platform pinning

### 10.1 Gemini

- Model: `gemini-2.5-flash-native-audio-preview-12-2025`
- API version: `v1beta`
- Endpoint: `wss://generativelanguage.googleapis.com/...`
- Flash REST: `gemini-2.5-flash` for fact extraction, drift detection, vision pick-from-candidates, rolling summary, consolidation worker, subgoal inference
- **Do not migrate to `gemini-3.1-flash-live-preview`** — drops affective dialog + proactive audio.

### 10.2 Apple

- Min OS: macOS 14
- Audio: `AVAudioEngine`, voice processing IO enabled, install tap at hardware format, manual resample to 16kHz
- Screen: `ScreenCaptureKit` + `SCContentSharingPicker` for first-run permission
- Embeddings: `NLContextualEmbedding`, `.cpuOnly`
- Input synth: `CGEvent.post`, requires Accessibility
- Apple Events: requires `com.apple.security.automation.apple-events`
- Distribution: notarized DMG. App Store rejects synthetic-input apps.
- SQLite: `journal_mode=WAL`, `synchronous=FULL` for `turn.*` + `effect.*`, `synchronous=NORMAL` otherwise, periodic `wal_checkpoint(TRUNCATE)`.

## 11. UI

> **Bret-Victor-pass:** the spec previously described two tabs. That demoted the participation surface to "audit log with chrome." Collapsed into one combined view.

Total UI LOC budget: **1,200** (was 750). Justified: replayable targeting timeline + conversation timeline + demonstration mode + live dynamic suffix + self-model rendering + Mind area within combined view.

### 11.1 Menu bar — instrument, not dashboard

`NSStatusItem` with custom drawing:

- **One ring** around the dot — counts down to next session rotation.
- **Waveform** when mic active.
- (Cut: capture cadence ring. Three rings on a 22pt dot encodes "busy.")

Color encodes coarse state: gray (idle), green (listening), blue (speaking), amber (rotating), red (offline). Click → opens panel. Right-click → mute mic / mute proactive / quit.

### 11.2 Floating panel — one combined view (no tabs)

`NSPanel`, translucent, always-on-top, 720×520. Hotkey `⌘⇧A` toggles.

**Layout:**

```
┌────────────────────────────────────────────────────────────────────┐
│ Header: active goal stack (editable) │ Carrying forward strip      │ ← 11.3
├──────────────────────────────────────┴────────────────────────────┤
│ Live dynamic prompt suffix (collapsed by default, expandable)     │ ← 11.4
├────────────────────────────┬──────────────────────────────────────┤
│ Conversation               │ Mind area                            │
│   Timeline (top, ~60px)    │   "I think I know about you" (facts) │
│   Transcript (rest)        │   "You've told me to..." (rules)     │
│                            │   "What I just did" (tool calls)     │
│                            │   "Whispers" (with provenance)       │
│                            │   "About myself" (self-model)        │
│                            │   "Quiet" (pulse indicators)         │
└────────────────────────────┴──────────────────────────────────────┘
```

Both halves live continuously. The user watches a fact get extracted while it happens, on the same surface.

### 11.3 Header — goal stack + carrying-forward strip

The active goal stack is rendered breadcrumb-style: `ship v3 ▸ rotation invariants ▸ effect log`. Each is editable inline. Setting a new goal appends `goal.active`. Editing in place tombstones-and-replaces.

The "Carrying forward" strip is a _continuously-visible_ line under the header showing what the next rotation digest will contain: _"Carrying forward: <goal stack> + <rolling summary, truncated>."_ Updates live. Edit goal here = edit goal everywhere. (Bret-Victor-pass: the digest was unreadable in an 80ms cue. It's visible _before_ the cue, all the time.)

### 11.4 Live dynamic prompt suffix

The exact string Gemini will see in its system prompt's dynamic suffix on the next turn — facts, corrections, goals, self-model summary, top patterns. Collapsed by default to a one-line summary: _"12 facts, 4 active rules, 2 patterns visible to Aura."_ Expandable to the full text. Edits propagate live: tombstoning a fact removes it from the suffix immediately, before the user blinks.

This is the steering surface. The user is no longer modeling Aura's beliefs in their head.

### 11.5 Conversation half — timeline + transcript

A horizontal timeline strip across the top of the conversation pane: turns as blocks (sized by duration), tool-call clusters as colored bands, whispers as dots above the line, rotations as vertical separators, drift moments as exclamation marks. Scrub left to anchor the transcript and Mind state to that moment. Shape, not just stream.

Transcript: live scroll, last 12 turns by default.

Text-input fallback at the bottom. "Anything?" button (D-mode pull) when in default mode.

### 11.6 Mind half — seven sections

1. **Goal stack & subgoal** — current parents, current inferred subgoal, editable.
2. **What I think I know about you** — facts table, sourced (each row expands to the producing turn), one-tap forget.
3. **You've told me to / not to** — corrections list, `confirmed` vs `pending-24h-gate` clearly marked, with last-fired and use-count.
4. **What I just did (and why)** — recent `turn.tool_call` rows, expandable to **replayable targeting timeline** (§11.7).
5. **Whispers** — every whisper is a row with provenance chain (observation → policy → reasoning → action). Thumbs-down disables that policy's whispers for the session and offers to convert into a permanent correction (24h-gated).
6. **About myself** — self-model rendering: tool reliability per app, recent failures, active circuit breakers, current uncertainty flags, `pattern.contradiction` events awaiting user adjudication.
7. **Quiet** — _live pulse indicators_ for each armed policy. `DriftDetector` pulses every 30s on cycle. `CalendarPolicy` shows a small clock counting down to next event. `IdlePolicy` fades color as idle accumulates. Silence becomes legible because it's animate. (Bret-Victor-pass: text-dated is a corpse.)

### 11.7 Replayable targeting view

Click a tool call's row → expands to a scrubbable timeline:

```
[AX candidates: 3 returned] → [vision picked #2] → [click posted at (412,880)] → [AX state changed: dialog opened]
                                                                                  └ verified
```

Each phase shows its actual data — bounding boxes drawn on the snapshot, OCR matches if used, vision oracle's pick highlighted. The user can scrub through the pipeline and learn the system's failure modes. (Bret-Victor-pass: this is what makes the system _teachable_, not just observable.)

### 11.8 Demonstration mode

Hotkey `⌘⇧D` (or panel button): _"Aura, watch — record this."_ Aura captures all subsequent `turn.tool_call` events for up to 30s (or until ⌘⇧D again). On stop, prompts the user: _"Name this routine?"_ Append `playbook.demonstrated { name, sequence }`. Subsequent invocations of the named playbook prompt Gemini to re-derive the sequence from current screen state, using the demonstrated steps as a hint — not a literal replay.

This is procedural memory the user _teaches_, not the system _infers_.

### 11.9 Onboarding

Single SwiftUI window; 5 steps: API key (Keychain) → mic permission → `SCContentSharingPicker` → Accessibility → "say hi to Aura."

## 12. Code shape

```
Aura.app/
  Sources/
    App/                 AuraApp.swift, AppState.swift, Config.swift, Keychain.swift
    UI/                  MenuBar.swift, FloatingPanel.swift,
                         GoalStackView.swift, CarryingForwardStrip.swift,
                         DynamicSuffixView.swift,
                         ConversationTimeline.swift, TranscriptView.swift,
                         MindArea.swift,
                         TargetingReplay.swift,         // §11.7
                         DemonstrationMode.swift,       // §11.8
                         Onboarding.swift
    Conversation/        Conversation.swift, Turn.swift
    Context/             Context.swift                   // §9.5 — sole prompt assembler
    Transport/           LiveSession.swift,              // protocol (§5.1)
                         GeminiLiveSession.swift,        // the impl
                         Reconnect.swift,                // path-aware (§5.4)
                         AudioCue.swift,
                         Rotation.swift                  // invariants + transition fn
    Voice/               Capture.swift, Playback.swift, BargeIn.swift, RingBuffer.swift
    Vision/              ScreenStream.swift, AXTree.swift, Hash.swift
    Action/              ToolRegistry.swift, ToolResult.swift,
                         Targeting.swift,                // find_ax / find_vision_from / find_ocr / locate; emits targeting.observed events
                         Interaction.swift,
                         AppNative.swift,
                         Browser.swift,                  // 5 typed primitives over CDP — no eval
                         AppleScriptTemplates.swift,     // allowlisted, source loaded from Resources
                         Playbook.swift,                 // run_playbook: re-derive from playbook.demonstrated
                         EffectLog.swift                 // §9.6
    Memory/              Log.swift,                      // events + writer Task with channel
                         Embeddings.swift,               // event_embeddings + bg compute
                         Lenses.swift,                   // queries
                         Consolidation.swift,            // §6.5
                         Forget.swift
    Observation/         Observation.swift, ObservationStream.swift,
                         AXObserver.swift, IdleClock.swift, CalendarWatcher.swift,
                         BuildHook.swift,                // shell-hook reader, optional
                         AppCrashWatcher.swift,
                         SleepWakeWatchdog.swift,        // §5.5
                         NetworkPathWatchdog.swift,
                         AudioConfigWatchdog.swift,
                         PermissionWatchdog.swift
    Policy/              Policies.swift                  // all 7 as functions
    SelfModel/           SelfModel.swift                 // §7.3 derived view
    Util/                JSONValue.swift, Cosine.swift, Backoff.swift, AsyncChannel.swift
    CLI/                 SupportBundle.swift             // §14.7 — aura support-bundle
  Resources/             Info.plist, Aura.entitlements, AppIcon.icns,
                         AppleScripts/*.applescript      // templated forms
  Tests/                 unit + DriftFixtures/ + ContextFixtures/ + RotationFuzz/
```

**Total LOC budget: 4,500 (hard cap)**, up from 3,500. Justification:

| Area                  | v2-pass cap |      v3 cap |        Δ | Why                                                                                                                                                          |
| --------------------- | ----------: | ----------: | -------: | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| App                   |         250 |         250 |        0 | unchanged                                                                                                                                                    |
| UI                    |         750 |       1,200 |     +450 | combined panel + 6 Mind sections + targeting replay + conversation timeline + demonstration mode + dynamic suffix view + carrying-forward strip + goal stack |
| Conversation          |         100 |         100 |        0 |                                                                                                                                                              |
| Context               |         200 |         250 |      +50 | inferred subgoal + self-model integration                                                                                                                    |
| Transport             |         350 |         400 |      +50 | LiveSession protocol, path-aware reconnect, rotation invariant enforcement                                                                                   |
| Voice                 |         300 |         300 |        0 |                                                                                                                                                              |
| Vision                |         200 |         200 |        0 |                                                                                                                                                              |
| Action                |         700 |         550 |     -150 | run_swift + run_shell removed; AppleScriptTemplates is small                                                                                                 |
| Memory                |         250 |         450 |     +200 | consolidation worker, dual timestamps, structured retrieval, FTS5 wiring                                                                                     |
| Observation           |         250 |         450 |     +200 | 4 new watchdogs (sleep/wake, network, audio config, permission)                                                                                              |
| Policy                |         200 |         250 |      +50 | drift detector subgoal inference call                                                                                                                        |
| SelfModel             |           0 |         150 |     +150 | new module                                                                                                                                                   |
| EffectLog (in Action) |           0 | (in Action) | included | included in Action delta                                                                                                                                     |
| CLI                   |           0 |         100 |     +100 | aura support-bundle                                                                                                                                          |
| Util                  |         150 |         200 |      +50 | AsyncChannel helper                                                                                                                                          |
| **Sum of soft caps**  |       3,700 |       4,850 |   +1,150 |                                                                                                                                                              |
| **Hard cap**          |       3,500 |   **4,500** |   +1,000 | overflow buffer is small                                                                                                                                     |

Several folders will come in lower than their soft cap (Voice, Vision, Conversation, Util are uncomplicated); 4,500 is realistic. If we go over, we cut UI features (timeline > demonstration mode > targeting-replay-detail > everything else), in that order.

## 13. Out of scope (v3.0, deliberate non-features)

- Cross-device memory sync (single-Mac only).
- Multi-user.
- Plugin system.
- Auto-updater. (User updates by re-downloading the DMG.)
- Safari/Arc support (Chrome only via CDP).
- **`run_swift` and `run_shell`.** Cut for v3.0; re-introduce in v3.1 only behind a developer-mode toggle requiring per-invocation user approval.
- **HNSW / quantized embedding index.** v3.0 caps recall to last 30 days; > 50K facts blocks the recall fallback. Documented scaling boundary.
- **Generic predictive layer.** Specific predictions live in calendar policy, drift detector subgoal inference, demonstration playbooks. A separate predictive subsystem is not in v3.0 and may never be — predict-and-prepare is a different product.
- AX-scraping the terminal for build failures (use `aura-shell-hook` instead).
- Cloud anything.
- Telemetry of any kind, automatic update of any kind.

## 14. Risks & their mitigations

| Risk                                                  | Mitigation                                                                                                                                                  |
| ----------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Gemini API key in config file leaks                   | Keychain only.                                                                                                                                              |
| Live API model deprecated mid-development             | `LiveSession` protocol abstracts; CI checks model availability daily.                                                                                       |
| Vendor risk (account suspension, geofence, ToS shift) | Documented as known fragility (§3 + §13). `LiveSession` protocol is the cheap optionality.                                                                  |
| `setupComplete` p99 blows the 300ms gap budget        | Measure first; publish honest SLO.                                                                                                                          |
| Rotation-during-tool-call duplicates an action        | §5.2 invariant 1 + §9.6 effect log + drain deadlines + correlation_id orphan detection.                                                                     |
| ANE contention stalling audio                         | `.cpuOnly` on embeddings. Embedding worker is post-rotation only.                                                                                           |
| AX trees lying on Electron / WKWebView                | Per-tool ground-truth probes; `find_vision_from` falls to OCR.                                                                                              |
| Procedural memory misfires                            | No silent auto-elevation. Demonstration mode = user agency. `pattern.procedural` events are _suggestions_ requiring explicit promote.                       |
| Drift detector hallucinating drift                    | (T-30s, now, inferred-subgoal) inputs; 50-scenario calibration set as ship gate; 60s minimum between whispers.                                              |
| Self-learning poisoning (bad-mood Tuesday rules)      | 24-hour governance gate before any correction enters the system prompt.                                                                                     |
| Sleep/wake state confusion                            | `SleepWakeWatchdog` force-rotates and discards mic ring on wake.                                                                                            |
| Network interface migration                           | `NetworkPathWatchdog` + path-conditioned reconnect (§5.4).                                                                                                  |
| USB audio hot-plug                                    | `AudioConfigWatchdog` reconfigures capture tap.                                                                                                             |
| Permission revoked mid-session                        | `PermissionWatchdog` appends `state.permission_lost`; `Context.swift` injects "X unavailable" into next prompt.                                             |
| Wall clock jumps backwards (NTP, timezone)            | `ts_mono` for ordering; `ts_wall` is display-only.                                                                                                          |
| `vacuum` corrupting the log                           | No `vacuum`. The log grows.                                                                                                                                 |
| Frame-slot race                                       | `OSAllocatedUnfairLock` + atomic exchange.                                                                                                                  |
| Log writer blocked by disk stall                      | Bounded `AsyncChannel` with backpressure; drop `state.observation`, `pattern.*`, `debug.*` first; never drop `turn.*` or `effect.*`.                        |
| User can't debug a bug                                | `aura support-bundle --since=Nh --redact-screenshots` (§14.7).                                                                                              |
| Voice processing IO unavailable                       | Non-fatal: degrade to no echo cancel, log once.                                                                                                             |
| Whisper at the worst possible moment (Zoom interview) | Mute hotkey + right-click menu; whispers off by default in any mode the user hasn't explicitly invoked; no whisper within 5s of detected meeting-app focus. |

### 14.7 `aura support-bundle` CLI (Cantrill + Taleb pass)

> **Antifragility requires a feedback loop. Zero telemetry ≠ zero diagnostics.**

```
aura support-bundle [--since 1h] [--redact-screenshots] [--output ~/Desktop/aura-bug.tgz]
```

Bundles:

- The last N hours of the event log (with screenshot blobs redacted to perceptual hash by default).
- The last 5 crash reports if any (from `~/Library/Logs/DiagnosticReports/` filtered to Aura).
- Platform context: `sw_vers`, `uname -a`, `system_profiler` audio + display + permissions excerpt.
- Active config (API key redacted).
- `Context.swift` fixture-test pass/fail summary.

**The bundle is written to disk and never sent anywhere.** The user inspects, decides, and shares (or doesn't) over whatever channel they prefer. Operator agency, not surveillance.

## 15. What "done for v3.0" means

- Cold launch to first listening state ≤ 1.5s
- Round-trip user-speech to assistant-speech ≤ **1.2s p50**, **2.5s p99**, **5s p99.9**.
- Hard rotation gap ≤ **300ms measured** with `setupComplete` p99 budget published; if p99 > 600ms, the SLO is updated to match.
- **Real torture test passes** (Cantrill-pass, replaces "1 hour smoke test"): an automated harness that runs an 8-hour session against a live Gemini Live endpoint with synthetic injected events: `pmset sleepnow` at minute 73, network interface flip at minute 142, USB audio hot-plug at minute 210, screen-recording permission revoked and re-granted at minute 260, system clock jumped forward 30 min by NTP at minute 311, disk fill to 99% at minute 380. After all of that: every `turn.tool_call` has a paired `turn.tool_result`, every `effect.posted` has a paired `effect.outcome`, no orphan correlation_ids, no out-of-order `ts_mono`, no audio glitch.
- **Rotation fuzz harness passes** (Taleb-pass): 10,000 simulated rotations with synthetic tool calls of varying duration and network jitter. Invariants in §5.2 + §9.6 hold on every iteration. Zero duplications, zero orphan effects.
- All 7 policies have at least one passing functional test.
- **Drift-detector fixture set**: ≤ 15% disagreement with labels on 50 cases (ship gate).
- **Context fixture set**: deterministic prompt assembly, ≤ 4K tokens, all assertions green (ship gate).
- Onboarding completes in < 90 seconds for a fresh user with permissions to grant.
- Binary size ≤ 30 MB (was 25; modest growth absorbed).
- ≤ 4,500 LOC.
- `aura support-bundle` produces a valid bundle in < 2s.

When all 11 are green, v3.0 ships.

---

## Appendix A — What v2 had that v3 deliberately drops

- Rust core + Swift shell → Swift only
- 10 crates → flat Swift modules
- Cloud Run proxy → direct API calls
- Memory agent on Cloud Run + Firestore → local SQLite event log + consolidation worker
- 4 separate memory subsystems → 1 log, 4 lenses + consolidation
- 20 tools with mixed contracts → 22 tools, structured `ToolResult`, AX-proposes/vision-disambiguates targeting
- Soft session handoff → hard rotation defined by invariants + effect-log discipline
- Coordinate-first click → seek-first click with per-tool probes
- Daemon process + IPC → one process
- `run_shell`, `run_swift` → cut entirely (templated AppleScript only)
- Procedural memory via cosine threshold → demonstration mode + consolidation-suggested patterns with explicit user promotion

## Appendix B — Pressure-test sources

**Pass 1:**

- `/carmack` — soft handoff, threading, push-mode contention, AX/hash false positives, vector recall on audio path, trigger supervision.
- `/hickey` — memory complecting, tool result coupling, proactivity duplication, `seek` polysemy, conversation/socket complecting, configuration as scattered mutable state, layout by implementation kind.
- Web research (April 2026) — model ID stability, JXA breakage on Sonoma+, `SCContentSharingPicker` requirement, ANE contention, async function calling semantics, affective dialog gating to 2.5.

**Pass 2:**

- `/carmack` — `setupComplete` latency reality, rotation-during-tool-call race, mic ring sizing, embedding worker scheduling, build-failure detection, AX→OCR→vision ordering, find_vision p99, frame-slot atomicity, SQLite writer thread, 600ms p50 SLO honesty.
- `/hickey` — closed event-kind enum, Policy protocol as function-in-costume, ToolOutcome as closed sum, procedural caching queries as facts, embeddings on the events row complecting, active-goal as mutable global, rotation-as-procedure, three places assembling context.
- `/bret-victor` — memory invisible to its subject, whispers as black-box verdicts, menu bar dot as smoke detector, active goal hidden, tool failures as opaque strings, rotation as missed participation moment, no representation of the conversation as shape, **Mind tab as the missing primary surface**.
- `/karpathy` — state digest amnesia, context budget blowup, drift hallucination without comparison frames, free-text failure reason as agent reward-hacking, find_vision inventing coords, **system prompt missing**, embedding cargo cult, **no eval story**, `Context.swift` as the one thing that has to be right.

**Pass 3 (this revision):**

- `/bryan-cantrill` — no diagnostic story, single timestamp footgun, sleep/wake unhandled, network migration absent, concurrency mix unsettled, vacuum bug-machine, no fsync discipline, smoke-test masquerading as torture-test, permission-revoked-mid-session unhandled, no-telemetry vs no-proprioception. **The one fix:** effect log + correlation_id discipline.
- `/joscha-bach` — four lenses are filters not memory systems, no consolidation loop, no self-model, flat goal string, drift detector lacks theory of mind, purely reactive, system prompt persona deferred. **The one fix:** consolidation worker.
- `/taleb` — single-vendor Google dependency, escape-hatch safety theater, drift detector asymmetric harm backwards, procedural memory has no ruin constraint, self-learning has no governance, dignity-destroying low-prob events unenumerated, TCC + App Store + Google three single-vendor moving targets, no proprioception is suicidal, p50 without p99.9, six-rotation test is turkey confidence. **The one fix (via negativa):** cut `run_swift` and `run_shell`.
- `/bret-victor` (verifier) — Mind tab demoted, controls are decorations, Quiet line is corpse text, rotation digest in 80ms cue is unreadable, three rings on dot encode "busy," targeting visualization is impressionistic, conversation timeline didn't make it, dynamic suffix invisible, 750-LOC budget tells truth (audit log not participation surface). **The one fix:** collapse Now/Mind into one panel, render dynamic suffix live, replace digest cue with continuously-visible strip.

All findings folded in. This is the third-pass spec.
