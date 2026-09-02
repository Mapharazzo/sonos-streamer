use std::{
    fs,
    io::Read,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::Arc,
    thread,
};

use crossbeam_channel::{bounded, unbounded};
use swyh_rs::{
    browser::{
        auth::PairingToken,
        bridge::{BrowserBridge, BrowserBridgeConfig},
        protocol::{AudioSession, ClientMessage, Hello, ServerEvent},
    },
    enums::{messages::MessageType, streaming::StreamingFormat},
    globals::statics::get_config_mut,
    openhome::rendercontrol::WavData,
    server::streaming_server::run_server_instance,
};
use tiny_http::Server;
use tungstenite::{Message, client::IntoClientRequest, http::header::ORIGIN};

#[test]
fn browser_pcm_is_served_as_wav_to_http_consumer() {
    let config_dir = std::env::temp_dir().join(format!(
        "swyh-rs-browser-integration-{}",
        std::process::id()
    ));
    fs::create_dir_all(&config_dir).expect("test config directory");
    let config_path = config_dir.join("config.toml");
    fs::write(
        &config_path,
        format!(
            "[configuration]\nConfigDir = {:?}\nConfigId = \"_integration\"\nReadOnly = true\n",
            config_dir
        ),
    )
    .expect("test config");
    // This integration test is the only test in its process and sets the override before
    // initializing global configuration or starting any worker threads.
    unsafe { std::env::set_var("SWYH_RS_CONFIGFILE", &config_path) };

    let mut config = get_config_mut();
    config.streaming_format = Some(StreamingFormat::Wav);
    config.bits_per_sample = Some(16);
    config.wav_stream_size = Some(swyh_rs::enums::streaming::StreamSize::U32maxChunked);
    config.capture_timeout = Some(50);
    drop(config);

    let server = Arc::new(Server::http("127.0.0.1:0").expect("HTTP server"));
    let http_addr = server.server_addr().to_ip().expect("TCP server address");
    let (feedback_tx, _feedback_rx) = unbounded::<MessageType>();
    let (shutdown_tx, shutdown_rx) = bounded::<()>(1);
    let server_thread = {
        let server = Arc::clone(&server);
        thread::spawn(move || {
            run_server_instance(
                server,
                WavData {
                    sample_format: cpal::SampleFormat::F32,
                    sample_rate: 48_000,
                    channels: 2,
                },
                feedback_tx,
                shutdown_rx,
            );
        })
    };

    let token = PairingToken::from_bytes([5; 32]);
    let (command_tx, _command_rx) = unbounded::<MessageType>();
    let (_event_tx, event_rx) = unbounded::<ServerEvent>();
    let mut bridge = BrowserBridge::spawn(
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

    let bridge_url = format!("ws://{}/browser/v1", bridge.local_addr());
    let mut request = bridge_url.into_client_request().expect("WebSocket request");
    request.headers_mut().insert(
        ORIGIN,
        "chrome-extension://abcdefghijklmnopabcdefghijklmnop"
            .parse()
            .unwrap(),
    );
    let (mut websocket, _) = tungstenite::connect(request).expect("WebSocket connect");
    let hello = ClientMessage::Hello(Hello {
        version: 1,
        token: token.to_hex(),
        audio: AudioSession::v1().hello(),
    });
    websocket
        .send(Message::Text(serde_json::to_string(&hello).unwrap().into()))
        .unwrap();
    assert!(matches!(websocket.read().unwrap(), Message::Text(_)));

    let response = ureq::get(format!("http://{http_addr}/stream/swyh.wav"))
        .call()
        .expect("WAV response");
    assert_eq!(
        response.headers().get("Content-Type").unwrap(),
        "audio/vnd.wave;codec=1"
    );

    websocket
        .send(Message::Binary(stereo_s16_packet(1000, -1000).into()))
        .unwrap();
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(4_000)
        .read_to_end(&mut bytes)
        .expect("WAV bytes");
    assert_eq!(&bytes[0..4], b"RIFF");
    assert!(bytes[44..].iter().any(|byte| *byte != 0));

    let _ = websocket.close(None);
    bridge.shutdown();
    shutdown_tx.send(()).unwrap();
    server_thread.join().unwrap();

    fs::remove_file(config_path).unwrap();
    fs::remove_dir(config_dir).unwrap();
}

fn stereo_s16_packet(left: i16, right: i16) -> Vec<u8> {
    let mut packet = Vec::with_capacity(AudioSession::v1().packet_bytes);
    for _ in 0..960 {
        packet.extend_from_slice(&left.to_le_bytes());
        packet.extend_from_slice(&right.to_le_bytes());
    }
    packet
}
