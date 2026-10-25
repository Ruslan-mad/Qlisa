# Windows network runtime and packaging

Qlisa's Windows network code uses these runtimes:

| Runtime | How Qlisa uses it | Distribution plan |
| --- | --- | --- |
| NDI Runtime | Dynamically loaded for NDI discovery, receive, and send | The full installer includes the official NDI 6.3.2 redistributable and runs it in the elevated NSIS install. It skips an installed NDI 6 runtime that is version 6.3.2 or newer. It does not bundle the NDI DLL directly. The compact updater does not include or install NDI. |
| FFmpeg and ffprobe | SRT input/output, conversion, and media probing | The full installer seeds the pinned files into `%LOCALAPPDATA%\Qlisa\runtime\` after SHA-256 checks. The compact updater keeps the existing cache. If files are missing or damaged outside a full install, the app downloads the pinned Gyan 9.0.2 archive and verifies both archive and executable hashes. |
| libmpv | Windows video playback | The full installer seeds the pinned DLL into `%LOCALAPPDATA%\Qlisa\runtime\` after SHA-256 checks. The compact updater keeps the existing cache. If missing or damaged, the app downloads the pinned shinchiro archive, verifies SHA-256, and extracts `libmpv-2.dll` with the in-process LZMA-capable 7z decoder. |
| Vulkan loader | Resolves Vulkan dependencies of the pinned libmpv build | The full installer seeds the pinned `vulkan-1.dll` into the runtime cache. libmpv loads dependencies from its own directory and standard Windows locations. The compact updater keeps the cache. |
| WebView2 Runtime | Windows application UI | Full setup embeds the Tauri offline installer. Compact updates keep Tauri's existing small WebView2 bootstrapper. |

NDI features remain optional at runtime. Full setup installs the official [NDI Runtime](https://ndi.video/) when NDI 6.3.2 or newer is not already installed. It does not replace a newer NDI 6 installation. NDI® is a registered trademark of Vizrt NDI AB.

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
The full installer embeds these pinned binaries; the compact updater does not. See the
[build provenance notice](../src-tauri/vendor/ffmpeg/README-Gyan-build.txt),
[source-offer status](THIRD_PARTY_SOURCE_OFFER.md), and
[third-party notices](../THIRD_PARTY_NOTICES.md).

## Build behavior

Qlisa ships two Windows installers. The full per-machine installer embeds the media runtime seed, pinned Vulkan loader, NDI redistributable, and Tauri's offline WebView2 installer. On first launch it verifies every bundled media file before copying it atomically to `%LOCALAPPDATA%\Qlisa\runtime\`; it does not fall back to a network download when a full-package file is absent or fails its pinned hash. The compact per-machine updater contains no media runtime or NDI payload. It updates Qlisa and leaves the per-user runtime cache in place. If that compact installation has a missing or invalid media runtime file, Qlisa uses the existing pinned upstream downloader.

The NDI redistributable runs in the elevated full-installer process. The NSIS hook checks the canonical NDI 6 runtime DLL version first, skips version 6.3.2 or newer, prevents automatic restart, and fails setup on a non-zero installer exit code. Development of NDI paths requires a separately staged test runtime; compilation does not require an NDI DLL import library.

The selected libmpv version is `v0.41.0-1055-g6fd80b200`, from the generic x86_64 `mpv-dev` asset in [shinchiro/mpv-winbuild-cmake release 20260923](https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20260923). Its archive SHA-256 is `372f29c292d0c8b4ce916225739e5872e35b8e11f3f4590c285baed8ba551100`; the extracted DLL SHA-256 is `751131f81b5ce485d046ff08d1a44a93c5455be5135d53c0ab19c56780925ff0`. Isolated playback, seek, and two-context smoke checks passed. WASAPI was configured; physical audio output was not verified. Exact dependency sources, build inputs, and combined license obligations remain unverified. The full installer includes this pinned DLL and verifies it before seeding the runtime cache. The compact updater omits it; the app downloads the pinned archive when it needs repair.

## Operational limits

- Windows SRT output currently carries program audio and video. Current macOS/Linux SRT output is video-only.
- NDI carries program audio according to the platform path documented in the current network I/O implementation.
- Local worker status and frame counters do not prove that a remote receiver is displaying the stream.
- Never log SRT passphrases or raw secret URLs.
