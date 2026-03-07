use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig};
use std::sync::mpsc;

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
            channels: 1,
            sample_rate: cpal::SampleRate(16000),
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
                let _ = sender.send(data.to_vec());
            },
            err_fn,
            None,
        )?;

        stream.play()?;
        Ok(stream)
    }
}
