use super::{
    audio::{AudioHub, GenerationSink},
    bridge::{ManagedBridge, ManagedInput},
    config::{ManagedConfig, select_interface},
    devices::OutputDevice,
    events::{BridgeEvent, EventBus},
    leases::{LeaseRegistry, random_id},
    preferences::Preferences,
    protocol::{Action, Caller, Command, CommandResult, Phase, Snapshot, Status},
    state::{Effect, Event, Session},
    streaming::{ManagedHttp, StreamEvent, StreamEventKind},
};
use crate::{
    browser::{calibration::CalibrationCoordinator, protocol::RendererSummary},
    enums::streaming::{BitDepth, StreamingFormat},
    openhome::rendercontrol::{Renderer, StreamInfo, WavData, discover_cancellable},
};
use crossbeam_channel::{Sender, bounded};
use std::{
    io,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub trait Backend {
    fn windows_format(&mut self) -> Result<WavData, &'static str>;
    fn start_capture(&mut self, sink: GenerationSink) -> Result<(), &'static str>;
    fn stop(&mut self);
    fn play(
        &mut self,
        renderer: &str,
        generation: u64,
        url: &str,
        sample_rate: u32,
    ) -> Result<(), &'static str>;
    fn volume(&mut self, renderer: &str, value: u8) -> Result<u8, &'static str>;
    fn calibrate(&mut self, _generation: u64, _sink: GenerationSink) -> Result<(), &'static str> {
        Err("calibration-unavailable")
    }
}

