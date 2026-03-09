use anyhow::Result;
use std::path::PathBuf;

const WAKEWORD_MODEL: &str = "hey-aura.rpw";

const REQUIRED_DIRS: &[&str] = &["models", "bin", "config", "logs"];

pub struct SetupStatus {
    pub wakeword_model_ready: bool,
}

impl SetupStatus {
    /// Returns true when core components are ready.
    /// With Gemini Live API, no local models are needed for STT/LLM/TTS.
    /// Wake word is optional — the daemon can start without it (uses always-on mic).
    pub fn is_ready(&self) -> bool {
        true
    }

    pub fn missing_components(&self) -> Vec<&str> {
        let mut missing = Vec::new();
        if !self.wakeword_model_ready {
            missing.push("Wake word model (hey-aura.rpw) — optional");
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
        let missing = status.missing_components();
        if missing.is_empty() {
            tracing::info!("All components ready");
        } else {
            tracing::info!("Optional components not found:");
            for component in missing {
                tracing::info!("  - {component}");
            }
        }
    }
}
