# Aura v3 — Design Spec

**Date:** 2026-04-29
**Status:** Draft, second-pass — pending user review
**Supersedes:** v1 + v2 (clean-slate rewrite — no code or architecture inherited)
**Reviewers:** `/carmack`, `/hickey`, `/bret-victor`, `/karpathy` (two passes)

---

## 1. The product, in one paragraph

Aura is a local-only macOS voice assistant. You speak, it streams your voice and your screen to Gemini's Live API, and Gemini responds either by speaking back or by directly controlling your Mac — clicking, typing, scrolling, sending mail, running scripts. It remembers what matters across sessions, learns from your corrections, and stays out of your way until you want it. Single Swift app, one binary, ~3.5K lines, no servers, no cloud infra of any kind beyond direct API calls to `generativelanguage.googleapis.com`.

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

This is the lever that collapses what would otherwise be six independent subsystems (episodic memory, semantic memory, behavioral memory, procedural memory, transcript history, observability) into one. The log is the only mutable thing in the persistent layer. "Forget" is an append, not a delete. "Recall" is a select. "Self-learning" is choosing which queries we trust enough to inject.

```sql
-- The source of truth. Append-only. Open schema.
events(
  id            INTEGER PRIMARY KEY,
  ts            INTEGER NOT NULL,           -- unix ms
  kind          TEXT NOT NULL,              -- namespaced string, see §6.1
  payload       TEXT NOT NULL,              -- JSON
  session_id    TEXT,
  tombstone_for INTEGER REFERENCES events(id)
);
CREATE INDEX events_ts ON events(ts);
CREATE INDEX events_kind ON events(kind);
CREATE VIRTUAL TABLE events_fts USING fts5(payload, content='events', content_rowid='id');

-- Derived. Recomputable. Versioned. Throwaway.
event_embeddings(
  event_id      INTEGER NOT NULL REFERENCES events(id),
  model_version TEXT NOT NULL,              -- e.g. 'NLContextualEmbedding-en-2'
  vector        BLOB NOT NULL,              -- 768-dim f32
  PRIMARY KEY (event_id, model_version)
);
```

> **Hickey-pass revision:** embeddings _belong_ to events, but embeddings depend on a model version that will change. Storing the embedding on the events row complects an immutable fact with a derived value subject to model upgrades. Decompose. Drop the `event_embeddings` table to recompute when the model version changes; the log is unaffected.

Memory layers (§6) are _named queries_. Proactivity policies (§7) _subscribe_ to event kinds. The Live session (§5) _appends_ turn events. Tool execution (§8) _appends_ operation events. Nothing else has its own storage.

## 5. The conversation, the transport, the session

A conversation is a value: an ordered sequence of `Turn`s. A `Turn` is one user-input + zero-or-more model-output events. A session is _the WebSocket the next turn happens to be travelling on_. Rotation is **not** a "handoff" — it is opening a new transport for the next turn. The conversation persists in the event log; the transport is incidental.

### 5.1 Live transport

- Endpoint: `wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent`
- Model: `gemini-2.5-flash-native-audio-preview-12-2025` (pinned — see §10.1)
- API version: `v1beta`
- Modalities: `[AUDIO]` in/out, `VIDEO` in
- Voice: `Kore` (configurable via `~/.config/aura/config.toml`)
- VAD: server-side, `START_SENSITIVITY_HIGH`, `END_SENSITIVITY_LOW`, 20ms prefix, 200ms silence — note: the 200ms silence is a hard floor on response latency, see §15
- Compression: sliding window, 500K-token cap (server-managed)
- Tools: ~25 declarations (§8)
- Affective dialog: enabled
- Proactive audio: **disabled at the API level** (we control proactivity ourselves — §7)

### 5.2 Hard rotation — invariants, not procedure

> **Hickey-pass revision:** rotation was originally written as seven mutations. The conversation-as-value framing demands instead that rotation be defined by _invariants_ — properties that must hold across the transition `LiveSession[A] → LiveSession[B]` — and a _transition function_, not a script.

**Invariants:**

1. **No undelivered tool result.** No in-flight `turn.tool_call` from a closing session is allowed to terminate without its `turn.tool_result` being delivered to _some_ socket — either the original (preferred) or, if the original has died, the next.
2. **No replayed audio.** Audio frames captured before the rotation cue are never sent on the new socket.
3. **No mid-utterance cut.** If Gemini is speaking when rotation begins, playback finishes. If Gemini is silent, rotation begins immediately.
4. **No mid-tool-call cut.** Pending tool calls drain with a 2s soft deadline; rotation defers up to 8s of headroom (§5.3) to honor it.
5. **Deterministic digest.** The state digest sent to the new session is a pure function of the log + active goal + pending-tool intent + rolling summary. No Flash call generates it on the rotation path.

