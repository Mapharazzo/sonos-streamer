use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, Instant};

use super::protocol::{
    Action, Caller, Capabilities, Command, CommandResult, MAX_COUNTER, Phase, Role, Snapshot,
    Source, Status,
};
use crate::browser::protocol::RendererSummary;

const REQUESTS_PER_CONTROLLER: usize = 256;
const MAX_CONTROLLERS: usize = 16;

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Effect {
    ReclaimSpeakers {
        generation: u64,
    },
    StopPlayback {
        generation: u64,
    },
    ReleaseBrowser {
        owner: String,
        transition_id: String,
    },
    OpenWindowsCapture {
        generation: u64,
    },
    PrepareBrowser {
        owner: String,
        generation: u64,
    },
    StartBrowser {
        generation: u64,
        lease_id: String,
    },
    SetVolume {
        renderer_id: String,
        value: u8,
    },
    Calibrate {
        generation: u64,
    },
    PersistPreferences,
    ReloadTab {
        owner: String,
        action_id: String,
    },
}

#[derive(Debug, Clone)]
pub enum Event {
    OperationFailed {
        generation: u64,
        code: &'static str,
    },
    Renderers(Vec<RendererSummary>),
    BrowserConnected {
        owner: String,
    },
    BrowserDisconnected {
        owner: String,
    },
    ProducerAttached {
        owner: String,
        generation: u64,
        lease_id: String,
    },
    ProducerDisconnected {
        owner: String,
        generation: u64,
    },
    StreamStarted {
        generation: u64,
    },
    PlaybackFailed {
        generation: u64,
        code: String,
    },
    TransitionTimedOut {
        transition_id: String,
    },
    Volume {
        renderer_id: String,
        value: u8,
    },
    Calibration {
        generation: u64,
        milliseconds: u32,
    },
}

struct Transition {
    id: String,
    owner: String,
    destination: Source,
}

struct BrowserStatus {
    shortcut: String,
    can_cast: bool,
    can_reload: bool,
    video_sync: bool,
}
impl Default for BrowserStatus {
    fn default() -> Self {
        Self {
            shortcut: String::new(),
            can_cast: false,
            can_reload: false,
            video_sync: true,
        }
    }
}

/// The sole playback authority. I/O occurs only through the returned effects.
pub struct Session {
    snapshot: Snapshot,
    browser_owner: Option<String>,
    browsers: BTreeMap<String, BrowserStatus>,
    reload: Option<(String, String, u64)>,
    capture_owner: Option<String>,
    producer_lease: Option<String>,
    play_requested: bool,
    transition: Option<Transition>,
    settings_owner: Option<String>,
    settings_deadline: Option<Instant>,
    cache: HashMap<String, VecDeque<(Command, CommandResult)>>,
}

impl Session {
    #[must_use]
    pub fn new(instance_id: String) -> Self {
        Self {
            snapshot: Snapshot {
                instance_id,
                revision: 1,
                generation: 1,
                source: Source::Chrome,
                phase: Phase::Idle,
                renderer_id: None,
                renderers: Vec::new(),
                volume: None,
                measured_delay_ms: 0,
                manual_offset_ms: 0,
                error_code: None,
                capabilities: Capabilities::default(),
            },
            browser_owner: None,
            browsers: BTreeMap::new(),
            reload: None,
            capture_owner: None,
            producer_lease: None,
            play_requested: false,
            transition: None,
            settings_owner: None,
            settings_deadline: None,
            cache: HashMap::new(),
        }
    }

    #[must_use]
    pub const fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub(crate) fn reserve_reload(&mut self, owner: String, action_id: String) {
        self.reload = Some((owner, action_id, self.snapshot.generation));
    }
    pub(crate) fn finish_reload(&mut self, action_id: &str) {
        if self
            .reload
            .as_ref()
            .is_some_and(|(_, id, _)| id == action_id)
        {
            self.reload = None;
        }
    }

    pub fn forget_controller(&mut self, connection_id: &str) {
        self.cache.remove(connection_id);
        // A lost controller may still be committing settings. Keep the short
        // reservation until explicit release, engine restart, or expiry.
    }
    pub fn cached_result(&self, caller: &Caller, command: &Command) -> Option<CommandResult> {
        self.cache
            .get(&caller.connection_id)?
            .iter()
            .find(|(previous, _)| previous == command)
            .map(|(_, result)| result.clone())
    }

