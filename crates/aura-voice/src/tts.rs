use anyhow::{Context, Result};
use kokoro_tts::{KokoroTts, Voice};
use std::path::PathBuf;
use std::sync::Arc;

const DEFAULT_MODEL_FILENAME: &str = "kokoro-v1.0.int8.onnx";
const DEFAULT_VOICES_FILENAME: &str = "voices.bin";
const APP_DATA_DIR: &str = "aura";
const MODELS_SUBDIR: &str = "models";
const KOKORO_SAMPLE_RATE: u32 = 24000;

#[derive(Debug, Clone)]
pub struct TtsConfig {
    pub model_path: PathBuf,
    pub voices_path: PathBuf,
}

impl Default for TtsConfig {
    fn default() -> Self {
        let data_dir = dirs::data_local_dir().unwrap_or_else(|| {
            tracing::warn!("Could not determine local data directory, falling back to '.'");
            PathBuf::from(".")
        });
        let models_dir = data_dir.join(APP_DATA_DIR).join(MODELS_SUBDIR);

        Self {
            model_path: models_dir.join(DEFAULT_MODEL_FILENAME),
            voices_path: models_dir.join(DEFAULT_VOICES_FILENAME),
        }
    }
}

/// Synthesizes speech from text using the Kokoro TTS engine (82M param neural model).
///
/// Outputs 24kHz mono f32 PCM audio.
pub struct TextToSpeech {
    engine: Arc<KokoroTts>,
}

impl TextToSpeech {
    pub async fn new(config: TtsConfig) -> Result<Self> {
        let engine = KokoroTts::new(&config.model_path, &config.voices_path)
            .await
            .context("Failed to initialize Kokoro TTS — download models with: curl -L -o ~/Library/Application\\ Support/aura/models/kokoro-v1.0.int8.onnx https://github.com/mzdk100/kokoro/releases/download/V1.0/kokoro-v1.0.int8.onnx")?;

        tracing::info!("Kokoro TTS engine initialized");

        Ok(Self {
            engine: Arc::new(engine),
        })
    }

    pub fn sample_rate(&self) -> u32 {
        KOKORO_SAMPLE_RATE
    }

    /// Synthesize text to f32 PCM audio samples at 24kHz.
    pub async fn synthesize(&self, text: &str) -> Result<Vec<f32>> {
        let (samples, took) = self
            .engine
            .synth(text, Voice::AfHeart(1.0))
            .await
            .context("TTS synthesis failed")?;

        tracing::debug!(
            samples = samples.len(),
            duration_ms = took.as_millis(),
            "TTS synthesis complete"
        );

        Ok(samples)
    }
}
