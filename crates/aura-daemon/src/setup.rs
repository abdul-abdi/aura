use anyhow::Result;
use std::path::PathBuf;

const WHISPER_MODEL: &str = "ggml-base.en.bin";
const LLM_MODEL: &str = "intent-model.gguf";
const PIPER_BINARY: &str = "piper";
const WAKEWORD_MODEL: &str = "hey-aura.rpw";

const REQUIRED_DIRS: &[&str] = &["models", "bin", "config", "logs"];

pub struct SetupStatus {
    pub whisper_model_ready: bool,
    pub llm_model_ready: bool,
    pub piper_ready: bool,
    pub wakeword_model_ready: bool,
}

impl SetupStatus {
    pub fn is_ready(&self) -> bool {
        self.whisper_model_ready && self.llm_model_ready && self.piper_ready
    }

    pub fn missing_components(&self) -> Vec<&str> {
        let mut missing = Vec::new();
        if !self.whisper_model_ready {
            missing.push("Whisper STT model (ggml-base.en.bin)");
        }
        if !self.llm_model_ready {
            missing.push("LLM model (intent-model.gguf)");
        }
        if !self.piper_ready {
            missing.push("Piper TTS (piper binary + voice model)");
        }
        if !self.wakeword_model_ready {
            missing.push("Wake word model (hey-aura.rpw)");
        }
        missing
    }
}

pub struct AuraSetup {
    data_dir: PathBuf,
}

impl AuraSetup {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    pub fn default_data_dir() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("aura")
    }

    pub fn check(&self) -> SetupStatus {
        let models_dir = self.data_dir.join("models");
        let bin_dir = self.data_dir.join("bin");

        SetupStatus {
            whisper_model_ready: models_dir.join(WHISPER_MODEL).exists(),
            llm_model_ready: models_dir.join(LLM_MODEL).exists(),
            piper_ready: bin_dir.join(PIPER_BINARY).exists()
                || which::which(PIPER_BINARY).is_ok(),
            wakeword_model_ready: models_dir.join(WAKEWORD_MODEL).exists(),
        }
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in REQUIRED_DIRS {
            std::fs::create_dir_all(self.data_dir.join(dir))?;
        }
        Ok(())
    }

    pub fn print_status(&self) {
        let status = self.check();
        if status.is_ready() {
            tracing::info!("All components ready");
        } else {
            tracing::warn!("Missing components:");
            for component in status.missing_components() {
                tracing::warn!("  - {component}");
            }
        }
    }
}
