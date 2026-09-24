# Dependency license audit

Audit date: 2026-09-24. This is a repository and release-preparation record, not legal advice. Package `license` fields and upstream notices were used for triage; release artifacts must be checked against the exact payload and upstream license texts.

## Source repository

The original locked Rust graph was inspected with `cargo metadata --locked` and `cargo tree --locked --target x86_64-pc-windows-msvc --features asio-support -e all`. `cargo metadata` supplied license fields from package manifests/registry metadata; `Cargo.lock` itself records versions and checksums, not license metadata. That audit predates the Tauri updater dependency and must not be read as a complete audit of the current graph. A follow-up check of the updater lockfile additions found 32 package entries; their declared licenses are permissive MIT, Apache-2.0, ISC, BSD, and Zlib, except `webpki-root-certs`, which declares CDLA-Permissive-2.0. The updater plugin 2.12.0 declares MIT OR Apache-2.0. This metadata check does not inspect the final installer payload or replace review of shipped license texts. The app itself is GPL-3.0-or-later.

### MPL-2.0 in the Windows graph

These dependencies are unmodified upstream registry crates. There are no Cargo `[patch]` entries, vendored dependency modifications, or local source patches. Qlisa's `src-tauri/src/qlab_import/patches.rs` is application code for QLab workspace data and is not a dependency patch.

