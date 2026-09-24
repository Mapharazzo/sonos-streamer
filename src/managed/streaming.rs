//! Small bounded HTTP audio server. Cancellable writes tolerate renderer buffering
//! while keeping owned shutdown reliable during an unlimited WAV response.
use super::audio::{AudioError, AudioHub};
use crossbeam_channel::{Receiver, Sender, bounded};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum StreamEventKind {
    Started,
    Ended,
}

#[derive(Debug, Clone)]
pub struct StreamEvent {
    pub kind: StreamEventKind,
    pub generation: u64,
    pub peer: SocketAddr,
}

pub struct ManagedHttp {
    addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    hub: Arc<AudioHub>,
    events: Receiver<StreamEvent>,
    join: Option<JoinHandle<()>>,
}

impl ManagedHttp {
    pub fn bind(addr: SocketAddr, instance_id: String, hub: Arc<AudioHub>) -> io::Result<Self> {
        if instance_id.is_empty()
            || instance_id.len() > 128
            || !instance_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid instance id",
            ));
        }
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let running = Arc::clone(&shutdown);
        let audio = Arc::clone(&hub);
        let (sender, events) = bounded(64);
        let join = thread::Builder::new()
            .name("managed_http".into())
            .spawn(move || {
                let mut workers: Vec<JoinHandle<()>> = vec![];
                while !running.load(Ordering::Acquire) {
                    let mut index = 0;
                    while index < workers.len() {
                        if workers[index].is_finished() {
                            let _ = workers.swap_remove(index).join();
                        } else {
                            index += 1;
                        }
                    }
                    match listener.accept() {
                        Ok((mut socket, peer)) => {
                            let _ = socket.set_read_timeout(Some(Duration::from_millis(250)));
                            let _ = socket.set_write_timeout(Some(Duration::from_millis(250)));
                            let _ = socket.set_nodelay(true);
                            if workers.len() >= 16 {
                                let _ = error_response(&mut socket, 503);
                                continue;
                            }
                            let audio = Arc::clone(&audio);
                            let cancel = Arc::clone(&running);
                            let sender = sender.clone();
                            let instance = instance_id.clone();
                            if let Ok(join) = thread::Builder::new()
                                .name("managed_wav".into())
                                .spawn(move || {
                                    if let Err(error) =
                                        serve(socket, peer, &instance, &audio, &cancel, &sender)
                                    {
                                        // Fixed fields only: never log request URLs or headers.
                                        eprintln!(
                                            "managed_audio_connection_end kind={:?} os_code={:?}",
                                            error.kind(),
                                            error.raw_os_error()
                                        );
                                    }
                                })
                            {
                                workers.push(join);
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5))
                        }
                        Err(_) => break,
                    }
                }
                for worker in workers {
                    let _ = worker.join();
                }
            })?;
        Ok(Self {
            addr,
            shutdown,
            hub,
            events,
            join: Some(join),
        })
    }

    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    #[must_use]
    pub fn events(&self) -> Receiver<StreamEvent> {
        self.events.clone()
    }

    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.hub.end_current();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for ManagedHttp {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn read_request(socket: &mut TcpStream, cancel: &AtomicBool) -> io::Result<String> {
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut bytes = Vec::new();
    while bytes.len() < 8192 && Instant::now() < deadline && !cancel.load(Ordering::Acquire) {
        let mut byte = [0];
        socket.read_exact(&mut byte)?;
        bytes.push(byte[0]);
        if bytes.ends_with(b"\r\n\r\n") {
            return String::from_utf8(bytes)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid HTTP header"));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "incomplete HTTP header",
    ))
}

fn error_response(socket: &mut TcpStream, status: u16) -> io::Result<()> {
    write!(
        socket,
        "HTTP/1.1 {status} Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
}

fn serve(
    mut socket: TcpStream,
    peer: SocketAddr,
    instance: &str,
    hub: &AudioHub,
    cancel: &AtomicBool,
    events: &Sender<StreamEvent>,
) -> io::Result<()> {
    let request = read_request(&mut socket, cancel)?;
    let mut lines = request.lines();
    let fields: Vec<&str> = lines
        .next()
        .unwrap_or("")
        .split_ascii_whitespace()
        .collect();
    if fields.len() != 3 || !matches!(fields[2], "HTTP/1.0" | "HTTP/1.1") {
        return error_response(&mut socket, 400);
    }
    if !matches!(fields[0], "GET" | "HEAD") {
        return error_response(&mut socket, 405);
    }
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("range")
            && value.trim() != "bytes=0-"
        {
            return error_response(&mut socket, 416);
        }
    }
    let prefix = format!("/managed/{instance}/");
    let Some(generation) = fields[1]
        .strip_prefix(&prefix)
        .and_then(|path| path.strip_suffix("/stream.wav"))
        .and_then(|id| id.parse::<u64>().ok())
    else {
        return error_response(&mut socket, 404);
    };
    let subscription = match hub.subscribe(generation) {
        Ok(subscription) => subscription,
        Err(AudioError::SubscriberLimit) => return error_response(&mut socket, 503),
        Err(_) => {
            return error_response(
                &mut socket,
                if generation > 0 && generation <= hub.latest_generation() {
                    410
                } else {
                    404
                },
            );
        }
    };
    let format = subscription.format();
    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: 4294967295\r\nConnection: close\r\nCache-Control: no-store\r\nAccept-Ranges: none\r\ntransferMode.dlna.org: Streaming\r\n\r\n")?;
    if fields[0] == "HEAD" {
        return Ok(());
    }
    socket.write_all(&wav_header(format.sample_rate))?;
    // A blocking Winsock send timeout can leave the connection unusable. Use
    // nonblocking partial writes so normal renderer buffering is recoverable.
    socket.set_write_timeout(None)?;
    socket.set_nonblocking(true)?;
    let _ = events.try_send(StreamEvent {
        kind: StreamEventKind::Started,
        generation,
        peer,
    });
    let outcome = (|| {
        while !cancel.load(Ordering::Acquire) {
            let samples = match subscription.recv_timeout(Duration::from_millis(100)) {
                Ok(samples) => samples,
                Err(AudioError::Timeout) => {
                    // A renderer still needs an audio clock during output silence.
                    write_audio(
                        &mut socket,
                        &vec![0; format.sample_rate as usize / 10 * 4],
                        || cancel.load(Ordering::Acquire) || !subscription.is_active(),
                    )?;
                    continue;
                }
                Err(_) => break,
            };
            let mut bytes = Vec::with_capacity(samples.len() * 2);
            for sample in samples.iter().copied() {
                let pcm = if sample.is_finite() {
                    (sample.clamp(-1.0, 1.0) * 32768.0) as i16
                } else {
                    0
                };
                bytes.extend_from_slice(&pcm.to_le_bytes());
            }
            write_audio(&mut socket, &bytes, || {
                cancel.load(Ordering::Acquire) || !subscription.is_active()
            })?;
        }
        Ok(())
    })();
    let _ = events.try_send(StreamEvent {
        kind: StreamEventKind::Ended,
        generation,
        peer,
    });
    outcome
}

fn write_audio(
    writer: &mut impl Write,
    mut bytes: &[u8],
    cancelled: impl Fn() -> bool,
) -> io::Result<()> {
    let mut deadline = Instant::now() + Duration::from_secs(5);
    while !bytes.is_empty() {
        if cancelled() {
            return Ok(());
        }
        match writer.write(bytes) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(count) => {
                bytes = &bytes[count..];
                deadline = Instant::now() + Duration::from_secs(5);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn wav_header(sample_rate: u32) -> [u8; 44] {
    let mut header = [0; 44];
    header[..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(u32::MAX - 8).to_le_bytes());
    header[8..16].copy_from_slice(b"WAVEfmt ");
    header[16..20].copy_from_slice(&16_u32.to_le_bytes());
    header[20..22].copy_from_slice(&1_u16.to_le_bytes());
    header[22..24].copy_from_slice(&2_u16.to_le_bytes());
    header[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    header[28..32].copy_from_slice(&(sample_rate * 4).to_le_bytes());
    header[32..34].copy_from_slice(&4_u16.to_le_bytes());
    header[34..36].copy_from_slice(&16_u16.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&(u32::MAX - 44).to_le_bytes());
    header
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct BufferingWriter {
        calls: usize,
        bytes: Vec<u8>,
    }
    impl Write for BufferingWriter {
        fn write(&mut self, input: &[u8]) -> io::Result<usize> {
            self.calls += 1;
            if self.calls == 2 {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let count = input.len().min(3);
            self.bytes.extend_from_slice(&input[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn partial_write_retry_preserves_pcm_bytes_exactly() {
        let mut writer = BufferingWriter::default();
        write_audio(&mut writer, b"0123456789", || false).unwrap();
        assert_eq!(writer.bytes, b"0123456789");
    }

    #[test]
    fn retired_stream_stops_before_writing_more_audio() {
        let mut writer = BufferingWriter::default();
        write_audio(&mut writer, b"0123456789", || true).unwrap();
        assert!(writer.bytes.is_empty());
        assert_eq!(writer.calls, 0);
    }
}
