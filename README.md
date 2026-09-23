# Qlisa

Qlisa is a Windows-focused cue-based application for stage playback. Build a cue list, select the next cue with the Playhead, and start it with GO. Audio, video, images, groups, timing, and output routing share one show workspace.

Qlisa is a community-maintained fork of [Inkue by FonograF](https://github.com/FonograF/Inkue). It keeps the `.inkue` workspace format and some compatibility names while adding its own playback, output, and show workflows. Qlisa is licensed under GPL-3.0-or-later.

## Features

- Cue lists with GO, STOP, hard stop, pause/resume, cue selection, Auto-Continue, Auto-Follow, pre-waits, and post-waits.
- Audio and video playback, image and text cues, nested groups, fades, trims, seek, loops, and headphone preview.
- Multiple named display outputs, per-cue routing, a mixer, output monitor, and active-cue controls.
- NDI and SRT network input/output. NDI requires a separately installed NDI Runtime. SRT uses FFmpeg.
- OSC, MIDI, timecode, live audio input, sACN/Art-Net lighting, and QLab workspace import.
- Queued media conversion for audio, video, and images. Conversion and media probing require FFmpeg/ffprobe.
- English and Russian interface, diagnostics, crash recovery, and `.inkue` workspaces.

Hardware, driver, codec, and network compatibility depends on the Windows system and installed runtimes. See [output notes](docs/qlisa-multi-output.md) for platform limits.

## Screenshots

Screenshots are not included yet. The `docs/design/` directory contains Qlisa branding assets, not application screenshots.

## Download and installation

Check [GitHub Releases](https://github.com/Ruslan-mad/Qlisa/releases) for Windows installers when available. The source repository is public; a published source tree alone does not mean a binary release or signed in-app updater is ready. Current release and updater status must be confirmed from a verified release.

Windows 10 or 11 is the supported release target. The application uses WebView2; current supported Windows versions normally include the WebView2 Runtime. If it is missing, install Microsoft's [Evergreen WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/).

NDI is optional. Install the official [NDI Runtime](https://ndi.video/) separately to use NDI sources or destinations. Qlisa does not distribute or install it. See [Windows runtime notes](docs/windows-network-runtime.md) for FFmpeg/SRT and packaging details.

## Build from source

The Windows development and packaging path uses Windows 10/11, Rust stable, Node.js, pnpm, Tauri 2 prerequisites, Visual Studio C++ Build Tools, and the Windows SDK. Install frontend dependencies with `pnpm install`.

Windows Qlisa builds use the ASIO-enabled Cargo feature and the ASIO 2.3 interfaces in `vendor/asiosdk/`. The Windows visual output engine loads `src-tauri/vendor/mpv/libmpv-2.dll` at runtime; this DLL is a local prerequisite and is not in Git. Development that uses NDI needs the separately installed NDI Runtime. NDI DLL files are not build or installer inputs. FFmpeg/ffprobe are needed to exercise media conversion and SRT and are staged locally for Windows installer creation; they are not stored in Git.

Useful commands from the repository root:

```powershell
pnpm install
pnpm test
pnpm build
pnpm tauri:dev
pnpm tauri:check
```

The Windows app commands above enable ASIO through the project scripts. `pnpm tauri:build` creates a bundled installer and requires the local runtime files described above. See the [project guide](docs/PROJECT_GUIDE.md) for prerequisites and current build limitations.

## Origin and licensing

Qlisa is a derivative work of [Inkue](https://github.com/FonograF/Inkue) by FonograF. Thank you to FonograF for the original project. Qlisa preserves upstream attribution and is licensed under [GPL-3.0-or-later](LICENSE). Third-party components have separate attribution and terms; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Contributing

See the [project guide](docs/PROJECT_GUIDE.md) for architecture, compatibility rules, commands, and focused technical documentation. Report issues and compatibility findings through GitHub Issues.
