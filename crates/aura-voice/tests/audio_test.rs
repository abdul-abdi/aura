use aura_voice::audio::AudioCapture;

#[test]
fn test_audio_capture_lists_devices() {
    let devices = AudioCapture::list_input_devices();
    assert!(devices.is_ok(), "Should be able to list audio devices");
    // CI might not have audio devices, so we just check it doesn't panic
}

#[test]
fn test_audio_capture_creates_with_default() {
    let capture = AudioCapture::new(None);
    // May fail in CI without audio hardware, but shouldn't panic
    if let Ok(cap) = capture {
        assert_eq!(cap.sample_rate(), 16000);
    }
}
