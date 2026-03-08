use anyhow::Result;
use std::path::PathBuf;

const WHISPER_MODEL: &str = "ggml-small.en.bin";
const LLM_MODEL: &str = "intent-model.gguf";
const KOKORO_MODEL: &str = "kokoro-v1.0.int8.onnx";
const KOKORO_VOICES: &str = "voices.bin";
const WAKEWORD_MODEL: &str = "hey-aura.rpw";

const REQUIRED_DIRS: &[&str] = &["models", "bin", "config", "logs"];

pub struct SetupStatus {
    pub whisper_model_ready: bool,
    pub llm_model_ready: bool,
    pub tts_ready: bool,
    pub wakeword_model_ready: bool,
}

impl SetupStatus {
    /// Returns true when core components are ready.
    /// Wake word is excluded — the daemon can start without it (uses push-to-talk fallback).
    pub fn is_ready(&self) -> bool {
        self.whisper_model_ready && self.llm_model_ready && self.tts_ready
    }

    pub fn missing_components(&self) -> Vec<&str> {
        let mut missing = Vec::new();
        if !self.whisper_model_ready {
            missing.push("Whisper STT model (ggml-small.en.bin)");
        }
        if !self.llm_model_ready {
            missing.push("LLM model (intent-model.gguf)");
        }
        if !self.tts_ready {
            missing.push("Kokoro TTS (kokoro-v1.0.int8.onnx + voices.bin)");
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
            .unwrap_or_else(|| {
                tracing::warn!("Could not determine local data directory, falling back to '.'");
                PathBuf::from(".")
            })
            .join("aura")
    }

    pub fn check(&self) -> SetupStatus {
        let models_dir = self.data_dir.join("models");

        SetupStatus {
            whisper_model_ready: models_dir.join(WHISPER_MODEL).exists(),
            llm_model_ready: models_dir.join(LLM_MODEL).exists(),
            tts_ready: models_dir.join(KOKORO_MODEL).exists()
                && models_dir.join(KOKORO_VOICES).exists(),
            wakeword_model_ready: models_dir.join(WAKEWORD_MODEL).exists(),
        }
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        // Create the data dir itself with restricted permissions
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::set_permissions(&self.data_dir, std::fs::Permissions::from_mode(0o700))?;

        for dir in REQUIRED_DIRS {
            let path = self.data_dir.join(dir);
            std::fs::create_dir_all(&path)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
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
