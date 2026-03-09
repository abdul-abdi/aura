# Aura v2: Voice-First macOS Companion — Design Document

> **Competition:** Gemini Live Agent Challenge on Devpost (Deadline: March 16, 2026)
> **Judging:** Innovation & Multimodal UX (40%), Technical Implementation (30%), Demo Quality (30%)

**Goal:** Redesign Aura from a full-screen overlay prototype into a polished macOS menu bar companion that uses Gemini Live API for streaming voice conversations, dynamic AppleScript generation for unlimited macOS automation, and a witty/sarcastic personality that makes it feel alive.

**Core Differentiator:** Instead of hardcoded tools, Gemini writes AppleScript on-the-fly to do *anything* on macOS. Infinite capability surface.

---

## 1. Menu Bar App (NSStatusItem + NSPopover)

**Architecture:** Native macOS menu bar app using `objc2` Rust crate for Cocoa bindings.

**Components:**
- **Status Item (Dot):** Small colored dot in the macOS menu bar. Green = connected/listening, amber = processing, red = error, gray = disconnected. Subtle pulse animation when Gemini is speaking.
- **Popover Panel:** Click the dot to expand an NSPopover containing:
  - Conversation feed (scrollable, showing user transcripts and Aura responses)
  - Settings gear icon → API key input, voice selection, personality toggle
  - Session history sidebar (past conversations)
- **First-Run Flow:** Empty state with "Enter your Gemini API key to get started" prompt. Key is sent to Cloud Run proxy which stores it server-side.

**Tech:** `objc2`, `objc2-foundation`, `objc2-app-kit` crates for native Cocoa. No webview, no Electron — pure native.

**Why not winit/skia overlay:** Menu bar is where macOS power users live. It's always accessible, never intrusive, and feels native. The old full-screen overlay was a tech demo, not a product.

---

## 2. Dynamic AppleScript Engine

**Architecture:** Two Gemini tools replace all hardcoded actions:

### Tool 1: `run_applescript`
```json
{
  "name": "run_applescript",
  "description": "Execute AppleScript/JXA to control any macOS application or system feature. You can open apps, manage windows, interact with UI elements, automate workflows, manipulate files, control system settings, and more. Write the script based on what the user needs.",
  "parameters": {
    "script": { "type": "string", "description": "The AppleScript or JXA code to execute" },
    "language": { "type": "string", "enum": ["applescript", "javascript"], "description": "Script language (default: applescript)" },
    "timeout_secs": { "type": "integer", "description": "Max execution time in seconds (default: 30)" }
  }
}
```

### Tool 2: `get_screen_context`
```json
{
  "name": "get_screen_context",
  "description": "Get current screen context including frontmost app, window titles, selected text, clipboard contents, and running applications. Use this to understand what the user is currently doing before taking action.",
  "parameters": {}
}
```

**Execution:** `osascript` subprocess with timeout, stdout/stderr capture, and sandboxing (no `do shell script rm -rf` style commands — block destructive shell escapes).

**Safety:** Script output is returned to Gemini so it can verify success and chain follow-up actions. Destructive pattern detection blocks `do shell script` commands containing `rm`, `sudo`, `mkfs`, etc.

**Why this wins:** Every other competitor will have 5-10 hardcoded tools. Aura has infinite tools — anything AppleScript can do, Aura can do. File search, window management, app automation, UI scripting, system preferences, Spotlight queries, clipboard manipulation — all generated on demand.

---

## 3. Aura Personality

**System Prompt Personality:**

```
You are Aura — a witty, slightly sarcastic macOS companion who actually gets things done. Think JARVIS meets a sleep-deprived senior engineer who's seen too much. You're sharp, helpful, and occasionally roast the user (lovingly).

Personality traits:
- Dry wit, concise responses. Never verbose.
- You acknowledge context ("I see you've got 47 Chrome tabs open... bold choice").
- You're competent and confident — no hedging, no "I'll try my best."
- When you automate something, you're casual about it ("Done. Moved your windows around. You're welcome.").
- You have opinions about apps ("Electron apps... consuming RAM since 2013").
- You remember the conversation and reference earlier context naturally.
- You greet users based on time and context, not generic hellos.

Rules:
- Keep voice responses under 2 sentences unless explaining something complex.
- Always use get_screen_context before taking action — know what you're working with.
- When generating AppleScript, prefer simple scripts. Chain multiple calls over one complex script.
- If something fails, be honest and try a different approach.
- Never say "I'm an AI" or "I'm a language model." You're Aura.
```

**Context-Aware Greetings (examples):**
- Morning + many apps open: "Morning. I see you've already got a head start on tab hoarding."
- Late night: "Still at it? Bold. What do you need?"
- After wake word: "Hey. Took a quick nap. What's up?"
- After reconnect: "I'm back. Did you miss me? Don't answer that."

