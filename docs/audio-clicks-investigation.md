# Intermittent clicks during audio and video playback

The Qlisa 1.5.14 release source (`6de2d3a`) and the current checkout use the
same streaming decoder for Audio Cues and video audio voices. In that release, an
ordinary refill stops partway through a decoded packet when the PCM ring reaches
its four-second target. The demuxer has already advanced past that packet. The
next refill starts with the following packet, so the unqueued tail is lost.
This creates a discontinuity within a playing file about once per refill cycle.
It does not require a ring underrun, device error, or loop boundary.

`StreamingAudioSource` has a ten-second ring and normally requests more PCM
when it falls below two seconds. The fix finishes the packet already decoded,
then lets the outer loop stop before decoding the next packet. The ring remains
bounded; a refill can exceed the four-second target by at most the remaining
frames in one decoder packet. The callback, routing, gain, and output level are
unchanged.

The regression test generates nonperiodic PCM WAV files in mono at 8,003 Hz
and stereo at 11,027 Hz. Both target watermarks split a packet. It runs at
least four separate refill jobs and compares every frame read from the ring
with a complete decode of the same file. This catches skipped frames that a
periodic test signal could conceal.

This code defect is a credible cause of the reported faint clicks in both
media types. It has not been tied to a recorded click from the event device.
After a build containing the fix, repeat the affected cues on that device and
check the existing diagnostics for underruns, output stream errors, and any
remaining clicks. Driver or hardware timing faults remain possible if clicks
persist. Loop-boundary behavior and stereo pan gain are separate issues and
were not changed here.
