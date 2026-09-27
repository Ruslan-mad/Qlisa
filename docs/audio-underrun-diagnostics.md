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

## Real-device reproduction

The prepared disposable profile and project are under
`%TEMP%\QlisaAudioUnderrun-20260927-1533`. The profile copies the machine audio
configuration and retains only local display output routes. OSC listens on
`127.0.0.1:53001`. NDI and SRT output routes are removed in this copy. The
user profile and `LOCALAPPDATA` are unchanged.

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
3. Save the app diagnostic log and timestamped OSC send log. Record the actual
   output device and format in the test notes; do not share the full audio
   preferences file.

The fixtures are local, so this run does not depend on external media sources.
The OSC log records send times, not when the app handled each command. The
`streaming_audio_loop_wall_clock_30m` ignored test uses a software-rendered
callback and decoder. It helps check loop/refill behavior but does not prove
speaker output. A physical run still needs the debug app and real output
device.

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
matching run reproduces it.
