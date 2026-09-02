# Chrome Tab Audio to Sonos: Design

## Status

Approved in chat on 2026-09-02. This document specifies a Windows-oriented prototype that extends `swyh-rs-cli` and adds a Manifest V3 Chrome extension.

## Problem

`swyh-rs` can discover Sonos renderers, expose a live WAV stream, and tell a renderer to play it, but its current audio source is Windows loopback capture. That captures every system sound and cannot identify a single Chrome tab.

Chrome can capture a selected tab's audio, but a Chrome extension cannot expose the LAN HTTP server that Sonos needs. Sonos pulls a stream URL rather than accepting a browser `MediaStream`. The existing Rust process therefore remains the explicitly started LAN bridge.

Sonos buffers live HTTP audio. For video sites this makes audio arrive after the picture. The existing microphone calibration can measure the speaker path. The extension will use that measurement to delay the page's HTML video frames while leaving audio undelayed on its route to Sonos.

## Goals

1. Capture only the Chrome tab selected by an explicit extension action.
2. Keep the captured tab silent on Windows and send its audio to the explicitly running local Rust process.
3. Reuse existing SSDP discovery, renderer control, Sonos-facing HTTP streaming, silence handling, and microphone latency detection.
4. Let the extension select one discovered renderer, start and stop casting, run automatic calibration, and request manual recalibration.
5. Delay ordinary HTML5 video on YouTube and Plex by the calibrated Sonos latency plus a saved manual offset.
6. Restore the page and renderer cleanly after Stop, navigation, tab close, bridge loss, or an unsupported video path.
7. Implement the extension independently under this repository's MIT license without importing, vendoring, translating, or copying third-party GPL implementation code.

## Non-goals for the prototype

- DRM-protected audio or video
- Subtitle or caption synchronization
- Guaranteed 4K video-delay performance
- More than one captured tab or browser producer at a time
- Independent delays for multiple renderers
- Chrome Web Store publication or production installer integration
- Automatically launching `swyh-rs` from Chrome
- Replacing the existing desktop-loopback mode

## Prior art and source boundary

Existing extensions demonstrate that WebGL overlays can delay HTML video and that browser audio can be relayed to Sonos. They are evidence of feasibility, not implementation inputs.

The new code will use only:

- public Chrome, Web Audio, WebGL, and HTML media APIs;
- public Sonos/UPnP behavior already embodied in this repository;
- the repository's existing Rust modules; and
- original modules, tests, names, and control flow written for this project.

No GPL source file, snippet, build artifact, package, or derived translation will be added. The implementation may use the standard idea of timestamping captured frames and rendering them later; copyright does not attach to that abstract technique. The repository will retain MIT licensing for the new code.

## System architecture

```text
YouTube/Plex tab
    |
    +-- tab audio --> Chrome offscreen audio engine --> localhost WebSocket
    |                                                    |
    |                                                    v
    |                                           browser PCM publisher
    |                                                    |
    |                                                    v
    |                                          existing WAV HTTP stream --> Sonos
    |
    +-- HTMLVideoElement --> WebGL texture queue --> delayed overlay on original player
                                                       ^
                                                       |
                              measured latency + saved manual offset
```

There are two trust/network boundaries:

- `127.0.0.1:5902` is the authenticated browser bridge. It never listens on a LAN address.
- The existing configured HTTP port, normally LAN port 5901, remains the stream endpoint fetched by Sonos.

## Rust browser-source mode

### Startup

`swyh-rs-cli --browser-source` starts a dedicated source mode with these invariants:

- Stream input is 48,000 Hz, two-channel audio.
- Windows loopback capture and its silence-injection device are not started.
- The latency microphone listener, SSDP discovery, renderer state, and Sonos HTTP server still start.
- Browser PCM is the only producer allowed to publish samples into streaming clients.
- Normal invocation without `--browser-source` is unchanged.

Browser mode forces 16-bit WAV for that process session and logs the effective override without rewriting the user's normal desktop-mode format. Existing WAV stream-size configuration remains honored, with the current chunked WAV default preferred.

### Modules and ownership

The Rust change introduces focused units:

