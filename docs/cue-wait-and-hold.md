# Cue wait progress and held video frames

Cue rows show Pre-Wait and Post-Wait progress from the existing `cue-time-update` event. The event identifies the active wait phase and carries elapsed and total wait time. The UI fills only the matching column. Paused cues keep their last published progress; pending Auto-Follow and Group child post-waits continue to use their existing monotonic deadlines.

Auto-Continue uses its own elapsed clock. It does not use the seekable action clock. A wait-ending snapshot clears the row when a pending continuation expires or is canceled.

Number publishes its master post-wait from its existing completion deadline. If the master has a fade-out, the progress remains at zero through the fade prefix and fills during the authored post-wait. The same Number timing snapshot receives the wait fields, so its elapsed, action, media, and remaining values stay intact. A paused Number reads from its frozen pause timestamp.

With Hold Last Frame enabled, mpv can keep a video slot open after media EOF. The slot still owns the displayed frame, but `eof-reached` marks playback as complete. Cue completion preserves the voice handle for Stop and cleanup while clearing the cue's transport state. A same-cue retrigger releases its old held layer when the next video action starts. Explicit stop and reset paths still release the retained output.

The row labels use the localized Auto-Continue and Auto-Follow names. Group and Number rows use double-click to expand or collapse; inline editors and controls keep their own double-click behavior.
