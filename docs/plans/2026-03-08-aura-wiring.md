# Aura End-to-End Wiring Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Wire all 6 crates together so `cargo run` starts a production-ready voice assistant — mic captures audio, VAD detects speech, STT transcribes, Ollama parses intent via Qwen3.5-4B, macOS executes the action, Skia overlay renders feedback. Ship it.

**Architecture:** The daemon spawns three async subsystems on a tokio broadcast event bus: (1) voice task: mic → VAD → STT → publishes VoiceCommand events, (2) processor task: subscribes to VoiceCommand → Ollama intent parsing → action execution → publishes results, (3) overlay: winit event loop on main thread with EventLoopProxy bridge from async tasks. Ollama runs as an external process (user installs separately), accessed via HTTP API with reqwest.

**Tech Stack:** Rust, tokio, cpal (audio), whisper-rs (STT), reqwest (Ollama HTTP), skia-safe + winit + softbuffer (overlay), clap (CLI), osascript (macOS actions)

**Model:** Qwen3.5-4B via Ollama — native function-calling support, excellent structured JSON output, ~2.5GB, fast on Apple Silicon

---

## Prerequisites (user does once)

```bash
# 1. Install Ollama
brew install ollama

# 2. Pull the intent model
ollama pull qwen3.5:4b

# 3. Download whisper STT model (~150MB)
mkdir -p ~/Library/Application\ Support/aura/models
curl -L -o ~/Library/Application\ Support/aura/models/ggml-base.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin

# 4. Start Ollama server (runs in background)
ollama serve
```

---

## What Exists vs What's Missing

| Component | Status | Action |
|-----------|--------|--------|
| AudioCapture (cpal) | Done | Ready |
| WakeWordDetector | Done | Optional — skip for v1, use VAD |
| SpeechToText (whisper-rs) | Done | Ready (needs model file) |
| VoicePipeline | Skeleton — no VAD, no STT wiring | **Task 2** |
| IntentParser | Done — parses JSON from LlmProvider | Ready |
| LlamaCppProvider | Dead code — returns empty string | **Task 1: Delete, replace with Ollama** |
| MacOSExecutor | Done — spawn_blocking'd | Ready |
| OverlayRenderer (Skia) | Done — all states render | Ready |
| OverlayWindow (winit) | Skeleton — no Skia surface | **Task 6** |
| Daemon event loop | Done — handles all events | Ready |
| main.rs wiring | Missing — just creates bus | **Task 5** |
| CLI | Missing | **Task 7** |
| Error handling / resilience | Missing | **Task 8** |
| Tests | 49 pass, but no integration tests | **Task 9** |

---

### Task 1: Replace llama-cpp-2 with Ollama HTTP Provider

**Why:** llama-cpp-2 requires compiling C++ from source, the token generation loop was never implemented (returns empty string), and managing GGUF files is unnecessary work. Ollama provides a simple HTTP API, handles model management, and runs Qwen3.5-4B out of the box.

**Files:**
- Delete: `crates/aura-llm/src/llamacpp.rs`
- Create: `crates/aura-llm/src/ollama.rs`
- Modify: `crates/aura-llm/src/lib.rs` (remove llamacpp module, add ollama)
- Modify: `crates/aura-llm/Cargo.toml` (remove llama-cpp-2, add reqwest)
- Modify: `crates/aura-daemon/Cargo.toml` (remove `features = ["llamacpp"]` from aura-llm dep)

**Step 1: Update aura-llm/Cargo.toml**

Remove llama-cpp-2 and the llamacpp feature. Add reqwest:

```toml
[package]
name = "aura-llm"
version.workspace = true
edition.workspace = true

[features]
test-support = []

[dev-dependencies]
aura-llm = { path = ".", features = ["test-support"] }

[dependencies]
tokio.workspace = true
tracing.workspace = true
anyhow.workspace = true
serde.workspace = true
serde_json.workspace = true
async-trait = "0.1"
reqwest = { version = "0.12", features = ["json"] }
```

**Step 2: Delete llamacpp.rs, create ollama.rs**

```rust
// crates/aura-llm/src/ollama.rs
use crate::provider::LlmProvider;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const DEFAULT_BASE_URL: &str = "http://localhost:11434";
const DEFAULT_MODEL: &str = "qwen3.5:4b";

#[derive(Debug, Clone)]
pub struct OllamaConfig {
    pub base_url: String,
    pub model: String,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            model: DEFAULT_MODEL.into(),
        }
    }
}

pub struct OllamaProvider {
    client: reqwest::Client,
    config: OllamaConfig,
}

#[derive(Serialize)]
struct GenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
    options: GenerateOptions,
}

#[derive(Serialize)]
struct GenerateOptions {
    temperature: f32,
    num_predict: u32,
}

#[derive(Deserialize)]
struct GenerateResponse {
    response: String,
}

impl OllamaProvider {
    pub fn new(config: OllamaConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self { client, config }
    }

    /// Check if Ollama is running and the model is available.
    pub async fn health_check(&self) -> Result<()> {
        // Check server is up
        self.client
            .get(format!("{}/api/tags", self.config.base_url))
            .send()
            .await
            .context("Ollama is not running. Start it with: ollama serve")?;

        // Check model is pulled
        let resp: serde_json::Value = self
            .client
            .get(format!("{}/api/tags", self.config.base_url))
            .send()
            .await?
            .json()
            .await?;

        let models = resp["models"]
            .as_array()
            .context("Unexpected Ollama API response")?;

        let model_prefix = self.config.model.split(':').next().unwrap_or(&self.config.model);
        let found = models.iter().any(|m| {
            m["name"]
                .as_str()
                .is_some_and(|n| n.starts_with(model_prefix))
        });

        if !found {
            anyhow::bail!(
                "Model '{}' not found. Pull it with: ollama pull {}",
                self.config.model,
                self.config.model
            );
        }

        Ok(())
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn complete(&self, prompt: &str) -> Result<String> {
        let request = GenerateRequest {
            model: &self.config.model,
            prompt,
            stream: false,
            options: GenerateOptions {
                temperature: 0.1,
                num_predict: 256,
            },
        };

        let resp = self
            .client
            .post(format!("{}/api/generate", self.config.base_url))
            .json(&request)
            .send()
            .await
            .context("Failed to reach Ollama — is it running?")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Ollama returned {status}: {body}");
        }

        let body: GenerateResponse = resp
            .json()
            .await
            .context("Failed to parse Ollama response")?;

        Ok(body.response)
    }
}
```

