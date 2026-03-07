use aura_voice::vad::{VadConfig, VadState, VoiceActivityDetector};

#[test]
fn test_silence_stays_silent() {
    let mut vad = VoiceActivityDetector::new(VadConfig::default());
    let silence = vec![0.0f32; 160];
    assert_eq!(vad.process(&silence), VadState::Silent);
}

#[test]
fn test_loud_audio_triggers_speaking() {
    let mut vad = VoiceActivityDetector::new(VadConfig::default());
    let loud: Vec<f32> = (0..160).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
    assert_eq!(vad.process(&loud), VadState::Speaking);
}

#[test]
fn test_silence_after_speaking_needs_holdoff() {
    let config = VadConfig {
        energy_threshold: 0.02,
        silence_frames_required: 3,
    };
    let mut vad = VoiceActivityDetector::new(config);
    let loud: Vec<f32> = (0..160).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
    let silence = vec![0.0f32; 160];

    // Start speaking
    vad.process(&loud);
    assert_eq!(vad.state(), VadState::Speaking);

    // 1 silent frame -- not enough
    vad.process(&silence);
    assert_eq!(vad.state(), VadState::Speaking);

    // 2 silent frames -- still not enough
    vad.process(&silence);
    assert_eq!(vad.state(), VadState::Speaking);

    // 3 silent frames -- now should be silent
    vad.process(&silence);
    assert_eq!(vad.state(), VadState::Silent);
}

#[test]
fn test_reset() {
    let mut vad = VoiceActivityDetector::new(VadConfig::default());
    let loud: Vec<f32> = (0..160).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
    vad.process(&loud);
    assert_eq!(vad.state(), VadState::Speaking);

    vad.reset();
    assert_eq!(vad.state(), VadState::Silent);
}

#[test]
fn test_empty_samples() {
    let mut vad = VoiceActivityDetector::new(VadConfig::default());
    assert_eq!(vad.process(&[]), VadState::Silent);
}
