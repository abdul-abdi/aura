/// Energy-based Voice Activity Detection.
///
/// Computes RMS energy per audio chunk and compares against a threshold.
/// Uses a simple state machine: silence -> speaking -> silence, with
/// hold-off to avoid cutting off mid-word.
const DEFAULT_ENERGY_THRESHOLD: f32 = 0.02;
const DEFAULT_SILENCE_FRAMES: usize = 120; // ~1.3s of silence before ending speech

#[derive(Debug, Clone)]
pub struct VadConfig {
    pub energy_threshold: f32,
    pub silence_frames_required: usize,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            energy_threshold: DEFAULT_ENERGY_THRESHOLD,
            silence_frames_required: DEFAULT_SILENCE_FRAMES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadState {
    Silent,
    Speaking,
}

pub struct VoiceActivityDetector {
    config: VadConfig,
    state: VadState,
    silence_counter: usize,
}

impl VoiceActivityDetector {
    pub fn new(config: VadConfig) -> Self {
        Self {
            config,
            state: VadState::Silent,
            silence_counter: 0,
        }
    }

    pub fn state(&self) -> VadState {
        self.state
    }

    /// Process a chunk of audio samples. Returns the new state.
    pub fn process(&mut self, samples: &[f32]) -> VadState {
        let energy = rms_energy(samples);

        if energy >= self.config.energy_threshold {
            self.silence_counter = 0;
            if self.state == VadState::Silent {
                tracing::debug!(energy, "VAD: speech start");
            }
            self.state = VadState::Speaking;
        } else if self.state == VadState::Speaking {
            self.silence_counter += 1;
            if self.silence_counter >= self.config.silence_frames_required {
                tracing::debug!(energy, frames = self.silence_counter, "VAD: speech end");
                self.state = VadState::Silent;
                self.silence_counter = 0;
            }
        }

        self.state
    }

    /// Reset to silent state.
    pub fn reset(&mut self) {
        self.state = VadState::Silent;
        self.silence_counter = 0;
    }
}

fn rms_energy(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}
