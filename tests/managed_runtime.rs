use std::{path::PathBuf, sync::Arc};
use swyh_rs::{
    browser::protocol::RendererSummary,
    managed::{
        audio::AudioHub,
        config::ManagedConfig,
        preferences::Preferences,
        protocol::{Action, Caller, Command, Phase, Role, Source, Status},
        runtime::{Backend, Runtime},
    },
    openhome::rendercontrol::WavData,
    utils::commandline::Args,
};

#[test]
fn managed_arguments_require_config_and_reject_legacy_options() {
    let args =
        Args::parse_args(["--managed", "--managed-config", "C:/private/sonos.json"]).unwrap();
    assert!(args.managed);
    assert!(!args.browser_source);
    for args in [
        vec!["--managed"],
        vec!["--managed-config", "x"],
        vec!["--managed", "--managed-config", "x", "--browser-source"],
        vec![
            "--managed",
            "--managed-config",
            "x",
            "--auto_resume",
            "true",
        ],
        vec!["--managed", "--managed-config", "x", "--typo"],
    ] {
        assert!(Args::parse_args(args).is_err());
    }
}

#[test]
fn config_is_strict_loopback_only_and_requires_absolute_private_paths() {
    let root = std::env::temp_dir();
    let valid = serde_json::json!({"version":1,"controlAddress":"127.0.0.1:5902","httpPort":5901,"tokenPath":root.join("token"),"preferencesPath":root.join("prefs.json")});
    assert!(ManagedConfig::from_json(&valid.to_string()).is_ok());
    for (key, value) in [
        ("version", serde_json::json!(2)),
        ("controlAddress", serde_json::json!("0.0.0.0:5902")),
        ("tokenPath", serde_json::json!("relative")),
        ("secretToken", serde_json::json!("no")),
    ] {
        let mut invalid = valid.clone();
        invalid[key] = value;
        assert!(
            ManagedConfig::from_json(&invalid.to_string()).is_err(),
            "{key}"
        );
    }
}

#[derive(Default)]
struct FakeBackend {
    opened: usize,
    stopped: usize,
    plays: Vec<(u64, String)>,
    no_device: bool,
    volume_reads: usize,
}
impl Backend for FakeBackend {
    fn read_volume(&mut self, _: &str) -> Result<u8, &'static str> {
        self.volume_reads += 1;
        Ok(37)
    }
    fn windows_format(&mut self) -> Result<WavData, &'static str> {
        if self.no_device {
            Err("output-device-unavailable")
        } else {
            Ok(WavData {
                sample_format: cpal::SampleFormat::F32,
                sample_rate: 44100,
                channels: 2,
            })
        }
    }
    fn start_capture(
        &mut self,
        _: swyh_rs::managed::audio::GenerationSink,
    ) -> Result<(), &'static str> {
        self.opened += 1;
        Ok(())
    }
    fn stop(&mut self) {
        self.stopped += 1;
    }
    fn play(&mut self, _: &str, generation: u64, url: &str, _: u32) -> Result<(), &'static str> {
        self.plays.push((generation, url.into()));
        Ok(())
    }
    fn volume(&mut self, _: &str, value: u8) -> Result<u8, &'static str> {
        Ok(value)
    }
}
fn command(
    runtime: &mut Runtime<FakeBackend>,
    action: Action,
) -> swyh_rs::managed::protocol::CommandResult {
    runtime.command(
        &Caller {
            connection_id: "native".into(),
            role: Role::Controller,
        },
        Command {
            request_id: format!("r{}", runtime.snapshot().revision),
            instance_id: "test-instance".into(),
            expected_revision: runtime.snapshot().revision,
            action,
        },
    )
}
fn room(runtime: &mut Runtime<FakeBackend>) {
    runtime.event(swyh_rs::managed::state::Event::Renderers(vec![
        RendererSummary {
            id: "room".into(),
            name: "Room".into(),
            model: "Sonos".into(),
            address: "127.0.0.1".into(),
        },
    ]));
}

