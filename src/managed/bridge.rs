use super::{
    audio::AudioHub,
    events::EventBus,
    leases::{LeaseRegistry, random_id},
    protocol::{Caller, Command, CommandResult, MAX_TEXT_BYTES, Role, parse_command},
    state::Event,
};
use crate::browser::{
    audio::decode_packet,
    auth::PairingToken,
    protocol::{AudioHello, AudioSession},
};
use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io,
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tungstenite::{
    Message, WebSocket, accept_hdr_with_config,
    handshake::server::{ErrorResponse, Request, Response},
    http::{StatusCode, header::ORIGIN},
    protocol::WebSocketConfig,
};

pub enum ManagedInput {
    Command {
        caller: Caller,
        command: Command,
        reply: Sender<CommandResult>,
    },
    Event(Event),
    ControllerDisconnected {
        caller: Caller,
    },
}

struct Shared {
    token: PairingToken,
    instance_id: String,
    commands: Sender<ManagedInput>,
    events: Arc<EventBus>,
    audio: Arc<AudioHub>,
    leases: Arc<LeaseRegistry>,
    shutdown: AtomicBool,
}

pub struct ManagedBridge {
    addr: SocketAddr,
    shared: Arc<Shared>,
    join: Option<JoinHandle<()>>,
}

impl ManagedBridge {
    pub fn bind(
        addr: SocketAddr,
        token: PairingToken,
        commands: Sender<ManagedInput>,
        events: Arc<EventBus>,
        audio: Arc<AudioHub>,
    ) -> io::Result<Self> {
        if !addr.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "control must be loopback",
            ));
        }
        let snapshot = events.latest().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "initial snapshot required")
        })?;
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            token,
            leases: Arc::new(LeaseRegistry::new(snapshot.instance_id.clone())),
            instance_id: snapshot.instance_id,
            commands,
            events,
            audio,
            shutdown: AtomicBool::new(false),
        });
        let running = Arc::clone(&shared);
        let join = thread::Builder::new()
            .name("managed_bridge".into())
            .spawn(move || {
                let mut connections: Vec<JoinHandle<()>> = vec![];
                while !running.shutdown.load(Ordering::Acquire) {
                    let mut index = 0;
                    while index < connections.len() {
                        if connections[index].is_finished() {
                            let _ = connections.swap_remove(index).join();
                        } else {
                            index += 1;
                        }
                    }
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if connections.len() >= 16 {
                                continue;
                            }
                            let shared = Arc::clone(&running);
                            if let Ok(join) = thread::Builder::new()
                                .name("managed_controller".into())
                                .spawn(move || connection(stream, &shared))
                            {
                                connections.push(join);
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5))
                        }
                        Err(_) => {
                            running.shutdown.store(true, Ordering::Release);
                            break;
                        }
                    }
                }
                for connection in connections {
                    let _ = connection.join();
                }
            })?;
        Ok(Self {
            addr,
            shared,
            join: Some(join),
        })
    }

    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    #[must_use]
    pub fn leases(&self) -> Arc<LeaseRegistry> {
        Arc::clone(&self.shared.leases)
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        !self.shared.shutdown.load(Ordering::Acquire)
    }

    pub fn shutdown(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
impl Drop for ManagedBridge {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum HelloTag {
    Hello,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ControlHello {
    #[serde(rename = "type")]
    _kind: HelloTag,
    version: u16,
    token: String,
    role: Role,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProducerHello {
    #[serde(rename = "type")]
    _kind: HelloTag,
    version: u16,
    token: String,
    instance_id: String,
    lease_id: String,
    audio: AudioHello,
}

fn send_json(socket: &mut WebSocket<TcpStream>, value: Value) -> bool {
    socket.send(Message::Text(value.to_string().into())).is_ok()
}

fn fail(socket: &mut WebSocket<TcpStream>, code: &str) {
    let _ = send_json(
        socket,
        json!({"type":"error", "code":code, "terminal":true}),
    );
    let _ = socket.close(None);
}

fn extension_origin(request: &Request) -> bool {
    request
        .headers()
        .get(ORIGIN)
        .and_then(|origin| origin.to_str().ok())
        .and_then(|origin| origin.strip_prefix("chrome-extension://"))
        .is_some_and(|id| id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte)))
}

fn connection(stream: TcpStream, shared: &Arc<Shared>) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
    let config = WebSocketConfig::default()
        .read_buffer_size(64 * 1024)
        .write_buffer_size(0)
        .max_write_buffer_size(128 * 1024)
        .max_message_size(Some(MAX_TEXT_BYTES))
        .max_frame_size(Some(MAX_TEXT_BYTES));
    let mut path = String::new();
    let mut browser_origin = false;
    #[allow(clippy::result_large_err)]
    let callback = |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
        browser_origin = extension_origin(request);
        path = request.uri().path().into();
        let allowed = request.uri().query().is_none()
            && match path.as_str() {
                "/control/v1" => browser_origin || !request.headers().contains_key(ORIGIN),
                "/audio/v1" => browser_origin,
                _ => false,
            };
        if allowed {
            Ok(response)
        } else {
            Err(Response::builder()
                .status(StatusCode::FORBIDDEN)
                .body(Some("forbidden".into()))
                .unwrap())
        }
    };
    let Ok(mut socket) = accept_hdr_with_config(stream, callback, Some(config)) else {
        return;
    };
    let _ = socket
        .get_mut()
        .set_read_timeout(Some(Duration::from_millis(25)));
    let deadline = Instant::now() + Duration::from_secs(2);
    let hello = loop {
        if shared.shutdown.load(Ordering::Acquire) {
            return;
        }
        if Instant::now() >= deadline {
            fail(&mut socket, "hello-timeout");
            return;
        }
        match socket.read() {
            Ok(Message::Text(text)) => break text,
            Ok(Message::Ping(_)) => {
                let _ = socket.flush();
            }
            Err(tungstenite::Error::Io(error)) if is_timeout(&error) => {}
            Ok(_) => {
                fail(&mut socket, "hello-required");
                return;
            }
            Err(_) => return,
        }
    };
    match path.as_str() {
        "/control/v1" => control(socket, shared, &hello, browser_origin),
        "/audio/v1" => producer(socket, shared, &hello),
        _ => {}
    }
}