- `browser::protocol`: serde request/event types, protocol versioning, validation, and audio-session metadata.
- `browser::bridge`: loopback TCP/WebSocket listener, authentication, connection lifecycle, message-size limits, and one-producer enforcement.
- `browser::audio`: signed-16 little-endian stereo decoding, conversion to the existing shared `f32` sample representation, sequencing, and publication.
- `browser::commands`: command and event channel types exchanged with the CLI control loop.

The CLI main loop continues to own mutable renderer playback state. The bridge sends it commands rather than calling renderer methods from a socket thread. Binary audio frames bypass the CLI loop and publish through a small source-neutral audio publisher extracted from the existing capture distribution path.

The WebSocket loop uses bounded read timeouts so it can alternate between incoming audio/control frames and outgoing renderer/calibration events without blocking audio while calibration is pending.

### Pairing and security

On the first browser-mode start, Rust obtains 32 random bytes from the operating system, stores the hexadecimal pairing token in the existing protected user configuration, and prints a one-time pairing instruction. An explicit `--show-browser-token` option displays it again when needed.

The extension stores the pasted token in `chrome.storage.local`. A connection is usable only after a valid `hello` message. The bridge also:

- binds only to `127.0.0.1`;
- accepts the expected `chrome-extension://` Origin form and rejects ordinary web origins;
- compares tokens without data-dependent early exit;
- permits one authenticated producer;
- caps text, binary, and queued message sizes;
- rejects unexpected formats, rates, channel counts, and message order; and
- never exposes the token on the LAN-facing HTTP server.

An Origin header is defense in depth; the pairing token is the authorization boundary because non-browser local processes can forge origins.

## Browser protocol v1

The bridge endpoint is `ws://127.0.0.1:5902/browser/v1`. JSON control messages carry a `type`, protocol `version` where applicable, and `requestId` for commands that receive a result.

The initial message is:

```json
{
  "type": "hello",
  "version": 1,
  "token": "hex-token",
  "audio": {
    "sampleRate": 48000,
    "channels": 2,
    "sampleFormat": "s16le",
    "framesPerPacket": 960
  }
}
```

After `hello-ok`, binary messages contain interleaved signed-16 little-endian samples. A normal packet contains 960 stereo frames, or 20 ms. The bridge accepts shorter final packets but rejects odd byte lengths or packets above the configured bound. Sequence and timing telemetry are reported in periodic JSON statistics rather than prepended to every PCM packet.

Client commands are:

- `list-renderers`
- `cast` with an opaque renderer ID
- `stop`
- `calibrate`
- `set-manual-offset` with a bounded signed millisecond value
- `get-state`

Server events are:

- `hello-ok` or a terminal authentication/protocol error
- `renderer-list`
- `cast-state`
- `stream-state`
- `calibration-started`
- `calibration-result`
- `calibration-failed`
- `audio-stats`
- recoverable `warning` or terminal `error`

Renderer IDs derive from the renderer description/location identity, not just its IP address, because multiple logical renderers can share an address. Protocol types hide internal `Renderer` fields from the extension.

## Extension architecture

The extension is stored under `chrome-extension/` and uses Manifest V3 with a documented minimum Chrome version of 116.

### Service worker

The service worker handles the toolbar/user gesture, gets a `chrome.tabCapture` stream ID for the active tab, creates the offscreen document, and coordinates popup/content-script state. It does not process continuous audio because Manifest V3 workers can be suspended.

### Offscreen audio engine

The offscreen document consumes the stream ID through `getUserMedia`, using only the audio track. It intentionally does not connect that track to an `AudioContext` destination, so Chrome's captured-tab suppression keeps Windows playback silent.

An `AudioWorkletProcessor` receives channel data. The engine normalizes mono to stereo when necessary, clamps samples, converts them to interleaved `s16le`, accumulates 20 ms packets, and sends them over the authenticated WebSocket.

The engine observes `WebSocket.bufferedAmount`. It never allows old packets to accumulate into a growing latency tail. Above a short threshold it drops stale unpublished audio, increments telemetry, and resumes at the live edge. Rust's existing stream reader supplies dithered silence during short underflows.

### Popup

The popup exposes:

- companion connection and authentication status;
- pairing-token entry when required;
- discovered renderer selection;
- Cast and Stop;
- Calibrate;
- automatic-calibration status and measured milliseconds;
- a signed manual synchronization adjustment; and
- clear audio-only/fallback warnings.

Closing the popup does not stop an active cast.

### Content script and site adapters

