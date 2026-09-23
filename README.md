# Qlisa

Qlisa is a Windows-focused cue-based application for stage playback. Build a cue list, select the next cue with the Playhead, and start it with GO. Audio, video, images, groups, timing, and output routing share one show workspace.

Qlisa is a community-maintained fork of [Inkue by FonograF](https://github.com/FonograF/Inkue). It keeps the `.inkue` workspace format and some compatibility names while adding its own playback, output, and show workflows. Qlisa is licensed under GPL-3.0-or-later.

Repository: [github.com/Ruslan-mad/Qlisa](https://github.com/Ruslan-mad/Qlisa).

## Features

- Cue lists with GO, STOP, hard stop, pause/resume, cue selection, Auto-Continue, Auto-Follow, pre-waits, and post-waits.
- Audio and video playback, image and text cues, nested groups, fades, trims, seek, loops, and headphone preview.
- Multiple named display outputs, per-cue routing, a mixer, output monitor, and active-cue controls.
- NDI and SRT network input/output. NDI requires a separately installed NDI Runtime. SRT uses FFmpeg.
- OSC, MIDI, timecode, live audio input, sACN/Art-Net lighting, and QLab workspace import.
- Queued media conversion for audio, video, and images. Conversion and media probing require FFmpeg/ffprobe.
- English and Russian interface, diagnostics, crash recovery, and `.inkue` workspaces.

Hardware, driver, codec, and network compatibility depends on the Windows system and the installed runtimes. See [output notes](docs/qlisa-multi-output.md) for known platform differences.

## Why Qlisa exists

Qlisa grew from Inkue into a Windows-oriented stage playback system for more involved live-show workflows. It is under active development; features and hardware paths continue to receive changes.

## Origin

Qlisa is a derivative work and fork of [Inkue](https://github.com/FonograF/Inkue) by FonograF. Thank you to FonograF for the original project. Qlisa preserves upstream attribution and remains GPL-3.0-or-later. See [LICENSE](LICENSE) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Screenshots

No current Qlisa screenshots are included in this source snapshot.

## Installation

Windows 10 or 11 is the supported release target. No binary release or installer has been published for version 1.5.2. Build from source using the instructions below. The application uses WebView2; current supported Windows versions normally include the WebView2 Runtime. If it is missing, install the Microsoft [Evergreen WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/).

NDI is optional. Install the official [NDI Runtime](https://ndi.video/) separately to use NDI sources or destinations. Qlisa does not distribute or install the NDI Runtime. Windows release packaging uses a pinned FFmpeg build for SRT and media conversion; its binaries are staged locally from outside this Git repository. See [Windows network runtime notes](docs/windows-network-runtime.md).

## Updating

There are no published Qlisa release binaries yet. The in-app updater is disabled: the app has no Qlisa signing key or update feed, and it does not contact Inkue's update service. Help → Check for updates and About → Check for updates report that updates are not configured. Until a signed Qlisa release feed is set up, install updates manually from a future Qlisa release page or build the desired source revision.

See the [updater signing plan](docs/release-signing-plan.md) for the proposed key and release setup. It is not an active update pipeline.

## Build from source

The maintained local development and Windows packaging path uses Windows 10/11, Rust stable, Node.js, pnpm, Tauri 2 prerequisites, Visual Studio C++ Build Tools, and the Windows SDK. Install frontend dependencies with `pnpm install`.

Windows Qlisa builds use the ASIO-enabled Cargo feature and the ASIO 2.3 interfaces in `vendor/asiosdk/`. The Windows visual output engine loads `src-tauri/vendor/mpv/libmpv-2.dll` at runtime; this DLL is a local prerequisite and is not in Git. Development that uses NDI needs the separately installed NDI Runtime. NDI DLL files are not build or installer inputs. FFmpeg/ffprobe are needed to exercise media conversion and SRT and are staged locally for Windows installer creation; they are not stored in Git.

Useful commands from the repository root:

```powershell
pnpm install
pnpm test
pnpm build
pnpm tauri:dev
pnpm tauri:check
```

The Windows app commands above enable ASIO through the project scripts. `pnpm tauri:build` creates a bundled installer. For release/version workflows, read [CLAUDE.md](CLAUDE.md) and [the project guide](docs/PROJECT_GUIDE.md) before running commands: the release command changes version files and creates a commit.

## Release preflight

[`scripts/publish.ps1`](scripts/publish.ps1) performs a local preflight by default. Pass `-RunChecks` to run frontend and Rust checks. Its `-PrepareRelease` mode is reserved for future work and currently fails closed on missing Qlisa updater configuration and signing credentials. It does not build an installer or release metadata and does not publish to GitHub. A published source repository would not mean binary releases or signed in-app updates are available.

Run Rust checks from `src-tauri`:

```powershell
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Repository structure

- `src/` — React and TypeScript interface.
- `src-tauri/src/` — Rust cue, show, playback, output, and device code.
- `src-tauri/vendor/` — local runtime prerequisites; proprietary or distributable binaries may be excluded from Git.
- `docs/` — architecture, packaging, and operational notes.
- `vendor/asiosdk/` — ASIO SDK interfaces used by Windows builds.

## Licensing

Qlisa is licensed under [GPL-3.0-or-later](LICENSE). Its Inkue origin and third-party components have separate attribution and terms. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for the evidence and open license questions for bundled or runtime components.

## Project status

Qlisa is actively developed. Expect bugs and changes as playback, output, and packaging workflows evolve. Contributions and issue reports are welcome.
