Qlisa FFmpeg runtime provenance

Provider: GyanD/codexffmpeg (public GitHub repository)
Release tag: 9.0.2
Release: https://github.com/GyanD/codexffmpeg/releases/tag/9.0.2
Archive: ffmpeg-9.0.2-full_build.zip
Archive URL: https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg-9.0.2-full_build.zip
Archive SHA-256: 759d0a9831c436a0eb331ad36f236c06cb04aaa0005da46571f0e9d3d9206f6b

Target: Windows x86_64
Variant: full build, GPLv3, static
FFmpeg version string: 9.0.2-full_build-www.gyan.dev
FFmpeg source commit: 946fcce07b6dcd0331c8cc609192aeff5e1924f8
FFmpeg source: https://github.com/FFmpeg/FFmpeg/commit/946fcce07b6dcd0331c8cc609192aeff5e1924f8
Upstream archive README: identifies GPL v3, static linking, and the FFmpeg source commit.

ffmpeg.exe SHA-256: 589e50b766d251afdf181dd664d40bd94407e200b019989fd7468c7d118a28d0
ffprobe.exe SHA-256: 02264dfa4f56baaa31e443ece8aa16d397f6b008200e7fd184d22b9009e75fd3
The executables report GPLv3 and SRT support. They also expose libx264, libx265,
AAC, and the scale, overlay, and format filters used by Qlisa.

The archive includes a GPL license file that matches the tracked LICENSE file.
Qlisa downloads this versioned archive directly from upstream and checks the
archive and extracted executable hashes. Qlisa does not redistribute the
runtime binaries in its installer or updater.

This record identifies the selected artifact and evidence checked locally. It
does not establish every source revision, patch, or complete build input for
all statically linked dependencies. The corresponding-source status is not
verified.
