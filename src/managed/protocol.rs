use serde::{Deserialize, Serialize};

use crate::browser::protocol::RendererSummary;

/// JavaScript clients must be able to compare these counters exactly.
pub const MAX_COUNTER: u64 = (1 << 53) - 1;
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_ID_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Chrome,
    Windows,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Idle,
    Starting,
    Streaming,
    Switching,
    Failed,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Controller,
    Browser,
}

/// Supplied by the authenticated transport, never deserialized from a command.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Caller {
    pub connection_id: String,
    pub role: Role,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Action {
    SelectSource {
        source: Source,
    },
    SelectRenderer {
        renderer_id: String,
    },
    StartWindows,
    ReclaimSpeakers,
    PrepareBrowser,
    StartBrowser {
        lease_id: String,
    },
    Stop,
    SetVolume {
        value: i32,
    },
    Calibrate,
    SetOffset {
        milliseconds: i32,
    },
    GetState,
    PrepareSettings,
    ReleaseSettings,
    CaptureReleased {
        transition_id: String,
    },
    ReloadTab,
    StopForReload {
        action_id: String,
    },
    BrowserStatus {
        shortcut: String,
        supported_tab: bool,
        has_recovery_tab: bool,
        #[serde(default = "legacy_video_sync")]
        video_sync_supported: bool,
    },
    BrowserActionResult {
        action_id: String,
        status: Status,
        code: String,
    },
}

const fn legacy_video_sync() -> bool {
    true
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename = "command",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub struct Command {
    pub request_id: String,
    pub instance_id: String,
    pub expected_revision: u64,
    pub action: Action,
}

// serde's struct `tag` adds an outgoing tag but does not consume it on input.
// An enum envelope validates and consumes the incoming discriminant strictly.
impl<'de> Deserialize<'de> for Command {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(
            tag = "type",
            rename_all = "kebab-case",
            rename_all_fields = "camelCase",
            deny_unknown_fields
        )]
        enum Envelope {
            Command {
                request_id: String,
                instance_id: String,
                expected_revision: u64,
                action: Action,
            },
        }
        let Envelope::Command {
            request_id,
            instance_id,
            expected_revision,
            action,
        } = Envelope::deserialize(deserializer)?;
        Ok(Self {
            request_id,
            instance_id,
            expected_revision,
            action,
        })
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Rejected,
    Failed,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename = "result",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub struct CommandResult {
    pub request_id: String,
    pub status: Status,
    pub code: String,
    pub revision: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub lease_id: Option<String>,
}

impl CommandResult {
    #[must_use]
    pub fn rejected(command: &Command, code: &str, revision: u64) -> Self {
        Self {
            request_id: command.request_id.clone(),
            status: Status::Rejected,
            code: code.into(),
            revision,
            lease_id: None,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capabilities {
    pub windows_available: bool,
    pub browser_connected: bool,
    pub browser_can_cast: bool,
    pub browser_can_reload: bool,
    pub calibration_available: bool,
    pub shortcut: String,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            windows_available: true,
            browser_connected: false,
            browser_can_cast: false,
            browser_can_reload: false,
            calibration_available: false,
            shortcut: String::new(),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename = "state",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub struct Snapshot {
    pub instance_id: String,
    pub revision: u64,
    pub generation: u64,
    pub source: Source,
    pub phase: Phase,
    pub renderer_id: Option<String>,
    pub renderers: Vec<RendererSummary>,
    pub volume: Option<u8>,
    pub measured_delay_ms: u32,
    pub manual_offset_ms: i32,
    pub error_code: Option<String>,
    pub capabilities: Capabilities,
}

pub fn parse_command(text: &str) -> Result<Command, &'static str> {
    if text.len() > MAX_TEXT_BYTES {
        return Err("message-too-large");
    }
    let command: Command = serde_json::from_str(text).map_err(|_| "invalid-command")?;
    command.validate()?;
    Ok(command)
}

impl Command {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !valid_id(&self.request_id) || !valid_id(&self.instance_id) {
            return Err("invalid-id");
        }
        if self.expected_revision == 0 || self.expected_revision > MAX_COUNTER {
            return Err("invalid-revision");
        }
        match &self.action {
            Action::SetVolume { value } if !(0..=100).contains(value) => Err("invalid-volume"),
            Action::SetOffset { milliseconds } if !(-5000..=5000).contains(milliseconds) => {
                Err("invalid-offset")
            }
            Action::SelectRenderer { renderer_id } if !valid_id(renderer_id) => {
                Err("invalid-renderer")
            }
            Action::StartBrowser { lease_id } if !valid_id(lease_id) => Err("invalid-lease"),
            Action::CaptureReleased { transition_id } if !valid_id(transition_id) => {
                Err("invalid-transition")
            }
            Action::BrowserStatus { shortcut, .. }
                if shortcut.len() > 64 || shortcut.chars().any(char::is_control) =>
            {
                Err("invalid-shortcut")
            }
            Action::StopForReload { action_id } if !valid_id(action_id) => {
                Err("invalid-browser-action")
            }
            Action::BrowserActionResult {
                action_id, code, ..
            } if !valid_id(action_id) || code.len() > 128 || code.chars().any(char::is_control) => {
                Err("invalid-browser-result")
            }
            _ => Ok(()),
        }
    }
}

#[must_use]
pub fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_ID_BYTES && !value.chars().any(char::is_control)
}
