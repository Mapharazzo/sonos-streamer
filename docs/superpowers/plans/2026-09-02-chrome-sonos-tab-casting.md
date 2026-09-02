# Chrome Sonos Tab Casting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build and verify a Windows Chrome extension prototype that sends only the selected tab's audio through the local `swyh-rs` process to Sonos and delays YouTube/Plex video by calibrated speaker latency.

**Architecture:** Add an authenticated synchronous WebSocket bridge and explicit 48 kHz browser-source mode to the Rust CLI, reusing its renderer discovery, WAV server, and microphone pulse detection. Add a build-free Manifest V3 extension whose offscreen document captures and packetizes audio while a content script maintains a timestamped, resource-bounded WebGL texture queue over YouTube and Plex video.

**Tech Stack:** Rust 2024, crossbeam-channel, tiny_http, tungstenite 0.30, serde/serde_json, getrandom 0.4, subtle 2.6, Chrome Manifest V3 (Chrome 116+), AudioWorklet, WebGL 2/1, JavaScript ES modules, Node built-in test runner.

**Spec:** `docs/superpowers/specs/2026-09-02-chrome-sonos-tab-casting-design.md`

## Global Constraints

- Windows is the acceptance platform; existing Linux compilation and normal desktop-loopback behavior must not regress.
- Chrome 116 is the minimum browser version.
- Browser sessions use 48,000 Hz, stereo, signed-16 little-endian PCM in 960-frame/20 ms packets.
- Browser mode forces 16-bit WAV only for that process session and never starts desktop loopback capture.
- The browser bridge binds only to `127.0.0.1:5902`; the Sonos HTTP server remains on the configured LAN port, normally 5901.
- One authenticated browser producer is allowed at a time.
- Effective video delay is `max(0, measured latency + manual offset)`, capped at 5,000 ms.
- Video capture is capped at 30 FPS, 1920x1080, and an estimated 512 MiB queued-texture budget, with a 640x360 minimum delayed surface.
- DRM, subtitle synchronization, guaranteed 4K delay, multi-tab producers, per-renderer multiroom delays, Web Store publication, and native process launching are out of scope.
- Do not import, vendor, translate, or copy third-party GPL source. All new source and tests must be original and remain MIT-licensed with this repository.
- Use TDD for every behavior change and commit after each task passes its focused tests.

## File Structure

### Rust files

- `src/browser/mod.rs`: browser subsystem exports and fixed protocol constants.
- `src/browser/mode.rs`: browser-mode session overrides and fixed `WavData`.
- `src/browser/auth.rs`: OS token generation, hex parsing, and constant-time validation.
- `src/browser/protocol.rs`: versioned JSON messages, renderer summaries, and validation errors.
- `src/browser/audio.rs`: `s16le` decode and publication into existing stream clients.
- `src/browser/commands.rs`: typed command/event channels and browser playback state.
- `src/browser/bridge.rs`: loopback WebSocket listener and one-producer connection loop.
- `src/browser/calibration.rs`: one-at-a-time asynchronous calibration and result events.
- `src/utils/audiodevices.rs`: expose a source-neutral f32 sample publisher while retaining RMS behavior.
- `src/utils/configuration.rs`: browser port and pairing token persistence.
- `src/utils/commandline.rs`: `--browser-source`, `--browser-port`, and `--show-browser-token`.
- `src/enums/messages.rs`: carry browser commands into the existing CLI loop.
- `src/bin/swyh-rs-cli.rs`: source-mode startup, bridge lifecycle, renderer commands, and stream/calibration event mapping.
- `src/lib.rs`: export `browser`.
- `tests/browser_bridge_stream.rs`: local WebSocket-to-WAV integration test with deterministic shutdown.

### Extension files

- `chrome-extension/manifest.json`: Manifest V3 permissions, popup, worker, static host matches, and local web-accessible modules.
- `chrome-extension/background.js`: offscreen lifecycle, capture gesture, tab lifecycle, and optional Plex registration.
- `chrome-extension/offscreen.html`: persistent offscreen host.
- `chrome-extension/offscreen.js`: WebSocket session, AudioContext/AudioWorklet lifecycle, state, and message routing.
- `chrome-extension/audio-worklet.js`: real-time channel extraction only.
- `chrome-extension/popup.html`, `popup.css`, `popup.js`: pairing, renderer, cast/stop, calibration, and offset UI.
- `chrome-extension/shared/pcm.js`: pure PCM clamp/interleave/packetization.
- `chrome-extension/shared/protocol.js`: protocol constants, request creation, and server-event validation.
- `chrome-extension/shared/session-state.js`: pure extension session reducer.
- `chrome-extension/content-script.js`: dynamic local-module bootstrap.
- `chrome-extension/video/frame-queue.js`: generic timestamped resource queue.
- `chrome-extension/video/resource-budget.js`: delay/dimension/FPS memory calculation.
- `chrome-extension/video/dominant-video.js`: pure eligible-video scoring.
- `chrome-extension/video/webgl-delay.js`: texture capture, downscale blit, overlay rendering, and cleanup.
- `chrome-extension/video/adapters/youtube.js`: YouTube video/overlay/fullscreen adapter.
- `chrome-extension/video/adapters/plex.js`: hosted/self-hosted Plex adapter.
- `chrome-extension/video/controller.js`: adapter selection, calibration updates, fallback, and lifecycle.
- `chrome-extension/package.json`: Node test and verification scripts with no runtime dependencies.
- `chrome-extension/tests/*.test.js`: pure module tests.
- `chrome-extension/tools/verify-extension.mjs`: manifest/file/remote-code verifier.
- `chrome-extension/README.md`: unpacked installation, pairing, supported sites, and diagnostics.
- `docs/windows-chrome-sonos-acceptance.md`: reproducible Windows hardware checklist and evidence table.

---

### Task 1: Browser Session Mode and Configuration

**Files:**
- Create: `src/browser/mod.rs`
- Create: `src/browser/mode.rs`
- Modify: `src/lib.rs`
- Modify: `src/utils/configuration.rs`
- Modify: `src/utils/commandline.rs`
- Modify: `src/bin/swyh-rs-cli.rs`
- Test: inline `#[cfg(test)]` modules in `src/browser/mode.rs`, `src/utils/configuration.rs`, and `src/utils/commandline.rs`

