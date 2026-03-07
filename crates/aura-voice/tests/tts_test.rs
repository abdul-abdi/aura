use aura_voice::tts::{TextToSpeech, TtsConfig};

#[test]
fn test_tts_config_defaults() {
    let config = TtsConfig::default();
    assert_eq!(config.sample_rate, 22050);
    assert!(!config.model_path.as_os_str().is_empty());
}

#[test]
#[ignore] // Requires Piper model
fn test_tts_synthesize() {
    let config = TtsConfig::default();
    let tts = TextToSpeech::new(config).unwrap();
    let audio = tts.synthesize("Hello, I'm Aura.").unwrap();
    assert!(!audio.is_empty());
}
