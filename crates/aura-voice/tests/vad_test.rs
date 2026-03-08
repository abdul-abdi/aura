use aura_voice::vad::{VadConfig, VadState, VoiceActivityDetector};

#[test]
fn test_silence_stays_silent() {
    let mut vad = VoiceActivityDetector::new(VadConfig::default()).unwrap();
    let silence = vec![0.0f32; 1600]; // 100ms of silence
    assert_eq!(vad.process(&silence), VadState::Silent);
}

#[test]
fn test_loud_audio_does_not_crash() {
    let mut vad = VoiceActivityDetector::new(VadConfig::default()).unwrap();
    // Generate a loud sine wave (speech-like)
    let loud: Vec<f32> = (0..1600)
        .map(|i| (i as f32 * 0.1).sin() * 0.8)
        .collect();
    let state = vad.process(&loud);
    // Silero may or may not trigger on synthetic sine — test that it doesn't crash
    assert!(state == VadState::Silent || state == VadState::Speaking);
}

#[test]
fn test_reset() {
    let mut vad = VoiceActivityDetector::new(VadConfig::default()).unwrap();
    vad.reset();
    assert_eq!(vad.state(), VadState::Silent);
}

#[test]
fn test_empty_samples() {
    let mut vad = VoiceActivityDetector::new(VadConfig::default()).unwrap();
    // Empty input should not crash and state stays silent
    assert_eq!(vad.process(&[]), VadState::Silent);
}

#[test]
fn test_custom_config() {
    let config = VadConfig {
        speech_threshold: 0.3,
        silence_frames_required: 10,
    };
    let vad = VoiceActivityDetector::new(config).unwrap();
    assert_eq!(vad.state(), VadState::Silent);
}
