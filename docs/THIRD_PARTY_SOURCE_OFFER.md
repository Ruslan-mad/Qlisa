# FFmpeg source offer and build provenance

## Selected FFmpeg build: BtbN FFmpeg 9.0

| Field | Value |
| --- | --- |
| Provider | [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds) |
| Release tag | [`autobuild-2026-09-23-14-55`](https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-09-23-14-55) |
| Archive | [`ffmpeg-n9.0.2-3-ga5923073bf-win64-gpl-9.0.zip`](https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-09-23-14-55/ffmpeg-n9.0.2-3-ga5923073bf-win64-gpl-9.0.zip) |
| Archive SHA-256 | `fdea132b8059ba9dfd6c1ce05bd831a85e665705f03e50162ae2fe599223b5ae` |
| Platform / variant | Windows x86_64, GPL, static |
| FFmpeg version | `n9.0.2-3-ga5923073bf-20260923` |
| FFmpeg source | [Commit `a5923073bfd8f25b7300d93af3f8e690174ebd30`](https://github.com/FFmpeg/FFmpeg/commit/a5923073bfd8f25b7300d93af3f8e690174ebd30) |
| BtbN build-system source | [Commit `ccbffa4f85d0e8de5c135c69ebb10e4c14911fa9`](https://github.com/BtbN/FFmpeg-Builds/tree/ccbffa4f85d0e8de5c135c69ebb10e4c14911fa9) |
| Variant configuration | [`variants/win64-gpl.sh` at the pinned build commit](https://github.com/BtbN/FFmpeg-Builds/blob/ccbffa4f85d0e8de5c135c69ebb10e4c14911fa9/variants/win64-gpl.sh) |
| `ffmpeg.exe` SHA-256 | `288ef71027b17e4d83d5d95777f14495fc59cc53e7b4c35637d0d68269c6d151` |
| `ffprobe.exe` SHA-256 | `8603cd025c0fa317091ea5e3f6b3a4181bf21c71a2dc30ac049789459265db7a` |
| License evidence | Release is labeled GPL, static; build configuration reports `--enable-gpl --enable-version3`. The archive's `LICENSE.txt` is GPL version 3. |
| libsrt recipe pin | [Pinned BtbN `scripts.d/50-srt.sh`](https://github.com/BtbN/FFmpeg-Builds/blob/ccbffa4f85d0e8de5c135c69ebb10e4c14911fa9/scripts.d/50-srt.sh) sets [Haivision SRT commit `ff8ab25c57aece5b7351defe36dacc94fc28527f`](https://github.com/Haivision/srt/commit/ff8ab25c57aece5b7351defe36dacc94fc28527f). This is a recipe pin, not independent binary attestation. |

Both executables are in this one archive. Its build configuration also enables
libsrt. The archive has no separate README or build notice; the project
provenance record is [`README-BtbN-build.txt`](../src-tauri/vendor/ffmpeg/README-BtbN-build.txt).

### Corresponding-source status: blocked

The pinned BtbN source commit provides public build scripts and variant
configuration. Its libsrt recipe pins the source commit listed above. This is
evidence of the recipe input, not independent proof of the source used in the
binary. One confirmed source gap remains: `scripts.d/50-onevpl.sh` downloads
the libvpl PR 198 patch from the mutable URL
[`https://github.com/intel/libvpl/pull/198.patch`](https://github.com/intel/libvpl/pull/198.patch)
and applies it. The exact patch content used for this artifact build has not
been confirmed. Do not describe binary redistribution as ready until that
patch is identified and included with the corresponding source material. Keep
the release compliance field `correspondingSource: false` until then.

## Superseded Gyan FFmpeg 9.0.1 build

The previously documented Gyan `ffmpeg-9.0.1-essentials_build.zip` had SHA-256
`fec81ae03971d9dd4be3ebe02e263bd2ec1d789483f931bdba5f5715e65da2e9` and used
FFmpeg commit [`bf1b838f2ab88b4f8fd83443325c782ea0e0f7fa`](https://github.com/FFmpeg/FFmpeg/commit/bf1b838f2ab88b4f8fd83443325c782ea0e0f7fa).
Its staged build notice identified SRT as `1.5.6-2-gfcae571`, corresponding to
[SRT commit `fcae57145c000a9e7b72aa777adb8f85c2463242`](https://github.com/Haivision/srt/commit/fcae57145c000a9e7b72aa777adb8f85c2463242).
This Gyan build is no longer the selected FFmpeg artifact. Its earlier source
offer gap and full build notice remain in Git history; they do not describe the
selected runtime.

## Historical shinchiro 20260923 build candidate

The `shinchiro/mpv-winbuild-cmake` release tagged [`20260923`](https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20260923) is a candidate for replacing the Gyan and separately sourced libmpv binaries. Its build-system commit is [`05a60b3cfd04e3e3b89918f4a27f3dde2935dff2`](https://github.com/shinchiro/mpv-winbuild-cmake/tree/05a60b3cfd04e3e3b89918f4a27f3dde2935dff2). The release identifies FFmpeg commit [`7d14defcc1b56a53f35dc78072844daaf50d58ea`](https://github.com/FFmpeg/FFmpeg/commit/7d14defcc1b56a53f35dc78072844daaf50d58ea) and mpv commit [`6fd80b2003873ef2bed09e78374549687a143236`](https://github.com/mpv-player/mpv/commit/6fd80b2003873ef2bed09e78374549687a143236). The release assets include `ffmpeg.exe` and a libmpv development archive, but no `ffprobe.exe`.

At that build-system commit, `packages/ffmpeg.cmake` passes `--disable-ffprobe`; obtaining `ffprobe.exe` requires changing that build configuration and rebuilding FFmpeg. The FFmpeg, mpv, and libsrt ExternalProject definitions do not pin `GIT_TAG`, so the build-system commit alone does not fix their dependency revisions. This candidate is **not** the selected FFmpeg runtime. Its corresponding-source and compliance status is separate from the BtbN artifact above.

Isolated smoke testing of the 20260923 candidate passed video and audio-track playback (`ao=wasapi` configured; physical audio output unverified), seek, two concurrent libmpv contexts, an actual SRT localhost loopback, and FFmpeg MP4-to-WAV conversion. App-level conversion through `ffprobe.exe` remains untested because this release has no `ffprobe.exe`.
