//! Voice Activity Detection wrapper using Google's WebRTC VAD.
//!
//! Provides speech detection to replace energy-threshold gating.
//! Operates on 16kHz mono PCM audio in 30ms frames (480 samples).

use webrtc_vad::{SampleRate, Vad, VadMode};

/// Frame size for 30ms at 16kHz
pub const VAD_FRAME_SIZE: usize = 480;

const PRE_ROLL_FRAMES: usize = 5; // 150ms pre-roll buffer
const HANGOVER_FRAMES: usize = 15; // 450ms hangover after speech

pub struct VoiceDetector {
    vad: Vad,
    pre_roll: Vec<Vec<i16>>,
    silence_frames: usize,
    in_speech: bool,
}

impl VoiceDetector {
    pub fn new() -> Result<Self, String> {
        let vad = Vad::new_with_rate_and_mode(SampleRate::Rate16kHz, VadMode::VeryAggressive);
        Ok(Self {
            vad,
            pre_roll: Vec::with_capacity(PRE_ROLL_FRAMES),
            silence_frames: 0,
            in_speech: false,
        })
    }

    /// Process a 30ms frame. Returns true if frame should be forwarded (speech or hangover).
    pub fn is_speech(&mut self, frame: &[i16]) -> bool {
        if frame.len() != VAD_FRAME_SIZE {
            return false;
        }
        let speech = self.vad.is_voice_segment(frame).unwrap_or(false);
        if speech {
            self.silence_frames = 0;
            self.in_speech = true;
            true
        } else if self.in_speech {
            self.silence_frames += 1;
            if self.silence_frames >= HANGOVER_FRAMES {
                self.in_speech = false;
                false
            } else {
                true
            }
        } else {
            let frame_i16 = frame.to_vec();
            if self.pre_roll.len() >= PRE_ROLL_FRAMES {
                self.pre_roll.remove(0);
            }
            self.pre_roll.push(frame_i16);
            false
        }
    }

    pub fn drain_pre_roll(&mut self) -> Vec<Vec<i16>> {
        std::mem::take(&mut self.pre_roll)
    }

    pub fn is_in_speech(&self) -> bool {
        self.in_speech
    }

    pub fn reset(&mut self) {
        self.pre_roll.clear();
        self.silence_frames = 0;
        self.in_speech = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silence_frame() -> Vec<i16> {
        vec![0i16; VAD_FRAME_SIZE]
    }

    #[test]
    fn new_detector_not_in_speech() {
        let detector = VoiceDetector::new().expect("VoiceDetector::new failed");
        assert!(!detector.is_in_speech());
    }

    #[test]
    fn silence_returns_false() {
        let mut detector = VoiceDetector::new().expect("VoiceDetector::new failed");
        let frame = silence_frame();
        assert!(!detector.is_speech(&frame));
    }

    #[test]
    fn wrong_frame_size_returns_false() {
        let mut detector = VoiceDetector::new().expect("VoiceDetector::new failed");
        let short_frame = vec![0i16; 100];
        assert!(!detector.is_speech(&short_frame));
        let long_frame = vec![0i16; 960];
        assert!(!detector.is_speech(&long_frame));
    }

    #[test]
    fn pre_roll_buffers_frames() {
        let mut detector = VoiceDetector::new().expect("VoiceDetector::new failed");
        let frame = silence_frame();
        // Feed 3 silence frames — all should return false and buffer into pre-roll
        for _ in 0..3 {
            assert!(!detector.is_speech(&frame));
        }
        let pre_roll = detector.drain_pre_roll();
        assert_eq!(pre_roll.len(), 3);
    }

    #[test]
    fn pre_roll_caps_at_max() {
        let mut detector = VoiceDetector::new().expect("VoiceDetector::new failed");
        let frame = silence_frame();
        // Feed more frames than PRE_ROLL_FRAMES
        for _ in 0..(PRE_ROLL_FRAMES + 3) {
            assert!(!detector.is_speech(&frame));
        }
        let pre_roll = detector.drain_pre_roll();
        assert_eq!(pre_roll.len(), PRE_ROLL_FRAMES);
    }

    #[test]
    fn reset_clears_state() {
        let mut detector = VoiceDetector::new().expect("VoiceDetector::new failed");
        let frame = silence_frame();
        // Buffer some pre-roll frames
        for _ in 0..PRE_ROLL_FRAMES {
            detector.is_speech(&frame);
        }
        detector.reset();
        assert!(!detector.is_in_speech());
        assert_eq!(detector.drain_pre_roll().len(), 0);
    }
}
