# Third-party notices

This file identifies upstream projects and runtime components used by Qlisa. Versions below describe the checked-out project and its currently documented Windows packaging inputs. They must be reviewed when any runtime binary changes. This summary does not replace the license texts or notices distributed with each component.

## Qlisa and Inkue

Qlisa is licensed under GPL-3.0-or-later. The complete license is in [LICENSE](LICENSE). Qlisa is a modified fork/derivative of [Inkue by FonograF](https://github.com/FonograF/Inkue), which is also GPL-3.0-or-later. Upstream copyright and license notices remain with the inherited code. Qlisa changes are maintained by Qlisa contributors and are not attributed to FonograF.

QLab is a trademark of Figure 53, LLC. Qlisa is not affiliated with or endorsed by Figure 53, LLC.

## NDI

Qlisa contains NDI integration code that dynamically loads the NDI Runtime installed by the user. Qlisa source and installers do not include `Processing.NDI.Lib.x64.dll`, the NDI Runtime, or NDI SDK binaries. NDI use is subject to the current [NDI SDK terms](http://ndi.link/ndisdk_license); obtain the Runtime from [ndi.video](https://ndi.video/). NDI® is a registered trademark of Vizrt NDI AB.

Release checklist: confirm that no NDI Runtime DLL is present in the source export or installer.

## FFmpeg, ffprobe, and SRT

The selected Windows runtime is BtbN/FFmpeg-Builds release tag
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
enables `libsrt`. The archive's `LICENSE.txt` is GPLv3; the staged
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

Windows visual playback loads `libmpv-2.dll` at runtime. The locally staged Windows x64 DLL is from the generic (non-v3), non-LGPL `mpv-dev` asset in [zhongfly/mpv-winbuild release 2026-09-23-f9850ee727](https://github.com/zhongfly/mpv-winbuild/releases/tag/2026-09-23-f9850ee727). The release reports mpv source commit [`f9850ee727ed54a4feb042a5c0802768f141810b`](https://github.com/mpv-player/mpv/commit/f9850ee727ed54a4feb042a5c0802768f141810b). Its archive SHA-256 is `fc099b9700730266a10dc6544f0c3670cbbc9d7dc81bda7421b802f9eb45876a`; the extracted DLL SHA-256 is `861ac44349277bdb17f2a2d229111edaa4438e0a3eca88fb49b7b684c66828e5`. The successful [GitHub Actions build run](https://github.com/zhongfly/mpv-winbuild/actions/runs/35864255638) used clang. The workflow source at [commit 88bdc4db](https://github.com/zhongfly/mpv-winbuild/blob/88bdc4db67bb476a7606921eb2d68b273440d59b/.github/workflows/mpv.yml) identifies the generic x86_64 build variant.

The DLL passed Qlisa ABI checks and a local smoke test for WASAPI audio, GPU video, absolute seek, and two simultaneous video contexts. This is a GPL-enabled build. The exact combined license and notices for its statically linked components remain unverified because the build-log source summary requires GitHub authentication and the downloaded release asset does not contain corresponding dependency sources. Do not distribute this DLL until those source revisions and notices are available. See [`scripts/runtime-manifest.json`](scripts/runtime-manifest.json) for hashes and build evidence.

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
