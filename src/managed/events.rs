use super::protocol::Snapshot;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError, bounded};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum BridgeEvent {
    State(Snapshot),
    ReleaseCapture { transition_id: String },
    BrowserAction { action_id: String },
}

impl BridgeEvent {
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        match self {
            Self::State(state) => serde_json::to_string(state),
            Self::ReleaseCapture { transition_id } => serde_json::to_string(
                &serde_json::json!({"type":"release-capture", "transitionId":transition_id}),
            ),
            Self::BrowserAction { action_id } => serde_json::to_string(
                &serde_json::json!({"type":"browser-action", "actionId":action_id, "action":"reload-tab"}),
            ),
        }
    }
}

struct Subscriber {
    owner: String,
    sender: Sender<BridgeEvent>,
    closed: Arc<AtomicBool>,
}
#[derive(Default)]
struct BusState {
    latest: Option<Snapshot>,
    subscribers: Vec<Subscriber>,
}

pub struct EventBus {
    capacity: usize,
    state: Mutex<BusState>,
}

impl EventBus {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.clamp(1, 256),
            state: Mutex::new(BusState::default()),
        }
    }

    #[must_use]
    pub fn subscribe(&self) -> EventSubscription {
        self.subscribe_for("")
    }

    #[must_use]
    pub fn subscribe_for(&self, owner: &str) -> EventSubscription {
        let (sender, receiver) = bounded(self.capacity);
        let closed = Arc::new(AtomicBool::new(false));
        let mut state = self.state.lock().unwrap();
        state
            .subscribers
            .retain(|entry| !entry.closed.load(Ordering::Acquire));
        if state.subscribers.len() >= 64 {
            closed.store(true, Ordering::Release);
        } else {
            if let Some(snapshot) = &state.latest {
                let _ = sender.try_send(BridgeEvent::State(snapshot.clone()));
            }
            state.subscribers.push(Subscriber {
                owner: owner.into(),
                sender,
                closed: Arc::clone(&closed),
            });
        }
        EventSubscription { receiver, closed }
    }

    pub fn publish(&self, snapshot: Snapshot) {
        let mut state = self.state.lock().unwrap();
        state.latest = Some(snapshot.clone());
        Self::send(&mut state, None, BridgeEvent::State(snapshot));
    }

    pub fn send_to(&self, owner: &str, event: BridgeEvent) {
        Self::send(&mut self.state.lock().unwrap(), Some(owner), event);
    }

    #[must_use]
    pub fn latest(&self) -> Option<Snapshot> {
        self.state.lock().unwrap().latest.clone()
    }

    fn send(state: &mut BusState, owner: Option<&str>, event: BridgeEvent) {
        state.subscribers.retain(|subscriber| {
            if subscriber.closed.load(Ordering::Acquire) {
                return false;
            }
            if owner.is_none_or(|owner| subscriber.owner == owner)
                && subscriber.sender.try_send(event.clone()).is_err()
            {
                subscriber.closed.store(true, Ordering::Release);
                return false;
            }
            true
        });
    }
}

pub struct EventSubscription {
    receiver: Receiver<BridgeEvent>,
    closed: Arc<AtomicBool>,
}

impl EventSubscription {
    pub fn recv_timeout(&self, timeout: Duration) -> Result<BridgeEvent, RecvTimeoutError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(RecvTimeoutError::Disconnected);
        }
        let result = self.receiver.recv_timeout(timeout);
        if self.closed.load(Ordering::Acquire) {
            return Err(RecvTimeoutError::Disconnected);
        }
        result
    }

    pub fn try_recv(&self) -> Result<BridgeEvent, TryRecvError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TryRecvError::Disconnected);
        }
        self.receiver.try_recv()
    }
}

impl Drop for EventSubscription {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
    }
}
