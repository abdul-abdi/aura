# Screen-Verified Tool Execution

## Problem

Gemini reports tool success before the screen confirms the action took effect. This causes:
1. **False positives**: Tool says "done" but the click missed or app didn't respond
2. **Premature responses**: Gemini speaks before the user sees the result on screen

Additionally, AppleScript-driven UI interactions are invisible — the user wants to see the cursor move and click.

## Solution

### 1. Daemon-Side Verification Gate

Add a hash-based screen change detection loop between tool execution and tool response delivery.

**Flow:**
```
1. GeminiEvent::ToolCall received
2. is_state_changing(tool_name)?
   YES → capture pre_hash from last known frame
3. execute_tool(name, args)
4. is_state_changing?
   YES → 150ms settle → poll loop:
         - trigger screenshot capture
         - wait for frame with hash != pre_hash
         - 200ms intervals, 2s timeout, 10 checks max
         → set verified = (hash changed)
         → capture post_state
         → compare post_state vs expectation → set warning if mismatch
   NO  → verified = None (not applicable)
5. Build response: { success, verified, post_state, warning, verification_reason }
6. send_tool_response(id, name, response)
```

**Parameters:**
- Poll interval: 200ms
- Timeout: 2s (10 checks max)
- No automatic retries — Gemini decides next step on failure

**State-changing tools:** click, click_element, type_text, press_key, move_mouse, scroll, drag, activate_app, click_menu_item

**Non-state-changing tools (unchanged):** get_screen_context, recall_memory, run_applescript, shutdown_aura

**Response shape:**
```json
{
  "success": true,
  "verified": true,
  "post_state": {
    "frontmost_app": "Safari",
    "focused_element": { "role": "AXTextField", "label": "Search", "value": "...", "bounds": {} },
    "screenshot_delivered": true
  },
  "warning": null
}
```

Unverified response:
```json
{
  "success": true,
  "verified": false,
  "verification_reason": "screen_unchanged_after_2s",
  "post_state": { ... },
  "warning": "post_state.focused_element does not match expected target"
}
```

### 2. System Prompt — Mouse Preference & Verification Behavior

**Mouse preference (clicks/navigation only):**
- click_element and click(x, y) become primary for clicking buttons, links, UI elements
- AppleScript kept for: text manipulation, window management, app-specific scripting, things with no visual equivalent

**Decision flow:**
1. Keyboard shortcut? → press_key
2. Clicking/navigating UI? → click_element or click(x,y) with visible mouse
3. App-specific scripting (no visual equivalent)? → AppleScript
4. Text/window manipulation? → AppleScript
5. Fallback? → get_screen_context() + retry with different approach

**Verification behavior:**
- `verified: false` → do NOT tell user the action succeeded. Call get_screen_context(), try different approach, report honestly if still failing.
- `verified: true` + warning → proceed with caution, may need get_screen_context()
- Never chain multiple unverified actions

### 3. Edge Cases

- **Screen animating**: Hash changes from animation, not action. Acceptable false-positive — post_state mismatch warning catches this.
- **No visible change** (e.g., clicking already-focused field): verified: false but post_state shows correct focus — Gemini reads post_state and proceeds.
- **Multiple rapid tool calls**: Each waits for its own verification. Sequential by design.

## Files Changed

| File | Change | ~Lines |
|------|--------|--------|
| `crates/aura-daemon/src/main.rs` | Pre-hash capture, poll loop, verification fields | ~40 |
| `crates/aura-gemini/src/config.rs` | System prompt update (strategy + verification) | ~30 |

No new files, crates, or protocol changes.

## Testing

- Unit test: mock frame hashes, verify poll loop returns correct verified/unverified states
- Integration test: execute click on known UI, assert hash changes and post_state matches
