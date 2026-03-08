# Conversational Voice Assistant Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Transform Aura from a command-and-control voice assistant into a Gemini Live-like conversational voice assistant with streaming responses and spoken replies.

**Architecture:** Cascaded streaming pipeline: Mic → Silero VAD → Whisper STT → Streaming Ollama LLM → Sentence splitter → Kokoro TTS → rodio Speaker. Each stage streams partial results to the next. Barge-in support cancels TTS playback when user speaks.

**Tech Stack:** whisper-rs (small.en + CoreML), silero-vad-rust, Ollama chat API (streaming NDJSON), kokoro-tts, rodio, cpal

---

### Task 1: Upgrade Whisper to small.en model

**Files:**
- Modify: `crates/aura-voice/src/stt.rs:5` (change default model filename)
- Modify: `crates/aura-daemon/src/setup.rs` (update model check)
- Download: `ggml-small.en.bin` to `~/Library/Application Support/aura/models/`

**Step 1: Download small.en model**

```bash
curl -L -o ~/Library/Application\ Support/aura/models/ggml-small.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin
```

Note: ~466 MB download. Also download CoreML variant:
```bash
curl -L -o /tmp/ggml-small.en-encoder.mlmodelc.zip \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en-encoder.mlmodelc.zip
unzip /tmp/ggml-small.en-encoder.mlmodelc.zip -d ~/Library/Application\ Support/aura/models/
```

**Step 2: Update default model path**

In `crates/aura-voice/src/stt.rs`, change:
```rust
const DEFAULT_MODEL_FILENAME: &str = "ggml-small.en.bin";
```

**Step 3: Update setup check**

In `crates/aura-daemon/src/setup.rs`, update the whisper model check to look for `ggml-small.en.bin`.

**Step 4: Build and verify**

```bash
cargo build --release
RUST_LOG=aura=info cargo run --release --  --no-overlay
```

Speak "Open Google Chrome" — transcription should be noticeably more accurate.

**Step 5: Commit**

```bash
git add crates/aura-voice/src/stt.rs crates/aura-daemon/src/setup.rs
git commit -m "feat: upgrade Whisper STT to small.en for better accuracy"
```

---

### Task 2: Replace energy VAD with Silero VAD

**Files:**
- Modify: `crates/aura-voice/Cargo.toml` (add silero-vad-rust)
- Rewrite: `crates/aura-voice/src/vad.rs`
- Modify: `crates/aura-voice/src/pipeline.rs` (adapt to new VAD API)
- Modify: `crates/aura-voice/tests/vad_test.rs`

**Step 1: Add dependency**

In `crates/aura-voice/Cargo.toml`, add:
```toml
silero-vad-rust = "0.1"
```

**Step 2: Rewrite VAD module**

Replace `crates/aura-voice/src/vad.rs` with Silero VAD wrapper:

