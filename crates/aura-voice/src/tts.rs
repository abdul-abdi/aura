use anyhow::{Context, Result};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

const DEFAULT_MODEL_FILENAME: &str = "piper-en-us.onnx";
const DEFAULT_SAMPLE_RATE: u32 = 22050;
const PIPER_BINARY_NAME: &str = "piper";
const APP_DATA_DIR: &str = "aura";
const MODELS_SUBDIR: &str = "models";
const BIN_SUBDIR: &str = "bin";

#[derive(Debug, Clone)]
pub struct TtsConfig {
    pub model_path: PathBuf,
    /// Sample rate of the output audio (used by downstream audio pipeline).
    pub sample_rate: u32,
    pub speaker_id: Option<u32>,
}

impl Default for TtsConfig {
    fn default() -> Self {
        let data_dir = dirs::data_local_dir().unwrap_or_else(|| {
            tracing::warn!("Could not determine local data directory, falling back to '.'");
            PathBuf::from(".")
        });

        Self {
            model_path: data_dir
                .join(APP_DATA_DIR)
                .join(MODELS_SUBDIR)
                .join(DEFAULT_MODEL_FILENAME),
            sample_rate: DEFAULT_SAMPLE_RATE,
            speaker_id: None,
        }
    }
}

/// Synthesizes speech from text using the Piper TTS engine.
///
/// Note: `synthesize()` is synchronous and blocks on the subprocess.
/// Callers in async contexts should use `tokio::task::spawn_blocking`.
pub struct TextToSpeech {
    config: TtsConfig,
    piper_path: PathBuf,
}

impl TextToSpeech {
    pub fn new(config: TtsConfig) -> Result<Self> {
        let piper_path = which::which(PIPER_BINARY_NAME)
            .or_else(|_| {
                let local = dirs::data_local_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(APP_DATA_DIR)
                    .join(BIN_SUBDIR)
                    .join(PIPER_BINARY_NAME);
                if local.exists() {
                    Ok(local)
                } else {
                    Err(which::Error::CannotFindBinaryPath)
                }
            })
            .context(
                "Piper binary not found. Install piper or place it in ~/.local/share/aura/bin/",
            )?;

        Ok(Self {
            config,
            piper_path,
        })
    }

    pub fn synthesize(&self, text: &str) -> Result<Vec<u8>> {
        let mut cmd = Command::new(&self.piper_path);
        cmd.arg("--model")
            .arg(&self.config.model_path)
            .arg("--output-raw")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());

        if let Some(speaker) = self.config.speaker_id {
            cmd.arg("--speaker").arg(speaker.to_string());
        }

        let mut child = cmd.spawn().context("Failed to start piper")?;

        let mut stdin = child.stdin.take().context("Failed to open piper stdin")?;
        stdin.write_all(text.as_bytes())?;
        drop(stdin);

        let output = child.wait_with_output()?;

        if !output.status.success() {
            anyhow::bail!(
                "Piper failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(output.stdout)
    }
}
