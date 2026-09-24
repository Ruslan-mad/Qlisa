# Qlisa 1.5.5

## Windows installation and media runtime

- Windows NSIS installs Qlisa for all users under Program Files and may request
  administrator approval through UAC.
- The installer and updater package no longer contain FFmpeg, ffprobe, or
  libmpv binaries. Qlisa downloads the pinned media runtime from upstream on
  first launch, verifies the hashes, and stores runtime files under the
  current user's `%LOCALAPPDATA%\Qlisa\runtime\` directory.
- Runtime preparation starts automatically. If a download fails, Qlisa offers
  retry or close. Later launches check the installed runtime and repair it when
  required.
- The Qlisa updater updates the application. A runtime version change is
  applied automatically on the next launch.

The upstream pins, hashes, license links, and source evidence are recorded in
[`scripts/runtime-manifest.json`](../scripts/runtime-manifest.json). This
packaging design does not determine or remove legal obligations for upstream
media binaries.
