# Critical Bugfix & Firestore Hardening Design

**Date:** 2026-03-12
**Context:** Post-audit fixes for hackathon readiness (deadline 2026-03-16)

## 1. Make EventBus Events Functional

`GeminiConnected`, `BargeIn`, `ToolExecuted` are published but never consumed with side effects. `Daemon::run()` should accept `ipc_tx` and `menubar_tx` and forward relevant events:

- `GeminiConnected` → IPC status "Connected" + green dot
- `BargeIn` → IPC status "Listening..."
- `ToolExecuted` → menubar activity indicator (already IPC'd inline)

## 2. Fix Tool Status Duplication in SwiftUI

`handleToolStatus` appends a new event for both `.running` and `.completed/.failed`, creating 2 rows per tool call. Fix: on completion/failure, find and update the existing `.running` event in-place instead of appending.

## 3. Pin Reconnect Banner + Fix Streaming Duplicate

- Move reconnect banner outside `ScrollView` so it's always visible when disconnected
- Fix assistant speech merge: remove `!update.done` guard so final chunk merges into existing row instead of creating a duplicate

## 4. Send TurnComplete via IPC + Fix Truncation

- `GeminiEvent::TurnComplete` → send `DaemonEvent::Transcript { done: true, text: "", role: assistant, source: voice }` to IPC
- `truncate_tool_response`: preserve `error` and `stdout` fields (truncated to 500 chars) instead of discarding everything

## 5. Firestore + GCP Fixes

- importance default: `0.0` → `0.5` in `firestore_doc_to_fact`
- Local fallback writes to Firestore: after `consolidate_locally`, daemon calls `FirestoreClient::write_fact` so both paths sync to cloud
- Add `created_at` timestamp to Firestore fact and session documents
- Concurrent Firestore writes in Cloud Run via `join_all`
- Block `| sh`, `| bash`, `| python`, `| zsh` in safety filter
- Add `.unknown` fallback to Swift `TranscriptSource` enum for forward compatibility
