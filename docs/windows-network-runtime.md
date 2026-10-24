# Windows network runtime and packaging

Qlisa's Windows network code uses these runtimes:

| Runtime | How Qlisa uses it | Distribution plan |
| --- | --- | --- |
| NDI Runtime | Dynamically loaded for NDI discovery, receive, and send | Installed separately by the user. Qlisa source and installer must not contain NDI DLLs. |
| FFmpeg and ffprobe | SRT input/output, conversion, and media probing | On first launch Qlisa downloads the versioned Gyan 9.0.2 archive directly from upstream, verifies SHA-256, and installs the executables under `%LOCALAPPDATA%\Qlisa\runtime\`. They are not included in the Qlisa installer or updater package. |
| libmpv | Windows video playback | On first launch Qlisa downloads the pinned shinchiro development archive directly from upstream, verifies SHA-256, extracts `libmpv-2.dll` with the in-process LZMA-capable 7z decoder, and stores it under `%LOCALAPPDATA%\Qlisa\runtime\`. It is not included in the Qlisa installer or updater package. |

NDI is optional. Install the official [NDI Runtime](https://ndi.video/) to use NDI sources or destinations. Qlisa does not download, bundle, or install it. NDI® is a registered trademark of Vizrt NDI AB.

## Pinned FFmpeg build

The selected build is Gyan FFmpeg 9.0.2 full, GPLv3 static, for Windows x86_64:

- Release tag: [`9.0.2`](https://github.com/GyanD/codexffmpeg/releases/tag/9.0.2)
- Archive: [`ffmpeg-9.0.2-full_build.zip`](https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg-9.0.2-full_build.zip)
- Archive SHA-256: `759d0a9831c436a0eb331ad36f236c06cb04aaa0005da46571f0e9d3d9206f6b`
- FFmpeg version: `9.0.2-full_build-www.gyan.dev`, from FFmpeg commit [`946fcce07b6dcd0331c8cc609192aeff5e1924f8`](https://github.com/FFmpeg/FFmpeg/commit/946fcce07b6dcd0331c8cc609192aeff5e1924f8)
- Executable SHA-256: `ffmpeg.exe` `589e50b766d251afdf181dd664d40bd94407e200b019989fd7468c7d118a28d0`; `ffprobe.exe` `02264dfa4f56baaa31e443ece8aa16d397f6b008200e7fd184d22b9009e75fd3`.
- License: the archive README identifies GPL v3, the build enables `--enable-gpl --enable-version3`, and its `LICENSE` matches the tracked file.
- Verified features: SRT, x264, x265, AAC, and the scale, overlay, and format filters.

Use the versioned asset URL and checksum in `scripts/runtime-manifest.json`.
The application embeds this manifest and downloads the archive directly from
upstream. It verifies the archive and each executable before installing them.
The Qlisa installer and updater do not contain these binaries. See the
[build provenance notice](../src-tauri/vendor/ffmpeg/README-Gyan-build.txt),
[source-offer status](THIRD_PARTY_SOURCE_OFFER.md), and
[third-party notices](../THIRD_PARTY_NOTICES.md).

## Build behavior

The Windows Tauri bundle contains no FFmpeg, ffprobe, or libmpv binary. NSIS installs Qlisa per-machine under Program Files. On first launch Qlisa checks its runtime manifest, downloads only missing or invalid components, verifies archive and file hashes, then installs them to `%LOCALAPPDATA%\Qlisa\runtime\`. The updater updates Qlisa only; a changed manifest triggers runtime repair on the next launch. The NDI runtime is dynamically discovered from the user's installation and is not a build input. Development of NDI paths requires a separately installed NDI Runtime; compilation does not require an NDI DLL import library.

The selected libmpv version is `v0.41.0-1055-g6fd80b200`, from the generic x86_64 `mpv-dev` asset in [shinchiro/mpv-winbuild-cmake release 20260923](https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20260923). Its archive SHA-256 is `372f29c292d0c8b4ce916225739e5872e35b8e11f3f4590c285baed8ba551100`; the extracted DLL SHA-256 is `751131f81b5ce485d046ff08d1a44a93c5455be5135d53c0ab19c56780925ff0`. Isolated playback, seek, and two-context smoke checks passed. WASAPI was configured; physical audio output was not verified. Exact dependency sources, build inputs, and combined license obligations remain unverified. The Qlisa installer does not include this DLL; the app downloads the pinned archive directly from upstream.

## Operational limits

- Windows SRT output currently carries program audio and video. Current macOS/Linux SRT output is video-only.
- NDI carries program audio according to the platform path documented in the current network I/O implementation.
- Local worker status and frame counters do not prove that a remote receiver is displaying the stream.
- Never log SRT passphrases or raw secret URLs.
