use std::{
    io,
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crossbeam_channel::{Receiver, Sender};
use tungstenite::{
    Error as WebSocketError, Message, WebSocket, accept_hdr_with_config,
    handshake::server::{ErrorResponse, Request, Response},
    http::{StatusCode, header::ORIGIN},
    protocol::WebSocketConfig,
};

use crate::browser::{
    audio::BrowserAudioPublisher,
    auth::PairingToken,
    commands::{BrowserCommand, BrowserEvent},
    protocol::{AudioSession, ClientMessage, ServerEvent, parse_client_message, validate_hello},
};
use crate::enums::messages::MessageType;

const BRIDGE_PATH: &str = "/browser/v1";
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CONNECTION_READ_TIMEOUT: Duration = Duration::from_millis(25);

#[derive(Debug, Clone)]
pub struct BrowserBridgeConfig {
    pub addr: SocketAddr,
    pub token: PairingToken,
    pub max_text_bytes: usize,
    pub max_binary_bytes: usize,
}

pub struct BrowserBridge;

#[derive(Debug)]
pub struct BrowserBridgeHandle {
    addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl BrowserBridgeHandle {
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for BrowserBridgeHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl BrowserBridge {
    pub fn spawn(
        config: BrowserBridgeConfig,
        command_tx: Sender<MessageType>,
        event_rx: Receiver<BrowserEvent>,
    ) -> io::Result<BrowserBridgeHandle> {
        if !config.addr.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "browser bridge must bind to a loopback address",
            ));
        }

        let listener = TcpListener::bind(config.addr)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = Arc::clone(&shutdown);
        let producer_active = Arc::new(AtomicBool::new(false));
        let next_producer_generation = Arc::new(AtomicU64::new(1));
        let join = thread::Builder::new()
            .name("browser_bridge".to_string())
            .spawn(move || {
                let mut connections = Vec::new();
                while !thread_shutdown.load(Ordering::Acquire) {
                    reap_finished_connections(&mut connections);
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let connection_config = config.clone();
                            let connection_commands = command_tx.clone();
                            let connection_events = event_rx.clone();
                            let connection_shutdown = Arc::clone(&thread_shutdown);
                            let connection_producer = Arc::clone(&producer_active);
                            let connection_generation = Arc::clone(&next_producer_generation);
                            if let Ok(join) = thread::Builder::new()
                                .name("browser_bridge_connection".to_string())
                                .spawn(move || {
                                    run_connection(
                                        stream,
                                        connection_config,
                                        connection_commands,
                                        connection_events,
                                        connection_shutdown,
                                        connection_producer,
                                        connection_generation,
                                    );
                                })
                            {
                                connections.push(join);
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(ACCEPT_POLL_INTERVAL);
                        }
                        Err(_) => break,
                    }
                }
                for connection in connections {
                    let _ = connection.join();
                }
            })?;

        Ok(BrowserBridgeHandle {
            addr,
            shutdown,
            join: Some(join),
        })
    }
}

fn reap_finished_connections(connections: &mut Vec<JoinHandle<()>>) {
    let mut index = 0;
    while index < connections.len() {
        if connections[index].is_finished() {
            let connection = connections.swap_remove(index);
            let _ = connection.join();
        } else {
            index += 1;
        }
    }
}

struct ProducerLease {
    active: Arc<AtomicBool>,
    generation: u64,
    command_tx: Sender<MessageType>,
}

impl ProducerLease {
    fn acquire(
        active: &Arc<AtomicBool>,
        next_generation: &AtomicU64,
        command_tx: &Sender<MessageType>,
    ) -> Option<Self> {
        active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self {
                active: Arc::clone(active),
                generation: next_generation.fetch_add(1, Ordering::Relaxed),
                command_tx: command_tx.clone(),
            })
    }
}

impl Drop for ProducerLease {
    fn drop(&mut self) {
        let _ = self
            .command_tx
            .send(MessageType::BrowserProducerDisconnected(self.generation));
        self.active.store(false, Ordering::Release);
    }
}

