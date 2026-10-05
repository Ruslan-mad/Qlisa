# Number timeline

Number action tracks use each Audio or Video cue's effective source range. A
finite `loop_count` stores extra repeats: zero plays once, and a value of `N`
plays the source range `N + 1` times. The Time tab configures this count. The
visible track ends after those passes or at the Number duration, whichever
comes first. Infinite loops fill the remaining Number timeline.

When the Number master is Audio or Video, its finite repeats also set the
Number timeline duration. For example, a 100-second trimmed range with two
extra repeats produces a 300-second Number timeline.

The canvas repeats the existing full-file waveform or filmstrip at the source
range duration. Waveform bins use absolute source time against the full cached
file duration. This keeps end trims from stretching silent file tails into the
visible crop. Trim changes only the source bounds and pass duration; it does
not require another waveform decode. A short final pass maps to the matching
prefix of the source crop. The Number playhead remains on the shared Number
clock, so it advances across repeated pass boundaries.
