# Context Overload Prevention

## Problem

Long-running Gemini Live sessions disconnect within 5 minutes, and each reconnect dies faster due to context snowballing. Root causes:

1. **Screenshots sent regardless of change** — 2fps × ~516 tokens/frame = ~62K tokens/minute even when screen is static
2. **Session resumption restores bloated context** — reconnecting with the stored handle preserves all accumulated screenshots/tools, each reconnect starts closer to the limit
3. **No tool response truncation** — `get_screen_context` AX dumps can be thousands of tokens, accumulate permanently
4. **SlidingWindow unconfigured** — `contextWindowCompression: { slidingWindow: {} }` has no `targetTokens`, relying on unknown server defaults

## Solution

### 1. Screenshot Gating — Only Send on Change

Before sending a screenshot to Gemini, compare the current frame hash against the last-sent hash. Skip if identical.

```
Screenshot sender:
  every 500ms → read latest_jpeg + hash
  → if hash == last_sent_hash → SKIP
  → if hash != last_sent_hash → send to Gemini, update last_sent_hash
```

**Idle throttling:** If screen unchanged for 5 seconds (10 consecutive skips at 500ms), switch to 2s polling interval. Resume 500ms on first change detected.

**Impact:** During static screens (talking, thinking), drops screenshot traffic from ~120/min to near zero. During active tool use, stays at 2fps.

### 2. Fresh Reconnects — Clear Session Resumption

On reconnect, do NOT send the previous session resumption handle. Start a fresh session.

```
Current: disconnect → reconnect → send stored handle → restore bloated context → dies faster
New:     disconnect → reconnect → fresh session → clean context → full budget
```

**Trade-off:** Gemini loses pre-disconnect memory. Acceptable since sessions die in under 5 minutes anyway — nothing worth preserving. System prompt re-establishes all tool definitions and behavior.

### 3. Tool Response Truncation

Cap large tool responses before sending to Gemini.

- **Max size:** 8000 characters. Responses exceeding this get truncated with `[truncated]` marker.
- **get_screen_context special handling:**
  - Cap AX elements to top 30 (focused element first, then by proximity to screen center)
  - Strip verbose fields (full bounds) from non-focused elements, keep role + label only
  - Full detail preserved for focused element and frontmost app
- **Other tools:** Most are small. Cap is a safety net for unexpected large payloads.

### 4. SlidingWindow Configuration

Set explicit `targetTokens` to enable server-side context compression.

```json
{ "slidingWindow": { "targetTokens": 500000 } }
```

**Why 500K:** Gemini Flash has 1M context. 500K gives the server room to compress before hitting the hard limit. Safety net behind client-side fixes.

## Files Changed

| File | Change | ~Lines |
|------|--------|--------|
| `crates/aura-daemon/src/main.rs` | Screenshot hash gating, idle throttling, tool response truncation | ~60 |
| `crates/aura-gemini/src/session.rs` | Remove resumption handle on reconnect | ~10 |
| `crates/aura-gemini/src/protocol.rs` | Add `target_tokens` to SlidingWindow | ~5 |

No new files or crates.

## Testing

- Verify screenshot skipping: mock static frames, confirm send count drops to zero
- Verify idle throttling: 10 consecutive skips triggers 2s interval, change resumes 500ms
- Verify tool truncation: large AX dump gets capped at 30 elements / 8000 chars
- Verify fresh reconnect: after disconnect, no resumption handle sent
- Integration: run session, confirm longer uptime and clean reconnects