**Transition (executes at T = 9 min from session start, or on disconnect):**

1. **Drain pending tool calls.** Wait up to 2s soft, 8s hard. If a tool exceeds 8s, the rotation forces cancellation: append `turn.tool_result` with `failure_reason = .rotationForced`, and _include this synthetic result in the digest_ — this is the only legitimate path for a tool result to enter the digest. (Carmack-pass: rotation duplicating an action is the trust-destroying bug.)
2. **Pause mic.** Frames buffered in a 2s ring (64 KB). Replay is gated on a VAD-active flag at resume — never replay silence into a new server VAD.
3. **Drain playback.** Mid-utterance finishes; otherwise immediate.
4. **Close** the WebSocket cleanly with goodbye frame.
5. **Audio cue.** 80ms 440Hz sine, -24dB. The user knows.
6. **Open** the new WebSocket. Wait for `setupComplete` (measured p50/p95/p99 in dev — see §15).
7. **Send digest.** A pure function of `(log, activeGoal, pendingToolIntent, rollingSummary)` (§9.5).
8. **Resume mic.** Replay buffered VAD-active frames on the new socket if any.

**Time budget:** target a ≤300ms gap from cue end to mic resume _measured_, with `setupComplete` p99 dominating. If `setupComplete` p99 exceeds 600ms in dev measurements, we publish that as the SLO instead of pretending. (Carmack-pass: the 300ms target was wishful.)

No "smart" handoff. No two sockets crossing streams. The cue is honest; the digest is byte-controlled; the invariants are testable.

### 5.3 Graceful end (the user's ask)

When session age exceeds 8 minutes AND user is not actively speaking AND no tool call is pending:

- Aura whispers (low volume, brief): _"Refreshing for the next stretch — give me a sec."_
- Hard rotation begins.

If user speaks during these conditions, defer to the next idle window. Never interrupt mid-turn. At T = 9 min the rotation begins regardless; the 8s tool-drain headroom (§5.2) is the only flexibility allowed.

### 5.4 Reconnect

Exponential backoff: 200ms → 400ms → 800ms → 1600ms → 3200ms, ±25% jitter. Max 5 attempts. After 5, surface to UI: "Aura's offline — check connection." User can manually retry. Session resumption handles are honored when offered by the server (token valid 2 hours), but we never depend on them — the state digest in §5.2 is enough on its own.

## 6. Memory — one log, four lenses

### 6.1 Open kind schema

> **Hickey-pass revision:** event kinds were a closed enum. Information is open. Use namespaced strings; the schema accretes without code changes.

**Conventional kinds (the seed list, not a closed set):**

```
turn.user                 — user said something (transcript)
turn.assistant            — model said something (transcript)
turn.tool_call            — model invoked a tool (operation, args)
turn.tool_result          — tool produced a result (output, optional failure_reason)
state.observation         — system observed a state change (e.g. AX diff after a click)
correction                — user explicitly corrected behavior ("don't", "always")
fact                      — user-profile fact extracted by Flash
goal.active               — user's stated focus goal (replaces mutable activeGoal field)
goal.cleared              — user ended an accountability session
trigger.fired             — proactive trigger emitted (with policy_id + provenance)
session.start, session.end
summary.rolling           — rolling 2-3KB conversation summary, updated turn-by-turn
config.change             — user changed settings
tombstone                 — invalidates an earlier event (for "forget")
```

New kinds are added by writing them. Code that doesn't know a kind passes it through.

### 6.2 Lenses (queries, not tables)

> **Hickey-pass revision:** "Procedural memory" was originally storing candidate playbook events back into the log — caching a query result as a fact. Procedural patterns are now a derived view, computed on demand from the log of `turn.tool_call` + `turn.tool_result` events. No new event kinds for "playbooks."

#### Episodic

```sql
SELECT * FROM events
WHERE ts > :since AND kind LIKE 'turn.%'
ORDER BY ts ASC;
```

Used for: rebuilding the active conversation, audit logs, debugging.

#### Semantic — structured profile primary, embeddings backup

> **Karpathy-pass revision:** 768-dim cosine over fact text is cargo cult for the first 1000 facts. Most useful queries are structured (entity-predicate-value). Use SQL/FTS5 as primary retrieval; keep embeddings as fallback for fuzzy matches.

At session end, a Flash REST call extracts up to 10 facts, structured:

