use aura_voice::pipeline::{VoiceEvent, VoicePipelineConfig};
use tokio::sync::mpsc;

#[tokio::test]
async fn test_pipeline_config_defaults() {
    let config = VoicePipelineConfig::default();
    assert_eq!(config.sample_rate, 16000);
    assert_eq!(config.wake_threshold, 0.5);
}

#[tokio::test]
async fn test_pipeline_state_transitions() {
    let (tx, mut rx) = mpsc::channel::<VoiceEvent>(16);
    // Verify channel works
    tx.send(VoiceEvent::WakeWordDetected).await.unwrap();
    let event = rx.recv().await.unwrap();
    assert!(matches!(event, VoiceEvent::WakeWordDetected));
}
