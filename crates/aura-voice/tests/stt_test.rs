use aura_voice::stt::{SpeechToText, SttConfig};

#[test]
fn test_stt_config_defaults() {
    let config = SttConfig::default();
    assert_eq!(config.language, "en");
    assert!(!config.translate);
}

// Integration test — requires model file, skip in CI
#[test]
#[ignore]
fn test_stt_transcribe_silence() {
    let config = SttConfig::default();
    let stt = SpeechToText::new(config).unwrap();

    let silence = vec![0.0f32; 16000 * 3]; // 3 seconds silence
    let result = stt.transcribe(&silence).unwrap();
    assert!(result.trim().is_empty() || result.len() < 20);
}
