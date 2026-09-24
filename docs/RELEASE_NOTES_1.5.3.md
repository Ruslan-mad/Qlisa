# Qlisa 1.5.3

## What's new

- Added signed update checks for installed Qlisa builds. Qlisa checks for updates shortly after startup. You can also check from **Help → Check for updates** or the **About** dialog.
- The update dialog shows release notes and download progress when an update is available.

## Improvements

- The update dialog reports when Qlisa is up to date and provides retry actions when a check or installation fails.
- If a cue is running or paused, or the workspace has unsaved changes, Qlisa asks you to stop the cues or save or discard the changes before installing an update.

## Fixes

- Qlisa checks cue playback and workspace changes again immediately before installation, including changes made while the update downloads.

## Known issues

- The updater is unavailable in browser-only local builds. Tauri builds check the signed Qlisa release feed; an update is available only when that feed contains a newer signed release.

## Requirements

- Windows 10 or Windows 11.
- WebView2 Runtime. Supported Windows versions normally include it; install Microsoft's Evergreen WebView2 Runtime if it is missing.
- A separately installed NDI Runtime for NDI input and output. Qlisa does not include or install the NDI Runtime.