struct PendingBrowserAction {
    owner: String,
    caller: Caller,
    result: CommandResult,
    deadline: Instant,
}
pub struct Runtime<B: Backend> {
    session: Session,
    backend: B,
    audio: Arc<AudioHub>,
    advertised: SocketAddr,
    pub bus: Arc<EventBus>,
    pub leases: Arc<LeaseRegistry>,
    preferences_path: Option<PathBuf>,
    active_lease: Option<String>,
    start_deadline: Option<(u64, Instant)>,
    transition_deadline: Option<(String, Instant)>,
    renderer_connections: std::collections::HashSet<SocketAddr>,
    renderer_lost_at: Option<Instant>,
    browser_actions: std::collections::HashMap<String, PendingBrowserAction>,
    browser_results: Vec<(Caller, CommandResult)>,
}
impl<B: Backend> Runtime<B> {
    pub fn new(
        instance: String,
        backend: B,
        audio: Arc<AudioHub>,
        preferences: Preferences,
        advertised: SocketAddr,
    ) -> Self {
        let leases = Arc::new(LeaseRegistry::new(instance.clone()));
        let mut session = Session::new(instance);
        session.restore_preferences(&preferences);
        let bus = Arc::new(EventBus::new(64));
        bus.publish(session.snapshot().clone());
        Self {
            session,
            backend,
            audio,
            advertised,
            bus,
            leases,
            preferences_path: None,
            active_lease: None,
            start_deadline: None,
            transition_deadline: None,
            renderer_connections: Default::default(),
            renderer_lost_at: None,
            browser_actions: Default::default(),
            browser_results: vec![],
        }
    }
    pub fn snapshot(&self) -> &Snapshot {
        self.session.snapshot()
    }
    pub fn backend(&self) -> &B {
        &self.backend
    }
    pub fn command(&mut self, caller: &Caller, command: Command) -> CommandResult {
        if let Some(result) = self.session.cached_result(caller, &command) {
            return result;
        }
        let action = command.action.clone();
        let (mut result, effects) = self.session.apply(caller, command);
        if result.status == Status::Ok
            && let Action::BrowserActionResult {
                action_id,
                status,
                code,
            } = action
        {
            if self
                .browser_actions
                .get(&action_id)
                .is_some_and(|pending| pending.owner == caller.connection_id)
            {
                self.complete_browser_action(&action_id, status, &code);
            } else {
                result.status = Status::Rejected;
                result.code = "stale-browser-action".into();
            }
        }
        let executed = !effects.is_empty();
        for effect in effects {
            if let Effect::ReloadTab { owner, .. } = effect {
                if self.browser_actions.len() >= 16 {
                    result.status = Status::Rejected;
                    result.code = "browser-busy".into();
                    break;
                }
                let Ok(action_id) = random_id() else {
                    result.status = Status::Failed;
                    result.code = "identity-unavailable".into();
                    break;
                };
                self.browser_actions.insert(
                    action_id.clone(),
                    PendingBrowserAction {
                        owner: owner.clone(),
                        caller: caller.clone(),
                        result: result.clone(),
                        deadline: Instant::now() + Duration::from_secs(17),
                    },
                );
                self.bus
                    .send_to(&owner, BridgeEvent::BrowserAction { action_id });
                continue;
            }
            if let Err(code) = self.execute(effect, &mut result) {
                result.status = Status::Failed;
                result.code = code.into();
                if self.snapshot().phase == Phase::Starting {
                    self.event(Event::PlaybackFailed {
                        generation: self.snapshot().generation,
                        code: code.into(),
                    });
                }
                break;
            }
        }
        if executed {
            result.revision = self.snapshot().revision;
        }
        self.session.finish_result(caller, &result);
        self.bus.publish(self.snapshot().clone());
        result
    }
    pub fn event(&mut self, event: Event) {
        if let Event::BrowserDisconnected { owner } = &event {
            let pending: Vec<_> = self
                .browser_actions
                .iter()
                .filter(|(_, p)| &p.owner == owner)
                .map(|(id, _)| id.clone())
                .collect();
            for id in pending {
                self.complete_browser_action(&id, Status::Failed, "browser-unavailable");
            }
        }
        let effects = self.session.on_event(event);
        let mut ignored = CommandResult {
            request_id: String::new(),
            status: Status::Ok,
            code: String::new(),
            revision: 0,
            lease_id: None,
        };
        for effect in effects {
            let _ = self.execute(effect, &mut ignored);
        }
        self.bus.publish(self.snapshot().clone());
    }
    fn execute(&mut self, effect: Effect, result: &mut CommandResult) -> Result<(), &'static str> {
        match effect {
            Effect::StopPlayback { generation } => {
                self.renderer_connections.clear();
                self.renderer_lost_at = None;
                self.audio.end(generation);
                self.backend.stop();
                self.start_deadline = None;
                if let Some(lease) = self.active_lease.take() {
                    self.leases.release(&lease);
                }
            }
            Effect::ReleaseBrowser {
                owner,
                transition_id,
            } => {
                self.transition_deadline = Some((
                    transition_id.clone(),
                    Instant::now() + Duration::from_secs(5),
                ));
                self.bus
                    .send_to(&owner, BridgeEvent::ReleaseCapture { transition_id });
            }
            Effect::OpenWindowsCapture { generation } => {
                let format = self.backend.windows_format()?;
                let sink = self
                    .audio
                    .begin(generation, format)
                    .map_err(|_| "unsupported-output-format")?;
                self.backend.start_capture(sink)?;
                self.start_play(generation, format.sample_rate)?;
            }
            Effect::PrepareBrowser { owner, generation } => {
                self.audio
                    .begin(
                        generation,
                        WavData {
                            sample_rate: 48000,
                            sample_format: cpal::SampleFormat::F32,
                            channels: 2,
                        },
                    )
                    .map_err(|_| "audio-generation-failed")?;
                let lease = self
                    .leases
                    .issue(&owner, generation, Instant::now())
                    .map_err(|_| "browser-lease-unavailable")?;
                self.active_lease = Some(lease.clone());
                result.lease_id = Some(lease);
                self.start_deadline = Some((generation, Instant::now() + Duration::from_secs(30)));
            }
            Effect::StartBrowser { generation, .. } => self.start_play(generation, 48000)?,
            Effect::SetVolume { renderer_id, value } => {
                let confirmed = self.backend.volume(&renderer_id, value)?;
                self.event(Event::Volume {
                    renderer_id,
                    value: confirmed,
                });
            }
            Effect::Calibrate { generation } => {
                let sink = self
                    .audio
                    .sink(generation)
                    .map_err(|_| "audio-generation-unavailable")?;
                self.backend.calibrate(generation, sink)?;
            }
            Effect::PersistPreferences => {
                if let Some(path) = &self.preferences_path {
                    Preferences::from_snapshot(self.snapshot())
                        .save_atomic(path)
                        .map_err(|_| "preferences-save-failed")?;
                }
            }
            Effect::ReloadTab { owner, action_id } => self
                .bus
                .send_to(&owner, BridgeEvent::BrowserAction { action_id }),
        }
        Ok(())
    }
    fn start_play(&mut self, generation: u64, sample_rate: u32) -> Result<(), &'static str> {
        let state = self.snapshot();
        let room = state.renderer_id.clone().ok_or("select-renderer")?;
        let url = format!(
            "http://{}/managed/{}/{generation}/stream.wav",
            self.advertised, state.instance_id
        );
        self.start_deadline = Some((generation, Instant::now() + Duration::from_secs(15)));
        self.backend.play(&room, generation, &url, sample_rate)
    }
    pub fn tick(&mut self, now: Instant) {
        let expired: Vec<_> = self
            .browser_actions
            .iter()
            .filter(|(_, p)| now >= p.deadline)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            self.complete_browser_action(&id, Status::Failed, "browser-action-timeout");
        }
        if self
            .renderer_lost_at
            .is_some_and(|at| now >= at + Duration::from_secs(2))
        {
            self.event(Event::PlaybackFailed {
                generation: self.snapshot().generation,
                code: "renderer-disconnected".into(),
            });
        }
        if self
            .start_deadline
            .as_ref()
            .is_some_and(|(generation, deadline)| {
                self.snapshot().phase == Phase::Starting
                    && self.snapshot().generation == *generation
                    && now >= *deadline
            })
        {
            self.event(Event::PlaybackFailed {
                generation: self.snapshot().generation,
                code: "renderer-unreachable".into(),
            });
        }
        if self
            .transition_deadline
            .as_ref()
            .is_some_and(|(_, deadline)| now >= *deadline)
        {
            let (transition_id, _) = self.transition_deadline.take().unwrap();
            self.event(Event::TransitionTimedOut { transition_id });
        }
    }
    pub fn shutdown(&mut self) {
        self.audio.end_current();
        self.backend.stop();
        if let Some(lease) = self.active_lease.take() {
            self.leases.release(&lease);
        }
    }
    pub fn awaiting_browser(&self, caller: &Caller, request_id: &str) -> bool {
        self.browser_actions
            .values()
            .any(|pending| &pending.caller == caller && pending.result.request_id == request_id)
    }
    pub fn take_browser_results(&mut self) -> Vec<(Caller, CommandResult)> {
        std::mem::take(&mut self.browser_results)
    }
    fn complete_browser_action(&mut self, id: &str, status: Status, code: &str) {
        if let Some(mut pending) = self.browser_actions.remove(id) {
            pending.result.status = status;
            pending.result.code = code.into();
            pending.result.revision = self.snapshot().revision;
            self.session.finish_result(&pending.caller, &pending.result);
            self.browser_results.push((pending.caller, pending.result));
        }
    }

    pub fn stream_event(&mut self, event: StreamEvent) {
        if event.generation != self.snapshot().generation {
            return;
        }
        let expected = self
            .snapshot()
            .renderer_id
            .as_ref()
            .and_then(|id| self.snapshot().renderers.iter().find(|room| &room.id == id))
            .and_then(|room| room.address.parse::<std::net::IpAddr>().ok());
        if Some(event.peer.ip()) != expected {
            return;
        }
        match event.kind {
            StreamEventKind::Started => {
                self.renderer_connections.insert(event.peer);
                self.renderer_lost_at = None;
                self.event(Event::StreamStarted {
                    generation: event.generation,
                });
            }
            StreamEventKind::Ended => {
                if self.renderer_connections.remove(&event.peer)
                    && self.renderer_connections.is_empty()
                {
                    self.renderer_lost_at = Some(Instant::now());
                }
            }
        }
    }
}
impl<B: Backend> Drop for Runtime<B> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct NativeBackend {
    config: ManagedConfig,
    rooms: Vec<Renderer>,
    device: Option<OutputDevice>,
    capture: Option<cpal::Stream>,
    capture_errors: Sender<()>,
    playing: Option<String>,
    calibration: CalibrationCoordinator,
    calibration_events: Sender<crate::browser::protocol::ServerEvent>,
    microphone: Option<cpal::Stream>,
}
impl Backend for NativeBackend {
    fn windows_format(&mut self) -> Result<WavData, &'static str> {
        let device = OutputDevice::open(self.config.output_device.as_deref())?;
        let format = device.format();
        self.device = Some(device);
        Ok(format)
    }
    fn start_capture(&mut self, sink: GenerationSink) -> Result<(), &'static str> {
        self.capture = Some(
            self.device
                .as_ref()
                .ok_or("output-device-unavailable")?
                .capture(sink, self.capture_errors.clone())?,
        );
        Ok(())
    }
    fn stop(&mut self) {
        self.capture = None;
        self.device = None;
        self.microphone = None;
        if let Some(id) = self.playing.take()
            && let Some(room) = self.rooms.iter_mut().find(|room| room.remote_addr == id)
        {
            room.stop_play();
        }
    }
    fn play(
        &mut self,
        renderer: &str,
        _generation: u64,
        url: &str,
        sample_rate: u32,
    ) -> Result<(), &'static str> {
        let address =
            select_interface(self.config.interface_address).map_err(|_| "interface-unavailable")?;
        let mut url = url::Url::parse(url).map_err(|_| "invalid-stream-url")?;
        url.set_ip_host(address).map_err(|_| "invalid-stream-url")?;
        let room = self
            .rooms
            .iter_mut()
            .find(|room| room.remote_addr == renderer)
            .ok_or("renderer-unavailable")?;
        self.playing = Some(renderer.into());
        room.play_url(
            url.as_str(),
            StreamInfo {
                sample_rate,
                bits_per_sample: BitDepth::Bits16,
                streaming_format: StreamingFormat::Wav,
                server_port: self.config.http_port,
            },
        )
        .map_err(|_| "renderer-unreachable")
    }
    fn volume(&mut self, renderer: &str, value: u8) -> Result<u8, &'static str> {
        let room = self
            .rooms
            .iter_mut()
            .find(|room| room.remote_addr == renderer)
            .ok_or("renderer-unavailable")?;
        room.set_volume(i32::from(value));
        u8::try_from(room.get_volume())
            .ok()
            .filter(|v| *v <= 100)
            .ok_or("volume-unavailable")
    }
    fn calibrate(&mut self, generation: u64, sink: GenerationSink) -> Result<(), &'static str> {
        if self.calibration.is_active() {
            return Err("calibration-busy");
        }
        self.microphone = Some(
            crate::listener::open_latency_listener(self.config.microphone.as_deref())
                .map_err(|_| "microphone-unavailable")?,
        );
        let measurement = Arc::new(move || {
            let (tx, rx) = bounded(1);
            crate::latency::arm_latency_response_waiter(tx);
            crate::latency::PULSE_INJECTED_AT.store(crate::latency::now_ms(), Ordering::SeqCst);
            let stereo = crate::latency::barker_mono()
                .into_iter()
                .flat_map(|sample| [sample, sample])
                .collect();
            if sink.publish(Arc::new(stereo)).is_err() {
                return None;
            }
            let result = rx.recv_timeout(Duration::from_secs(5)).ok().flatten();
            crate::latency::PULSE_INJECTED_AT.store(0, Ordering::SeqCst);
            result
        });
        self.calibration
            .start_with_measurement(
                generation.to_string(),
                self.playing.clone().unwrap_or_default(),
                self.calibration_events.clone(),
                measurement,
            )
            .map_err(|_| "calibration-busy")
    }
}

