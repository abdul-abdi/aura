use anyhow::Result;
use tokio::sync::mpsc;

const DEFAULT_SAMPLE_RATE: u32 = 16_000;
const DEFAULT_WAKE_THRESHOLD: f32 = 0.5;
const DEFAULT_SILENCE_TIMEOUT_MS: u64 = 2_000;
const DEFAULT_MAX_LISTEN_MS: u64 = 10_000;

#[derive(Debug, Clone)]
pub enum VoiceEvent {
    WakeWordDetected,
    ListeningStarted,
    Transcription { text: String },
    ListeningStopped,
    Error { message: String },
}

pub struct VoicePipelineConfig {
    pub sample_rate: u32,
    pub wake_threshold: f32,
    pub silence_timeout_ms: u64,
    pub max_listen_ms: u64,
}

impl Default for VoicePipelineConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            wake_threshold: DEFAULT_WAKE_THRESHOLD,
            silence_timeout_ms: DEFAULT_SILENCE_TIMEOUT_MS,
            max_listen_ms: DEFAULT_MAX_LISTEN_MS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineState {
    Idle,
    Listening,
    Processing,
}

pub struct VoicePipeline {
    config: VoicePipelineConfig,
    state: PipelineState,
    event_tx: mpsc::Sender<VoiceEvent>,
}

impl VoicePipeline {
    pub fn new(config: VoicePipelineConfig, event_tx: mpsc::Sender<VoiceEvent>) -> Self {
        Self {
            config,
            state: PipelineState::Idle,
            event_tx,
        }
    }

    pub fn state(&self) -> PipelineState {
        self.state
    }

    pub async fn on_wake_word_detected(&mut self) -> Result<()> {
        if self.state != PipelineState::Idle {
            anyhow::bail!("Cannot start listening from state: {:?}", self.state);
        }
        self.event_tx.send(VoiceEvent::WakeWordDetected).await?;
        self.event_tx.send(VoiceEvent::ListeningStarted).await?;
        self.state = PipelineState::Listening;
        Ok(())
    }

    pub async fn on_audio_captured(&mut self, _audio: &[f32]) -> Result<()> {
        if self.state != PipelineState::Listening {
            return Ok(());
        }

        self.state = PipelineState::Processing;

        // STT will be integrated here -- for now emit placeholder
        self.event_tx
            .send(VoiceEvent::Transcription {
                text: String::new(),
            })
            .await?;
        self.event_tx.send(VoiceEvent::ListeningStopped).await?;
        self.state = PipelineState::Idle;

        Ok(())
    }
}
