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

The documented payload is Gyan FFmpeg **9.0.1 essentials**, archive [`ffmpeg-9.0.1-essentials_build.zip`](https://github.com/GyanD/codexffmpeg/releases/download/9.0.1/ffmpeg-9.0.1-essentials_build.zip), SHA-256 `fec81ae03971d9dd4be3ebe02e263bd2ec1d789483f931bdba5f5715e65da2e9`. The locally inspected executable hashes were `ffmpeg.exe` `72a489eccd008c2ec2c0a5856c5c75bc3d8bbfa90166c4566865c246445e6aa3` and `ffprobe.exe` `19202b23c0043f15ad1b7bce2344f406fd52bd6efd8f995ce02e7392a1cec52f`; match these against the runtime files recorded in a release compliance manifest. The included Gyan build notice says GPL v3 and identifies upstream FFmpeg commit [`bf1b838f2a`](https://github.com/FFmpeg/FFmpeg/commit/bf1b838f2a). The same notice lists external libraries and exact-looking revision strings, including libsrt **1.5.6-2-gfcae571**, under MPL-2.0.

This is a static FFmpeg build with many external libraries. The FFmpeg source commit by itself is not the corresponding source for the complete binary. Before redistributing `ffmpeg.exe` or `ffprobe.exe`, collect and retain the precise Gyan source/build inputs for this archive, all covered sources and modifications for bundled libraries (including the identified libsrt revision), configuration/build information, and all applicable GPL/MPL/license notices. The exact corresponding source set has not yet been verified or assembled. **Binary release blocker.**

### libmpv

The local `libmpv-2.dll` reports `v0.41.0-458-g062f4bf04` and appears associated with the shinchiro Windows build project, but its exact mapping to a shinchiro release asset/build is unverified. The version string points to upstream mpv commit [`062f4bf04`](https://github.com/mpv-player/mpv/commit/062f4bf04); the embedded feature list reports `gpl`. mpv identifies its default license as GPL-2.0-or-later and documents a distinct LGPL mode that excludes GPL-only files. The upstream mpv source is available at [mpv-player/mpv](https://github.com/mpv-player/mpv); the Windows build project and its build scripts are [shinchiro/mpv-winbuild-cmake](https://github.com/shinchiro/mpv-winbuild-cmake).

The exact dependency revisions, local patches, full build configuration, and corresponding source package for this DLL have not yet been verified. The mpv commit alone is not sufficient to identify source for all statically linked dependencies. Collect that source and all applicable notices/licenses for the actual DLL before shipping it. **Binary release blocker.**

## Decision

- **Source repository:** no dependency-license blocker found. Locked MPL crates and development tooling have public source; the repository does not include compiled third-party binaries as part of this license audit.
- **Binary release:** not ready until corresponding source and notices for the exact FFmpeg/libsrt and libmpv payloads are gathered and matched to the final binaries.
- **Updater:** this audit does not determine updater signing-key readiness.