fn is_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

fn lifecycle(shared: &Shared, input: ManagedInput) -> bool {
    if shared
        .commands
        .send_timeout(input, Duration::from_millis(250))
        .is_err()
    {
        // Losing a capture-disconnect event would leave unsafe ownership state.
        // Force runtime shutdown instead of silently dropping that event.
        shared.shutdown.store(true, Ordering::Release);
        return false;
    }
    true
}

struct ControlGuard<'a> {
    shared: &'a Shared,
    caller: Caller,
}
impl Drop for ControlGuard<'_> {
    fn drop(&mut self) {
        if self.caller.role == Role::Browser {
            self.shared
                .leases
                .unregister_browser(&self.caller.connection_id);
            lifecycle(
                self.shared,
                ManagedInput::Event(Event::BrowserDisconnected {
                    owner: self.caller.connection_id.clone(),
                }),
            );
        }
        lifecycle(
            self.shared,
            ManagedInput::ControllerDisconnected {
                caller: self.caller.clone(),
            },
        );
    }
}

struct Pending {
    request_id: String,
    deadline: Instant,
    reply: Receiver<CommandResult>,
}

fn control(mut socket: WebSocket<TcpStream>, shared: &Shared, text: &str, browser_origin: bool) {
    let Ok(hello) = serde_json::from_str::<ControlHello>(text) else {
        fail(&mut socket, "invalid-hello");
        return;
    };
    if hello.version != 1 {
        fail(&mut socket, "unsupported-version");
        return;
    }
    if !shared.token.matches_hex(&hello.token) {
        fail(&mut socket, "authentication-failed");
        return;
    }
    if hello.role == Role::Browser && !browser_origin {
        fail(&mut socket, "browser-origin-required");
        return;
    }
    let Ok(connection_id) = random_id() else {
        fail(&mut socket, "identity-unavailable");
        return;
    };
    let caller = Caller {
        connection_id,
        role: hello.role,
    };
    if caller.role == Role::Browser
        && shared
            .leases
            .register_browser(&caller.connection_id)
            .is_err()
    {
        fail(&mut socket, "browser-busy");
        return;
    }
    let _guard = ControlGuard {
        shared,
        caller: caller.clone(),
    };
    if caller.role == Role::Browser
        && !lifecycle(
            shared,
            ManagedInput::Event(Event::BrowserConnected {
                owner: caller.connection_id.clone(),
            }),
        )
    {
        return;
    }
    let events = shared.events.subscribe_for(&caller.connection_id);
    if !send_json(
        &mut socket,
        json!({"type":"hello-ok", "version":1, "instanceId":shared.instance_id,"connectionId":caller.connection_id}),
    ) {
        return;
    }
    let mut pending: Vec<Pending> = vec![];
    while !shared.shutdown.load(Ordering::Acquire) {
        loop {
            match events.try_recv() {
                Ok(event) => {
                    let Ok(json) = event.to_json() else {
                        return;
                    };
                    if socket.send(Message::Text(json.into())).is_err() {
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    fail(&mut socket, "controller-overflow");
                    return;
                }
            }
        }
        let mut index = 0;
        while index < pending.len() {
            let outcome = pending[index].reply.try_recv();
            match outcome {
                Ok(result) => {
                    pending.swap_remove(index);
                    if !send_json(&mut socket, serde_json::to_value(result).unwrap()) {
                        return;
                    }
                }
                Err(error)
                    if error == TryRecvError::Disconnected
                        || Instant::now() >= pending[index].deadline =>
                {
                    let request = pending.swap_remove(index);
                    if !send_json(
                        &mut socket,
                        json!({"type":"result","requestId":request.request_id,"status":"failed","code":"outcome-unknown","revision":shared.events.latest().map_or(1, |state| state.revision)}),
                    ) {
                        return;
                    }
                }
                Err(_) => index += 1,
            }
        }
        match socket.read() {
            Ok(Message::Text(text)) => {
                let command = match parse_command(&text) {
                    Ok(command) => command,
                    Err(code) => {
                        fail(&mut socket, code);
                        return;
                    }
                };
                if pending.len() >= 16 {
                    fail(&mut socket, "too-many-requests");
                    return;
                }
                let (reply, result) = bounded(1);
                let request_id = command.request_id.clone();
                if shared
                    .commands
                    .send_timeout(
                        ManagedInput::Command {
                            caller: caller.clone(),
                            command,
                            reply,
                        },
                        Duration::from_millis(100),
                    )
                    .is_err()
                {
                    fail(&mut socket, "controller-busy");
                    return;
                }
                pending.push(Pending {
                    request_id,
                    deadline: Instant::now() + Duration::from_secs(15),
                    reply: result,
                });
            }
            Ok(Message::Close(_)) => {
                let _ = socket.flush();
                return;
            }
            Ok(Message::Ping(_)) => {
                let _ = socket.flush();
            }
            Ok(Message::Pong(_)) => {}
            Ok(_) => {
                fail(&mut socket, "audio-not-allowed");
                return;
            }
            Err(tungstenite::Error::Io(error)) if is_timeout(&error) => {}
            Err(_) => return,
        }
    }
    let _ = socket.close(None);
}

struct ProducerGuard<'a> {
    shared: &'a Shared,
    owner: String,
    generation: u64,
    lease: String,
}
impl Drop for ProducerGuard<'_> {
    fn drop(&mut self) {
        self.shared.leases.release(&self.lease);
        lifecycle(
            self.shared,
            ManagedInput::Event(Event::ProducerDisconnected {
                owner: self.owner.clone(),
                generation: self.generation,
            }),
        );
    }
}