```json
{
  "entity": "user",
  "predicate": "lives_in",
  "value": "Kenya",
  "confidence": 0.9
}
```

Each fact is appended as `kind='fact'`. Embeddings are computed off-path and inserted into `event_embeddings` (§4).

**Primary recall (used by `Context.swift` §9.5):**

```sql
-- Structured query first: deterministic, debuggable.
SELECT id, payload FROM events
WHERE kind = 'fact' AND tombstone_for IS NULL
  AND json_extract(payload, '$.entity') = :entity
ORDER BY ts DESC LIMIT 16;
```

**FTS5 fallback for keyword recall:**

```sql
SELECT events.id, events.payload FROM events
JOIN events_fts ON events_fts.rowid = events.id
WHERE events_fts MATCH :keywords
  AND events.kind = 'fact'
  AND events.tombstone_for IS NULL
LIMIT 16;
```

**Embedding fallback (only when keyword + structured both return < 4 hits):**

```sql
-- Use the latest model_version's vectors.
SELECT events.id, events.payload
FROM events JOIN event_embeddings em ON em.event_id = events.id
WHERE events.kind = 'fact' AND events.tombstone_for IS NULL
  AND em.model_version = :current_model
ORDER BY cosine(em.vector, :query_embedding) DESC
LIMIT 8;
```

All three pass through a recency × frequency × citation prior before being injected into the prompt. (Karpathy-pass: top-K cosine is amnesia at scale.)

#### Behavioral (the corrections lens)

A small in-process pattern matcher scans every `turn.user` for negation/imperative correction patterns ("no", "stop", "don't", "always", "never", "use X not Y"). When matched, the floating panel surfaces a one-tap confirmation: _"Should I remember: `<rule>`?"_ On confirmation, append `kind='correction'`.

Active rules are concatenated into the system prompt at session start by `Context.swift` (§9.5), under "Things this user has asked you to remember:". Capped at 30 rules; oldest non-recently-used drop off after 90 days. Only rules used in the last 14 days are included by default; the rest are available via FTS5 lookup if Gemini calls `recall(query)`.

#### Procedural — derived view, not stored facts

A query over `turn.tool_call` + `turn.tool_result` finds successful sequences (≥ 3 calls, no failures, no user interruption). When the user's preceding utterance has cosine > 0.85 with three or more such sequences, the sequence is a _trusted playbook_ — surfaced to Gemini as an additional tool `run_playbook(name)` for the duration of the session. Computed at session start; not persisted as new events.

**Opt-in.** If after 30 days of opt-in no playbook has been used twice, the feature is silently disabled and the user is told: _"I tried learning your routines and didn't find clean patterns — turning that off."_ Honest failure beats hidden creep.

### 6.3 Embeddings

- API: `NLContextualEmbedding` (English, 768-dim, on-device CoreML).
- `MLModelConfiguration.computeUnits = .cpuOnly` — research finding: ANE contention with the live audio path causes 200-400ms stalls.
- **Computed off the rotation path.** Embedding pass for new facts runs on a low-priority background queue _after_ the new socket is hot — never before. (Carmack-pass.)
- Cosine similarity computed with `Accelerate.vDSP` (SIMD, ~5µs per pair on M-series).
- Latency budget for embedding pass at session end: measured in dev; if > 1s for 10 facts, parallelize.
- Stored in `event_embeddings(event_id, model_version, vector)`. Recomputable when the model version changes; the log is unaffected. (Hickey-pass.)

### 6.4 "Forget" semantics

`forget(id)` appends a `tombstone` event referencing `id`. All queries filter `tombstone_for IS NULL`. Storage is never reclaimed automatically; a `vacuum` command (CLI subcommand of the binary) does compaction if the user asks.

## 7. Proactivity — one pipeline, many policies

```
ObservationStream  ──►  [policy₁, policy₂, …, policyₙ]  ──►  Action
```

An `Observation` is a value:

```swift
struct Observation {
  let ts: Date
  let kind: String          // namespaced, see §6.1's open schema
  let context: [String: JSONValue]
}
```

A `Policy` is a function — no protocol, no class, no inheritance:

```swift
typealias Policy = (Observation, HistoryView) async -> Action?
```

> **Hickey-pass revision:** the `Policy` protocol was a function in a costume. Use the function. Compose with combinators when needed.

An `Action` is `whisper(text, provenance)` or `notify(payload)` or `runTool(name, args)`. Whispers carry full provenance metadata so the user can inspect the chain in the Mind tab (§11.3).

### 7.1 The shipped policies

