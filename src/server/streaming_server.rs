use crate::{
    debug_agent::agent_log,
    enums::{
        messages::MessageType,
        streaming::{BitDepth, StreamingContext, StreamingFormat, StreamingState},
    },
    globals::statics::get_clients_mut,
    latency::{arm_latency_response_waiter, pulse_trigger_path},
    openhome::rendercontrol::WavData,
    server::query_params::StreamingParams,
    utils::rwstream::ChannelStream,
    utils::ui_logger::{LogCategory, ui_log},
};
use crossbeam_channel::{Receiver, Sender, bounded, never, unbounded};
use ecow::EcoString;
use log::debug;
use std::io::Cursor;
use std::{io, net::IpAddr, sync::Arc, thread, time::Duration};
use tiny_http::{Header, Method, Response, Server};

/// streaming state feedback for a client
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StreamerFeedBack {
    pub remote_ip: EcoString,
    pub streaming_state: StreamingState,
}

/// `run_server` - run a tiny-http webserver to serve streaming requests from renderers
///
/// all music is sent with the sample rate of the source in the requested audio format (lpcm/wav/rf64)
/// in the requested bit depth (16 or 24)
/// the samples are read as f32 slices from a crossbeam channel fed by the `wave_reader`
/// a `ChannelStream` is created for this purpose, and inserted in the array of active
/// "clients" for the `wave_reader`
pub fn run_server(
    local_addr: &IpAddr,
    server_port: u16,
    wd: WavData,
    feedback_tx: &Sender<MessageType>,
) {
    let addr = format!("{local_addr}:{server_port}");
    ui_log(
        LogCategory::Info,
        &format!(
            "The streaming server is listening on http://{addr}/stream/swyh.wav (latency probe: http://{addr}/latency/trigger)"
        ),
    );
    // get the needed config info upfront
    let stream_config = StreamingContext::from_config();
    ui_log(
        LogCategory::Info,
        &format!(
            "Default streaming sample rate: {}, bits per sample: {}, format: {}",
            wd.sample_rate, stream_config.bits_per_sample, stream_config.streaming_format,
        ),
    );

    let server = Arc::new(Server::http(addr).unwrap_or_else(|e| {
        ui_log(
            LogCategory::Error,
            &format!("Error starting server thread: {e}"),
        );
        panic!("Can't start server thread: {e}");
    }));
    run_server_instance(server, wd, feedback_tx.clone(), never());
}

pub fn run_server_instance(
    server: Arc<Server>,
    wd: WavData,
    feedback_tx: Sender<MessageType>,
    shutdown_rx: Receiver<()>,
) {
    let mut handles = Vec::new();
    loop {
        reap_finished_requests(&mut handles);
        if shutdown_rx.try_recv().is_ok() {
            break;
        }
        match server.recv_timeout(Duration::from_millis(25)) {
            Ok(Some(request)) => {
                let request_feedback = feedback_tx.clone();
                handles.push(thread::spawn(move || {
                    handle_request(request, wd, &request_feedback);
                }));
            }
            Ok(None) => {}
            Err(error) => {
                ui_log(
                    LogCategory::Error,
                    &format!("HTTP server receive error: {error}"),
                );
                break;
            }
        }
    }

    for handle in handles {
        if let Err(error) = handle.join() {
            ui_log(
                LogCategory::Error,
                &format!("Server request thread ended with error {error:?}"),
            );
        }
    }
}

fn reap_finished_requests(handles: &mut Vec<thread::JoinHandle<()>>) {
    let mut index = 0;
    while index < handles.len() {
        if handles[index].is_finished() {
            let handle = handles.swap_remove(index);
            if let Err(error) = handle.join() {
                ui_log(
                    LogCategory::Error,
                    &format!("Server request thread ended with error {error:?}"),
                );
            }
        } else {
            index += 1;
        }
    }
}

fn handle_request(rq: tiny_http::Request, wd: WavData, feedback_tx: &Sender<MessageType>) {
    debug!(
        "{:?} {} from {}",
        *rq.method(),
        rq.url(),
        rq.remote_addr().unwrap()
    );
    #[cfg(debug_assertions)]
    dump_rq_headers(&rq);
    let url_path = rq.url().split('?').next().unwrap_or(rq.url()).to_string();
    if url_path == "/latency/trigger" && *rq.method() == Method::Get {
        latency_trigger_get(rq);
        return;
    }
    let mut streaming_ctx = StreamingContext::from_config();
    streaming_ctx.set_remote_addr(&rq);
    streaming_ctx.set_sample_data(wd);
    let sp = StreamingParams::from_query_string(rq.url());
    if sp.path.is_none() {
        return bad_request(&streaming_ctx, rq);
    }
    streaming_ctx.update_format(&sp);
    debug!("{streaming_ctx:?}");
    match *rq.method() {
        Method::Get => streaming_request(&streaming_ctx, feedback_tx, rq),
        Method::Head => head_request(&streaming_ctx, rq),
        _ => invalid_request(&streaming_ctx, rq),
    }
}

