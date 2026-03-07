use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

pub struct TtsConfig {
    pub model_path: PathBuf,
    pub sample_rate: u32,
    pub speaker_id: Option<u32>,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            model_path: dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("aura")
                .join("models")
                .join("piper-en-us.onnx"),
            sample_rate: 22050,
            speaker_id: None,
        }
    }
}

pub struct TextToSpeech {
    config: TtsConfig,
    piper_path: PathBuf,
}

impl TextToSpeech {
    pub fn new(config: TtsConfig) -> Result<Self> {
        let piper_path = which::which("piper")
            .or_else(|_| {
                let local = dirs::data_local_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("aura")
                    .join("bin")
                    .join("piper");
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

        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin.write_all(text.as_bytes())?;
        }

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