| Policy                | Mode                | Trigger / mechanism                                                                                                                                                                                                                                                                                                                                            |
| --------------------- | ------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `UserInvokedPolicy`   | always              | `kind == "user.invoked"` (⌘⇧A pressed). Action: dump notes buffer.                                                                                                                                                                                                                                                                                             |
| `CalendarPolicy`      | always              | `EventKit` predicate, event in next 5 min. Dedupe: per-event-id + recurrence-instance-id. Whisper 1× per event.                                                                                                                                                                                                                                                |
| `BuildFailurePolicy`  | always              | macOS UserNotifications observer for "Build failed" / "Tests failed" notifications from Xcode, plus a tiny bundled CLI shim (`aura-shell-hook`) the user opts into in `.zshrc` to publish exit codes. AX-scraping the terminal is **out of scope**; if the user doesn't install the shim, the policy is dormant. (Carmack-pass: spec the mechanism or cut it.) |
| `ErrorDialogPolicy`   | always              | AX detects modal error dialog within 2s of last tool call. Action: whisper + offer dismiss.                                                                                                                                                                                                                                                                    |
| `IdlePolicy`          | accountability-only | idle > 30 min (`IOKit` HID idle). Action: whisper.                                                                                                                                                                                                                                                                                                             |
| `AppCrashPolicy`      | always              | `NSWorkspace` crash notification. Action: notify (no whisper).                                                                                                                                                                                                                                                                                                 |
| `DriftDetectorPolicy` | accountability-only | every 30s, ask Flash on a _separate_ `URLSession` with 2s hard deadline. Prompt is a **comparative** judgment over `(screenshot @ T-30s, screenshot @ now, goal)`, not a single frame. (Karpathy-pass.) Action: whisper if drift detected (with provenance).                                                                                                   |

**Calibration set (ship gate):** 50 hand-labeled `(goal, screen-before, screen-now, expected-drift?)` tuples live in `Tests/DriftFixtures/`. The test target runs them on every prompt change. Fail rate > 15% blocks ship. (Karpathy-pass.)

Each policy is an independent supervised `Task`, restarted on crash with exponential backoff (1s → 60s capped). One policy failing does not affect others. Each policy's last error is observable in the Mind tab.

### 7.2 Modes

> **Hickey-pass revision:** the active goal was a mutable global field. Now it's an event of `kind='goal.active'`; modes query it.

The active _policy set_ is a value (`Set<Policy.ID>`). Modes are presets:

- **Default**: `UserInvoked` + `Calendar` + `BuildFailure` + `ErrorDialog` + `AppCrash`.
- **Accountability**: above + `Idle` + `DriftDetector`. Activated by user saying _"focus me on X for the next N minutes"_. Aura appends `kind='goal.active' { value: "X", duration: N }` to the log; on N expiring, `kind='goal.cleared'` is appended. Querying "the active goal at time T" is a select. No mutable field.

The user's words can change the policy set; _that's_ the toggle.

## 8. Action — tools as actions on the world

### 8.1 The contract

> **Hickey + Karpathy joint revision:** the closed `ToolOutcome` enum couldn't grow. The `failed(reason: String)` free-text reason was a reward-hacking surface for the agent. Restructure: a `ToolResult` map with structured `status` and `failure_reason`, plus free-text `detail` for humans.

```swift
struct ToolResult {
  let status: Status                       // .ok | .needsClarification | .failed
  let output: JSONValue?                   // present iff status == .ok
  let question: String?                    // present iff status == .needsClarification
  let failureReason: ToolFailureReason?    // present iff status == .failed (structured)
  let detail: String?                      // human-readable, all statuses
  let confidence: Double?                  // optional (some tools have a meaningful confidence)
}

enum ToolFailureReason: String, Codable {
  case noStateChange           // we acted but nothing observably changed
  case targetNotFound          // locate / find_* couldn't resolve
  case timeout
  case permissionDenied
  case preconditionFailed      // app not running, file missing, etc.
  case rotationForced          // the only path tool_results reach the digest
  case toolError(code: String) // catch-all with structured code
}
```

The agent dispatches off `status` + `failureReason`. The user reads `detail`. The map is open — fields can be added without breaking old call sites.

The system independently appends `state.observation` events. Tools do their job; observers observe.

### 8.2 Targeting — composition, not parallel strategies

> **Carmack + Karpathy convergent revision:** the original ordering AX → OCR → vision had OCR in the middle (latency trap). And `find_vision` was asked to _invent_ coordinates — a hallucination surface. Reorder; have vision _pick from_ AX-detected candidate regions; OCR is last resort.

