# Main window placement

The main window stores its last normal position and client size, plus its
maximized state, in the per-user machine configuration file
`Inkue/main-window-state.json`. This file is separate from project files and
global preferences. Output, Preferences, Mixer, and other secondary windows do
not use this state.

The backend records physical screen coordinates and the client-area size. It
does not replace the saved normal rectangle while the main window is
maximized or minimized. On startup it applies the normal rectangle, then
restores maximized state. If the saved position no longer intersects an
available monitor, it places the window in the primary monitor's work area.
Invalid state falls back to the configured startup window size.

The frontend does not normalize the main window on mount. That old behavior
forced every launch back to a windowed state and would have undone restoration.

## Verification

- Three focused Rust tests pass for monitor clamping, invalid state, and
  preserving the normal rectangle through maximize/minimize events.
- `pnpm tauri:check` passes.
- On Windows, moving and resizing the normal window, then closing and relaunching
  restored its position and size. Maximizing, closing, and relaunching restored
  the maximized state; restoring to normal returned to the saved normal
  rectangle. Repeated close/relaunch checks showed no size drift.

The physical monitor-disconnect and mixed-DPI cases were not tested on hardware.
Updater-specific restart behavior was also not tested. The invalid and
off-screen placement cases are covered by pure geometry tests.
