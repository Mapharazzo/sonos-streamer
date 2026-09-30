# Sonos Tab Caster

## Elgato Remote (recommended)

Install Elgato Remote on Windows. Its supervisor starts the Sonos companion automatically. Load the installed development extension from `%LOCALAPPDATA%\ElgatoRemote\sonos-extension\current`; remove the older development entry first. The fixed ID is `bhlhfokccmkkadhhhhkamndjfaekehjg`.

The popup connects automatically through Chrome Native Messaging. No token is shown, copied or stored in managed extension storage. If the app is missing or was repaired, open it and click **Connect**. Choose a room and audio source, then explicitly click **Cast current tab** or **Start Windows audio**. Calibration still uses **Calibrate & apply**. Refresh open media tabs after extension updates.

Ordinary HTTP/HTTPS tabs can cast audio with no video-delay script or additional site permission. The popup shows **Audio only — no video delay** and disables video calibration/manual adjustment for these tabs. Sonos still buffers playback; this mode does not promise zero audio latency. YouTube and Plex retain their video-sync controls. Chrome internal pages, extension pages, local files, and the Chrome Web Store cannot be selected for this mode. Chrome may also reject capture of protected content.

Multiple browsers or profiles can keep the managed extension connected at once.
They share room, source, volume and playback state; only one audio producer plays.
While another browser casts, **Cast this tab instead** stops its stream and waits
for capture cleanup before starting this tab. Stop and volume work from either
popup. Reload targets the browser that owns the active (or most recent) cast.
Opening/reconnecting an extension never takes over playback. If a competing
command or cleanup failure interrupts a handoff, press Cast again explicitly.
The browser shortcut can also switch Windows audio to a tab; choosing a source
alone still leaves it idle. Reload every installed copy after an extension update.

`npm run package` produces a development ZIP and matching native-host policy. Production packaging requires `node tools/package-extension.mjs --channel production --store-key <PUBLIC_BASE64_SPKI>` with the actual distinct store public key. Public signing/store publication remains a release prerequisite.

## Standalone legacy prototype

The following manual-token instructions apply only after explicitly choosing **Standalone tab caster (legacy)** in the popup.
This unpacked Manifest V3 extension captures audio from the tab selected with its toolbar popup, sends 48 kHz stereo PCM to an explicitly running `swyh-rs-cli` process on `127.0.0.1`, and asks that process to play the existing WAV stream on one Sonos renderer. Chrome's tab-capture behavior suppresses that tab's normal local output; this extension does not reconnect it to the Windows speakers.

For YouTube and Plex HTML5 video, the extension measures the Sonos path through the companion's microphone calibration and delays video frames by the measured latency plus a saved manual adjustment. The video implementation is repository-owned WebGL code; it does not import or copy another delay extension.

## Requirements

- Windows 10 or 11, Chrome 116 or newer, and a Sonos renderer on the same LAN.
- A microphone or line input that can hear the selected Sonos speaker for automatic and button-triggered calibration.
- The Rust companion left running in a terminal. The extension intentionally cannot install, launch, or hide it.
- Ordinary HTML5 direct-play video for visual delay. DRM-protected or origin-tainted video may continue as explicit audio-only playback.

## Build and start the Windows companion

From the repository in PowerShell:

```powershell
cargo build --release --bin swyh-rs-cli
.\target\release\swyh-rs-cli.exe --show-browser-token
```

The second command creates the pairing secret if necessary, prints it once, and exits. Treat it like a local password. Start the companion separately and keep its terminal open:

```powershell
.\target\release\swyh-rs-cli.exe --browser-source --mic "Microphone name or unique substring"
```

Omit `--mic` to use the Windows default input. Put the microphone close enough to hear the calibration signal. Do not use `--no_calibrate` when testing automatic synchronization. The defaults are HTTP port 5901 for Sonos and loopback WebSocket port 5902 for the extension. This prototype extension connects to port 5902, so do not override `--browser-port` during acceptance testing. If Windows Firewall asks, allow the companion on the private network so Sonos can fetch port 5901; the control bridge itself remains loopback-only.

## Load and pair the unpacked extension

1. Open `chrome://extensions`, enable **Developer mode**, and choose **Load unpacked**.
2. Select this `chrome-extension` directory. Alternatively, run `npm run package`, extract `dist/sonos-tab-caster.zip`, and load the extracted directory.
3. Open a playing web tab and click **Sonos Tab Caster** in Chrome's toolbar. Use YouTube or Plex for video sync.
4. Paste the token from `--show-browser-token`, click **Pair**, choose a discovered Sonos room, then click **Cast current tab**.
5. In Elgato Remote mode, click **Calibrate & apply** while casting. A successful measurement immediately sets the video delay and resets the previous manual adjustment to zero; a failed measurement keeps your existing settings. The popup shows the measured delay and the resulting video delay. You can fine-tune it afterwards using **Manual adjustment** and **Apply**. In standalone legacy mode, wait for automatic calibration or click **Run audible calibration**; that mode retains its saved manual adjustment.