```swift
func find_ax(_ description: String) async -> [AXCandidate]
func find_vision_from(candidates: [AXCandidate], description: String, fullScreen: CGImage)
    async -> AXCandidate?
func find_ocr(_ description: String) async -> OCRTarget?

// Default composer used by Gemini.
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

**Truth conditions:**

- `find_ax` — structural; returns _all_ AX candidates with role+label match. Cheap (~5ms).
- `find_vision_from` — Gemini Flash REST: given a screenshot + N labeled candidate regions + description, returns _which candidate_ (or none). Bounded set means hallucination floor is zero by construction. p99 budget: 3s hard cap, in-flight dedup. Circuit breaker: 3 fails → 30s cooldown. (Carmack + Karpathy.)
- `find_ocr` — last resort when AX returns nothing (web content, Electron). `VNRecognizeTextRequest` against a fresh capture.

The closed set of `find_vision_from` candidates is the key safety move. We don't ask the model to invent pixels; we ask it to pick from an enumerated list.

### 8.3 Tool catalogue

#### Interaction (5)

`click(target)`, `type(text)`, `key(combo)`, `scroll(target, direction, amount)`, `drag(from, to)`

Each has a **per-tool ground-truth probe**:

- `click` → after CGEvent post, poll AX focus / pixel hash for change; on no change, retry once via keyboard shortcut if applicable.
- `type` → read `AXValue` of focused element after typing; verify substring match on what we typed.
- `key` → check AX focus or active app changed.
- `scroll` → AX scroll-position changed.
- `drag` → both endpoints' AX states changed.

If verification fails, return `failureReason = .noStateChange` with `detail = "posted CGEvent, no observable change"`. Don't lie to Gemini.

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
- `browser.eval(js)` — returns JSON
- `browser.find(selector or text)` — DOM-side, no AX involvement

Chrome must be launched with `--remote-debugging-port=9222`. Aura detects this and prompts the user to enable it (one click in onboarding). Safari/Arc support: deferred to v3.1.

#### Escape hatch (3, sandboxed)

- `run_applescript(source)` — 10s timeout, 10KB output cap. **Blocks**: `do shell script`, `run script`, AppleScript-concatenated obfuscation patterns.
- `run_shell(command)` — 10s timeout, 10KB cap. **Blocks**: `|`, `;`, `` ` ``, `$()`, `>`, `<`, `&&`, `||`, `sudo`. Writes restricted to `~/Documents`, `~/Downloads`, `~/Desktop`, `~/Library/Caches/aura/`.
- `run_swift(source)` — `swift -e` one-shot. Same timeouts and write restrictions.

First call to any escape-hatch tool in a session prompts the user inline: _"Aura wants to run a Swift snippet. Show me what / Approve / Always allow this session."_

#### Memory (3)

- `remember(fact)` — appends a manual `kind='fact'` event with full embedding.
- `recall(query)` — returns top-N matching facts (structured + FTS5 + embedding fallback per §6.2).
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
│   – Menu bar, FloatingPanel, Mind tab, Onboarding                 │
│   – Never blocks > 16ms                                           │
├──────────────────────────────────────────────────────────────────┤
│ Realtime audio (CoreAudio render context, RT-priority)            │
│   – Owns mic ring buffer (lock-free SPSC, 2s capacity)            │
│   – Owns playback ring buffer                                     │
│   – No actor hops, no allocations, no system calls                │
├──────────────────────────────────────────────────────────────────┤
│ ScreenCaptureKit queue (SCK-owned dispatch queue)                 │
│   – On every frame: hash, drop if unchanged, drop if backpressure │
│   – Single-slot "next frame to send", protected by                │
│     OSAllocatedUnfairLock around an atomic-exchange of the buffer │
│     reference. (Carmack-pass: no hand-wave on the race.)          │
├──────────────────────────────────────────────────────────────────┤
│ Network writer (single Task, NOT a dispatch queue — pick one)     │
│   – Sole writer to the WebSocket                                  │
│   – Drains: audio chunks (every 100ms), screen frames (changed),  │
│     tool results                                                   │
├──────────────────────────────────────────────────────────────────┤
│ Log writer (its own Task, bounded queue with backpressure)        │
│   – Sole writer to SQLite (WAL mode)                              │
│   – Backpressure policy: drop state.observation events first;     │
│     never drop turn.* events. (Carmack-pass.)                     │
├──────────────────────────────────────────────────────────────────┤
│ Tool dispatch (Task pool, max 8 concurrent)                       │
│   – Never blocks; tools are async by contract                     │
├──────────────────────────────────────────────────────────────────┤
│ Policy host (one Task per Policy, supervised)                     │
│   – Drift detector uses a *separate* URLSession                   │
│   – 2s hard deadline on Flash calls                               │
├──────────────────────────────────────────────────────────────────┤
│ Embedding worker (low-priority background Task)                   │
│   – Runs after new socket is hot post-rotation, never before      │
└──────────────────────────────────────────────────────────────────┘
```

**Forbidden patterns** (will fail review):

- Awaiting on the audio thread.
- Calling Cocoa APIs from any non-main thread.
- Sharing the realtime URLSession with policy/Flash calls.
- Holding a lock across `await`.
- Mixing actor-isolation with dispatch-queue serialization on the same object.

### 9.5 `Context.swift` — the model-context module

> **Karpathy-pass: "the one ML thing this design has to get right."** All model-facing context is assembled in _exactly one place_. No other module reads cross-kind from the log to produce prompt material. Everything else queries one kind at a time.

**Two pure functions:**

```swift
func sessionStartContext(now: Date, log: LogView, mode: Mode) -> SystemPrompt
func rotationDigest(activeConversation: Conversation,
                    activeGoal: GoalEvent?,
                    pendingTool: ToolCall?,
                    log: LogView) -> Digest
