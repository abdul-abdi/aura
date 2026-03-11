use aura_daemon::setup::AuraSetup;
use tempfile::TempDir;

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
    // With Gemini, is_ready() should return true — no local models needed
    let tmp = TempDir::new().unwrap();
    let setup = AuraSetup::new(tmp.path().to_path_buf());
    let status = setup.check();

    assert!(
        status.is_ready(),
        "Should be ready — no local models needed with Gemini"
    );
}

#[test]
fn test_missing_components_is_empty() {
    let tmp = TempDir::new().unwrap();
    let setup = AuraSetup::new(tmp.path().to_path_buf());
    let status = setup.check();
    let missing = status.missing_components();

    assert!(missing.is_empty(), "No components should be missing");
}
