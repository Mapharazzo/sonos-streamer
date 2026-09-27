use std::time::{Duration, Instant};
use std::{net::TcpStream, sync::Arc};
use swyh_rs::managed::{
    events::{BridgeEvent, EventBus},
    leases::{LeaseError, LeaseRegistry},
    state::Session,
};
use swyh_rs::{
    browser::{auth::PairingToken, protocol::AudioSession},
    managed::{
        audio::AudioHub,
        bridge::{ManagedBridge, ManagedInput},
        protocol::Role,
    },
    openhome::rendercontrol::WavData,
};
use tungstenite::{Message, WebSocket, client::IntoClientRequest, http::header::ORIGIN};

struct Fixture {
    bridge: ManagedBridge,
    bus: Arc<EventBus>,
    audio: Arc<AudioHub>,
    inputs: crossbeam_channel::Receiver<ManagedInput>,
    token: PairingToken,
}

impl Fixture {
    fn new() -> Self {
        let bus = Arc::new(EventBus::new(16));
        bus.publish(Session::new("instance-1".into()).snapshot().clone());
        let audio = Arc::new(AudioHub::new());
        let (sender, inputs) = crossbeam_channel::unbounded();
        let token = PairingToken::from_bytes([9; 32]);
        let bridge = ManagedBridge::bind(
            "127.0.0.1:0".parse().unwrap(),
            token.clone(),
            sender,
            Arc::clone(&bus),
            Arc::clone(&audio),
        )
        .unwrap();
        Self {
            bridge,
            bus,
            audio,
            inputs,
            token,
        }
    }

    fn socket(
        &self,
        path: &str,
        origin: Option<&str>,
    ) -> Result<WebSocket<TcpStream>, Box<dyn std::error::Error>> {
        let mut request =
            format!("ws://{}{path}", self.bridge.local_addr()).into_client_request()?;
        if let Some(origin) = origin {
            request
                .headers_mut()
                .insert(ORIGIN, origin.parse().unwrap());
        }
        let stream = TcpStream::connect(self.bridge.local_addr())?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        Ok(tungstenite::client(request, stream)?.0)
    }

    fn controller(&self, browser: bool) -> (WebSocket<TcpStream>, String) {
        let mut socket = self
            .socket(
                "/control/v1",
                browser.then_some("chrome-extension://abcdefghijklmnopabcdefghijklmnop"),
            )
            .unwrap();
        socket.send(Message::Text(serde_json::json!({"type":"hello","version":1,"token":self.token.to_hex(),"role":if browser {"browser"} else {"controller"}}).to_string().into())).unwrap();
        let hello = receive(&mut socket);
        assert_eq!(hello["type"], "hello-ok");
        assert_eq!(receive(&mut socket)["type"], "state");
        (socket, hello["connectionId"].as_str().unwrap().into())
    }
}

fn receive(socket: &mut WebSocket<TcpStream>) -> serde_json::Value {
    serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap()
}

#[test]
fn authenticated_controllers_receive_the_same_live_state_without_becoming_producers() {
    let mut fixture = Fixture::new();
    let (mut first, _) = fixture.controller(false);
    let (mut second, _) = fixture.controller(false);
    let mut snapshot = Session::new("instance-1".into()).snapshot().clone();
    snapshot.revision = 4;
    fixture.bus.publish(snapshot);
    assert_eq!(receive(&mut first)["revision"], 4);
    assert_eq!(receive(&mut second)["revision"], 4);
    first
        .send(Message::Text(
            include_str!("fixtures/managed-select-source.json").into(),
        ))
        .unwrap();
    match fixture.inputs.recv_timeout(Duration::from_secs(2)).unwrap() {
        ManagedInput::Command {
            caller,
            command,
            reply,
        } => {
            assert_eq!(caller.role, Role::Controller);
            assert_eq!(command.request_id, "request-1");
            reply
                .send(swyh_rs::managed::protocol::CommandResult::rejected(
                    &command,
                    "stale-revision",
                    4,
                ))
                .unwrap();
        }
        _ => panic!("native hello must not reserve capture"),
    }
    assert_eq!(receive(&mut first)["code"], "stale-revision");
    fixture.bridge.shutdown();
}

