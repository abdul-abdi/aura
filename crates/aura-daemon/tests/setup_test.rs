use aura_daemon::setup::AuraSetup;
use tempfile::TempDir;

#[test]
fn test_setup_detects_missing_models() {
    let tmp = TempDir::new().unwrap();
    let setup = AuraSetup::new(tmp.path().to_path_buf());
    let status = setup.check();

    assert!(!status.whisper_model_ready);
    assert!(!status.llm_model_ready);
    assert!(!status.wakeword_model_ready);
}

#[test]
fn test_setup_creates_directories() {
    let tmp = TempDir::new().unwrap();
    let setup = AuraSetup::new(tmp.path().to_path_buf());
    setup.ensure_dirs().unwrap();

    assert!(tmp.path().join("models").exists());
    assert!(tmp.path().join("bin").exists());
    assert!(tmp.path().join("config").exists());
    assert!(tmp.path().join("logs").exists());
}

#[test]
fn test_missing_components_lists_all() {
    let tmp = TempDir::new().unwrap();
    let setup = AuraSetup::new(tmp.path().to_path_buf());
    let status = setup.check();
    let missing = status.missing_components();

    assert!(missing.len() >= 3);
    assert!(!status.is_ready());
}

#[test]
fn test_is_ready_with_models_present() {
    let tmp = TempDir::new().unwrap();
    let setup = AuraSetup::new(tmp.path().to_path_buf());
    setup.ensure_dirs().unwrap();

    // Create fake model files
    std::fs::write(tmp.path().join("models/ggml-small.en.bin"), b"fake").unwrap();
    std::fs::write(tmp.path().join("models/intent-model.gguf"), b"fake").unwrap();
    std::fs::write(tmp.path().join("models/kokoro-v1.0.int8.onnx"), b"fake").unwrap();
    std::fs::write(tmp.path().join("models/voices.bin"), b"fake").unwrap();

    let status = setup.check();
    assert!(status.whisper_model_ready);
    assert!(status.llm_model_ready);
    assert!(status.tts_ready);
    assert!(status.is_ready());
}