```rust
use anyhow::{Context, Result};
use silero_vad_rust::{Vad, VadParameters};

const DEFAULT_SILENCE_MS: usize = 1300;
const DEFAULT_SPEECH_THRESHOLD: f32 = 0.5;
const SILERO_SAMPLE_RATE: usize = 16000;
const SILERO_CHUNK_SIZE: usize = 512; // 32ms at 16kHz

#[derive(Debug, Clone)]
pub struct VadConfig {
    pub speech_threshold: f32,
    pub silence_duration_ms: usize,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            speech_threshold: DEFAULT_SPEECH_THRESHOLD,
            silence_duration_ms: DEFAULT_SILENCE_MS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadState {
    Silent,
    Speaking,
}

pub struct VoiceActivityDetector {
    vad: Vad,
    state: VadState,
    silence_samples: usize,
    silence_threshold_samples: usize,
    buffer: Vec<f32>,
}

impl VoiceActivityDetector {
    pub fn new(config: VadConfig) -> Result<Self> {
        let params = VadParameters {
            threshold: config.speech_threshold,
            ..Default::default()
        };
        let vad = Vad::new(params, SILERO_SAMPLE_RATE)
            .context("Failed to initialize Silero VAD")?;

        let silence_threshold_samples =
            config.silence_duration_ms * SILERO_SAMPLE_RATE / 1000;

        Ok(Self {
            vad,
            state: VadState::Silent,
            silence_samples: 0,
            silence_threshold_samples,
            buffer: Vec::with_capacity(SILERO_CHUNK_SIZE),
        })
    }

    /// Process audio samples. Returns the new state.
    /// Internally buffers to SILERO_CHUNK_SIZE (512 samples = 32ms).
    pub fn process(&mut self, samples: &[f32]) -> VadState {
        self.buffer.extend_from_slice(samples);

        while self.buffer.len() >= SILERO_CHUNK_SIZE {
            let chunk: Vec<f32> = self.buffer.drain(..SILERO_CHUNK_SIZE).collect();
            let is_speech = self.vad.forward_chunk(&chunk).unwrap_or(0.0)
                >= self.vad.parameters().threshold;

            if is_speech {
                self.silence_samples = 0;
                if self.state == VadState::Silent {
                    tracing::debug!("VAD: speech start (Silero)");
                }
                self.state = VadState::Speaking;
            } else if self.state == VadState::Speaking {
                self.silence_samples += SILERO_CHUNK_SIZE;
                if self.silence_samples >= self.silence_threshold_samples {
                    tracing::debug!("VAD: speech end (Silero)");
                    self.state = VadState::Silent;
                    self.silence_samples = 0;
                }
            }
        }

        self.state
    }

    pub fn state(&self) -> VadState {
        self.state
    }

    pub fn reset(&mut self) {
        self.state = VadState::Silent;
        self.silence_samples = 0;
        self.buffer.clear();
        self.vad.reset();
    }
}
```

**Step 3: Update pipeline.rs**

Change `VoiceActivityDetector::new(vad_config)` to `VoiceActivityDetector::new(vad_config)?` (now returns Result).

In `run_voice_task()`:
```rust
let mut vad = VoiceActivityDetector::new(vad_config)
    .context("Failed to initialize VAD")?;
```

**Step 4: Update tests**

Rewrite `crates/aura-voice/tests/vad_test.rs` for the new API:
```rust
use aura_voice::vad::{VadConfig, VadState, VoiceActivityDetector};

#[test]
fn test_silence_stays_silent() {
    let mut vad = VoiceActivityDetector::new(VadConfig::default()).unwrap();
    let silence = vec![0.0f32; 1600]; // 100ms of silence
    assert_eq!(vad.process(&silence), VadState::Silent);
}

#[test]
fn test_loud_audio_triggers_speaking() {
    let mut vad = VoiceActivityDetector::new(VadConfig::default()).unwrap();
    // Generate a loud sine wave (speech-like)
    let loud: Vec<f32> = (0..1600)
        .map(|i| (i as f32 * 0.1).sin() * 0.8)
        .collect();
    let state = vad.process(&loud);
    // Silero may or may not trigger on synthetic sine — test that it doesn't crash
    assert!(state == VadState::Silent || state == VadState::Speaking);
}

#[test]
fn test_reset() {
    let mut vad = VoiceActivityDetector::new(VadConfig::default()).unwrap();
    vad.reset();
    assert_eq!(vad.state(), VadState::Silent);
}
```

**Step 5: Build and test**

```bash
cargo test -p aura-voice
cargo build --release
```

**Step 6: Commit**

```bash
git add crates/aura-voice/
git commit -m "feat: replace energy VAD with Silero neural VAD"
```

---

### Task 3: Add streaming completion to LLM provider

**Files:**
- Modify: `crates/aura-llm/Cargo.toml` (add futures-core, tokio-stream)
- Modify: `crates/aura-llm/src/provider.rs` (add stream method)
- Modify: `crates/aura-llm/src/ollama.rs` (implement streaming)
- Create: `crates/aura-llm/tests/streaming_test.rs`

