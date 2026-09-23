# Qlisa 1.5.2

## Changes

- Moved Active Cues into the main window's shared right sidebar alongside the
  Inspector. The panel lists running and paused cues, including nested Group
  children, and provides per-cue pause, resume, and stop controls where
  supported.
- Added elapsed time, finite-duration progress, and remaining time to Active
  Cues. Indefinite cues do not show progress or remaining time.
- Included `ffprobe.exe` in the Windows network runtime sync alongside
  `ffmpeg.exe`.

## Checks

- `pnpm test`: 389 passed.
- `pnpm exec tsc --noEmit`: passed.
- `pnpm build`: passed.
- `cargo check --tests`: passed.
- `pnpm tauri:check`: passed.
- The Rust test executable exits with `0xc0000139` before tests start. Rust
  tests did not run successfully.
