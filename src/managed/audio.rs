use super::protocol::MAX_COUNTER;
use crate::{openhome::rendercontrol::WavData, utils::rwstream::AudioSamples};
use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
use std::{
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

const QUEUE_PACKETS: usize = 8;
const MAX_SUBSCRIBERS: usize = 16;
const MAX_SAMPLES: usize = 32768;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AudioError {
    Retired,
    InvalidGeneration,
    UnsupportedFormat,
    PartialFrame,
    TooLarge,
    SubscriberLimit,
    Timeout,
}

impl fmt::Display for AudioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for AudioError {}

struct Generation {
    id: u64,
    format: WavData,
    retired: AtomicBool,
    subscribers: Mutex<Vec<Sender<AudioSamples>>>,
    dropped: AtomicU64,
}

#[derive(Default)]
struct HubState {
    latest: u64,
    active: Option<Arc<Generation>>,
}

#[derive(Default)]
pub struct AudioHub {
    state: Mutex<HubState>,
}

impl AudioHub {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin(&self, generation: u64, format: WavData) -> Result<GenerationSink, AudioError> {
        if generation == 0 || generation > MAX_COUNTER {
            return Err(AudioError::InvalidGeneration);
        }
        if format.channels != 2 || !(8000..=192000).contains(&format.sample_rate) {
            return Err(AudioError::UnsupportedFormat);
        }
        let mut state = self.state.lock().unwrap();
        if generation <= state.latest {
            return Err(AudioError::InvalidGeneration);
        }
        if let Some(old) = state.active.take() {
            old.retired.store(true, Ordering::Release);
        }
        let active = Arc::new(Generation {
            id: generation,
            format,
            retired: AtomicBool::new(false),
            subscribers: Mutex::new(vec![]),
            dropped: AtomicU64::new(0),
        });
        state.latest = generation;
        state.active = Some(Arc::clone(&active));
        Ok(GenerationSink { active })
    }

    pub fn subscribe(&self, generation: u64) -> Result<AudioSubscription, AudioError> {
        let state = self.state.lock().unwrap();
        let active = state
            .active
            .as_ref()
            .filter(|active| active.id == generation)
            .ok_or(AudioError::Retired)?;
        if active.retired.load(Ordering::Acquire) {
            return Err(AudioError::Retired);
        }
        let mut subscribers = active.subscribers.lock().unwrap();
        if subscribers.len() >= MAX_SUBSCRIBERS {
            return Err(AudioError::SubscriberLimit);
        }
        let (sender, receiver) = bounded(QUEUE_PACKETS);
        subscribers.push(sender.clone());
        Ok(AudioSubscription {
            active: Arc::clone(active),
            sender,
            receiver,
        })
    }

    pub fn end(&self, generation: u64) {
        let mut state = self.state.lock().unwrap();
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.id == generation)
            && let Some(active) = state.active.take()
        {
            active.retired.store(true, Ordering::Release);
        }
    }

    pub fn end_current(&self) {
        if let Some(active) = self.state.lock().unwrap().active.take() {
            active.retired.store(true, Ordering::Release);
        }
    }

    #[must_use]
    pub fn latest_generation(&self) -> u64 {
        self.state.lock().unwrap().latest
    }
}

#[derive(Clone)]
pub struct GenerationSink {
    active: Arc<Generation>,
}

impl GenerationSink {
    pub fn publish(&self, samples: AudioSamples) -> Result<(), AudioError> {
        if self.active.retired.load(Ordering::Acquire) {
            return Err(AudioError::Retired);
        }
        if samples.len() > MAX_SAMPLES {
            return Err(AudioError::TooLarge);
        }
        if !samples
            .len()
            .is_multiple_of(usize::from(self.active.format.channels))
        {
            return Err(AudioError::PartialFrame);
        }
        if samples.is_empty() {
            return Ok(());
        }
        self.active.subscribers.lock().unwrap().retain(|sender| {
            match sender.try_send(Arc::clone(&samples)) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => {
                    self.active.dropped.fetch_add(1, Ordering::Relaxed);
                    true
                }
                Err(TrySendError::Disconnected(_)) => false,
            }
        });
        Ok(())
    }

    #[must_use]
    pub fn dropped_packets(&self) -> u64 {
        self.active.dropped.load(Ordering::Relaxed)
    }
}

pub struct AudioSubscription {
    active: Arc<Generation>,
    sender: Sender<AudioSamples>,
    receiver: Receiver<AudioSamples>,
}

impl Drop for AudioSubscription {
    fn drop(&mut self) {
        self.active
            .subscribers
            .lock()
            .unwrap()
            .retain(|sender| !sender.same_channel(&self.sender));
    }
}

impl AudioSubscription {
    #[must_use]
    pub fn format(&self) -> WavData {
        self.active.format
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<AudioSamples, AudioError> {
        if self.active.retired.load(Ordering::Acquire) {
            return Err(AudioError::Retired);
        }
        let received = self
            .receiver
            .recv_timeout(timeout)
            .map_err(|error| match error {
                crossbeam_channel::RecvTimeoutError::Timeout => AudioError::Timeout,
                crossbeam_channel::RecvTimeoutError::Disconnected => AudioError::Retired,
            });
        if self.active.retired.load(Ordering::Acquire) {
            return Err(AudioError::Retired);
        }
        received
    }
}