fn producer(mut socket: WebSocket<TcpStream>, shared: &Shared, text: &str) {
    let Ok(hello) = serde_json::from_str::<ProducerHello>(text) else {
        fail(&mut socket, "invalid-hello");
        return;
    };
    if hello.version != 1 {
        fail(&mut socket, "unsupported-version");
        return;
    }
    if !shared.token.matches_hex(&hello.token) {
        fail(&mut socket, "authentication-failed");
        return;
    }
    let session = AudioSession::v1();
    if hello.audio != session.hello() {
        fail(&mut socket, "unsupported-audio");
        return;
    }
    let grant = match shared
        .leases
        .attach(&hello.instance_id, &hello.lease_id, Instant::now())
    {
        Ok(grant) => grant,
        Err(_) => {
            fail(&mut socket, "invalid-lease");
            return;
        }
    };
    let guard = ProducerGuard {
        shared,
        owner: grant.owner,
        generation: grant.generation,
        lease: hello.lease_id,
    };
    let Ok(sink) = shared.audio.sink(guard.generation) else {
        fail(&mut socket, "retired-generation");
        return;
    };
    if !lifecycle(
        shared,
        ManagedInput::Event(Event::ProducerAttached {
            owner: guard.owner.clone(),
            generation: guard.generation,
            lease_id: guard.lease.clone(),
        }),
    ) {
        return;
    }
    if !send_json(
        &mut socket,
        json!({"type":"hello-ok","version":1,"instanceId":shared.instance_id,"generation":guard.generation,"audio":session}),
    ) {
        return;
    }
    while !shared.shutdown.load(Ordering::Acquire) && shared.leases.is_active(&guard.lease) {
        match socket.read() {
            Ok(Message::Binary(bytes)) => {
                let samples = match decode_packet(&session, &bytes) {
                    Ok(samples) => samples,
                    Err(_) => {
                        fail(&mut socket, "invalid-audio-packet");
                        return;
                    }
                };
                if sink.publish(samples).is_err() {
                    fail(&mut socket, "retired-generation");
                    return;
                }
            }
            Ok(Message::Ping(_)) => {
                let _ = socket.flush();
            }
            Ok(Message::Pong(_)) => {}
            Ok(Message::Close(_)) => {
                let _ = socket.flush();
                return;
            }
            Ok(_) => {
                fail(&mut socket, "binary-audio-required");
                return;
            }
            Err(tungstenite::Error::Io(error)) if is_timeout(&error) => {}
            Err(_) => return,
        }
    }
    let _ = socket.close(None);
}
