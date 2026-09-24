# Third-party notices

This file identifies upstream projects and runtime components used by Qlisa. Windows installer and updater packages do not contain FFmpeg, ffprobe, or libmpv binaries. On first launch, Qlisa downloads pinned upstream assets directly and stores required runtime files under `%LOCALAPPDATA%\Qlisa\runtime\`. This download flow does not determine or remove legal obligations for those binaries. Versions below describe the pinned upstream assets and must be reviewed when any runtime pin changes. This summary does not replace applicable license texts or notices.

## Qlisa and Inkue

Qlisa is licensed under GPL-3.0-or-later. The complete license is in [LICENSE](LICENSE). Qlisa is a modified fork/derivative of [Inkue by FonograF](https://github.com/FonograF/Inkue), which is also GPL-3.0-or-later. Upstream copyright and license notices remain with the inherited code. Qlisa changes are maintained by Qlisa contributors and are not attributed to FonograF.

QLab is a trademark of Figure 53, LLC. Qlisa is not affiliated with or endorsed by Figure 53, LLC.

## NDI

Qlisa contains NDI integration code that dynamically loads the NDI Runtime installed by the user. Qlisa source and installers do not include `Processing.NDI.Lib.x64.dll`, the NDI Runtime, or NDI SDK binaries. NDI use is subject to the current [NDI SDK terms](http://ndi.link/ndisdk_license); obtain the Runtime from [ndi.video](https://ndi.video/). NDI® is a registered trademark of Vizrt NDI AB.

Release checklist: confirm that no NDI Runtime DLL is present in the source export or installer.

## FFmpeg, ffprobe, and SRT

The selected Windows runtime is downloaded directly from BtbN/FFmpeg-Builds release tag
[`autobuild-2026-09-23-14-55`](https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-09-23-14-55),
asset `ffmpeg-n9.0.2-3-ga5923073bf-win64-gpl-9.0.zip`. It is a Windows x86_64,
GPL static build. The archive SHA-256 is
`fdea132b8059ba9dfd6c1ce05bd831a85e665705f03e50162ae2fe599223b5ae`; both
`ffmpeg.exe` and `ffprobe.exe` come from that archive. Their SHA-256 values,
FFmpeg source commit, and pinned BtbN build-system commit are recorded in the
[BtbN build notice](src-tauri/vendor/ffmpeg/README-BtbN-build.txt).

The FFmpeg version string is `n9.0.2-3-ga5923073bf-20260923`. Its source is
[FFmpeg commit `a5923073bfd8f25b7300d93af3f8e690174ebd30`](https://github.com/FFmpeg/FFmpeg/commit/a5923073bfd8f25b7300d93af3f8e690174ebd30).
The build identifies GPL version 3 (`--enable-gpl --enable-version3`) and
enables `libsrt`. The archive's `LICENSE.txt` is GPLv3; the tracked
`src-tauri/vendor/ffmpeg/LICENSE` has the same SHA-256 and contents. Qlisa
invokes FFmpeg for SRT; Qlisa does not link SRT directly.

The BtbN build scripts and FFmpeg source are public, but the exact corresponding
source set for all statically linked components, patches, and applicable
third-party notices has not yet been assembled and reviewed. This is the
remaining FFmpeg source-compliance blocker. Do not describe binary
redistribution as ready until that review is complete. See the
[source-offer status](docs/THIRD_PARTY_SOURCE_OFFER.md).

Gyan FFmpeg 9.0.1 is superseded. Its prior provenance and unresolved source
offer remain in Git history; they do not describe the selected runtime.

## mpv / libmpv

Windows visual playback loads `libmpv-2.dll` at runtime. Qlisa pins Windows x64 DLL version `v0.41.0-1055-g6fd80b200` from the generic x86_64 `mpv-dev` asset in [shinchiro/mpv-winbuild-cmake release 20260923](https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20260923). The asset is `mpv-dev-x86_64-20260923-git-6fd80b2003.7z`, SHA-256 `372f29c292d0c8b4ce916225739e5872e35b8e11f3f4590c285baed8ba551100`; the extracted DLL SHA-256 is `751131f81b5ce485d046ff08d1a44a93c5455be5135d53c0ab19c56780925ff0`. It reports mpv source commit [`6fd80b2003873ef2bed09e78374549687a143236`](https://github.com/mpv-player/mpv/commit/6fd80b2003873ef2bed09e78374549687a143236). The release's [build run](https://github.com/shinchiro/mpv-winbuild-cmake/actions/runs/35800030374) and build-system commit [`05a60b3cfd04e3e3b89918f4a27f3dde2935dff2`](https://github.com/shinchiro/mpv-winbuild-cmake/tree/05a60b3cfd04e3e3b89918f4a27f3dde2935dff2) identify the clang generic x86_64 build.

The DLL passed isolated playback, seek, and two simultaneous libmpv context checks. WASAPI audio was configured; physical audio output was not verified. The exact combined license, dependency revisions, patches, and corresponding source for its statically linked components remain unverified. The Qlisa package does not contain this DLL and obtains it directly from upstream; that delivery flow does not establish or remove legal obligations. See [`scripts/runtime-manifest.json`](scripts/runtime-manifest.json) and [the 20260923 source/build record](docs/shinchiro-20260923-source-build-manifest.md) for hashes, build evidence, and the remaining source gap.

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