The content script contains a site-neutral `VideoDelayController` and small YouTube and Plex adapters. An adapter selects the principal playing `HTMLVideoElement`, supplies a stable overlay host, and responds to player resize/fullscreen/navigation changes. The delay core does not contain site selectors.

The first prototype targets:

- `https://www.youtube.com/*`
- `https://app.plex.tv/*`; and
- user-configured/self-hosted Plex origins granted through optional host permission.

The manifest declares YouTube and hosted Plex directly. A self-hosted Plex origin is requested from the user with `optional_host_permissions`; after approval, the service worker registers the same repository-owned content script for that origin with `chrome.scripting.registerContentScripts`.

If an allowed page has several videos, the adapter selects the largest visible playing element and will not delay thumbnails.

## Timed WebGL video queue

### Frame capture and presentation

`requestVideoFrameCallback` signals a newly presented source frame. At most 30 frames per second are captured for the prototype. Each accepted frame is uploaded from the `HTMLVideoElement` into one reusable source texture. When resource budgeting requires a smaller queued frame, a framebuffer pass blits that source texture into a scaled queue texture before the source texture is reused. The resulting queue texture is stored with:

```text
due_at = monotonic_capture_time + effective_delay
```

`requestAnimationFrame` drives presentation. On each tick it retires all due entries, displays the newest due texture, and releases superseded textures. This timestamp model supports variable-rate media and displays without deriving delay from an assumed refresh rate.

The original video remains visible until the first texture has been captured successfully. The controller then inserts and shows a pointer-transparent WebGL canvas in the adapter-provided layer and hides only the source video pixels while preserving the site's controls and interaction layers.

### Delay changes, pause, and seek

The effective delay is:

```text
max(0, measured_sonos_latency + saved_manual_offset)
```

It is capped at 5,000 ms. When the value changes, queued due times shift by the delta. Reducing the delay retires newly overdue frames; increasing it holds the last displayed frame until the adjusted queue becomes due.

Already queued frames are not discarded on ordinary pause or seek. They drain while audio already buffered in Sonos drains. Newly decoded post-seek frames follow them with the same wall-clock delay. Navigation to a different media element creates a new controller and disposes the old one.

### Resource limits

The queue has both a frame-count cap and an estimated GPU-byte cap. The prototype:

- captures no faster than 30 FPS;
- never captures above 1920x1080;
- uses the two-stage framebuffer blit to scale queued dimensions downward when necessary to keep the planned delay within a 512 MiB estimated texture budget;
- refuses to go below 640x360 solely to maintain a long delay; and
- removes the overlay and reports audio-only fallback if allocation, cross-origin upload, context restoration, or the queue budget cannot be satisfied.

Every discarded or displayed texture is explicitly deleted. Resize, page visibility, WebGL context loss, Stop, connection loss, and controller replacement all have cleanup paths that restore the exact original inline style values.

## Casting and calibration flow

1. The user opens a YouTube or Plex playback tab and invokes the extension.
2. The extension authenticates, retrieves renderers, and the user selects one.
3. On Cast, Chrome capture starts and the offscreen engine begins sending live PCM.
4. Rust commands the renderer to fetch the existing WAV stream.
5. `StreamingState::Started` confirms that Sonos opened the HTTP stream.
6. After a short settling delay, Rust starts one calibration attempt by injecting the existing Barker pulse into the Sonos stream.
7. The configured Windows microphone detects the acoustic pulse and returns round-trip latency.
8. Rust sends `calibration-result`; the extension adds the saved per-renderer manual offset and updates the timed video queue.
9. The Calibrate button repeats steps 6 through 8 without rebuilding capture or the renderer stream.

Only one calibration may be armed at once. Automatic calibration does not use the current reconnect-until-threshold behavior; it needs the measured value. A timeout leaves casting active and uses the last saved result for that renderer, or zero plus the manual offset if no result exists. The popup displays that degraded state.

Calibration produces an audible test pulse. The popup indicates when it is about to run and when it completes.

## State and cleanup

The extension persists the pairing token, last renderer ID, last successful measured delay per renderer, manual offset per renderer, and user-granted Plex origins. It does not persist captured audio or frames.

Stop is idempotent and performs these actions:

