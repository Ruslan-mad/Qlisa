# Qlisa project state

Last documentation review: **2026-09-24**. The current release is **1.5.2**.
The release version files are synchronized by `release.mjs` when the release is
built.

This file is a short orientation, not a claim that a release build or every
hardware path has just been verified. Read [CLAUDE.md](CLAUDE.md) and the
[project guide](docs/PROJECT_GUIDE.md) before implementing changes. Use live
source and a fresh command run for current test/build results; historical counts
and unreconciled release notes have been removed from this snapshot.

## Implemented areas represented in the tree

### Concert Number editor and runtime

- Number has an editable shared timeline with nested Group lanes, group/media
  waveforms, seek/trim, and quick actions. Audio, Video, and Group can be the
  master; supported action children are Audio, Video, Image, and Group.
- The timeline cursor drives visual preview of active Video/Image actions and
  supports a separate headphone preview for Number audio, outside program out.
- Persisted offsets schedule actions against the trimmed master timeline.
  Number-level fades and configurable stop-on-start / start-after-finish
  actions are available. Child pre-waits are normalized to zero.
- Natural master completion stops temporary media actions. Light, Browser,
  Scene, and other unsupported action types are not supported by Number; legacy
  enabled unsupported children can block GO. Real device timing still needs
  hardware rehearsal.

### Other implemented areas through 1.5.2

- Media conversion supports Audio, Video, and Image sources, queued batch jobs,
  progress/cancel, and applying or restoring converted media. It depends on a
  usable FFmpeg/ffprobe runtime; codec compatibility warnings remain advisory.
- Audio file playback can stream through bounded PCM buffers. Audio, video,
  and network diagnostics are available in a separate diagnostics window.
- Camera cues can control live input audio level and mute. Output preferences
  include global physical fullscreen controls; cue audio uses exclusive Main
  or enabled Aux buses, while Preview/Headphones stays separate.
- A second Qlisa process redirects to the running instance.
- Active Cues moved to the main window's shared right sidebar (`7a5aafe`). It shows running
  and paused cues, including nested Group children, with elapsed time, finite
  duration progress, remaining time, and per-cue controls where supported.
- Windows runtime preparation uses a pinned external FFmpeg archive; FFmpeg
  and ffprobe are staged locally for installer builds and are not source-repo
  assets. NDI Runtime is a separate user installation and is never bundled.

- **Cue/show control:** cue lists and Playhead, GO/STOP/hard stop/pause/resume,
  Auto-Continue/Auto-Follow, pre/post-waits, nested groups, control cues,
  timecode/MIDI triggers, undo/redo, QLab workspace import.
- **Media:** audio, video, image, camera, text, and MIDI file cues; trim/seek,
  fades, loops/slices/Devamp, per-cue output/level and visual-layer controls;
  isolated headphone preview and asynchronous cached media metadata.
- **Outputs:** named display destinations, native libmpv/OpenGL compositor,
  layer geometry/blending/fades, output timer, output monitor, floating timer,
  Mixer window; NDI/SRT input and output workers. The Inspector can select Main
  together with other output destinations without showing a duplicate Main.
- **External control / show I/O:** OSC send/receive and feedback, MIDI send and
  MIDI-file playback, MTC/LTC timecode, Mic/live input, DMX sACN/Art-Net,
  fixtures and fixture groups.
- **Operator and support UI:** cue inspector/editors, cart/show views,
  preferences, preflight/relink, health banner, logs, crash recovery, English
  and Russian UI dictionaries.
- **Branding:** Qlisa Icon Pack assets for EXE/taskbar, titlebar/About, and
  favicon/PWA.

This inventory comes from current source/configuration. It does not claim every
driver, hardware device, codec, NDI receiver, or OS combination has been tested.

## Current architecture references

