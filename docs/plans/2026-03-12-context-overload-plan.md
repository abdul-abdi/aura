# Context Overload Prevention — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Prevent context overload in long-running Gemini Live sessions by gating screenshot sending, clearing resumption handles on reconnect, truncating tool responses, and configuring SlidingWindow.

**Architecture:** Four independent changes: (1) screenshot hash gating in the daemon capture loop skips sending unchanged frames to Gemini, with idle throttling after 5s of no change; (2) session reconnection clears the resumption handle for a fresh context; (3) tool responses are truncated before sending to Gemini; (4) SlidingWindow gets an explicit targetTokens parameter.

**Tech Stack:** Rust, tokio, serde, existing `Arc<AtomicU64>` frame hash infrastructure.

---

### Task 1: Create feature branch and add last-sent hash tracking

**Files:**
- Modify: `crates/aura-daemon/src/main.rs:690-692` (capture loop setup)

**Step 1: Create the feature branch**

```bash
git checkout -b feature/context-overload-prevention
```

**Step 2: Add a shared AtomicU64 for the last-sent screenshot hash**

After line 692 (`let last_frame_hash = Arc::new(AtomicU64::new(0));`), add:

```rust
let last_sent_hash = Arc::new(AtomicU64::new(0));
```

Clone it for the capture loop, alongside the existing clones (after line 709 `let cap_last_hash = ...`):

```rust
let cap_last_sent = Arc::clone(&last_sent_hash);
```

**Step 3: Build and verify**

Run: `cargo build -p aura-daemon 2>&1 | head -20`
Expected: compiles (unused variable warnings are fine — Task 2 uses them)

**Step 4: Commit**

```bash
git add crates/aura-daemon/src/main.rs
git commit -m "chore: add last_sent_hash AtomicU64 for screenshot gating"
```

---

### Task 2: Gate screenshot sending — only send when hash changes

**Files:**
- Modify: `crates/aura-daemon/src/main.rs:805-826` (the `send_video` block in the capture loop)

**Step 1: Replace the unconditional send_video block**

Replace lines 805-826 (from `// Send to Gemini —` through the `tracing::debug!` block) with:

```rust
            // Only send to Gemini if the screen actually changed since last send.
            // This is the #1 context savings: static screens produce zero token cost.
            let already_sent = cap_last_sent.load(Ordering::Acquire);
            if frame.hash == already_sent {
                // Frame captured (hash differs from last_frame_hash) but already sent
                // to Gemini — skip. Still resolve waiter so tool spawns don't hang.
                if let Some(tx) = cap_trigger.take_waiter() {
                    let _ = tx.send(());
                }
                tracing::trace!("Skipped duplicate send (hash unchanged since last send)");
                continue;
            }

            if let Err(e) = cap_session.send_video(&frame.jpeg_base64) {
                tracing::debug!("Dropped screen frame (channel not ready): {e}");
                if let Some(tx) = cap_trigger.take_waiter() {
                    let _ = tx.send(());
                }
                continue;
            }
            cap_last_sent.store(frame.hash, Ordering::Release);

            // Signal any awaiting tool spawn that the screenshot was delivered
            if let Some(tx) = cap_trigger.take_waiter() {
                let _ = tx.send(());
            }
            tracing::debug!(
                width = frame.width,
                height = frame.height,
                scale_factor = frame.scale_factor,
                size_kb = frame.jpeg_base64.len() / 1024,
                "Sent screen frame"
            );
```

**Step 2: Build and verify**

Run: `cargo build -p aura-daemon 2>&1 | head -20`
Expected: compiles with no errors

**Step 3: Commit**

```bash
git add crates/aura-daemon/src/main.rs
git commit -m "feat: gate screenshot sending on hash change — skip unchanged frames"
```

---

### Task 3: Add idle throttling — slow polling when screen is static

**Files:**
- Modify: `crates/aura-daemon/src/main.rs:710-714` (interval setup in capture loop)
- Modify: `crates/aura-daemon/src/main.rs` (after the send block, before loop end)

**Step 1: Add idle counter and dynamic interval logic**

After line 711 (`let mut censored_warned = false;`), add:

```rust
        let mut idle_skip_count: u32 = 0;
        const IDLE_THRESHOLD: u32 = 10; // 10 × 500ms = 5s of no change → slow down
```

**Step 2: After the hash-gating skip (the `if frame.hash == already_sent` block), increment the idle counter and adjust interval**

Inside the `if frame.hash == already_sent` block, before the `continue;`, add:

```rust
                idle_skip_count += 1;
                if idle_skip_count == IDLE_THRESHOLD {
                    // Screen static for 5s — switch to slow polling (2s)
                    interval = tokio::time::interval(Duration::from_millis(2000));
                    interval.tick().await;
                    tracing::debug!("Screen idle for 5s — switching to 2s capture interval");
                }
```

**Step 3: After a successful send (after `cap_last_sent.store(...)`), reset idle counter and restore fast interval**

Right after the `cap_last_sent.store(frame.hash, Ordering::Release);` line, add:

```rust
            // Screen changed — reset idle counter and restore fast polling
            if idle_skip_count >= IDLE_THRESHOLD {
                interval = tokio::time::interval(Duration::from_millis(500));
                interval.tick().await;
                tracing::debug!("Screen changed — restoring 500ms capture interval");
            }
            idle_skip_count = 0;
```

**Step 4: Build and verify**

Run: `cargo build -p aura-daemon 2>&1 | head -20`
Expected: compiles with no errors

**Step 5: Commit**

```bash
git add crates/aura-daemon/src/main.rs
git commit -m "feat: idle throttle — slow to 2s capture interval after 5s of no screen change"
```