| Component | Locked version(s) | Use in Windows build | Source |
| --- | --- | --- | --- |
| Symphonia and codec/format crates | 0.5.5 (`symphonia`, `symphonia-bundle-flac`, `symphonia-bundle-mp3`, `symphonia-codec-aac`, `symphonia-codec-adpcm`, `symphonia-codec-pcm`, `symphonia-codec-vorbis`, `symphonia-core`, `symphonia-format-isomp4`, `symphonia-format-mkv`, `symphonia-format-ogg`, `symphonia-format-riff`, `symphonia-metadata`, `symphonia-utils-xiph`) | Audio decoding; linked into Qlisa | [crates.io source](https://crates.io/crates/symphonia/0.5.5) and each crate/version in [Cargo.lock](../src-tauri/Cargo.lock) |
| cssparser, cssparser-macros, selectors | cssparser 0.29.6 and 0.36.0; cssparser-macros 0.6.1; selectors 0.24.0 and 0.36.1 | Tauri/Wry web view support dependency graph | [cssparser 0.36.0](https://crates.io/crates/cssparser/0.36.0), [selectors 0.36.1](https://crates.io/crates/selectors/0.36.1), and versions in [Cargo.lock](../src-tauri/Cargo.lock) |
| dtoa-short | 0.3.5 | CSS parser dependency | [crates.io source](https://crates.io/crates/dtoa-short/0.3.5) |
| option-ext | 0.2.0 | Tauri/dirs dependency | [crates.io source](https://crates.io/crates/option-ext/0.2.0) |

The lockfile and crate registry identify exact source versions, and the corresponding crate source archives are publicly downloadable. The source repository publishes lockfiles rather than copies of these dependency source trees, so the dependencies themselves are not redistributed by the source repository. This is not a source-publication blocker. For a binary release, keep the MPL-2.0 licenses/notices from each exact crate and make the corresponding covered crate source available for the versions compiled into the binary. Recheck if any dependency is patched or replaced.

### Frontend graph

The `pnpm-lock.yaml` registry package/version graph was checked against npm registry metadata. The metadata declared MIT, Apache-2.0, BSD-2-Clause/3-Clause, ISC, 0BSD, Unlicense, Zlib, Unicode-3.0, CC-BY-4.0, and LGPL-3.0-or-later or combinations of those licenses. This review includes `@tauri-apps/plugin-updater` 2.12.0 (MIT OR Apache-2.0). No AGPL, SSPL, BUSL, proprietary, or other GPL declaration was found in that metadata review.

LGPL declarations are on platform-specific Sharp 0.35.2 packages (`@img/sharp-*` and `@img/sharp-libvips-*`); [package.json](../package.json) lists Sharp only under `devDependencies`. The CC-BY-4.0 declaration is `caniuse-lite`, a build-time browser-compatibility database. I checked the existing `dist` frontend output: it contains no `sharp`, `libvips`, or `caniuse-lite` references. This supports that these packages are build tools rather than frontend runtime code. The final Tauri installer payload was not inspected in this subtask, so this does not certify every installer file. Preserve their package notices if distributing build tooling or a source bundle that includes installed frontend dependencies. They do not create a source-repository blocker.

The review is based on lockfile versions and registry metadata, not the package tarballs' complete license texts. No incompatible or distribution-prohibiting frontend license was identified. A release SBOM/license report should still be generated from the exact build environment and shipped files.

## Binary release dependencies

### FFmpeg and libsrt

The selected payload is BtbN FFmpeg **9.0 GPL static**, archive [`ffmpeg-n9.0.2-3-ga5923073bf-win64-gpl-9.0.zip`](https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-09-23-14-55/ffmpeg-n9.0.2-3-ga5923073bf-win64-gpl-9.0.zip), SHA-256 `fdea132b8059ba9dfd6c1ce05bd831a85e665705f03e50162ae2fe599223b5ae`. The archive contains both executables; their hashes are recorded in [`README-BtbN-build.txt`](../src-tauri/vendor/ffmpeg/README-BtbN-build.txt). The archive's `LICENSE.txt` is GPL version 3 and matches the staged `src-tauri/vendor/ffmpeg/LICENSE`. Its build configuration enables `libsrt`. The pinned BtbN recipe identifies libsrt source commit [`ff8ab25c57aece5b7351defe36dacc94fc28527f`](https://github.com/Haivision/srt/commit/ff8ab25c57aece5b7351defe36dacc94fc28527f); this is a recipe pin, not independent binary attestation.

The confirmed remaining FFmpeg source gap is in BtbN `scripts.d/50-onevpl.sh`: it downloads and applies libvpl PR 198 from a mutable `pull/198.patch` URL, and the exact patch content used for this artifact build is unconfirmed. **Binary release blocker.** See [FFmpeg source offer](THIRD_PARTY_SOURCE_OFFER.md).

### libmpv

The staged `libmpv-2.dll` is version `v0.41.0-1055-g6fd80b200` from the generic x86_64 asset in [shinchiro/mpv-winbuild-cmake release 20260923](https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20260923). The archive `mpv-dev-x86_64-20260923-git-6fd80b2003.7z` has SHA-256 `372f29c292d0c8b4ce916225739e5872e35b8e11f3f4590c285baed8ba551100`; the DLL hash is `751131f81b5ce485d046ff08d1a44a93c5455be5135d53c0ab19c56780925ff0`. The release identifies mpv source commit [`6fd80b2003873ef2bed09e78374549687a143236`](https://github.com/mpv-player/mpv/commit/6fd80b2003873ef2bed09e78374549687a143236) and build-system commit [`05a60b3cfd04e3e3b89918f4a27f3dde2935dff2`](https://github.com/shinchiro/mpv-winbuild-cmake/tree/05a60b3cfd04e3e3b89918f4a27f3dde2935dff2). Isolated smoke checks passed playback, seek, and two simultaneous libmpv contexts; WASAPI was configured, but physical audio output was not verified. Exact dependency source revisions, patches, complete resolved configuration, and combined license obligations remain unverified. **Binary release blocker until corresponding source and notices are available.**

## Decision

- **Source repository:** no dependency-license blocker found. Locked MPL crates and development tooling have public source; the repository does not include compiled third-party binaries as part of this license audit.
- **Binary release:** not ready until corresponding source and notices for the exact BtbN FFmpeg build and libmpv payloads are gathered and matched to the final binaries.
- **Updater:** this audit does not determine updater signing-key readiness.
