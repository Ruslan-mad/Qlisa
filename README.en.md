<p align="right"><a href="README.md">Русский</a> | <strong>English</strong></p>

<p align="center">
  <img src="docs/assets/readme/qlisa-header.svg" alt="Qlisa" width="820">
</p>

<p align="center">
  <a href="https://github.com/Ruslan-mad/Qlisa/releases/latest"><img alt="Windows 10 and 11" src="https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D4?logo=windows&logoColor=white"></a>
  <a href="https://github.com/Ruslan-mad/Qlisa/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/Ruslan-mad/Qlisa?label=latest%20release"></a>
  <a href="LICENSE"><img alt="GPL-3.0-or-later" src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue"></a>
</p>

<p align="center">
  <a href="https://github.com/Ruslan-mad/Qlisa/releases/latest"><strong>⬇ Download Qlisa for Windows</strong></a> ·
  <a href="#screenshots">Screenshots</a> ·
  <a href="docs/README.md">Documentation</a> ·
  <a href="https://github.com/Ruslan-mad/Qlisa/issues">Report an issue</a>
</p>

# Qlisa

**Qlisa helps run audio, video, images, and other actions during concerts, theatre, presentations, and other live events.** Prepare a sequence of actions in advance, then run it with **GO** during the show.

Instead of separate players and folders full of files with confusing names, the show lives in one project. For example, one Cue can start music, the next can show a title card, and another can begin automatically after a video ends.

During a show, Qlisa can help you:

- start a music track;
- play a video or display an image and text;
- fade audio or send it to a selected device;
- start the next Cue automatically;
- send a command to other equipment or software;
- show different content on multiple displays.

## How Cue List and GO work

A **Cue** is one show action, such as playing audio, displaying video or text, waiting, changing an audio level, or sending a command to an external system. A **Cue List** is an ordered list of these actions. Use it to prepare the show and configure its media, levels, outputs, and delays.

The **Playhead** points to the Cue that will start when you press **GO**. After it starts, the Playhead moves down the list. The **Active Cues** panel shows Cues that have started and their state. **STOP** stops selected playback using its normal behavior. Use Hard Stop for an immediate stop.

You can choose how a Cue advances the list. **Auto-Continue** starts the next Cue after a set delay measured from the current action's start, so Cues can overlap. **Auto-Follow** starts the next Cue after the current action finishes and its delay elapses.

## Screenshots

![Qlisa main window with its Cue List](docs/screenshots/main-window.png)

| Active Cues | Cue Inspector |
| --- | --- |
| ![Active Cues panel](docs/screenshots/active-cues.png) | ![Cue settings in the Inspector](docs/screenshots/inspector.png) |

## Features

### Audio, video, and images

- Play audio and video; display images and text.
- Set levels, fade in and out, trim the beginning and end, seek, and loop playback.
- Choose audio devices, mix audio, and preview through headphones.
- Use multiple display outputs and route visual Cues to specific outputs.
- Set separate image geometry for different outputs.

### Cue types and show control

Available Cue types include Audio, Video, Image, Text, Memo, Wait, Fade, Stop, Group, Number, and more. Qlisa supports pre- and post-action delays, loops, control of other Cues, and groups that run actions in sequence or in parallel.

Configurable automatic transitions complement manual control through the Playhead, GO, STOP, and Active Cues panel. Waveform and media preview caches are stored alongside the project.

### Connecting to other systems

Qlisa supports MIDI, OSC, Timecode, sACN, Art-Net, NDI, and SRT. A Cue can run media and send commands to compatible software or systems.

To use NDI, install the official [NDI Runtime](https://ndi.video/) separately. Qlisa does not include it in the installer.

### Conversion and import

Built-in tools convert audio, video, and images. FFmpeg and ffprobe handle media processing and inspection. Qlisa supports importing QLab projects to transfer prepared Cue Lists.

## Installation

Qlisa is for **Windows 10 and Windows 11**. The installer installs the app for all users under `C:\Program Files\Qlisa` and may ask for administrator rights.

1. Open the [latest release page](https://github.com/Ruslan-mad/Qlisa/releases/latest) and download the Windows installer.

2. Run the installer and complete setup.

3. Start Qlisa. The first launch requires an internet connection to download the Media Runtime components.

NDI Runtime is required only when using NDI. Install it separately from the [NDI website](https://ndi.video/).

## Projects

New projects use the `.qlisa` extension. Qlisa also opens `.inkue` projects; both extensions use the same workspace structure. Keep media files in accessible locations and check their paths when moving a project to another computer.

## Updates and Media Runtime

Qlisa checks for app updates through GitHub Releases. The user starts an update from the app. Qlisa delays installation while Cues are playing.

On first launch, Qlisa downloads FFmpeg, ffprobe, and libmpv directly from pinned upstream releases, verifies their SHA-256 checksums, and stores the components in `%LOCALAPPDATA%\Qlisa\runtime`. On later launches, the app checks the files and downloads them again if they are missing or damaged. If a download fails, Qlisa shows the reason and a Retry button. You do not need to choose FFmpeg or libmpv manually. An internet connection is required for the initial download and repairs. The Qlisa installer does not contain these components.

See [Windows Media Runtime](docs/windows-network-runtime.md) for details.

## Project status

Qlisa is actively developed. The interface is available in Russian and English. Windows 10 and Windows 11 are the primary release platforms. Official releases for macOS and Linux are not available. Report bugs and suggest improvements in [GitHub Issues](https://github.com/Ruslan-mad/Qlisa/issues).

## Development

To build Qlisa, you need Windows 10 or 11, Rust stable, Node.js, pnpm, Tauri 2 prerequisites, Visual Studio C++ Build Tools, and the Windows SDK. See the [developer guide](docs/PROJECT_GUIDE.md) for setup, build instructions, and project details.

## Inkue project

Qlisa is based on the open-source project [Inkue by FonograF](https://github.com/FonograF/Inkue). We thank the author and Inkue contributors for the original project.

## License

Qlisa is licensed under [GPL-3.0-or-later](LICENSE). Third-party components have separate licenses and notices: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Documentation

Technical guides in docs are currently in English; the 1.5.10 release notes are available in English and Russian.

- [Qlisa documentation](docs/README.md)
- [Developer guide](docs/PROJECT_GUIDE.md)
- [Release preparation](docs/RELEASING.md)
- [Version 1.5.10 release notes](docs/RELEASE_NOTES_1.5.10.md)
- [License](LICENSE) · [Third-party notices](THIRD_PARTY_NOTICES.md)