pub fn run(path: &Path) -> io::Result<()> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let signal = shutdown.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Release)).map_err(io::Error::other)?;
    run_until(path, shutdown)
}
pub fn run_until(path: &Path, shutdown: Arc<AtomicBool>) -> io::Result<()> {
    let config = ManagedConfig::load(path)?;
    let token = config.token()?;
    let preferences = Preferences::load(&config.preferences_path)?;
    let instance = random_id().map_err(io::Error::other)?;
    let audio = Arc::new(AudioHub::new());
    let (input_tx, input_rx) = bounded(256);
    let (capture_tx, capture_rx) = bounded(1);
    let (cal_tx, cal_rx) = bounded(8);
    let backend = NativeBackend {
        config: config.clone(),
        rooms: vec![],
        device: None,
        capture: None,
        capture_errors: capture_tx,
        playing: None,
        calibration: CalibrationCoordinator::new(),
        calibration_events: cal_tx,
        microphone: None,
    };
    let mut runtime = Runtime::new(
        instance.clone(),
        backend,
        audio.clone(),
        preferences,
        SocketAddr::from(([127, 0, 0, 1], config.http_port)),
    );
    runtime.preferences_path = Some(config.preferences_path.clone());
    let mut http = ManagedHttp::bind(
        SocketAddr::from(([0, 0, 0, 0], config.http_port)),
        instance,
        audio.clone(),
    )?;
    let mut bridge = ManagedBridge::bind(
        config.control_address,
        token,
        input_tx,
        runtime.bus.clone(),
        audio,
    )?;
    runtime.leases = bridge.leases();
    let (discovery_tx, discovery_rx) = bounded(1);
    let discover_stop = Arc::new(AtomicBool::new(false));
    let worker_stop = discover_stop.clone();
    let interface = config.interface_address;
    let worker = std::thread::spawn(move || discover_loop(interface, worker_stop, discovery_tx));
    println!(
        "Managed Sonos ready (idle); protocol=1 control={}",
        bridge.local_addr()
    );
    let mut network_check = Instant::now();
    let mut browser_replies: Vec<(Caller, String, Sender<CommandResult>)> = vec![];
    while !shutdown.load(Ordering::Acquire) && bridge.is_running() {
        if let Ok(input) = input_rx.recv_timeout(Duration::from_millis(20)) {
            match input {
                ManagedInput::Command {
                    caller,
                    command,
                    reply,
                } => {
                    let result = runtime.command(&caller, command);
                    if result.status == Status::Ok
                        && runtime.awaiting_browser(&caller, &result.request_id)
                    {
                        browser_replies.push((caller, result.request_id, reply));
                    } else {
                        let _ = reply.try_send(result);
                    }
                }
                ManagedInput::Event(event) => runtime.event(event),
                ManagedInput::ControllerDisconnected { caller } => {
                    runtime.session.forget_controller(&caller.connection_id);
                    browser_replies.retain(|(owner, _, _)| owner != &caller);
                }
            }
        }
        if let Ok(rooms) = discovery_rx.try_recv() {
            let summaries = rooms
                .iter()
                .map(|room| RendererSummary {
                    id: room.remote_addr.clone(),
                    name: room.dev_name.clone(),
                    model: room.dev_model.clone(),
                    address: room.remote_addr.clone(),
                })
                .collect();
            runtime.backend.rooms = rooms;
            runtime.event(Event::Renderers(summaries));
        }
        for event in http.events().try_iter() {
            runtime.stream_event(event);
        }
        if capture_rx.try_recv().is_ok() {
            runtime.event(Event::PlaybackFailed {
                generation: runtime.snapshot().generation,
                code: "output-device-lost".into(),
            });
        }
        for event in cal_rx.try_iter() {
            match event {
                crate::browser::protocol::ServerEvent::CalibrationResult {
                    request_id,
                    milliseconds,
                    ..
                } => {
                    if let Ok(generation) = request_id.parse() {
                        runtime.event(Event::Calibration {
                            generation,
                            milliseconds,
                        });
                    }
                    runtime.backend.microphone = None;
                }
                crate::browser::protocol::ServerEvent::CalibrationFailed { request_id, .. } => {
                    runtime.backend.microphone = None;
                    if let Ok(generation) = request_id.parse() {
                        runtime.event(Event::OperationFailed {
                            generation,
                            code: "calibration-timeout",
                        });
                    }
                }
                _ => (),
            }
        }
        if network_check.elapsed() > Duration::from_secs(2) {
            network_check = Instant::now();
            if matches!(runtime.snapshot().phase, Phase::Starting | Phase::Streaming)
                && select_interface(config.interface_address).is_err()
            {
                runtime.event(Event::PlaybackFailed {
                    generation: runtime.snapshot().generation,
                    code: "interface-unavailable".into(),
                });
            }
        }
        runtime.tick(Instant::now());
        for (caller, result) in runtime.take_browser_results() {
            browser_replies.retain(|(owner, request_id, reply)| {
                if owner == &caller && request_id == &result.request_id {
                    let _ = reply.try_send(result.clone());
                    false
                } else {
                    true
                }
            });
        }
    }
    runtime.shutdown();
    bridge.shutdown();
    http.shutdown();
    discover_stop.store(true, Ordering::Release);
    let _ = worker.join();
    if !shutdown.load(Ordering::Acquire) {
        return Err(io::Error::other("managed control server stopped"));
    }
    Ok(())
}
fn discover_loop(
    interface: Option<std::net::IpAddr>,
    stop: Arc<AtomicBool>,
    sender: Sender<Vec<Renderer>>,
) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(2)))
        .build()
        .into();
    while !stop.load(Ordering::Acquire) {
        if let Ok(address) = select_interface(interface)
            && let Some(rooms) =
                discover_cancellable(&agent, &hashbrown::HashMap::new(), address, Some(&stop))
        {
            let _ = sender.try_send(rooms);
        }
        for _ in 0..100 {
            if stop.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
