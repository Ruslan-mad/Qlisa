# Headphone preview transport

The Clip Editor headphone control is a local audition transport. It uses the
configured preview output and never creates a show cue or changes the show
Playhead.

The transport starts at the editor cursor. Audio and Video pass file time
directly to the preview builder, which applies the cue trim and playback rate
once. Number uses its shared Number clock cursor; the backend maps that clock
through master and child offsets, trim windows, playback rates, loops, disabled
state, and mute state.

The headphone button enables or stops the audition. The editor's Play/Pause
controls pause and resume the headphone session while keeping the silent video
or Number preview in sync. Audio also has a Play/Pause control in the editor
header. Enabling headphones while the visual editor is paused creates a paused
audio session; enabling it while the editor is playing starts audio immediately.
Frame stepping pauses the headphone voice before seeking. Timeline drags update
the local cursor; pointer release sends one seek
to the headphone session. A seek replaces the preview voice at that position
and retains the paused state. Every replacement uses the existing preview
generation guard, so a late decode cannot replace a newer request or undo a
pause/stop.

The preview session remains outside workspace cue state. Its audio is routed
through the configured preview output, excluded from the program mix and taps,
and stopped when its owning editor closes or its cue is removed.

## Verification limits

Frontend cursor selection, control wiring, store behavior, and generation
cleanup have focused Vitest coverage. The serial Rust test pass completed with
847 passed and 10 ignored, including preview command and audio-engine tests.
Physical output routing and manual UI playback were not tested.
