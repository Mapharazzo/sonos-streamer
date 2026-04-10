use std::sync::atomic::AtomicU64;
use std::time::{SystemTime, UNIX_EPOCH};

pub static PULSE_INJECTED_AT: AtomicU64 = AtomicU64::new(0);

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

pub fn barker_mono() -> Vec<f32> {
    let barker: [f32; 11] = [1.0, 1.0, 1.0, -1.0, -1.0, -1.0, 1.0, -1.0, -1.0, 1.0, -1.0];
    let mut pulse = Vec::with_capacity(1100);
    // Stretch the pulse to make it audible and detectable (~25ms)
    for bit in barker.iter() {
        for _ in 0..100 {
            pulse.push(*bit);
        }
    }
    pulse
}
