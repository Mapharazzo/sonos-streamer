# Managed Sonos control protocol v1

Managed mode separates an authenticated controller from the single browser audio
producer. Start it with `swyh-rs-cli --managed --managed-config <absolute-json-path>`.
Standalone CLI modes retain their separate configuration and behavior.

The supervisor creates a private token file containing 64 hexadecimal characters
and a configuration file (Windows example):

```json
{
  "version": 1,
  "controlAddress": "127.0.0.1:5902",
  "httpPort": 5901,
  "tokenPath": "C:/Users/you/AppData/Local/ElgatoRemote/sonos/token",
  "preferencesPath": "C:/Users/you/AppData/Local/ElgatoRemote/sonos/preferences.json"
}
```

Optional `interfaceAddress`, `outputDevice` (exact Windows playback device name)
and `microphone` settings customize hardware selection. Control always binds to
loopback; the audio HTTP port must be reachable from Sonos on the LAN. Missing
devices or an unavailable configured interface do not prevent idle readiness.
Discovery retries; capture and microphone streams are opened only on Start and
Calibrate respectively. Restart restores source, room and offset, never capture.
The selected speaker must fetch the current generation within 15 seconds or
playback fails and capture is closed. A browser reconnect requires a fresh
cleanup acknowledgment before another capture can begin.

Control uses loopback `/control/v1`; managed audio uses `/audio/v1`. A controller
authenticates with version 1, the private pairing token and its role (`controller`
or `browser`). The transport supplies the connection identity; a command cannot
claim another caller's identity. Authentication must not acquire the audio lease.

## Commands and results

The executable fixture at `tests/fixtures/managed-select-source.json` is the
cross-language command example. Each command carries `requestId`, `instanceId`,
`expectedRevision` and a tagged action. Unknown fields/actions, malformed IDs,
unsafe counters and out-of-range numeric parameters are rejected before dispatch.

Actions: `select-source`, `select-renderer`, `start-windows`, `prepare-browser`,
`start-browser`, `stop`, `set-volume`, `calibrate`, `set-offset`, `get-state`,
`capture-released`, `reload-tab`, `browser-status`, `browser-action-result`.
The authoritative Rust schema is `src/managed/protocol.rs`.

Results contain `type:result`, request ID, `status` (`ok`, `rejected`, `failed`),
code and revision. Transport/runtime failures use bounded codes, not raw errors.
An accepted command requests an operation; the state event confirms playback.
The runtime returns a browser lease only to the requesting browser controller.
Reload uses a separate unpredictable browser action ID. Its result is held until
that browser reports completion (or the bounded operation times out); another
controller cannot claim completion. `invoke-extension` means reload succeeded
but a fresh popup/shortcut invocation is required for capture. Transport replies
wait up to 20 seconds to accommodate the browser's 15-second reload limit.

Deduplication is scoped to authenticated controller connection and request ID.
The last 256 requests are cached per connection, with at most 16 connections.
An identical repeat gets the original result without repeating effects; changing
the payload under an existing ID is rejected. State is queried on reconnect;
old Start commands are not replayed. The Elgato client adds its durable ledger.

## State and transitions

State contains instance ID, revision, generation, selected source (`chrome` or
`windows`), phase, selected room and room list, confirmed volume, measured delay,
manual offset, an optional error code and control capabilities. Revision and
generation are bounded by JavaScript's maximum safe integer. Counter exhaustion
rejects mutations; runtime must retire the exhausted instance.

Startup is idle. Selecting a source never starts capture. Windows Start requires
a selected available room; browser Start additionally requires a prepared and
attached producer lease belonging to the browser controller. Renderer feedback
marks streaming only after Start and only for the current generation.

A source change retires the current audio generation and stops playback. When
Chrome owns capture, the state enters `switching` and asks its owner to release
capture. Only the matching owner and transition ID may acknowledge cleanup.
That acknowledgment is independent of display revision because volume/discovery
updates may have arrived while cleanup was in flight. Successful cleanup selects
the new source and leaves it idle; another deliberate Start is required.

Failed cleanup, a timed-out transition or browser disconnection stops playback
without switching into Windows capture. Stop during a transition cancels its
destination and creates a new cleanup transition. Delayed acknowledgments and
feedback cannot revive an old source. Local controller disconnection alone does
not own or stop playback.

The pure state machine returns typed effects for the runtime. It does not open
devices, issue SOAP requests or send network packets. Tests in
`tests/managed_state.rs` pin the wire and ownership behavior independently of
hardware; they do not claim real Sonos acceptance.

The managed socket bridge authenticates before dispatch, rejects ordinary web
origins, limits connections/messages, and broadcasts ordered state to each
controller. Slow readers are disconnected and recover from the latest snapshot.
Only the authenticated browser owner can mint a 30-second, single-use producer
lease, scoped to the server instance and audio generation. Controller disconnects
do not stop playback; producer/browser ownership loss is delivered to the runtime.