```

Both deterministic given inputs. Both tested against fixtures in `Tests/ContextFixtures/`.

**Architecture:** `SystemPrompt` is built as `stable_prefix + dynamic_suffix` so Gemini's prompt cache rewards us. The stable prefix is the persona, tool-routing guidance, refusal rules, examples, and the structured `ToolFailureReason` semantics. The dynamic suffix is recently-active corrections + a small structured user profile (entity-predicate-value, capped at 20) + the active goal if any.

**Token budget:**

- Target: 2K tokens for the system prompt.
- Hard cap: 4K. If exceeded, drop the oldest non-recently-cited corrections; log a warning.
- Eval set in `Tests/ContextFixtures/` measures token count + assertion-based content checks (every shipped correction rule must reduce a known failure scenario).

**Rolling summary:** `Context.swift` also runs the per-turn rolling-summary update — one cheap Flash call against the last summary + new turns, kept ≤ 3KB. This `summary.rolling` event is what `rotationDigest` ships, not raw turns. (Karpathy-pass: raw turns are amnesia.)

This single module is the system's prompt-engineering surface. Iterating on prompts touches one file. The eval runs on every change to it.

## 10. Platform pinning

### 10.1 Gemini

- Model: `gemini-2.5-flash-native-audio-preview-12-2025` (Developer API path; AI Studio key)
- API version: `v1beta`
- Endpoint: `wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent`
- Flash REST: `gemini-2.5-flash` for fact extraction, drift detection, vision pick-from-candidates, rolling summary
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

> **Bret-Victor-pass revision:** the original UI was a status indicator and a transcript. The user could neither see what Aura believed about them, nor inspect why a whisper happened, nor steer the model's understanding. The Mind tab below is the design's primary participatory surface.

Four surfaces. Total Swift UI code budget: **750 LOC** (was 500; the Mind tab is worth the bump).

### 11.1 Menu bar — instrument panel, not status light

`NSStatusItem` with a small custom-drawn icon, not just a colored dot:

- A tiny ring around the dot ticks down to next session rotation (the user _sees_ time).
- Faint waveform inside when the mic is active (the user _sees_ their voice being heard).
- Second small ring shows screen-capture cadence.
- A tool-class glyph appears for the duration of any tool execution.

Color encodes coarse state: gray (idle), green (listening), blue (speaking), amber (rotating), red (offline). The glyphs encode the rest. Click → opens panel. Right-click → mute mic / mute proactive / quit.

### 11.2 Floating panel — `NSPanel`, ⌘⇧A toggle

Two tabs: **Now** and **Mind**.

**Now** (default tab): live transcript scroll, last 6 turns, text-input fallback, "Anything?" button, settings sprocket. The active goal string is rendered in the panel header — always visible, editable inline. (Bret-Victor: "the highest-leverage single value, hidden.")

**During rotation cue**: a glanceable element shows the digest about to be sent — _"Carrying forward: <goal> + last summary."_ One tap to edit goal before send. If the user does nothing, it sends. (Bret-Victor: rotation as participation moment.)

### 11.3 Mind tab — the user's reflection in the system

A primary, scrollable, faceted view of the event log. Sections, in order:

1. **What I'm working on:** the active `goal.active` event, editable inline. Setting it from here is equivalent to saying it.
2. **Things I think I know about you:** facts, sourced — each row expands to show the turn that produced it. One-tap _forget_.
3. **Things you've told me to do or not do:** corrections, with last-fired timestamp and use-count. Thumbs-down disables; long-press deletes.
4. **What I just did:** recent `turn.tool_call` + `turn.tool_result` rows. Each expands to a Targeting view: a snapshot at the time of the call, with AX bounding boxes drawn, OCR matches highlighted, vision oracle's pick marked. Failed calls show _why_ — which strategy failed, what was offered. Thumbs-down on a tool call appends a `correction`.
5. **Whispers:** every whisper is a row. Expands to its full provenance chain — observation → policy → reasoning → action. Thumbs-down disables that policy's whispers for the rest of the session.
6. **Quiet:** a small line — _"DriftDetector: last checked 18s ago — aligned. CalendarPolicy: armed, next event 14:30."_ Silence becomes legible.

This is the steering interface. The substrate is already there; the tab faces the log toward the user.

### 11.4 Onboarding

Single SwiftUI window; 5 steps: API key → mic permission → `SCContentSharingPicker` → Accessibility → "say hi to Aura."

## 12. Code shape

```
Aura.app/
  Sources/
    App/                 AuraApp.swift, AppState.swift, Config.swift, Keychain.swift
    UI/                  MenuBar.swift, FloatingPanel.swift, MindTab.swift,
                         Targeting+View.swift, Onboarding.swift
    Conversation/        Conversation.swift, Turn.swift
    Context/             Context.swift                     // §9.5 — sole prompt assembler
    Transport/           LiveTransport.swift, Reconnect.swift, AudioCue.swift,
                         Rotation.swift                    // invariants + transition fn
    Voice/               Capture.swift, Playback.swift, BargeIn.swift, RingBuffer.swift
    Vision/              ScreenStream.swift, AXTree.swift, Hash.swift
    Action/              ToolRegistry.swift, ToolResult.swift,
                         Targeting.swift,                  // find_ax / find_vision_from / find_ocr / locate
                         Interaction.swift,
                         AppNative.swift,
                         Browser.swift,
                         EscapeHatch.swift
    Memory/              Log.swift,                        // events + writer Task
                         Embeddings.swift,                 // event_embeddings + bg compute
                         Lenses.swift,                     // episodic / semantic / behavioral / procedural
                         Forget.swift
    Observation/         Observation.swift, ObservationStream.swift,
                         AXObserver.swift, IdleClock.swift, CalendarWatcher.swift,
                         BuildHook.swift,                  // shell-hook reader, optional
                         AppCrashWatcher.swift
    Policy/              Policies.swift                    // all 7 as functions in one file
    Util/                JSONValue.swift, Cosine.swift, Backoff.swift
  Resources/             Info.plist, Aura.entitlements, AppIcon.icns
  Tests/                 — eval fixtures in DriftFixtures/, ContextFixtures/
                         — unit tests for Memory/Lenses, Policy, Targeting,
                           Rotation invariants
