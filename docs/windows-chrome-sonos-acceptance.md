# Windows Chrome-to-Sonos acceptance record

This is the hardware acceptance ledger for the Chrome tab-casting prototype. Automated tests prove packet, protocol, state, and queue behavior; they do not prove Windows tab suppression, a real microphone round trip, Sonos buffering, or site-specific rendering.

## Test identity

Fill these fields before testing and keep evidence under a timestamped directory outside the package artifact.

| Field | Value |
| --- | --- |
| Date/time and timezone | Pending |
| Git commit | Pending |
| Windows edition/build | Pending |
| Chrome version | Pending |
| Companion command line (redact token) | Pending |
| PC network/interface | Pending |
| Microphone model/input name | Pending |
| Sonos room and model | Pending |
| Sonos firmware/app version | Pending |
| YouTube URL/content | Pending |
| Plex origin/content/direct-play status | Pending |
| Evidence root path | Pending |

Record the raw calibration result separately from the signed manual offset. Never record the pairing token.

## Ten-step hardware table

Use `PASS`, `FAIL`, or `BLOCKED` in Result. Every failure needs a timestamp, observed/expected behavior, relevant companion and Chrome diagnostics, and a reproducible evidence path.

| # | Acceptance action and expected evidence | Date | Commit | Chrome version | Sonos model/version | Measured delay | Result | Evidence path |
| ---: | --- | --- | --- | --- | --- | ---: | --- | --- |
| 1 | Start `swyh-rs-cli --browser-source`, explicitly reveal the saved token, load the unpacked extension, pair, discover rooms, and select one renderer. | Pending | Pending | Pending | Pending | N/A | Pending | Pending |
| 2 | Cast ordinary 1080p YouTube playback. Confirm Windows output is silent and only the selected tab reaches Sonos; unrelated tabs are not captured. | Pending | Pending | Pending | Pending | N/A | Pending | Pending |
| 3 | Observe the audible automatic calibration. Record the raw result and confirm the captured tab receives the resulting effective video delay. | Pending | Pending | Pending | Pending | Pending | Pending | Pending |
| 4 | Press **Run audible calibration**. Confirm one new measurement completes and updates delay without restarting tab capture or Sonos playback. | Pending | Pending | Pending | Pending | Pending | Pending | Pending |
| 5 | Apply negative and positive manual offsets. Confirm each changes lip sync once, clamps within 0–5000 ms effective delay, and does not recalibrate. | Pending | Pending | Pending | Pending | Pending + offsets | Pending | Pending |
| 6 | On YouTube, seek, pause/resume, resize, enter/leave fullscreen, and play the next video. Confirm controls remain above the overlay and no stale canvas remains. | Pending | Pending | Pending | Pending | Pending | Pending | Pending |
| 7 | Repeat casting and interaction checks with Plex Web direct-play content. Record the exact Plex origin and whether explicit self-hosted permission was used. | Pending | Pending | Pending | Pending | Pending | Pending | Pending |
| 8 | Press **Stop**. Confirm Sonos stops, Chrome capture ends, the bridge closes, and the exact original video presentation is restored. | Pending | Pending | Pending | Pending | N/A | Pending | Pending |
| 9 | Terminate the Rust companion during a cast. Confirm the extension performs the same one-shot renderer/capture/audio/video cleanup and reports disconnection. | Pending | Pending | Pending | Pending | N/A | Pending | Pending |
| 10 | Exercise protected or deliberately unsupported video. Confirm explicit audio-only fallback, original video restoration, usable controls, and no black player or retry loop. | Pending | Pending | Pending | Pending | N/A | Pending | Pending |

## Evidence capture

For each row, save:

- a short screen recording or timestamped screenshots showing popup state and the media player;
- the companion log covering Cast, HTTP stream open, calibration, and Stop/failure;
- relevant service-worker and media-tab console output with the pairing token redacted;
- the raw latency, manual offset, and observed effective delay; and
- concise observed-versus-expected notes.

Before row 2, open Windows Volume Mixer and establish that the tab is audible locally before capture. During Cast, record that its local meter/output is suppressed while Sonos plays. For the unrelated-tab check, briefly play a distinct sound in a second tab and confirm it does not reach Sonos.

## Final sign-off

Acceptance is complete only when all ten rows pass on both ordinary YouTube and Plex direct-play paths, or after evidence-backed defects have received red-green automated tests, fixes, and passing reruns. Add the fresh outputs of:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cd chrome-extension
npm test
npm run verify
npm run package
cargo build --release --all-features   # on Windows
```

| Sign-off field | Value |
| --- | --- |
| Tester | Pending |
| Final commit | Pending |
| All rows pass | Pending |
| Automated verification evidence | Pending |
| Windows release artifact hash/path | Pending |
| Notes/open defects | Pending |