1. tell Rust to stop the selected renderer;
2. stop tab capture and close the WebSocket;
3. tear down the AudioWorklet/offscreen session;
4. delete queued textures and canvases;
5. restore the source video's original styles; and
6. update popup/badge state.

Tab close, capture-track end, terminal bridge error, and WebSocket loss invoke the same cleanup. A normal in-tab navigation keeps capture only if Chrome keeps the capture track alive and a supported adapter successfully attaches to the new page; otherwise it stops safely.

## Failure behavior

| Failure | Required behavior |
|---|---|
| Rust unavailable | Disable Cast and show how to start browser-source mode. |
| Pairing rejected | Keep audio stopped and request a correct token. |
| No renderer | Keep capture stopped and show discovery status. |
| Sonos does not open stream | Report failure, stop the attempted renderer session, and keep the page unchanged. |
| Calibration times out | Continue audio with saved/manual delay and show a warning. |
| No eligible video | Continue audio-only and show a warning. |
| DRM/cross-origin WebGL upload fails | Restore original video and continue audio-only. |
| Queue cap or WebGL context fails | Free all textures, restore video, and continue audio-only. |
| Audio backpressure | Drop stale unpublished packets and report counters; never grow latency without bound. |
| WebSocket/capture terminates | Stop renderer and restore the page. |

## Verification strategy

### Rust automated tests

- pairing token generation/serialization and authentication outcomes;
- protocol version, ordering, format, and message-size rejection;
- `s16le` stereo decoding and conversion boundaries;
- browser-source isolation from loopback capture;
- one-producer enforcement and disconnect cleanup;
- renderer command routing with fake renderer/control responses;
- calibration success, busy, timeout, and event mapping; and
- an integration test that sends known PCM over a local WebSocket, opens the WAV endpoint as a simulated renderer, and validates headers plus corresponding non-silent sample bytes.

### Extension automated tests

Pure JavaScript units will be importable into Node's built-in test runner where browser APIs are not required. Tests cover:

- PCM clamping, stereo interleave, and 20 ms packetization;
- WebSocket backpressure/drop decisions;
- renderer state reduction and cleanup idempotency;
- dominant-video selection scoring;
- timed frame-queue ordering, delay increases/decreases, caps, pause/seek draining, and disposal; and
- YouTube/Plex adapter URL and layout decisions using small fakes.

A static verifier checks Manifest V3 permissions, referenced files, absence of remote executable code, and source-tree packaging.

### Build and static checks

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-features`
- Node extension tests and manifest verifier
- unpacked extension directory/zip contains only repository-owned source and assets
- Windows Rust build succeeds

### Windows acceptance

On Windows with Chrome, a configured microphone, and a Sonos renderer:

1. Start `swyh-rs-cli --browser-source` and pair the extension.
2. Cast ordinary 1080p YouTube playback. Confirm Windows output is silent and only the selected tab reaches Sonos.
3. Observe automatic calibration and confirm the video controller receives the measured delay.
4. Run manual Calibrate and confirm the delay updates without restarting capture.
5. Adjust the manual offset in both directions and confirm the overlay timing changes.
6. Seek, pause/resume, enter/leave fullscreen, and play the next video; confirm the player remains usable and cleanup has no stale canvas.
7. Repeat with Plex Web direct-play content.
8. Press Stop and confirm Sonos stops, capture ends, and the original video is restored.
9. Terminate Rust during a cast and confirm the extension performs the same safe cleanup.
10. Exercise a protected or deliberately unsupported video and confirm explicit audio-only fallback rather than a black player.

Hardware acceptance is recorded with app logs, extension diagnostics, measured latency, and pass/fail notes for both sites. Automated simulations prove protocol and queue behavior but do not substitute for this hardware check.

## Acceptance criteria

The prototype is complete when:

- normal desktop-loopback behavior remains passing and browser mode never mixes it with tab audio;
- the extension and explicitly running Rust app complete authenticated capture, renderer selection, cast, stop, automatic calibration, and manual calibration;
- the existing Sonos-facing server streams the selected tab's audio while Chrome leaves the PC output silent;
- independently implemented WebGL delay works on ordinary YouTube and Plex HTML5 video, including pause, seek, resize, and fullscreen, within the documented caps;
- every documented failure restores the original page or provides explicit audio-only fallback;
- automated Rust and extension checks pass; and
- Windows hardware acceptance evidence covers both YouTube and Plex.
