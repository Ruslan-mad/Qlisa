<p align="right"><strong>English</strong> | <a href="README.ru.md">Русский</a></p>

<p align="center">
  <img src="docs/assets/readme/qlisa-header.svg" alt="Qlisa — cue-based playback for live shows on Windows" width="820">
</p>

<p align="center">
  <a href="https://github.com/Ruslan-mad/Qlisa/releases/latest"><img alt="Windows" src="https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D4?logo=windows&logoColor=white"></a>
  <a href="https://github.com/Ruslan-mad/Qlisa/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/Ruslan-mad/Qlisa?label=latest%20release"></a>
  <a href="LICENSE"><img alt="GPL-3.0-or-later" src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue"></a>
  <a href="https://www.rust-lang.org/"><img alt="Rust" src="https://img.shields.io/badge/Rust-stable-orange?logo=rust&logoColor=white"></a>
  <a href="https://v2.tauri.app/"><img alt="Tauri 2" src="https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white"></a>
</p>

<p align="center">
  <a href="https://github.com/Ruslan-mad/Qlisa/releases/latest"><strong>Download Qlisa for Windows</strong></a> ·
  <a href="https://github.com/Ruslan-mad/Qlisa/releases">All releases</a> ·
  <a href="docs/README.md">Documentation</a> ·
  <a href="#screenshots">Screenshots</a> ·
  <a href="https://github.com/Ruslan-mad/Qlisa/issues">Report a bug</a> ·
  <a href="https://github.com/Ruslan-mad/Qlisa">Source code</a>
</p>

Qlisa is cue-based playback software for live shows and stage production on Windows. Build a cue list, select the next cue with the Playhead, and start it with **GO**. Audio, video, images, groups, timing, and output routing share one show workspace.

Qlisa is a community-maintained derivative of [Inkue by FonograF](https://github.com/FonograF/Inkue). New projects use `.qlisa`; legacy `.inkue` projects remain supported, and both extensions use the same JSON workspace format. The `.qlisa` Windows file association is planned for Qlisa 1.5.8. The latest released version is 1.5.7.

## About

Use Qlisa to prepare and run a show from a cue list. Cues can play media, control other cues, wait for timing, or send messages to connected systems. The Playhead selects the next cue; **GO**, **STOP**, and the transport controls run the show.

## Screenshots

![Qlisa main window with a cue list](docs/screenshots/main-window.png)

| Active Cues | Inspector |
| --- | --- |
| ![Active Cues with playback progress and controls](docs/screenshots/active-cues.png) | ![Cue inspector](docs/screenshots/inspector.png) |

## Features

- Audio, Video, Image, Text, Memo, Wait, Fade, Stop, Group, and Number cues, with cue timing, fades, trims, seek, loops, Auto-Continue, and Auto-Follow.
- Active Cues and Inspector panels, named display outputs, per-cue routing, audio mixing, and headphone preview.
- MIDI, OSC, timecode, sACN/Art-Net lighting, SRT, and NDI input or output. NDI requires the separately installed NDI Runtime.
- Media conversion for audio, video, and images. Conversion and media probing use FFmpeg and ffprobe.
- QLab workspace import, English and Russian interface, diagnostics, and crash recovery.

## Installation

1. Open [the latest Qlisa release](https://github.com/Ruslan-mad/Qlisa/releases/latest).
2. Download the Windows installer and run it. Windows may ask for administrator approval to install Qlisa for all users.
3. Start Qlisa. On first launch, it downloads and verifies the pinned media runtime components.

NDI is optional. Install the official [NDI Runtime](https://ndi.video/) separately only if you use NDI sources or destinations. Qlisa does not distribute or install it.

## Media runtime and updates

On first launch, Qlisa downloads the pinned FFmpeg, ffprobe, and libmpv runtime components from their upstream release URLs. It verifies their SHA-256 hashes and stores them under `%LOCALAPPDATA%\Qlisa\runtime`. Later launches check the installed runtime and repair missing or damaged files when needed. See [Windows runtime notes](docs/windows-network-runtime.md) for technical details.

Qlisa checks for signed application updates through GitHub Releases. You start an update from the app. Qlisa does not install an update while cues are active.

## Build from source

The supported development and packaging target is Windows 10 or 11. You need Rust stable, Node.js, pnpm, Tauri 2 prerequisites, Visual Studio C++ Build Tools, and the Windows SDK. Install dependencies with `pnpm install`; see the [project guide](docs/PROJECT_GUIDE.md) for build commands and setup details.

## Project status

Qlisa is actively developed for Windows. macOS and Linux are not supported release targets.

## Origin and license

Qlisa is a derivative work of [Inkue](https://github.com/FonograF/Inkue) by FonograF. Thank you to the Inkue contributors for the project Qlisa grew from.

Qlisa is licensed under [GPL-3.0-or-later](LICENSE). Third-party software has separate notices and terms in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Documentation

- [Documentation index](docs/README.md)
- [Project guide](docs/PROJECT_GUIDE.md)
- [Release preparation](docs/RELEASING.md)
- [Release notes for 1.5.6](docs/RELEASE_NOTES_1.5.6.md)
- [License](LICENSE) · [Third-party notices](THIRD_PARTY_NOTICES.md)
