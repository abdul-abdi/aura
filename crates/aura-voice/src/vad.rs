/// Neural Voice Activity Detection using the Silero VAD model.
///
/// Wraps `voice_activity_detector` crate to provide speech/silence
/// state transitions with configurable silence holdoff.
use anyhow::{Context, Result};
use voice_activity_detector::VoiceActivityDetector as SileroVad;

const DEFAULT_SPEECH_THRESHOLD: f32 = 0.5;
const DEFAULT_SILENCE_FRAMES: usize = 40; // ~1.3s at 32ms/frame
const SAMPLE_RATE: i64 = 16000;
const CHUNK_SIZE: usize = 512; // 32ms at 16kHz

#[derive(Debug, Clone)]
pub struct VadConfig {
    pub speech_threshold: f32,
    pub silence_frames_required: usize,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            speech_threshold: DEFAULT_SPEECH_THRESHOLD,
            silence_frames_required: DEFAULT_SILENCE_FRAMES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadState {
    Silent,
    Speaking,
}

pub struct VoiceActivityDetector {
    vad: SileroVad,
    config: VadConfig,
    state: VadState,
    silence_counter: usize,
    buffer: Vec<f32>,
}

impl VoiceActivityDetector {
    pub fn new(config: VadConfig) -> Result<Self> {
        let vad = SileroVad::builder()
            .sample_rate(SAMPLE_RATE)
            .chunk_size(CHUNK_SIZE)
            .build()
            .context("Failed to initialize Silero VAD model")?;

        Ok(Self {
            vad,
            config,
            state: VadState::Silent,
            silence_counter: 0,
            buffer: Vec::with_capacity(CHUNK_SIZE),
        })
    }

    pub fn state(&self) -> VadState {
        self.state
    }

    /// Process a chunk of audio samples. Returns the new state.
    /// Internally buffers to CHUNK_SIZE (512 samples = 32ms at 16kHz).
    pub fn process(&mut self, samples: &[f32]) -> VadState {
        self.buffer.extend_from_slice(samples);

        while self.buffer.len() >= CHUNK_SIZE {
            let chunk: Vec<f32> = self.buffer.drain(..CHUNK_SIZE).collect();
            let probability = self.vad.predict(chunk);

            if probability >= self.config.speech_threshold {
                self.silence_counter = 0;
                if self.state == VadState::Silent {
                    tracing::debug!(probability, "VAD: speech start (Silero)");
                }
                self.state = VadState::Speaking;
            } else if self.state == VadState::Speaking {
                self.silence_counter += 1;
                if self.silence_counter >= self.config.silence_frames_required {
                    tracing::debug!(probability, frames = self.silence_counter, "VAD: speech end (Silero)");
                    self.state = VadState::Silent;
                    self.silence_counter = 0;
                }
            }
        }

        self.state
    }

    /// Reset to silent state.
    pub fn reset(&mut self) {
        self.state = VadState::Silent;
        self.silence_counter = 0;
        self.buffer.clear();
    }
}
