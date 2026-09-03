use crate::{
    browser::protocol::{ClientMessage, RendererSummary, ServerEvent},
    enums::streaming::StreamingState,
};
use std::time::{Duration, Instant};

pub fn renderer_list_event(
    request_id: impl Into<String>,
    renderers: impl IntoIterator<Item = RendererSummary>,
) -> ServerEvent {
    ServerEvent::RendererList {
        request_id: request_id.into(),
        renderers: renderers.into_iter().collect(),
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BrowserCommand {
    pub request_id: String,
    pub kind: BrowserCommandKind,
    pub producer_generation: u64,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum BrowserCommandKind {
    ListRenderers,
    Cast { renderer_id: String },
    Stop,
    Calibrate,
    SetManualOffset { milliseconds: i32 },
    GetState,
}

impl BrowserCommand {
    #[must_use]
    pub fn from_client_message(message: ClientMessage, producer_generation: u64) -> Option<Self> {
        let (request_id, kind) = match message {
            ClientMessage::Hello(_) => return None,
            ClientMessage::ListRenderers { request_id } => {
                (request_id, BrowserCommandKind::ListRenderers)
            }
            ClientMessage::Cast {
                request_id,
                renderer_id,
            } => (request_id, BrowserCommandKind::Cast { renderer_id }),
            ClientMessage::Stop { request_id } => (request_id, BrowserCommandKind::Stop),
            ClientMessage::Calibrate { request_id } => (request_id, BrowserCommandKind::Calibrate),
            ClientMessage::SetManualOffset {
                request_id,
                milliseconds,
            } => (
                request_id,
                BrowserCommandKind::SetManualOffset { milliseconds },
            ),
            ClientMessage::GetState { request_id } => (request_id, BrowserCommandKind::GetState),
        };
        Some(Self {
            request_id,
            kind,
            producer_generation,
        })
    }
}

pub type BrowserEvent = ServerEvent;

#[must_use]
pub fn should_accept_stream_feedback(
    selected_address: Option<&str>,
    remote_address: &str,
    state: StreamingState,
    matching_connection_active: bool,
) -> bool {
    selected_address == Some(remote_address)
        && (state == StreamingState::Started || !matching_connection_active)
}

#[derive(Debug)]
struct PendingStreamEnd {
    remote_address: String,
    deadline: Instant,
}

#[derive(Debug, Default)]
pub struct BrowserStreamEndDebouncer {
    pending: Option<PendingStreamEnd>,
}

impl BrowserStreamEndDebouncer {
    pub fn note_ended(&mut self, remote_address: &str, now: Instant, grace: Duration) {
        self.pending = Some(PendingStreamEnd {
            remote_address: remote_address.to_string(),
            deadline: now + grace,
        });
    }

    pub fn note_started(&mut self, remote_address: &str) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.remote_address == remote_address)
        {
            self.pending = None;
        }
    }

    pub fn clear(&mut self) {
        self.pending = None;
    }

    pub fn take_due(
        &mut self,
        now: Instant,
        selected_address: Option<&str>,
        matching_connection_active: bool,
    ) -> Option<String> {
        let pending = self.pending.as_ref()?;
        if now < pending.deadline {
            return None;
        }
        let pending = self.pending.take().expect("pending stream end");
        if selected_address == Some(pending.remote_address.as_str()) && !matching_connection_active
        {
            Some(pending.remote_address)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum BrowserPlaybackState {
    Idle,
    Starting {
        renderer_id: String,
        producer_generation: u64,
        started_at: Instant,
    },
    Streaming {
        renderer_id: String,
        auto_calibration_started: bool,
        producer_generation: u64,
    },
    Calibrating {
        renderer_id: String,
        producer_generation: u64,
    },
    Failed {
        renderer_id: Option<String>,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PlaybackAction {
    None,
    StartCalibration,
}

impl BrowserPlaybackState {
    #[must_use]
    pub fn starting(renderer_id: impl Into<String>, producer_generation: u64) -> Self {
        Self::starting_at(renderer_id, producer_generation, Instant::now())
    }

    #[must_use]
    pub fn starting_at(
        renderer_id: impl Into<String>,
        producer_generation: u64,
        started_at: Instant,
    ) -> Self {
        Self::Starting {
            renderer_id: renderer_id.into(),
            producer_generation,
            started_at,
        }
    }

    pub fn on_stream_started(&mut self) -> PlaybackAction {
        if let Self::Starting {
            renderer_id,
            producer_generation,
            ..
        } = self
        {
            let renderer_id = renderer_id.clone();
            let producer_generation = *producer_generation;
            *self = Self::Streaming {
                renderer_id,
                auto_calibration_started: true,
                producer_generation,
            };
            PlaybackAction::StartCalibration
        } else {
            PlaybackAction::None
        }
    }

    #[must_use]
    pub fn renderer_id(&self) -> Option<&str> {
        match self {
            Self::Idle => None,
            Self::Starting { renderer_id, .. }
            | Self::Streaming { renderer_id, .. }
            | Self::Calibrating { renderer_id, .. } => Some(renderer_id),
            Self::Failed { renderer_id, .. } => renderer_id.as_deref(),
        }
    }

    #[must_use]
    pub fn on_producer_disconnected(&mut self, generation: u64) -> bool {
        let matches = match self {
            Self::Starting {
                producer_generation,
                ..
            }
            | Self::Streaming {
                producer_generation,
                ..
            }
            | Self::Calibrating {
                producer_generation,
                ..
            } => *producer_generation == generation,
            Self::Idle | Self::Failed { .. } => false,
        };
        if matches {
            *self = Self::Idle;
        }
        matches
    }

    pub fn on_start_timeout(&mut self, now: Instant, timeout: Duration) -> bool {
        let Self::Starting {
            renderer_id,
            started_at,
            ..
        } = self
        else {
            return false;
        };
        if now.saturating_duration_since(*started_at) < timeout {
            return false;
        }
        *self = Self::Failed {
            renderer_id: Some(renderer_id.clone()),
            message: "renderer did not request the audio stream in time".to_string(),
        };
        true
    }

    #[must_use]
    pub const fn phase(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Starting { .. } => "starting",
            Self::Streaming { .. } => "streaming",
            Self::Calibrating { .. } => "calibrating",
            Self::Failed { .. } => "failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{
        BrowserPlaybackState, BrowserStreamEndDebouncer, PlaybackAction, renderer_list_event,
        should_accept_stream_feedback,
    };
    use crate::browser::protocol::{RendererSummary, ServerEvent};
    use crate::enums::streaming::StreamingState;

    #[test]
    fn stream_started_arms_auto_calibration_once() {
        let mut state = BrowserPlaybackState::starting("renderer-1", 7);

        assert_eq!(state.on_stream_started(), PlaybackAction::StartCalibration);
        assert_eq!(state.on_stream_started(), PlaybackAction::None);
    }

    #[test]
    fn only_the_owning_producer_disconnect_stops_playback() {
        let mut state = BrowserPlaybackState::starting("renderer-1", 8);

        assert!(!state.on_producer_disconnected(7));
        assert_eq!(state.renderer_id(), Some("renderer-1"));
        assert!(state.on_producer_disconnected(8));
        assert_eq!(state, BrowserPlaybackState::Idle);
    }

    #[test]
    fn starting_state_times_out_once_after_the_deadline() {
        let started_at = Instant::now();
        let mut state = BrowserPlaybackState::starting_at("renderer-1", 8, started_at);

        assert!(
            !state.on_start_timeout(started_at + Duration::from_secs(9), Duration::from_secs(10))
        );
        assert!(state.on_start_timeout(
            started_at + Duration::from_secs(10),
            Duration::from_secs(10)
        ));
        assert_eq!(state.phase(), "failed");
        assert!(!state.on_start_timeout(
            started_at + Duration::from_secs(11),
            Duration::from_secs(10)
        ));
    }

    #[test]
    fn stream_feedback_must_belong_to_the_selected_renderer() {
        assert!(!should_accept_stream_feedback(
            Some("192.0.2.10"),
            "192.0.2.11",
            StreamingState::Started,
            true,
        ));
        assert!(!should_accept_stream_feedback(
            Some("192.0.2.10"),
            "192.0.2.10",
            StreamingState::Ended,
            true,
        ));
        assert!(should_accept_stream_feedback(
            Some("192.0.2.10"),
            "192.0.2.10",
            StreamingState::Ended,
            false,
        ));
    }

    #[test]
    fn transient_stream_end_waits_for_reconnect_before_finalizing() {
        let now = Instant::now();
        let grace = Duration::from_secs(2);
        let mut debounce = BrowserStreamEndDebouncer::default();
        debounce.note_ended("192.0.2.10", now, grace);

        assert_eq!(
            debounce.take_due(now + Duration::from_secs(1), Some("192.0.2.10"), false),
            None,
        );
        assert_eq!(
            debounce.take_due(now + grace, Some("192.0.2.10"), true),
            None,
        );

        debounce.note_ended("192.0.2.10", now, grace);
        assert_eq!(
            debounce.take_due(now + grace, Some("192.0.2.10"), false),
            Some("192.0.2.10".to_string()),
        );

        debounce.note_ended("192.0.2.10", now, grace);
        debounce.note_started("192.0.2.10");
        assert_eq!(
            debounce.take_due(now + grace, Some("192.0.2.10"), false),
            None,
        );
    }

    #[test]
    fn renderer_discovery_builds_an_unsolicited_list_update() {
        let renderer = RendererSummary {
            id: "renderer-1".to_string(),
            name: "Living room".to_string(),
            model: "Sonos One".to_string(),
            address: "192.0.2.10".to_string(),
        };

        assert_eq!(
            renderer_list_event("renderer-update", vec![renderer.clone()]),
            ServerEvent::RendererList {
                request_id: "renderer-update".to_string(),
                renderers: vec![renderer],
            }
        );
    }
}
