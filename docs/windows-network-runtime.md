# Windows network runtime and packaging

Qlisa's Windows network code uses two separately supplied runtimes:

| Runtime | How Qlisa uses it | Distribution plan |
| --- | --- | --- |
| NDI Runtime | Dynamically loaded for NDI discovery, receive, and send | Installed separately by the user. Qlisa source and installer must not contain NDI DLLs. |
| FFmpeg and ffprobe | SRT input/output, conversion, and media probing | Pinned binaries are staged locally outside Git and included with the Windows installer with their license and build notice. |

NDI is optional. Install the official [NDI Runtime](https://ndi.video/) to use NDI sources or destinations. Qlisa does not download, bundle, or install it. NDI® is a registered trademark of Vizrt NDI AB.

## Pinned FFmpeg build

The selected build is BtbN FFmpeg 9.0 GPL static for Windows x86_64:

- Release tag: [`autobuild-2026-09-23-14-55`](https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-09-23-14-55)
- Archive: [`ffmpeg-n9.0.2-3-ga5923073bf-win64-gpl-9.0.zip`](https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-09-23-14-55/ffmpeg-n9.0.2-3-ga5923073bf-win64-gpl-9.0.zip)
- Archive SHA-256: `fdea132b8059ba9dfd6c1ce05bd831a85e665705f03e50162ae2fe599223b5ae`
- FFmpeg version: `n9.0.2-3-ga5923073bf-20260923`, from FFmpeg commit [`a5923073bfd8f25b7300d93af3f8e690174ebd30`](https://github.com/FFmpeg/FFmpeg/commit/a5923073bfd8f25b7300d93af3f8e690174ebd30)
- Build scripts/configuration: BtbN/FFmpeg-Builds commit [`ccbffa4f85d0e8de5c135c69ebb10e4c14911fa9`](https://github.com/BtbN/FFmpeg-Builds/tree/ccbffa4f85d0e8de5c135c69ebb10e4c14911fa9)
- Executables: `ffmpeg.exe` and `ffprobe.exe` from this one archive; their hashes are listed in the [build provenance notice](../src-tauri/vendor/ffmpeg/README-BtbN-build.txt).
- License: GPLv3 (`--enable-gpl --enable-version3`); archive `LICENSE.txt` matches the staged `LICENSE` file.
- SRT: enabled in the build configuration.

Use this versioned asset URL and verify its checksum before staging. The
provider's floating `latest` URL is not a release pin. Runtime executables are
kept outside Git; a local release setup may stage them in
`src-tauri/vendor/ffmpeg/` for the Tauri bundle. Keep both executables from the
same archive, the matching `LICENSE`, and `README-BtbN-build.txt` together.

The BtbN build scripts and FFmpeg source are public, but the exact corresponding
source set for statically linked components and applicable notices still needs
review. Binary redistribution remains blocked until the exact sources and
notices are assembled. See [source-offer status](THIRD_PARTY_SOURCE_OFFER.md)
and [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md).

## Build behavior

The Windows bundle maps local FFmpeg files through `src-tauri/tauri.windows.conf.json`. Release packaging expects the local FFmpeg staging files and fails if they are missing. The NDI runtime is dynamically discovered from the user's installation and is not a build input. Development of NDI paths requires a separately installed NDI Runtime; compilation does not require an NDI DLL import library.

The local Windows `libmpv-2.dll` is a separate visual playback prerequisite. The staged DLL is `v0.41.0-1055-g6fd80b200`, from the generic x86_64 `mpv-dev` asset in [shinchiro/mpv-winbuild-cmake release 20260923](https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20260923). Its archive SHA-256 is `372f29c292d0c8b4ce916225739e5872e35b8e11f3f4590c285baed8ba551100`; the DLL SHA-256 is `751131f81b5ce485d046ff08d1a44a93c5455be5135d53c0ab19c56780925ff0`. Isolated playback, seek, and two-context smoke checks passed. WASAPI was configured; physical audio output was not verified. Exact dependency sources, build inputs, and combined license obligations remain unverified before binary distribution.

## Operational limits

- Windows SRT output currently carries program audio and video. Current macOS/Linux SRT output is video-only.
- NDI carries program audio according to the platform path documented in the current network I/O implementation.
- Local worker status and frame counters do not prove that a remote receiver is displaying the stream.
- Never log SRT passphrases or raw secret URLs.
