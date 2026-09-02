use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::browser::auth::PairingToken;

pub const PROTOCOL_VERSION: u16 = 1;
pub const FRAMES_PER_PACKET: u32 = 960;
pub const MAX_REQUEST_ID_BYTES: usize = 128;
pub const MAX_MANUAL_OFFSET_MS: i32 = 5_000;
pub const MAX_AUDIO_PACKET_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SampleFormat {
    S16Le,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioHello {
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: SampleFormat,
    pub frames_per_packet: u32,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Hello {
    pub version: u16,
    pub token: String,
    pub audio: AudioHello,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSession {
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: SampleFormat,
    pub frames_per_packet: u32,
    pub packet_bytes: usize,
    pub max_packet_bytes: usize,
}

impl AudioSession {
    #[must_use]
    pub const fn v1() -> Self {
        let packet_bytes = FRAMES_PER_PACKET as usize * 2 * size_of::<i16>();
        Self {
            sample_rate: 48_000,
            channels: 2,
            sample_format: SampleFormat::S16Le,
            frames_per_packet: FRAMES_PER_PACKET,
            packet_bytes,
            max_packet_bytes: MAX_AUDIO_PACKET_BYTES,
        }
    }

    #[must_use]
    pub const fn hello(self) -> AudioHello {
        AudioHello {
            sample_rate: self.sample_rate,
            channels: self.channels,
            sample_format: self.sample_format,
            frames_per_packet: self.frames_per_packet,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ClientMessage {
    Hello(Hello),
    ListRenderers {
        request_id: String,
    },
    Cast {
        request_id: String,
        renderer_id: String,
    },
    Stop {
        request_id: String,
    },
    Calibrate {
        request_id: String,
    },
    SetManualOffset {
        request_id: String,
        milliseconds: i32,
    },
    GetState {
        request_id: String,
    },
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct RendererSummary {
    pub id: String,
    pub name: String,
    pub model: String,
    pub address: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ServerEvent {
    HelloOk {
        audio: AudioSession,
    },
    RendererList {
        request_id: String,
        renderers: Vec<RendererSummary>,
    },
    CastState {
        request_id: Option<String>,
        renderer_id: Option<String>,
        state: String,
    },
    StreamState {
        renderer_id: Option<String>,
        state: String,
    },
    CalibrationStarted {
        request_id: String,
    },
    CalibrationResult {
        request_id: String,
        renderer_id: String,
        milliseconds: u32,
    },
    CalibrationFailed {
        request_id: String,
        code: String,
        message: String,
    },
    AudioStats {
        received_packets: u64,
        dropped_packets: u64,
    },
    ManualOffsetSet {
        request_id: String,
        milliseconds: i32,
    },
    Warning {
        request_id: Option<String>,
        code: String,
        message: String,
    },
    Error {
        request_id: Option<String>,
        code: String,
        message: String,
        terminal: bool,
    },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ProtocolError {
    InvalidJson,
    UnsupportedVersion,
    AuthenticationFailed,
    UnsupportedAudio,
    InvalidRequestId,
    ManualOffsetOutOfRange,
}

impl ProtocolError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidJson => "invalid-json",
            Self::UnsupportedVersion => "unsupported-version",
            Self::AuthenticationFailed => "authentication-failed",
            Self::UnsupportedAudio => "unsupported-audio",
            Self::InvalidRequestId => "invalid-request-id",
            Self::ManualOffsetOutOfRange => "manual-offset-out-of-range",
        }
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl Error for ProtocolError {}

pub fn parse_client_message(text: &str) -> Result<ClientMessage, ProtocolError> {
    let message: ClientMessage =
        serde_json::from_str(text).map_err(|_| ProtocolError::InvalidJson)?;

    let request_id = match &message {
        ClientMessage::Hello(_) => None,
        ClientMessage::ListRenderers { request_id }
        | ClientMessage::Cast { request_id, .. }
        | ClientMessage::Stop { request_id }
        | ClientMessage::Calibrate { request_id }
        | ClientMessage::SetManualOffset { request_id, .. }
        | ClientMessage::GetState { request_id } => Some(request_id),
    };
    if request_id.is_some_and(|id| id.is_empty() || id.len() > MAX_REQUEST_ID_BYTES) {
        return Err(ProtocolError::InvalidRequestId);
    }

    if let ClientMessage::SetManualOffset { milliseconds, .. } = &message
        && !(-MAX_MANUAL_OFFSET_MS..=MAX_MANUAL_OFFSET_MS).contains(milliseconds)
    {
        return Err(ProtocolError::ManualOffsetOutOfRange);
    }

    Ok(message)
}

pub fn validate_hello(hello: &Hello, token: &PairingToken) -> Result<AudioSession, ProtocolError> {
    if hello.version != PROTOCOL_VERSION {
        return Err(ProtocolError::UnsupportedVersion);
    }
    if !token.matches_hex(&hello.token) {
        return Err(ProtocolError::AuthenticationFailed);
    }

    let session = AudioSession::v1();
    if hello.audio != session.hello() {
        return Err(ProtocolError::UnsupportedAudio);
    }
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::{
        AudioHello, ClientMessage, Hello, ProtocolError, RendererSummary, SampleFormat,
        parse_client_message, validate_hello,
    };
    use crate::browser::auth::PairingToken;

    #[test]
    fn hello_requires_exact_v1_audio_contract() {
        let token = PairingToken::from_bytes([7; 32]);
        let hello = Hello {
            version: 1,
            token: token.to_hex(),
            audio: AudioHello {
                sample_rate: 48_000,
                channels: 2,
                sample_format: SampleFormat::S16Le,
                frames_per_packet: 960,
            },
        };

        assert_eq!(validate_hello(&hello, &token).unwrap().packet_bytes, 3840);
    }

    #[test]
    fn hello_rejects_wrong_version_audio_or_token() {
        let token = PairingToken::from_bytes([7; 32]);
        let valid = Hello {
            version: 1,
            token: token.to_hex(),
            audio: AudioHello {
                sample_rate: 48_000,
                channels: 2,
                sample_format: SampleFormat::S16Le,
                frames_per_packet: 960,
            },
        };

        let mut wrong_version = valid.clone();
        wrong_version.version = 2;
        assert_eq!(
            validate_hello(&wrong_version, &token),
            Err(ProtocolError::UnsupportedVersion)
        );

        let mut wrong_audio = valid.clone();
        wrong_audio.audio.channels = 1;
        assert_eq!(
            validate_hello(&wrong_audio, &token),
            Err(ProtocolError::UnsupportedAudio)
        );

        let mut wrong_token = valid;
        wrong_token.token = "00".repeat(32);
        assert_eq!(
            validate_hello(&wrong_token, &token),
            Err(ProtocolError::AuthenticationFailed)
        );
    }

    #[test]
    fn parses_kebab_case_command_with_camel_case_fields() {
        let parsed = parse_client_message(
            r#"{"type":"set-manual-offset","requestId":"offset-1","milliseconds":-250}"#,
        )
        .expect("manual offset command");

        assert_eq!(
            parsed,
            ClientMessage::SetManualOffset {
                request_id: "offset-1".to_string(),
                milliseconds: -250,
            }
        );
    }

    #[test]
    fn rejects_empty_or_oversized_request_ids() {
        let empty = r#"{"type":"get-state","requestId":""}"#;
        assert_eq!(
            parse_client_message(empty),
            Err(ProtocolError::InvalidRequestId)
        );

        let oversized = format!(
            r#"{{"type":"get-state","requestId":"{}"}}"#,
            "x".repeat(129)
        );
        assert_eq!(
            parse_client_message(&oversized),
            Err(ProtocolError::InvalidRequestId)
        );
    }

    #[test]
    fn rejects_manual_offset_outside_five_seconds() {
        for milliseconds in [i32::MIN, -5001, 5001, i32::MAX] {
            let json = format!(
                r#"{{"type":"set-manual-offset","requestId":"offset","milliseconds":{milliseconds}}}"#
            );
            assert_eq!(
                parse_client_message(&json),
                Err(ProtocolError::ManualOffsetOutOfRange)
            );
        }
    }

    #[test]
    fn renderer_summary_serializes_only_public_protocol_fields() {
        let summary = RendererSummary {
            id: "uuid:living-room".to_string(),
            name: "Living Room".to_string(),
            model: "Sonos One".to_string(),
            address: "192.0.2.10".to_string(),
        };

        assert_eq!(
            serde_json::to_value(summary).expect("renderer JSON"),
            serde_json::json!({
                "id": "uuid:living-room",
                "name": "Living Room",
                "model": "Sonos One",
                "address": "192.0.2.10"
            })
        );
    }
}