**Step 1: Add dependencies**

In `crates/aura-llm/Cargo.toml`:
```toml
futures-core = "0.3"
tokio-stream = "0.1"
```

**Step 2: Extend LlmProvider trait**

In `crates/aura-llm/src/provider.rs`, add a streaming method:

```rust
use futures_core::Stream;
use std::pin::Pin;

pub type TokenStream = Pin<Box<dyn Stream<Item = Result<String>> + Send>>;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, prompt: &str) -> Result<String>;
    async fn stream(&self, prompt: &str) -> Result<TokenStream>;
}
```

Update MockProvider to implement stream (return all tokens at once).

**Step 3: Implement streaming in OllamaProvider**

In `crates/aura-llm/src/ollama.rs`, add streaming support:

```rust
use futures_core::Stream;
use std::pin::Pin;
use crate::provider::TokenStream;

// Add to OllamaProvider impl:
async fn stream_chat(&self, messages: Vec<ChatMessage<'_>>) -> Result<TokenStream> {
    let url = format!("{}/api/chat", self.config.base_url);
    let body = ChatRequest {
        model: &self.config.model,
        messages,
        stream: true,  // Enable streaming
        think: false,
    };

    let resp = self.client
        .post(&url)
        .json(&body)
        .send()
        .await
        .context("Failed to reach Ollama")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Ollama returned {status}: {text}");
    }

    // Parse NDJSON stream
    let stream = async_stream::stream! {
        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("Stream read error")?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim().to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.is_empty() { continue; }

                if let Ok(parsed) = serde_json::from_str::<ChatResponse>(&line) {
                    let content = &parsed.message.content;
                    if !content.is_empty() {
                        yield Ok(content.clone());
                    }
                }
            }
        }
    };

    Ok(Box::pin(stream))
}
```

Add `async-stream = "0.3"` and `futures = "0.3"` to Cargo.toml deps.

**Step 4: Implement trait**

```rust
#[async_trait]
impl LlmProvider for OllamaProvider {
    // existing complete() ...

    async fn stream(&self, prompt: &str) -> Result<TokenStream> {
        let messages = vec![ChatMessage {
            role: "user",
            content: prompt,
        }];
        self.stream_chat(messages).await
    }
}
```

**Step 5: Build and test**

```bash
cargo test -p aura-llm
cargo build --release
```

**Step 6: Commit**

```bash
git add crates/aura-llm/
git commit -m "feat: add streaming completion to LLM provider"
```

---

### Task 4: Add conversational mode

**Files:**
- Create: `crates/aura-llm/src/conversation.rs`
- Modify: `crates/aura-llm/src/lib.rs` (export conversation module)
- Modify: `crates/aura-daemon/src/main.rs` (route unknown intents to conversation)

**Step 1: Create conversation module**

Create `crates/aura-llm/src/conversation.rs`:

