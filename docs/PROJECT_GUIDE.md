# Qlisa project guide

This is the technical handoff for a coding session starting in this checkout.
It describes the source tree as it exists, rather than proposals or external
product research. Check `git status` before editing: this workspace may contain
uncommitted restoration or feature work. Do not assume the checked-out commit
alone represents the running desktop build.

## Product and current baseline

Qlisa is a live-show cue-list application, derived from Inkue. The release
command keeps the application version synchronized in `package.json`,
`src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, and `src-tauri/Cargo.lock`.
`.inkue` workspaces and several internal `inkue` names are retained for
compatibility. This is not an official upstream Inkue release checkout. The
current version in this checkout is 1.5.4. The source repository is public at
<https://github.com/Ruslan-mad/Qlisa>. Check its Releases page for binary
downloads. The Tauri updater is configured for signed GitHub Releases. The
1.5.4 local updater E2E check passed. The 1.5.5 per-machine installation and
runtime-bootstrap checks are pending. Runtime source and license evidence is
tracked separately; fetching the media files from upstream does not establish
legal compliance. See [Windows release preparation](RELEASING.md).

The app has a React/TypeScript Tauri front end and a Rust backend. The backend
owns cue state, playback, native audio/video/output windows, networking,
lighting, timecode, workspace persistence, and device integration. The UI
issues typed Tauri commands and listens for backend events.

Features represented in the current tree include:

- Cue-list editing, multi-selection, reorder/group, cue colors and notes,
  undo/redo, cart/show views, inspector, keyboard transport, and multiple cue
  lists.
- Audio, Video, Image, Camera, Text, Memo, Wait, Group, Stop, Fade, Devamp,
  Start/Pause/Resume/Load/Reset/Goto/Arm/Disarm, OSC, MIDI, MIDI File, Light,
  Mic, Timecode, and Script cues.
- Audio trim/rate/loop/slices, fades and levels; video/image geometry/layers,
  output routing and fades; headphone preview; NDI/SRT input and output;
  MIDI/OSC/timecode triggers; sACN/Art-Net lighting; QLab workspace import.
- Number cues have a shared editable timeline, nested Group tracks, Group or
  media masters, offsets, fades, and start/finish flow actions. Media conversion
  supports queued Audio/Video/Image jobs. Audio, video, and network diagnostics
  open in a separate window; Camera cues expose live-input audio controls.
- Global physical fullscreen controls can show or hide physical display outputs
  and assign monitors. Qlisa enforces one running instance; reopening the app
  focuses the existing instance.
- Named display outputs and separate Mixer, Output Monitor, and floating timer
  windows. The main app can also degrade to an audio-only/headless-output
  session when an output engine dependency cannot be started.

This list describes code/UI support, not proof that every device/driver or
network combination has been physically validated on every operating system.

## Repository map

```text
src/                              React + TypeScript app
  App.tsx                         Main window composition and top-level dialogs
  components/CueList/             Cue table, cart mode, row/context interactions
  components/Inspector/            Per-cue editors, media preview, clip controls
  components/Transport/            GO/STOP/PAUSE controls
  components/ShowMode/              Large operator view
  components/Preferences/           General, audio, display, MIDI, network settings
  components/Lighting/              Fixture patch, dashboard, groups
  components/OutputPatches/          Workspace output patch editor
  components/InputPatches/           Live audio input patch editor
  components/OscPatches/             Workspace OSC patch editor
  components/Editor/                 Clip/timeline editing
  components/ActiveCues/             Active cue view
  windows/                           Mixer, output monitor, floating window UIs
  stores/                            Zustand workspace/transport/timing/update stores
  hooks/                             Tauri event and keyboard integration
  lib/types.ts                       Shared frontend DTOs and persisted shapes
  lib/commands.ts                    Typed invoke wrappers for Tauri commands
  i18n/                              English/Russian dictionaries and locale logic

src-tauri/src/
  lib.rs                              Tauri setup, command registration, background services
  state/app_state.rs                  Shared managed state and CueRegistry setup
  cue/                                Cue trait, types, factories, cue implementations
  show/                               Workspace, cue lists, transport, event loop, undo
  engine/                             Audio, output, media, network, MIDI, OSC, DMX, timecode
  engine/output_engine/               Native destination pipelines and compositor
  commands/                           Tauri IPC handlers by domain
  preferences.rs                      Global preference types + workspace mirror/schema
  machine_config.rs                   Machine-local device/runtime settings
  qlab_import/                        Native QLab archive and cue mapping
  recovery.rs                         Crash-recovery workspace snapshot
  tests/                              Rust integration tests and media fixtures
  capabilities/                       Tauri window permissions
  tauri*.conf.json                    App windows and platform bundle resources