#[test]
fn two_browser_sockets_receive_state_without_stealing_each_others_audio_lease() {
    let mut fixture = Fixture::new();
    let (mut first, first_id) = fixture.controller(true);
    let (mut second, second_id) = fixture.controller(true);
    assert_ne!(first_id, second_id);
    let lease = fixture
        .bridge
        .leases()
        .issue(&first_id, 2, Instant::now())
        .unwrap();
    fixture
        .bridge
        .leases()
        .attach("instance-1", &lease, Instant::now())
        .unwrap();
    let mut snapshot = Session::new("instance-1".into()).snapshot().clone();
    snapshot.revision = 7;
    fixture.bus.publish(snapshot);
    assert_eq!(receive(&mut first)["revision"], 7);
    assert_eq!(receive(&mut second)["revision"], 7);
    second.close(None).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let input = fixture
            .inputs
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        if matches!(input, ManagedInput::Event(swyh_rs::managed::state::Event::BrowserDisconnected {owner}) if owner == second_id)
        {
            break;
        }
    }
    assert!(fixture.bridge.leases().is_active(&lease));
    fixture.bridge.shutdown();
}

#[test]
fn ordinary_web_origins_and_wrong_tokens_cannot_control_playback() {
    let mut fixture = Fixture::new();
    assert!(
        fixture
            .socket("/control/v1", Some("https://example.com"))
            .is_err()
    );
    assert!(fixture.socket("/audio/v1", None).is_err());
    let mut bad = fixture.socket("/control/v1", None).unwrap();
    bad.send(Message::Text(
        serde_json::json!({"type":"hello","version":1,"token":"wrong","role":"controller"})
            .to_string()
            .into(),
    ))
    .unwrap();
    assert_eq!(receive(&mut bad)["code"], "authentication-failed");
    let mut binary = fixture.socket("/control/v1", None).unwrap();
    binary.send(Message::Binary(vec![0; 4].into())).unwrap();
    assert_eq!(receive(&mut binary)["code"], "hello-required");
    assert!(fixture.inputs.try_recv().is_err());
    fixture.bridge.shutdown();
}

#[test]
fn leased_browser_audio_reaches_only_its_generation() {
    let mut fixture = Fixture::new();
    let (_control, owner) = fixture.controller(true);
    assert!(matches!(
        fixture.inputs.recv_timeout(Duration::from_secs(1)).unwrap(),
        ManagedInput::Event(_)
    ));
    let _sink = fixture
        .audio
        .begin(
            2,
            WavData {
                sample_format: cpal::SampleFormat::F32,
                sample_rate: 48000,
                channels: 2,
            },
        )
        .unwrap();
    let audio = fixture.audio.subscribe(2).unwrap();
    let lease = fixture
        .bridge
        .leases()
        .issue(&owner, 2, Instant::now())
        .unwrap();
    let mut producer = fixture
        .socket(
            "/audio/v1",
            Some("chrome-extension://abcdefghijklmnopabcdefghijklmnop"),
        )
        .unwrap();
    producer.send(Message::Text(serde_json::json!({"type":"hello","version":1,"token":fixture.token.to_hex(),"instanceId":"instance-1","leaseId":lease,"audio":AudioSession::v1().hello()}).to_string().into())).unwrap();
    assert_eq!(receive(&mut producer)["type"], "hello-ok");
    producer
        .send(Message::Binary(vec![0, 64, 0, 192].into()))
        .unwrap();
    assert_eq!(
        &*audio.recv_timeout(Duration::from_secs(1)).unwrap(),
        &[0.5, -0.5]
    );
    fixture.audio.end(2);
    let _new_sink = fixture
        .audio
        .begin(
            3,
            WavData {
                sample_format: cpal::SampleFormat::F32,
                sample_rate: 48000,
                channels: 2,
            },
        )
        .unwrap();
    let next = fixture.audio.subscribe(3).unwrap();
    producer
        .send(Message::Binary(vec![0, 64, 0, 192].into()))
        .unwrap();
    assert!(next.recv_timeout(Duration::from_millis(50)).is_err());
    fixture.bridge.shutdown();
}

#[test]
fn controller_state_is_broadcast_and_replayed_to_new_subscribers() {
    let bus = EventBus::new(4);
    let a = bus.subscribe();
    let b = bus.subscribe();
    let state = Session::new("instance-1".into()).snapshot().clone();
    bus.publish(state.clone());
    assert_eq!(
        a.recv_timeout(Duration::from_secs(1)).unwrap(),
        BridgeEvent::State(state.clone())
    );
    assert_eq!(
        b.recv_timeout(Duration::from_secs(1)).unwrap(),
        BridgeEvent::State(state.clone())
    );
    assert_eq!(
        bus.subscribe()
            .recv_timeout(Duration::from_secs(1))
            .unwrap(),
        BridgeEvent::State(state)
    );
}

