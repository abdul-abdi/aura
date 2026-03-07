use anyhow::{anyhow, Result};
use rustpotter::{Rustpotter, RustpotterConfig, WakewordRef};

pub struct WakeWordConfig {
    pub threshold: f32,
    pub avg_threshold: f32,
}

pub struct WakeWordDetector {
    rustpotter: Rustpotter,
}

impl WakeWordDetector {
    pub fn new(config: WakeWordConfig) -> Result<Self> {
        let mut rp_config = RustpotterConfig::default();
        rp_config.detector.threshold = config.threshold;
        rp_config.detector.avg_threshold = config.avg_threshold;
        rp_config.fmt.sample_rate = 16000;
        rp_config.fmt.channels = 1;

        let rustpotter = Rustpotter::new(&rp_config).map_err(|e| anyhow!(e))?;

        Ok(Self { rustpotter })
    }

    pub fn add_wakeword_from_file(&mut self, path: &str) -> Result<()> {
        self.rustpotter
            .add_wakeword_from_file("hey_aura", path)
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }

    pub fn add_wakeword_from_ref(&mut self, wakeword: WakewordRef) -> Result<()> {
        self.rustpotter
            .add_wakeword_ref("hey_aura", wakeword)
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }

    pub fn process(&mut self, audio: &[f32]) -> bool {
        let frame_size = self.rustpotter.get_samples_per_frame();
        if frame_size == 0 {
            return false;
        }
        for chunk in audio.chunks(frame_size) {
            if chunk.len() == frame_size {
                let samples: Vec<f32> = chunk.to_vec();
                if self.rustpotter.process_samples(samples).is_some() {
                    return true;
                }
            }
        }
        false
    }
}