**Step 3: Update lib.rs**

```rust
//! Aura LLM: local language model interface for intent parsing

pub mod intent;
pub mod ollama;
pub mod provider;
```

**Step 4: Update aura-daemon/Cargo.toml**

Change the aura-llm dependency from `features = ["llamacpp"]` to no features:

```toml
aura-llm = { path = "../aura-llm" }
```

**Step 5: Write tests**

```rust
// crates/aura-llm/tests/ollama_test.rs
use aura_llm::ollama::{OllamaConfig, OllamaProvider};
use aura_llm::provider::LlmProvider;

#[tokio::test]
#[ignore] // Requires Ollama running with qwen3.5:4b pulled
async fn test_ollama_health_check() {
    let provider = OllamaProvider::new(OllamaConfig::default());
    provider.health_check().await.unwrap();
}

#[tokio::test]
#[ignore] // Requires Ollama running with qwen3.5:4b pulled
async fn test_ollama_complete() {
    let provider = OllamaProvider::new(OllamaConfig::default());
    let result = provider.complete("Say hello in one word.").await.unwrap();
    assert!(!result.is_empty());
}

#[tokio::test]
#[ignore] // Requires Ollama running with qwen3.5:4b pulled
async fn test_ollama_intent_parsing() {
    use aura_llm::intent::{Intent, IntentParser};

    let provider = OllamaProvider::new(OllamaConfig::default());
    let parser = IntentParser::new(Box::new(provider));
    let intent = parser.parse("open Safari").await.unwrap();
    assert!(
        matches!(intent, Intent::OpenApp { ref name } if name.to_lowercase().contains("safari")),
        "Expected OpenApp with Safari, got: {intent:?}"
    );
}
```

**Step 6: Run tests**

```bash
cargo test -p aura-llm -- --include-ignored  # with Ollama running
cargo test -p aura-llm                        # without Ollama (ignored tests skip)
```

**Step 7: Commit**

```bash
git add crates/aura-llm/ crates/aura-daemon/Cargo.toml
git rm crates/aura-llm/src/llamacpp.rs
git commit -m "feat: replace llama-cpp-2 with Ollama HTTP provider (Qwen3.5-4B)"
```

---

### Task 2: Voice Activity Detection Module

**Why:** The voice pipeline needs to detect when the user starts and stops speaking. A simple energy-based VAD works well for close-mic desktop use — no model needed.

**Files:**
- Create: `crates/aura-voice/src/vad.rs`
- Modify: `crates/aura-voice/src/lib.rs`
- Create: `crates/aura-voice/tests/vad_test.rs`

**Step 1: Create VAD**

```rust
// crates/aura-voice/src/vad.rs

/// Simple energy-based voice activity detector.
pub struct EnergyVad {
    threshold: f32,
    silence_frames_needed: usize,
    silent_count: usize,
    active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadState {
    Silence,
    SpeechStart,
    Speech,
    SpeechEnd,
}

impl EnergyVad {
    pub fn new(threshold: f32, silence_frames_needed: usize) -> Self {
        Self {
            threshold,
            silence_frames_needed,
            silent_count: 0,
            active: false,
        }
    }

    pub fn process(&mut self, audio: &[f32]) -> VadState {
        let energy = if audio.is_empty() {
            0.0
        } else {
            audio.iter().map(|s| s * s).sum::<f32>() / audio.len() as f32
        };

        if energy > self.threshold {
            self.silent_count = 0;
            if !self.active {
                self.active = true;
                return VadState::SpeechStart;
            }
            VadState::Speech
        } else if self.active {
            self.silent_count += 1;
            if self.silent_count >= self.silence_frames_needed {
                self.active = false;
                self.silent_count = 0;
                return VadState::SpeechEnd;
            }
            VadState::Speech
        } else {
            VadState::Silence
        }
    }

    pub fn reset(&mut self) {
        self.active = false;
        self.silent_count = 0;
    }

    pub fn is_active(&self) -> bool {
        self.active
    }
}
```

**Step 2: Register module**

Add `pub mod vad;` to `crates/aura-voice/src/lib.rs`.

**Step 3: Write tests**