#[test]
fn slow_controller_is_disconnected_without_blocking_other_controllers() {
    let bus = EventBus::new(2);
    let slow = bus.subscribe();
    let fast = bus.subscribe();
    let mut state = Session::new("instance-1".into()).snapshot().clone();
    for revision in 1..=10 {
        state.revision = revision;
        bus.publish(state.clone());
        assert_eq!(
            fast.recv_timeout(Duration::from_secs(1)).unwrap(),
            BridgeEvent::State(state.clone())
        );
    }
    assert!(slow.recv_timeout(Duration::from_millis(1)).is_err());
}

#[test]
fn capture_release_is_only_delivered_to_its_browser_owner() {
    let bus = EventBus::new(4);
    let owner = bus.subscribe_for("browser");
    let native = bus.subscribe_for("native");
    bus.send_to(
        "browser",
        BridgeEvent::ReleaseCapture {
            transition_id: "transition-1".into(),
        },
    );
    assert_eq!(
        owner.recv_timeout(Duration::from_secs(1)).unwrap(),
        BridgeEvent::ReleaseCapture {
            transition_id: "transition-1".into()
        }
    );
    assert!(native.recv_timeout(Duration::from_millis(1)).is_err());
}

#[test]
fn browser_lease_is_owned_single_use_expiring_and_instance_bound() {
    let leases = LeaseRegistry::new("instance-1".into());
    leases.register_browser("browser").unwrap();
    leases.register_browser("other").unwrap();
    let now = Instant::now();
    assert_eq!(leases.issue("native", 2, now), Err(LeaseError::NotOwner));
    let lease = leases.issue("browser", 2, now).unwrap();
    assert!(leases.attach("other-instance", &lease, now).is_err());
    assert!(leases.attach("instance-1", "not-a-lease", now).is_err());
    let grant = leases.attach("instance-1", &lease, now).unwrap();
    assert_eq!(grant.owner, "browser");
    assert_eq!(grant.generation, 2);
    assert!(leases.attach("instance-1", &lease, now).is_err());
    assert_eq!(leases.issue("browser", 3, now), Err(LeaseError::Busy));
    assert_eq!(leases.issue("other", 3, now), Err(LeaseError::Busy));
    leases.unregister_browser("other");
    assert!(leases.is_active(&lease));
    leases.release(&lease);
    let next = leases.issue("browser", 3, now).unwrap();
    assert_eq!(
        leases.attach("instance-1", &next, now + Duration::from_secs(31)),
        Err(LeaseError::Expired)
    );
}

#[test]
fn owner_disconnect_invalidates_pending_and_active_audio_leases() {
    let leases = LeaseRegistry::new("instance-1".into());
    leases.register_browser("browser").unwrap();
    let now = Instant::now();
    let pending = leases.issue("browser", 2, now).unwrap();
    leases.unregister_browser("unrelated");
    assert!(leases.attach("instance-1", &pending, now).is_ok());
    leases.unregister_browser("browser");
    assert!(!leases.is_active(&pending));
    leases.register_browser("replacement").unwrap();
    assert!(leases.issue("replacement", 3, now).is_ok());
}

#[test]
fn oversized_control_frames_close_without_dispatching_a_command() {
    let mut fixture = Fixture::new();
    let (mut socket, _) = fixture.controller(false);
    let _ = socket.send(Message::Text("x".repeat(65537).into()));
    assert!(socket.read().is_err());
    while let Ok(input) = fixture.inputs.try_recv() {
        assert!(!matches!(input, ManagedInput::Command { .. }));
    }
    fixture.bridge.shutdown();
}

#[test]
fn browser_identity_requires_an_extension_origin_even_with_valid_token() {
    let mut fixture = Fixture::new();
    let mut socket = fixture.socket("/control/v1", None).unwrap();
    socket.send(Message::Text(serde_json::json!({"type":"hello","version":1,"token":fixture.token.to_hex(),"role":"browser"}).to_string().into())).unwrap();
    assert_eq!(receive(&mut socket)["code"], "browser-origin-required");
    assert!(fixture.inputs.try_recv().is_err());
    fixture.bridge.shutdown();
}

#[test]
fn unauthenticated_connections_have_a_bounded_lifetime() {
    let mut fixture = Fixture::new();
    let mut socket = fixture.socket("/control/v1", None).unwrap();
    let started = Instant::now();
    assert_eq!(receive(&mut socket)["code"], "hello-timeout");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(fixture.inputs.try_recv().is_err());
    fixture.bridge.shutdown();
}