---

### Task 4: Fresh reconnects — clear resumption handle on every reconnect

**Files:**
- Modify: `crates/aura-gemini/src/session.rs:459-460` (where resumption handle is read before setup)

**Step 1: Clear the resumption handle before building the setup message**

Replace lines 459-460:

```rust
    let resumption_handle = state.resumption_handle.lock().await.clone();
    let setup = build_setup_message(&state.config, resumption_handle);
```

With:

```rust
    // Always start fresh — don't restore previous context.
    // Session resumption preserves all accumulated screenshots/tool responses,
    // causing context to snowball and sessions to die faster on each reconnect.
    {
        let mut handle = state.resumption_handle.lock().await;
        if handle.is_some() {
            tracing::info!("Clearing resumption handle for fresh session (context overload prevention)");
            *handle = None;
        }
    }
    let setup = build_setup_message(&state.config, None);
```

**Step 2: Build and verify**

Run: `cargo build -p aura-gemini 2>&1 | head -20`
Expected: compiles with no errors

**Step 3: Commit**

```bash
git add crates/aura-gemini/src/session.rs
git commit -m "feat: always start fresh session on reconnect — prevent context snowballing"
```

---

### Task 5: Truncate large tool responses before sending to Gemini

**Files:**
- Modify: `crates/aura-daemon/src/main.rs:1267-1270` (tool response send block)

**Step 1: Add a truncation helper function**

Add this function somewhere above the main event loop (near the other helper functions, e.g. near `is_state_changing_tool`):

```rust
/// Truncate a tool response to stay within context budget.
/// Caps the serialized JSON at `max_chars` characters.
const MAX_TOOL_RESPONSE_CHARS: usize = 8000;

fn truncate_tool_response(response: &mut serde_json::Value) {
    // For get_screen_context: trim the elements list
    if let Some(ctx) = response.get_mut("context") {
        if let Some(obj) = ctx.as_object_mut() {
            if let Some(elements) = obj.get_mut("elements") {
                if let Some(arr) = elements.as_array_mut() {
                    if arr.len() > 30 {
                        arr.truncate(30);
                        arr.push(serde_json::json!({"truncated": true, "original_count": arr.len() + 30}));
                    }
                    // Strip verbose bounds from non-first elements
                    for (i, el) in arr.iter_mut().enumerate() {
                        if i > 0 {
                            if let Some(obj) = el.as_object_mut() {
                                obj.remove("bounds");
                            }
                        }
                    }
                }
            }
        }
    }

    // General size cap: if still too large, truncate the serialized form
    let serialized = response.to_string();
    if serialized.len() > MAX_TOOL_RESPONSE_CHARS {
        if let Some(obj) = response.as_object_mut() {
            // Keep only success, verified, post_state, and a truncation marker
            let success = obj.get("success").cloned();
            let verified = obj.get("verified").cloned();
            obj.clear();
            if let Some(s) = success {
                obj.insert("success".to_string(), s);
            }
            if let Some(v) = verified {
                obj.insert("verified".to_string(), v);
            }
            obj.insert(
                "truncated".to_string(),
                serde_json::json!(format!("Response truncated from {} chars to save context", serialized.len())),
            );
        }
    }
}
```

**Step 2: Call truncation before sending the tool response**

At line 1268 (before `tool_session.send_tool_response(...)`), add:

```rust
                            truncate_tool_response(&mut response);
```

Note: `response` needs to be mutable at this point. It's already declared as `let mut response` at line 1097, so this works.

**Step 3: Build and verify**

Run: `cargo build -p aura-daemon 2>&1 | head -20`
Expected: compiles with no errors

**Step 4: Commit**

```bash
git add crates/aura-daemon/src/main.rs
git commit -m "feat: truncate large tool responses before sending to Gemini"
```

---

### Task 6: Configure SlidingWindow with targetTokens

**Files:**
- Modify: `crates/aura-gemini/src/protocol.rs:161-163` (SlidingWindow struct)
- Modify: `crates/aura-gemini/src/session.rs:732-733` (where SlidingWindow is instantiated)

**Step 1: Add target_tokens field to SlidingWindow struct**

Replace lines 161-163 in `protocol.rs`:

```rust
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlidingWindow {}
```

With:

```rust
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlidingWindow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_tokens: Option<u32>,
}
```

**Step 2: Set targetTokens in the session setup**

Replace line 733 in `session.rs`:

```rust
                sliding_window: SlidingWindow {},
```

With:

```rust
                sliding_window: SlidingWindow {
                    target_tokens: Some(500_000),
                },
```

**Step 3: Build and verify**

Run: `cargo build -p aura-gemini 2>&1 | head -20`
Expected: compiles with no errors

**Step 4: Commit**

```bash
git add crates/aura-gemini/src/protocol.rs crates/aura-gemini/src/session.rs
git commit -m "feat: configure SlidingWindow with 500K targetTokens for context compression"
```

---

## Execution Order

| Order | Task | Description |
|-------|------|-------------|
| 1 | Task 1 | Create branch + add last_sent_hash tracking |
| 2 | Task 2 | Gate screenshot sending on hash change |
| 3 | Task 3 | Add idle throttling |
| 4 | Task 4 | Fresh reconnects (clear resumption handle) |
| 5 | Task 5 | Truncate tool responses |
| 6 | Task 6 | Configure SlidingWindow targetTokens |

Tasks 4, 5, and 6 are independent of each other and can be done in any order after Task 3.
Tasks 4 and 6 touch `aura-gemini`, Tasks 1-3 and 5 touch `aura-daemon`.