```

Total LOC budget: **3,500** (hard cap). Per-folder soft caps: App 250, UI 750, Conversation 100, Context 200, Transport 350, Voice 300, Vision 200, Action 700, Memory 250, Observation 250, Policy 200, Util 150. Sum ≈ 3,700; small overrun expected and absorbed by Conversation/Util shrinkage. If any cap is hit beyond +10%, that folder is wrong.

## 13. Out of scope (v3.0)

- Cross-device memory sync (single-Mac only, on purpose)
- Multi-user
- Plugin system
- Auto-updater
- Safari/Arc support (Chrome only via CDP)
- Procedural memory enabled by default (it's an opt-in experiment)
- Local LLM (no MLX, no Ollama — Gemini Live is the brain)
- AX-scraping the terminal for build failures (use the optional `aura-shell-hook` instead)
- Cloud anything

## 14. Risks & their mitigations

| Risk                                                   | Mitigation                                                                                               |
| ------------------------------------------------------ | -------------------------------------------------------------------------------------------------------- |
| Gemini API key in config file leaks                    | Keychain only; no fallback to file.                                                                      |
| Live API model deprecated mid-development              | Pinned in `Config`; CI check fetches `models?key=` daily during dev to catch deprecation.                |
| `setupComplete` p99 blows the 300ms gap budget         | Measure first; publish honest SLO. If p99 > 600ms, change the gap target rather than the implementation. |
| Rotation-during-tool-call duplicates an action         | §5.2 invariant 1 + 2s soft / 8s hard drain + synthetic `.rotationForced` result in digest.               |
| ANE contention stalling audio                          | `.cpuOnly` on embeddings. Embedding worker is post-rotation only.                                        |
| AX trees lying on Electron / WKWebView                 | Per-tool ground-truth probes catch silent-no-op clicks; `find_vision_from(candidates: [])` falls to OCR. |
| Procedural memory drift                                | Off by default; auto-disables after 30 days of low utility.                                              |
| Drift detector spamming whispers / hallucinating drift | (T-30s, now) comparison prompt; 50-scenario calibration set as ship gate; 60s minimum between whispers.  |
| Build-failure hook unavailable                         | Policy is dormant if user doesn't install `aura-shell-hook`; documented in onboarding.                   |
| User turns Aura off and we never know                  | We don't run telemetry — that's the price of "no cloud shit." Accepted.                                  |
| Voice processing IO unavailable on some Macs           | Non-fatal: degrade gracefully to no echo cancel, log once.                                               |
| Frame-slot race between SCK queue and network writer   | `OSAllocatedUnfairLock` around the slot pointer; atomic exchange on swap.                                |
| Log writer blocked by disk stall                       | Bounded queue with backpressure; drop `state.observation` first, never `turn.*`.                         |

## 15. What "done for v3.0" means

- Cold launch to first listening state ≤ 1.5s
- Round-trip user-speech to assistant-speech ≤ **1.2s p50** (Carmack-pass: 200ms server-side VAD silence + 100ms framing + WS hop + inference + audio out + 40ms playback prebuffer makes <1s impossible. Honest SLO over wishful one.)
- Hard rotation gap ≤ **300ms measured** with `setupComplete` p99 budget published; if p99 > 600ms, gap SLO is updated to match
- 1 hour continuous use with at least 6 rotations, no crash, no leak, no audio glitch, no duplicated tool action (the §5.2 invariant test)
- All 7 policies have at least one passing test
- Drift-detector fixture set: ≤ 15% disagreement with labels on 50 cases (ship gate)
- Context fixture set: deterministic prompt assembly, ≤ 4K tokens, all assertions green (ship gate)
- Onboarding completes in < 90 seconds for a fresh user with permissions to grant
- Binary size ≤ 25 MB
- ≤ 3,500 LOC

When all 10 are green, v3.0 ships.

---

## Appendix A — What v2 had that v3 deliberately drops

- Rust core + Swift shell → Swift only
- 10 crates → flat Swift modules
- Cloud Run proxy (`aura-proxy`) → direct API calls
- Memory agent on Cloud Run + Firestore → local SQLite event log
- 4 separate memory subsystems → 1 log, 4 lenses (with procedural as derived view)
- 20 tools with mixed contracts → ~25 tools, structured `ToolResult`, AX-proposes/vision-disambiguates targeting
- Soft session handoff → hard rotation defined by invariants
- Coordinate-first click → seek-first click with per-tool probes
- Daemon process + IPC → one process

## Appendix B — Pressure-test sources

**First pass:**

- `/carmack` — flagged the soft handoff, threading model, push-mode contention, AX/hash false positives, vector recall on audio path, trigger supervision.
- `/hickey` — flagged memory complecting, tool result coupling, proactivity duplication, `seek` polysemy, conversation/socket complecting, configuration as scattered mutable state, layout by implementation kind.
- Web research (April 2026) — model ID stability, JXA breakage on Sonoma+, `SCContentSharingPicker` requirement, ANE contention with concurrent CoreML, async function calling semantics, affective dialog gating to 2.5.

**Second pass:**

- `/carmack` — `setupComplete` latency budget, rotation-during-tool-call duplication race, mic ring buffer sizing, embedding worker scheduling, build-failure mechanism unspecified, AX→OCR→vision ordering wrong, `find_vision` p99 budget, frame-slot race hand-wave, SQLite writer thread missing, 600ms p50 SLO dishonest.
- `/hickey` — closed event-kind enum, Policy protocol as function-in-costume, ToolOutcome as closed sum, procedural caching queries as facts, embeddings on the events row complecting derived with source, active-goal as mutable global, rotation-as-procedure vs rotation-as-invariant, three places assembling model context.
- `/bret-victor` — memory invisible to its subject, whispers as black-box verdicts, menu bar dot as smoke detector, active goal hidden, tool failures as opaque strings, rotation as missed participation moment, no representation of the conversation as shape, **Mind tab as the missing primary surface**.
- `/karpathy` — state digest amnesia (last 2 turns), context budget blowup, drift detector hallucination without comparison frames, free-text failure reason as agent reward-hacking surface, find_vision inventing coords vs picking from candidates, **system prompt entirely missing from spec**, embedding cargo cult vs structured retrieval, **no eval story**, the `Context.swift` module as the one thing that has to be right.

All findings folded in. This is the second-pass spec.
