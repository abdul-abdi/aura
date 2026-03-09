use aura_daemon::setup::AuraSetup;
use tempfile::TempDir;

#[test]
fn test_setup_detects_missing_wakeword() {
    let tmp = TempDir::new().unwrap();
    let setup = AuraSetup::new(tmp.path().to_path_buf());
    let status = setup.check();

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
fn test_is_ready_without_local_models() {
    // With Gemini, is_ready() should return true even without local models
    let tmp = TempDir::new().unwrap();
    let setup = AuraSetup::new(tmp.path().to_path_buf());
    let status = setup.check();

    assert!(
        status.is_ready(),
        "Should be ready — no local models needed with Gemini"
    );
}

#[test]
fn test_wakeword_detected_when_present() {
    let tmp = TempDir::new().unwrap();
    let setup = AuraSetup::new(tmp.path().to_path_buf());
    setup.ensure_dirs().unwrap();

    std::fs::write(tmp.path().join("models/hey-aura.rpw"), b"fake").unwrap();

    let status = setup.check();
    assert!(status.wakeword_model_ready);
}

#[test]
fn test_missing_components_lists_optional() {
    let tmp = TempDir::new().unwrap();
    let setup = AuraSetup::new(tmp.path().to_path_buf());
    let status = setup.check();
    let missing = status.missing_components();

    // Only wakeword is missing, and it's optional
    assert_eq!(missing.len(), 1);
    assert!(missing[0].contains("hey-aura.rpw"));
}
