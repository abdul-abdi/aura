use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig};
use std::sync::mpsc;

/// Target sample rate for Whisper STT.
pub const SAMPLE_RATE: u32 = 16_000;
pub const CHANNELS: u16 = 1;

/// Preferred capture rates, in order. 48kHz is ideal because 48000/16000 = 3 (integer ratio).
const PREFERRED_RATES: &[u32] = &[48_000, 44_100, 96_000, 88_200, 16_000];

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
            buffer_size: cpal::BufferSize::Default,
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
        let err_fn = |err| tracing::error!("Audio stream error: {err}");
        let ratio = self.resample_ratio;

        let stream = self
            .device
            .build_input_stream(
                &self.config,
                move |data: &[f32], _| {
                    let resampled = if (ratio - 1.0).abs() < f64::EPSILON {
                        data.to_vec()
                    } else {
                        downsample(data, ratio)
                    };
                    if sender.send(resampled).is_err() {
                        tracing::warn!("Audio receiver dropped, data lost");
                    }
                },
                err_fn,
                None,
            )
            .context("Failed to build audio input stream")?;

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

    // Fall back to the default config's rate
    let first = supported.first().context("No supported audio configs")?;
    Ok(first.min_sample_rate().0)
}

/// Downsample audio by the given ratio using linear interpolation.
fn downsample(input: &[f32], ratio: f64) -> Vec<f32> {
    let output_len = (input.len() as f64 / ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        let frac = (src_pos - idx as f64) as f32;

        let sample = if idx + 1 < input.len() {
            input[idx] * (1.0 - frac) + input[idx + 1] * frac
        } else {
            input[idx.min(input.len() - 1)]
        };
        output.push(sample);
    }

    output
}