    pub fn restore_preferences(&mut self, preferences: &super::preferences::Preferences) {
        self.snapshot.source = preferences.source;
        self.snapshot
            .renderer_id
            .clone_from(&preferences.renderer_id);
        self.snapshot.manual_offset_ms = preferences.manual_offset_ms;
    }

    /// Runtime results (notably the private lease) must be replayed exactly.
    pub fn finish_result(&mut self, caller: &Caller, result: &CommandResult) {
        if let Some(cache) = self.cache.get_mut(&caller.connection_id)
            && let Some((_, cached)) = cache
                .iter_mut()
                .find(|(command, _)| command.request_id == result.request_id)
        {
            *cached = result.clone();
        }
    }

    pub fn apply(&mut self, caller: &Caller, command: Command) -> (CommandResult, Vec<Effect>) {
        if let Some(cache) = self.cache.get(&caller.connection_id)
            && let Some((previous, result)) = cache
                .iter()
                .find(|(old, _)| old.request_id == command.request_id)
        {
            return if previous == &command {
                (result.clone(), vec![])
            } else {
                (
                    CommandResult::rejected(
                        &command,
                        "request-id-conflict",
                        self.snapshot.revision,
                    ),
                    vec![],
                )
            };
        }
        if !self.cache.contains_key(&caller.connection_id) && self.cache.len() >= MAX_CONTROLLERS {
            return (
                CommandResult::rejected(&command, "controller-limit", self.snapshot.revision),
                vec![],
            );
        }
        let before = self.snapshot.clone();
        let outcome = self
            .check_command(&command)
            .and_then(|()| self.dispatch(caller, &command));
        let (status, code, effects) = match outcome {
            Ok(effects) => {
                if self.snapshot != before {
                    self.bump_revision();
                }
                (Status::Ok, String::new(), effects)
            }
            Err(code) => (Status::Rejected, code.into(), vec![]),
        };
        let result = CommandResult {
            request_id: command.request_id.clone(),
            status,
            code,
            revision: self.snapshot.revision,
            lease_id: None,
        };
        let cache = self.cache.entry(caller.connection_id.clone()).or_default();
        if cache.len() == REQUESTS_PER_CONTROLLER {
            cache.pop_front();
        }
        cache.push_back((command, result.clone()));
        (result, effects)
    }

