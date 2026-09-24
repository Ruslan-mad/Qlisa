# Windows local build and release

`scripts/publish.ps1` prepares a **local-only signed Windows build**. It does
not create a tag, push, or publish a GitHub Release. Its installer, `.sig`, and
`latest.json` are private local test artifacts. The installer and updater
package contain Qlisa, not FFmpeg, ffprobe, or libmpv binaries. Qlisa downloads
the pinned media archives directly from upstream when needed. This packaging
choice does not determine or remove legal obligations for those upstream
binaries.

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
- `scripts/runtime-manifest.json` with the pinned upstream archive URLs and
  SHA-256 values. The app embeds this manifest as runtime configuration. No
  local media binaries, staging directory, or `prepare-runtime.ps1` step is
  required for a release build.
- NSIS per-machine install mode and updater `passive` install mode are checked
  separately. The former installs under Program Files and can trigger UAC.

The script checks that media binaries are absent from the Windows bundle
resource map. When 7-Zip is available, it also lists the completed installer
and rejects media runtime binaries in its payload. It scans the source tree
for NDI Runtime DLLs. It does not require 7-Zip, Minisign, source archives, or a local
`.release-compliance.json` for a local build. Tauri CLI 2 creates the updater
signature during the build when `bundle.createUpdaterArtifacts` is `true`.

Keep an encrypted backup of the private signing key outside the repository.
Losing this key prevents updates for users who trust the configured public
key.

## Prepare a local build

Create and review the release notes, then run from the repository root:

```powershell
.\scripts\publish.ps1 -Version 1.5.5
```

The script checks the repository, runtime manifest pins, and signing setup. It runs
frontend tests/build and Rust metadata/check/test/clippy checks. Then
`scripts/release.mjs` updates and commits the local version files and builds
the NSIS installer and Tauri updater artifact. The script creates the
installer, its `.sig`, and `latest.json` under
`src-tauri/target/release/prepared/vX.Y.Z/`. It prints the artifact paths and
SHA-256 values. It does not tag, push, or publish anything.

`-DryRun` checks prerequisites without changing
version files or building artifacts:

```powershell
.\scripts\publish.ps1 -Version 1.5.5 -DryRun
```

Clippy warnings do not fail this check; a non-zero Clippy exit does. The
pipeline does not run repository-wide `cargo fmt --check` because the current
baseline has unformatted files outside this release change.

With Tauri 2 `createUpdaterArtifacts: true`, the Windows NSIS installer is the
updater artifact and has a matching `.exe.sig`. Qlisa's `latest.json` stores
the exact signature text and installer URL. `v1Compatible` mode uses a
different `.nsis.zip` updater artifact and is not used here.

## Publication and update test

A successful local build does not establish legal compliance. Qlisa packages
do not host the media binaries, but the application downloads them directly
from upstream. Do not treat that choice as proof that legal obligations do not
apply. Current pin evidence and source gaps are recorded in
`scripts/runtime-manifest.json`, `docs/THIRD_PARTY_SOURCE_OFFER.md`, and
`docs/dependency-license-audit.md`. Review applicable obligations before
publication. Upload the installer, matching `.sig`, and `latest.json`; do not
upload media runtime binaries as Qlisa release assets.

For updater testing, use the isolated procedure in
[`UPDATER_TESTING.md`](UPDATER_TESTING.md). Do not treat a locally generated
`latest.json` as an available feed unless its installer URL and signature are
reachable by the test installation.

If preparation fails before the version commit, `scripts/release.mjs` restores
the version files it changed. A later build or metadata failure preserves the
local version commit for inspection. Review that commit and the generated
files, fix the reported issue, and continue from a clean working tree.
