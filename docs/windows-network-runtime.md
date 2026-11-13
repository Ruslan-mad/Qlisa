# Windows network runtime and packaging

Qlisa's Windows network code uses these runtimes:

| Runtime | How Qlisa uses it | Distribution plan |
| --- | --- | --- |
| NDI Runtime | Dynamically loaded for NDI discovery, receive, and send | The full installer includes the official NDI 6.3.2 redistributable and runs it in the elevated NSIS install. It skips an installed NDI 6 runtime that is version 6.3.2 or newer. It does not bundle the NDI DLL directly. The compact updater does not include or install NDI. |
| FFmpeg and ffprobe | SRT input/output, conversion, and media probing | The full installer seeds the recommended kit into `%LOCALAPPDATA%\Qlisa\runtime\` after SHA-256 checks. The compact updater keeps the installed kit. An empty cache downloads the pinned Qlisa runtime asset on first launch. A compatible older kit stays active until an operator selects Update components. |
| libmpv | Windows video playback | The full installer seeds the recommended kit DLL into `%LOCALAPPDATA%\Qlisa\runtime\` after SHA-256 checks. The compact updater keeps the installed kit. First launch downloads the pinned Qlisa runtime asset only when no kit exists. Missing or damaged files require an explicit repair action. |
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

The approved kit in `scripts/runtime-manifest.json` pins versioned Qlisa release
assets and hashes. The application checks the package hash and each runtime file
before installation. The legacy upstream records in that manifest document
build provenance; they are not acquisition URLs. The full installer embeds the
recommended kit; the compact updater does not. See the
[build provenance notice](../src-tauri/vendor/ffmpeg/README-Gyan-build.txt),
[source-offer status](THIRD_PARTY_SOURCE_OFFER.md), and
[third-party notices](../THIRD_PARTY_NOTICES.md).

## Build behavior

Qlisa ships two Windows installers. The full per-machine installer embeds the recommended media runtime kit, pinned Vulkan loader, NDI redistributable, and Tauri's offline WebView2 installer. On first launch it verifies bundled media files before copying them atomically to `%LOCALAPPDATA%\Qlisa\runtime\`. If the runtime cache is empty and a bundled media file is absent or invalid, bootstrap can download the approved kit. An invalid cache requires explicit repair. The compact per-machine updater contains no media runtime or NDI payload. It updates Qlisa and leaves the per-user runtime cache in place. A compatible installed kit remains active across app releases. Empty caches are populated on first launch; damaged kits show an explicit repair action. New kits download only after an operator selects Update components in Preferences.

The NDI redistributable runs in the elevated full-installer process. The NSIS hook checks the canonical NDI 6 runtime DLL version first, skips version 6.3.2 or newer, prevents automatic restart, and fails setup on a non-zero installer exit code. Development of NDI paths requires a separately staged test runtime; compilation does not require an NDI DLL import library.

The selected libmpv version is `v0.41.0-1055-g6fd80b200`. Its upstream build evidence remains recorded in `scripts/runtime-manifest.json`; the current approved kit packages only `libmpv-2.dll` in a versioned Qlisa asset. The package and DLL hashes are pinned there. Isolated playback, seek, and two-context smoke checks passed. WASAPI was configured; physical audio output was not verified. Exact dependency sources, build inputs, and combined license obligations remain unverified. The full installer includes the pinned DLL and verifies it before seeding the runtime cache. The compact updater omits it.

## Operational limits

- Windows SRT output currently carries program audio and video. Current macOS/Linux SRT output is video-only.
- NDI carries program audio according to the platform path documented in the current network I/O implementation.
- Local worker status and frame counters do not prove that a remote receiver is displaying the stream.
- Never log SRT passphrases or raw secret URLs.
