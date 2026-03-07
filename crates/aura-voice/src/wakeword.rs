use anyhow::{anyhow, Result};
use rustpotter::{Rustpotter, RustpotterConfig, WakewordRef};

use crate::audio::{CHANNELS, SAMPLE_RATE};

const WAKEWORD_NAME: &str = "hey_aura";

pub struct WakeWordConfig {
    pub threshold: f32,
    pub avg_threshold: f32,
}

pub struct WakeWordDetector {
    rustpotter: Rustpotter,
    buffer: Vec<f32>,
}

impl WakeWordDetector {
    pub fn new(config: WakeWordConfig) -> Result<Self> {
        let mut rp_config = RustpotterConfig::default();
        rp_config.detector.threshold = config.threshold;
        rp_config.detector.avg_threshold = config.avg_threshold;
        rp_config.fmt.sample_rate = SAMPLE_RATE as usize;
        rp_config.fmt.channels = CHANNELS;

        let rustpotter = Rustpotter::new(&rp_config).map_err(|e| anyhow!(e))?;

        Ok(Self {
            rustpotter,
            buffer: Vec::new(),
        })
    }

    pub fn add_wakeword_from_file(&mut self, path: &str) -> Result<()> {
        self.rustpotter
            .add_wakeword_from_file(WAKEWORD_NAME, path)
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }

    pub fn add_wakeword_from_ref(&mut self, wakeword: WakewordRef) -> Result<()> {
        self.rustpotter
            .add_wakeword_ref(WAKEWORD_NAME, wakeword)
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }

    pub fn process(&mut self, audio: &[f32]) -> bool {
        let frame_size = self.rustpotter.get_samples_per_frame();
        if frame_size == 0 {
            return false;
        }

        self.buffer.extend_from_slice(audio);

        while self.buffer.len() >= frame_size {
            let frame: Vec<f32> = self.buffer.drain(..frame_size).collect();
            if self.rustpotter.process_samples(frame).is_some() {
                return true;
            }
        }

        false
    }
}
