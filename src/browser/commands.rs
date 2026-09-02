use crate::browser::protocol::{ClientMessage, ServerEvent};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BrowserCommand {
    pub request_id: String,
    pub kind: BrowserCommandKind,
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
    pub fn from_client_message(message: ClientMessage) -> Option<Self> {
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
        Some(Self { request_id, kind })
    }
}

pub type BrowserEvent = ServerEvent;

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum BrowserPlaybackState {
    Idle,
    Starting {
        renderer_id: String,
    },
    Streaming {
        renderer_id: String,
        auto_calibration_started: bool,
    },
    Calibrating {
        renderer_id: String,
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
    pub fn starting(renderer_id: impl Into<String>) -> Self {
        Self::Starting {
            renderer_id: renderer_id.into(),
        }
    }

    pub fn on_stream_started(&mut self) -> PlaybackAction {
        if let Self::Starting { renderer_id } = self {
            let renderer_id = renderer_id.clone();
            *self = Self::Streaming {
                renderer_id,
                auto_calibration_started: true,
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
            Self::Starting { renderer_id }
            | Self::Streaming { renderer_id, .. }
            | Self::Calibrating { renderer_id } => Some(renderer_id),
            Self::Failed { renderer_id, .. } => renderer_id.as_deref(),
        }
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
    use super::{BrowserPlaybackState, PlaybackAction};

    #[test]
    fn stream_started_arms_auto_calibration_once() {
        let mut state = BrowserPlaybackState::starting("renderer-1");

        assert_eq!(state.on_stream_started(), PlaybackAction::StartCalibration);
        assert_eq!(state.on_stream_started(), PlaybackAction::None);
    }
}