**Interfaces:**
- Produces: `BrowserMode::from_args(&Args, &Configuration) -> Option<BrowserMode>`.
- Produces: `BrowserMode::wav_data() -> WavData` fixed to F32/48 kHz/stereo for the existing f32 stream pipeline.
- Produces: `BrowserMode::apply_session_overrides(&mut Configuration)` forcing WAV/16-bit without persisting the override.
- Produces: `Args.browser_source: bool`, `Args.browser_port: Option<u16>`, and `Args.show_browser_token: bool`.
- Produces: `Configuration.browser_bridge_port: u16` defaulting to 5902 and `Configuration.browser_pairing_token: Option<String>`.

- [ ] **Step 1: Write failing browser-mode tests**

```rust
#[test]
fn browser_mode_is_fixed_stereo_48k_and_wav_16() {
    let mut config = Configuration::new();
    config.streaming_format = Some(StreamingFormat::Lpcm);
    config.bits_per_sample = Some(24);
    let mode = BrowserMode::new(5902);

    mode.apply_session_overrides(&mut config);
    let wav = mode.wav_data();

    assert_eq!(wav.sample_rate, cpal::SampleRate(48_000));
    assert_eq!(wav.channels, 2);
    assert_eq!(wav.sample_format, cpal::SampleFormat::F32);
    assert_eq!(config.streaming_format, Some(StreamingFormat::Wav));
    assert_eq!(config.bits_per_sample, Some(16));
}

#[test]
fn parses_browser_source_options() {
    let args = Args::parse_args(["--browser-source", "--browser-port", "6002"])
        .expect("browser args");
    assert!(args.browser_source);
    assert_eq!(args.browser_port, Some(6002));
}
```

- [ ] **Step 2: Run the focused tests and confirm the new API is missing**

Run: `cargo test --all-features browser_mode_is_fixed_stereo_48k_and_wav_16`

Run: `cargo test --all-features parses_browser_source_options`

Expected: FAIL because `BrowserMode`, configuration fields, and CLI options do not exist.

- [ ] **Step 3: Implement the browser mode and parseable CLI fields**

```rust
pub const DEFAULT_BROWSER_BRIDGE_PORT: u16 = 5902;
pub const BROWSER_SAMPLE_RATE: u32 = 48_000;
pub const BROWSER_CHANNELS: u16 = 2;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct BrowserMode { pub bridge_port: u16 }

impl BrowserMode {
    pub fn new(bridge_port: u16) -> Self { Self { bridge_port } }
    pub fn wav_data(self) -> WavData {
        WavData {
            sample_format: cpal::SampleFormat::F32,
            sample_rate: cpal::SampleRate(BROWSER_SAMPLE_RATE),
            channels: BROWSER_CHANNELS,
        }
    }
    pub fn apply_session_overrides(self, config: &mut Configuration) {
        config.streaming_format = Some(StreamingFormat::Wav);
        config.bits_per_sample = Some(16);
    }
}
```

Refactor `Args::parse()` to delegate to a testable `parse_args`, preserve every existing switch, add the three browser switches, and add serde defaults/migration for the two configuration fields. In CLI startup, select fixed browser `WavData` before any output-device requirement, but do not yet skip loopback; Task 5 completes startup branching.

- [ ] **Step 4: Run focused and regression tests**

Run: `cargo test --all-features browser_mode`

Run: `cargo test --all-features parses_browser_source`

Expected: PASS.

Run: `cargo test --all-features`

Expected: all pre-existing tests PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/browser src/lib.rs src/utils/configuration.rs src/utils/commandline.rs src/bin/swyh-rs-cli.rs
git commit -m "feat(browser): add explicit browser source mode"
```

### Task 2: Pairing and Versioned Protocol

**Files:**
- Create: `src/browser/auth.rs`
- Create: `src/browser/protocol.rs`
- Modify: `src/browser/mod.rs`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Test: inline module tests in the two new files

**Interfaces:**
- Produces: `ensure_pairing_token(config: &mut Configuration) -> Result<PairingToken, AuthError>`.
- Produces: `PairingToken::matches_hex(&self, candidate: &str) -> bool` using `subtle::ConstantTimeEq` on 32 decoded bytes.
- Produces: `parse_client_message(text: &str) -> Result<ClientMessage, ProtocolError>`.
- Produces: `validate_hello(&Hello, &PairingToken) -> Result<AudioSession, ProtocolError>`.
- Produces: `AudioSession::v1() -> AudioSession` as the single fixed session contract used by validation, decoding, and tests.
- Produces: serde `ClientMessage`, `ServerEvent`, `RendererSummary`, and `AudioSession` types used by Tasks 4–6.

- [ ] **Step 1: Add dependencies and write failing auth/protocol tests**

Add `getrandom = "0.4"`, `serde_json = "1.0"`, `subtle = "2.6"`, and `tungstenite = "0.30"` to `Cargo.toml`, then write:

```rust
#[test]
fn generated_token_round_trips_and_rejects_wrong_value() {
    let token = PairingToken::generate().expect("OS randomness");
    assert_eq!(token.to_hex().len(), 64);
    assert!(token.matches_hex(&token.to_hex()));
    assert!(!token.matches_hex(&"00".repeat(32)));
    assert!(!token.matches_hex("not-hex"));
}

#[test]
fn hello_requires_exact_v1_audio_contract() {
    let token = PairingToken::from_bytes([7; 32]);
    let hello = Hello {
        version: 1,
        token: token.to_hex(),
        audio: AudioHello {
            sample_rate: 48_000,
            channels: 2,
            sample_format: SampleFormat::S16Le,
            frames_per_packet: 960,
        },
    };
    assert_eq!(validate_hello(&hello, &token).unwrap().packet_bytes, 3840);
}
```

- [ ] **Step 2: Run tests and verify missing types fail**

Run: `cargo test --all-features browser::auth`

Run: `cargo test --all-features browser::protocol`

Expected: FAIL because auth and protocol modules do not exist.

- [ ] **Step 3: Implement token and protocol primitives**

```rust
pub struct PairingToken([u8; 32]);