#[test]
fn selected_and_restored_room_volume_is_queried_without_a_volume_write() {
    for restored in [false, true] {
        let mut runtime = Runtime::new(
            "test-instance".into(),
            FakeBackend::default(),
            Arc::new(AudioHub::new()),
            Preferences {
                renderer_id: restored.then(|| "room".into()),
                ..Preferences::default()
            },
            "127.0.0.1:5901".parse().unwrap(),
        );
        assert_eq!(runtime.snapshot().volume, None);
        room(&mut runtime);
        if !restored {
            assert_eq!(
                command(
                    &mut runtime,
                    Action::SelectRenderer {
                        renderer_id: "room".into()
                    }
                )
                .status,
                Status::Ok
            );
        }
        assert_eq!(runtime.snapshot().volume, Some(37));
        assert_eq!(runtime.backend().volume_reads, 1);
        assert_eq!(runtime.backend().opened, 0);
    }
}

#[test]
fn startup_preferences_never_open_capture_and_delayed_discovery_allows_retry() {
    let mut runtime = Runtime::new(
        "test-instance".into(),
        FakeBackend::default(),
        Arc::new(AudioHub::new()),
        Preferences {
            source: Source::Windows,
            renderer_id: Some("room".into()),
            manual_offset_ms: 0,
        },
        "127.0.0.1:5901".parse().unwrap(),
    );
    assert_eq!(runtime.snapshot().phase, Phase::Idle);
    assert_eq!(runtime.backend().opened, 0);
    assert_eq!(
        command(&mut runtime, Action::StartWindows).code,
        "renderer-unavailable"
    );
    room(&mut runtime);
    assert_eq!(
        command(&mut runtime, Action::StartWindows).status,
        Status::Ok
    );
    assert_eq!(runtime.backend().opened, 1);
    assert_eq!(runtime.snapshot().phase, Phase::Starting);
    let generation = runtime.snapshot().generation;
    runtime.event(swyh_rs::managed::state::Event::StreamStarted { generation });
    assert_eq!(runtime.snapshot().phase, Phase::Streaming);
    assert!(
        runtime.backend().plays[0]
            .1
            .ends_with(&format!("/{generation}/stream.wav"))
    );
    command(&mut runtime, Action::Stop);
    assert_eq!(runtime.snapshot().phase, Phase::Idle);
}

#[test]
fn missing_device_fails_start_but_keeps_control_ready() {
    let mut runtime = Runtime::new(
        "test-instance".into(),
        FakeBackend {
            no_device: true,
            ..Default::default()
        },
        Arc::new(AudioHub::new()),
        Preferences {
            source: Source::Windows,
            renderer_id: Some("room".into()),
            manual_offset_ms: 0,
        },
        "127.0.0.1:5901".parse().unwrap(),
    );
    room(&mut runtime);
    command(&mut runtime, Action::StartWindows);
    assert_eq!(
        runtime.snapshot().error_code.as_deref(),
        Some("output-device-unavailable")
    );
    assert_eq!(runtime.backend().opened, 0);
    assert_eq!(
        command(
            &mut runtime,
            Action::SelectSource {
                source: Source::Chrome
            }
        )
        .status,
        Status::Ok
    );
}