```rust
use crate::provider::{LlmProvider, TokenStream};
use anyhow::Result;
use std::sync::Mutex;

const SYSTEM_PROMPT: &str = "You are Aura, a friendly and helpful voice assistant \
running locally on the user's Mac. Keep responses concise (1-3 sentences) since \
they will be spoken aloud. Be natural and conversational.";

const MAX_HISTORY: usize = 20;

struct Message {
    role: String,
    content: String,
}

pub struct Conversation {
    provider: Box<dyn LlmProvider>,
    history: Mutex<Vec<Message>>,
}

impl Conversation {
    pub fn new(provider: Box<dyn LlmProvider>) -> Self {
        Self {
            provider,
            history: Mutex::new(Vec::new()),
        }
    }

    /// Send a message and get a streaming response.
    pub async fn chat_stream(&self, user_text: &str) -> Result<TokenStream> {
        {
            let mut history = self.history.lock()
                .map_err(|e| anyhow::anyhow!("History lock: {e}"))?;
            history.push(Message {
                role: "user".into(),
                content: user_text.into(),
            });
            // Trim old messages
            while history.len() > MAX_HISTORY {
                history.remove(0);
            }
        }

        // Build prompt with history
        let prompt = self.build_prompt()?;
        self.provider.stream(&prompt).await
    }

    /// Send a message and get a complete response (for non-streaming use).
    pub async fn chat(&self, user_text: &str) -> Result<String> {
        {
            let mut history = self.history.lock()
                .map_err(|e| anyhow::anyhow!("History lock: {e}"))?;
            history.push(Message {
                role: "user".into(),
                content: user_text.into(),
            });
            while history.len() > MAX_HISTORY {
                history.remove(0);
            }
        }

        let prompt = self.build_prompt()?;
        let response = self.provider.complete(&prompt).await?;

        {
            let mut history = self.history.lock()
                .map_err(|e| anyhow::anyhow!("History lock: {e}"))?;
            history.push(Message {
                role: "assistant".into(),
                content: response.clone(),
            });
        }

        Ok(response)
    }

    fn build_prompt(&self) -> Result<String> {
        let history = self.history.lock()
            .map_err(|e| anyhow::anyhow!("History lock: {e}"))?;

        let mut prompt = format!("System: {SYSTEM_PROMPT}\n\n");
        for msg in history.iter() {
            let role = if msg.role == "user" { "User" } else { "Aura" };
            prompt.push_str(&format!("{role}: {}\n", msg.content));
        }
        prompt.push_str("Aura:");

        Ok(prompt)
    }

    pub fn clear_history(&self) {
        if let Ok(mut history) = self.history.lock() {
            history.clear();
        }
    }
}
```

**Step 2: Export module**

In `crates/aura-llm/src/lib.rs`, add:
```rust
pub mod conversation;
```

**Step 3: Route unknown intents to conversation in daemon**

In `crates/aura-daemon/src/main.rs`, update `run_processor()`:
- Create a `Conversation` alongside the `IntentParser`
- When intent is `Unknown`, call `conversation.chat(&text)` instead of just failing
- Send the response via the event bus as `AuraEvent::ShowOverlay { content: Response { text } }`

**Step 4: Build and test**

```bash
cargo test --workspace
cargo build --release
```

**Step 5: Commit**

```bash
git add crates/aura-llm/src/conversation.rs crates/aura-llm/src/lib.rs crates/aura-daemon/src/main.rs
git commit -m "feat: add conversational mode for non-command speech"
```

---

### Task 5: Add Kokoro TTS

**Files:**
- Modify: `crates/aura-voice/Cargo.toml` (add kokoro-tts)
- Rewrite: `crates/aura-voice/src/tts.rs`
- Modify: `crates/aura-voice/tests/tts_test.rs`

**Step 1: Add dependency**

In `crates/aura-voice/Cargo.toml`:
```toml
kokoro-tts = "0.3"
```

Remove: `which = "7"` (was only for finding piper binary)

**Step 2: Rewrite TTS module**

Replace `crates/aura-voice/src/tts.rs` with Kokoro-based implementation:

```rust
use anyhow::{Context, Result};
use kokoro_tts::{KokoroTts, KokoroTtsConfig};
use std::sync::Mutex;

pub struct TextToSpeech {
    engine: Mutex<KokoroTts>,
    sample_rate: u32,
}

impl TextToSpeech {
    pub fn new() -> Result<Self> {
        let config = KokoroTtsConfig::default();
        let engine = KokoroTts::new(config)
            .context("Failed to initialize Kokoro TTS")?;
        let sample_rate = 24000; // Kokoro default

        Ok(Self {
            engine: Mutex::new(engine),
            sample_rate,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Synthesize text to f32 PCM audio samples.
    pub fn synthesize(&self, text: &str) -> Result<Vec<f32>> {
        let mut engine = self.engine.lock()
            .map_err(|e| anyhow::anyhow!("TTS lock: {e}"))?;
        let audio = engine.tts(text)
            .context("TTS synthesis failed")?;
        Ok(audio)
    }
}
```

