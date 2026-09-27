# Audio trim loop regression

## Defect and fix

`AudioCue` initializes a voice at `start_time` and sets `end_frame` from
`end_time` (or the source end for start-only trims). The PCM callback used to
wrap every ordinary loop to frame 0. This made the first pass start at the
trim start and later passes play audio before the trim.

`Voice` now stores `loop_start_frame`, with 0 as the default. `AudioCue` sets it
from its effective start frame. The PCM callback uses that frame when it wraps
an ordinary loop. Sliced playback keeps its own segment boundaries. Streaming
trim loops continue to use `StreamingAudioSource`'s existing bounded range.
For start-only loops, the worker handles physical EOF when decoder metadata
reports a slightly longer duration than the decoded samples. It sets the loop
end to the actual decoded frame count before starting the next pass; the mixer
reads the same atomic trim end as its loop boundary.

Trimmed stream refills also finish the decoded packet that crosses the four
second target. The demuxer has already consumed that packet, so stopping at the
target inside it discarded the packet tail; the next refill began at the next
packet and shifted PCM on later passes. The start-only 100-repeat fixture runs
past this four-second boundary. Other stream modes keep their existing target
stop behavior.

`engine::audio_engine::tests::audio_cue_go_wires_trim_ranges_for_wav_and_mp3_matrix`
drives production `AudioCue::load` and `AudioCue::go` for WAV and MP3 with no
trim, end-only, start-only, both bounds, and slices. It checks the configured
source range and slice program.

`engine::audio_engine::tests::trimmed_wav_mp3_loops_keep_pcm_contiguous_for_100_repeats`
also drives production AudioCue GO, waits for source readiness, ticks the cue,
and consumes its actual engine command ring through the callback for start-only
and both-bound WAV/MP3 loops. It renders 100 repeats, compares each pass's first
sample with decoded PCM at the effective start, and checks silence, underruns,
and seam continuity. `pcm_loop_wraps_to_effective_trim_start` covers the
decoded PCM callback directly.

`engine::output_engine::slot::tests::installed_mpv_ab_loop_stays_inside_trim_for_finite_infinite_pause_and_seek`
uses the installed Windows libmpv and FFmpeg runtime. It generates a short test
video, checks finite and infinite trimmed loops, exercises pause, seek, and
resume, then checks 100 start-only repeats after loaded duration supplies the
A-B range. It uses production slot-loop helpers. This test is Windows-only and
ignored by default because it requires the installed runtime.

## Other cue types

`ImageCue` uses display duration and has no source trim or repeat window.
`MidiFileCue` plays its full MIDI file and has no trim range. `GroupCue` loops by
retriggering its children; `NumberCue` schedules child actions on its master
timeline. These cues do not discard a media start bound during source looping.
`VideoCue` owns the ordinary video trim window; the slot now uses A-B bounds for
trimmed repeats and keeps slice loops on their existing path.

## Verification and limits

Verified on 2026-09-27. The serial Rust suite passed 816 tests; 9 were ignored.
The focused audio-engine suite passed 75 tests; 1 was ignored. Each WAV and MP3
100-repeat callback case rendered 101 passes, including both-bound and
start-only trims. All reported zero underruns, silent frames, and rendered zero
frames; each pass's first sample matched the expected trim start, and measured
pass difference was 0.

The installed Windows libmpv test passed in 20.69 seconds. It verified two
finite repeats, infinite looping, pause, seek, resume, and 100 start-only
repeats. The test remains ignored by default and requires the installed Qlisa
runtime under `%LOCALAPPDATA%\Qlisa\runtime`.

The debug application also completed an actual 30-minute transport run on
2026-09-27, from 19:31:46.190 to 20:01:46.459 (1,800.269 seconds). It used the
ASIO-enabled build with the virtual CABLE-A device at 48 kHz, a trimmed VideoCue
(15–45 seconds, infinite repeat), infinite WAV trim (2–28 seconds), full-file
MP3, display output, and a local SRT receiver. An automated OSC script issued
91 audio GOs, 31 video GOs, 90 seeks, and pause/resume and stop/start actions.
The run recorded zero audio underruns and no warning or error logs. This was a
transport/stability run; it did not verify 100 physical repeats of a trimmed
cue. The 100-repeat PCM tests and headless mpv test above provide those range
checks separately.

Separate manual playback checks used one WAV cue and one MP3 cue: three seeks
on the WAV cue and two on the MP3 cue while audio was playing. The playhead
moved after each seek, with no yellow underrun indicator.

No physical Windows ASIO hardware, speaker-level listening check, or packaged
installer was tested. The app run used a virtual audio device. The results
validate callback rendering, headless libmpv, and the debug app transport under
that device; they do not establish audio quality on physical hardware or
behavior of the packaged release.
