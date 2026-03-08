use aura_voice::tts::TtsConfig;

#[test]
fn test_tts_config_defaults() {
    let config = TtsConfig::default();
    assert!(!config.model_path.as_os_str().is_empty());
    assert!(!config.voices_path.as_os_str().is_empty());
    assert!(config
        .model_path
        .to_str()
        .unwrap()
        .contains("kokoro-v1.0.int8.onnx"));
}

#[tokio::test]
#[ignore] // Requires Kokoro model files
async fn test_tts_synthesize() {
    use aura_voice::tts::TextToSpeech;

    let config = TtsConfig::default();
    let tts = TextToSpeech::new(config).await.unwrap();
    let audio = tts.synthesize("Hello, I'm Aura.").await.unwrap();
    assert!(!audio.is_empty());
}
