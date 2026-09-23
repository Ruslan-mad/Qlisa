# Qlisa output destinations

The show preferences define named output destinations at
`preferences.display.output_destinations`; `default_output_id` chooses the
default. Each destination has a stable ID, name, kind (`display`, `ndi`, or
`srt`), enabled state, and kind-specific settings. Legacy workspaces can be
loaded and receive a deterministic physical `Main` destination when needed.

The default destination must be an enabled physical display. Network outputs
are selected explicitly in a visual cue's Output selector. A missing/disabled
destination must not silently redirect a cue to a different stream or display.

## Display outputs

Preferences → Display configures named physical/floating display outputs,
monitor binding, transforms, identify/test patterns, and default routing. Each
enabled destination has its own native output/render pipeline, mpv context,
slot/layer registry, and fade state. Floating output window geometry is stored
with the destination and constrained to a visible monitor on restore. Bound
screen outputs use fullscreen/locked window behavior.

Timers and test patterns route to the selected output. The output monitor is a
separate auxiliary window that can view an output source; it is not a routable
program destination itself.

## Per-output FTB

The transport bar shows one FTB control for each configured physical or
network output. Controls use the stable `output_id`; state is backend-owned and
volatile, so a frontend reload does not clear a blackout and Preferences never
stores it.

FTB draws a dedicated black overlay after the complete compositor and output
transform. It does not stop cues, change media position, or touch audio. A
physical output stays fullscreen. An NDI/SRT pipeline stays configured and
continues to send black frames, so receivers keep the same source.

The green `OK` state means the pipeline is active and healthy. Red `FTB` means
the final output is black. Amber `N/A` means the pipeline is unavailable; the
text and tooltip also report the reason.

The frontend reads `get_output_control_statuses` after startup and listens for
`output-control-status-changed`. It toggles one destination with
`toggle_output_ftb(output_id)`.

## NDI/SRT outputs

Preferences → NDI/SRT Output manages named network sinks. The Inspector's
Output selector routes a visual cue to one of these destinations. The backend
renders a hidden destination pipeline and submits its **final composited BGRA
frames**: cue geometry/crop, layer order/blend/opacity, fades, and output
transform are already applied.

- NDI has stream name/group and quality options. `LowBandwidth` caps submitted
  raster size at 1280×720 while preserving aspect ratio; `Highest` preserves the
  compositor raster dimensions.
- SRT supports listener/caller/rendezvous roles, host, port, latency, stream ID,
  payload size, encryption passphrase, late-packet drop, and encoder hints.
- An enabled destination starts its worker and waits for the first frame.
  Workers start/restart the selected sender, report local state and submitted /
  superseded frame counters, and expose the latest error.
- Frame submission replaces a pending stale frame instead of waiting for a
  slow encoder/network consumer. This prevents the output rendering thread from
  being paced by network delivery.
- A program-audio tap is available to network senders. NDI carries program
  audio; SRT carries program audio on Windows and is currently video-only on
  macOS/Linux. Check `engine/network_io.rs` before changing this platform rule.
- `Ready` in the provider panel means the local NDI runtime or SRT-capable
  FFmpeg can be loaded. Per-output `Streaming` means frames were accepted by a
  local sender/worker. Neither state proves a remote receiver is decoding or
  displaying the show.

The native transport and rendering code is in `src-tauri/src/engine/network_io.rs`
and `src-tauri/src/engine/output_engine/`; preferences and routing DTOs live in
`src-tauri/src/preferences.rs`. Current startup/resource lookup is in
`docs/windows-network-runtime.md`.

## Network inputs

Camera cues can use enumerated devices, NDI sources, and SRT sources. NDI source
discovery and receive use the loaded NDI runtime. SRT input uses the packaged
FFmpeg worker rather than depending on the system libmpv build's optional SRT
protocol. Input frame/audio processing feeds the existing visual/audio
composition path. Errors are retained as cue diagnostics so they can be
inspected after startup failure.

## Limits and validation

- Provider availability is machine-specific. A successful compile does not
  prove that NDI runtime initialization, a remote network path, or a particular
  capture device works on the target computer.
- Network status is local submission/worker status, not remote receiver
  telemetry.
- Retired output pipelines keep native resources alive through a safety
  lifetime policy; repeated reconfiguration can temporarily retain resources
  until process exit.
- Verify output routing, video/audio behavior, and installer contents on the
  target machine before show use or external distribution.

Useful checks live in `src-tauri/src/engine/network_io.rs` and
`src-tauri/tests/`. Do not quote historical test totals; run `cargo test` and
`pnpm test` on the current tree for fresh results.
