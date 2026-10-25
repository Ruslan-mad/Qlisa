# Windows local build and release

`scripts/publish.ps1` builds and signs the full and compact Windows NSIS
installers and creates local `latest.json` metadata. It updates and commits
the version files, but it does not tag, push, or publish a GitHub Release.
Publication is a separate step after review.

Automatic GitHub CI runs are disabled. Maintainers can start the available
checks manually from GitHub Actions. A local release build and the GitHub
Actions workflow are separate checks.

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
- A maintainer may use the local signing wrapper at
  `%USERPROFILE%\.tauri\Invoke-QlisaReleaseSigning.ps1`. It reads a
  CurrentUser-protected credential and exposes signing variables only to the
  wrapped process, then restores the previous environment. Check that the
  wrapper exists before using it. Do not copy its credential store into the
  repository or pass the password on a command line.
- `scripts/runtime-manifest.json` and `scripts/full-installer-manifest.json`
  with pinned archive URLs, file hashes, and installer hashes. The full
  installer needs matching local FFmpeg, ffprobe, libmpv, and Vulkan inputs.
  `scripts/prepare-full-installer.ps1` validates and stages them under ignored
  `src-tauri/target/`. It uses a matching local runtime cache or `src-tauri/vendor/`
  when available. The NDI redistributable is downloaded from its pinned
  official URL if no local path is supplied.
- NSIS per-machine install mode and updater `passive` install mode are checked
  separately. The former installs under Program Files and can trigger UAC.

The script validates separate compact and full Tauri configurations. The
compact updater must not include media, Vulkan, or NDI payloads. The full
installer must include the pinned runtime seed, official NDI redistributable,
and offline WebView2 mode. It checks hashes and Authenticode signatures for
staged official installers. When 7-Zip is available, it also lists both final
NSIS packages to check their payload split. Tauri CLI 2 creates a `.sig` for
each installer when `bundle.createUpdaterArtifacts` is `true`.

Keep an encrypted backup of the private signing key outside the repository.
Losing this key prevents updates for users who trust the configured public
key.

## Prepare a local build

Create and review the release notes for the target version. Replace the
`<target-version>` placeholder below with that version, then run from the
repository root:

```powershell
.\scripts\publish.ps1 -Version "<target-version>"
```

The script checks the repository, runtime manifest pins, and signing setup. It runs
frontend, release-pipeline, and Rust checks. It stages the full installer
inputs, builds the Windows ASIO executable once, then packages it with compact
and full NSIS configurations. `scripts/release.mjs` updates and commits the
local version files before the build.

The output directory contains `Qlisa_X.Y.Z_x64-full-setup.exe` and its `.sig`,
plus `Qlisa_X.Y.Z_x64-update-setup.exe` and its `.sig`. The full installer
contains the runtime seed, NDI redistributable, and offline WebView2 installer.
The compact updater contains none of those payloads and removes the full
payload files left by a prior full install. `latest.json` points to the compact
updater and its exact signature. Artifacts and metadata are under
`src-tauri/target/release/`; the script prints their paths and SHA-256 values.
It does not tag, push, or publish anything.

When the local signing wrapper is present, invoke the same pipeline through it:

```powershell
$signingWrapper = Join-Path $env:USERPROFILE '.tauri\Invoke-QlisaReleaseSigning.ps1'
if (-not (Test-Path -LiteralPath $signingWrapper -PathType Leaf)) {
    throw 'Local signing wrapper is missing.'
}
& $signingWrapper -Action { .\scripts\publish.ps1 -Version "<target-version>" }
```

The wrapper uses Windows DPAPI with the current user's profile. It is a local
credential helper, not a repository file. Keep the signing key and protected
credential outside the repository. The normal masked prompt remains available
when the wrapper is not used.

`-DryRun` checks prerequisites without changing
version files or building artifacts:

```powershell
.\scripts\publish.ps1 -Version "<target-version>" -DryRun
```

Clippy warnings do not fail this check; a non-zero Clippy exit does. The
pipeline does not run repository-wide `cargo fmt --check` because the current
baseline has unformatted files outside this release change.

With Tauri 2 `createUpdaterArtifacts: true`, both Windows NSIS variants have a
matching `.exe.sig`. Qlisa's `latest.json` stores the compact updater's exact
signature text and URL. `v1Compatible` mode uses a different `.nsis.zip`
updater artifact and is not used here.

## Publication and update test

A successful local build does not establish legal compliance. The full
installer includes selected media binaries and the NDI redistributable; the
compact updater does not. Do not treat package separation as proof that legal
obligations do not apply. Current pin evidence and source gaps are recorded in
`scripts/runtime-manifest.json`, `docs/THIRD_PARTY_SOURCE_OFFER.md`, and
`docs/dependency-license-audit.md`. Review applicable obligations before
publication. Upload both installer variants, their matching `.sig` files, and
`latest.json`. Do not upload standalone runtime files as separate release
assets.

For updater testing, use the isolated procedure in
[`UPDATER_TESTING.md`](UPDATER_TESTING.md). Do not treat a locally generated
`latest.json` as an available feed unless its installer URL and signature are
reachable by the test installation.

If preparation fails before the version commit, `scripts/release.mjs` restores
the version files it changed. A later build or metadata failure preserves the
local version commit for inspection. Review that commit and the generated
files, fix the reported issue, and continue from a clean working tree.
