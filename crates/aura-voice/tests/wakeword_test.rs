use aura_voice::wakeword::{WakeWordConfig, WakeWordDetector};

#[test]
fn test_wakeword_detector_creates() {
    let config = WakeWordConfig {
        threshold: 0.5,
        avg_threshold: 0.25,
    };
    let detector = WakeWordDetector::new(config);
    assert!(detector.is_ok());
}

#[test]
fn test_wakeword_silence_no_detection() {
    let config = WakeWordConfig {
        threshold: 0.5,
        avg_threshold: 0.25,
    };
    let mut detector = WakeWordDetector::new(config).unwrap();

    // Feed silence — should not trigger
    let silence = vec![0.0f32; 16000]; // 1 second of silence
    let detected = detector.process(&silence);
    assert!(!detected);
}