```rust
// crates/aura-voice/tests/vad_test.rs
use aura_voice::vad::{EnergyVad, VadState};

#[test]
fn test_silence_stays_silent() {
    let mut vad = EnergyVad::new(0.001, 3);
    let silence = vec![0.0_f32; 160];
    assert_eq!(vad.process(&silence), VadState::Silence);
    assert!(!vad.is_active());
}

#[test]
fn test_speech_start_detected() {
    let mut vad = EnergyVad::new(0.001, 3);
    let loud = vec![0.1_f32; 160]; // energy = 0.01, well above 0.001
    assert_eq!(vad.process(&loud), VadState::SpeechStart);
    assert!(vad.is_active());
}

#[test]
fn test_speech_continues() {
    let mut vad = EnergyVad::new(0.001, 3);
    let loud = vec![0.1_f32; 160];
    vad.process(&loud); // SpeechStart
    assert_eq!(vad.process(&loud), VadState::Speech);
}

#[test]
fn test_speech_end_after_silence_frames() {
    let mut vad = EnergyVad::new(0.001, 3);
    let loud = vec![0.1_f32; 160];
    let silence = vec![0.0_f32; 160];

    vad.process(&loud); // SpeechStart
    assert_eq!(vad.process(&silence), VadState::Speech); // 1 silent
    assert_eq!(vad.process(&silence), VadState::Speech); // 2 silent
    assert_eq!(vad.process(&silence), VadState::SpeechEnd); // 3 silent = end
    assert!(!vad.is_active());
}

#[test]
fn test_brief_pause_doesnt_end_speech() {
    let mut vad = EnergyVad::new(0.001, 3);
    let loud = vec![0.1_f32; 160];
    let silence = vec![0.0_f32; 160];

    vad.process(&loud); // SpeechStart
    vad.process(&silence); // 1 silent
    assert_eq!(vad.process(&loud), VadState::Speech); // speech resumes
    assert!(vad.is_active());
}

#[test]
fn test_reset() {
    let mut vad = EnergyVad::new(0.001, 3);
    let loud = vec![0.1_f32; 160];
    vad.process(&loud);
    assert!(vad.is_active());
    vad.reset();
    assert!(!vad.is_active());
}

#[test]
fn test_empty_audio() {
    let mut vad = EnergyVad::new(0.001, 3);
    assert_eq!(vad.process(&[]), VadState::Silence);
}
```

**Step 4: Run tests**

```bash
cargo test -p aura-voice
```

**Step 5: Commit**

```bash
git add crates/aura-voice/
git commit -m "feat: add energy-based voice activity detection"
```

---

### Task 3: Rewrite Voice Pipeline with Real Audio → VAD → STT

**Why:** The pipeline skeleton has a state machine but doesn't capture audio, run VAD, or call STT. This task makes it real — a long-running async task that captures from mic, detects speech boundaries, and transcribes.

**Files:**
- Rewrite: `crates/aura-voice/src/pipeline.rs`
- Update: `crates/aura-voice/tests/pipeline_test.rs`

**Step 1: Rewrite pipeline.rs**

Key design decisions:
- Bridge cpal's `std::sync::mpsc` to `tokio::sync::mpsc` via a dedicated thread
- Wrap `SpeechToText` in `Arc` so `spawn_blocking` can borrow it
- VAD detects speech start/end, audio accumulates only during speech
- Max 30s recording cap prevents runaway buffers

```rust
// crates/aura-voice/src/pipeline.rs
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::audio::{AudioCapture, SAMPLE_RATE};
use crate::stt::{SpeechToText, SttConfig};
use crate::vad::{EnergyVad, VadState};
use crate::wakeword::{WakeWordConfig, WakeWordDetector};

const VAD_ENERGY_THRESHOLD: f32 = 0.0005;
const VAD_SILENCE_FRAMES: usize = 25; // ~1.5s at 60ms/chunk
const MAX_AUDIO_SAMPLES: usize = SAMPLE_RATE as usize * 30;

#[derive(Debug, Clone)]
pub enum VoiceEvent {
    WakeWordDetected,
    ListeningStarted,
    Transcription { text: String },
    ListeningStopped,
    Error { message: String },
}

#[derive(Debug, Clone)]
pub struct VoicePipelineConfig {
    pub use_wakeword: bool,
    pub wakeword_model_path: Option<String>,
    pub stt_config: SttConfig,
}

impl Default for VoicePipelineConfig {
    fn default() -> Self {
        Self {
            use_wakeword: false,
            wakeword_model_path: None,
            stt_config: SttConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineState {
    Idle,
    Listening,
    Transcribing,
}

/// Runs the voice pipeline as a long-lived async task.
/// Captures mic audio, detects speech via VAD, transcribes via whisper, emits events.
pub async fn run_voice_pipeline(
    config: VoicePipelineConfig,
    event_tx: mpsc::Sender<VoiceEvent>,
) -> Result<()> {
    // Bridge cpal's std::sync::mpsc → tokio::sync::mpsc
    let (async_tx, mut async_rx) = mpsc::channel::<Vec<f32>>(64);
    let capture = AudioCapture::new(None)?;
    let (sync_tx, sync_rx) = std::sync::mpsc::channel::<Vec<f32>>();
    let _stream = capture.start(sync_tx)?;

    std::thread::spawn(move || {
        while let Ok(chunk) = sync_rx.recv() {
            if async_tx.blocking_send(chunk).is_err() {
                break;
            }
        }
    });

    // Load whisper model (CPU-heavy, do off-thread)
    let stt_config = config.stt_config.clone();
    let stt = Arc::new(
        tokio::task::spawn_blocking(move || SpeechToText::new(stt_config)).await??,
    );

    // Optional wake word
    let mut wakeword: Option<WakeWordDetector> = if config.use_wakeword {
        if let Some(ref path) = config.wakeword_model_path {
            let mut det = WakeWordDetector::new(WakeWordConfig {
                threshold: 0.5,
                avg_threshold: 0.25,
            })?;
            det.add_wakeword_from_file(path)?;
            Some(det)
        } else {
            tracing::warn!("Wake word enabled but no model — falling back to VAD");
            None
        }
    } else {
        None
    };

    let mut vad = EnergyVad::new(VAD_ENERGY_THRESHOLD, VAD_SILENCE_FRAMES);
    let mut state = PipelineState::Idle;
    let mut audio_buffer: Vec<f32> = Vec::new();

    tracing::info!(wakeword = config.use_wakeword, "Voice pipeline started");

    while let Some(chunk) = async_rx.recv().await {
        match state {
            PipelineState::Idle => {
                if let Some(ref mut ww) = wakeword {
                    if ww.process(&chunk) {
                        let _ = event_tx.send(VoiceEvent::WakeWordDetected).await;
                        let _ = event_tx.send(VoiceEvent::ListeningStarted).await;
                        state = PipelineState::Listening;
                        audio_buffer.clear();
                        vad.reset();
                    }
                } else if let VadState::SpeechStart = vad.process(&chunk) {
                    let _ = event_tx.send(VoiceEvent::ListeningStarted).await;
                    state = PipelineState::Listening;
                    audio_buffer.clear();
                    audio_buffer.extend_from_slice(&chunk);
                }
            }
            PipelineState::Listening => {
                audio_buffer.extend_from_slice(&chunk);

                let should_transcribe = if audio_buffer.len() > MAX_AUDIO_SAMPLES {
                    tracing::warn!("Max recording length reached");
                    true
                } else {
                    matches!(vad.process(&chunk), VadState::SpeechEnd)
                };

                if should_transcribe {
                    state = PipelineState::Transcribing;
                    let duration = audio_buffer.len() as f32 / SAMPLE_RATE as f32;
                    tracing::info!(duration_secs = duration, "Transcribing speech");

                    let audio = std::mem::take(&mut audio_buffer);
                    let stt_clone = Arc::clone(&stt);
                    match tokio::task::spawn_blocking(move || stt_clone.transcribe(&audio)).await {
                        Ok(Ok(text)) if !text.is_empty() => {
                            tracing::info!(text = %text, "Transcription complete");
                            let _ = event_tx
                                .send(VoiceEvent::Transcription { text })
                                .await;
                        }
                        Ok(Ok(_)) => {
                            tracing::debug!("Empty transcription, skipping");
                        }
                        Ok(Err(e)) => {
                            tracing::error!(%e, "STT failed");
                            let _ = event_tx
                                .send(VoiceEvent::Error {
                                    message: format!("Transcription failed: {e}"),
                                })
                                .await;
                        }
                        Err(e) => {
                            tracing::error!(%e, "STT task panicked");
                        }
                    }

                    let _ = event_tx.send(VoiceEvent::ListeningStopped).await;
                    state = PipelineState::Idle;
                    vad.reset();
                }
            }
            PipelineState::Transcribing => {
                // Drop audio while transcribing — shouldn't reach here in practice
            }
        }
    }

    tracing::warn!("Audio stream ended, voice pipeline shutting down");
    Ok(())
}
```