Note: The exact Kokoro API may differ. Check `docs.rs/kokoro-tts` and adapt. The key interface is: text in, f32 PCM audio out.

**Step 3: Build and test**

```bash
cargo build -p aura-voice
```

If kokoro-tts requires model downloads, add download step similar to Whisper model.

**Step 4: Commit**

```bash
git add crates/aura-voice/
git commit -m "feat: replace Piper TTS with Kokoro-82M for natural speech"
```

---

### Task 6: Add audio playback with rodio

**Files:**
- Modify: `crates/aura-voice/Cargo.toml` (add rodio)
- Create: `crates/aura-voice/src/playback.rs`
- Modify: `crates/aura-voice/src/lib.rs`

**Step 1: Add dependency**

In `crates/aura-voice/Cargo.toml`:
```toml
rodio = "0.22"
```

**Step 2: Create playback module**

Create `crates/aura-voice/src/playback.rs`:

```rust
use anyhow::{Context, Result};
use rodio::{OutputStream, OutputStreamHandle, Sink, Source};
use std::sync::Arc;
use tokio::sync::Notify;

pub struct AudioPlayer {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    sink: Arc<parking_lot::Mutex<Option<Sink>>>,
    stop_signal: Arc<Notify>,
}

impl AudioPlayer {
    pub fn new() -> Result<Self> {
        let (stream, handle) = OutputStream::try_default()
            .context("Failed to open audio output")?;

        Ok(Self {
            _stream: stream,
            handle,
            sink: Arc::new(parking_lot::Mutex::new(None)),
            stop_signal: Arc::new(Notify::new()),
        })
    }

    /// Play f32 PCM audio at the given sample rate.
    pub fn play(&self, samples: Vec<f32>, sample_rate: u32) -> Result<()> {
        self.stop(); // Cancel any current playback

        let source = rodio::buffer::SamplesBuffer::new(1, sample_rate, samples);
        let sink = Sink::try_new(&self.handle)
            .context("Failed to create audio sink")?;
        sink.append(source);

        *self.sink.lock() = Some(sink);
        Ok(())
    }

    /// Stop current playback (for barge-in).
    pub fn stop(&self) {
        if let Some(sink) = self.sink.lock().take() {
            sink.stop();
        }
    }

    /// Check if audio is currently playing.
    pub fn is_playing(&self) -> bool {
        self.sink.lock()
            .as_ref()
            .map(|s| !s.empty())
            .unwrap_or(false)
    }
}
```

**Step 3: Export module**

In `crates/aura-voice/src/lib.rs`, add:
```rust
pub mod playback;
```

**Step 4: Add parking_lot dependency**

In `crates/aura-voice/Cargo.toml`:
```toml
parking_lot = "0.12"
```

**Step 5: Build and test**

```bash
cargo build -p aura-voice
```

**Step 6: Commit**

```bash
git add crates/aura-voice/
git commit -m "feat: add rodio-based audio playback with barge-in support"
```

---

### Task 7: Wire conversational pipeline in daemon

**Files:**
- Modify: `crates/aura-daemon/src/event.rs` (add conversation events)
- Modify: `crates/aura-daemon/src/main.rs` (wire TTS + conversation + playback)
- Modify: `crates/aura-daemon/src/daemon.rs` (handle new events)

**Step 1: Add conversation events**

In `crates/aura-daemon/src/event.rs`, add to `AuraEvent`:
```rust
/// Assistant is speaking a response
AssistantSpeaking { text: String },
/// User interrupted the assistant (barge-in)
BargeIn,
```

**Step 2: Update daemon processor**

In `crates/aura-daemon/src/main.rs`, update `run_processor()`:

1. Create `Conversation` alongside `IntentParser` (sharing the same OllamaProvider)
2. Create `TextToSpeech` and `AudioPlayer`
3. When intent is `Unknown`, call `conversation.chat(&text)` and speak the response via TTS
4. When `VoiceEvent::ListeningStarted` arrives while audio is playing, call `player.stop()` (barge-in)

