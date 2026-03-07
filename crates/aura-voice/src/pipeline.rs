use anyhow::Result;
use tokio::sync::mpsc;

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
            sample_rate: 16000,
            wake_threshold: 0.5,
            silence_timeout_ms: 2000,
            max_listen_ms: 10000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PipelineState {
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

    pub fn state(&self) -> &str {
        match self.state {
            PipelineState::Idle => "idle",
            PipelineState::Listening => "listening",
            PipelineState::Processing => "processing",
        }
    }

    pub async fn on_wake_word_detected(&mut self) -> Result<()> {
        self.state = PipelineState::Listening;
        self.event_tx.send(VoiceEvent::WakeWordDetected).await?;
        self.event_tx.send(VoiceEvent::ListeningStarted).await?;
        Ok(())
    }

    pub async fn on_audio_captured(&mut self, _audio: Vec<f32>) -> Result<()> {
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

        self.state = PipelineState::Idle;
        self.event_tx.send(VoiceEvent::ListeningStopped).await?;

        Ok(())
    }
}