The companion must already be running. Pairing or Cast does not launch a Windows executable.

## Plex

`https://app.plex.tv` is enabled by default. For a self-hosted Plex Web URL, expand **Self-hosted Plex** in the popup, enter the full origin such as `http://plex.lan:32400`, and click **Allow**. Chrome asks for that exact scheme, host, and port before registering the same packaged content script. Reload an already-open Plex tab after granting access.

Prefer direct-play HTML5 content. If Plex uses DRM, a protected surface, or a video upload WebGL cannot read, the original video is restored and the popup reports audio-only mode; Sonos audio can continue.

## Stop and recovery

**Stop** first asks the renderer to stop, then closes tab capture and its audio graph, disposes the video queue, restores the source video's exact inline styles, and disconnects the local bridge. The same cleanup runs for tab close/navigation, capture-track end, companion loss, and fatal protocol errors. Pair again to start a new session.

If the popup says the content script is unavailable, reload the media page after installing or granting its origin. After reloading or updating the extension itself, refresh any already-open media tab before casting again: Chrome invalidates the old content script. The updated handler catches that invalidation and disposes its old video overlay. If calibration times out, verify the Windows default input or the `--mic` name, microphone privacy permission, input level, and physical placement; the last successful measurement (or zero) remains active.

Useful diagnostics:

- companion terminal/log: renderer discovery, HTTP stream open, calibration pulse and result;
- `chrome://extensions` → Sonos Tab Caster → **service worker**: worker errors;
- media-tab DevTools console: content-script/WebGL fallback;
- popup counters and warning text: bridge and renderer state.

Never paste the pairing token into logs or issue reports.

## Standalone prototype limits

- One producing extension session, one captured tab, and one selected renderer at a time.
- Video sync supports YouTube, hosted Plex, and explicitly granted self-hosted Plex. Other ordinary web tabs support audio-only capture.
- Video capture at no more than 60 FPS and 1920×1080, with a 512 MiB estimated queued-texture budget. Longer delays reduce queued resolution to stay within that budget. Capture follows the frame presentation clock, avoiding the former callback-timing bug that could turn 30 FPS into 15 FPS.
- Effective video delay clamped to 0–5000 ms; budget fallback refuses scaling below 640×360.
- No protected-video bypass, PC-wide audio capture, system audio-delay driver, native-app auto-launch, Chrome Web Store signing, or unattended background service.

Run `npm test`, `npm run verify`, and `npm run package` from this directory for local checks. Hardware results belong in [`../docs/windows-chrome-sonos-acceptance.md`](../docs/windows-chrome-sonos-acceptance.md).
# Elgato Remote managed connection

The default **Cast current tab** shortcut is **Ctrl+Shift+8**, active while Chrome
has focus. The popup shows the binding Chrome actually assigned. If unassigned or
conflicting, set it at `chrome://extensions/shortcuts`; user remapping is respected.
The Stream Deck will target an observed Chrome window and invoke that binding.

**Reload / Retry tab** stops capture, reloads the last selected media tab once,
waits up to 15 seconds for the same origin, and attempts to restore seekable video
position. Live/protected media and autoplay restrictions may require manual
resume. Reload never manufactures a tab capture grant: press Cast or the shortcut
again after the reload. Closing/navigating away from the selected tab cancels
recovery. The recovery tab is kept only for the current Chrome browser session.

Choose **Elgato Remote** in the popup; it connects automatically to the installed
Windows client. Pairing keeps shared controls connected without
capturing audio. Choose a room and **Chrome tab** or **All Windows audio**. Changing
source stops playback; click Cast or Start afterward. Stop preserves the control
connection. The Windows client and Stream Deck share the same source and room.

The coordinated Windows installer provides the unpacked extension at
`%LOCALAPPDATA%\ElgatoRemote\sonos-extension\current`. Load that directory once
from `chrome://extensions` in Developer mode. No pairing code is needed. After upgrades,
Reload the extension card; keep the same directory to preserve its identity.
Managed mode supports all Windows audio through the companion; the standalone
prototype limits above apply to the legacy connection.

On generic audio-only tabs, Reload / Retry waits for the page to load without a
video adapter; resume the site's playback manually and press Cast again.

**Standalone tab caster (legacy)** explicitly selects the original `/browser/v1`
companion. Supply that companion's token when changing connection modes. The
extension never retries a managed mutation against the legacy endpoint.
