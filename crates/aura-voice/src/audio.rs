use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig};
use std::sync::mpsc;

pub const SAMPLE_RATE: u32 = 16_000;
pub const CHANNELS: u16 = 1;

pub struct AudioCapture {
    device: Device,
    config: StreamConfig,
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

        let config = StreamConfig {
            channels: CHANNELS,
            sample_rate: cpal::SampleRate(SAMPLE_RATE),
            buffer_size: cpal::BufferSize::Default,
        };

        Ok(Self { device, config })
    }

    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate.0
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

        let stream = self.device.build_input_stream(
            &self.config,
            move |data: &[f32], _| {
                if sender.send(data.to_vec()).is_err() {
                    tracing::warn!("Audio receiver dropped, data lost");
                }
            },
            err_fn,
            None,
        ).context("Device does not support 16kHz mono f32 input")?;

        stream.play()?;
        Ok(stream)
    }
}