**Step 2: Update pipeline tests**

The old tests tested the struct-based API. The new pipeline is a free function. Update tests to cover the public types and test VAD integration (the actual pipeline needs a mic so it's tested via smoke test in Task 10).

```rust
// crates/aura-voice/tests/pipeline_test.rs
use aura_voice::pipeline::{PipelineState, VoiceEvent, VoicePipelineConfig};

#[test]
fn test_pipeline_config_defaults() {
    let config = VoicePipelineConfig::default();
    assert!(!config.use_wakeword);
    assert!(config.wakeword_model_path.is_none());
}

#[test]
fn test_pipeline_state_variants() {
    // Ensure all states exist and are comparable
    assert_ne!(PipelineState::Idle, PipelineState::Listening);
    assert_ne!(PipelineState::Listening, PipelineState::Transcribing);
}

#[test]
fn test_voice_event_debug() {
    // Ensure VoiceEvent variants are constructable
    let events = vec![
        VoiceEvent::WakeWordDetected,
        VoiceEvent::ListeningStarted,
        VoiceEvent::Transcription { text: "hello".into() },
        VoiceEvent::ListeningStopped,
        VoiceEvent::Error { message: "fail".into() },
    ];
    for e in &events {
        let _ = format!("{e:?}"); // Debug works
    }
    assert_eq!(events.len(), 5);
}
```

**Step 3: Run tests**

```bash
cargo test -p aura-voice
```

**Step 4: Commit**

```bash
git add crates/aura-voice/
git commit -m "feat: rewrite voice pipeline with real mic capture, VAD, and STT"
```

---

### Task 4: Intent-to-Action Bridge

**Why:** `Intent` (aura-llm) and `Action` (aura-bridge) are parallel enums in separate crates. The daemon needs a mapper.

**Files:**
- Create: `crates/aura-daemon/src/intent_bridge.rs`
- Modify: `crates/aura-daemon/src/lib.rs`
- Create: `crates/aura-daemon/tests/intent_bridge_test.rs`

**Step 1: Create bridge**

```rust
// crates/aura-daemon/src/intent_bridge.rs
use aura_bridge::actions::Action;
use aura_llm::intent::Intent;

/// Maps a parsed intent to an executable action.
/// Returns None for intents that have no corresponding system action.
pub fn intent_to_action(intent: &Intent) -> Option<Action> {
    match intent {
        Intent::OpenApp { name } => Some(Action::OpenApp { name: name.clone() }),
        Intent::SearchFiles { query } => Some(Action::SearchFiles { query: query.clone() }),
        Intent::TileWindows { layout } => Some(Action::TileWindows { layout: layout.clone() }),
        Intent::LaunchUrl { url } => Some(Action::LaunchUrl { url: url.clone() }),
        Intent::SummarizeScreen => None,
        Intent::Unknown { .. } => None,
    }
}
```

**Step 2: Register module**

Add `pub mod intent_bridge;` to `crates/aura-daemon/src/lib.rs`.

**Step 3: Write tests**

```rust
// crates/aura-daemon/tests/intent_bridge_test.rs
use aura_bridge::actions::Action;
use aura_daemon::intent_bridge::intent_to_action;
use aura_llm::intent::Intent;

#[test]
fn test_open_app_maps() {
    let intent = Intent::OpenApp { name: "Safari".into() };
    let action = intent_to_action(&intent).unwrap();
    assert!(matches!(action, Action::OpenApp { name } if name == "Safari"));
}

#[test]
fn test_search_maps() {
    let intent = Intent::SearchFiles { query: "notes".into() };
    let action = intent_to_action(&intent).unwrap();
    assert!(matches!(action, Action::SearchFiles { query } if query == "notes"));
}

#[test]
fn test_tile_maps() {
    let intent = Intent::TileWindows { layout: "left-right".into() };
    let action = intent_to_action(&intent).unwrap();
    assert!(matches!(action, Action::TileWindows { layout } if layout == "left-right"));
}

#[test]
fn test_launch_url_maps() {
    let intent = Intent::LaunchUrl { url: "https://example.com".into() };
    let action = intent_to_action(&intent).unwrap();
    assert!(matches!(action, Action::LaunchUrl { url } if url == "https://example.com"));
}

#[test]
fn test_unknown_maps_to_none() {
    assert!(intent_to_action(&Intent::Unknown { raw: "x".into() }).is_none());
}

#[test]
fn test_summarize_maps_to_none() {
    assert!(intent_to_action(&Intent::SummarizeScreen).is_none());
}
```

**Step 4: Run tests**

```bash
cargo test -p aura-daemon
```

**Step 5: Commit**

```bash
git add crates/aura-daemon/
git commit -m "feat: add intent-to-action bridge mapper"
```

---

### Task 5: Wire Everything in main.rs + Processor Task

**Why:** This is the integration task. main.rs spawns: voice pipeline task, voice→bus bridge task, intent+action processor task, and the daemon event loop. Everything communicates via the event bus.

**Files:**
- Rewrite: `crates/aura-daemon/src/main.rs`
- Modify: `crates/aura-daemon/src/daemon.rs` (add `run_processor` function)

**Step 1: Add processor to daemon.rs**

```rust
// Append to crates/aura-daemon/src/daemon.rs (after existing code)

use aura_bridge::actions::ActionExecutor;
use aura_llm::intent::IntentParser;
use crate::intent_bridge::intent_to_action;

/// Subscribes to VoiceCommand events, parses intent, executes action, publishes result.
pub async fn run_processor(
    bus: EventBus,
    parser: IntentParser,
    executor: Box<dyn ActionExecutor>,
) {
    let mut rx = bus.subscribe();

    loop {
        match rx.recv().await {
            Ok(AuraEvent::VoiceCommand { text }) => {
                send_event(&bus, AuraEvent::ShowOverlay {
                    content: OverlayContent::Processing,
                });

                match parser.parse(&text).await {
                    Ok(intent) => {
                        tracing::info!(?intent, "Intent parsed");
                        send_event(&bus, AuraEvent::IntentParsed { intent: intent.clone() });

                        match intent_to_action(&intent) {
                            Some(action) => {
                                let result = executor.execute(&action).await;
                                if result.success {
                                    send_event(&bus, AuraEvent::ActionExecuted {
                                        description: result.description,
                                    });
                                } else {
                                    send_event(&bus, AuraEvent::ActionFailed {
                                        description: format!("{action:?}"),
                                        error: result.description,
                                    });
                                }
                            }
                            None => {
                                let msg = match &intent {
                                    aura_llm::intent::Intent::Unknown { raw } => {
                                        format!("I heard: \"{raw}\" — not sure what to do.")
                                    }
                                    _ => format!("Intent {intent:?} isn't actionable yet."),
                                };
                                send_event(&bus, AuraEvent::ShowOverlay {
                                    content: OverlayContent::Response { text: msg },
                                });
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(%e, "Intent parsing failed");
                        send_event(&bus, AuraEvent::ActionFailed {
                            description: "Intent parsing".into(),
                            error: e.to_string(),
                        });
                    }
                }
            }
            Ok(AuraEvent::Shutdown) => {
                tracing::info!("Processor shutting down");
                break;
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "Processor lagged behind event bus");
            }
            Err(e) => {
                tracing::error!(%e, "Processor bus error");
                break;
            }
        }
    }
}
```

**Step 2: Rewrite main.rs**

```rust
// crates/aura-daemon/src/main.rs
use anyhow::Result;
use aura_bridge::macos::MacOSExecutor;
use aura_daemon::bus::EventBus;
use aura_daemon::daemon::{run_processor, Daemon};
use aura_daemon::event::{AuraEvent, OverlayContent};
use aura_daemon::setup::AuraSetup;
use aura_llm::intent::IntentParser;
use aura_llm::ollama::{OllamaConfig, OllamaProvider};
use aura_voice::pipeline::{run_voice_pipeline, VoiceEvent, VoicePipelineConfig};
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

const EVENT_BUS_CAPACITY: usize = 64;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!("Aura starting...");

    // First-run setup: create directories, check models
    let setup = AuraSetup::new(AuraSetup::default_data_dir());
    setup.ensure_dirs()?;
    setup.print_status();

    let status = setup.check();
    if !status.whisper_model_ready {
        tracing::error!("Whisper model not found. Download it:");
        tracing::error!("  mkdir -p ~/Library/Application\\ Support/aura/models");
        tracing::error!("  curl -L -o ~/Library/Application\\ Support/aura/models/ggml-base.en.bin \\");
        tracing::error!("    https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin");
        anyhow::bail!("Whisper STT model missing. See instructions above.");
    }

    // Check Ollama
    let ollama_config = OllamaConfig::default();
    let ollama = OllamaProvider::new(ollama_config);
    ollama.health_check().await?;
    tracing::info!("Ollama connected, model ready");

    let bus = EventBus::new(EVENT_BUS_CAPACITY);

    // --- Voice pipeline ---
    let (voice_tx, mut voice_rx) = mpsc::channel::<VoiceEvent>(32);
    let voice_bus = bus.clone();

    // Bridge: VoiceEvent → AuraEvent
    tokio::spawn(async move {
        while let Some(event) = voice_rx.recv().await {
            let aura_event = match event {
                VoiceEvent::WakeWordDetected => AuraEvent::WakeWordDetected,
                VoiceEvent::ListeningStarted => {
                    AuraEvent::ShowOverlay { content: OverlayContent::Listening }
                }
                VoiceEvent::Transcription { text } => AuraEvent::VoiceCommand { text },
                VoiceEvent::ListeningStopped => AuraEvent::ListeningStopped,
                VoiceEvent::Error { message } => {
                    AuraEvent::ShowOverlay { content: OverlayContent::Error { message } }
                }
            };
            if let Err(e) = voice_bus.send(aura_event) {
                tracing::warn!("Voice bridge send failed: {e}");
            }
        }
    });

    tokio::spawn(async move {
        let config = VoicePipelineConfig::default();
        if let Err(e) = run_voice_pipeline(config, voice_tx).await {
            tracing::error!("Voice pipeline crashed: {e}");
        }
    });

    // --- Intent + Action processor ---
    let parser = IntentParser::new(Box::new(ollama));
    let executor: Box<dyn aura_bridge::actions::ActionExecutor> =
        Box::new(MacOSExecutor::new());
    let processor_bus = bus.clone();
    tokio::spawn(async move {
        run_processor(processor_bus, parser, executor).await;
    });

    // --- Main event loop ---
    let daemon = Daemon::new(bus);
    daemon.run().await?;

    tracing::info!("Aura shut down.");
    Ok(())
}
```

**Step 3: Build**

```bash
cargo build -p aura-daemon
```

**Step 4: Commit**

```bash
git add crates/aura-daemon/
git commit -m "feat: wire voice, Ollama intent parsing, and action execution in daemon"
```

---

### Task 6: Wire Skia Overlay with Softbuffer

**Why:** The overlay window exists but doesn't render anything — RedrawRequested is empty and there's no Skia surface. We need to create a CPU raster surface via softbuffer, render with OverlayRenderer on each frame, and accept messages from async tasks via EventLoopProxy.

**Files:**
- Rewrite: `crates/aura-overlay/src/window.rs`
- Modify: `crates/aura-overlay/Cargo.toml` (add softbuffer)

**Step 1: Add softbuffer dependency**

```toml
# Add to crates/aura-overlay/Cargo.toml [dependencies]
softbuffer = "0.4"
```

**Step 2: Rewrite window.rs**

The implementer should build this with:

1. `EventLoop<OverlayMessage>` with user events for Show/Hide/UpdateState
2. `ApplicationHandler<OverlayMessage>` impl on `OverlayApp`
3. On `Resumed`: create window (transparent, no decorations, always-on-top, initially hidden), create `softbuffer::Surface`, create `OverlayRenderer`
4. On `RedrawRequested`: create Skia raster surface → call `renderer.render(canvas, state, dt)` → read pixels → convert RGBA→XRGB (softbuffer uses 0RGB) → present buffer
5. On `UserEvent(Show { state })`: update state, show window, request_redraw
6. On `UserEvent(Hide)`: hide window
7. Continuous animation: after each RedrawRequested when visible, schedule next frame via `request_redraw()`
8. Track `last_frame: Instant` for delta-time calculation

**Key types to export:**

```rust
#[derive(Debug, Clone)]
pub enum OverlayMessage {
    Show { state: OverlayState },
    Hide,
    UpdateState { state: OverlayState },
}

pub struct OverlayApp { /* internal */ }

impl OverlayApp {
    pub fn new() -> Self;
}

pub fn create_event_loop() -> Result<EventLoop<OverlayMessage>>;
```

**RGBA→XRGB conversion** (Skia outputs RGBA8888, softbuffer expects 0RGB32):

```rust
fn rgba_to_xrgb(rgba: &[u8]) -> Vec<u32> {
    rgba.chunks_exact(4)
        .map(|px| {
            let r = px[0] as u32;
            let g = px[1] as u32;
            let b = px[2] as u32;
            (r << 16) | (g << 8) | b
        })
        .collect()
}
```

**Step 3: Build**

```bash
cargo build -p aura-overlay
```

**Step 4: Commit**

```bash
git add crates/aura-overlay/
git commit -m "feat: wire Skia renderer to winit window with softbuffer blitting"
```

---

### Task 7: CLI with clap + Overlay Integration in main.rs

**Why:** Users need CLI flags to control behavior: `--no-overlay` for headless testing, `--verbose` for debug logs, `--ollama-url` for custom Ollama instances. And main.rs needs to conditionally start the overlay on the main thread.

**Files:**
- Modify: `crates/aura-daemon/Cargo.toml` (add clap)
- Rewrite: `crates/aura-daemon/src/main.rs` (add CLI + overlay)

**Step 1: Add clap to Cargo.toml**

```toml
clap = { version = "4", features = ["derive"] }
```

**Step 2: Add CLI struct and overlay wiring**

```rust
use clap::Parser;

#[derive(Parser)]
#[command(name = "aura", about = "Voice-first AI desktop companion")]
struct Cli {
    /// Skip overlay (terminal-only mode for debugging)
    #[arg(long)]
    no_overlay: bool,

    /// Enable wake word detection
    #[arg(long)]
    wakeword: bool,

    /// Path to wake word model file (.rpw)
    #[arg(long)]
    wakeword_model: Option<String>,

    /// Ollama server URL
    #[arg(long, default_value = "http://localhost:11434")]
    ollama_url: String,

    /// Ollama model name
    #[arg(long, default_value = "qwen3.5:4b")]
    ollama_model: String,

    /// Enable debug logging
    #[arg(short, long)]
    verbose: bool,
}
```

**Overlay integration pattern:**

When `--no-overlay` is NOT set, the main thread runs the winit event loop. The tokio runtime runs in a separate thread. When `--no-overlay` IS set, tokio runs on main thread as before.

```rust
fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.no_overlay {
        // Tokio on main thread, no overlay
        tokio::runtime::Runtime::new()?.block_on(run_headless(cli))
    } else {
        // Winit on main thread, tokio on background thread
        let event_loop = aura_overlay::window::create_event_loop()?;
        let proxy = event_loop.create_proxy();

        let rt = tokio::runtime::Runtime::new()?;
        rt.spawn(async move {
            if let Err(e) = run_with_overlay(cli, proxy).await {
                tracing::error!("Aura error: {e}");
            }
        });

        let mut app = aura_overlay::window::OverlayApp::new();
        event_loop.run_app(&mut app)?;
        Ok(())
    }
}
```

**Step 3: Build and verify**

```bash
cargo build -p aura-daemon
```

**Step 4: Commit**

```bash
git add crates/aura-daemon/
git commit -m "feat: add CLI flags and overlay integration on main thread"
```

---

### Task 8: Error Handling, Resilience, and Graceful Shutdown

**Why:** Production apps need: Ollama reconnection if it's temporarily unavailable, graceful shutdown that cleans up all tasks, and proper error propagation that doesn't crash the whole app.

**Files:**
- Modify: `crates/aura-llm/src/ollama.rs` (retry logic)
- Modify: `crates/aura-daemon/src/daemon.rs` (RecvError::Lagged handling, CancellationToken)
- Modify: `crates/aura-daemon/src/main.rs` (graceful shutdown)

**Step 1: Add retry to Ollama provider**

Wrap the HTTP call with a simple retry (3 attempts, 1s backoff) for transient network errors. Do NOT retry on 4xx errors (bad request/model not found).

```rust
// In ollama.rs complete(), wrap the request:
const MAX_RETRIES: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(1);

for attempt in 1..=MAX_RETRIES {
    match self.try_complete(prompt).await {
        Ok(response) => return Ok(response),
        Err(e) if attempt < MAX_RETRIES && is_transient(&e) => {
            tracing::warn!(attempt, %e, "Ollama request failed, retrying");
            tokio::time::sleep(RETRY_DELAY).await;
        }
        Err(e) => return Err(e),
    }
}
```

**Step 2: Handle RecvError::Lagged in daemon event loop**

Replace the catch-all `Err(e)` in daemon.rs with explicit handling:

```rust
Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
    tracing::warn!(skipped = n, "Event bus lagged, some events were dropped");
    // Continue processing — lagging is recoverable
}
Err(tokio::sync::broadcast::error::RecvError::Closed) => {
    tracing::info!("Event bus closed, shutting down");
    break;
}
```

**Step 3: Add tokio_util::CancellationToken for coordinated shutdown**

Add `tokio-util = "0.7"` to aura-daemon's Cargo.toml. Pass a CancellationToken to all spawned tasks. On Ctrl+C or Shutdown event, cancel the token so all tasks wind down.

**Step 4: Run tests**

```bash
cargo test --workspace
```

**Step 5: Commit**

```bash
git add crates/
git commit -m "feat: add retry logic, graceful shutdown, and lagged event handling"
```

---

### Task 9: Integration Tests

**Why:** We have 49 unit tests but no integration tests that verify the components work together. Add tests that exercise: event bus → processor → action flow, and Ollama → intent parsing (when available).

**Files:**
- Create: `crates/aura-daemon/tests/integration_test.rs`

**Step 1: Write integration test (mock-based, no real audio/Ollama)**

```rust
// crates/aura-daemon/tests/integration_test.rs
use aura_bridge::actions::MockExecutor;
use aura_daemon::bus::EventBus;
use aura_daemon::daemon::run_processor;
use aura_daemon::event::{AuraEvent, OverlayContent};
use aura_llm::intent::IntentParser;
use aura_llm::provider::MockProvider;
use std::time::Duration;

#[tokio::test]
async fn test_voice_command_to_action_executed() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();

    // Mock LLM returns open_app intent
    let provider = MockProvider::new(vec![
        ("open Safari", r#"{"type":"open_app","name":"Safari"}"#),
    ]);
    let parser = IntentParser::new(Box::new(provider));
    let executor: Box<dyn aura_bridge::actions::ActionExecutor> =
        Box::new(MockExecutor::new());

    let processor_bus = bus.clone();
    tokio::spawn(async move {
        run_processor(processor_bus, parser, executor).await;
    });

    // Give processor time to subscribe
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send a voice command
    bus.send(AuraEvent::VoiceCommand { text: "open Safari".into() }).unwrap();

    // Collect events — we should see Processing overlay, IntentParsed, ActionExecuted
    let mut got_intent = false;
    let mut got_action = false;

    let timeout = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match rx.recv().await {
                Ok(AuraEvent::IntentParsed { .. }) => got_intent = true,
                Ok(AuraEvent::ActionExecuted { description }) => {
                    got_action = true;
                    assert!(description.contains("Safari"));
                    break;
                }
                Ok(_) => {} // ShowOverlay::Processing, etc.
                Err(e) => panic!("Bus error: {e}"),
            }
        }
    });

    timeout.await.expect("Timed out waiting for ActionExecuted");
    assert!(got_intent, "Should have received IntentParsed");
    assert!(got_action, "Should have received ActionExecuted");

    // Shutdown
    bus.send(AuraEvent::Shutdown).unwrap();
}

#[tokio::test]
async fn test_unknown_intent_shows_response() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();

    let provider = MockProvider::new(vec![]);
    let parser = IntentParser::new(Box::new(provider));
    let executor: Box<dyn aura_bridge::actions::ActionExecutor> =
        Box::new(MockExecutor::new());

    let processor_bus = bus.clone();
    tokio::spawn(async move {
        run_processor(processor_bus, parser, executor).await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    bus.send(AuraEvent::VoiceCommand { text: "what's the weather".into() }).unwrap();

    let timeout = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match rx.recv().await {
                Ok(AuraEvent::ShowOverlay { content: OverlayContent::Response { text } }) => {
                    assert!(text.contains("not sure"));
                    break;
                }
                Ok(_) => {}
                Err(e) => panic!("Bus error: {e}"),
            }
        }
    });

    timeout.await.expect("Timed out waiting for Response overlay");
    bus.send(AuraEvent::Shutdown).unwrap();
}
```

**Step 2: Enable test-support feature for integration test**

The MockProvider and MockExecutor require the `test-support` feature. Add to `crates/aura-daemon/Cargo.toml`:

```toml
[dev-dependencies]
tempfile = "3"
aura-llm = { path = "../aura-llm", features = ["test-support"] }
aura-bridge = { path = "../aura-bridge", features = ["test-support"] }
```

And add `test-support` feature to aura-bridge/Cargo.toml:

```toml
[features]
test-support = []
```

**Step 3: Run all tests**

```bash
cargo test --workspace
```

**Step 4: Commit**

```bash
git add crates/
git commit -m "test: add integration tests for voice command → action flow"
```

---

### Task 10: Smoke Test — End-to-End Run

**Why:** Verify the complete pipeline on real hardware. This is manual testing.

**Prerequisites:**

```bash
# Ensure Ollama is running with the model
ollama serve &
ollama pull qwen3.5:4b

# Ensure whisper model exists
ls ~/Library/Application\ Support/aura/models/ggml-base.en.bin
```

**Test 1: Headless mode**

```bash
RUST_LOG=info cargo run -- --no-overlay
```

Speak "open Safari". Expected logs:
```
INFO aura_voice::pipeline: Voice pipeline started wakeword=false
INFO aura_voice::pipeline: Transcription complete text="open Safari"
INFO aura_daemon::daemon: Voice command received command="open Safari"
INFO aura_daemon::daemon: Intent parsed intent=OpenApp { name: "Safari" }
INFO aura_bridge::macos: Opening application app="Safari"
INFO aura_daemon::daemon: Action executed description="Opened Safari"
```

Safari should open.

**Test 2: With overlay**

```bash
RUST_LOG=info cargo run
```

Speak "open Safari". Expected:
- Transparent overlay appears with listening waveform
- Transitions to processing orb
- Shows "Opened Safari" response card
- Card dissolves after 3 seconds
- Safari opens

**Test 3: Error cases**

- "open NonExistentApp123" → error card with failure message
- "blah blah random words" → "not sure what to do" response card
- Kill Ollama mid-session → error card, retry on next command

**Test 4: Ctrl+C**

Press Ctrl+C → clean shutdown, no panics.

**Fix any issues found, commit:**

```bash
git add -A
git commit -m "fix: smoke test fixes from end-to-end testing"
```

---

### Task 11: Production Cleanup

**Why:** Remove dead code, ensure all warnings are clean, update setup.rs for Ollama, final polish.

**Files:**
- Delete: `crates/aura-llm/src/llamacpp.rs` (if not already deleted in Task 1)
- Modify: `crates/aura-daemon/src/setup.rs` (check for Ollama instead of LLM model file)
- Modify: `crates/aura-llm/Cargo.toml` (remove llama-cpp-2, llamacpp feature entirely)
- Run: `cargo clippy --workspace` and fix all warnings

**Step 1: Update setup.rs**

Replace `llm_model_ready` (checks for GGUF file) with `ollama_ready` (checks if Ollama is reachable):

```rust
pub struct SetupStatus {
    pub whisper_model_ready: bool,
    pub ollama_reachable: bool,
    pub piper_ready: bool,
    pub wakeword_model_ready: bool,
}

impl SetupStatus {
    pub fn is_ready(&self) -> bool {
        self.whisper_model_ready && self.ollama_reachable
    }
}
```

The `check()` method should try to connect to Ollama at `http://localhost:11434/api/tags`. Since check() is sync, use a simple `std::net::TcpStream::connect_timeout` to port 11434 instead of making an HTTP call.

**Step 2: Clean up warnings**

```bash
cargo clippy --workspace -- -D warnings
```

Fix everything.

**Step 3: Run full test suite**

```bash
cargo test --workspace
```

All tests pass, no warnings.

**Step 4: Commit**

```bash
git add -A
git commit -m "chore: production cleanup — remove llama-cpp-2, update setup checks, fix clippy"
```

---

## Task Dependency Graph

```
Task 1 (Ollama provider)  ──┐
Task 2 (VAD module)        ──┤
Task 3 (Voice pipeline)   ──┼── Task 5 (Wire main.rs) ── Task 7 (CLI + overlay) ── Task 8 (Resilience) ── Task 10 (Smoke test) ── Task 11 (Cleanup)
Task 4 (Intent bridge)    ──┤
Task 6 (Skia + softbuffer) ─┘
                              └── Task 9 (Integration tests)
```

**Parallel groups:**
- Group A: Tasks 1, 2, 4, 6 (fully independent)
- Group B: Task 3 (depends on Task 2)
- Group C: Task 5 (depends on 1, 3, 4)
- Group D: Tasks 7, 9 (depend on 5; independent of each other)
- Group E: Task 6 can be done anytime, integrated in Task 7
- Group F: Tasks 8, 10, 11 (sequential, after everything else)

## Definition of Done

- [ ] `cargo run -- --no-overlay` works: speak → transcribe → intent → action → log output
- [ ] `cargo run` works: same flow + overlay renders all states
- [ ] `cargo test --workspace` passes (all existing + new tests)
- [ ] `cargo clippy --workspace -- -D warnings` clean
- [ ] Ctrl+C shuts down cleanly
- [ ] Ollama not running → clear error message at startup
- [ ] Whisper model missing → clear error message with download instructions
- [ ] Unknown commands → graceful "not sure what to do" response
- [ ] 3+ second silence after speaking → transcription triggers
- [ ] No panics, no unsafe code, no dead code warnings
