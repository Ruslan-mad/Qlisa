# FFmpeg 9.0.1 Essentials source status

## Identified build

| Field | Value |
| --- | --- |
| Binary | Gyan FFmpeg 9.0.1 Essentials, Windows x64 |
| Exact archive | [`ffmpeg-9.0.1-essentials_build.zip`](https://github.com/GyanD/codexffmpeg/releases/download/9.0.1/ffmpeg-9.0.1-essentials_build.zip) |
| Archive SHA-256 | `fec81ae03971d9dd4be3ebe02e263bd2ec1d789483f931bdba5f5715e65da2e9` |
| FFmpeg source | [FFmpeg commit `bf1b838f2ab88b4f8fd83443325c782ea0e0f7fa`](https://github.com/FFmpeg/FFmpeg/commit/bf1b838f2ab88b4f8fd83443325c782ea0e0f7fa) |
| License / build configuration | The archive notice says GPL v3 and records `--enable-gpl --enable-version3 --enable-static`; it also records the `release-essentials` configuration and external-library version strings. |
| libsrt | Build notice reports `1.5.6-2-gfcae571`, resolving to [SRT commit `fcae57145c000a9e7b72aa777adb8f85c2463242`](https://github.com/Haivision/srt/commit/fcae57145c000a9e7b72aa777adb8f85c2463242); that revision's [`LICENSE`](https://github.com/Haivision/srt/blob/fcae57145c000a9e7b72aa777adb8f85c2463242/LICENSE) identifies MPL-2.0. |
| Local binary hashes | `ffmpeg.exe`: `72a489eccd008c2ec2c0a5856c5c75bc3d8bbfa90166c4566865c246445e6aa3`; `ffprobe.exe`: `19202b23c0043f15ad1b7bce2344f406fd52bd6efd8f995ce02e7392a1cec52f` |

The archive `LICENSE` and `README.txt` are staged as
`src-tauri/vendor/ffmpeg/LICENSE` and
`src-tauri/vendor/ffmpeg/README-Gyan-build.txt`. Their SHA-256 values and the
archive and executable hashes are pinned in
[`scripts/runtime-manifest.json`](../scripts/runtime-manifest.json). The
release entry in ignored `.release-compliance.json` must remain
`correspondingSource: false` until the gap below is closed.

## Status: blocked

The single remaining gap is that Gyan does not publish a complete corresponding
source package for this exact archive. The [official 9.0.1 build-support tree](https://api.github.com/repos/GyanD/codexffmpeg/git/trees/9.0.1?recursive=1)
contains only `.github/FUNDING.yml` and `README.md`; the [release record](https://github.com/GyanD/codexffmpeg/releases/tag/9.0.1)
points to FFmpeg and ships a build notice, but not Gyan's build scripts/patches
or the exact source trees and modifications for the statically linked external
libraries, including libsrt. The FFmpeg and SRT source links above identify
upstream revisions; they do not establish that those are the complete sources
used for these binaries.

The archive identified by SHA-256 above, its FFmpeg source, recorded build
configuration, Gyan `LICENSE` and build notice, and the exact SRT revision and
license are the minimum known set. This is not yet a complete corresponding
source offer for the statically linked build. Do not label it GPL/MPL compliant
or provide a source archive made only from FFmpeg and SRT trees. Ask Gyan for
the exact build workflow, patches, and source inputs for this archive; then
preserve those materials with the release.

## shinchiro 20260923 build candidate

The `shinchiro/mpv-winbuild-cmake` release tagged [`20260923`](https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20260923) is a candidate for replacing the Gyan and separately sourced libmpv binaries. Its build-system commit is [`05a60b3cfd04e3e3b89918f4a27f3dde2935dff2`](https://github.com/shinchiro/mpv-winbuild-cmake/tree/05a60b3cfd04e3e3b89918f4a27f3dde2935dff2). The release identifies FFmpeg commit [`7d14defcc1b56a53f35dc78072844daaf50d58ea`](https://github.com/FFmpeg/FFmpeg/commit/7d14defcc1b56a53f35dc78072844daaf50d58ea) and mpv commit [`6fd80b2003873ef2bed09e78374549687a143236`](https://github.com/mpv-player/mpv/commit/6fd80b2003873ef2bed09e78374549687a143236). The release assets include `ffmpeg.exe` and a libmpv development archive, but no `ffprobe.exe`.

At that build-system commit, `packages/ffmpeg.cmake` passes `--disable-ffprobe`; obtaining `ffprobe.exe` requires changing that build configuration and rebuilding FFmpeg. The FFmpeg, mpv, and libsrt ExternalProject definitions do not pin `GIT_TAG`, so the build-system commit alone does not fix their dependency revisions. This candidate is **not** a replacement for the staged runtime and does not close the corresponding-source or compliance gap. The exact build inputs, dependency commits, configuration, applicable notices, and a matching source archive still need verification before any runtime switch.
