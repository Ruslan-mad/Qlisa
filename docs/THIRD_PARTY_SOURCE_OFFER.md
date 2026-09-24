# Corresponding source status for Windows runtime

## Status

**Not ready for a binary release.** The available evidence identifies the exact
Gyan FFmpeg archive, its FFmpeg source commit, the build notice, and the SRT
revision reported by that notice. It does not identify and verify all source
inputs and modifications used to make the statically linked executables. No
archive in this repository is presented as the complete corresponding source.

The release process must keep the ignored local `.release-compliance.json` entry for
FFmpeg at `correspondingSource: false` until a maintainer has assembled and
reviewed a complete source offer for the exact binaries. Do not create a
`*-corresponding-source.zip` from only the FFmpeg and SRT source trees.

## Exact binary build identified

| Item | Evidence |
| --- | --- |
| Provider and release | [GyanD/codexffmpeg 9.0.1 release](https://github.com/GyanD/codexffmpeg/releases/tag/9.0.1) |
| Runtime archive | `ffmpeg-9.0.1-essentials_build.zip` |
| Archive URL | <https://github.com/GyanD/codexffmpeg/releases/download/9.0.1/ffmpeg-9.0.1-essentials_build.zip> |
| Archive SHA-256 | `fec81ae03971d9dd4be3ebe02e263bd2ec1d789483f931bdba5f5715e65da2e9` |
| Archive contents | `ffmpeg.exe`, `ffprobe.exe`, `LICENSE`, and `README.txt` from the same archive |
| FFmpeg source revision reported by Gyan | [FFmpeg commit `bf1b838f2ab88b4f8fd83443325c782ea0e0f7fa`](https://github.com/FFmpeg/FFmpeg/commit/bf1b838f2ab88b4f8fd83443325c782ea0e0f7fa) |
| Reported license/build mode | Gyan's `README.txt` says GPL v3; the executable reports `--enable-gpl --enable-version3 --enable-static` |
| SRT reported by Gyan's build notice | `1.5.6-2-gfcae571`; the short revision resolves to [SRT commit `fcae57145c000a9e7b72aa777adb8f85c2463242`](https://github.com/Haivision/srt/commit/fcae57145c000a9e7b72aa777adb8f85c2463242) |
| SRT license | MPL-2.0, verified in `LICENSE` at the exact SRT source commit linked above |

The pinned acquisition and binary checks are in
[`scripts/runtime-manifest.json`](../scripts/runtime-manifest.json). Run
[`scripts/prepare-runtime.ps1`](../scripts/prepare-runtime.ps1) to download the
archive, verify its SHA-256 and the notice files, check the SRT protocol, and
stage only the two executables in the ignored vendor directory.

## What is known about the build

The archive's `README.txt` is retained in
`src-tauri/vendor/ffmpeg/README-Gyan-build.txt`. It identifies FFmpeg 9.0.1,
the FFmpeg source commit, GPL v3, the configuration used for Gyan's
`release-essentials` build, and reported external-library version strings.
The archive's `LICENSE` is retained beside that notice. The version strings
below are exactly those printed in the archive notice. They help identify the
build inputs, but do not prove source archive completeness, patches, or exact
source checkout contents.

| External library | Version reported by Gyan |
| --- | --- |
| AMF | `v1.5.2-2-gc35f613` |
| aom | `v3.14.1-147-gec0dedc1a2` |
| AviSynthPlus | `v3.7.5-362-gf4628d0a` |
| cairo | `1.18.5` |
| ffnvcodec | `n13.1.15.0-1-geddcea9` |
| fontconfig | `2.18.3` |
| freetype | `VER-2-14-3` |
| fribidi | `v1.0.16-5-g069a7e3` |
| gmp | `6.3.0-2` |
| gnutls | `3.8.13-1` |
| gsm | `1.0.24` |
| harfbuzz | `14.3.0-10-g9f2f0317` |
| lame | `3.100` |
| libass | `0.17.5-3-g89cc0f4` |
| libgme | `0.6.6` |
| libiconv | `1.19-1` |
| libopencore-amrnb / libopencore-amrwb | `0.1.6` / `0.1.6` |
| libssh | `0.12.0` |
| libtheora | `v1.2.0` |
| libwebp | `v1.6.0-199-g94d3c4a` |
| libxml2 | `v2.15.0-122-gddcb79dc` |
| openAL | `1.25.2` |
| openjpeg | `2.5.4` |
| libopenmpt | `libopenmpt-0.6.28-40-gefc11a27` |
| opus | `v1.6.1-50-g3da9f7a6` |
| rubberband | `v4.0.0` |
| SDL | `release-2.32.0-228-ga2e7c76bd` |
| speex | `Speex-1.2.1-51-g0589522` |
| libsrt | `v1.5.6-2-gfcae571` |
| VAAPI | `2.25.0` |
| vidstab | `v1.1.2-105-gc7a720a` |
| vmaf | `v3.2.0-9-g4991d2b5` |
| vo-amrwbenc | `0.1.3` |
| vorbis | `v1.3.7-37-g1b75110b` |
| VPL | `2.17` |
| vpx | `v1.16.0-184-g0cfc6da39` |
| x264 | `v0.165.3223` |
| x265 | `4.3-6-g9ddc216` |
| xvid | `v1.3.7` |
| zeromq | `4.3.5` |
| zimg | `release-3.0.6-252-gf6cc75a` |

The extracted `ffmpeg.exe` also reports Gyan's version string, GCC version,
FFmpeg library versions, and the configure flags. The pinned script checks the
binary hashes against the hashes measured from the pinned archive and checks
that the binary advertises the SRT protocol. These checks establish which
archive is staged. They do not establish a complete source offer.

## Remaining evidence required

The source package is incomplete until the following points are resolved for
the exact Gyan archive:

- Obtain and preserve the Gyan build scripts and any patches or build-system
  changes used for this release. The release record and binary build notice do
  not provide a complete, reproducible build recipe or identify every patch.
- Recover and verify the exact source checkout and any local changes for each
  enabled external library recorded in the notice. The version labels above
  help identify the inputs, but they are not the source trees or patch set.
- Review the exact licenses for enabled libraries and determine which
  components carry source-offer obligations in this statically linked build.
  Current evidence is not enough to state a complete list of GPL/MPL-covered
  external components or their required source packages.
- Verify that the source and build materials correspond to both shipped
  executables and preserve the package, checksums, and written offer with the
  release assets.

The table records libraries and version labels explicitly present in the
archive notice. It is not a conclusion that every listed library has the same
license or the same source-distribution requirement.

## Release handling

Keep the binaries out of Git. Include the archive's `LICENSE` and Gyan build
notice with the staged runtime and installer. The ignored local
`.release-compliance.json` remains the release-specific record of the reviewed
source archive and hashes; it must not mark the source as corresponding until
the requirements above are closed. The runtime manifest records acquisition
facts and current gaps. Neither file can make an incomplete source package
complete by attestation alone.

NDI Runtime is installed by the user and must not be bundled. libmpv is a
separate unresolved runtime: its local DLL hash and reported mpv commit are
recorded in the runtime manifest, but its originating build, dependencies,
configuration, and corresponding source are not verified by this document.
