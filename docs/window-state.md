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

The placement reducer has Rust tests for monitor clamping and preserving the
normal rectangle through maximize/minimize events. Native close/relaunch and
maximized restore checks are still required; updater restart behavior and
multi-monitor hardware layouts have not been verified here.
