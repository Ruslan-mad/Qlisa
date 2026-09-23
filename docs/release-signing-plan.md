# Qlisa updater signing plan

This is a plan, not an enabled release pipeline. The in-app updater remains
disabled until Qlisa has a signing key, updater configuration, and a tested
release process. Do not reuse Inkue keys or update metadata.

## Create and protect the key

Generate the key only on the release maintainer's trusted Windows machine.
Keep the private key outside the repository, restrict access, and store a
separate encrypted backup. Never paste the private key into source control,
issues, logs, or chat. Losing it prevents signing updates for existing users.

From the repository root in PowerShell:

```powershell
$qlisaSigningKey = Join-Path $env:USERPROFILE '.tauri\qlisa.key'
pnpm tauri signer generate -w $qlisaSigningKey
```

This follows the official [Tauri updater signing instructions](https://v2.tauri.app/plugin/updater/).
The command writes the private key and a `.pub` public key. Store the private
key and its backup securely. After review, copy only the public key into the
Tauri updater `pubkey` setting. The public key is not a secret.

## Configure and prepare a release

Only after the key is created and protected:

1. Configure the Tauri updater with the Qlisa public key, the HTTPS endpoint
   `https://github.com/Ruslan-mad/Qlisa/releases/latest/download/latest.json`,
   and `createUpdaterArtifacts: true`.
2. Build the Windows NSIS installer with the private key available through
   Tauri's signing environment variables. Do not place key contents in command
   history or repository files.
3. Prepare the signed NSIS update asset, its generated `.sig`, and `latest.json`.
   The metadata must contain the exact signature text and the HTTPS asset URL.
   Verify the signature with the configured public key before any release.
4. Review the installer and metadata locally. Publishing remains a separate,
   explicit release action; key setup alone does not enable the updater.

## Manual installation safety

Until an updater release is available, install any future release manually.
Before starting the installer, confirm that no cue is active or paused, stop the
show, save the workspace, and close Qlisa. Qlisa does not auto-install updates;
do not add unattended or automatic installation to the updater flow.

The signing key, updater endpoint, bundle configuration, asset names, and
release procedure still require implementation and validation. This document
does not generate keys, enable updater artifacts, build an installer, or publish
files.
