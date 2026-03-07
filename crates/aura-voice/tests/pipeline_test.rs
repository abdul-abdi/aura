use aura_voice::pipeline::{PipelineState, VoiceEvent, VoicePipeline, VoicePipelineConfig};
use tokio::sync::mpsc;

#[tokio::test]
async fn test_pipeline_config_defaults() {
    let config = VoicePipelineConfig::default();
    assert_eq!(config.sample_rate, 16_000);
    assert_eq!(config.wake_threshold, 0.5);
    assert_eq!(config.silence_timeout_ms, 2_000);
    assert_eq!(config.max_listen_ms, 10_000);
}

#[tokio::test]
async fn test_pipeline_idle_to_listening() {
    let (tx, mut rx) = mpsc::channel::<VoiceEvent>(16);
    let config = VoicePipelineConfig::default();
    let mut pipeline = VoicePipeline::new(config, tx);

    assert_eq!(pipeline.state(), PipelineState::Idle);

    pipeline.on_wake_word_detected().await.unwrap();
    assert_eq!(pipeline.state(), PipelineState::Listening);

    let e1 = rx.recv().await.unwrap();
    assert!(matches!(e1, VoiceEvent::WakeWordDetected));
    let e2 = rx.recv().await.unwrap();
    assert!(matches!(e2, VoiceEvent::ListeningStarted));
}

#[tokio::test]
async fn test_pipeline_listening_to_idle_on_audio() {
    let (tx, mut rx) = mpsc::channel::<VoiceEvent>(16);
    let config = VoicePipelineConfig::default();
    let mut pipeline = VoicePipeline::new(config, tx);

    pipeline.on_wake_word_detected().await.unwrap();
    // drain wake word events
    let _ = rx.recv().await;
    let _ = rx.recv().await;

    pipeline.on_audio_captured(&[0.0; 1600]).await.unwrap();
    assert_eq!(pipeline.state(), PipelineState::Idle);

    let e1 = rx.recv().await.unwrap();
    assert!(matches!(e1, VoiceEvent::Transcription { .. }));
    let e2 = rx.recv().await.unwrap();
    assert!(matches!(e2, VoiceEvent::ListeningStopped));
}

#[tokio::test]
async fn test_pipeline_ignores_audio_when_idle() {
    let (tx, mut rx) = mpsc::channel::<VoiceEvent>(16);
    let config = VoicePipelineConfig::default();
    let mut pipeline = VoicePipeline::new(config, tx);

    pipeline.on_audio_captured(&[0.0; 1600]).await.unwrap();
    assert_eq!(pipeline.state(), PipelineState::Idle);

    // No events should have been sent
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn test_wake_word_rejected_when_not_idle() {
    let (tx, _rx) = mpsc::channel::<VoiceEvent>(16);
    let config = VoicePipelineConfig::default();
    let mut pipeline = VoicePipeline::new(config, tx);
    pipeline.on_wake_word_detected().await.unwrap();
    assert!(pipeline.on_wake_word_detected().await.is_err());
    assert_eq!(pipeline.state(), PipelineState::Listening);
}
