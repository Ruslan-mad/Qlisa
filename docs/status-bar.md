# Status bar

This guide describes an unreleased feature in the current source checkout. It is
not part of the published 1.5.13 installer.

The status bar sits at the bottom of the main window. Settings → Personalization
→ Status bar controls visibility, metric sides, order, and enabled state. The
default layout enables eight metrics and leaves five optional metrics disabled.
Move a metric between sides and use the up/down controls to reorder it. The GPU
selector chooses an adapter by its stable Windows adapter ID; Automatic picks
an available adapter with dedicated memory. Preferences apply from the settings
draft when the user saves them.

## Metrics

| Metric | Meaning |
| --- | --- |
| Cue count | Recursive count for the selected Cue List. The tooltip separates Groups, Numbers, and disabled cues. |
| Duration | Sum of declared finite durations for enabled top-level Cue List items, including media trim/repeats and waits. Group and Number child durations are included once. This is an authored duration estimate, not guaranteed show runtime: manual GO timing and parallel playback can change elapsed time. `+∞` marks infinite playback; `+?` marks unknown duration. |
| Active | Unique active cues in the selected list. The tooltip also shows unique active cues across the workspace. |
| Problems | Cached validation problems and missing-media counts for the selected list. This does not scan media files. |
| CPU, RAM | Windows system load and memory. RAM shows used and total memory. |
| GPU, VRAM | Selected adapter utilization, video-decode utilization, and dedicated memory. The selected adapter name appears in the tooltip. |
| Audio gaps | Lifetime underrun events and inserted silent output frames since the audio engine started. Initial stream-readiness waits and control-seek rebuffer silence are excluded. |
| Video FPS | Successful physical display presents per visible output, sampled as a rate. The tooltip lists outputs and MPV-reported dropped frames. It does not compare against container FPS or monitor refresh rate. |
| Network drops | Active NDI/SRT input and output counters with separate video-frame, audio-sample, and audio-frame units. Queue pacing replacements are shown separately and are not packet loss. |
| Qlisa memory | The Qlisa process working set. |
| Disk | Free space on the project volume, or on the application profile volume when no project path is available. |

## Sampling and unavailable data

The UI polls requested metric families at 1 Hz while the status bar is visible.
System sampling runs in a background worker and stops its Windows PDH/DXGI
sampling handles after five seconds without a reader. Disk capacity is checked
in a background worker at most every ten seconds. Runtime counters are read
from cached or atomic state. MPV drop properties are sampled in a background
worker at most once per second. These reads do not decode media, rebuild
waveforms, or probe files. Audio underrun counters use atomic increments on the
audio callback; that callback does not perform expensive probes or lock for
these metrics.

CPU, RAM, and process memory use Windows APIs. GPU utilization and dedicated
memory depend on Windows WDDM counters and driver support. Other operating
systems, unavailable counters, missing providers, or busy data sources can
return no sample. The UI shows `—` for unavailable data; it does not treat
missing data as zero. Old samples fade and are marked stale.

The video FPS value is based on successful physical display presents, not
network outputs or hidden/headless output windows. Decoder drop counts come
from MPV when available. Network counters report local input/output queue
drops only. They do not measure remote packet loss. `Superseded` counts describe
bounded output queue pacing and remain separate because replacing an older
frame can be normal when an output runs at a lower rate.

## Verification on 2026-10-07

- Rust tests: 1,017 passed. Frontend tests: 513 passed. TypeScript caught an
  optional `runtime.videoOutputs` snapshot-field mismatch during review; that
  type issue was fixed.
- `pnpm tauri:check` passed and built the debug app.
- A separate Windows hardware probe reported an NVIDIA RTX 3080 with
  12,678,332,416 bytes of dedicated memory, Intel UHD 770, and CPU/RAM samples.
  The probe also verified idle and resume behavior for system sampling.
- Chrome UI checks used mock IPC. They covered Preferences Apply/Cancel,
  metric ordering, GPU selection, drop display, stale state, and list changes.
- Keyboard overflow check passed at a 1,024 px viewport width. The left strip was
  512 px wide and its content was 625 px; End reached scroll position 113 and
  Home returned to position 0. The fixture contained eight cues.
- A follow-up found that the native `update_display_preferences` command used a
  field whitelist that omitted `status_bar`. The earlier mock IPC fixture did
  not exercise that backend write. The command now persists the status bar
  preferences, and a Rust regression test covers the production update helper
  while checking that output routing fields remain unchanged. All 15 targeted
  `preferences_cmds` Rust tests passed after the fix. The native GUI has not yet
  been re-tested with this fix.

These checks do not establish real physical-display FPS or drop rates, audio
underrun rates on playback hardware, NDI/SRT receiver behavior, or full native
GUI end-to-end behavior.
