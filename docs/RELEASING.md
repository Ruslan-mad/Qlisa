# Windows release preparation

`scripts/publish.ps1` prepares a signed Windows release in the local working
tree. It commits the synchronized version files, but does not create a tag,
push, or publish a GitHub Release. The
updater release is **not ready** until the FFmpeg and libmpv corresponding
source archives and notices pass the compliance checks below, and a signed
release completes an end-to-end update test.

## Prerequisites

- Windows, Git, Rust/Cargo, Node.js, and pnpm.
- `pnpm.cmd` or `pnpm` and `node.exe` available in the current PowerShell
  `PATH`. If PowerShell reports that either command is missing, install the
  supported Node.js and pnpm versions, reopen PowerShell, and check with
  `Get-Command node,pnpm`.
- A clean `main` branch with all four project version fields in sync.
- A Qlisa updater public key in `src-tauri/tauri.conf.json`, updater artifacts
  enabled, and the HTTPS GitHub Releases endpoint configured.
- The private signing key at `%USERPROFILE%\.tauri\qlisa.key` by default, or
  pass its external path with `-SigningKeyPath`. The script never creates,
  reads for display, or prints the private key. It prompts for the key password
  with a masked PowerShell prompt. You can instead set
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` in the current shell before running it;
  never pass the password as a command argument.
- The FFmpeg and libmpv runtime files and the reviewed compliance manifest
  described below.
- 7-Zip (`7z.exe`) to inspect both generated NSIS archives for NDI Runtime
  files, and Minisign to cryptographically verify the Tauri updater signature.
  Both commands must be available in `PATH`.
- `docs/RELEASE_NOTES_X.Y.Z.md` for the requested version.

Do not generate or replace the signing key as part of release preparation.
Keep an encrypted backup outside the repository. Losing this key prevents
updates for users who trust the configured public key.

## Runtime compliance manifest

Keep a local `.release-compliance.json` at the repository root. It is ignored
by Git. It must identify the exact runtime files, corresponding source
archives, and notices for FFmpeg and libmpv. The script checks each file's
SHA-256, requires a maintainer review record, and checks that runtime and notice
paths are configured for the Windows bundle. The source archives and notices
are included in the prepared release asset list.

Use this shape and replace all example values with reviewed files and hashes:
The mpv notice path below is only an example: that notice is not currently
present or configured in `tauri.windows.conf.json`. Add the verified notice
and bundle resource before a release. The script rejects a missing file or
resource mapping.

```json
{
  "schemaVersion": 1,
  "reviewedBy": "maintainer name",
  "reviewedAt": "2026-09-24",
  "components": [
    {
      "name": "ffmpeg",
      "version": "9.0.1",
      "sourceUrl": "https://vendor.example/path/to/corresponding-source.zip",
      "license": "GPL-3.0-or-later",
      "correspondingSource": true,
      "sourceArchive": {
        "path": "C:/release-sources/ffmpeg-corresponding-source.zip",
        "sha256": "64 hexadecimal characters",
        "includeInRelease": true
      },
      "runtimeFiles": [
        {
          "path": "src-tauri/vendor/ffmpeg/ffmpeg.exe",
          "bundlePath": "vendor/ffmpeg/ffmpeg.exe",
          "sha256": "64 hexadecimal characters"
        },
        {
          "path": "src-tauri/vendor/ffmpeg/ffprobe.exe",
          "bundlePath": "vendor/ffmpeg/ffprobe.exe",
          "sha256": "64 hexadecimal characters"
        }
      ],
      "notices": [
        {
          "path": "src-tauri/vendor/ffmpeg/LICENSE",
          "bundlePath": "vendor/ffmpeg/LICENSE",
          "sha256": "64 hexadecimal characters"
        },
        {
          "path": "src-tauri/vendor/ffmpeg/README-Gyan-build.txt",
          "bundlePath": "vendor/ffmpeg/README-Gyan-build.txt",
          "sha256": "64 hexadecimal characters"
        }
      ]
    },
    {
      "name": "libmpv",
      "version": "v0.41.0-458-g062f4bf04",
      "sourceUrl": "https://vendor.example/path/to/corresponding-source.zip",
      "license": "GPL-2.0-or-later",
      "correspondingSource": true,
      "sourceArchive": {
        "path": "C:/release-sources/libmpv-corresponding-source.zip",
        "sha256": "64 hexadecimal characters",
        "includeInRelease": true
      },
      "runtimeFiles": [
        {
          "path": "src-tauri/vendor/mpv/libmpv-2.dll",
          "bundlePath": "vendor/mpv/libmpv-2.dll",
          "sha256": "64 hexadecimal characters"
        }
      ],
      "notices": [
        {
          "path": "src-tauri/vendor/mpv/THIRD-PARTY-NOTICES.txt",
          "bundlePath": "vendor/mpv/THIRD-PARTY-NOTICES.txt",
          "sha256": "64 hexadecimal characters"
        }
      ]
    }
  ]
}
```

The manifest records a maintainer attestation. It does not establish that an
archive is legally complete by itself. Before marking `correspondingSource`
true, match the source archive, patches, build configuration, and all
third-party notices to the exact binaries being shipped. The FFmpeg archive
must cover the documented Gyan build and included libraries, including
libsrt. The libmpv archive must cover its exact dependency revisions and build
inputs. Do not prepare a binary release until that work is complete.

## Prepare a release

Create and review the release notes, then run from the repository root:

```powershell
.\scripts\publish.ps1 -Version 1.5.3
```

The script requires branch `main`, a clean tree, and a version greater than
the synchronized project version. It checks runtime and compliance inputs,
scans the source tree for NDI DLLs, runs frontend tests/build and Rust
metadata/check/test/format/clippy checks, and calls `scripts/release.mjs` to
update versions, commit them as `release: vX.Y.Z`, and build the production
NSIS installer plus Tauri updater artifact from that commit. If FFmpeg is not staged, it runs
`scripts/sync-network-runtime.ps1` after the signing and compliance gates pass.
Clippy warnings do not fail this check; the current source has existing
dead-code warnings. A non-zero Clippy exit still blocks preparation.

`-DryRun` checks the repository and configured release prerequisites without
running tests, changing versions, or building. It does not waive the
compliance-manifest requirement.

With Tauri 2 `createUpdaterArtifacts: true`, the NSIS installer is also the
updater artifact and has a matching `.exe.sig`. Qlisa's `latest.json` points to
the exact installer URL and stores the exact signature text. Tauri's
`v1Compatible` mode uses a different `.nsis.zip` updater artifact and is not
used here. Preparation decodes Tauri's base64 public-key and signature boxes
to temporary Minisign text files, verifies the installer signature, then
removes those temporary files. It inspects the NSIS installer with 7-Zip and
rejects it if it contains an NDI Runtime DLL.

The prepared files are under
`src-tauri/target/release/prepared/vX.Y.Z/`. Review the printed asset paths and
SHA-256 values, release notes, generated `latest.json`, installer, updater
bundle/signature, corresponding source archives, and notices. Version changes
are committed locally before the build. Review the version commit and prepared
files before publication.

## Publication and update test

Publication is a separate maintainer action. Before creating `vX.Y.Z` on
GitHub, review the prepared commit and every release asset. Upload the NSIS
installer and matching `.exe.sig`, `latest.json`,
corresponding source archives, and notices. Do not publish if any required
source or notice is missing. The updater feed must be the `latest.json` asset
from that same release.

After the first signed release, test an actual update from an older installed
version. Confirm detection, notes, signature acceptance, active-cue install
guard, installation, restart, and the new running version. Type checks and a
successful build do not establish updater readiness.

If preparation fails before the version commit, `scripts/release.mjs` restores
the version files it changed and unstages them. If Cargo metadata or commit
creation fails, it attempts the same rollback. Once the version commit exists,
a later build or metadata failure preserves that commit for inspection. Fix
the reported problem, inspect the commit and artifacts, then continue from a
clean working tree.
