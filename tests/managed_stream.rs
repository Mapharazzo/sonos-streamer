use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::{Duration, Instant},
};
use swyh_rs::{
    managed::{
        audio::{AudioError, AudioHub},
        streaming::{ManagedHttp, StreamEventKind},
    },
    openhome::rendercontrol::WavData,
};

fn format(rate: u32) -> WavData {
    WavData {
        sample_format: cpal::SampleFormat::F32,
        sample_rate: rate,
        channels: 2,
    }
}

fn request(addr: SocketAddr, path: &str) -> (TcpStream, String) {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        headers.push(byte[0]);
        assert!(headers.len() < 8192);
    }
    (stream, String::from_utf8(headers).unwrap())
}

#[test]
fn generations_cannot_publish_or_consume_each_others_audio() {
    let hub = AudioHub::new();
    let old = hub.begin(1, format(44100)).unwrap();
    let subscription = hub.subscribe(1).unwrap();
    old.publish(Arc::new(vec![0.25, -0.25])).unwrap();
    hub.end(1);
    let new = hub.begin(2, format(48000)).unwrap();
    assert_eq!(
        old.publish(Arc::new(vec![0.5, -0.5])),
        Err(AudioError::Retired)
    );
    assert!(hub.subscribe(1).is_err());
    assert!(
        subscription
            .recv_timeout(Duration::from_millis(20))
            .is_err()
    );
    let current = hub.subscribe(2).unwrap();
    new.publish(Arc::new(vec![0.75, -0.75])).unwrap();
    assert_eq!(
        &*current.recv_timeout(Duration::from_secs(1)).unwrap(),
        &[0.75, -0.75]
    );
    assert!(hub.begin(1, format(48000)).is_err());
}

#[test]
fn slow_consumers_have_bounded_queues_and_cannot_block_capture() {
    let hub = AudioHub::new();
    let sink = hub.begin(1, format(48000)).unwrap();
    let slow = hub.subscribe(1).unwrap();
    let start = Instant::now();
    for _ in 0..1000 {
        sink.publish(Arc::new(vec![0.25; 1920])).unwrap();
    }
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(sink.dropped_packets() > 0);
    let mut received = 0;
    while slow.recv_timeout(Duration::from_millis(1)).is_ok() {
        received += 1;
    }
    assert!(received <= 8, "queue retained {received} packets");
}

#[test]
fn malformed_audio_and_unsupported_formats_are_rejected() {
    let hub = AudioHub::new();
    assert!(hub.begin(0, format(48000)).is_err());
    let mut surround = format(48000);
    surround.channels = 6;
    assert!(hub.begin(1, surround).is_err());
    let sink = hub.begin(1, format(48000)).unwrap();
    assert_eq!(
        sink.publish(Arc::new(vec![0.5])),
        Err(AudioError::PartialFrame)
    );
    assert_eq!(
        sink.publish(Arc::new(vec![0.0; 65536])),
        Err(AudioError::TooLarge)
    );
}

#[test]
fn wav_headers_match_each_generation_and_pcm_is_clamped() {
    let hub = Arc::new(AudioHub::new());
    let mut server = ManagedHttp::bind(
        "127.0.0.1:0".parse().unwrap(),
        "instance-1".into(),
        Arc::clone(&hub),
    )
    .unwrap();
    for (generation, rate) in [(1, 44100), (2, 48000)] {
        let sink = hub.begin(generation, format(rate)).unwrap();
        let (mut reader, headers) = request(
            server.local_addr(),
            &format!("/managed/instance-1/{generation}/stream.wav"),
        );
        assert!(headers.starts_with("HTTP/1.1 200"));
        assert!(headers.contains("Content-Type: audio/wav"));
        let event = server
            .events()
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert_eq!(event.kind, StreamEventKind::Started);
        assert_eq!(event.generation, generation);
        let mut wav = [0; 44];
        reader.read_exact(&mut wav).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), rate);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 2);
        sink.publish(Arc::new(vec![
            0.5,
            -0.5,
            2.0,
            -2.0,
            f32::NAN,
            f32::INFINITY,
        ]))
        .unwrap();
        let mut pcm = [0; 12];
        reader.read_exact(&mut pcm).unwrap();
        assert_eq!(pcm, [0, 64, 0, 192, 255, 127, 0, 128, 0, 0, 0, 0]);
        hub.end(generation);
        assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
        let event = server
            .events()
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert_eq!(event.kind, StreamEventKind::Ended);
    }
    server.shutdown();
}