docs/                                  Project guide and focused implementation notes
vendor/asiosdk/                        Required Windows ASIO SDK for Qlisa builds
src-tauri/vendor/ffmpeg/               FFmpeg license/provenance notices
scripts/                               Native runtime sync and asset helpers
```

Some source files and thread labels still use Inkue identifiers intentionally.
Do not rename persisted keys, command/event names, `.inkue` extensions, or OSC
addresses as a cosmetic cleanup without a migration plan.

## Architecture and data flow

```text
React UI
   │ typed invoke()                         backend emit() events
   ▼                                                  ▲
commands/* ── Workspace + CueRegistry ── show/transport + event_loop
                              │                         │
                              ├── Cue implementations ─┤
                              │                         ▼
                              └──────────── engines (audio/output/DMX/network/MIDI…)
```

- **`engine/`** implements media/device primitives. It should not make cue-list
  or playhead decisions.
- **`cue/`** owns per-cue configuration, runtime state, and lifecycle methods.
  `CueRegistry` creates/deserializes cue implementations behind the object-safe
  `Cue` trait.
- **`show/`** owns workspaces, cue-list structure, playhead, transport ordering,
  timed completion/continue chains, event dispatch, and undo history.
- **`commands/`** validates/maps UI requests into workspace/transport/engine
  operations. Frontend code should call wrappers in `src/lib/commands.ts`, not
  raw command strings scattered through components.
- **`src/lib/types.ts`** is the shared UI contract. Keep it aligned with Rust
  command DTOs and serde representations.
- **`src/hooks/useTauriEvents.ts`** subscribes to backend updates. Backend
  change notifications are event-driven; do not add frequent UI polling for
  state that already has an event.

The main mutable show object is `Arc<Mutex<Workspace>>`, managed through
`AppState`. Commands and the show loop serialize mutations through it. Keep
critical sections short: filesystem work, media decoding, network setup, and
native window creation should happen outside the workspace lock where the
existing command does so. Native windows must obey the platform main-thread
requirements in their owning module.

## Cue lifecycle and transport contract

`CueState` is `Standby`, `Running`, `Paused`, or `Completed`. Most cues share
identity, cue number/name/color/notes, pre/post-wait, continue mode, serialize,
GO/stop/pause/resume/tick methods through `cue/traits.rs`. Runtime details may
be snapshotted/reapplied when an inspector edit rebuilds a cue; preserve that
behavior when editing cue serialization or update commands.

**GO** takes the cue at the list Playhead, advances the outer Playhead, applies
the cue action, performs declared Stop/Fade/Devamp/Control behavior, then
evaluates immediate chaining. Auto-Continue with zero post-wait can chain
immediately; otherwise the 30 Hz show event loop evaluates post-wait/completion.
Auto-Follow chains when the action starts or the cue finishes, depending on the
cue's runtime semantics. Disabled cues are skipped. Sequential/looping Playlist
groups can retain and absorb the outer GO until their child sequence releases
the Playhead.

- **STOP** requests the cue's normal stop behavior. The default audio fade-out
  is `DEFAULT_FADE_OUT_MS = 500`; a cue may have its own stop semantics.
- **Hard Stop** cuts the selected cue immediately. **Hard Stop All** is the
  panic path: it hard-stops known running cues, clears engine voices/output,
  then resets stale cue bookkeeping. The UI maps double Escape to this action.
- **Pause/resume** retains elapsed/action/media position. Seek is allowed for
  supported media cues while paused. Audio callback state is changed by queued
  engine commands, not by locking the callback from the UI.
- A **Stop Cue** targets all or selected cue IDs. A **Fade Cue** can target
  nested cue voices, controls audio gain/pan and/or a visual layer, and can
  stop the target cues when the fade completes.
- **Cue number** is a string (`"1"`, `"1.5"`, `"Intro"`); selection is separate
  from the Playhead. Do not treat list position or displayed cue number as a
  stable cue identity; use UUID `CueId`.
- Group modes are Simultaneous, Sequential, Playlist (exclusive child at a
  time, optional wrap), and Start Random (shuffle-bag). A group may own multiple
  child voices. Targeting/stopping/fading a group must resolve child voices
  recursively (`all_voice_ids`, `get_recursive`).

Visual Video/Image/Camera cues are compositor layers; starting a new layer does
not generally stop other visual layers. Text cues have their own next-GO
behavior. Fade a visual cue through its own layer opacity, not a global fade
overlay; the global curtain is reserved for blackout/panic behavior. Video's
picture and decoded audio voice have separate engine objects and must keep
start/pause/seek/loop/stop/EOF state aligned.

## Media, audio, video, and preview

### Audio engine

`engine/audio_engine.rs` owns cpal output streams and the real-time callback;
`engine/voice.rs` contains per-voice signal/fade/routing state. Audio files
normally decode through `StreamingAudioSource` into bounded PCM rings
(`cue/media_decode.rs`); the callback consumes ready samples without waiting on
the decoder. The legacy whole-file PCM path remains as a fallback. A Voice is
then rendered/mixed by the callback. Multiple devices/output patches use
additional streams/voice pools where configured.

**Real-time invariant:** the cpal callback does not allocate, block on a mutex,
or perform file/network/device I/O. Commands arrive through bounded lock-free
rings; status goes back through rings/atomics. Avoid logging, heap growth, file
access, Tauri events, or workspace locking from the callback. The audio callback
also serves live input feeds and optional post-master network audio taps.

The Windows Qlisa application always uses the Cargo feature `asio-support`.
Build or run the Windows app with an ASIO-enabled command; do not omit ASIO.
Use the versioned release command for Windows release builds, since the
lower-level `pnpm tauri:build` also creates installers. macOS and Linux use
their normal cpal host (CoreAudio, ALSA/PipeWire, depending on host
availability). Mic Cue
capture is a separate input stream with a bounded ring/resampler; it is not an
audio file voice.

### Video and outputs

`engine/output_engine/` owns a named native output pipeline, libmpv context,
slot registry, GL render/composite path, per-layer geometry/blend/opacity, and
fade state. There are independent pipelines for configured destinations. A
video cue's image is rendered by mpv; its audio is decoded/played through the
audio engine so it can share audio levels, routing, and stop controls.

Each visual cue owns a slot/layer. Keep layer order, slot ownership and voice
IDs aligned when editing. A cue may be nested in a group, so lifecycle cleanup
cannot assume a cue is top-level. The output render path can submit final
composited BGRA frames to a selected NDI/SRT worker. Physical display and
network destinations are distinct destination kinds. The default destination
must remain an enabled physical display; network output is selected explicitly.

Video/image editing includes media thumbnails/metadata, scrub/seek, trims,
geometry, layer properties, and a video preview. Headphone preview is a separate
operator-local session, held in `AppState::preview_session`, outside the show
workspace/playhead. There is at most one active preview voice. Its monotonically
increasing generation token prevents a slow/stale decode or late completion
event from replacing/clearing a newer preview. Media metadata probes are
deduplicated and run in background workers; the cue-list summary reads a cache
snapshot rather than probing files on every render.

The transport bar's global fullscreen control lists physical display outputs,
shows or hides them, and assigns a monitor to an output. It controls physical
display windows; network destinations remain explicitly routed outputs.

With an ASIO backend, Settings > Audio can route preview to a different stereo
pair of the already-open main stream. The pair must exist and must differ from
the main program pair. These preview voices are excluded from the canonical
program bus, program VU, Output Patch VU, and NDI/SRT audio taps. Invalid or
unavailable pairs return an error and never fall back to the program output.
Separate-device preview remains available.
Applying the setting rejects a main-device Output Patch that uses either
reserved preview channel. Runtime submission also rejects later-loaded stereo
or level-matrix routes that target those channels and raises an error alert.

The default Settings > Audio view is intentionally simple. It shows the main
and preview outputs as device, channel/stereo pair, runtime status, and Test.
Backend, buffer size, input device, and Output/Input patches are under the
non-persisted **Advanced settings** toggle. ASIO preview and its test tone use
an isolated preview voice on the selected pair; separate-device preview uses an
aux route. Neither route enters the program bus or NDI/SRT audio taps.

The exclusive logical cue buses are `Main` and enabled `Aux` buses; each cue
selects one. Preview/Headphones remains an operator-only route. Physical Aux
device/channel bindings are machine-local, while cue bus assignments stay in
the workspace. An enabled Aux without a local binding is unavailable and fails
preflight/GO instead of silently routing to Main. See
[audio bus routing](audio-bus-routing.md) for migration and compatibility
details.

The auxiliary windows are declared in `tauri.conf.json`; their frontend entries
are under `src/windows/`. Tauri capabilities for each window are declared in
`src-tauri/capabilities/`; a new window often needs a capability as well as a
frontend component.

### NDI and SRT

Network settings are stored with named destinations in
`preferences.display.output_destinations`; see `qlisa-multi-output.md` for
routing and runtime details. `engine/network_io.rs` contains shared config,
runtime discovery, NDI dynamic loading, NDI sender/receiver workers, SRT FFmpeg
sender/input workers, queues, and live status. `output_engine/mod.rs` connects
composited frames and program-audio taps to those workers.

- Windows builds do not stage or bundle FFmpeg, ffprobe, or libmpv. On first
  launch, a preparation window downloads the pinned FFmpeg pair and libmpv
  directly from their upstream URLs, checks their SHA256 hashes, and installs
  them under `%LOCALAPPDATA%\Qlisa\runtime\` before opening the main window.
  Missing or damaged files are repaired automatically; preparation errors offer
  Retry or Close. NDI is dynamically loaded from the user's separately
  installed NDI Runtime; the NDI DLL is never bundled by Qlisa.
- The SRT sender carries program audio and video on Windows. On macOS/Linux,
  current SRT output is video-only; NDI carries program audio on all supported
  platforms according to the code's status text.
- Video frame queues use a latest-frame policy: slow consumers supersede stale
  images instead of blocking the output render path. Network status reports
  local sender state and counters, not that a remote receiver is displaying the
  stream.
- The NDI Runtime and FFmpeg searches are documented in
  `windows-network-runtime.md`. Do not log SRT passphrases or raw secret URLs.

NDI/SRT Camera input is implemented through the native NDI receiver or FFmpeg
SRT worker feeding the visual cue/output path. A network source error is exposed
on the cue. NDI runtime discovery and actual delivery are different conditions;
`Ready` means the local provider is usable, while per-output state changes to
`Streaming` only after frames are submitted.
Camera cues can adjust or mute the live source's audio while it is active. This
controls the camera's attached audio voice; it does not pause the live source.

## Workspace persistence and configuration

- A `.inkue` is a JSON workspace containing metadata, cue lists/cues (including
  group children), a compatibility/runtime preferences mirror, audio
  input/output patches, OSC
  patches, DMX universe mappings, fixture definitions/groups, and the active
  cue-list ID.
- File paths are recursively made relative to the workspace's parent directory
  when possible. Files outside that directory or on another drive remain
  absolute. Loading resolves relative paths against the workspace location.
  **Collect and Save** copies referenced media into a destination folder and
  updates references for portability.
- Workspace schema version is declared in `show/workspace.rs`. Unknown or bad
  cue JSON can be skipped and counted; the app should tell the operator instead
  of silently losing cues.
- Unsaved edits are snapshotted periodically to a crash-recovery file. A clean
  exit removes the current recovery snapshot; explicit recovery actions live in
  `recovery.rs` and `commands/recovery_cmds.rs`.
- `AppPreferences` is machine-global in `%APPDATA%/Inkue/preferences.json`.
  It contains the General/Audio defaults, display/theme/timer/Clip Editor
  visibility, named display/NDI/SRT destinations, and the global default
  output. Physical audio device/backend/buffer state remains in `audio.json`;
  OSC, network interface, timecode, and MIDI state remain in their existing
  machine files. `Workspace.preferences` stays in the schema as a
  compatibility/runtime mirror and is not authoritative or dirty when only
  Settings change.
- When the global file is absent, the first real old workspace (after output
  migration) seeds it; New before a workspace load may seed current defaults.
  Once present, Open/Recovery overlay the global tree and append only unknown
  legacy output IDs once. Existing global IDs and the global default output
  always win. Project-specific cues, patches, and other show data remain in
  `.inkue`; Project Settings are not a separate UI yet.
- Cue identity uses UUIDs. Display numbers may be automatically renumbered on
  reorder depending on preferences, and explicit renumber commands exist.

When changing a persisted field: add serde defaults/migration behavior, update
Rust and `src/lib/types.ts`, update the command/store/editor path, and add a
compatibility test for old workspaces where relevant. Preferences are not all
stored at the same scope; inspect the struct before deciding.

## Build, test, and package

### Required local dependencies

- Rust stable toolchain, Node.js, pnpm, and Tauri 2 platform prerequisites.
- `pnpm install` at repository root.
- libmpv for visual playback. Windows first launch downloads the pinned DLL
  from upstream, verifies its SHA256 hash, and installs it into
  `%LOCALAPPDATA%\Qlisa\runtime\`; macOS/Linux use platform libmpv.
- Windows Qlisa app builds always use `vendor/asiosdk/` via
  `src-tauri/.cargo/config.toml` and enable `asio-support`.
- Windows Qlisa downloads FFmpeg/ffprobe from the pinned BtbN FFmpeg 9.0 GPL
  static archive on first launch and installs them under
  `%LOCALAPPDATA%\Qlisa\runtime\`. See [runtime pins and source provenance](THIRD_PARTY_SOURCE_OFFER.md).
  NSIS installs Qlisa per-machine under `C:\Program Files\Qlisa`. The runtime
  manifest pins upstream URLs and SHA256 hashes. The media binaries are not
  inputs to the installer or updater package; the updater updates Qlisa only.
  NDI Runtime is installed separately by the user.

### Commands

From repository root:

```powershell
pnpm install
pnpm tauri:dev              # Windows app development build, with ASIO
pnpm tauri:check            # Windows debug/no-bundle build, with ASIO
pnpm test
pnpm build                 # TypeScript check + Vite build
pnpm exec tauri build --debug --no-bundle -- --features asio-support # Windows test build, no installer
pnpm release -- 1.5.5 --dry-run # validate versions and show the release plan
pnpm release -- 1.5.5          # Windows ASIO exe; sync version and commit before build
pnpm release -- 1.5.5 --bundle msi # explicitly create only an MSI installer
pnpm tauri:build                  # lower-level ASIO build; bundles installers
```

From `src-tauri/`:

```powershell
cargo test                 # unit + integration tests
cargo test --lib           # unit tests only
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The project Cargo config keeps incremental artifacts in `src-tauri/target/`,
uses 20 build jobs, and selects Rust's bundled `rust-lld` for
`x86_64-pc-windows-msvc`. This is the portable LLD linker supplied by rustup;
it avoids a machine-specific `lld-link.exe` path while retaining the MSVC ABI.
Use `cargo check` for the quickest Rust feedback, followed by `cargo test` when
behavior needs verification. Do not run `cargo clean` in the normal edit /
check / test loop: removing the cache makes the next build pay the full
dependency compile cost again. `CARGO_BUILD_JOBS` and `CARGO_INCREMENTAL` may
still be overridden for a one-off diagnostic, but are not normal settings.

The release profile keeps `opt-level = 3` and ASIO, but uses `lto = "thin"`
with `codegen-units = 16`. Compared with full LTO and one codegen unit, this
preserves cross-crate production optimization while parallelizing more of the
release compile. The expected trade-off is a much shorter release build for a
small possible change in binary size or peak optimization; verify playback on
the normal Windows ASIO path before distributing a release.

The Windows release pipeline starts with `scripts/publish.ps1 -Version X.Y.Z`.
It requires a clean `main` branch, synchronized version fields, a signing key,
release notes, and pinned media archive metadata. It does not require locally
staged media binaries. It runs frontend and Rust checks, creates the local
`release: vX.Y.Z` version commit, then builds the Windows ASIO NSIS installer
and Tauri updater artifact. Tauri updater artifacts are the NSIS executable and
its `.exe.sig`; the media runtime and NDI Runtime are not included. The script
prepares local assets and metadata only. It does not tag, push, or publish to
GitHub. See [Windows release preparation](RELEASING.md) for prerequisites and
publication steps.

The updater plugin is registered in the Rust app and configured with Qlisa's
public key and GitHub Releases `latest.json` endpoint. It checks for updates at
startup and through the Help/About UI. It downloads the signed update, then
blocks installation while cues run or the workspace has unsaved changes.
The 1.5.4 local updater E2E check passed. The 1.5.5 per-machine installation,
bootstrap, and updater checks remain pending. Runtime source and license
evidence has open items; downloading media assets from upstream does not
establish legal compliance. Source materials and notices remain separate from
the Qlisa binary release. Release artifacts are not checked into Git; do not
reuse Inkue updater metadata.

`scripts/release.mjs` performs the version update, local release commit, and
build used by the release pipeline. Its `--dry-run` checks version consistency
and the requested version, then prints the release plan without editing,
committing, or building. The full `publish.ps1 -DryRun` also checks release
prerequisites and pinned media metadata, but does not run tests or build
artifacts. Before the version
commit, failures restore only version files changed by that run. After the
commit, failures preserve it for inspection. The recorded commit and artifact
hashes support release traceability.

After each completed and verified logical task, the team lead must create a
separate Git commit before building or handing off that version. Create a
checkpoint commit before a risky experiment. Do not start the next task with
uncommitted changes unless they are explicitly marked WIP. Record the commit
hash used to produce each build artifact so it can be traced to its source.

### Test map

- Frontend tests are colocated as `*.test.ts` / `*.test.tsx`, run by Vitest.
- Rust unit tests cover cue models/serialization, timing and curves, media
  decode/metadata, engine scheduling, routing/compositing, and UI command
  contracts.
- `src-tauri/tests/` contains workspace persistence, cue behavior, transport GO,
  registry, media-decode, and camera-probe integration suites. Some hardware or
  runtime-dependent behavior cannot be established by pure unit tests.
- Before reporting a test/build result, run it against the current tree and
  record the exact command and outcome. Historical counts in old notes are not
  authoritative.

## Current limitations and operational cautions

- Platform target support does not establish identical hardware behavior.
  Validate native output, audio drivers, monitor enumeration, NDI runtime, and
  SRT transport on the target OS and devices.
- SRT program audio is currently Windows-only; non-Windows SRT is video-only.
- Output/network status is local health/submit status; it does not confirm a
  remote endpoint is decoding/displaying the result.
- If a Windows runtime asset is locked by a running process, the build script
  may retain the existing copied resource. Close the app and verify the output
  file before treating the rebuild as updated.
- A configured cue with missing/unreadable media cannot play. Use Preflight and
  Relink Media; inspect the log and health banner for decode/runtime errors.
- Use `docs/README.md` to find current Qlisa technical and release documentation.

### Runtime control status (post-fix)

The earlier eight-item audit of seek/completion, delayed Auto-Follow, nested
transport actions, disabled Group children, pause timing, and seek-independent
Auto-Continue has been implemented and passed focused Rust verification. The
reported looped Video #1 → separate Auto-Follow Audio #2 → Stop #3 sequence is
covered by deterministic regression cases, but still needs a manual check with
the target machine's real audio/output devices.

- Live audio/video voice ownership now prevents duration/seek position alone
  from resetting a still-playing cue. Stale completion status is ignored while
  the voice remains alive; a completed engine voice is not resurrected by seek.
- Pending Auto-Follow and Group post-completion advances are tied to source
  execution markers/generations and the expected list/playhead/Group driver.
  Stop/Reset invalidates the deadline. If a Group's current source is canceled,
  it parks at that boundary until a manual GO or new Playhead placement.
- Group action cues go through shared recursive transport dispatch; nested
  Start/Stop/Pause/Resume/Load/Reset/Goto/Fade/Devamp/Arm/Disarm are handled,
  cycle starts are guarded, and disabled children/targets are skipped.
- Pause-aware action clocks cover Audio, Video, MIDI File, Mic, Image, Text,
  Wait, Group, and Fade. Camera/Timecode pause remains a no-op. Engine-submitted
  fades may continue while the cue clock is paused.
- Auto-Continue uses an independent monotonic action clock for seekable media;
  seek/scrub changes progress but not the continuation deadline. Sequential
  Groups poll that same pause-aware clock instead of a wall-clock AC timer.

Focused results from **2026-09-18**: `runtime_control_regressions` 28/28,
`transport_go_tests` 27/27, `show::event_loop::tests` 9/9 (618 filtered), and
`cargo check --lib` passed in one diagnostic run with `CARGO_INCREMENTAL=0` and
`CARGO_BUILD_JOBS=2`. These were temporary diagnostic settings, not recommended
defaults. This was not a frontend build, app launch, package build, or
physical-device test. Full root-cause details, behavior, tests, and remaining
risks are in [runtime-control-audit.md](runtime-control-audit.md).

### Browser Cue MVP

Browser Cue displays one validated `http://` or `https://` URL in one shared
Tauri WebView. The page is an exclusive fullscreen visual source: it does not
enter the mpv compositor, crossfade with mpv layers, or produce NDI/SRT output.
Only physical display destinations are accepted; the selected destination's
monitor is used for WebView placement. The page owns its own refresh loop.

The surface is pre-created as a hidden window from `browser-surface.html` and
reused across cues. This keeps WebView2/browser-process construction out of the
GO path; GO only schedules navigation and native window updates. `reload_on_go`
controls navigation on GO; otherwise the loaded page stays alive while the
surface is hidden on soft Stop. Hard Stop hides the surface and navigates it to
`about:blank`, so page scripts stop; the next GO loads the page again while
retaining the pre-created WebView. A new Browser start recursively reconciles all
older Browser cues, including Group children, to Standby and emits Stopped
without hiding the new owner. This bounds the WebView/process lifecycle to one
surface, so many Browser cues do not multiply resident browser processes or
windows. Runtime memory still depends on the page and WebView implementation;
it is not measured by offline tests.

GO, STOP, and tick only reserve a generation and enqueue native WebView work
with Tauri's main-thread dispatcher. They never wait for WebView creation,
navigation, or window operations while holding workspace/output locks. A stale
callback checks its generation before revealing the surface, so a late request
cannot resurrect a stopped cue or hide a newer owner. A five-second watchdog
invalidates a request that never reaches the dispatcher. Navigation failures
publish `status: error` through `browser-surface-state` without blocking
transport.

The Browser Inspector also has a small inline preview. It is an ordinary
frontend iframe, not the shared fullscreen WebView and not a second native
window. The iframe keeps a 1280×720 page viewport and is scaled into a 16:9
preview canvas (Fit by default, with a 100% view for inspecting pixels). It
loads only validated HTTP(S) URLs with
`allow-scripts allow-forms allow-same-origin` inside sandbox. Popup and
top-navigation permissions remain disabled; the external origin still does
not receive Qlisa IPC or capabilities.
Reload changes the iframe identity without changing the page URL. Sites can
still refuse inline loading with X-Frame-Options or CSP; fullscreen Browser
output is independent and can continue to work.

The external page receives no Qlisa IPC or plugin capability. Query, fragment,
and user/password components are removed from runtime status diagnostics; do
not put secrets in persisted URLs unless the page requires them.

### Media conversion

The Media Converter reads media metadata with `ffprobe` and converts audio,
video, and image files with FFmpeg. It supports queued batch conversion, job
progress and cancellation, and applying an output to its cue with a restore
path. Compatibility results are heuristics for the playback engine; they do
not guarantee decode quality on every platform. Conversion and metadata probes
run outside the audio callback and transport path. Windows first launch
downloads `ffmpeg.exe` and `ffprobe.exe` from one pinned BtbN archive directly
from upstream. See [Windows network runtime packaging](windows-network-runtime.md).

### Diagnostics

The Diagnostics window reports audio stream health, per-cue streaming state
and underruns, audio scheduler and memory peaks, video decoder/output status,
and live NDI/SRT input/output counters. The view is a local runtime snapshot;
network counters show worker activity and do not confirm remote playback.
Statistics can be reset from the diagnostics UI. Diagnostics are observational
and do not guarantee that a device or stream works under every load.

### Concert Number timeline

`CueType::Number` is a persisted cue type implemented with the recursive Group
container. The cue list shows one Number row; its expanded children remain
editable in the shared Number timeline. The timeline renders nested Group
tracks and media waveforms, with trim and seek controls. Audio, Video, or Group
can be the master; a Group master must have a finite duration. Supported action
types are Audio, Video, Image, and Group. Each action uses its persisted offset
from the master timeline; child pre-waits are normalized to zero. Equal offsets
start together, and natural master completion stops temporary media actions.
The timeline cursor previews the active Video/Image action; Number audio can be
previewed on headphones without routing it to program output.

The Number inspector configures a fade-in/fade-out envelope, a stop-on-start
mode (none, all, audio, video, or selected cues), and cues to start when Number
finishes. A missing or unsupported master blocks GO. Legacy enabled
unsupported children such as Light or Browser can also block GO; Scene and
other unsupported cue types are not Number actions. GO, Pause/Resume, Stop/Hard
Stop, timeline seek, trim, and relink are supported. Device timing still needs
rehearsal on the target system.

## Active Cues sidebar

The main window shows Active Cues and Inspector in one mutually exclusive
right sidebar. They share a width and resize handle. The toolbar buttons sit
side by side, with Active Cues before Inspector. `inkue_ui_layout.rightPanel`
stores the selected view; the old `inspectorOpen` boolean is read as a
compatibility fallback when no valid `rightPanel` value exists.

Active Cues flattens running and paused cues from the recursive summaries in
`workspaceStore.cues`, including nested Group children. It reads per-cue time
from event-driven `timingStore`; it does not own playback state or run a timer.
The cue list retains its existing 30 Hz timing events. Finite-duration cues
show progress and remaining time; indefinite cues show neither. Pause and
Resume are enabled for Audio, Video, Image, Fade, Wait, Text, Group, Number,
Mic, and MIDI File cues. Other types show a disabled pause control. Pause,
Resume, and Stop address cues by ID, including nested Group children. Command
state events report the actual lifecycle state.

## Troubleshooting

| Symptom | Check |
|---|---|
| App opens, but video/image output is unavailable | Check the runtime status in Qlisa and confirm `%LOCALAPPDATA%\Qlisa\runtime\libmpv-2.dll` is present and loadable; open the app log. |
| Audio is silent or the selected device disappeared | Check Preferences → Audio, health alerts, and machine config. The device watchdog attempts fallback/recovery; verify Output Patches and channel mapping. |
| Media cue appears stuck/loading or has no duration | Check path, codec/decode alert, file permissions, and Preflight/Relink. Inspect logs; metadata and decodes run off the UI path. |
| NDI not available | Install the official NDI Runtime separately, then check Preferences → NDI/SRT Output status. A valid local runtime is required for NDI discovery/sender startup. Qlisa does not bundle the DLL. |
| SRT reports unavailable | Check `%LOCALAPPDATA%\Qlisa\runtime\ffmpeg.exe` and confirm the pinned Windows build described in [FFmpeg source and build provenance](THIRD_PARTY_SOURCE_OFFER.md) supports SRT. |
| Runtime preparation fails | Check network access to the pinned upstream URLs in `scripts/runtime-manifest.json`, then choose Retry on the preparation screen. |
| A new Tauri window fails to use filesystem/window APIs | Check the window's label and `src-tauri/capabilities/*.json`; permissions are scoped by window. |
| Build output does not match a fresh source edit | Close Qlisa/tauri dev if files are locked, rebuild, inspect the executable/resources timestamp, then launch the intended output path. |

Logs are available from the in-app Logs view and its Open Folder action. Avoid
including private media paths, OSC passwords, or SRT passphrases in bug reports.

## Where to change what

| Change | Primary files | Also check |
|---|---|---|
| Add/change a cue type | `src-tauri/src/cue/{types,traits,registry}.rs`, new `*_cue.rs`, `state/app_state.rs` | `src/lib/types.ts`, inspector tab, cue list icon/creation UI, `commands/cue_cmds.rs`, serde/registry tests, QLab mapping if applicable |
| GO, STOP, Pause, Auto-Continue/Follow | `show/transport.rs`, `show/event_loop.rs`, `commands/transport_cmds.rs` | cue trait/lifecycle, nested groups, emitted events, `transport_go_tests.rs` |
| Workspace fields and migrations | `show/workspace.rs`, cue serialization, `preferences.rs` | `src/lib/types.ts`, `workspaceStore.ts`, save/load commands, old JSON tests |
| Audio file playback or callback | `engine/audio_engine.rs`, `engine/voice.rs`, `cue/audio_cue.rs`, `cue/media_decode.rs` | RT-safety, ring command/status, device/patch routing, pause/seek/stop tests |
| Video/output/geometry/fade | `engine/output_engine/`, `cue/video_cue.rs`, `cue/image_cue.rs`, `cue/camera_cue.rs` | paired video audio voice, output destination routing, capability/window threading, compositor tests |
| File preview/metadata/trim UI | `commands/cue_cmds.rs`, `engine/media_metadata.rs`, `state/app_state.rs` | Inspector components, preview session generation rules, typed events and tests |
| NDI/SRT and network output | `engine/network_io.rs`, `engine/output_engine/mod.rs`, `commands/network_io_cmds.rs` | network worker lifecycle, audio taps/queues, destination config, `tauri.windows.conf.json`, runtime/license notes |
| Audio/display/preferences | `preferences.rs`, `machine_config.rs`, `commands/preferences_cmds.rs` | preference modal, shared TS types/defaults, migration tests, machine vs workspace scope |
| OSC/MIDI/timecode | matching `engine/*`, `cue/*`, `commands/*` modules | cue trigger routing, UI editor, machine config and integration tests |
| Lighting/DMX | `engine/dmx_*`, `engine/fixture.rs`, `cue/light_cue.rs`, `commands/light_cmds.rs` | fixture/patch UI, network interface selection, workspace persistence |
| UI-only cue table/inspector | `src/components/CueList/`, `src/components/Inspector/` | `src/lib/types.ts`, commands wrapper, selection/keyboard tests, translations |
| Tauri window, permission, installer | `src-tauri/tauri*.conf.json`, `src-tauri/capabilities/`, `build.rs` | window component, asset scopes, runtime manifest pins and provenance; media binaries download on first launch and are not vendor resources |

## First-pass workflow for a new coding session

1. Read this guide and the focused docs relevant to the change.
2. Inspect `git status --short` and current source before using historical notes.
3. Trace the feature in both directions: UI → `lib/commands.ts` → Rust command
   → show/cue/engine, and backend event → `useTauriEvents.ts` → store/UI.
4. Preserve cue UUID/runtime-state/RT/threading/persistence invariants above.
5. Make the smallest relevant change, run the matching tests plus the pertinent
   Rust/frontend checks, and state what was not exercised on real hardware.
