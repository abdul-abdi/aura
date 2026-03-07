use aura_daemon::setup::AuraSetup;
use tempfile::TempDir;

#[test]
fn test_setup_detects_missing_models() {
    let tmp = TempDir::new().unwrap();
    let setup = AuraSetup::new(tmp.path().to_path_buf());
    let status = setup.check();

    assert!(!status.whisper_model_ready);
    assert!(!status.llm_model_ready);
    assert!(!status.piper_ready);
}

#[test]
fn test_setup_creates_directories() {
    let tmp = TempDir::new().unwrap();
    let setup = AuraSetup::new(tmp.path().to_path_buf());
    setup.ensure_dirs().unwrap();

    assert!(tmp.path().join("models").exists());
    assert!(tmp.path().join("bin").exists());
    assert!(tmp.path().join("config").exists());
}
