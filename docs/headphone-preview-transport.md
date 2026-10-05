# Headphone preview transport

The Clip Editor has one preview transport for the visual preview and its
optional headphone sound. Play/Pause starts, pauses, and resumes that transport.
The headphone button only mutes or unmutes its sound. Muting does not stop the
visual preview or move its cursor. The preview uses the configured preview
output and never creates a show cue or changes the show Playhead.

The transport starts at the editor cursor. Audio preview uses file time and
respects the cue trim and playback rate. Video uses its source-file cursor.
Number uses its shared Number clock; the backend maps that clock through
master and child offsets, trim windows, playback rates, loops, disabled state,
and mute state.

Timeline drags update the visual cursor continuously. Pointer release sends the
final position to the audio preview. Frame stepping also seeks the audio preview
and leaves it paused until Play. A seek on a live standalone Audio or Video
preview reuses its voice and decoded stream; repeated seeks do not decode the
whole file or create a new voice for every position. If the voice has ended,
the transport ends. A seek then moves the cursor without starting audio; the
next Play starts a fresh preview at the selected position. Backend recovery
starts a fresh voice only when a command resumes or seeks an existing session
whose voice has already ended. Number seeks retain the Number mix behavior and
rebuild its sources.

The preview session remains outside workspace cue state. Its audio is routed
through the configured preview output, excluded from the program mix and taps,
and stopped when its owning editor closes or its cue is removed.

## Verification limits

The Rust preview library tests passed: 38 passed, including WAV/MP3 streaming
and repeated-seek coverage. Run the manual checks in
[the follow-up regression checklist](BUGFIXES_NEXT.md) for transport, mute,
repeated seek, paused seek, EOF, and Number behavior. Physical output routing
and manual UI playback require a configured audio device and have not been
confirmed by this document update.