/// Manual latency probe: inject Barker pulse and return JSON `latency_ms` or timeout.
fn latency_trigger_get(rq: tiny_http::Request) {
    let (tx, rx) = bounded(1);
    arm_latency_response_waiter(tx);
    let path = pulse_trigger_path();
    let _ = std::fs::write(&path, b"1");
    let body = match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Some(ms)) => format!("{{\"latency_ms\":{ms}}}\n"),
        Ok(None) => "{\"latency_ms\":null,\"error\":\"aborted\"}\n".to_string(),
        Err(_) => "{\"latency_ms\":null,\"error\":\"timeout\"}\n".to_string(),
    };
    let mut headers = get_std_headers();
    if let Ok(h) = Header::from_bytes(&b"Content-Type"[..], b"application/json") {
        headers.push(h);
    }
    let len = body.len();
    let resp = Response::new(
        tiny_http::StatusCode(200),
        headers,
        Cursor::new(body.into_bytes()),
        Some(len),
        None,
    );
    if let Err(e) = rq.respond(resp) {
        ui_log(
            LogCategory::Error,
            &format!("latency/trigger respond error: {e}"),
        );
    }
}

/// dump response headers
#[cfg(debug_assertions)]
fn dump_resp_headers(response: &Response<ChannelStream>) {
    debug!("==> Response:");
    debug!(
        " ==> Content-Length: {}",
        response.data_length().unwrap_or(0)
    );
    for hdr in response.headers() {
        debug!(" ==> Response {hdr:?}");
    }
}

/// dump the request headers
#[cfg(debug_assertions)]
fn dump_rq_headers(rq: &tiny_http::Request) {
    for hdr in rq.headers() {
        debug!(" <== Request {hdr:?}");
    }
}

