# Audio underrun diagnostics

The cpal callback reports streaming underruns through the existing bounded
status ring. It does not format text, log, lock, or perform I/O. The show event
loop formats reports after it drains that ring.

Each report records PCM fill at callback-block entry and when the report is
formed, ring capacity, silent output frames and milliseconds, source/output
rates, playback frame, stream ID, seek and decoder-session generations,
readiness/EOF/playback state, refill request and worker state, decoder-pool
queue/active counts, refill age and last queue wait, and loop-boundary
proximity. The event loop retains the last 32 audio Play/Stop/Pause/Resume/Seek
markers from the prior ten seconds. It attaches cue IDs, names, and numbers
when markers arrive, outside the callback. Later markers for a voice inherit
its last known cue owner. A first `resume` marker with no earlier Play/Resume
marker for that voice is rendered as `start`; this represents a held startup
voice becoming audible after its buffer is ready.

Decoder session state is `0` when no session was published, `1` when a session
is open, and `2` when opening failed or the file has no audio track. EOF is a
separate boolean. Playback state is `0` paused, `1` preload, and `2` playing.
A `ready=false` value at a failed read marks a cold-start or seek refill
window; it does not prove why the window occurred. The reports from 17:40:46
and 17:41:22 do not identify a common cause. They came from different GO runs,
had 10 ms and 20 ms of silence, and were not near a loop boundary.

The later cold-start reproduction captured three underruns before the readiness
gate was added:

- At `18:45:53.920`, 480 output frames (10 ms) were silent. GO was sent at
  `18:45:53.889`. The stream held 35,059 frames in a 480,000-frame ring,
  `ready=false`, and playback had already been published. The source had not
  reached its existing 36,000-frame ready watermark.
- At `18:46:13.936`, another GO produced 480 silent frames (10 ms) with
  `ready=false`.
- At `18:47:13.945`, the stream had session state `0`, a queued refill job,
  and no active job when the underrun was reported.

These observations identify the reproduced startup failure: GO published a
streaming voice as playing before the source reached readiness. Audio Cue now
holds that voice paused and resumes it from the normal cue tick after readiness.
The logs do not establish a cause for the earlier 17:40 reports.

## Live-app reproduction

The prepared disposable profile and project are under
`%TEMP%\QlisaAudioUnderrun-20260927-1533`. The profile copies the machine audio
configuration and preferences. The original isolated setup retained only local
display output routes and removed NDI/SRT routes. The current combined scenario
copy retains Display output and adds a local SRT listener on port `55082` with a
local receiver. OSC listens on `127.0.0.1:53001`. These are disposable test
copies; the user profile and `LOCALAPPDATA` are unchanged. Use the isolated
variant to test audio and display without network output. Use the combined
variant to exercise video, audio, Display, and local SRT together.

1. Start the debug app from the repository PowerShell session:

   ```powershell
   .\scripts\diagnostics\Launch-AudioUnderrunProfile.ps1 `
     -ProfileRoot "$env:TEMP\QlisaAudioUnderrun-20260927-1533"
   ```

   This opens
   `%TEMP%\QlisaAudioUnderrun-20260927-1533\Show\AudioUnderrun.qlisa` with
   `%TEMP%\QlisaAudioUnderrun-20260927-1533\AppData\Roaming` as `APPDATA`.
   It sets `QLISA_FFMPEG_PATH` to
   `%LOCALAPPDATA%\Qlisa\runtime\ffmpeg.exe` for the app process. It leaves
   `LOCALAPPDATA` at its existing value. Do not start a
   second app instance: the single-instance handler can forward a project to
   the already-running process, which would keep its original profile.
2. After the project has loaded and OSC responds, immediately start the cold
   GO sample, then leave the script running for 30 minutes:

   ```powershell
   .\scripts\diagnostics\Invoke-AudioUnderrunRepro.ps1 `
     -AudioCueNumber 2 -SecondAudioCueNumber 3 -VideoCueNumber 1 -Minutes 30
   ```

   Cue 1 is a local video with audio, trimmed to 25–75% and set to loop
   indefinitely. Cue 2 is a quiet WAV loop trimmed to 2–28 seconds. Cue 3 is a
   quiet ordinary MP3 cue. The files are 60, 30, and 32 seconds long and have
   about −30 dBFS signal level. The script starts video, then immediately sends
   the first Audio Cue GO. It cycles pause, seek to 4 seconds, resume, stop,
   and GO on the other Audio Cue. It restarts video every 50 seconds so the
   video fixture does not finish. Use `-SecondAudioCueNumber ''` to repeat only
   cue 2.