fn run_connection(
    stream: TcpStream,
    config: BrowserBridgeConfig,
    command_tx: Sender<MessageType>,
    event_rx: Receiver<BrowserEvent>,
    shutdown: Arc<AtomicBool>,
    producer_active: Arc<AtomicBool>,
    next_producer_generation: Arc<AtomicU64>,
) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let message_limit = config.max_text_bytes.max(config.max_binary_bytes);
    let websocket_config = WebSocketConfig::default()
        .read_buffer_size(message_limit.min(64 * 1024))
        .write_buffer_size(0)
        .max_write_buffer_size(message_limit.saturating_mul(2).max(1))
        .max_message_size(Some(message_limit))
        .max_frame_size(Some(message_limit));
    #[allow(clippy::result_large_err)]
    let callback = |request: &Request, response: Response| {
        if request.uri().path() != BRIDGE_PATH || !has_extension_origin(request) {
            return Err(forbidden_response());
        }
        Ok(response)
    };
    let Ok(mut socket) = accept_hdr_with_config(stream, callback, Some(websocket_config)) else {
        return;
    };
    let _ = socket
        .get_mut()
        .set_read_timeout(Some(CONNECTION_READ_TIMEOUT));

    let publisher = BrowserAudioPublisher::new(AudioSession::v1());
    let mut producer_lease: Option<ProducerLease> = None;

    while !shutdown.load(Ordering::Acquire) {
        if producer_lease.is_some() {
            while let Ok(event) = event_rx.try_recv() {
                if send_event(&mut socket, &event).is_err() {
                    return;
                }
            }
        }

        match socket.read() {
            Ok(Message::Text(text)) => {
                if text.len() > config.max_text_bytes {
                    send_terminal_error(&mut socket, "message-too-large", "text message too large");
                    return;
                }
                let message = match parse_client_message(text.as_str()) {
                    Ok(message) => message,
                    Err(error) => {
                        send_terminal_error(&mut socket, error.code(), "invalid control message");
                        return;
                    }
                };

                if let Some(lease) = &producer_lease {
                    if let Some(command) =
                        BrowserCommand::from_client_message(message, lease.generation)
                    {
                        if command_tx
                            .send(MessageType::BrowserCommand(command))
                            .is_err()
                        {
                            send_terminal_error(
                                &mut socket,
                                "bridge-stopping",
                                "command receiver unavailable",
                            );
                            return;
                        }
                    } else {
                        send_terminal_error(
                            &mut socket,
                            "unexpected-hello",
                            "already authenticated",
                        );
                        return;
                    }
                } else {
                    let ClientMessage::Hello(hello) = message else {
                        send_terminal_error(&mut socket, "hello-required", "authenticate first");
                        return;
                    };
                    let session = match validate_hello(&hello, &config.token) {
                        Ok(session) => session,
                        Err(error) => {
                            send_terminal_error(&mut socket, error.code(), "hello rejected");
                            return;
                        }
                    };
                    let Some(lease) = ProducerLease::acquire(
                        &producer_active,
                        &next_producer_generation,
                        &command_tx,
                    ) else {
                        send_terminal_error(
                            &mut socket,
                            "producer-busy",
                            "another browser producer is active",
                        );
                        return;
                    };
                    producer_lease = Some(lease);
                    if send_event(&mut socket, &ServerEvent::HelloOk { audio: session }).is_err() {
                        return;
                    }
                }
            }
            Ok(Message::Binary(bytes)) => {
                if producer_lease.is_none() {
                    let _ = socket.close(None);
                    return;
                }
                if bytes.len() > config.max_binary_bytes {
                    send_terminal_error(
                        &mut socket,
                        "message-too-large",
                        "binary message too large",
                    );
                    return;
                }
                if publisher.publish(&bytes).is_err() {
                    send_terminal_error(&mut socket, "invalid-audio", "invalid audio packet");
                    return;
                }
            }
            Ok(Message::Ping(_)) => {
                let _ = socket.flush();
            }
            Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {}
            Ok(Message::Close(_)) => return,
            Err(WebSocketError::Io(error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(WebSocketError::ConnectionClosed | WebSocketError::AlreadyClosed) => return,
            Err(_) => return,
        }
    }
    let _ = socket.close(None);
}

fn has_extension_origin(request: &Request) -> bool {
    request
        .headers()
        .get(ORIGIN)
        .and_then(|value| value.to_str().ok())
        .and_then(|origin| origin.strip_prefix("chrome-extension://"))
        .is_some_and(|id| id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte)))
}

