# Number timeline

Number action tracks use each Audio or Video cue's effective source range. A
finite `loop_count` stores extra repeats: zero plays once, and a value of `N`
plays the source range `N + 1` times. The visible track ends after those passes
or at the Number duration, whichever comes first. Infinite loops fill the
remaining Number timeline.

When the Number master is Audio or Video, its finite repeats also set the
Number timeline duration. For example, a 100-second trimmed range with two
extra repeats produces a 300-second Number timeline.

The canvas repeats the existing full-file waveform or filmstrip at the source
range duration. Trim changes only the source bounds and pass duration; it does
not require another waveform decode. The Number playhead remains on the shared
Number clock, so it advances across repeated pass boundaries.