| Area | Source of truth |
|---|---|
| Coding and compatibility invariants | `CLAUDE.md` |
| Onboarding, architecture, lifecycle, data flow, and file map | `docs/PROJECT_GUIDE.md` |
| Rust cue behavior | `src-tauri/src/cue/` |
| Cue-list and GO lifecycle | `src-tauri/src/show/` |
| Audio/video/network/device engines | `src-tauri/src/engine/` |
| IPC command implementation | `src-tauri/src/commands/` |
| Frontend command/DTO/event contracts | `src/lib/commands.ts`, `src/lib/types.ts`, `src/hooks/useTauriEvents.ts` |
| Workspace and machine settings | `src-tauri/src/show/workspace.rs`, `preferences.rs`, `machine_config.rs` |
| Output routing and network runtime | `docs/qlisa-multi-output.md`, `docs/windows-network-runtime.md` |

## Build and test status

Current checks for this documentation snapshot:

- `pnpm test`: **389 passed**.
- `pnpm exec tsc --noEmit`: **passed**.
- `pnpm build`: **passed**.
- `cargo check --tests`: **passed**.
- `pnpm tauri:check`: **passed**.
- The Rust test executable exits with **0xc0000139 before running tests**; Rust
  tests have not passed and remain unverified.

These checks do not prove a complete packaged application or physical audio,
video, NDI/SRT, or other device path. Installer and hardware paths still need
separate verification. Use these commands for a fresh check:

```powershell
# repository root
pnpm test
pnpm build
pnpm tauri:dev           # Windows app, with ASIO
pnpm exec tauri build --debug --no-bundle -- --features asio-support # Windows test build, no installer

# src-tauri
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Windows Qlisa app builds always use ASIO. Create MSI/NSIS installers only when
explicitly requested. Installer builds require locally staged FFmpeg/ffprobe
files under `src-tauri/vendor/ffmpeg/` and a local Windows libmpv DLL. NDI
Runtime is installed separately by the user. See the
[build/package guide](docs/PROJECT_GUIDE.md#build-test-and-package).

## Known operational limitations

### Preferences scope

`AppPreferences` is authoritative in the machine-global
`%APPDATA%/Inkue/preferences.json` file. Physical audio/display/output
configuration stays outside project files; `Workspace.preferences` remains a
serialized compatibility/runtime mirror. If the global file is absent, the
first genuinely loaded legacy workspace seeds it (a New project may seed only
when it is the first operation). Once present, global values win and unknown
legacy output destination IDs are appended once so cue routing is preserved;
the global default output is never replaced by a later project. Project cues,
patches, and other show data remain in `.inkue`.

- The repository targets Windows, macOS, and Linux, but native drivers and
  output/runtime dependencies require platform-specific verification.
- SRT output carries program audio on Windows; current macOS/Linux SRT output
  is video-only. NDI carries program audio per runtime code.
- Network output status/counters describe local worker submission; they do not
  prove a remote receiver is displaying the stream.
- Recent fixes include SRT FFmpeg process-lifecycle containment (`78eda06`),
  SRT audio-queue recovery under overflow (`39b30cd`), moving output applies
  off the UI thread (`3371015`), serializing output-destination applies
  (`5e5f094`), and keeping SRT video pacing on cadence (`f723691`).
- A possible startup race during repeated SRT-port reconfiguration and
  pre-connect backlog behavior remain deferred by user decision; they were not
  changed in these fixes.
- FFmpeg/ffprobe and notices are prepared from a pinned external archive and
  staged locally for Windows installer builds; they are not stored in Git.
  NDI Runtime is installed by the user and is never bundled. The Windows
  libmpv DLL remains a local prerequisite. The Qlisa updater stays disabled
  until its own signing key and update endpoint are ready.
- The planned public source URL is `https://github.com/Ruslan-mad/Qlisa`; until
  publication, the source is only in the local checkout. No binary release is
  available. `scripts/publish.ps1` performs a local preflight; its installer
  and release-metadata preparation pipeline is not implemented.
- A running Windows process may lock libmpv or another resource. A build can
  preserve an existing copied file in that case, so close the app and verify
  the actual bundle before distributing it.

Historical StageCUE documents in `docs/stagecue-*.md` and `docs/source-register.md`
are external product research; they are not a roadmap or Qlisa feature contract.
