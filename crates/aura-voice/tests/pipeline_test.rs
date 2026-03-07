use aura_voice::pipeline::VoiceEvent;

#[test]
fn test_voice_event_variants() {
    let _started = VoiceEvent::ListeningStarted;
    let _stopped = VoiceEvent::ListeningStopped;
    let _transcription = VoiceEvent::Transcription {
        text: "hello".into(),
    };
    let _error = VoiceEvent::Error {
        message: "test".into(),
    };
}
