use anyhow::{Context, Result};
use rodio::{OutputStream, OutputStreamHandle, Sink};
use std::sync::{Arc, Mutex};

pub struct AudioPlayer {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    sink: Arc<Mutex<Option<Sink>>>,
}

impl AudioPlayer {
    pub fn new() -> Result<Self> {
        let (stream, handle) =
            OutputStream::try_default().context("Failed to open audio output device")?;

        Ok(Self {
            _stream: stream,
            handle,
            sink: Arc::new(Mutex::new(None)),
        })
    }

    /// Play f32 PCM audio at the given sample rate.
    pub fn play(&self, samples: Vec<f32>, sample_rate: u32) -> Result<()> {
        self.stop(); // Cancel any current playback

        let source = rodio::buffer::SamplesBuffer::new(1, sample_rate, samples);
        let sink = Sink::try_new(&self.handle).context("Failed to create audio sink")?;
        sink.append(source);

        *self.sink.lock().map_err(|e| anyhow::anyhow!("Sink lock: {e}"))? = Some(sink);
        Ok(())
    }

    /// Stop current playback (for barge-in).
    pub fn stop(&self) {
        if let Ok(mut guard) = self.sink.lock() {
            if let Some(sink) = guard.take() {
                sink.stop();
            }
        }
    }

    /// Check if audio is currently playing.
    pub fn is_playing(&self) -> bool {
        self.sink
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|s| !s.empty()))
            .unwrap_or(false)
    }
}
