# Network media I/O: current implementation

This note reflects the source in this checkout. It replaces an older design
draft that described the sender/receiver workers as planned work. The current
implementation is in `src-tauri/src/engine/network_io.rs` and
`src-tauri/src/engine/output_engine/`.

## Architecture

```text
OutputDestination preferences
   ├── display ── native output pipeline + mpv/OpenGL compositor
   ├── NDI ─────── hidden compositor pipeline ── bounded latest-frame handoff ── NDI worker
   └── SRT ─────── hidden compositor pipeline ── bounded latest-frame handoff ── FFmpeg worker
                                                        ▲
AudioEngine post-master tap ───────────────────────────┘
```

`output_engine::configure_outputs` validates output records, prepares network
workers/audio taps, creates the required native pipeline(s), and then reconciles
the active routing map. Each visual cue resolves an explicit `output_id`, the
configured default, and the validated enabled fallback according to the output
registry. Network sinks are explicit cue destinations; they are not display
tabs or physical monitor selections.

The compositor hands off final BGRA frames after cue layering, geometry,
opacity/blending/fades and output transforms. Each network destination has a
worker and latest-frame slot. Replacing a pending frame increments the
superseded counter; a slow sender does not block the GL render thread. Sender
workers start on the first frame and update per-output `WaitingForFrame`,
`Streaming`, `Error`, or `Stopped` status.

The audio callback exposes bounded post-master subscriptions. The callback
produces into a ring without waiting for a network worker. NDI sends video and
audio. SRT uses FFmpeg and sends audio on Windows; the current non-Windows SRT
worker sends video only. Format changes invalidate/restart the affected
network worker rather than silently sending at the wrong sample rate.

The separate Diagnostics window includes live network input/output status and
worker counters. These describe local worker state and submissions; they do not
confirm that a remote receiver decoded or displayed the stream. Live SRT input
audio loss is reported as a warning while video continues, not as a complete
connection failure.

## Protocol workers and inputs

- **NDI:** dynamically load the NDI runtime and resolve the required SDK
  symbols; no NDI import library is needed to compile. The same runtime layer
  supports source discovery, receive, and send.
- **SRT output/input:** use the application-packaged FFmpeg executable when
  available. Windows release packaging stages a pinned external FFmpeg build;
  the binary is not stored in Git. Runtime and installer details are described in
  `windows-network-runtime.md`. SRT settings are validated before worker
  startup; credentials must remain redacted in logs and status DTOs.
- **Inputs:** Camera cue setup chooses an enumerated local camera, NDI source,
  or validated SRT settings. NDI receive uses the dynamic SDK; SRT decode uses
  FFmpeg pipes into the media/output path. Input errors become cue diagnostics.

The Preferences runtime probe only tests whether its local transport runtime
can load; it does not bind a socket or prove a receiver is available. The
per-destination output status updates only when the sender worker processes
frames. Neither is remote receiver confirmation.

## Packaging and tests

Windows bundling maps staged FFmpeg files from `src-tauri/vendor/ffmpeg`
through `src-tauri/tauri.windows.conf.json`. `build.rs` rejects missing/empty
FFmpeg payloads for release builds. The pinned FFmpeg archive is fetched and
staged outside Git; the NDI Runtime is installed separately by the user and
must never be copied into Qlisa or its installer. Windows libmpv is also a
local prerequisite and is not tracked. See `windows-network-runtime.md` for
the pinned build and notices.

Pure validation, queue behavior, frame/audio handling, and worker state tests
are in the network module. NDI integration tests require a working runtime and
source; end-to-end stream delivery requires a receiving machine/application.
Run `cargo test` for the current tree and separately validate physical network
paths on the deployment computer.
