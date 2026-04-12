use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Sample, SampleFormat};
use log::{error, info, warn};
use std::sync::atomic::Ordering;

use crate::enums::messages::MessageType;
use crate::globals::statics::get_msgchannel;
use crate::latency::{PULSE_INJECTED_AT, barker_mono, complete_latency_measurement, now_ms};
use crate::utils::audiodevices::{cpal_device_display_name, pick_input_cpal_device};

const NCC_THRESHOLD: f32 = 0.35;

/// Start the latency mic listener. `input_name` is optional substring match (case-insensitive).
pub fn start_latency_listener(input_name: Option<String>) {
    std::thread::spawn(move || {
        let device = match pick_input_cpal_device(input_name.as_deref()) {
            Some(d) => d,
            None => {
                error!("No input device available for latency listener.");
                return;
            }
        };

        let config = match device.default_input_config() {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to get default input config: {e}");
                return;
            }
        };

        let channels = usize::from(config.channels());
        info!(
            "Latency listener on: {} ({} ch, {:?})",
            cpal_device_display_name(&device),
            channels,
            config.sample_format()
        );

        let pulse = barker_mono();
        let pulse_energy: f32 = pulse.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);

        let mut buffer: Vec<f32> = Vec::new();
        let err_fn = |err| error!("Input audio stream error: {err}");

        let stream_result = match config.sample_format() {
            SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &_| {
                    process_input(data, channels, &mut buffer, &pulse, pulse_energy)
                },
                err_fn,
                None,
            ),
            SampleFormat::I16 => device.build_input_stream(
                &config.into(),
                move |data: &[i16], _: &_| {
                    let f32s: Vec<f32> = data.iter().map(|s| s.to_sample()).collect();
                    process_input(&f32s, channels, &mut buffer, &pulse, pulse_energy)
                },
                err_fn,
                None,
            ),
            SampleFormat::U16 => device.build_input_stream(
                &config.into(),
                move |data: &[u16], _: &_| {
                    let f32s: Vec<f32> = data.iter().map(|s| s.to_sample()).collect();
                    process_input(&f32s, channels, &mut buffer, &pulse, pulse_energy)
                },
                err_fn,
                None,
            ),
            SampleFormat::I32 => device.build_input_stream(
                &config.into(),
                move |data: &[i32], _: &_| {
                    let f32s: Vec<f32> = data.iter().map(|s| s.to_sample()).collect();
                    process_input(&f32s, channels, &mut buffer, &pulse, pulse_energy)
                },
                err_fn,
                None,
            ),
            other => {
                warn!("Unsupported input sample format {other:?}; latency detection disabled.");
                return;
            }
        };

        let stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to build input stream: {e}");
                return;
            }
        };

        stream.play().expect("latency listener stream");

        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
}

/// Interleaved samples -> mono mean per frame.
fn downmix_interleaved(samples: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    if channels == 0 {
        return;
    }
    let frames = samples.len() / channels;
    for f in 0..frames {
        let base = f * channels;
        let mut sum = 0.0f32;
        for c in 0..channels {
            sum += samples[base + c];
        }
        out.push(sum / channels as f32);
    }
}

fn process_input(
    interleaved: &[f32],
    channels: usize,
    buffer: &mut Vec<f32>,
    pulse: &[f32],
    pulse_energy: f32,
) {
    let injected_at = PULSE_INJECTED_AT.load(Ordering::SeqCst);
    if injected_at == 0 {
        buffer.clear();
        return;
    }

    let mut frame_mono = Vec::new();
    downmix_interleaved(interleaved, channels, &mut frame_mono);
    buffer.extend_from_slice(&frame_mono);

    // Cap buffer (~3s at 48kHz mono)
    if buffer.len() > 150_000 {
        buffer.drain(0..buffer.len().saturating_sub(100_000));
    }

    let plen = pulse.len();
    if buffer.len() < plen {
        return;
    }

    let search_len = buffer.len() - plen;
    let mut best_ncc = 0.0f32;
    let step = 4usize.max(plen / 2000);

    for i in (0..search_len).step_by(step) {
        let mut dot = 0.0f32;
        let mut sig_e = 0.0f32;
        for j in 0..plen {
            let s = buffer[i + j];
            dot += s * pulse[j];
            sig_e += s * s;
        }
        let sig_energy = sig_e.sqrt().max(1e-9);
        let ncc = dot / (sig_energy * pulse_energy);
        if ncc > best_ncc {
            best_ncc = ncc;
        }
    }

    if best_ncc >= NCC_THRESHOLD {
        let now = now_ms();
        let latency = now.saturating_sub(injected_at);
        info!(
            "Latency pulse detected (ncc={best_ncc:.3}) — round-trip {} ms",
            latency
        );
        complete_latency_measurement(latency);
        let _ = get_msgchannel().0.send(MessageType::LatencyResult(latency));
        PULSE_INJECTED_AT.store(0, Ordering::SeqCst);
        buffer.clear();
    }
}
