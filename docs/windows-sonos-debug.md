# Windows + Sonos streaming debug handoff

Use this doc to continue debugging on Windows without re-deriving context from chat history.

## Problem in one sentence

**Sonos (e.g. Era 300) sometimes stops the HTTP WAV stream after ~10–20s**, especially with **WASAPI loopback + idle / near-silence**. Suspects included **flat digital PCM**, **bad `Content-Length`**, and **capture / startup ordering**.

## What runtime evidence already showed

1. **`U32maxNotChunked` + tiny_http**  
   Huge non-chunked `Content-Length` correlated with Sonos dropping the connection.  
   **Mitigation:** default + config migration to **`U32maxChunked`** (chunked WAV); wizard sets chunked for WAV.

2. **Stale / confusing startup**  
   Logs implied capture before play.  
   **Mitigation:** defer loopback capture until just before `play()`, log “loopback active” after `stream.play()`.

3. **NDJSON (`.cursor/debug-2feb3d.log`)**  
   - **H4** `pcm_read_progress`: reads were advancing → **not** a stuck producer.  
   - **H1** `respond_ok`: HTTP response built OK → **not** proof Sonos kept reading.  
   - **H12** (dither): with old **1e-3** gate, **almost no** dither lines in a long run → **quiet-path dither rarely ran** while Sonos still disconnected.

4. **Dither direction (see `git log`, e.g. `75925b0`, package version 1.20.3+)**  
   - Looser **Ok-path** gate: **`max(peak, rms) ≤ 0.02`** (~−34 dBFS) → `maybe_dither_quiet_capture`.  
   - **`recv_timeout` synthetic silence:** **always** sub-LSB dither (no gate) + log tag `recv_timeout_silence`.  
   - **H12:** `dither_applied` with **`tag`** (`capture_ok` vs `recv_timeout_silence`), sampled every **60** applies.

## Key files

| Area | File |
|------|------|
| Chunked WAV / stream size defaults + migration | `src/utils/configuration.rs` |
| Wizard defaults | `src/utils/wizard.rs` |
| Dither + capture timeout silence + H4/H12 | `src/utils/rwstream.rs` |
| H1 respond | `src/server/streaming_server.rs` |
| NDJSON append | `src/debug_agent.rs` (path: `CARGO_MANIFEST_DIR/.cursor/debug-2feb3d.log`) |
| CLI / capture start | `src/bin/swyh-rs-cli.rs`, `src/utils/audiodevices.rs` |

## Local debug loop (Windows)

1. **`git pull`** → confirm recent `main` (commit message mentions dither / Sonos streaming fixes).  
2. **`cargo build --release`**.  
3. **Optional:** delete **`%REPO%\.cursor\debug-2feb3d.log`** before a run for clean NDJSON.  
4. Run **`swyh-rs-cli.exe`** (wizard or normal) with VB-Cable + Sonos.  
5. **Reproduce:** idle loopback **2+ minutes**; note exact time if Sonos drops.  
6. **Inspect:** `.cursor\debug-2feb3d.log`  
   - Lots of **`capture_ok`** H12 → dither is hitting idle buffers.  
   - Many **`recv_timeout_silence`** → capture thread slow vs `capture_timeout` (check `capture_timeout` in config, e.g. 2000 ms).  
   - **Few H12 but early drop** → dither hypothesis weak; consider **chunked WAV vs Sonos**, **DLNA headers**, or **`auto_resume` / re-`play()`** on stream end.

## Open hypotheses if drops persist

- **A:** Sonos dislikes **chunked WAV** for long sessions → try **LPCM** or different headers / `contentFeatures.dlna.org`.  
- **B:** Player closes for reasons other than “flat silence” (HTTP / DLNA policy) → **auto-resume** on `StreamingState::Ended`.  
- **C:** **`recv_timeout`** path dominates → tune **`capture_timeout`** or fix producer backpressure (channel full / drop).

## What “done” looks like

- Stream **stable ≥ 2 min** with **idle** loopback, **or**  
- Logs prove **which** hypothesis (H12 rate, stream end event) so the next change is targeted.

## Session / logging notes

- Debug NDJSON uses **session id** `2feb3d` in payloads (see `src/debug_agent.rs`).  
- Do not log secrets; keep `data` fields small.