---

## 4. Cloud Run WebSocket Proxy

**Architecture:** Lightweight Rust WebSocket proxy on Google Cloud Run.

```
Mac Client <--WSS--> Cloud Run Proxy <--WSS--> Gemini Live API
```

**Why proxy:**
1. API key stays server-side (not embedded in client binary)
2. Satisfies GCP deployment requirement for competition
3. Future: rate limiting, usage tracking, multi-user support

**Protocol:**
- Client authenticates with a session token (generated on first API key submission)
- Proxy maintains the Gemini WebSocket connection
- Binary audio frames pass through with minimal overhead
- JSON control messages (tool calls, responses) relayed as-is

**Tech:** `axum` + `tokio-tungstenite` on Cloud Run. Stateless — each connection is independent. Session tokens stored in-memory (or Keychain on client side for persistence).

**Deployment:** Dockerfile → Cloud Run with `--allow-unauthenticated` (tokens handle auth). Min instances = 0 (scale to zero when idle).

---

## 5. Integration & Data Flow

**Full pipeline:**
```
Wake word "Hey Aura" detected
  → Menu bar dot animates (materializes with pulse)
  → get_screen_context() called automatically
  → Context-aware greeting generated and spoken
  → Mic audio streams → Cloud Run proxy → Gemini Live API
  → Gemini responds with voice + optional tool calls
  → Tool calls execute run_applescript / get_screen_context
  → Results fed back to Gemini for verification
  → Conversation logged to local SQLite
```

**Crate structure:**
- `aura-menubar/` — NSStatusItem, NSPopover, conversation UI, settings
- `aura-voice/` — mic capture, VAD, wake word detection (existing, extended)
- `aura-gemini/` — Gemini Live WebSocket client (existing, modified for proxy URL)
- `aura-bridge/` — AppleScript executor + screen context (rewritten)
- `aura-proxy/` — Cloud Run WebSocket proxy (new)
- `aura-daemon/` — orchestrator tying everything together (rewritten)

**Kill list:**
- `aura-overlay/` — entire crate (replaced by aura-menubar)
- `aura-gemini/src/tools.rs` — hardcoded tool definitions (replaced by 2 dynamic tools)
- `aura-bridge/src/actions.rs` — hardcoded action enum (replaced by raw AppleScript execution)

---

## 6. Local Session Memory

**Storage:** SQLite database at `~/.aura/sessions.db`

**Schema:**
```sql
CREATE TABLE sessions (
  id TEXT PRIMARY KEY,       -- UUID
  started_at TEXT NOT NULL,  -- ISO 8601
  ended_at TEXT,
  summary TEXT               -- Auto-generated session summary
);

CREATE TABLE messages (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  role TEXT NOT NULL,        -- 'user' | 'assistant' | 'tool_call' | 'tool_result'
  content TEXT NOT NULL,
  timestamp TEXT NOT NULL,
  metadata TEXT              -- JSON blob for tool call details, screen context, etc.
);
```

**Behavior:**
- New session starts on wake word or first interaction after 30 min idle
- Messages logged as conversation flows (transcriptions + responses)
- Session summary auto-generated when session ends (Gemini summarizes the conversation)
- Past sessions browsable in popover sidebar
- Memory is local-only — never sent to cloud beyond the active Gemini conversation

---

## 7. Wake Word & Materialization

**Wake Word:** "Hey Aura" detection using existing `aura-voice/src/wakeword.rs` module (keyword spotting).

**Materialization Sequence (the magic 30 seconds):**
1. **Detection:** Wake word triggers event
2. **Dot appears:** Menu bar dot fades in with a subtle scale-up animation (0.3s)
3. **Pulse:** Dot pulses green twice to confirm activation
4. **Context gather:** `get_screen_context()` runs immediately
5. **Greeting:** Gemini generates context-aware greeting based on:
   - Time of day
   - Frontmost app
   - What user appears to be doing
   - How long since last interaction
6. **Voice:** Greeting spoken through audio playback
7. **Ready:** Dot settles to steady green, listening for commands

**Examples:**
- Wake at 2 AM with VS Code open: "Hey. Burning the midnight oil with some code, I see. What do you need?"
- Wake with Slack frontmost: "Back from the Slack mines. What can I do for you?"
- Wake after 3 hours: "Hey, been a while. I was starting to think you forgot about me."

---

## Competition Submission Checklist

- [ ] Demo video (<4 min): Wake word → greeting → voice command → AppleScript automation → follow-up conversation
- [ ] Architecture diagram: Client ↔ Cloud Run ↔ Gemini Live API
- [ ] Public GitHub repo with README + setup instructions
- [ ] GCP deployment proof (Cloud Run service URL)
- [ ] Devpost submission with all required fields