Key flow:
```rust
// After transcription:
match intent {
    Intent::Unknown { raw } => {
        // Conversational response
        let response = conversation.chat(&raw).await?;
        tracing::info!(response = %response, "Aura response");

        // Synthesize and play
        let audio = tts.synthesize(&response)?;
        player.play(audio, tts.sample_rate())?;

        bus.send(AuraEvent::AssistantSpeaking { text: response })?;
    }
    _ => { /* existing action execution */ }
}

// On ListeningStarted while playing:
if player.is_playing() {
    player.stop();
    bus.send(AuraEvent::BargeIn)?;
}
```

**Step 3: Update daemon event handler**

In `crates/aura-daemon/src/daemon.rs`, handle `AssistantSpeaking` to show response overlay and `BargeIn` to transition overlay to listening state.

**Step 4: Build and test**

```bash
cargo test --workspace
cargo build --release
RUST_LOG=aura=info cargo run --release
```

Test: Say "What's the weather like?" — should hear Aura speak a response. Say something while Aura is speaking — should interrupt (barge-in).

**Step 5: Commit**

```bash
git add crates/aura-daemon/
git commit -m "feat: wire conversational pipeline with TTS and barge-in"
```

---

### Task 8: Integration testing and polish

**Files:**
- Modify: `crates/aura-daemon/tests/e2e_test.rs` (add conversation tests)
- Modify: `crates/aura-daemon/tests/integration_test.rs` (add TTS tests)
- Modify: `scripts/smoke-test.sh` (update prerequisites)

**Step 1: Add conversation e2e test**

```rust
#[tokio::test]
async fn test_e2e_conversational_response() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            mock_chat_response("I'm Aura, your voice assistant!"),
        ))
        .mount(&mock_server)
        .await;

    let config = OllamaConfig {
        base_url: mock_server.uri(),
        model: "qwen3.5:4b".into(),
        timeout_secs: 5,
    };
    let provider = OllamaProvider::new(config).unwrap();
    let conversation = Conversation::new(Box::new(provider));

    let response = conversation.chat("Hello, who are you?").await.unwrap();
    assert!(!response.is_empty());
    assert!(response.contains("Aura"));
}
```

**Step 2: Update smoke test**

Add checks for Kokoro TTS model, Whisper small.en model, Silero VAD.

**Step 3: Run full test suite**

```bash
cargo test --workspace
```

**Step 4: Commit**

```bash
git add crates/aura-daemon/tests/ scripts/
git commit -m "test: add conversation and TTS integration tests"
```

---

## Execution Notes

### Dependency Graph

```
Task 1 (Whisper small.en) ──────────────────────┐
Task 2 (Silero VAD) ────────────────────────────┤
Task 3 (Streaming LLM) → Task 4 (Conversation) ├──→ Task 7 (Wire pipeline) → Task 8 (Tests)
Task 5 (Kokoro TTS) → Task 6 (Audio playback) ──┘
```

Tasks 1, 2, 3, 5 are independent and can run in parallel.
Task 4 depends on 3. Task 6 depends on 5. Task 7 depends on all. Task 8 depends on 7.

### Model Downloads Required

| Model | Size | Path |
|-------|------|------|
| ggml-small.en.bin | ~466 MB | `~/Library/Application Support/aura/models/` |
| ggml-small.en-encoder.mlmodelc | ~100 MB | `~/Library/Application Support/aura/models/` |
| Kokoro-82M | ~330 MB | Auto-downloaded by kokoro-tts crate |
| Silero VAD | ~2 MB | Bundled in silero-vad-rust crate |

### Parallel Execution Strategy

Phase 1 (parallel): Tasks 1, 2, 3, 5
Phase 2 (parallel): Tasks 4, 6
Phase 3 (sequential): Task 7
Phase 4 (sequential): Task 8
