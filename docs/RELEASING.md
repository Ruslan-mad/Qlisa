# Windows local build and release

`scripts/publish.ps1` prepares a **local-only signed Windows build**. It does
not create a tag, push, or publish a GitHub Release. Its installer, `.sig`, and
`latest.json` are private local test artifacts. Do not publish them before the
corresponding source and required notices for FFmpeg and libmpv are assembled
and verified.

## Local build prerequisites

- Windows, Node.js LTS, pnpm, Rust stable with the MSVC target, and Git for
  Windows. Install the project-local Tauri CLI with `pnpm install`.
- A clean `main` branch, synchronized project versions, and release notes at
  `docs/RELEASE_NOTES_X.Y.Z.md`.
- Tauri updater artifacts enabled, the Qlisa updater public key, and the HTTPS
  GitHub Releases endpoint in `src-tauri/tauri.conf.json`.
- The private updater key at `%USERPROFILE%\.tauri\qlisa.key`, or its external
  path passed with `-SigningKeyPath`. The script prompts for the password with
  a masked PowerShell prompt. It never prints or creates the private key.
- FFmpeg, ffprobe, and libmpv runtime files matching the SHA-256 pins in
  `scripts/runtime-manifest.json`. FFmpeg must also include its pinned license
  and `README-BtbN-build.txt` notice. If FFmpeg is absent, the script stages
  the pinned archive through `scripts/prepare-runtime.ps1`.

The script checks that these runtime files and notices are configured in
`src-tauri/tauri.windows.conf.json`. It scans the source tree for NDI Runtime
DLLs. It does not require 7-Zip, Minisign, source archives, or a local
`.release-compliance.json` for a local build. Tauri CLI 2 creates the updater
signature during the build when `bundle.createUpdaterArtifacts` is `true`.

Keep an encrypted backup of the private signing key outside the repository.
Losing this key prevents updates for users who trust the configured public
key.

## Prepare a local build

Create and review the release notes, then run from the repository root:

```powershell
.\scripts\publish.ps1 -Version 1.5.3
```

The script checks the repository, runtime hashes, and signing setup. It runs
frontend tests/build and Rust metadata/check/test/clippy checks. Then
`scripts/release.mjs` updates and commits the local version files and builds
the NSIS installer and Tauri updater artifact. The script creates the
installer, its `.sig`, and `latest.json` under
`src-tauri/target/release/prepared/vX.Y.Z/`. It prints the artifact paths and
SHA-256 values. It does not tag, push, or publish anything.

`-DryRun` checks prerequisites and any staged runtime files without changing
version files or building artifacts:

```powershell
.\scripts\publish.ps1 -Version 1.5.3 -DryRun
```

Clippy warnings do not fail this check; a non-zero Clippy exit does. The
pipeline does not run repository-wide `cargo fmt --check` because the current
baseline has unformatted files outside this release change.

With Tauri 2 `createUpdaterArtifacts: true`, the Windows NSIS installer is the
updater artifact and has a matching `.exe.sig`. Qlisa's `latest.json` stores
the exact signature text and installer URL. `v1Compatible` mode uses a
different `.nsis.zip` updater artifact and is not used here.

## Publication and update test

A successful local build does not establish that redistribution is ready.
Before publishing, provide and review the exact corresponding sources, build
inputs, and notices required for the pinned BtbN FFmpeg 9.0 GPL static build
(including its statically linked components) and the selected libmpv DLL.
Current evidence and remaining gaps are recorded in
`scripts/runtime-manifest.json`, `docs/THIRD_PARTY_SOURCE_OFFER.md`, and
`docs/dependency-license-audit.md`.
Do not publish until the required source and notices are assembled and
verified. Upload the installer, matching `.sig`, `latest.json`, and required
source and notice assets to the same release.

For updater testing, use the isolated procedure in
[`UPDATER_TESTING.md`](UPDATER_TESTING.md). Do not treat a locally generated
`latest.json` as an available feed unless its installer URL and signature are
reachable by the test installation.

If preparation fails before the version commit, `scripts/release.mjs` restores
the version files it changed. A later build or metadata failure preserves the
local version commit for inspection. Review that commit and the generated
files, fix the reported issue, and continue from a clean working tree.
