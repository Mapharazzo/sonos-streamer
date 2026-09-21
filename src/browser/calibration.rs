use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use crossbeam_channel::Sender;

use crate::{
    autocalibrate::measure_latency_roundtrip,
    browser::{commands::BrowserEvent, protocol::ServerEvent},
};

type MeasureLatency = Arc<dyn Fn() -> Option<u64> + Send + Sync>;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CalibrationBusy;

#[derive(Clone)]
pub struct CalibrationCoordinator {
    active: Arc<AtomicBool>,
    settle_delay: Duration,
    measure: MeasureLatency,
}

impl Default for CalibrationCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl CalibrationCoordinator {
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
    #[must_use]
    pub fn new() -> Self {
        Self::with_measurement(
            Duration::from_millis(800),
            Arc::new(measure_latency_roundtrip),
        )
    }

    fn with_measurement(settle_delay: Duration, measure: MeasureLatency) -> Self {
        Self {
            active: Arc::new(AtomicBool::new(false)),
            settle_delay,
            measure,
        }
    }

    pub fn start(
        &self,
        request_id: impl Into<String>,
        renderer_id: impl Into<String>,
        event_tx: Sender<BrowserEvent>,
    ) -> Result<(), CalibrationBusy> {
        self.start_with_measurement(request_id, renderer_id, event_tx, Arc::clone(&self.measure))
    }

    /// Managed mode injects the pulse into its generation, without the legacy trigger file.
    pub fn start_with_measurement(
        &self,
        request_id: impl Into<String>,
        renderer_id: impl Into<String>,
        event_tx: Sender<BrowserEvent>,
        measure: MeasureLatency,
    ) -> Result<(), CalibrationBusy> {
        self.active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| CalibrationBusy)?;

        let request_id = request_id.into();
        let renderer_id = renderer_id.into();
        let _ = event_tx.send(ServerEvent::CalibrationStarted {
            request_id: request_id.clone(),
        });
        let active = Arc::clone(&self.active);
        let settle_delay = self.settle_delay;
        thread::Builder::new()
            .name("browser_calibration".to_string())
            .spawn(move || {
                thread::sleep(settle_delay);
                let result = measure();
                active.store(false, Ordering::Release);
                let event = if let Some(milliseconds) = result {
                    ServerEvent::CalibrationResult {
                        request_id,
                        renderer_id,
                        milliseconds: u32::try_from(milliseconds).unwrap_or(u32::MAX),
                    }
                } else {
                    ServerEvent::CalibrationFailed {
                        request_id,
                        code: "calibration-timeout".to_string(),
                        message: "no calibration pulse was detected".to_string(),
                    }
                };
                let _ = event_tx.send(event);
            })
            .expect("failed to start browser calibration worker");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use crossbeam_channel::{bounded, unbounded};

    use super::{CalibrationBusy, CalibrationCoordinator};
    use crate::browser::protocol::ServerEvent;

    #[test]
    fn calibration_runs_once_and_rejects_overlap() {
        let (release_tx, release_rx) = bounded::<()>(0);
        let coordinator = CalibrationCoordinator::with_measurement(
            Duration::ZERO,
            Arc::new(move || {
                release_rx.recv().expect("release measurement");
                Some(123)
            }),
        );
        let (event_tx, event_rx) = unbounded();

        coordinator
            .start("cal-1", "renderer-1", event_tx.clone())
            .expect("first calibration");
        assert_eq!(
            coordinator.start("cal-2", "renderer-1", event_tx),
            Err(CalibrationBusy)
        );
        assert!(matches!(
            event_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            ServerEvent::CalibrationStarted { request_id } if request_id == "cal-1"
        ));

        release_tx.send(()).expect("finish measurement");
        assert!(matches!(
            event_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            ServerEvent::CalibrationResult {
                request_id,
                renderer_id,
                milliseconds: 123,
            } if request_id == "cal-1" && renderer_id == "renderer-1"
        ));
    }
}
