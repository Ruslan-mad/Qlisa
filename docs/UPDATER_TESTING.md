# Updater testing

Qlisa uses the Tauri updater to update the application. The media runtime is independent: `ffmpeg.exe`, `ffprobe.exe`, and `libmpv-2.dll` are downloaded from pinned upstream URLs, hash-checked, and stored in `%LOCALAPPDATA%\Qlisa\runtime`. They are not bundled in the NSIS installer or updater artifact.

## Required local test

Before a public binary release, run the signed local loopback test in [UPDATER_TESTING_LOCAL.md](UPDATER_TESTING_LOCAL.md). It tests version 1.5.5 → 1.5.6 with an isolated `QlisaUpdaterTest` identity and no GitHub Release. It must verify:

- NSIS installs under `C:\Program Files\QlisaUpdaterTest` with `installMode = perMachine` and UAC.
- The Tauri updater uses `installMode = passive`; this is separate from the NSIS install mode.
- The runtime downloads into LocalAppData and stays outside Program Files.
- The update accepts a valid signature, rejects an invalid signature, restarts the app, and preserves test settings and workspace data.
- Runtime files remain available after the Qlisa update, and the second launch does not download them again.

The previous local 1.5.3 → 1.5.4 test passed. The new 1.5.5 → 1.5.6 per-machine test is pending. Do not describe the updater as verified for this installation model until the new test passes.

## GitHub Release test

Prefer the loopback procedure. Use a temporary GitHub Release only if a real hosted-feed behavior cannot be checked locally and the release owner authorizes publication. Before publishing, show the proposed tag and exact assets. Never point the production updater endpoint at a test tag. Keep the temporary release marked prerelease and not latest; remove its release and tag after the test if repository policy permits.

Use the same pinned public updater key for both test builds. Do not publish the private signing key, change the production endpoint for a test, or include media runtime binaries in the installer or updater archive.
