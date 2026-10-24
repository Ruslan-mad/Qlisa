# Third-party runtime provenance

Qlisa's Windows installer and updater do not contain FFmpeg, ffprobe, or
libmpv binaries. The application downloads pinned archives directly from their
upstream releases and verifies SHA-256 values. This delivery method does not
remove any obligations that apply to the upstream binaries. The records below
identify the selected artifacts and known source gaps; they are not a
compliance attestation or a corresponding-source package.

## Selected FFmpeg build: Gyan FFmpeg 9.0.2 full

| Field | Value |
| --- | --- |
| Provider | [GyanD/codexffmpeg](https://github.com/GyanD/codexffmpeg) |
| Release tag | [`9.0.2`](https://github.com/GyanD/codexffmpeg/releases/tag/9.0.2) |
| Archive | [`ffmpeg-9.0.2-full_build.zip`](https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg-9.0.2-full_build.zip) |
| Archive SHA-256 | `759d0a9831c436a0eb331ad36f236c06cb04aaa0005da46571f0e9d3d9206f6b` |
| Platform / variant | Windows x86_64, GPLv3, static full build |
| FFmpeg version | `9.0.2-full_build-www.gyan.dev` |
| FFmpeg source | [Commit `946fcce07b6dcd0331c8cc609192aeff5e1924f8`](https://github.com/FFmpeg/FFmpeg/commit/946fcce07b6dcd0331c8cc609192aeff5e1924f8) |
| `ffmpeg.exe` SHA-256 | `589e50b766d251afdf181dd664d40bd94407e200b019989fd7468c7d118a28d0` |
| `ffprobe.exe` SHA-256 | `02264dfa4f56baaa31e443ece8aa16d397f6b008200e7fd184d22b9009e75fd3` |
| License evidence | Archive README identifies GPL v3; build configuration reports `--enable-gpl --enable-version3`; archive `LICENSE` matches the tracked license text. |
| Feature evidence | Verified executables report SRT, x264, x265, AAC, and the scale, overlay, and format filters. |

The archive README identifies the FFmpeg source revision and describes static
linking. The full source set for statically linked dependencies and their
patches has not been independently verified. Gyan maintains versioned release
assets; this pin does not depend on a daily autobuild retention window. Qlisa
downloads the archive directly from upstream and does not redistribute the
runtime binaries in its installer or updater. The tracked record is
[`README-Gyan-build.txt`](../src-tauri/vendor/ffmpeg/README-Gyan-build.txt).

## Selected shinchiro 20260923 libmpv asset

Qlisa uses only the libmpv development asset from
[shinchiro/mpv-winbuild-cmake release `20260923`](https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20260923):
`mpv-dev-x86_64-20260923-git-6fd80b2003.7z`. Its SHA-256 is
`372f29c292d0c8b4ce916225739e5872e35b8e11f3f4590c285baed8ba551100`. The
staged DLL reports `v0.41.0-1055-g6fd80b200` and has SHA-256
`751131f81b5ce485d046ff08d1a44a93c5455be5135d53c0ab19c56780925ff0`. The
release identifies mpv source commit
[`6fd80b2003873ef2bed09e78374549687a143236`](https://github.com/mpv-player/mpv/commit/6fd80b2003873ef2bed09e78374549687a143236)
and build-system commit
[`05a60b3cfd04e3e3b89918f4a27f3dde2935dff2`](https://github.com/shinchiro/mpv-winbuild-cmake/tree/05a60b3cfd04e3e3b89918f4a27f3dde2935dff2).
The release links [workflow run `35800030374`](https://github.com/shinchiro/mpv-winbuild-cmake/actions/runs/35800030374).
The workflow identifies clang and the generic x86_64 target; the container tag
and build caches are mutable.

The extracted DLL passed playback, seek, and two simultaneous libmpv context
checks. WASAPI was configured; physical audio output was not verified. Exact
dependency source revisions and patches, the fully resolved build
configuration, and a corresponding-source package remain unverified. Assess
applicable obligations against the exact release payload.