impl PairingToken {
    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes)?;
        Ok(Self(bytes))
    }
    pub fn matches_hex(&self, candidate: &str) -> bool {
        decode_32_bytes(candidate)
            .map(|other| bool::from(self.0.ct_eq(&other)))
            .unwrap_or(false)
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ClientMessage {
    Hello(Hello),
    ListRenderers { request_id: String },
    Cast { request_id: String, renderer_id: String },
    Stop { request_id: String },
    Calibrate { request_id: String },
    SetManualOffset { request_id: String, milliseconds: i32 },
    GetState { request_id: String },
}
```

Implement original hex encode/decode helpers, serde rename coverage, maximum manual offset of ±5,000 ms, request-ID length validation, and renderer summary fields `id`, `name`, `model`, and `address`.

- [ ] **Step 4: Run protocol tests and clippy**

Run: `cargo test --all-features browser::auth`

Run: `cargo test --all-features browser::protocol`

Expected: PASS.

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/browser
git commit -m "feat(browser): define authenticated bridge protocol"
```

### Task 3: Browser PCM Decode and Source-neutral Publication

**Files:**
- Create: `src/browser/audio.rs`
- Modify: `src/browser/mod.rs`
- Modify: `src/utils/audiodevices.rs`
- Test: inline tests in `src/browser/audio.rs` and `src/utils/audiodevices.rs`

**Interfaces:**
- Consumes: `AudioSession` from Task 2.
- Produces: `decode_packet(session: &AudioSession, bytes: &[u8]) -> Result<AudioSamples, AudioPacketError>`.
- Produces: `publish_samples(samples: AudioSamples)` in `utils::audiodevices`, shared by CPAL and browser inputs.
- Produces: `BrowserAudioPublisher::publish(&self, bytes: &[u8]) -> Result<usize, AudioPacketError>`.

- [ ] **Step 1: Write failing PCM boundary tests**

```rust
#[test]
fn decodes_interleaved_s16le_to_normalized_f32() {
    let session = AudioSession::v1();
    let samples = decode_packet(&session, &[0x00, 0x80, 0xff, 0x7f]).unwrap();
    assert_eq!(samples.len(), 2);
    assert_eq!(samples[0], -1.0);
    assert!((samples[1] - 0.999_969_5).abs() < 0.000_001);
}

#[test]
fn rejects_partial_stereo_frame_and_oversized_packet() {
    let session = AudioSession::v1();
    assert_eq!(decode_packet(&session, &[0; 2]), Err(AudioPacketError::PartialFrame));
    assert_eq!(
        decode_packet(&session, &vec![0; session.max_packet_bytes + 4]),
        Err(AudioPacketError::TooLarge)
    );
}
```

- [ ] **Step 2: Run and confirm decode API is absent**

Run: `cargo test --all-features browser::audio`

Expected: FAIL because `decode_packet` is undefined.

- [ ] **Step 3: Implement decode and extract publication**

```rust
pub fn decode_packet(session: &AudioSession, bytes: &[u8]) -> Result<AudioSamples, AudioPacketError> {
    if bytes.len() > session.max_packet_bytes { return Err(AudioPacketError::TooLarge); }
    if bytes.len() % 4 != 0 { return Err(AudioPacketError::PartialFrame); }
    let decoded = bytes.chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32768.0)
        .collect::<Vec<_>>();
    Ok(AudioSamples::new(decoded))
}
```

Move the current client fan-out into `pub fn publish_samples(samples: AudioSamples)`. Keep RMS delivery in a separate private `publish_capture_samples(samples, rms_sender)` so browser packets do not unexpectedly enter the loopback RMS path. Make CPAL callbacks call the extracted functions without changing their semantics.

- [ ] **Step 4: Run audio and existing stream tests**

Run: `cargo test --all-features browser::audio`

Run: `cargo test --all-features utils::rwstream`

Run: `cargo test --all-features utils::samples_conv`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/browser src/utils/audiodevices.rs
git commit -m "feat(browser): publish decoded tab PCM"
```

### Task 4: WebSocket Bridge and One-producer Lifecycle

**Files:**
- Create: `src/browser/commands.rs`
- Create: `src/browser/bridge.rs`
- Modify: `src/browser/mod.rs`
- Test: inline bridge tests using `TcpListener` on port 0 and `tungstenite::connect`

**Interfaces:**
- Consumes: protocol/auth/audio APIs from Tasks 2–3.
- Produces: `BrowserCommand { request_id: String, kind: BrowserCommandKind }`.
- Produces: `BrowserEvent` serializable to `ServerEvent`.
- Produces: `BrowserBridge::spawn(config, command_tx, event_rx) -> io::Result<BrowserBridgeHandle>`.
- Produces: `BrowserBridgeHandle::local_addr()` and idempotent `shutdown()`.

- [ ] **Step 1: Write failing bridge lifecycle tests**

```rust
#[test]
fn bridge_rejects_web_origin_and_requires_hello_before_binary() {
    let harness = BridgeHarness::start();
    assert_eq!(harness.connect_with_origin("https://example.com").status(), 403);

    let mut ws = harness.connect_extension();
    ws.send(Message::Binary(vec![0; 3840].into())).unwrap();
    assert!(matches!(ws.read(), Err(_) | Ok(Message::Close(_))));
}

#[test]
fn second_authenticated_producer_is_rejected_until_first_disconnects() {
    let harness = BridgeHarness::start();
    let _first = harness.authenticated_client();
    let mut second = harness.extension_client();
    second.send(Message::Text(harness.hello_json().into())).unwrap();
    assert_error_code(second.read().unwrap(), "producer-busy");
}
```

- [ ] **Step 2: Run and verify bridge tests fail**

Run: `cargo test --all-features browser::bridge`

Expected: FAIL because the bridge and harness do not exist.

- [ ] **Step 3: Implement the bounded synchronous connection loop**

```rust
pub struct BrowserBridgeConfig {
    pub addr: SocketAddr,
    pub token: PairingToken,
    pub max_text_bytes: usize,
    pub max_binary_bytes: usize,
}

pub struct BrowserBridgeHandle {
    addr: SocketAddr,
    shutdown_tx: Sender<()>,
    join: Option<JoinHandle<()>>,
}
```

Use `accept_hdr_with_config` to require path `/browser/v1`, validate the Origin scheme, and cap WebSocket messages at 64 KiB/text and 64 KiB/binary. Set a short TCP read timeout. In one loop: read at most one incoming message, publish valid binary audio only after `hello-ok`, forward commands, drain outgoing events, respond to ping/close, and observe shutdown. Guard the authenticated producer with an atomic compare/exchange that is always released by a connection guard's `Drop`.

- [ ] **Step 4: Run bridge tests including shutdown/reconnect**

Run: `cargo test --all-features browser::bridge`

Expected: PASS with no leaked listener threads.

- [ ] **Step 5: Commit**

```bash
git add src/browser
git commit -m "feat(browser): add authenticated localhost bridge"
```

### Task 5: CLI Renderer Control and Browser-source Isolation

**Files:**
- Create: `src/browser/calibration.rs`
- Modify: `src/browser/commands.rs`
- Modify: `src/enums/messages.rs`
- Modify: `src/autocalibrate.rs`
- Modify: `src/bin/swyh-rs-cli.rs`
- Modify: `src/utils/commandline.rs`
- Test: inline state-machine and calibration tests

**Interfaces:**
- Consumes: `BrowserCommand`/`BrowserEvent` and `BrowserMode`.
- Produces: `BrowserPlaybackState::{Idle, Starting, Streaming, Calibrating, Failed}` transitions.
- Produces: `CalibrationCoordinator::start(event_tx) -> Result<(), CalibrationBusy>`.
- Produces: CLI command handlers for list, cast, stop, calibrate, manual-offset echo, and state.

- [ ] **Step 1: Write failing source-isolation and transition tests**

```rust
#[test]
fn browser_startup_plan_never_opens_loopback_capture() {
    let plan = StartupPlan::for_browser(BrowserMode::new(5902));
    assert_eq!(plan.audio_source, AudioSourcePlan::BrowserOnly);
    assert!(plan.start_latency_listener);
    assert!(plan.start_ssdp);
}

#[test]
fn stream_started_arms_auto_calibration_once() {
    let mut state = BrowserPlaybackState::starting("renderer-1");
    assert_eq!(state.on_stream_started(), PlaybackAction::StartCalibration);
    assert_eq!(state.on_stream_started(), PlaybackAction::None);
}
```

- [ ] **Step 2: Run and verify state APIs fail**

Run: `cargo test --all-features browser_startup_plan`

Run: `cargo test --all-features stream_started_arms_auto_calibration`

Expected: FAIL because startup/state types do not exist.

- [ ] **Step 3: Implement source branching and command ownership**

In browser mode:

```rust
let (wd, loopback_stream) = if let Some(mode) = browser_mode {
    mode.apply_session_overrides(&mut config);
    (mode.wav_data(), None)
} else {
    let device = require_output_device(audio_output_device_opt)?;
    let wd = wav_data_from_device(&device);
    (wd, Some(start_loopback_capture(device, rms_channel.clone())?))
};
```

Start the bridge before entering the CLI message loop. Add browser commands to `MessageType`, map renderer `location` to opaque ID, clone the selected `Renderer` into the existing `playing` collection, and keep all calls to `Renderer::play`/`stop_play` in the CLI thread. Browser mode must not auto-play `last_renderer`; it waits for `cast`.

Persist the user's ordinary configuration and any newly generated pairing token before applying browser runtime overrides. Replace unconditional later `sync_config` calls with an explicit persistence choice: desktop mode may write and install configuration, while browser mode installs the runtime WAV/16-bit clone into `get_config_mut()` without writing that clone to disk. `--show-browser-token` prints the token only on explicit invocation and exits before starting discovery or listeners.

Refactor `measure_latency_roundtrip` so a `CalibrationCoordinator` can run it on one worker thread after an 800 ms settle delay and emit `CalibrationResult`/`CalibrationFailed`. Preserve `run_startup_calibration` unchanged for desktop mode. The manual command uses the same coordinator and returns `calibration-busy` if one is active.

- [ ] **Step 4: Run focused tests and CLI dry runs**

Run: `cargo test --all-features browser::commands`

Run: `cargo test --all-features browser::calibration`

Expected: PASS.

Run: `cargo run -- --browser-source --dry-run`

Expected: exits successfully after discovery/setup validation without logging loopback capture startup.

Run: `cargo run -- --dry-run`

Expected: existing desktop dry-run still succeeds.

- [ ] **Step 5: Commit**

```bash
git add src/browser src/enums/messages.rs src/autocalibrate.rs src/bin/swyh-rs-cli.rs src/utils/commandline.rs
git commit -m "feat(browser): control Sonos from browser sessions"
```

### Task 6: Rust WebSocket-to-WAV Integration Test

**Files:**
- Modify: `src/server/streaming_server.rs`
- Create: `tests/browser_bridge_stream.rs`
- Test: `tests/browser_bridge_stream.rs`

**Interfaces:**
- Consumes: bridge and browser-mode APIs from Tasks 1–5.
- Produces: `run_server_instance(server, wd, feedback_tx, shutdown_rx)` with deterministic test shutdown; production `run_server` wraps it.
- Proves: authenticated WebSocket PCM reaches a simulated Sonos HTTP GET as valid non-silent WAV bytes.

- [ ] **Step 1: Write the failing end-to-end integration test**

```rust
#[test]
fn browser_pcm_is_served_as_wav_to_http_consumer() {
    let harness = FullBridgeHarness::start();
    let mut ws = harness.authenticated_client();
    ws.send(Message::Binary(stereo_s16_packet(1000, -1000).into())).unwrap();

    let response = ureq::get(&harness.wav_url()).call().unwrap();
    assert_eq!(response.headers().get("Content-Type").unwrap(), "audio/wav");
    let bytes = response.into_body().into_reader().take(8192).bytes()
        .collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(&bytes[0..4], b"RIFF");
    assert!(bytes[44..].iter().any(|byte| *byte != 0));

    harness.shutdown();
}
```

- [ ] **Step 2: Run and confirm production server cannot yet shut down in a test**

Run: `cargo test --all-features --test browser_bridge_stream -- --nocapture`

Expected: FAIL because `run_server_instance` and the full harness are absent.

- [ ] **Step 3: Add deterministic server injection/shutdown and complete the harness**

Refactor request handling without changing headers or production threading. The injected server loop uses `recv_timeout` and checks `shutdown_rx`; request worker handles retain existing behavior. The harness binds both servers to port 0, inserts a test renderer stream client through a real HTTP GET, sends repeated PCM packets until the consumer has bytes, then closes WebSocket, HTTP, bridge, and server threads.

- [ ] **Step 4: Run integration and all Rust tests**

Run: `cargo test --all-features --test browser_bridge_stream -- --nocapture`

Expected: PASS and process exits without hanging.

Run: `cargo test --all-features`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/server/streaming_server.rs tests/browser_bridge_stream.rs
git commit -m "test(browser): verify tab PCM reaches WAV stream"
```

### Task 7: Manifest, Shared Protocol, and Session State

**Files:**
- Create: `chrome-extension/manifest.json`
- Create: `chrome-extension/package.json`
- Create: `chrome-extension/shared/protocol.js`
- Create: `chrome-extension/shared/session-state.js`
- Create: `chrome-extension/tests/protocol.test.js`
- Create: `chrome-extension/tests/session-state.test.js`

**Interfaces:**
- Produces: `BRIDGE_URL`, `PROTOCOL_VERSION`, `createHello`, `createCommand`, and `parseServerEvent`.
- Produces: `initialSessionState()` and `reduceSession(state, event)`.
- Supplies state shape to background, offscreen, popup, and video tasks.

- [ ] **Step 1: Write failing pure protocol/state tests**

```javascript
import test from 'node:test';
import assert from 'node:assert/strict';
import { createHello, parseServerEvent } from '../shared/protocol.js';
import { initialSessionState, reduceSession } from '../shared/session-state.js';

test('hello exactly matches Rust v1 audio contract', () => {
  assert.deepEqual(createHello('abc'), {
    type: 'hello', version: 1, token: 'abc',
    audio: { sampleRate: 48000, channels: 2, sampleFormat: 's16le', framesPerPacket: 960 }
  });
});

test('disconnect resets active cast but keeps pairing state', () => {
  const active = { ...initialSessionState(), paired: true, phase: 'streaming', tabId: 42 };
  const next = reduceSession(active, { type: 'bridge-disconnected' });
  assert.equal(next.phase, 'disconnected');
  assert.equal(next.tabId, null);
  assert.equal(next.paired, true);
});
```

- [ ] **Step 2: Run Node tests and confirm modules are absent**

Run: `cd chrome-extension && node --test tests/protocol.test.js tests/session-state.test.js`

Expected: FAIL with module-not-found.

- [ ] **Step 3: Implement original shared modules and minimal manifest**

Manifest permissions must be exactly `activeTab`, `offscreen`, `scripting`, `storage`, and `tabCapture`; host permissions include YouTube, hosted Plex, and localhost WebSocket access, with `optional_host_permissions` for `http://*/*` and `https://*/*` used only through the explicit self-hosted Plex flow. Set background type to `module`, popup path, minimum Chrome 116, and web-accessible local video modules.

Implement strict event parsing that rejects non-objects, unknown terminal protocol versions, invalid renderer arrays, and unbounded strings. The reducer owns `phase`, `connected`, `paired`, `renderers`, `selectedRendererId`, `tabId`, `measuredDelayMs`, `manualOffsetMs`, `warning`, and counters.

- [ ] **Step 4: Run pure tests**

Run: `cd chrome-extension && node --test tests/protocol.test.js tests/session-state.test.js`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add chrome-extension
git commit -m "feat(extension): scaffold protocol and session state"
```

### Task 8: PCM Packetizer, AudioWorklet, and Offscreen Bridge

**Files:**
- Create: `chrome-extension/shared/pcm.js`
- Create: `chrome-extension/audio-worklet.js`
- Create: `chrome-extension/offscreen.html`
- Create: `chrome-extension/offscreen.js`
- Create: `chrome-extension/tests/pcm.test.js`
- Create: `chrome-extension/tests/backpressure.test.js`

**Interfaces:**
- Consumes: Task 7 protocol/state.
- Produces: `PcmPacketizer.push(left, right) -> ArrayBuffer[]` and `reset()`.
- Produces: `shouldDropAudio(bufferedAmount, packetBytes) -> boolean`.
- Produces: offscreen runtime messages `bridge-connect`, `start-capture`, `stop-session`, and `browser-command`.
- Produces: state/event broadcasts to the service worker and popup.

- [ ] **Step 1: Write failing packetization/backpressure tests**

```javascript
test('packetizer emits exactly 960 interleaved stereo frames', () => {
  const p = new PcmPacketizer(960);
  const left = new Float32Array(960).fill(-1);
  const right = new Float32Array(960).fill(1);
  const [packet] = p.push(left, right);
  const view = new DataView(packet);
  assert.equal(packet.byteLength, 3840);
  assert.equal(view.getInt16(0, true), -32768);
  assert.equal(view.getInt16(2, true), 32767);
});

test('high websocket backlog drops new live input', () => {
  assert.equal(shouldDropAudio(256 * 1024, 3840), true);
  assert.equal(shouldDropAudio(0, 3840), false);
});
```

- [ ] **Step 2: Run tests and verify missing implementation**

Run: `cd chrome-extension && node --test tests/pcm.test.js tests/backpressure.test.js`

Expected: FAIL.

- [ ] **Step 3: Implement packetizer and audio/offscreen lifecycle**

```javascript
export function floatToS16(value) {
  const clamped = Math.max(-1, Math.min(1, Number.isFinite(value) ? value : 0));
  return clamped < 0 ? Math.round(clamped * 32768) : Math.round(clamped * 32767);
}
```

The worklet must only copy each 128-frame render quantum to its `MessagePort`; it performs no WebSocket work. The offscreen document creates `AudioContext({sampleRate: 48000})`, loads the local worklet, creates an `AudioWorkletNode` with zero outputs as the graph sink, packetizes worklet messages, and sends only below the backlog threshold. It owns the single WebSocket, authenticates before enabling audio, drains Rust events while capture continues, and tears down tracks/context/socket idempotently.

- [ ] **Step 4: Run all extension pure tests**

Run: `cd chrome-extension && node --test tests/*.test.js`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add chrome-extension
git commit -m "feat(extension): stream packetized tab audio"
```

### Task 9: Background Capture Orchestration and Popup Controls

**Files:**
- Create: `chrome-extension/background.js`
- Create: `chrome-extension/popup.html`
- Create: `chrome-extension/popup.css`
- Create: `chrome-extension/popup.js`
- Create: `chrome-extension/tests/background-state.test.js`
- Modify: `chrome-extension/manifest.json`

**Interfaces:**
- Consumes: Task 8 offscreen message protocol.
- Produces: `ensureOffscreenDocument()`, `startCapture(tabId, rendererId)`, `stopCapture(reason)`, and `registerPlexOrigin(origin)`.
- Produces: popup commands `pair`, `select-renderer`, `cast`, `stop`, `calibrate`, and `set-offset`.

- [ ] **Step 1: Write failing lifecycle tests around extracted pure decisions**

```javascript
test('cast requires token, renderer, and supported active tab', () => {
  assert.deepEqual(validateCastRequest({ token: '', rendererId: '', url: 'https://youtube.com/watch?v=x' }),
    ['pairing-token', 'renderer']);
});

test('terminal capture event requests one idempotent cleanup', () => {
  const first = cleanupTransition(streamingState, 'track-ended');
  const second = cleanupTransition(first.state, 'track-ended');
  assert.equal(first.shouldCleanup, true);
  assert.equal(second.shouldCleanup, false);
});
```

- [ ] **Step 2: Run the focused test and confirm orchestration helpers are absent**

Run: `cd chrome-extension && node --test tests/background-state.test.js`

Expected: FAIL.

- [ ] **Step 3: Implement capture and accessible popup**

The Cast button message must cause the service worker to call `chrome.tabCapture.getMediaStreamId({targetTabId})` while acting on the user's extension invocation, then send the stream ID to the offscreen document. The worker watches tab removal/navigation and capture status. Self-hosted Plex registration validates an `http`/`https` origin, requests that exact origin, and registers `content-script.js` only after permission succeeds.

The popup renders state without framework dependencies, never logs the token, labels calibration as audible, keeps Cast disabled until connected/paired/renderer-selected, and keeps Stop available during starting/streaming/calibrating phases. Store token, last renderer, measured delays, and manual offsets in `chrome.storage.local`.

- [ ] **Step 4: Run tests and static JSON parse**

Run: `cd chrome-extension && node --test tests/*.test.js`

Expected: PASS.

Run: `node -e "JSON.parse(require('fs').readFileSync('chrome-extension/manifest.json'))"`

Expected: exit 0.

- [ ] **Step 5: Commit**

```bash
git add chrome-extension
git commit -m "feat(extension): add casting controls and capture lifecycle"
```

### Task 10: Timed Frame Queue and Resource Budget

**Files:**
- Create: `chrome-extension/video/frame-queue.js`
- Create: `chrome-extension/video/resource-budget.js`
- Create: `chrome-extension/tests/frame-queue.test.js`
- Create: `chrome-extension/tests/resource-budget.test.js`

**Interfaces:**
- Produces: `TimedFrameQueue({release, maxFrames, maxBytes})` with `enqueue`, `takeDue`, `setDelay`, `clear`, `length`, and `bytes`.
- Produces: `chooseFramePlan({sourceWidth, sourceHeight, fps, delayMs, maxBytes}) -> FramePlan | null`.

- [ ] **Step 1: Write failing queue timing and budget tests**

```javascript
test('delay reduction retires stale frames and releases superseded resources', () => {
  const released = [];
  const q = new TimedFrameQueue({ release: x => released.push(x.id), maxFrames: 10, maxBytes: 1000 });
  q.enqueue({ id: 1 }, 100, 300, 10);
  q.enqueue({ id: 2 }, 200, 300, 10);
  q.setDelay(100);
  assert.equal(q.takeDue(350).id, 2);
  assert.deepEqual(released, [1]);
});

test('resource plan downscales but never below 640x360', () => {
  const plan = chooseFramePlan({ sourceWidth: 1920, sourceHeight: 1080, fps: 30, delayMs: 3000, maxBytes: 512 * 1024 ** 2 });
  assert.ok(plan.width <= 1920 && plan.height <= 1080);
  assert.ok(plan.width >= 640 && plan.height >= 360);
  assert.ok(plan.estimatedBytes <= 512 * 1024 ** 2);
});
```

- [ ] **Step 2: Run tests and verify modules are absent**

Run: `cd chrome-extension && node --test tests/frame-queue.test.js tests/resource-budget.test.js`

Expected: FAIL.

- [ ] **Step 3: Implement generic resources without DOM/WebGL dependencies**

Queue each entry as `{resource, capturedAt, dueAt, bytes}`. `takeDue(now)` returns only the newest due resource and releases all earlier due resources. `setDelay(newDelay)` shifts every `dueAt` by the exact delta. `clear()` releases every queued resource once. Budgeting uses 4 bytes/pixel, `ceil(fps * delayMs / 1000) + 2` frames, a maximum source of 1920x1080, proportional downscale, and returns `null` if the required size falls below 640x360.

- [ ] **Step 4: Run queue tests**

Run: `cd chrome-extension && node --test tests/frame-queue.test.js tests/resource-budget.test.js`

Expected: PASS including zero delay, negative requested delay clamp, cap overflow, and idempotent clear cases.

- [ ] **Step 5: Commit**

```bash
git add chrome-extension/video chrome-extension/tests
git commit -m "feat(extension): add bounded timed video queue"
```

### Task 11: WebGL Delay Surface

**Files:**
- Create: `chrome-extension/video/webgl-delay.js`
- Create: `chrome-extension/tests/webgl-lifecycle.test.js`
- Modify: `chrome-extension/manifest.json`

**Interfaces:**
- Consumes: `TimedFrameQueue` and `FramePlan` from Task 10.
- Produces: `WebGlDelaySurface(video, placement, options)` with `start(delayMs)`, `setDelay(delayMs)`, and idempotent `dispose()`.
- Emits: `onFallback({code, message})` without leaving the source video hidden.

- [ ] **Step 1: Write failing lifecycle tests with a fake GL adapter**

```javascript
test('source video is hidden only after first successful captured texture', () => {
  const fake = makeSurfaceHarness();
  fake.surface.start(800);
  assert.equal(fake.video.style.opacity, '1');
  fake.deliverFrame(0);
  assert.equal(fake.video.style.opacity, '0');
});

test('allocation failure restores exact original inline styles', () => {
  const fake = makeSurfaceHarness({ failAllocation: true, originalOpacity: '0.7' });
  fake.surface.start(800);
  fake.deliverFrame(0);
  assert.equal(fake.video.style.opacity, '0.7');
  assert.equal(fake.canvases.length, 0);
});
```

- [ ] **Step 2: Run and confirm surface is absent**

Run: `cd chrome-extension && node --test tests/webgl-lifecycle.test.js`

Expected: FAIL.

- [ ] **Step 3: Implement original WebGL capture/downscale/render pipeline**

Create a pointer-transparent overlay canvas. Compile a minimal passthrough shader. Maintain one reusable full-source texture and framebuffer-blit each accepted source frame into a budgeted queue texture. Use `requestVideoFrameCallback` for new frames, throttled to 30 FPS, and `requestAnimationFrame` to call `queue.takeDue(performance.now())` and render the returned texture. Explicitly delete replaced/displayed/queued textures.

Save exact original `style.opacity`, `style.visibility`, and placement-related values before modification. Handle resize by rebuilding only after a new frame succeeds. On WebGL context loss, CORS/DRM upload exception, allocation failure, cap failure, or disposal: cancel callbacks, delete GPU objects, remove canvas, and restore exact styles.

- [ ] **Step 4: Run lifecycle and queue tests**

Run: `cd chrome-extension && node --test tests/frame-queue.test.js tests/resource-budget.test.js tests/webgl-lifecycle.test.js`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add chrome-extension/video chrome-extension/tests chrome-extension/manifest.json
git commit -m "feat(extension): render calibrated delayed video"
```

### Task 12: YouTube and Plex Adapters

**Files:**
- Create: `chrome-extension/video/dominant-video.js`
- Create: `chrome-extension/video/adapters/youtube.js`
- Create: `chrome-extension/video/adapters/plex.js`
- Create: `chrome-extension/video/controller.js`
- Create: `chrome-extension/content-script.js`
- Create: `chrome-extension/tests/adapters.test.js`
- Modify: `chrome-extension/manifest.json`

**Interfaces:**
- Consumes: `WebGlDelaySurface`.
- Produces: `selectDominantVideo(candidates, viewport) -> HTMLVideoElement | null`.
- Produces: adapter contract `{matches(location), findVideo(document), placement(video), observe(callback), dispose()}`.
- Produces: content messages `video-delay:set`, `video-delay:stop`, and status replies `active`/`audio-only`/`error`.

- [ ] **Step 1: Write failing selection and site tests**

```javascript
test('dominant selection ignores paused thumbnails and chooses largest visible player', () => {
  const videos = [fakeVideo({ area: 20_000, paused: false }), fakeVideo({ area: 900_000, paused: false })];
  assert.equal(selectDominantVideo(videos, viewport), videos[1]);
});

test('adapters recognize YouTube and hosted or self-hosted Plex', () => {
  assert.equal(youtubeAdapter.matches(new URL('https://www.youtube.com/watch?v=x')), true);
  assert.equal(plexAdapter.matches(new URL('https://app.plex.tv/desktop/')), true);
  assert.equal(plexAdapter.matches(new URL('http://plex.lan:32400/web/')), true);
});
```

- [ ] **Step 2: Run and verify adapter modules fail**

Run: `cd chrome-extension && node --test tests/adapters.test.js`

Expected: FAIL.

- [ ] **Step 3: Implement thin site adapters and controller**

YouTube placement targets the main player container while keeping `.ytp-chrome-*` controls above the overlay. Plex placement uses the nearest stable player container and keeps its control layer above the overlay. Both use `ResizeObserver`, `MutationObserver`, and `fullscreenchange` only to request controller re-evaluation; they do not own texture logic.

The controller waits for an eligible playing video, selects the adapter from the URL, creates one surface, and replaces it when single-page navigation changes the media element. `video-delay:set` clamps effective delay to 0–5,000 ms; `video-delay:stop` disposes. Any surface fallback reports audio-only and never retries in a tight loop.

`content-script.js` dynamically imports only packaged extension URLs. Add static content-script matches for YouTube and `app.plex.tv`; self-hosted registration reuses the same loader.

- [ ] **Step 4: Run all video tests**

Run: `cd chrome-extension && node --test tests/frame-queue.test.js tests/resource-budget.test.js tests/webgl-lifecycle.test.js tests/adapters.test.js`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add chrome-extension
git commit -m "feat(extension): support YouTube and Plex video delay"
```

### Task 13: Calibration-to-video Integration and Failure Cleanup

**Files:**
- Modify: `chrome-extension/offscreen.js`
- Modify: `chrome-extension/background.js`
- Modify: `chrome-extension/popup.js`
- Modify: `chrome-extension/shared/session-state.js`
- Create: `chrome-extension/tests/integration-state.test.js`

**Interfaces:**
- Consumes: Rust `calibration-result`, saved per-renderer values, and content-script commands.
- Produces: `effectiveDelay(measuredMs, manualOffsetMs) -> 0..5000`.
- Produces: one shared terminal cleanup transition for Stop, track end, tab close, socket loss, and fatal protocol errors.

- [ ] **Step 1: Write failing calibration and cleanup integration tests**

```javascript
test('calibration result persists raw measurement and sends clamped effective delay', () => {
  const result = applyCalibration(streamingState({ manualOffsetMs: -100 }), { latencyMs: 1800 });
  assert.equal(result.state.measuredDelayMs, 1800);
  assert.deepEqual(result.effects, [{ type: 'send-video-delay', delayMs: 1700 }]);
});

test('socket loss stops renderer intent, capture, and overlay exactly once', () => {
  const result = terminalCleanup(streamingState(), 'socket-lost');
  assert.deepEqual(result.effects.map(x => x.type),
    ['best-effort-stop-renderer', 'stop-media-tracks', 'dispose-audio', 'dispose-video']);
  assert.equal(terminalCleanup(result.state, 'socket-lost').effects.length, 0);
});
```

- [ ] **Step 2: Run and verify integration behavior is missing**

Run: `cd chrome-extension && node --test tests/integration-state.test.js`

Expected: FAIL.

- [ ] **Step 3: Wire automatic/manual calibration and terminal paths**

When Rust reports stream started, show calibrating state; when raw latency arrives, persist it under the renderer ID, combine with manual offset, and send `video-delay:set` to the captured tab. The Calibrate button sends only the Rust command. Offset changes recompute immediately without recalibration and send the protocol's `set-manual-offset` telemetry command.

Calibration timeout uses the last successful measurement or zero and raises a nonterminal warning. Stop/socket/track/tab terminal events call one idempotent effect runner that best-effort stops the renderer before closing the bridge, stops tracks/context, tells the content script to restore video, and clears active IDs.

- [ ] **Step 4: Run all extension tests**

Run: `cd chrome-extension && node --test tests/*.test.js`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add chrome-extension
git commit -m "feat(extension): synchronize calibration and cleanup"
```

### Task 14: Static Verification, Packaging, and User Documentation

**Files:**
- Create: `chrome-extension/tools/verify-extension.mjs`
- Create: `chrome-extension/tools/package-extension.mjs`
- Create: `chrome-extension/README.md`
- Create: `docs/windows-chrome-sonos-acceptance.md`
- Modify: `chrome-extension/package.json`
- Modify: `README.md`
- Test: verifier self-tests inside `chrome-extension/tests/verifier.test.js`

**Interfaces:**
- Produces: `npm test`, `npm run verify`, and `npm run package`.
- Produces: deterministic `dist/sonos-tab-caster.zip` excluded from git.
- Produces: exact Windows setup and evidence capture instructions.

- [ ] **Step 1: Write a failing verifier test**

```javascript
test('verifier rejects remote executable scripts and missing manifest files', async () => {
  const bad = fixtureExtension({ popup: 'missing.html', source: 'import("https://evil.invalid/x.js")' });
  const errors = await verifyExtension(bad);
  assert.ok(errors.some(x => x.includes('missing.html')));
  assert.ok(errors.some(x => x.includes('remote executable code')));
});
```

- [ ] **Step 2: Run and confirm verifier is absent**

Run: `cd chrome-extension && node --test tests/verifier.test.js`

Expected: FAIL.

- [ ] **Step 3: Implement verifier, package script, and docs**

The verifier parses the manifest, checks version 3/minimum 116, validates every referenced local file, disallows `http:`/`https:` script/import/worker sources, checks that executable files are repository-owned paths, and rejects source references to third-party delay packages. The package script runs tests and verification first, then creates a zip containing only extension runtime files, excluding tests, tools, `node_modules`, and `dist`.

Document browser-mode startup, one-time token pairing, unpacked loading, microphone selection, hosted/self-hosted Plex permission, diagnostics, audio-only fallback, Stop behavior, and all prototype exclusions. Add the exact ten-step hardware table from the spec with columns for date, commit, Chrome version, Sonos model/version, measured delay, result, and evidence path.

- [ ] **Step 4: Run complete local verification**

Run: `cd chrome-extension && npm test && npm run verify && npm run package`

Expected: all tests PASS, verifier reports zero errors, and `dist/sonos-tab-caster.zip` is created but ignored.

Run: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add .gitignore README.md chrome-extension docs/windows-chrome-sonos-acceptance.md
git commit -m "docs(browser): add packaging and Windows acceptance guide"
```

### Task 15: Windows Build and Hardware Acceptance

**Files:**
- Modify: implementation files only for defects proven by Windows evidence
- Modify: `docs/windows-chrome-sonos-acceptance.md`
- Test: Windows release build, unpacked extension, YouTube, Plex, microphone, and Sonos hardware

**Interfaces:**
- Consumes: complete prototype from Tasks 1–14.
- Produces: recorded acceptance evidence for every spec criterion.

- [ ] **Step 1: Establish clean pre-Windows baseline**

Run: `git status --short`

Expected: empty.

Run: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features && (cd chrome-extension && npm test && npm run verify && npm run package)`

Expected: every command PASS.

- [ ] **Step 2: Build the Windows release artifact**

Run the repository's established Windows transfer/build workflow if available; otherwise on Windows run:

```powershell
cargo build --release --all-features
```

Expected: `target\release\swyh-rs-cli.exe` builds successfully and `--help` lists browser-source options.

- [ ] **Step 3: Execute YouTube acceptance**

Start `swyh-rs-cli.exe --browser-source`, load the unpacked extension, pair, select a renderer, and execute rows 1–6 of `docs/windows-chrome-sonos-acceptance.md` using ordinary 1080p YouTube. Record logs, raw measured latency, manual-offset response, pause/seek/fullscreen/next-video results, PC silence, and Sonos-only audio.

Expected: every YouTube row PASS or a concrete defect is reproduced with timestamped evidence.

- [ ] **Step 4: Execute Plex and failure-path acceptance**

Execute rows 7–10 with Plex direct-play, Stop, forced Rust termination, and unsupported/DRM fallback. Record the exact Plex origin and whether optional permission was used.

Expected: every Plex/failure row PASS; protected content restores original video and explicitly reports audio-only.

- [ ] **Step 5: Fix only evidence-backed Windows defects with red-green tests**

For each failure, first add the smallest automated reproduction to the owning Rust or extension test file, run it to see FAIL, patch the implementation, rerun focused tests to PASS, then rerun the affected Windows acceptance row. Do not weaken the acceptance criterion or silently convert a required path to fallback.

- [ ] **Step 6: Run final full verification and inspect diff**

Run: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features && (cd chrome-extension && npm test && npm run verify && npm run package)`

Expected: PASS.

Run: `git diff --check && git status --short`

Expected: only the acceptance evidence and intentional red-green fixes are present.

- [ ] **Step 7: Commit verified acceptance**

```bash
git add docs/windows-chrome-sonos-acceptance.md src tests chrome-extension
git commit -m "test(browser): verify Windows Sonos tab casting"
```

## Completion Audit

Before claiming completion, inspect the current files and evidence rather than relying on task checkmarks:

1. Confirm `--browser-source` cannot reach `capture_output_audio` and normal mode still can.
2. Confirm no listener other than the existing Sonos HTTP server binds a non-loopback address.
3. Confirm a binary WebSocket packet is rejected before auth and decoded/published after auth.
4. Confirm extension capture never connects tab audio to a local destination.
5. Confirm automatic and button-triggered calibration both deliver raw measurements to the captured tab.
6. Confirm manual offset is applied exactly once and effective delay clamps to 0–5,000 ms.
7. Confirm YouTube and Plex adapters use the shared original WebGL controller and restore exact video styles.
8. Confirm queue frame rate, dimensions, byte cap, and minimum-size fallback match the spec.
9. Confirm Stop, tab close, socket loss, track end, WebGL failure, and DRM failure have verified cleanup/fallback behavior.
10. Search for copied/vendor artifacts and remote executable code; inspect license headers and dependency licenses.
11. Re-run all Rust, extension, package, and Windows checks and attach their fresh output/evidence.
12. Match every acceptance-criteria bullet in the spec to a passing automated test or recorded Windows hardware row.
