//! Startup latency calibration: inject Barker pulse, measure round-trip, reconnect if too high.

use std::net::IpAddr;
use std::thread;
use std::time::Duration;

use crossbeam_channel::bounded;
use log::{info, warn};

use crate::latency::{arm_latency_response_waiter, pulse_trigger_path};
use crate::openhome::rendercontrol::{Renderer, StreamInfo};

/// Inject a pulse (via streaming path) and wait for mic detection. Requires an active HTTP stream.
#[must_use]
pub fn measure_latency_roundtrip() -> Option<u64> {
    let (tx, rx) = bounded(1);
    arm_latency_response_waiter(tx);
    let path = pulse_trigger_path();
    if std::fs::write(&path, b"x").is_err() {
        warn!("Could not write pulse trigger at {path:?}");
        return None;
    }
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Some(ms)) => Some(ms),
        Ok(None) => None,
        Err(_) => {
            warn!("Latency measurement timed out (is the speaker streaming?)");
            None
        }
    }
}

/// After playback starts: measure latency; if above threshold, stop/play all renderers and retry.
pub fn run_startup_calibration(
    players: &mut [Renderer],
    local_addr: &IpAddr,
    streaminfo: StreamInfo,
    threshold_ms: u32,
    max_retries: usize,
) {
    // Allow Sonos to open the HTTP stream before first pulse.
    thread::sleep(Duration::from_millis(800));
    for attempt in 0..max_retries {
        let Some(ms) = measure_latency_roundtrip() else {
            warn!("Calibration attempt {}: no measurement", attempt + 1);
            thread::sleep(Duration::from_millis(400));
            continue;
        };
        if ms <= threshold_ms {
            info!("Startup latency OK: {ms} ms (threshold {threshold_ms} ms)");
            return;
        }
        warn!(
            "Latency {ms} ms > {threshold_ms} ms — reconnecting (attempt {})",
            attempt + 1
        );
        for p in players.iter_mut() {
            p.stop_play();
        }
        thread::sleep(Duration::from_millis(250));
        for p in players.iter_mut() {
            let _ = p.play(local_addr, streaminfo);
        }
        thread::sleep(Duration::from_millis(1200));
    }
    warn!("Startup calibration stopped after {max_retries} attempts");
}
