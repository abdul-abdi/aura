use anyhow::{Context, Result};
use std::path::PathBuf;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const DEFAULT_MODEL_FILENAME: &str = "ggml-base.en.bin";
const DEFAULT_MODEL_DIR: &str = "aura";
const DEFAULT_MODELS_SUBDIR: &str = "models";
const DEFAULT_LANGUAGE: &str = "en";

pub struct SttConfig {
    pub model_path: PathBuf,
    pub language: String,
    pub translate: bool,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            model_path: dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(DEFAULT_MODEL_DIR)
                .join(DEFAULT_MODELS_SUBDIR)
                .join(DEFAULT_MODEL_FILENAME),
            language: DEFAULT_LANGUAGE.into(),
            translate: false,
        }
    }
}

pub struct SpeechToText {
    ctx: WhisperContext,
    language: String,
    translate: bool,
}

impl SpeechToText {
    pub fn new(config: SttConfig) -> Result<Self> {
        let ctx = WhisperContext::new_with_params(
            config.model_path.to_str().context("Invalid model path")?,
            WhisperContextParameters::default(),
        )
        .context("Failed to load whisper model")?;

        Ok(Self {
            ctx,
            language: config.language,
            translate: config.translate,
        })
    }

    pub fn transcribe(&self, audio: &[f32]) -> Result<String> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(&self.language));
        params.set_translate(self.translate);
        params.set_no_timestamps(true);
        params.set_single_segment(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);

        let mut state = self.ctx.create_state()?;
        state.full(params, audio)?;

        let num_segments = state.full_n_segments()?;
        let mut text = String::new();
        for i in 0..num_segments {
            let segment = state
                .full_get_segment_text(i)
                .context(format!("Failed to get segment {i} text"))?;
            text.push_str(&segment);
        }

        Ok(text.trim().to_string())
    }
}
