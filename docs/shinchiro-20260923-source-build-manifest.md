# shinchiro 20260923 source/build manifest

Status: **provenance incomplete; this document is not a release-readiness or compliance attestation.** It records confirmed source refs and identifies missing dependency provenance. It does not claim that the release binaries can be reproduced from the available public data.

## Selected release artifacts

| Component | Selected artifact | Confirmed source commit | Evidence |
| --- | --- | --- | --- |
| Historical FFmpeg runtime (not selected) | `ffmpeg-x86_64-git-7d14defcc.7z` | `7d14defcc1b56a53f35dc78072844daaf50d58ea` | [20260923 release assets](https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20260923); [FFmpeg commit](https://github.com/FFmpeg/FFmpeg/commit/7d14defcc1b56a53f35dc78072844daaf50d58ea) |
| Selected libmpv development package | `mpv-dev-x86_64-20260923-git-6fd80b2003.7z`; SHA-256 `372f29c292d0c8b4ce916225739e5872e35b8e11f3f4590c285baed8ba551100` | `6fd80b2003873ef2bed09e78374549687a143236` | [20260923 release assets](https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20260923); [mpv commit](https://github.com/mpv-player/mpv/commit/6fd80b2003873ef2bed09e78374549687a143236) |
| Build system | `shinchiro/mpv-winbuild-cmake` at `05a60b3cfd04e3e3b89918f4a27f3dde2935dff2` | `05a60b3cfd04e3e3b89918f4a27f3dde2935dff2` | [release workflow run #1355](https://github.com/shinchiro/mpv-winbuild-cmake/actions/runs/35800030374); [build-system commit](https://github.com/shinchiro/mpv-winbuild-cmake/commit/05a60b3cfd04e3e3b89918f4a27f3dde2935dff2) |

The selected package is the generic `x86_64` variant. It is not the separate `x86_64-v3` artifact.

## Confirmed build recipe and configuration

The pinned build-system source provides these first-party recipes:

- [FFmpeg recipe](https://github.com/shinchiro/mpv-winbuild-cmake/blob/05a60b3cfd04e3e3b89918f4a27f3dde2935dff2/packages/ffmpeg.cmake)
- [libSRT recipe](https://github.com/shinchiro/mpv-winbuild-cmake/blob/05a60b3cfd04e3e3b89918f4a27f3dde2935dff2/packages/libsrt.cmake)
- [mpv/libmpv recipe](https://github.com/shinchiro/mpv-winbuild-cmake/blob/05a60b3cfd04e3e3b89918f4a27f3dde2935dff2/packages/mpv.cmake)
- [mpv packaging recipe](https://github.com/shinchiro/mpv-winbuild-cmake/blob/05a60b3cfd04e3e3b89918f4a27f3dde2935dff2/packages/mpv-packaging.cmake)
- [All package recipes and patch files at that commit](https://github.com/shinchiro/mpv-winbuild-cmake/tree/05a60b3cfd04e3e3b89918f4a27f3dde2935dff2/packages)
- [Clang workflow at that commit](https://github.com/shinchiro/mpv-winbuild-cmake/blob/05a60b3cfd04e3e3b89918f4a27f3dde2935dff2/.github/workflows/mpv_clang.yml)

The workflow's generic x86_64 matrix entry targets `x86_64-w64-mingw32`, uses the `clang` toolchain, and invokes CMake with `-DENABLE_CCACHE=ON`, `-DCLANG_PACKAGES_LTO=ON`, Ninja, and the `src_packages` shared source location. It runs `ninja update` before the build. The workflow uses `ghcr.io/shinchiro/archlinux:latest` and restores package/build caches; the image tag and caches are mutable, so these references do not fully pin the environment that produced the release.

The FFmpeg recipe at this build-system commit explicitly passes `--disable-ffprobe`. This explains why the published FFmpeg archive has no `ffprobe.exe`. A local FFmpeg pair build must minimally override that setting or its target list and must record the actual configure command and source hash used.

## Local FFmpeg pair source pins

The intended local FFmpeg/ffprobe pair uses FFmpeg commit `7d14defcc1b56a53f35dc78072844daaf50d58ea` and libSRT commit `ff8ab25c57aece5b7351defe36dacc94fc28527f` ([libSRT commit](https://github.com/Haivision/srt/commit/ff8ab25c57aece5b7351defe36dacc94fc28527f)). The latter is the supplied local build pin; it is not evidence of the libSRT revision linked into the published FFmpeg or libmpv binaries.

This manifest records the intended source pins only. It does not yet record local build output hashes, actual `ffmpeg.exe`/`ffprobe.exe` version output, the final configure command, or a completed Qlisa conversion test.

## Local Windows pair-build attempt (2026-09-24)

The first local attempt did **not** produce either executable. Per the local build report, it ran on Windows with portable MSYS2, target `x86_64-w64-mingw32`, GCC, Ninja, and `MAKEJOBS=8`. The recipe edits were: pin FFmpeg to the source commit above, remove `--disable-ffprobe` from the FFmpeg recipe, and pin the libSRT recipe to version 1.5.7 at commit `ff8ab25c57aece5b7351defe36dacc94fc28527f`. No codec, library, or other FFmpeg options were changed.

The build stopped at step 321/750 while fetching the `opus-dnn` model from [Xiph](https://media.xiph.org/opus/models/opus_data-8a07d57c4fce6fb30f23b3e0d264004e04f1d7b421f5392ef61543d021a439af.tar.gz). Independent HTTP retries received 0 bytes; CMake left a partial cache of about 15 KB. No `ffmpeg.exe` or `ffprobe.exe` was built. This failure is documented from the local builder report; it is not a public build artifact. Local evidence is in `C:\Users\Mad\AppData\Local\Temp\qlisa-ffmpeg-pair-build`, with the reported log at `C:\Users\Mad\AppData\Local\Temp\qlisa-ffmpeg-pair-build\ffmpeg-build.log`. These temporary files are outside this repository and are not included in the source bundle.

## Unresolved libmpv dependency provenance

The staged DLL is `v0.41.0-1055-g6fd80b200`, SHA-256 `751131f81b5ce485d046ff08d1a44a93c5455be5135d53c0ab19c56780925ff0`. Its archive has SHA-256 `372f29c292d0c8b4ce916225739e5872e35b8e11f3f4590c285baed8ba551100`. Isolated playback, seek, and two concurrent libmpv context checks passed; WASAPI was configured, but physical audio output was not verified. These binary and smoke records are also in `scripts/runtime-manifest.json`.

The public release establishes the mpv source commit and the build-system commit. It does **not** establish exact source commits for every statically linked libmpv dependency, the exact dependency patches applied to that build, or the full resolved build configuration.

The release's [workflow run #1355](https://github.com/shinchiro/mpv-winbuild-cmake/actions/runs/35800030374) identifies the build and source refs but its short-retention log artifacts are expired. The workflow source shows package source caches and `ninja update`; package recipes can select mutable upstream refs. A different public gist revision was verified to belong to the following day's run, so its dependency hashes are intentionally excluded here. In particular, the local libSRT pin above must not be presented as the historical libSRT revision in the published libmpv build.

To complete compliance provenance, preserve a source/build bundle for the exact DLL: immutable source commits for every linked dependency, the exact build-system commit and applied patches, configure/build commands, toolchain/container image digest, and output hash. The DLL hash and local smoke evidence are now recorded, but the corresponding source bundle is still missing; status remains **not ready**.
