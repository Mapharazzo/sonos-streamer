use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::time::{SystemTime, UNIX_EPOCH};

use crossbeam_channel::Sender;

use crate::globals::statics::get_config;

pub static PULSE_INJECTED_AT: AtomicU64 = AtomicU64::new(0);

/// One-shot waiter for HTTP `/latency/trigger` or startup calibration.
static PENDING_LATENCY_REPLY: Mutex<Option<Sender<Option<u64>>>> = Mutex::new(None);

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
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

/// Path for the latency pulse trigger file (same directory as config).
#[must_use]
pub fn pulse_trigger_path() -> PathBuf {
    get_config().config_dir().join("trigger_pulse.txt")
}

/// Register a channel to receive `Some(latency_ms)` when the next pulse is detected, or `None` on abandon.
pub fn arm_latency_response_waiter(reply: Sender<Option<u64>>) {
    if let Ok(mut g) = PENDING_LATENCY_REPLY.lock() {
        *g = Some(reply);
    }
}

/// Called from the mic listener when a pulse is detected.
pub fn complete_latency_measurement(millis: u64) {
    if let Ok(mut g) = PENDING_LATENCY_REPLY.lock() {
        if let Some(tx) = g.take() {
            let _ = tx.send(Some(millis));
        }
    }
}