#[test]
fn preferences_atomically_replace_and_contain_no_playback_ownership() {
    let path: PathBuf = std::env::temp_dir().join(format!(
        "managed-prefs-{}.json",
        swyh_rs::managed::leases::random_id().unwrap()
    ));
    let mut prefs = Preferences::default();
    prefs.save_atomic(&path).unwrap();
    prefs.source = Source::Windows;
    prefs.manual_offset_ms = 123;
    prefs.save_atomic(&path).unwrap();
    assert_eq!(Preferences::load(&path).unwrap(), prefs);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(!content.contains("generation"));
    assert!(!content.contains("token"));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn runtime_replays_exact_result_after_state_has_advanced() {
    let mut runtime = Runtime::new(
        "test-instance".into(),
        FakeBackend::default(),
        Arc::new(AudioHub::new()),
        Preferences::default(),
        "127.0.0.1:5901".parse().unwrap(),
    );
    let caller = Caller {
        connection_id: "native".into(),
        role: Role::Controller,
    };
    let request = Command {
        request_id: "same".into(),
        instance_id: "test-instance".into(),
        expected_revision: 1,
        action: Action::SelectSource {
            source: Source::Windows,
        },
    };
    let first = runtime.command(&caller, request.clone());
    room(&mut runtime);
    assert_eq!(runtime.command(&caller, request), first);
}

#[test]
fn start_deadline_retires_audio_and_keeps_control_retryable() {
    let audio = Arc::new(AudioHub::new());
    let mut runtime = Runtime::new(
        "test-instance".into(),
        FakeBackend::default(),
        audio.clone(),
        Preferences {
            source: Source::Windows,
            renderer_id: Some("room".into()),
            manual_offset_ms: 0,
        },
        "127.0.0.1:5901".parse().unwrap(),
    );
    room(&mut runtime);
    command(&mut runtime, Action::StartWindows);
    let generation = runtime.snapshot().generation;
    runtime.tick(std::time::Instant::now() + std::time::Duration::from_secs(16));
    assert_eq!(
        runtime.snapshot().error_code.as_deref(),
        Some("renderer-unreachable")
    );
    assert!(audio.sink(generation).is_err());
    assert_eq!(
        command(&mut runtime, Action::StartWindows).status,
        Status::Ok
    );
}

#[test]
fn managed_runtime_is_ready_without_devices_or_configured_lan_and_shuts_down() {
    use std::{
        net::TcpListener,
        sync::atomic::{AtomicBool, Ordering},
        time::{Duration, Instant},
    };
    let root = std::env::temp_dir().join(format!(
        "managed-runtime-{}",
        swyh_rs::managed::leases::random_id().unwrap()
    ));
    std::fs::create_dir(&root).unwrap();
    let config_path = root.join("config.json");
    let token = "19".repeat(32);
    std::fs::write(root.join("token"), &token).unwrap();
    let control = TcpListener::bind("127.0.0.1:0").unwrap();
    let control_address = control.local_addr().unwrap();
    drop(control);
    let http = TcpListener::bind("127.0.0.1:0").unwrap();
    let http_port = http.local_addr().unwrap().port();
    drop(http);
    let config = serde_json::json!({"version":1,"controlAddress":control_address.to_string(),"httpPort":http_port,"tokenPath":root.join("token"),"preferencesPath":root.join("prefs.json"),"interfaceAddress":"192.0.2.254","outputDevice":"No such audio device"});
    std::fs::write(&config_path, config.to_string()).unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let running = shutdown.clone();
    let worker =
        std::thread::spawn(move || swyh_rs::managed::runtime::run_until(&config_path, running));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut socket = loop {
        match tungstenite::connect(format!("ws://{control_address}/control/v1")) {
            Ok((socket, _)) => break socket,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => panic!("readiness: {error}"),
        }
    };
    socket
        .send(tungstenite::Message::text(
            serde_json::json!({"type":"hello","version":1,"role":"controller","token":token})
                .to_string(),
        ))
        .unwrap();
    let mut state = None;
    for _ in 0..3 {
        let event: serde_json::Value =
            serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
        if event["type"] == "state" {
            state = Some(event);
            break;
        }
    }
    let state = state.unwrap();
    assert_eq!(state["phase"], "idle");
    assert_eq!(state["generation"], 1);
    shutdown.store(true, Ordering::Release);
    worker.join().unwrap().unwrap();
    drop(socket);
    for name in ["token", "config.json"] {
        std::fs::remove_file(root.join(name)).unwrap();
    }
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn fake_renderer_fetches_real_pcm_and_stop_retires_its_url() {
    use std::{
        io::{Read, Write},
        net::TcpStream,
        time::Duration,
    };
    use swyh_rs::managed::streaming::ManagedHttp;
    let audio = Arc::new(AudioHub::new());
    let mut http = ManagedHttp::bind(
        "127.0.0.1:0".parse().unwrap(),
        "test-instance".into(),
        audio.clone(),
    )
    .unwrap();
    let mut runtime = Runtime::new(
        "test-instance".into(),
        FakeBackend::default(),
        audio.clone(),
        Preferences {
            source: Source::Windows,
            renderer_id: Some("room".into()),
            manual_offset_ms: 0,
        },
        http.local_addr(),
    );
    room(&mut runtime);
    command(&mut runtime, Action::StartWindows);
    let url = url::Url::parse(&runtime.backend().plays[0].1).unwrap();
    let generation = runtime.snapshot().generation;
    let mut socket = TcpStream::connect(http.local_addr()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(
        socket,
        "GET {} HTTP/1.1\r\nHost: localhost\r\n\r\n",
        url.path()
    )
    .unwrap();
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        socket.read_exact(&mut byte).unwrap();
        headers.push(byte[0]);
    }
    let mut wav = [0; 44];
    socket.read_exact(&mut wav).unwrap();
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 44100);
    let started = http.events().recv_timeout(Duration::from_secs(1)).unwrap();
    runtime.event(swyh_rs::managed::state::Event::StreamStarted {
        generation: started.generation,
    });
    audio
        .sink(generation)
        .unwrap()
        .publish(Arc::new(vec![0.5, -0.5]))
        .unwrap();
    let mut pcm = [0; 4];
    socket.read_exact(&mut pcm).unwrap();
    assert_eq!(pcm, [0, 64, 0, 192]);
    command(&mut runtime, Action::Stop);
    assert!(audio.sink(generation).is_err());
    let mut stale = TcpStream::connect(http.local_addr()).unwrap();
    write!(
        stale,
        "GET {} HTTP/1.1\r\nHost: localhost\r\n\r\n",
        url.path()
    )
    .unwrap();
    let mut response = String::new();
    stale.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 410"));
    runtime.shutdown();
    http.shutdown();
}

#[test]
fn reload_result_waits_for_matching_browser_completion_and_is_cached() {
    let mut runtime = Runtime::new(
        "test-instance".into(),
        FakeBackend::default(),
        Arc::new(AudioHub::new()),
        Preferences::default(),
        "127.0.0.1:5901".parse().unwrap(),
    );
    let browser = Caller {
        connection_id: "browser".into(),
        role: Role::Browser,
    };
    let native = Caller {
        connection_id: "native".into(),
        role: Role::Controller,
    };
    runtime.event(swyh_rs::managed::state::Event::BrowserConnected {
        owner: "browser".into(),
    });
    runtime.command(
        &browser,
        Command {
            request_id: "status".into(),
            instance_id: "test-instance".into(),
            expected_revision: runtime.snapshot().revision,
            action: Action::BrowserStatus {
                shortcut: "Ctrl+Shift+8".into(),
                supported_tab: true,
                has_recovery_tab: true,
            },
        },
    );
    let events = runtime.bus.subscribe_for("browser");
    let _ = events.recv_timeout(std::time::Duration::from_secs(1));
    let request = Command {
        request_id: "reload".into(),
        instance_id: "test-instance".into(),
        expected_revision: runtime.snapshot().revision,
        action: Action::ReloadTab,
    };
    assert_eq!(runtime.command(&native, request.clone()).status, Status::Ok);
    assert!(runtime.awaiting_browser(&native, "reload"));
    let mut action_id = None;
    for _ in 0..2 {
        if let Ok(swyh_rs::managed::events::BridgeEvent::BrowserAction { action_id: id }) =
            events.recv_timeout(std::time::Duration::from_millis(50))
        {
            action_id = Some(id);
            break;
        }
    }
    runtime.command(
        &browser,
        Command {
            request_id: "done".into(),
            instance_id: "test-instance".into(),
            expected_revision: runtime.snapshot().revision,
            action: Action::BrowserActionResult {
                action_id: action_id.unwrap(),
                status: Status::Ok,
                code: "invoke-extension".into(),
            },
        },
    );
    let completed = runtime.take_browser_results();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].0, native);
    assert_eq!(completed[0].1.code, "invoke-extension");
    assert_eq!(runtime.command(&native, request), completed[0].1);
}