3. Save the app diagnostic log and timestamped OSC send log. For the combined
   variant, also save the SRT receiver log. Record the actual output device and
   format in the test notes; do not share the full audio preferences file.

The fixtures are local, so this run does not depend on external media sources.
The OSC log records send times, not when the app handled each command. The
`streaming_audio_loop_wall_clock_30m` ignored test uses a software-rendered
callback and decoder. It helps check loop/refill behavior but does not prove
speaker output. A physical run still needs the debug app and real output
device.

### Completed combined run (2026-09-27)

The debug app ran from `19:31:46.190` to `20:01:46.459` Moscow time, for
`1,800.269` seconds. The script sent 91 Audio Cue GO commands, 31 Video Cue GO
commands, 90 seeks, and repeated Audio Cue stop, pause, and resume actions. The
app log contained zero `audio::underrun` events and zero WARN/ERROR entries in
the run interval, so the app reported zero silent output frames from streaming
underruns.

The debug build had its ASIO feature enabled, but the selected cpal device used
the WASAPI shared backend and was the virtual Windows `CABLE-A` device at
48 kHz. This validates the live app and cpal output path under the combined
workload. It does not establish audible output from physical speakers or an
ASIO hardware device.

Across samples after five minutes, app CPU ranged from 3.289% to 3.966% of 20
logical processors; working set ranged from 722.63 MB to 746.85 MB, and private
bytes from 1,926.73 MB to 1,981.26 MB. The monitored app children, FFmpeg
encoder, and FFmpeg receiver used 9.656% to 13.064% CPU and 1,970.5 MB to
2,004.45 MB working set. The OSC PowerShell process was not included.
These measurements remained stable during the sampled interval.

After the run, manual UI checks performed three seeks in one WAV cue and two
seeks in one MP3 cue while playback was active. The playheads moved, no yellow
warning appeared, and the app logged no audio warning. One WAV seek was also
performed during the long run. Existing underrun tests still assert that a
genuine unready/empty source reports an underrun; this run did not weaken that
warning condition.

## Startup readiness

Initial streaming GO uses the existing 750 ms ready watermark. Audio Cue
submits a not-yet-ready streaming voice in the existing paused state. Its
30 Hz cue tick resumes the voice when the source reports ready; the callback
never waits. Action and continue clocks start at that point. The held interval
does not count toward a finite cue's action duration, and Pause/Resume leaves
the voice held until readiness. PCM and already-ready streams keep the direct
start path. The change does not alter ring capacity, trim, loop, seek, waveform,
or callback behavior. The `ready`, fill, and refill-job fields still describe
any underrun that occurs after playback begins; assign a cause only after a
matching run reproduces it. The cold-start publication race above has been
reproduced and addressed; this guidance applies to any later underrun cause.

## Captured event context and limits

Each underrun report includes cue ID/number/name when known, voice and stream
IDs, report time, source/output rates, silent frames and duration, ring fill at
callback block entry and report time, capacity, decoder session state and
generation, seek generation, readiness and EOF, playback frame/state, refill
request and job queued/active flags, worker-pool pending/active counts, last
refill age and queue wait, and loop-boundary proximity. The callback stores
bounded event data only. The show event loop adds cue labels and formats logs.

The event loop retains up to 32 Play/Stop/Pause/Resume/Seek markers from the
prior ten seconds. Each marker includes its age, voice, command, frame, and cue
label when known. This covers nearby transport actions on the underrunning voice
and other cues. A first Resume with no earlier Play/Resume marker for that voice
is labeled `start`, because it can be the readiness release of a held startup
voice. This context is bounded; actions older than ten seconds or markers lost
to a full ring are not guaranteed to appear. An OSC send timestamp alone does
not prove when the app handled the action.
