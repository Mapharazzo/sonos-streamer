use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use log::{info, error};
use std::sync::atomic::Ordering;
use crate::latency::{PULSE_INJECTED_AT, barker_mono, now_ms};

pub fn start_latency_listener() {
    std::thread::spawn(|| {
        let host = cpal::default_host();
        let device = match host.default_input_device() {
            Some(d) => d,
            None => {
                error!("No default input device found for latency listener.");
                return;
            }
        };

        let config = match device.default_input_config() {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to get default input config: {}", e);
                return;
            }
        };

        info!("Listening for latency pulse on input device: {}", device.name().unwrap_or_default());

        let pulse = barker_mono();
        let mut buffer: Vec<f32> = Vec::new();

        let err_fn = |err| error!("An error occurred on the input audio stream: {}", err);

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &_| process_input(data, &mut buffer, &pulse),
                err_fn,
                None,
            ),
            _ => {
                error!("Unsupported microphone format. F32 required for prototype.");
                return;
            }
        }.unwrap();

        stream.play().unwrap();

        // Keep thread alive
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
}

fn process_input(data: &[f32], buffer: &mut Vec<f32>, pulse: &[f32]) {
    let injected_at = PULSE_INJECTED_AT.load(Ordering::SeqCst);
    if injected_at == 0 {
        // Not waiting for a pulse
        if !buffer.is_empty() {
            buffer.clear();
        }
        return;
    }

    // Naive stereo-to-mono downmix (just take alternating samples assuming 2 channels)
    // For a robust implementation, we'd query config.channels()
    for (i, &sample) in data.iter().enumerate() {
        if i % 2 == 0 { 
            buffer.push(sample);
        }
    }

    // Cap buffer size (~3 seconds of audio at 48kHz)
    if buffer.len() > 150_000 {
        buffer.drain(0..buffer.len() - 100_000);
    }

    // Cross-correlation
    if buffer.len() >= pulse.len() {
        let mut max_corr = 0.0;
        
        let search_len = buffer.len() - pulse.len();
        // Step by 10 to save CPU on the prototype scan
        for i in (0..search_len).step_by(10) { 
            let mut corr = 0.0;
            for j in 0..pulse.len() {
                corr += buffer[i + j] * pulse[j];
            }
            if corr > max_corr {
                max_corr = corr;
            }
        }

        // Threshold for detection - will need tuning based on mic gain!
        if max_corr > 50.0 { 
            let now = now_ms();
            let latency = now - injected_at;
            info!("======================================================");
            info!("💥 PULSE DETECTED! Max Correlation: {:.2}", max_corr);
            info!("⏱️  TOTAL LATENCY MEASURED: {} ms", latency);
            info!("======================================================");
            
            // Reset state
            PULSE_INJECTED_AT.store(0, Ordering::SeqCst);
            buffer.clear();
        }
    }
}