/// GET METHOD request - request to start streaming
fn streaming_request(
    streaming_ctx: &StreamingContext,
    feedback_channel: &Sender<MessageType>,
    request: tiny_http::Request,
) {
    ui_log(
        LogCategory::Info,
        &format!(
            "Streaming request {} from {}",
            streaming_ctx.url, streaming_ctx.remote_addr
        ),
    );
    // get the dlna headers
    let headers = get_dlna_headers(streaming_ctx);

    // create the channelstream that receives the samples and streams them on demand
    let (tx, rx) = unbounded();
    let channel_stream = ChannelStream::new(
        tx,
        rx,
        streaming_ctx.remote_ip.clone(),
        streaming_ctx.needs_wav_hdr(),
        streaming_ctx.sample_rate,
        streaming_ctx.bits_per_sample as u16,
        streaming_ctx.streaming_format,
    );
    let nclients = {
        let mut clients = get_clients_mut();
        clients.insert(streaming_ctx.remote_addr.clone(), channel_stream.clone());
        clients.len()
    };
    debug!("Now have {nclients} streaming clients");

    feedback_channel
        .send(MessageType::PlayerMessage(StreamerFeedBack {
            remote_ip: streaming_ctx.remote_ip.clone(),
            streaming_state: StreamingState::Started,
        }))
        .unwrap_or_else(|e| {
            ui_log(
                LogCategory::Error,
                &format!("HTTP Server: error writing feedback channel {e:?}"),
            );
            panic!("Http server feedback error:{e:?}");
        });

    // check for upfront audio buffering needed
    if streaming_ctx.buffering_delay_msec > 0 {
        thread::sleep(Duration::from_millis(
            streaming_ctx.buffering_delay_msec.into(),
        ));
    }
    ui_log(
        LogCategory::Info,
        &format!(
            "Streaming {}, input sample format {:?}, \
                            channels=2, rate={}, bps = {}, to {}",
            streaming_ctx.dlna_string(),
            streaming_ctx.sample_format,
            streaming_ctx.sample_rate,
            streaming_ctx.bits_per_sample,
            streaming_ctx.remote_addr
        ),
    );
    let response = Response::new(
        tiny_http::StatusCode(200),
        headers,
        channel_stream,
        streaming_ctx.streamsize,
        None,
    )
    .with_chunked_threshold(streaming_ctx.chunksize);
    #[cfg(debug_assertions)]
    dump_resp_headers(&response);
    let e = request.respond(response);
    // #region agent log
    match &e {
        Ok(()) => agent_log(
            "H1",
            "streaming_server.rs:respond",
            "respond_ok",
            r#"{"status":"ok"}"#,
        ),
        Err(err) => agent_log(
            "H1",
            "streaming_server.rs:respond",
            "respond_err",
            &format!(r#"{{"kind":"{:?}"}}"#, err.kind()),
        ),
    }
    // #endregion
    if e.is_err() {
        ui_log(
            LogCategory::Error,
            &format!(
                "=>Http connection with {} terminated [{e:?}]",
                streaming_ctx.remote_addr
            ),
        );
    }
    let nclients = {
        let mut clients = get_clients_mut();
        clients.remove(&streaming_ctx.remote_addr);
        clients.len()
    };
    debug!("Now have {nclients} streaming clients left");
    // inform the main thread that this renderer has finished receiving
    // necessary if the connection close was not caused by our own GUI
    // so that we can update the corresponding button state
    feedback_channel
        .send(MessageType::PlayerMessage(StreamerFeedBack {
            remote_ip: streaming_ctx.remote_ip.clone(),
            streaming_state: StreamingState::Ended,
        }))
        .unwrap_or_else(|e| {
            ui_log(
                LogCategory::Error,
                &format!("HTTP Server: error writing feedback channel {e}"),
            );
            panic!("Http server feedback channel error:{e}");
        });
    ui_log(
        LogCategory::Info,
        &format!("Streaming to {} has ended", streaming_ctx.remote_addr),
    );
}

/// HEAD METHOD request
fn head_request(streaming_ctx: &StreamingContext, rq: tiny_http::Request) {
    debug!("HEAD rq from {}", streaming_ctx.remote_addr);
    // get the dlna headers
    let headers = get_dlna_headers(streaming_ctx);

    let response = Response::new(
        tiny_http::StatusCode(200),
        headers,
        io::empty(),
        Some(0),
        None,
    );
    if let Err(e) = rq.respond(response) {
        ui_log(
            LogCategory::Error,
            &format!(
                "=>Http HEAD connection with {} terminated [{e}]",
                streaming_ctx.remote_addr
            ),
        );
    }
}

/// invalid METHOD request
fn invalid_request(streaming_ctx: &StreamingContext, rq: tiny_http::Request) {
    ui_log(
        LogCategory::Error,
        &format!(
            "Unsupported HTTP method request {:?} from {}",
            *rq.method(),
            streaming_ctx.remote_addr
        ),
    );
    let headers = get_std_headers();
    let response = Response::new(
        tiny_http::StatusCode(405),
        headers,
        io::empty(),
        Some(0),
        None,
    );
    if let Err(e) = rq.respond(response) {
        ui_log(
            LogCategory::Error,
            &format!(
                "=>Http connection with {} terminated [{e}]",
                streaming_ctx.remote_addr
            ),
        );
    }
}

/// this request is not recotgnized, reject with an error 404
fn bad_request(streaming_ctx: &StreamingContext, rq: tiny_http::Request) {
    ui_log(
        LogCategory::Warning,
        &format!(
            "Unrecognized request '{}' from '{}'",
            rq.url(),
            streaming_ctx.remote_addr
        ),
    );
    let headers = get_std_headers();
    let response = Response::new(
        tiny_http::StatusCode(404),
        headers,
        io::empty(),
        Some(0),
        None,
    );
    if let Err(e) = rq.respond(response) {
        ui_log(
            LogCategory::Error,
            &format!(
                "=>Http streaming request with {} terminated [{e}]",
                streaming_ctx.remote_addr
            ),
        );
    }
}

/// get the dlna headers
fn get_dlna_headers(streaming_ctx: &StreamingContext) -> Vec<Header> {
    let mut headers = get_std_headers();
    // get the dlna format string
    let ct_text = match streaming_ctx.streaming_format {
        StreamingFormat::Wav | StreamingFormat::Rf64 => "audio/vnd.wave;codec=1".to_string(),
        StreamingFormat::Lpcm => match streaming_ctx.bits_per_sample {
            BitDepth::Bits16 => {
                format!("audio/L16;rate={};channels=2", streaming_ctx.sample_rate)
            }
            BitDepth::Bits24 => {
                format!("audio/L24;rate={};channels=2", streaming_ctx.sample_rate)
            }
        },
    };
    // and add dlna headers
    headers.push(Header::from_bytes(&b"Content-Type"[..], ct_text.as_bytes()).unwrap());
    headers.push(Header::from_bytes(&b"TransferMode.dlna.org"[..], &b"Streaming"[..]).unwrap());
    headers
}

/// get the standard headers
fn get_std_headers() -> Vec<Header> {
    // reserve space for dlna headers to be added later
    let mut headers = Vec::with_capacity(8);
    headers.push(Header::from_bytes(&b"Server"[..], &b"swyh-rs tiny-http"[..]).unwrap());
    headers.push(Header::from_bytes(&b"icy-name"[..], &b"swyh-rs"[..]).unwrap());
    headers.push(Header::from_bytes(&b"Connection"[..], &b"close"[..]).unwrap());

    /* don't accept range headers (Linn) until I know how to handle them
    but don't send this header as the MPD player ignores the "none" value anyway and uses ranges
    headers.push(Header::from_bytes(&b"Accept-Ranges"[..], &b"none"[..]).unwrap()); */

    headers
}

#[cfg(test)]
mod tests {
    use std::{thread, time::Duration};

    use super::reap_finished_requests;

    #[test]
    fn completed_request_threads_are_reaped() {
        let mut handles = vec![thread::spawn(|| {})];
        for _ in 0..100 {
            if handles[0].is_finished() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }

        reap_finished_requests(&mut handles);

        assert!(handles.is_empty());
    }
}
