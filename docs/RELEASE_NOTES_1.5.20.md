# Qlisa 1.5.20

## Full installer and compact updates

Qlisa now provides two Windows installers:

- `Qlisa_1.5.20_x64-full-setup.exe` is for a new or offline installation. It
  includes hash-pinned FFmpeg, ffprobe, libmpv, the Vulkan loader, an offline
  WebView2 installer, and the official NDI 6.3.2 redistributable installer.
- `Qlisa_1.5.20_x64-update-setup.exe` is the compact updater. It does not
  include the media runtime or NDI redistributable. `latest.json` points to
  this package.

The full installer verifies the bundled media files before copying them to
`%LOCALAPPDATA%\Qlisa\runtime`. This supports first launch without network
access. A missing or damaged full-installer payload stops runtime preparation
with an error instead of silently switching to a download. The compact updater
preserves the per-user runtime cache and removes only the full-installer files
left in the application folder.

The full installer runs the signed NDI redistributable in its elevated setup
process when NDI 6.3.2 or newer is not already installed. It does not replace a
newer NDI 6 runtime, and it stops setup if the redistributable reports an
installation error. A reboot-required result is recorded without restarting
Windows.

Both packages are signed for the Tauri updater. The full installer is for
installation; the compact package is used by the automatic updater.
