# Third-party notices

This file identifies upstream projects and runtime components used by Qlisa. Versions below describe the checked-out project and its currently documented Windows packaging inputs. They must be reviewed when any runtime binary changes. This summary does not replace the license texts or notices distributed with each component.

## Qlisa and Inkue

Qlisa is licensed under GPL-3.0-or-later. The complete license is in [LICENSE](LICENSE). Qlisa is a modified fork/derivative of [Inkue by FonograF](https://github.com/FonograF/Inkue), which is also GPL-3.0-or-later. Upstream copyright and license notices remain with the inherited code. Qlisa changes are maintained by Qlisa contributors and are not attributed to FonograF.

QLab is a trademark of Figure 53, LLC. Qlisa is not affiliated with or endorsed by Figure 53, LLC.

## NDI

Qlisa contains NDI integration code that dynamically loads the NDI Runtime installed by the user. Qlisa source and installers do not include `Processing.NDI.Lib.x64.dll`, the NDI Runtime, or NDI SDK binaries. NDI use is subject to the current [NDI SDK terms](http://ndi.link/ndisdk_license); obtain the Runtime from [ndi.video](https://ndi.video/). NDI® is a registered trademark of Vizrt NDI AB.

Release checklist: confirm that no NDI Runtime DLL is present in the source export or installer.

## FFmpeg, ffprobe, and SRT

The Windows release packaging workflow stages a fixed Gyan FFmpeg essentials build outside Git, then includes `ffmpeg.exe`, `ffprobe.exe`, and the build's GPL license/notice in the installer. The documented build is Gyan FFmpeg **9.0.1 essentials**, archive `https://github.com/GyanD/codexffmpeg/releases/download/9.0.1/ffmpeg-9.0.1-essentials_build.zip`, SHA-256 `fec81ae03971d9dd4be3ebe02e263bd2ec1d789483f931bdba5f5715e65da2e9`. The Gyan build page is at [gyan.dev](https://www.gyan.dev/ffmpeg/builds/). This version is the source of the locally staged build notice and must be revalidated against the final staging workflow before a binary release.

This FFmpeg build is GPLv3. Its build notice reports upstream FFmpeg source commit `bf1b838f2a` and build configuration. The `ffprobe.exe` comes from the same build. When these executables are distributed, make available the corresponding source for the exact FFmpeg build, including applicable modifications and build information, under the GPL terms that apply to that build. The source must match the binaries actually distributed.

SRT is not linked directly by Qlisa. Qlisa invokes FFmpeg, which uses its `libsrt` protocol support. The Gyan 9.0.1 build notice identifies SRT as **1.5.6-2-gfcae571**, licensed under MPL-2.0. For a binary release, the corresponding source package must cover the exact FFmpeg build and all included libraries, including this libsrt revision, with required license texts, notices, modifications, and build configuration. The FFmpeg Git commit alone is not the complete corresponding source for this statically linked external-library build. The source/build inputs for the exact Gyan archive have not yet been assembled or independently verified; binary release remains blocked pending that work. This version statement applies only to the documented Gyan 9.0.1 build; update it if the release payload changes.

## mpv / libmpv

Windows visual playback loads `libmpv-2.dll` at runtime. The local DLL reports version `v0.41.0-458-g062f4bf04` and appears to be associated with the [shinchiro Windows build project](https://github.com/shinchiro/mpv-winbuild-cmake); its exact mapping to a shinchiro release asset and build has not been verified. The version string identifies mpv source commit [`062f4bf04`](https://github.com/mpv-player/mpv/commit/062f4bf04); the DLL's embedded feature list reports the `gpl` feature. mpv source defaults to GPL-2.0-or-later; the mpv project states that LGPL mode requires excluding GPL-only files, and linked libraries can also affect the resulting license. Treat this concrete DLL as GPL-2.0-or-later unless its matching build evidence establishes otherwise. The upstream mpv commit is identified, but the exact dependency revisions, patches, build configuration, and corresponding source archive for this DLL have not yet been verified. Before distributing it, collect that complete corresponding source and all dependency notices/licenses for the actual binary. Binary release remains blocked pending that work.

## Steinberg ASIO SDK

The Windows build uses interfaces from Steinberg ASIO SDK **2.3** through CPAL's ASIO support. The vendored SDK has been reduced to the CPAL-required ASIO host/common sources and headers, plus the upstream README and license files; the SDK demos, logo, and PDF material are not included. The SDK's `LICENSE.txt` offers either Steinberg's proprietary license or GPL version 3. The Qlisa source distribution selects the GPLv3 option for SDK files covered by that dual-license notice. Retain the applicable license files and file-specific notices. ASIO is a Steinberg trademark.

## DSEG timer font

Qlisa includes `DSEG7Classic-Regular.ttf`, identified in the source as DSEG7 Classic. The accompanying `vendor/fonts/DSEG-LICENSE.txt` gives copyright to keshikan (2017) and licenses the font under SIL Open Font License 1.1. Keep the license and Reserved Font Name terms with redistributions.

## Rust and frontend dependencies

The lockfiles are `src-tauri/Cargo.lock` and `pnpm-lock.yaml`. Qlisa does not patch third-party dependency source; its Rust files under `src-tauri/src/qlab_import/patches.rs` implement QLab data transformations and are not dependency patches.

The Windows Rust graph includes unmodified MPL-2.0 crates: Symphonia **0.5.5** and its locked codec/format crates; `cssparser` **0.29.6** and **0.36.0**, `cssparser-macros` **0.6.1**, `selectors` **0.24.0** and **0.36.1**, and `dtoa-short` **0.3.5** through Tauri/Wry's HTML/CSS stack; and `option-ext` **0.2.0**. Cargo records these versions in `src-tauri/Cargo.lock`, and the crate sources are publicly available from crates.io. The dependency sources are not locally modified. When distributing compiled binaries, retain the MPL-2.0 notices and make the exact covered source available, including any changes if that ever changes. The updater dependency `tauri-plugin-updater` **2.12.0** declares MIT OR Apache-2.0; its 32 additional locked packages declare MIT, Apache-2.0, ISC, BSD, Zlib, or CDLA-Permissive-2.0 (`webpki-root-certs`). These declarations come from package metadata and do not certify the complete installer payload. See [dependency license audit](docs/dependency-license-audit.md) for scope and limits.

The frontend dependency license declarations were reviewed against registry metadata. The scan found MIT, Apache-2.0, BSD-2-Clause/3-Clause, ISC, 0BSD, Unlicense, Zlib, Unicode-3.0, CC-BY-4.0, and LGPL-3.0-or-later declarations; no AGPL, SSPL, BUSL, proprietary, or other GPL package declaration was found in that audit. LGPL declarations belonged to platform-specific `@img/sharp-libvips-*` / `@img/sharp-*` packages pulled by Sharp, which was listed only under `devDependencies` at audit time. The checked `dist` frontend output contained no Sharp/libvips references. The final installer payload was not checked. Frontend licenses and scope are recorded in [dependency license audit](docs/dependency-license-audit.md). This metadata scan is a triage aid, not a substitute for the license texts shipped by package authors.

## Qlisa artwork and screenshots

The owner states that new Qlisa images/icons not inherited from Inkue were created for this project with OpenAI image generation. The current image paths under `docs/design`, `public`, and `src-tauri/icons` were checked against the available `upstream/master` tree; their blobs differ. No inherited or unknown image in those checked paths was identified. The generation provenance is owner-provided.

Current Qlisa screenshots have not yet been published.
