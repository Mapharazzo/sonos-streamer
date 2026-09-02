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
