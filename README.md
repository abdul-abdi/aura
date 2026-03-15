<div align="center">

<img src="assets/icon.png" alt="Aura" width="120" height="120" />

# Aura

**Your Mac, voice-controlled.**

[![macOS](https://img.shields.io/badge/macOS-14+-000000?style=flat-square&logo=apple&logoColor=white)](https://www.apple.com/macos/)
[![Rust](https://img.shields.io/badge/Rust-native-f74c00?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![Gemini](https://img.shields.io/badge/Gemini-Live_API-4285F4?style=flat-square&logo=google&logoColor=white)](https://ai.google.dev/)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue?style=flat-square)](LICENSE)

</div>

<br/>

<div align="center">
<img src="assets/promo.gif" alt="Aura demo — voice-controlled macOS" width="100%" />

[Watch full 60s video →](assets/promo.mp4)
</div>

<br/>

You're deep in work. Twelve tabs open, three apps side by side, a document you need to reference while typing into another window. You reach for the mouse, click, switch, scroll, copy, switch back, paste. Repeat. All day.

**What if you could just say it?**

> *"Open Safari and go to the project board."*
>
> *"Take a look at my screen and summarize what's going on."*
>
> *"Move the mouse to that submit button and click it."*
>
> *"Press Cmd+Shift+4 and take a screenshot."*

Aura is an AI that lives in your menu bar. It hears you, sees your screen, and acts — moving the mouse, typing, clicking, running scripts — all through natural conversation. No keyboard shortcuts to memorize. No workflow apps to configure. Just talk.

Aura speaks with **Kore**, a calm, clear voice from Gemini's native audio model — chosen for its natural cadence in back-and-forth conversation. It doesn't narrate actions or over-explain. It responds the way a competent colleague would: briefly, directly, then acts. When you interrupt mid-sentence, it stops immediately and adapts — conversations feel human, not scripted.

Built on the **Gemini Live API** for real-time bidirectional audio and vision streaming, with **Google Cloud Run** and **Firestore** for persistent cross-session memory.

### Built with

| Google Technology | How Aura uses it |
|---|---|
| **Gemini Live API** | Bidirectional WebSocket for simultaneous audio + vision streaming |
| **Gemini 2.5 Flash** (native audio) | Real-time voice conversation with native speech synthesis |
| **Gemini 3 Flash** | Vision oracle — refines click coordinates from screenshots |
| **Google Search grounding** | Live web answers (weather, news, facts) without leaving the conversation |
| **Cloud Run** | WebSocket proxy relay + memory consolidation agent |
| **Firestore** | Cross-device persistent memory (facts, session summaries) |
| **Gemini ADK** | Powers the memory agent's session consolidation pipeline |

---

## How it works

A small green dot appears in your menu bar. That's Aura, always listening.

When you speak, your voice streams in real-time to Google's Gemini Live API. Aura simultaneously watches your screen at 2 frames per second, so it always knows what you're looking at. When Gemini decides to act, it calls one of Aura's native tools — and your Mac responds instantly.

```
    ┌─────────┐  16kHz audio   ┌─────────────────────┐
    │   You   │ ─────────────→ │                     │
    │  speak  │                │  Gemini Live API    │
    └─────────┘                │  (bidirectional WS) │
                               │                     │
    ┌─────────┐  2 FPS JPEG    │  sees + hears you   │
    │ Screen  │ ─────────────→ │  simultaneously     │
    │ capture │                └──────────┬──────────┘
    └─────────┘                           │
                                 speaks back (24kHz)
                                 OR calls tools
                                          │
                     ┌────────────────────┼────────────────────┐
                     ▼                    ▼                    ▼
               ┌──────────┐        ┌──────────┐        ┌──────────┐
               │  Click   │        │   Type   │        │  Script  │
               │  Scroll  │        │   Keys   │        │  Shell   │
               │  Drag    │        │   Text   │        │  Apps    │
               └──────────┘        └──────────┘        └──────────┘
                     │                    │                    │
                     └────────────────────┼────────────────────┘
                                          ▼
                                   Screen updates
                                   (loop continues)
```

The entire pipeline — voice capture, screen analysis, tool execution — runs as native Rust. No Electron. No browser. No latency from web tech. Just raw speed on bare metal macOS.

## What Aura can do

| | Capability | How it works |
|---|---|---|
| **Talk** | Real-time voice conversation | 16kHz capture, 24kHz playback, barge-in detection |
| **See** | Understands your screen | 2 FPS capture with change detection, reads accessibility labels |
| **Click** | Precise mouse control | Move, click (left/right, single/double/triple), scroll, drag |
| **Type** | Keyboard automation | Type text, press shortcuts (Cmd+C, Cmd+V, etc.), special keys |
| **Script** | AppleScript & shell | Control any macOS app — open files, switch tabs, manage windows |
| **Search** | Live web answers | Google Search grounding for real-time facts, weather, news |
| **Ground** | Anti-hallucination | Screen verification, accessibility cross-check, Search grounding |
| **Remember** | Persistent memory | SQLite-backed session history across restarts |
| **Protect** | Defense-in-depth safety | Pattern blocklists, input clamping, obfuscation detection |

## Example commands

```
"Open Finder and go to my Downloads folder."
"What app am I looking at right now?"
"Click the blue button in the top right."
"Type 'meeting notes' into the search bar and press Enter."
"Drag that file to the Desktop."
"Press Cmd+Z to undo."
"Close this window."
```

## Native tools (20)

Gemini calls these through the Live API's function calling protocol. Every tool maps to a native macOS API — no shell wrappers, no browser automation frameworks.

| Category | Tools |
|---|---|
| **Mouse** | `click`, `click_element`, `context_menu_click`, `move_mouse`, `drag`, `scroll` |
| **Keyboard** | `type_text`, `press_key`, `select_text` |
| **Apps** | `activate_app`, `click_menu_item` |
| **Scripts** | `run_applescript`, `run_javascript`, `run_shell_command` |
| **Memory** | `save_memory`, `recall_memory` |
| **Context** | `get_screen_context`, `write_clipboard` |
| **System** | `shutdown_aura` |
| **Web** | Google Search (grounding tool — built into Gemini) |

Tools execute asynchronously (max 8 concurrent). Safe action chains — like click → type → enter — pipeline without screen verification, up to 3 deep with 30ms settle delays.

## Core pipeline

The loop that runs continuously while Aura is active:

```
 1. Mic captures 16kHz PCM in 100ms chunks
 2. Voice Activity Detection checks energy against ambient threshold
 3. Active audio streams to Gemini Live API over WebSocket
 4. Screen capture sends 2 FPS JPEG (skips unchanged frames via perceptual hash)
 5. Gemini processes audio + vision simultaneously
 6. Gemini responds with:
    ├─ Speech → 24kHz PCM decoded and played through system speaker
    ├─ Tool call → dispatched to native macOS tool → result fed back to Gemini
    └─ Both → speech streams while tools execute in parallel
 7. Barge-in: if user speaks during playback → playback stops → Gemini notified
 8. Screen re-captured after tool execution → Gemini sees the result of its action
 9. Loop continues until session ends
```

**Latency budget:** Voice-in to voice-out is bounded by Gemini's inference time. The local pipeline (capture → encode → send, receive → decode → play) adds <50ms total.

---

## Get started

**1. Clone and build**

> ```bash
> git clone https://github.com/abdul-abdi/aura.git && cd aura
> ```
>
> Requires **Rust 1.85+** and **Xcode Command Line Tools**.

**2. Get a Gemini API key**

> Grab a free key from [Google AI Studio](https://aistudio.google.com/apikey). Add it to your config:
>
> ```bash
> mkdir -p ~/.config/aura
> echo 'api_key = "YOUR_KEY_HERE"' > ~/.config/aura/config.toml
> ```

**3. Build, install, and launch**

> ```bash
> bash scripts/dev.sh
> ```
>
> This builds the Rust daemon + SwiftUI app, installs `Aura.app` to `/Applications`, and launches it. One command.

**4. Grant permissions**

> Aura needs three macOS permissions to function:
>
> | Permission | Why |
> |---|---|
> | Microphone | To hear your voice |
> | Screen Recording | To see your screen |
> | Accessibility | To control mouse and keyboard |
>
> The app walks you through granting each one on first launch.

**5. Start talking**

> The green dot appears in your menu bar. You're live.

---

## Architecture

10 Rust crates + a SwiftUI shell, each with one job. No Electron. No web views. Pure native macOS.

```
┌─ USER HARDWARE ───────────────────────────────────────────────────────────┐
│  Microphone (16kHz)    Speaker (24kHz)    Display    Keyboard & Mouse    │
└──────┬──────────────────────┬──────────────────┬──────────────┬──────────┘
       │                      ▲                  │              ▲
       ▼                      │                  ▼              │
┌─ AURA DAEMON (orchestrator + event bus) ──────────────────────────────────┐
│                                                                           │
│  aura-voice ──────→ aura-gemini ←──────── aura-screen                    │
│  (capture/playback)  (Live API WS)         (2 FPS + accessibility)       │
│                          │                                                │
│                    tool calls from Gemini                                 │
│                          │                                                │
│           ┌──────────────┼──────────────┐                                │
│           ▼              ▼              ▼                                 │
│      aura-input    aura-bridge    aura-memory                            │
│      (mouse/kbd)   (AppleScript)  (SQLite FTS5)                          │
│                                                                           │
│  aura-menubar ←──── IPC (Unix socket, JSONL) ────→ SwiftUI App          │
│  (Cocoa status dot)                                                       │
└───────────────────────────────────────────────────────────────────────────┘
       │                                                        │
       ▼                                                        ▼
┌─ GOOGLE CLOUD (optional) ─────────────────────────────────────────────────┐
│  aura-proxy (Cloud Run)     memory-agent (Cloud Run)     Firestore       │
│  WebSocket relay            Gemini-powered session       facts & sessions│
│  per-device auth            consolidation via ADK        per device      │
└───────────────────────────────────────────────────────────────────────────┘
```

| Crate | Purpose |
|---|---|
| `aura-daemon` | Orchestrator — event bus, tool dispatch, session lifecycle |
| `aura-gemini` | Bidirectional WebSocket client for Gemini Live API |
| `aura-voice` | CoreAudio capture + rodio playback + barge-in detection |
| `aura-screen` | Screen capture, perceptual change detection, accessibility tree |
| `aura-bridge` | AppleScript execution with multi-layer safety gates |
| `aura-input` | CGEvent synthetic mouse + keyboard input |
| `aura-memory` | SQLite persistence (WAL mode, FTS5 full-text search) |
| `aura-menubar` | Cocoa FFI — NSStatusItem, NSPopover, context menu |
| `aura-proxy` | Cloud Run WebSocket relay with per-device auth |
| `aura-firestore` | Firestore REST client for cross-device memory sync |

Deep dive: [ARCHITECTURE.md](ARCHITECTURE.md)

## Build from source

```bash
# Full dev workflow: build, install to /Applications, and launch
bash scripts/dev.sh

# Or just build the .app bundle without installing
bash scripts/bundle.sh
open target/release/Aura.app
```

## Deploy to Google Cloud

Aura's cloud backend (WebSocket proxy + memory agent + Firestore) deploys to Google Cloud with a single script. Infrastructure-as-code — no manual console steps.

**Automated (one command):**

```bash
export GEMINI_API_KEY="your-key"
bash scripts/deploy-gcp.sh --project your-gcp-project-id
```

This enables Firestore, creates Secret Manager entries, deploys the memory agent to Cloud Run, and configures IAM — fully automated.

**What gets deployed:**

| Service | Platform | Purpose |
|---|---|---|
| `aura-memory-agent` | Cloud Run | Session consolidation via Gemini ADK — distills conversations into persistent facts |
| `aura-proxy` | Cloud Run | WebSocket relay to Gemini Live API with per-device token auth |
| Firestore | Native mode | Document store for cross-device facts and session summaries |
| Secret Manager | GCP | Stores API keys and auth tokens (never in code or env vars) |

**CI/CD:** Push to `main` triggers automatic deployment via GitHub Actions with Workload Identity Federation (no service account keys). See `.github/workflows/deploy-cloud.yml`.

## Grounding & accuracy

AI controlling your desktop can't afford to hallucinate. Aura uses three layers to keep Gemini grounded:

1. **Screen verification** — After every click or navigation, Aura captures the screen and feeds it back to Gemini, closing the perception-action loop. Gemini sees the result of its own actions and self-corrects.
2. **Accessibility cross-check** — Before clicking a UI element, Aura reads the macOS accessibility tree (element roles, labels, bounds) and cross-references it with Gemini's visual understanding. If the accessibility data contradicts the model's target, a secondary **Gemini 3 Flash vision oracle** refines the coordinates from a fresh screenshot.
3. **Google Search grounding** — Factual questions (weather, news, definitions) are answered via Google Search as a Gemini tool, grounding responses in real-time web results instead of parametric memory.

## Safety

Aura runs AI-generated actions on your machine. We take that seriously.

Every script passes through **pattern blocklists** that catch destructive commands (`rm -rf`, `sudo`, `mkfs`). Every input is **clamped** to safe ranges. **Obfuscation detection** catches commands split across variables to bypass filters. An **automation permission preflight** checks access before running scripts. And a **destructive action guardrail** requires spoken confirmation before anything that permanently deletes data.

Terminal apps (Terminal, iTerm, Warp, etc.) are blocked entirely from script execution to prevent unintended command runs.

---

<div align="center">

Requires macOS 14+, Rust 1.85+, and a free [Gemini API key](https://aistudio.google.com/apikey).

[Apache-2.0 License](LICENSE)

</div>