    fn check_command(&self, command: &Command) -> Result<(), &'static str> {
        command.validate()?;
        if command.instance_id != self.snapshot.instance_id {
            return Err("stale-instance");
        }
        // An acknowledgment is scoped by its transition, not a changing display revision.
        let revision_independent = matches!(
            command.action,
            Action::GetState
                | Action::ReleaseSettings
                | Action::CaptureReleased { .. }
                | Action::BrowserActionResult { .. }
        );
        if !revision_independent && command.expected_revision != self.snapshot.revision {
            return Err("stale-revision");
        }
        if !matches!(command.action, Action::GetState)
            && (self.snapshot.revision >= MAX_COUNTER || self.snapshot.generation >= MAX_COUNTER)
        {
            return Err("counter-exhausted");
        }
        Ok(())
    }

    fn require_browser(&self, caller: &Caller) -> Result<(), &'static str> {
        if caller.role != Role::Browser || !self.browsers.contains_key(&caller.connection_id) {
            return Err("not-browser-owner");
        }
        Ok(())
    }

    fn require_renderer(&self) -> Result<String, &'static str> {
        let id = self
            .snapshot
            .renderer_id
            .as_ref()
            .ok_or("select-renderer")?;
        if !self
            .snapshot
            .renderers
            .iter()
            .any(|renderer| &renderer.id == id)
        {
            return Err("renderer-unavailable");
        }
        Ok(id.clone())
    }

    fn dispatch(
        &mut self,
        caller: &Caller,
        command: &Command,
    ) -> Result<Vec<Effect>, &'static str> {
        if self
            .settings_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.settings_owner = None;
            self.settings_deadline = None;
        }
        if self.settings_owner.is_some()
            && !matches!(
                command.action,
                Action::GetState
                    | Action::ReleaseSettings
                    | Action::BrowserStatus { .. }
                    | Action::CaptureReleased { .. }
                    | Action::BrowserActionResult { .. }
            )
        {
            return Err("settings-in-progress");
        }
        match &command.action {
            Action::PrepareBrowser
            | Action::StartBrowser { .. }
            | Action::StartWindows
            | Action::ReclaimSpeakers
            | Action::ReloadTab
                if self.reload.is_some() =>
            {
                Err("reload-in-progress")
            }
            Action::PrepareSettings => {
                if caller.role != Role::Controller {
                    return Err("controller-required");
                }
                if !matches!(self.snapshot.phase, Phase::Idle | Phase::Failed)
                    || self.capture_owner.is_some()
                    || self.transition.is_some()
                {
                    return Err("stop-playback-first");
                }
                self.settings_owner = Some(caller.connection_id.clone());
                self.settings_deadline = Some(Instant::now() + Duration::from_secs(60));
                Ok(vec![])
            }
            Action::ReleaseSettings => {
                if self.settings_owner.as_deref() != Some(caller.connection_id.as_str()) {
                    return Err("not-settings-owner");
                }
                self.settings_owner = None;
                self.settings_deadline = None;
                Ok(vec![])
            }
            Action::GetState => Ok(vec![]),
            Action::ReclaimSpeakers => {
                self.require_renderer()?;
                if self.snapshot.phase == Phase::Streaming {
                    self.snapshot.phase = Phase::Starting;
                    self.snapshot.error_code = None;
                    Ok(vec![Effect::ReclaimSpeakers {
                        generation: self.snapshot.generation,
                    }])
                } else if matches!(self.snapshot.phase, Phase::Idle | Phase::Failed) {
                    if self.snapshot.source != Source::Windows {
                        return Err("browser-capture-required");
                    }
                    let mut start = command.clone();
                    start.action = Action::StartWindows;
                    self.dispatch(caller, &start)
                } else {
                    Err("playback-busy")
                }
            }
            Action::SelectRenderer { renderer_id } => {
                if !matches!(self.snapshot.phase, Phase::Idle | Phase::Failed)
                    || self.capture_owner.is_some()
                {
                    return Err("stop-before-changing-room");
                }
                if !self
                    .snapshot
                    .renderers
                    .iter()
                    .any(|room| &room.id == renderer_id)
                {
                    return Err("renderer-unavailable");
                }
                self.snapshot.renderer_id = Some(renderer_id.clone());
                self.snapshot.volume = None;
                self.snapshot.measured_delay_ms = 0;
                self.snapshot.error_code = None;
                self.snapshot.phase = Phase::Idle;
                Ok(vec![Effect::PersistPreferences])
            }
            Action::SelectSource { source } => {
                if self.transition.is_some() {
                    return Err("transition-in-progress");
                }
                if self.snapshot.source == *source {
                    return Ok(vec![]);
                }
                Ok(self.stop_and_select(*source))
            }
            Action::Stop => {
                if self.snapshot.phase == Phase::Idle && self.capture_owner.is_none() {
                    return Ok(vec![]);
                }
                Ok(self.stop_and_select(self.snapshot.source))
            }
            Action::StartWindows => {
                if self.snapshot.source != Source::Windows {
                    return Err("select-windows-source");
                }
                self.require_idle()?;
                self.require_renderer()?;
                self.new_generation();
                self.play_requested = true;
                self.snapshot.phase = Phase::Starting;
                self.snapshot.error_code = None;
                Ok(vec![Effect::OpenWindowsCapture {
                    generation: self.snapshot.generation,
                }])
            }
            Action::StopForReload { action_id } => {
                self.require_browser(caller)?;
                if !self.reload.as_ref().is_some_and(|(owner, id, generation)| {
                    owner == &caller.connection_id
                        && id == action_id
                        && *generation == self.snapshot.generation
                }) {
                    return Err("stale-browser-action");
                }
                if self.snapshot.phase == Phase::Idle && self.capture_owner.is_none() {
                    return Ok(vec![]);
                }
                Ok(self.stop_and_select(self.snapshot.source))
            }
            Action::PrepareBrowser => {
                self.require_browser(caller)?;
                if self.snapshot.source != Source::Chrome {
                    return Err("select-chrome-source");
                }
                self.require_idle()?;
                self.require_renderer()?;
                self.new_generation();
                self.capture_owner = Some(caller.connection_id.clone());
                self.browser_owner = Some(caller.connection_id.clone());
                self.refresh_browser_capabilities();
                self.snapshot.phase = Phase::Starting;
                self.snapshot.error_code = None;
                Ok(vec![Effect::PrepareBrowser {
                    owner: caller.connection_id.clone(),
                    generation: self.snapshot.generation,
                }])
            }
            Action::StartBrowser { lease_id } => {
                self.require_browser(caller)?;
                if self.snapshot.source != Source::Chrome
                    || self.snapshot.phase != Phase::Starting
                    || self.play_requested
                    || self.capture_owner.as_deref() != Some(&caller.connection_id)
                    || self.producer_lease.as_ref() != Some(lease_id)
                {
                    return Err("producer-not-ready");
                }
                self.require_renderer()?;
                self.play_requested = true;
                Ok(vec![Effect::StartBrowser {
                    generation: self.snapshot.generation,
                    lease_id: lease_id.clone(),
                }])
            }
            Action::CaptureReleased { transition_id } => {
                self.require_browser(caller)?;
                let transition = self.transition.as_ref().ok_or("stale-transition")?;
                if &transition.id != transition_id || transition.owner != caller.connection_id {
                    return Err("stale-transition");
                }
                let destination = transition.destination;
                self.transition = None;
                self.capture_owner = None;
                self.producer_lease = None;
                self.snapshot.source = destination;
                self.snapshot.phase = Phase::Idle;
                self.snapshot.error_code = None;
                Ok(vec![Effect::PersistPreferences])
            }
            Action::SetVolume { value } => {
                let renderer_id = self.require_renderer()?;
                // Publish the confirmed volume only after the renderer reports it.
                Ok(vec![Effect::SetVolume {
                    renderer_id,
                    value: u8::try_from(*value).map_err(|_| "invalid-volume")?,
                }])
            }
            Action::Calibrate => {
                if self.snapshot.source != Source::Chrome
                    || self.snapshot.phase != Phase::Streaming
                    || !self.snapshot.capabilities.calibration_available
                {
                    return Err("chrome-stream-required");
                }
                Ok(vec![Effect::Calibrate {
                    generation: self.snapshot.generation,
                }])
            }
            Action::SetOffset { milliseconds } => {
                if self.snapshot.source != Source::Chrome {
                    return Err("chrome-source-required");
                }
                self.snapshot.manual_offset_ms = *milliseconds;
                Ok(vec![Effect::PersistPreferences])
            }
            Action::BrowserStatus {
                shortcut,
                supported_tab,
                has_recovery_tab,
                video_sync_supported,
            } => {
                self.require_browser(caller)?;
                self.browsers.insert(
                    caller.connection_id.clone(),
                    BrowserStatus {
                        shortcut: shortcut.clone(),
                        can_cast: *supported_tab,
                        can_reload: *has_recovery_tab,
                        video_sync: *video_sync_supported,
                    },
                );
                self.refresh_browser_capabilities();
                Ok(vec![])
            }
            Action::ReloadTab => {
                let owner = self.browser_owner.clone().ok_or("browser-unavailable")?;
                if !self.snapshot.capabilities.browser_can_reload {
                    return Err("no-recovery-tab");
                }
                Ok(vec![Effect::ReloadTab {
                    owner,
                    action_id: command.request_id.clone(),
                }])
            }
            Action::BrowserActionResult { .. } => {
                self.require_browser(caller)?;
                // The runtime correlates this result to a pending browser action.
                Ok(vec![])
            }
        }
    }

    fn require_idle(&self) -> Result<(), &'static str> {
        if self.transition.is_some() {
            return Err("transition-in-progress");
        }
        if self.capture_owner.is_some() {
            return Err("browser-cleanup-required");
        }
        if !matches!(self.snapshot.phase, Phase::Idle | Phase::Failed) {
            return Err("already-playing");
        }
        Ok(())
    }

    fn refresh_browser_capabilities(&mut self) {
        let status = self
            .browser_owner
            .as_ref()
            .and_then(|owner| self.browsers.get(owner));
        self.snapshot.capabilities.browser_connected = !self.browsers.is_empty();
        self.snapshot.capabilities.browser_can_cast = status.is_some_and(|s| s.can_cast);
        self.snapshot.capabilities.browser_can_reload = status.is_some_and(|s| s.can_reload);
        self.snapshot.capabilities.shortcut =
            status.map_or_else(String::new, |s| s.shortcut.clone());
        self.snapshot.capabilities.calibration_available = self.snapshot.source == Source::Chrome
            && self.snapshot.phase == Phase::Streaming
            && status.is_some_and(|s| s.video_sync);
    }

    fn stop_and_select(&mut self, destination: Source) -> Vec<Effect> {
        let old_generation = self.snapshot.generation;
        self.new_generation();
        self.producer_lease = None;
        self.snapshot.error_code = None;
        let mut effects = vec![Effect::StopPlayback {
            generation: old_generation,
        }];
        if let Some(owner) = self.capture_owner.clone() {
            let id = format!("transition-{}", self.snapshot.generation);
            self.transition = Some(Transition {
                id: id.clone(),
                owner: owner.clone(),
                destination,
            });
            self.snapshot.phase = Phase::Switching;
            effects.push(Effect::ReleaseBrowser {
                owner,
                transition_id: id,
            });
        } else {
            self.transition = None;
            self.snapshot.source = destination;
            self.snapshot.phase = Phase::Idle;
            effects.push(Effect::PersistPreferences);
        }
        effects
    }

    fn new_generation(&mut self) {
        self.play_requested = false;
        self.snapshot.generation = self.snapshot.generation.saturating_add(1).min(MAX_COUNTER);
    }

    fn bump_revision(&mut self) {
        self.snapshot.revision = self.snapshot.revision.saturating_add(1).min(MAX_COUNTER);
    }

    fn fail_playback(&mut self, code: &str) -> Vec<Effect> {
        let request_cleanup = self.transition.is_none()
            && self.capture_owner.is_some()
            && self
                .capture_owner
                .as_ref()
                .is_some_and(|owner| self.browsers.contains_key(owner));
        let generation = self.snapshot.generation;
        self.new_generation();
        self.transition = None;
        self.producer_lease = None;
        self.snapshot.phase = Phase::Failed;
        self.snapshot.error_code = Some(code.into());
        let mut effects = vec![Effect::StopPlayback { generation }];
        if request_cleanup {
            let owner = self.capture_owner.clone().unwrap();
            let transition_id = format!("transition-{}", self.snapshot.generation);
            self.transition = Some(Transition {
                id: transition_id.clone(),
                owner: owner.clone(),
                destination: self.snapshot.source,
            });
            effects.push(Effect::ReleaseBrowser {
                owner,
                transition_id,
            });
        }
        effects
    }

    pub fn on_event(&mut self, event: Event) -> Vec<Effect> {
        let before = self.snapshot.clone();
        let mut effects = vec![];
        match event {
            Event::OperationFailed { generation, code } => {
                if self.snapshot.generation == generation {
                    self.snapshot.error_code = Some(code.into());
                }
            }
            Event::Renderers(mut renderers) => {
                renderers.truncate(128);
                self.snapshot.renderers = renderers;
                if self.snapshot.renderer_id.is_some()
                    && self.require_renderer().is_err()
                    && matches!(self.snapshot.phase, Phase::Starting | Phase::Streaming)
                {
                    effects = self.fail_playback("renderer-unavailable");
                }
            }
            Event::BrowserConnected { owner } => {
                self.browsers.entry(owner.clone()).or_default();
                if self.browser_owner.is_none() {
                    self.browser_owner = Some(owner);
                }
                self.refresh_browser_capabilities();
            }
            Event::BrowserDisconnected { owner } => {
                self.browsers.remove(&owner);
                if self.capture_owner.as_ref() == Some(&owner) {
                    // The connection's producer lease is revoked by the bridge.
                    // Retire the generation; no remaining/reconnected browser
                    // inherits permission to capture or acknowledge its cleanup.
                    effects = self.fail_playback("browser-cleanup-required");
                    self.capture_owner = None;
                }
                if self.browser_owner.as_ref() == Some(&owner) {
                    self.browser_owner = self.browsers.keys().next().cloned();
                }
                self.refresh_browser_capabilities();
            }
            Event::ProducerAttached {
                owner,
                generation,
                lease_id,
            } => {
                if self.capture_owner.as_ref() == Some(&owner)
                    && self.snapshot.generation == generation
                    && self.snapshot.phase == Phase::Starting
                    && self.producer_lease.is_none()
                {
                    self.producer_lease = Some(lease_id);
                }
            }
            Event::ProducerDisconnected { owner, generation } => {
                if self.capture_owner.as_ref() == Some(&owner)
                    && self.snapshot.generation == generation
                {
                    effects = self.fail_playback("browser-cleanup-required");
                }
            }
            Event::StreamStarted { generation } => {
                if self.snapshot.generation == generation
                    && self.snapshot.phase == Phase::Starting
                    && self.play_requested
                {
                    self.snapshot.phase = Phase::Streaming;
                    self.snapshot.capabilities.calibration_available =
                        self.snapshot.source == Source::Chrome;
                }
            }
            Event::PlaybackFailed { generation, code } => {
                if self.snapshot.generation == generation {
                    let code = if code.len() <= 128
                        && code
                            .bytes()
                            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
                    {
                        &code
                    } else {
                        "playback-failed"
                    };
                    effects = self.fail_playback(code);
                }
            }
            Event::TransitionTimedOut { transition_id } => {
                if self
                    .transition
                    .as_ref()
                    .is_some_and(|pending| pending.id == transition_id)
                {
                    effects = self.fail_playback("browser-cleanup-required");
                }
            }
            Event::Volume { renderer_id, value } => {
                if self.snapshot.renderer_id.as_ref() == Some(&renderer_id) && value <= 100 {
                    self.snapshot.volume = Some(value);
                }
            }
            Event::Calibration {
                generation,
                milliseconds,
            } => {
                if self.snapshot.generation == generation
                    && self.snapshot.source == Source::Chrome
                    && self.snapshot.phase == Phase::Streaming
                {
                    self.snapshot.measured_delay_ms = milliseconds.min(5000);
                    // A successful calibration replaces any previous manual trial.
                    self.snapshot.manual_offset_ms = 0;
                    self.snapshot.error_code = None;
                    effects.push(Effect::PersistPreferences);
                }
            }
        }
        self.refresh_browser_capabilities();
        if self.snapshot != before {
            self.bump_revision();
        }
        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abandoned_settings_reservation_expires() {
        let mut session = Session::new("instance-1".into());
        session.settings_owner = Some("lost-controller".into());
        session.settings_deadline = Some(Instant::now() - Duration::from_secs(1));
        let request = Command {
            request_id: "after-expiry".into(),
            instance_id: "instance-1".into(),
            expected_revision: 1,
            action: Action::SelectSource {
                source: Source::Windows,
            },
        };
        let result = session
            .apply(
                &Caller {
                    connection_id: "other".into(),
                    role: Role::Controller,
                },
                request,
            )
            .0;
        assert_eq!(result.status, Status::Ok);
        assert!(session.settings_owner.is_none());
    }

    #[test]
    fn exhausted_counters_reject_mutations_instead_of_wrapping() {
        for revision in [false, true] {
            let mut session = Session::new("instance-1".into());
            if revision {
                session.snapshot.revision = MAX_COUNTER;
            } else {
                session.snapshot.generation = MAX_COUNTER;
            }
            let request = Command {
                request_id: "stop".into(),
                instance_id: "instance-1".into(),
                expected_revision: session.snapshot.revision,
                action: Action::Stop,
            };
            let (result, effects) = session.apply(
                &Caller {
                    connection_id: "test".into(),
                    role: Role::Controller,
                },
                request,
            );
            assert_eq!(result.code, "counter-exhausted");
            assert!(effects.is_empty());
        }
    }
}