fn forbidden_response() -> ErrorResponse {
    let mut response = ErrorResponse::new(Some("Forbidden".to_string()));
    *response.status_mut() = StatusCode::FORBIDDEN;
    response
}

fn send_event(
    socket: &mut WebSocket<TcpStream>,
    event: &ServerEvent,
) -> Result<(), WebSocketError> {
    let text = serde_json::to_string(event)
        .map_err(|error| WebSocketError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    socket.send(Message::Text(text.into()))
}

fn send_terminal_error(socket: &mut WebSocket<TcpStream>, code: &str, message: &str) {
    let _ = send_event(
        socket,
        &ServerEvent::Error {
            request_id: None,
            code: code.to_string(),
            message: message.to_string(),
            terminal: true,
        },
    );
}

#[cfg(test)]
mod tests {
    use std::{
        net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
        thread,
        time::{Duration, Instant},
    };

    use crossbeam_channel::{Receiver, Sender, unbounded};
    use tungstenite::{
        Error, Message, WebSocket,
        client::IntoClientRequest,
        connect,
        http::{HeaderValue, StatusCode, header::ORIGIN},
        stream::MaybeTlsStream,
    };

    use super::{
        BrowserBridge, BrowserBridgeConfig, BrowserBridgeHandle, reap_finished_connections,
    };
    use crate::browser::{
        auth::PairingToken,
        commands::{BrowserCommand, BrowserCommandKind, BrowserEvent},
        protocol::{AudioSession, ClientMessage, Hello, ServerEvent},
    };

    type TestSocket = WebSocket<MaybeTlsStream<TcpStream>>;

    struct BridgeHarness {
        handle: BrowserBridgeHandle,
        token: PairingToken,
        command_rx: Receiver<crate::enums::messages::MessageType>,
        _event_tx: Sender<BrowserEvent>,
    }

    impl BridgeHarness {
        fn start() -> Self {
            let token = PairingToken::from_bytes([9; 32]);
            let (command_tx, command_rx) = unbounded();
            let (event_tx, event_rx) = unbounded();
            let handle = BrowserBridge::spawn(
                BrowserBridgeConfig {
                    addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)),
                    token: token.clone(),
                    max_text_bytes: 64 * 1024,
                    max_binary_bytes: 64 * 1024,
                },
                command_tx,
                event_rx,
            )
            .expect("browser bridge");
            Self {
                handle,
                token,
                command_rx,
                _event_tx: event_tx,
            }
        }

        fn url(&self) -> String {
            format!("ws://{}/browser/v1", self.handle.local_addr())
        }

        fn connect_with_origin(&self, origin: &str) -> Result<TestSocket, Error> {
            let mut request = self.url().into_client_request().expect("client request");
            request
                .headers_mut()
                .insert(ORIGIN, HeaderValue::from_str(origin).expect("origin"));
            connect(request).map(|(mut socket, _)| {
                if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .expect("read timeout");
                }
                socket
            })
        }

        fn extension_client(&self) -> TestSocket {
            self.connect_with_origin("chrome-extension://abcdefghijklmnopabcdefghijklmnop")
                .expect("extension client")
        }

        fn hello_json(&self) -> String {
            serde_json::to_string(&ClientMessage::Hello(Hello {
                version: 1,
                token: self.token.to_hex(),
                audio: AudioSession::v1().hello(),
            }))
            .expect("hello JSON")
        }

        fn authenticated_client(&self) -> TestSocket {
            let mut socket = self.extension_client();
            socket
                .send(Message::Text(self.hello_json().into()))
                .expect("send hello");
            assert!(matches!(
                read_event(&mut socket),
                ServerEvent::HelloOk { .. }
            ));
            socket
        }
    }

    impl Drop for BridgeHarness {
        fn drop(&mut self) {
            self.handle.shutdown();
        }
    }

    fn read_event(socket: &mut TestSocket) -> ServerEvent {
        let Message::Text(text) = socket.read().expect("server event") else {
            panic!("expected text server event");
        };
        serde_json::from_str(&text).expect("valid server event")
    }

    #[test]
    fn bridge_rejects_web_origin_and_requires_hello_before_binary() {
        let harness = BridgeHarness::start();
        let rejected = harness
            .connect_with_origin("https://example.com")
            .expect_err("web origin must be rejected");
        let Error::Http(response) = rejected else {
            panic!("expected HTTP rejection, got {rejected:?}");
        };
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let mut socket = harness.extension_client();
        socket
            .send(Message::Binary(vec![0; 3840].into()))
            .expect("binary frame");
        assert!(matches!(socket.read(), Err(_) | Ok(Message::Close(_))));
    }

    #[test]
    fn second_producer_is_rejected_until_first_disconnects() {
        let harness = BridgeHarness::start();
        let first = harness.authenticated_client();

        let mut second = harness.extension_client();
        second
            .send(Message::Text(harness.hello_json().into()))
            .expect("second hello");
        assert!(matches!(
            read_event(&mut second),
            ServerEvent::Error { code, terminal: true, .. } if code == "producer-busy"
        ));

        drop(first);
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let mut replacement = harness.extension_client();
            replacement
                .send(Message::Text(harness.hello_json().into()))
                .expect("replacement hello");
            match read_event(&mut replacement) {
                ServerEvent::HelloOk { .. } => break,
                ServerEvent::Error { code, .. }
                    if code == "producer-busy" && Instant::now() < deadline =>
                {
                    thread::yield_now();
                }
                event => panic!("unexpected replacement response: {event:?}"),
            }
        }
    }

    #[test]
    fn authenticated_command_is_forwarded_with_request_id() {
        let harness = BridgeHarness::start();
        let mut socket = harness.authenticated_client();
        socket
            .send(Message::Text(
                r#"{"type":"list-renderers","requestId":"renderers-1"}"#.into(),
            ))
            .expect("list command");

        let crate::enums::messages::MessageType::BrowserCommand(command) = harness
            .command_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("forwarded command")
        else {
            panic!("expected browser command");
        };
        assert_eq!(
            command,
            BrowserCommand {
                request_id: "renderers-1".to_string(),
                kind: BrowserCommandKind::ListRenderers,
                producer_generation: 1,
            }
        );
    }

    #[test]
    fn authenticated_disconnect_is_forwarded_with_its_generation() {
        let harness = BridgeHarness::start();
        let socket = harness.authenticated_client();
        drop(socket);

        assert!(matches!(
            harness.command_rx.recv_timeout(Duration::from_secs(1)),
            Ok(crate::enums::messages::MessageType::BrowserProducerDisconnected(1))
        ));
    }

    #[test]
    fn bridge_shutdown_is_idempotent() {
        let mut harness = BridgeHarness::start();

        harness.handle.shutdown();
        harness.handle.shutdown();
    }

    #[test]
    fn bridge_refuses_non_loopback_bind_address() {
        let token = PairingToken::from_bytes([4; 32]);
        let (command_tx, _) = unbounded();
        let (_, event_rx) = unbounded();

        let result = BrowserBridge::spawn(
            BrowserBridgeConfig {
                addr: "0.0.0.0:0".parse().unwrap(),
                token,
                max_text_bytes: 64 * 1024,
                max_binary_bytes: 64 * 1024,
            },
            command_tx,
            event_rx,
        );

        assert_eq!(
            result.expect_err("LAN bind must be rejected").kind(),
            std::io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn finished_connection_threads_are_reaped() {
        let mut handles = vec![thread::spawn(|| {})];
        handles[0].thread().unpark();
        let deadline = Instant::now() + Duration::from_secs(1);
        while !handles[0].is_finished() && Instant::now() < deadline {
            thread::yield_now();
        }

        reap_finished_connections(&mut handles);

        assert!(handles.is_empty());
    }
}
