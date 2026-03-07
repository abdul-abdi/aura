use anyhow::{Context, Result};
use std::path::PathBuf;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

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
                .join("aura")
                .join("models")
                .join("ggml-base.en.bin"),
            language: "en".into(),
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
            if let Ok(segment) = state.full_get_segment_text(i) {
                text.push_str(&segment);
            }
        }

        Ok(text.trim().to_string())
    }
}