#[test]
fn retired_urls_and_unknown_instances_never_receive_a_new_stream() {
    let hub = Arc::new(AudioHub::new());
    let mut server = ManagedHttp::bind(
        "127.0.0.1:0".parse().unwrap(),
        "instance-1".into(),
        Arc::clone(&hub),
    )
    .unwrap();
    let _old = hub.begin(1, format(44100)).unwrap();
    hub.end(1);
    let _new = hub.begin(2, format(48000)).unwrap();
    assert!(
        request(server.local_addr(), "/managed/instance-1/1/stream.wav")
            .1
            .starts_with("HTTP/1.1 410")
    );
    assert!(
        request(server.local_addr(), "/managed/other/2/stream.wav")
            .1
            .starts_with("HTTP/1.1 404")
    );
    assert!(
        request(server.local_addr(), "/managed/instance-1/100/stream.wav")
            .1
            .starts_with("HTTP/1.1 404")
    );
    server.shutdown();
}

#[test]
fn shutdown_cancels_idle_and_nonreading_renderer_connections() {
    let hub = Arc::new(AudioHub::new());
    let mut server = ManagedHttp::bind(
        "127.0.0.1:0".parse().unwrap(),
        "instance-1".into(),
        Arc::clone(&hub),
    )
    .unwrap();
    let sink = hub.begin(1, format(48000)).unwrap();
    let _idle = TcpStream::connect(server.local_addr()).unwrap();
    let (_stalled, _) = request(server.local_addr(), "/managed/instance-1/1/stream.wav");
    let samples = Arc::new(vec![0.5; 32768]);
    for _ in 0..2000 {
        sink.publish(Arc::clone(&samples)).unwrap();
    }
    let started = Instant::now();
    server.shutdown();
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn disconnected_consumers_release_slots_without_waiting_for_audio() {
    let hub = AudioHub::new();
    let _sink = hub.begin(1, format(48000)).unwrap();
    for _ in 0..40 {
        let subscription = hub
            .subscribe(1)
            .expect("slot reclaimed while source is silent");
        drop(subscription);
    }
}

#[test]
fn renderer_buffering_pause_does_not_close_the_stream() {
    let hub = Arc::new(AudioHub::new());
    let mut server = ManagedHttp::bind(
        "127.0.0.1:0".parse().unwrap(),
        "buffering".into(),
        Arc::clone(&hub),
    )
    .unwrap();
    let sink = hub.begin(1, format(48000)).unwrap();
    let (mut reader, _) = request(server.local_addr(), "/managed/buffering/1/stream.wav");
    assert_eq!(
        server
            .events()
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .kind,
        StreamEventKind::Started
    );
    // Fill the TCP buffers without making the test depend on their OS size.
    let producer = std::thread::spawn(move || {
        let samples = Arc::new(vec![0.25; 32768]);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            sink.publish(Arc::clone(&samples)).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    producer.join().unwrap();
    assert!(
        server.events().try_recv().is_err(),
        "buffering is not a disconnect"
    );
    let mut data = [0; 65536];
    let deadline = Instant::now() + Duration::from_millis(400);
    while Instant::now() < deadline {
        assert!(
            reader.read(&mut data).unwrap() > 0,
            "stream ended after buffering"
        );
    }
    server.shutdown();
}
