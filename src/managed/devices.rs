//! Capture is owned by the runtime thread; callbacks publish only to one generation.
use super::audio::GenerationSink;
use crate::openhome::rendercontrol::WavData;
use cpal::{
    Sample, SampleFormat, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use crossbeam_channel::Sender;
use std::sync::Arc;

pub struct OutputDevice {
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
}

#[derive(serde::Serialize)]
pub struct DeviceChoice {
    pub id: String,
    pub name: String,
    pub direction: &'static str,
}

/// Enumerates metadata only. It never builds or starts an audio stream.
pub fn list_devices() -> Result<Vec<DeviceChoice>, &'static str> {
    let host = cpal::default_host();
    let mut result = Vec::new();
    for (direction, devices) in [
        ("output", host.output_devices()),
        ("input", host.input_devices()),
    ] {
        for device in devices.map_err(|_| "device-list-unavailable")?.take(32) {
            if let (Ok(id), Ok(description)) = (device.id(), device.description()) {
                let id = format!("id:{id}");
                let name = description
                    .extended()
                    .first()
                    .cloned()
                    .unwrap_or_else(|| description.name().to_owned());
                if id.len() <= 512
                    && name.len() <= 512
                    && !id.chars().any(char::is_control)
                    && !name.chars().any(char::is_control)
                {
                    result.push(DeviceChoice {
                        id,
                        name,
                        direction,
                    });
                }
            }
        }
    }
    Ok(result)
}
impl OutputDevice {
    pub fn open(name: Option<&str>) -> Result<Self, &'static str> {
        let host = cpal::default_host();
        let device = if let Some(name) = name.filter(|n| !n.is_empty()) {
            host.output_devices()
                .map_err(|_| "output-device-unavailable")?
                .find(|device| {
                    if let Some(id) = name.strip_prefix("id:") {
                        device.id().is_ok_and(|d| d.to_string() == id)
                    } else {
                        device.description().is_ok_and(|d| d.name() == name)
                    }
                })
        } else {
            host.default_output_device()
        }
        .ok_or("output-device-unavailable")?;
        let config = device
            .default_output_config()
            .map_err(|_| "output-format-unavailable")?;
        if config.channels() != 2
            || !(8000..=192000).contains(&config.sample_rate())
            || !matches!(
                config.sample_format(),
                SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16 | SampleFormat::I32
            )
        {
            return Err("unsupported-output-format");
        }
        Ok(Self { device, config })
    }
    pub fn format(&self) -> WavData {
        WavData {
            sample_rate: self.config.sample_rate(),
            sample_format: self.config.sample_format(),
            channels: 2,
        }
    }
    pub fn capture(
        &self,
        sink: GenerationSink,
        errors: Sender<()>,
    ) -> Result<cpal::Stream, &'static str> {
        let stream = match self.config.sample_format() {
            SampleFormat::F32 => capture::<f32>(&self.device, &self.config.config(), sink, errors),
            SampleFormat::I16 => capture::<i16>(&self.device, &self.config.config(), sink, errors),
            SampleFormat::U16 => capture::<u16>(&self.device, &self.config.config(), sink, errors),
            SampleFormat::I32 => capture::<i32>(&self.device, &self.config.config(), sink, errors),
            _ => return Err("unsupported-output-format"),
        }
        .map_err(|_| "capture-open-failed")?;
        stream.play().map_err(|_| "capture-start-failed")?;
        Ok(stream)
    }
}
fn capture<T: Sample + SizedSample + dasp_sample::ToSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sink: GenerationSink,
    errors: Sender<()>,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    device.build_input_stream(
        config,
        move |data: &[T], _: &_| {
            for chunk in data.chunks(32768) {
                let _ = sink.publish(Arc::new(
                    chunk.iter().map(|s| s.to_sample::<f32>()).collect(),
                ));
            }
        },
        move |_| {
            let _ = errors.try_send(());
        },
        None,
    )
}
