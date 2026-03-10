use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

/// Target sample rate for Whisper STT.
pub const SAMPLE_RATE: u32 = 16_000;
pub const CHANNELS: u16 = 1;

/// Preferred capture rates, in order. 48kHz is ideal because 48000/16000 = 3 (integer ratio).
const PREFERRED_RATES: &[u32] = &[48_000, 44_100, 96_000, 88_200, 16_000];

/// Number of input frames per resampler chunk. Must match `SincFixedIn::new` chunk_size.
const RESAMPLE_CHUNK_SIZE: usize = 1024;

pub struct AudioCapture {
    device: Device,
    config: StreamConfig,
    /// Ratio of capture rate to target rate (capture_rate / SAMPLE_RATE).
    /// When 1.0, no resampling is needed.
    resample_ratio: f64,
}

impl AudioCapture {
    pub fn new(device_name: Option<&str>) -> Result<Self> {
        let host = cpal::default_host();

        let device = match device_name {
            Some(name) => host
                .input_devices()?
                .find(|d| d.name().map(|n| n == name).unwrap_or(false))
                .context(format!("Audio device '{name}' not found"))?,
            None => host
                .default_input_device()
                .context("No default input device found")?,
        };

        // Find the best supported sample rate
        let supported: Vec<_> = device
            .supported_input_configs()
            .context("Failed to query supported input configs")?
            .collect();

        let capture_rate = pick_capture_rate(&supported)?;

        let config = StreamConfig {
            channels: CHANNELS,
            sample_rate: cpal::SampleRate(capture_rate),
            buffer_size: cpal::BufferSize::Fixed(256),
        };

        let resample_ratio = capture_rate as f64 / SAMPLE_RATE as f64;
        tracing::info!(
            device = %device.name().unwrap_or_default(),
            capture_rate,
            target_rate = SAMPLE_RATE,
            resample_ratio,
            "Audio capture configured"
        );

        Ok(Self {
            device,
            config,
            resample_ratio,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    pub fn list_input_devices() -> Result<Vec<String>> {
        let host = cpal::default_host();
        let devices: Vec<String> = host
            .input_devices()?
            .filter_map(|d| d.name().ok())
            .collect();
        Ok(devices)
    }

    pub fn start(&self, sender: mpsc::Sender<Vec<f32>>) -> Result<Stream> {
        let ratio = self.resample_ratio;
        let capture_rate = self.config.sample_rate.0;

        // Create a stateful sinc resampler if the capture rate differs from the target.
        // The resampler is shared with the cpal callback via Arc<Mutex> so it persists
        // across callbacks (avoiding clicks at chunk boundaries).
        //
        // SincFixedIn expects exactly `RESAMPLE_CHUNK_SIZE` input frames per call.
        // cpal delivers variable-length buffers, so we accumulate samples in an input
        // staging buffer and only process when a full chunk is ready.
        struct ResampleState {
            resampler: SincFixedIn<f32>,
            input_buf: Vec<f32>,
            chunk_buf: Vec<f32>,
        }

        let resampler_state: Option<Arc<Mutex<ResampleState>>> =
            if (ratio - 1.0).abs() > f64::EPSILON {
                let params = SincInterpolationParameters {
                    sinc_len: 128,
                    f_cutoff: 0.925,
                    interpolation: SincInterpolationType::Linear,
                    oversampling_factor: 128,
                    window: WindowFunction::BlackmanHarris2,
                };
                let resampler = SincFixedIn::<f32>::new(
                    SAMPLE_RATE as f64 / capture_rate as f64,
                    1.0,
                    params,
                    RESAMPLE_CHUNK_SIZE,
                    1, // mono
                )
                .map_err(|e| anyhow::anyhow!("Failed to create resampler: {e}"))?;
                Some(Arc::new(Mutex::new(ResampleState {
                    resampler,
                    input_buf: Vec::with_capacity(RESAMPLE_CHUNK_SIZE * 2),
                    chunk_buf: Vec::with_capacity(RESAMPLE_CHUNK_SIZE),
                })))
            } else {
                None
            };

        // Build a reusable audio data callback factory.
        // Both the primary (Fixed(256)) and fallback (Default) stream attempts share
        // the same Arc<Mutex<ResampleState>> and a cloned sender so either closure
        // can own them.
        let make_callback = |rs: Option<Arc<Mutex<ResampleState>>>, tx: mpsc::Sender<Vec<f32>>| {
            move |data: &[f32], _: &_| {
                match &rs {
                    None => {
                        // No resampling needed — forward directly.
                        if tx.send(data.to_vec()).is_err() {
                            tracing::warn!("Audio receiver dropped");
                        }
                    }
                    Some(state) => {
                        let Ok(mut st) = state.lock() else {
                            return; // mutex poisoned — silently skip this callback
                        };
                        st.input_buf.extend_from_slice(data);

                        // Destructure to allow the borrow checker to see disjoint field borrows.
                        let ResampleState {
                            ref mut resampler,
                            ref mut input_buf,
                            ref mut chunk_buf,
                        } = *st;

                        // Process full chunks until fewer than RESAMPLE_CHUNK_SIZE remain.
                        while input_buf.len() >= RESAMPLE_CHUNK_SIZE {
                            // Reuse pre-allocated chunk_buf to avoid per-iteration allocation.
                            chunk_buf.clear();
                            chunk_buf.extend_from_slice(&input_buf[..RESAMPLE_CHUNK_SIZE]);
                            input_buf.drain(..RESAMPLE_CHUNK_SIZE);
                            let wave_in = [chunk_buf.as_slice()];
                            match resampler.process(&wave_in, None) {
                                Ok(output) => {
                                    let resampled = output.into_iter().next().unwrap_or_default();
                                    if tx.send(resampled).is_err() {
                                        tracing::warn!("Audio receiver dropped");
                                        return;
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("Resampling error: {e}");
                                }
                            }
                        }
                    }
                }
            }
        };

        let err_fn = |err| tracing::error!("Audio stream error: {err}");

        // Try Fixed(256) first for lower capture latency; fall back to Default if
        // the hardware or driver rejects the fixed buffer size.
        let stream_result = self.device.build_input_stream(
            &self.config,
            make_callback(resampler_state.clone(), sender.clone()),
            err_fn,
            None,
        );

        let stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Fixed buffer size not supported ({e}), falling back to default");
                let mut fallback_config = self.config.clone();
                fallback_config.buffer_size = cpal::BufferSize::Default;
                self.device
                    .build_input_stream(
                        &fallback_config,
                        make_callback(resampler_state, sender),
                        |err| tracing::error!("Audio stream error: {err}"),
                        None,
                    )
                    .context("Failed to build audio input stream")?
            }
        };

        stream.play()?;
        Ok(stream)
    }
}

/// Pick the best capture rate from supported configs.
/// Prefers rates that divide evenly into 16kHz.
fn pick_capture_rate(supported: &[cpal::SupportedStreamConfigRange]) -> Result<u32> {
    for &rate in PREFERRED_RATES {
        let sample_rate = cpal::SampleRate(rate);
        if supported.iter().any(|c| {
            c.channels() >= 1
                && c.min_sample_rate() <= sample_rate
                && c.max_sample_rate() >= sample_rate
        }) {
            return Ok(rate);
        }
    }

    // Fall back to the default config's rate (capped at 48kHz)
    let first = supported.first().context("No supported audio configs")?;
    Ok(first.max_sample_rate().0.min(48_000))
}
