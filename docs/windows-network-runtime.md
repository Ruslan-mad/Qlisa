# Windows network runtime and packaging

Qlisa's Windows network code uses two separately supplied runtimes:

| Runtime | How Qlisa uses it | Distribution plan |
| --- | --- | --- |
| NDI Runtime | Dynamically loaded for NDI discovery, receive, and send | Installed separately by the user. Qlisa source and installer must not contain NDI DLLs. |
| FFmpeg and ffprobe | SRT input/output, conversion, and media probing | Pinned binaries are staged locally outside Git and included with the Windows installer with their license and build notice. |

NDI is optional. Install the official [NDI Runtime](https://ndi.video/) to use NDI sources or destinations. Qlisa does not download, bundle, or install it. NDI® is a registered trademark of Vizrt NDI AB.

## FFmpeg build currently documented

The known build is Gyan FFmpeg 9.0.1 essentials for Windows x64:

- Archive: `https://github.com/GyanD/codexffmpeg/releases/download/9.0.1/ffmpeg-9.0.1-essentials_build.zip` (GyanD release archive; build provider: [Gyan](https://www.gyan.dev/ffmpeg/builds/))
- SHA-256: `fec81ae03971d9dd4be3ebe02e263bd2ec1d789483f931bdba5f5715e65da2e9`
- License: GPLv3
- libsrt: `1.5.6-2-gfcae571`, as reported by the build notice
- Upstream FFmpeg commit and build configuration: see the Gyan build notice shipped with that archive.

Use the versioned archive URL and verify its checksum before staging. The provider's floating `latest` URL is not a release pin. The binaries and notices are kept outside Git; a local release setup may stage them in `src-tauri/vendor/ffmpeg/` for the existing Tauri bundle mapping. Keep `ffmpeg.exe`, `ffprobe.exe`, `LICENSE`, and `README-Gyan-build.txt` from the same archive together. Do not replace one file independently.

The FFmpeg and libsrt source obligations apply when redistributing the installer. Include the exact corresponding source/build configuration or a compliant written offer with each binary release. Update [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md) when the staged version changes.

## Build behavior

The Windows bundle maps local FFmpeg files through `src-tauri/tauri.windows.conf.json`. Release packaging expects the local FFmpeg staging files and fails if they are missing. The NDI runtime is dynamically discovered from the user's installation and is not a build input. Development of NDI paths requires a separately installed NDI Runtime; compilation does not require an NDI DLL import library.

The local Windows `libmpv-2.dll` is a separate visual playback prerequisite. It reports the enabled `gpl` feature and file version `v0.41.0-458-g062f4bf04` (source commit `062f4bf04`). Treat this binary as GPL; the full build configuration and dependency sources still require verification. See the third-party notices before making a binary release.

## Operational limits

- Windows SRT output currently carries program audio and video. Current macOS/Linux SRT output is video-only.
- NDI carries program audio according to the platform path documented in the current network I/O implementation.
- Local worker status and frame counters do not prove that a remote receiver is displaying the stream.
- Never log SRT passphrases or raw secret URLs.
