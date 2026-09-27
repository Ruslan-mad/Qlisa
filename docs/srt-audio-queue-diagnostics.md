# SRT audio queue diagnostics

The Windows SRT sender uses one FFmpeg process with separate raw video and
program-audio named pipes. The audio queue is a FIFO capped at 500 ms. Its
producer is the network output worker, which drains the post-master audio tap
every 10 ms. Its consumer writes interleaved stereo `f32le` blocks to FFmpeg.
Pipe writes can block while FFmpeg reads inputs, encodes AAC, or waits on its
SRT output.

When the queue overflows, the report includes the current and lifetime
high-water queue frames, queue duration, recent enqueue, queue-pop, and
successfully written pipe rates, cumulative dropped frames, and raw audio
sample-clock positions.
Those sample-clock positions count frames at the negotiated sample rate. They
are not FFmpeg packet timestamps, encoded output timestamps, or proof of remote
delivery. The log labels FFmpeg as alive only at the last sender health check.
The receiver's decoded FFmpeg progress and audio filter timestamps are the
end-to-end timing evidence.

An SRT listener without a caller can leave FFmpeg waiting at its output. In
that state, the pipe writer may block and the bounded queue will drop old
audio. The warning remains visible because those frames were lost. A connected
receiver test is required to decide whether a drop points to a sender defect.

The production FFmpeg process also emits mux progress to its existing stderr
reader. Periodic diagnostics include FFmpeg's output frame count, `out_time_us`,
progress state, and progress age. These values describe muxed stream progress.
They do not isolate audio PTS or confirm remote decoding. The loopback test
separately records decoded audio PTS from `ashowinfo` and decoded output time
from `-progress`.

## Connected loopback test (Windows)

Run the ignored test with the repository's staged FFmpeg runtime. It starts the
production `SrtFfmpegSender` in listener mode, starts a local FFmpeg caller,
submits 48 kHz stereo audio every 10 ms and 640x360 video at 25 fps, then checks
decoded video progress, decoded audio sample count, audio PTS, queue drops, and
queue depth. FFmpeg's receiver-side `-progress` and `ashowinfo` output provide
decoded timestamps; sender byte and sample counts are never treated as output
PTS. The test also retains the receiver's last 80 stderr lines. If either FFmpeg
process exits or a sender pipe fails, the test prints the receiver exit status,
stderr tail, and sender error before it asserts. This preserves the reason for
failure instead of stopping at a generic broken-pipe panic.

The harness keeps a live caller through FFmpeg input probing and stream
analysis. Lack of decoded progress for five seconds alone does not prove that
the SRT connection failed, so retries occur only after the caller process exits.
The overall startup deadline remains 20 seconds.

```powershell
$env:QLISA_SRT_AUDIO_TEST_SECS = '10' # Short smoke run; default is 1800 seconds.
cargo test --manifest-path src-tauri/Cargo.toml --features asio-support `
  srt_output_audio_remains_current_with_connected_ffmpeg_receiver -- --ignored --nocapture
```

Use the default 1800-second run for the stability check. The test chooses a
free loopback port and uses the staged runtime discovered by Qlisa. It does
not read or change application preferences.

## Interpreting results

- `audio_queue_frames` and `audio_queue_ms` show current backlog. The high-water
  counter records the largest observed backlog.
- `audio_pipe_write_current_blocked_ms` reports an in-progress pipe write. The
  accumulated blocked time reports completed writes.
- Enqueue, queue-pop, and pipe-write rates cover the latest report interval.
  They distinguish slow production, queue service, and a blocked OS pipe.
  Pipe-write counts advance only after `write_all` completes.
- `audio_dropped_frames` is cumulative data loss. A nonzero value remains a
  warning even when no receiver is connected.
- Sender sample-clock positions are local raw-input counts. Compare them with
  receiver output PTS and decoded audio time before attributing delay to AAC,
  FFmpeg muxing, or SRT.
- A stable connected run should sustain near 48,000 enqueued and pipe-written
  frames per second, decoded audio PTS and output progress should track wall
  time, and the queue should have zero drops.
